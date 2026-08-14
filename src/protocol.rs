use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use adoc_core::DiagnosticSeverity as CoreDiagnosticSeverity;
use adoc_index::workspace_diagnostics;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::{
    notification::{
        DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
        Notification as LspNotification, PublishDiagnostics,
    },
    request::{DocumentSymbolRequest, GotoDefinition, Request as LspRequest},
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, InitializeParams, InitializeResult, Location,
    NumberOrString, PublishDiagnosticsParams, ServerInfo, SymbolKind,
    TextDocumentContentChangeEvent, Uri,
};

use crate::{
    capabilities::server_capabilities,
    handlers::{definition::definition_at_offset, document_symbols::document_symbols},
    position::PositionEncoding,
    server::ServerError,
    state::{document_path, ServerState},
};

pub fn run_connection(connection: &Connection) -> Result<(), ServerError> {
    let (initialize_id, initialize_value) = connection.initialize_start()?;
    let initialize_params: InitializeParams = serde_json::from_value(initialize_value)?;
    let encoding = PositionEncoding::negotiate(&initialize_params.capabilities);
    let mut server = ProtocolServer::new(encoding);
    let roots = workspace_roots(&initialize_params);
    if !roots.is_empty() {
        server.state.index_workspace(roots)?;
    }

    let result = InitializeResult {
        capabilities: server_capabilities(encoding),
        server_info: Some(ServerInfo {
            name: "adoc-ls".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    };
    connection.initialize_finish(initialize_id, serde_json::to_value(result)?)?;

    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                let response = server.handle_request(request);
                connection
                    .sender
                    .send(response.into())
                    .map_err(|_| ServerError::ChannelClosed)?;
            }
            Message::Notification(notification) => {
                if let Some(notification) = server.handle_notification(notification)? {
                    connection
                        .sender
                        .send(notification.into())
                        .map_err(|_| ServerError::ChannelClosed)?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

struct ProtocolServer {
    state: ServerState,
    encoding: PositionEncoding,
}

impl ProtocolServer {
    fn new(encoding: PositionEncoding) -> Self {
        Self {
            state: ServerState::default(),
            encoding,
        }
    }

    fn handle_request(&self, request: Request) -> Response {
        match request.method.as_str() {
            DocumentSymbolRequest::METHOD => self
                .request_response::<DocumentSymbolParams, _>(request, |params| {
                    self.document_symbol_response(params)
                }),
            GotoDefinition::METHOD => self
                .request_response::<GotoDefinitionParams, _>(request, |params| {
                    self.definition_response(params)
                }),
            _ => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request `{}`", request.method),
            ),
        }
    }

    fn request_response<P, R>(&self, request: Request, handler: impl FnOnce(P) -> R) -> Response
    where
        P: serde::de::DeserializeOwned,
        R: serde::Serialize,
    {
        match serde_json::from_value(request.params) {
            Ok(params) => Response::new_ok(request.id, handler(params)),
            Err(error) => Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            ),
        }
    }

    fn handle_notification(
        &mut self,
        notification: Notification,
    ) -> Result<Option<Notification>, ServerError> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Some(params) = decode_notification(notification.params) else {
                    return Ok(None);
                };
                self.did_open(params)
            }
            DidChangeTextDocument::METHOD => {
                let Some(params) = decode_notification(notification.params) else {
                    return Ok(None);
                };
                self.did_change(params)
            }
            DidCloseTextDocument::METHOD => {
                let Some(params) = decode_notification(notification.params) else {
                    return Ok(None);
                };
                self.did_close(params)
            }
            _ => Ok(None),
        }
    }

    fn did_open(
        &mut self,
        params: DidOpenTextDocumentParams,
    ) -> Result<Option<Notification>, ServerError> {
        let uri = params.text_document.uri.as_str().to_owned();
        self.state.open(
            &uri,
            &params.text_document.text,
            params.text_document.version,
        );
        Ok(self.diagnostics_notification(&params.text_document.uri))
    }

    fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Result<Option<Notification>, ServerError> {
        let uri = params.text_document.uri.as_str().to_owned();
        let Some(open_document) = self.state.documents.get(&uri) else {
            return Ok(None);
        };
        let text = apply_content_changes(
            open_document.document.text.clone(),
            params.content_changes,
            self.encoding,
        )?;
        self.state.change(&uri, &text, params.text_document.version);
        Ok(self.diagnostics_notification(&params.text_document.uri))
    }

    fn did_close(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> Result<Option<Notification>, ServerError> {
        let uri = params.text_document.uri.as_str();
        self.state.close(uri)?;
        Ok(Some(Notification::new(
            PublishDiagnostics::METHOD.to_owned(),
            PublishDiagnosticsParams::new(params.text_document.uri, Vec::new(), None),
        )))
    }

    fn diagnostics_notification(&self, uri: &Uri) -> Option<Notification> {
        let uri_text = uri.as_str();
        let open_document = self.state.documents.get(uri_text)?;
        let path = document_path(uri_text);
        let diagnostics = workspace_diagnostics(&self.state.index, &path)
            .into_iter()
            .filter_map(|diagnostic| {
                Some(Diagnostic {
                    range: self
                        .encoding
                        .range(&open_document.document.text, diagnostic.range)?,
                    severity: Some(match diagnostic.severity {
                        CoreDiagnosticSeverity::Information => DiagnosticSeverity::INFORMATION,
                        CoreDiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                        CoreDiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                    }),
                    code: Some(NumberOrString::String(diagnostic.code.as_str().to_owned())),
                    source: Some("adoc-ls".to_owned()),
                    message: diagnostic.message,
                    ..Diagnostic::default()
                })
            })
            .collect();
        Some(Notification::new(
            PublishDiagnostics::METHOD.to_owned(),
            PublishDiagnosticsParams::new(uri.clone(), diagnostics, Some(open_document.version)),
        ))
    }

    #[allow(deprecated)]
    fn document_symbol_response(
        &self,
        params: DocumentSymbolParams,
    ) -> Option<DocumentSymbolResponse> {
        let document = &self
            .state
            .documents
            .get(params.text_document.uri.as_str())?
            .document;
        let symbols = document_symbols(document)
            .into_iter()
            .filter_map(|symbol| {
                Some(DocumentSymbol {
                    name: symbol.name,
                    detail: None,
                    kind: if symbol.level == 0 {
                        SymbolKind::FILE
                    } else {
                        SymbolKind::NAMESPACE
                    },
                    tags: None,
                    deprecated: None,
                    range: self.encoding.range(&document.text, symbol.range)?,
                    selection_range: self
                        .encoding
                        .range(&document.text, symbol.selection_range)?,
                    children: None,
                })
            })
            .collect();
        Some(DocumentSymbolResponse::Nested(symbols))
    }

    fn definition_response(&self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let document_uri = params
            .text_document_position_params
            .text_document
            .uri
            .as_str();
        let document = &self.state.documents.get(document_uri)?.document;
        let offset = self.encoding.offset(
            &document.text,
            params.text_document_position_params.position,
        )?;
        let current_path = document_path(document_uri);
        let target = definition_at_offset(&self.state.index, &current_path, document, offset)?;
        let target_text = self
            .state
            .index
            .file(&target.path)
            .map(|file| file.document.text.clone())
            .or_else(|| fs::read_to_string(&target.path).ok())
            .unwrap_or_default();
        let uri = path_to_uri(&target.path)?;
        let range = self.encoding.range(&target_text, target.range)?;
        Some(GotoDefinitionResponse::Scalar(Location::new(uri, range)))
    }
}

fn decode_notification<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Option<T> {
    serde_json::from_value(value).ok()
}

fn apply_content_changes(
    mut text: String,
    changes: Vec<TextDocumentContentChangeEvent>,
    encoding: PositionEncoding,
) -> Result<String, ServerError> {
    for change in changes {
        if let Some(range) = change.range {
            let start = encoding
                .offset(&text, range.start)
                .ok_or_else(|| ServerError::InvalidChange("invalid change start".to_owned()))?;
            let end = encoding
                .offset(&text, range.end)
                .ok_or_else(|| ServerError::InvalidChange("invalid change end".to_owned()))?;
            if start > end {
                return Err(ServerError::InvalidChange(
                    "change start is after its end".to_owned(),
                ));
            }
            text.replace_range(start..end, &change.text);
        } else {
            text = change.text;
        }
    }
    Ok(text)
}

#[allow(deprecated)]
fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    if let Some(folders) = &params.workspace_folders {
        return folders
            .iter()
            .filter_map(|folder| uri_to_path(&folder.uri))
            .collect();
    }
    if let Some(uri) = &params.root_uri {
        return uri_to_path(uri).into_iter().collect();
    }
    params
        .root_path
        .as_deref()
        .map(PathBuf::from)
        .into_iter()
        .collect()
}

fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    url::Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

fn path_to_uri(path: &Path) -> Option<Uri> {
    Uri::from_str(url::Url::from_file_path(path).ok()?.as_str()).ok()
}

#[cfg(test)]
mod tests {
    use std::thread;

    use lsp_server::{Connection, Message, Notification, Request, RequestId};
    use lsp_types::{
        notification::{
            DidOpenTextDocument, Initialized, Notification as LspNotification, PublishDiagnostics,
        },
        request::{DocumentSymbolRequest, Initialize, Request as LspRequest, Shutdown},
        ClientCapabilities, DidOpenTextDocumentParams, DocumentSymbolParams, InitializeParams,
        InitializedParams, PartialResultParams, Position, Range, TextDocumentContentChangeEvent,
        TextDocumentIdentifier, TextDocumentItem, Uri, WorkDoneProgressParams,
    };

    use crate::position::PositionEncoding;

    use super::{apply_content_changes, run_connection};

    #[test]
    fn applies_incremental_utf16_changes() {
        let text = apply_content_changes(
            "A😀B\n".to_owned(),
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 1), Position::new(0, 3))),
                range_length: Some(2),
                text: "C".to_owned(),
            }],
            PositionEncoding::Utf16,
        )
        .unwrap();

        assert_eq!(text, "ACB\n");
    }

    #[test]
    fn serves_initialize_sync_diagnostics_symbols_and_shutdown() {
        let (server_connection, client_connection) = Connection::memory();
        let server = thread::spawn(move || run_connection(&server_connection));
        let uri: Uri = "file:///guide.adoc".parse().unwrap();

        client_connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                Initialize::METHOD.to_owned(),
                InitializeParams {
                    capabilities: ClientCapabilities::default(),
                    ..InitializeParams::default()
                },
            )))
            .unwrap();
        assert!(matches!(
            client_connection.receiver.recv().unwrap(),
            Message::Response(_)
        ));
        client_connection
            .sender
            .send(Message::Notification(Notification::new(
                Initialized::METHOD.to_owned(),
                InitializedParams {},
            )))
            .unwrap();

        client_connection
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.to_owned(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: uri.clone(),
                        language_id: "asciidoc".to_owned(),
                        version: 1,
                        text: "= Guide\n\nSee <<missing>>.\n".to_owned(),
                    },
                },
            )))
            .unwrap();
        let diagnostics = client_connection.receiver.recv().unwrap();
        assert!(matches!(
            diagnostics,
            Message::Notification(ref notification)
                if notification.method == PublishDiagnostics::METHOD
        ));

        client_connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                DocumentSymbolRequest::METHOD.to_owned(),
                DocumentSymbolParams {
                    text_document: TextDocumentIdentifier { uri },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                },
            )))
            .unwrap();
        assert!(matches!(
            client_connection.receiver.recv().unwrap(),
            Message::Response(_)
        ));

        client_connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(3),
                Shutdown::METHOD.to_owned(),
                (),
            )))
            .unwrap();
        assert!(matches!(
            client_connection.receiver.recv().unwrap(),
            Message::Response(_)
        ));
        client_connection
            .sender
            .send(Message::Notification(Notification::new(
                "exit".to_owned(),
                (),
            )))
            .unwrap();

        server.join().unwrap().unwrap();
    }
}
