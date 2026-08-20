use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use adoc_core::DiagnosticSeverity as CoreDiagnosticSeverity;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::{
    notification::{
        DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
        Notification as LspNotification, PublishDiagnostics,
    },
    request::{
        CodeActionRequest, Completion, DocumentSymbolRequest, ExecuteCommand, GotoDefinition,
        Request as LspRequest,
    },
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse, Command,
    CompletionItem, CompletionItemKind, CompletionList, CompletionParams, CompletionResponse,
    CompletionTextEdit, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, ExecuteCommandParams,
    GotoDefinitionParams, GotoDefinitionResponse, InitializeParams, InitializeResult, Location,
    NumberOrString, PublishDiagnosticsParams, ServerInfo, SymbolKind,
    TextDocumentContentChangeEvent, TextEdit, Uri,
};

use crate::{
    capabilities::server_capabilities,
    handlers::{
        code_actions::{
            code_actions_for, LivePreview, RENDER_LIVE_PREVIEW_COMMAND, RENDER_PREVIEW_COMMAND,
            STOP_LIVE_PREVIEW_COMMAND,
        },
        completion::{completion_at_offset, Candidate, CandidateKind},
        definition::definition_at_offset,
        diagnostics::diagnostics,
        document_symbols::document_symbols,
        execute_command::{refresh_preview, render_preview},
    },
    position::PositionEncoding,
    preview::{BrowserSink, PreviewMode, PreviewSink, SystemLauncher},
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
    renderer: Box<dyn adoc_render::Renderer>,
    sink: Box<dyn PreviewSink>,
    /// Documents with a live preview on screen, re-rendered when they are saved.
    live_previews: std::collections::BTreeSet<String>,
}

impl ProtocolServer {
    fn new(encoding: PositionEncoding) -> Self {
        Self {
            state: ServerState::default(),
            encoding,
            renderer: Box::new(adoc_render::SystemAsciidoctor::default()),
            sink: Box::new(BrowserSink::new(
                std::env::temp_dir().join("adoc-ls-preview"),
                SystemLauncher,
            )),
            live_previews: std::collections::BTreeSet::new(),
        }
    }

    fn handle_request(&mut self, request: Request) -> Response {
        match request.method.as_str() {
            DocumentSymbolRequest::METHOD => self
                .request_response::<DocumentSymbolParams, _>(request, |params| {
                    self.document_symbol_response(params)
                }),
            GotoDefinition::METHOD => self
                .request_response::<GotoDefinitionParams, _>(request, |params| {
                    self.definition_response(params)
                }),
            Completion::METHOD => self.request_response::<CompletionParams, _>(request, |params| {
                self.completion_response(params)
            }),
            CodeActionRequest::METHOD => self
                .request_response::<CodeActionParams, _>(request, |params| {
                    self.code_action_response(&params)
                }),
            ExecuteCommand::METHOD => self.execute_command_response(request),
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
            DidSaveTextDocument::METHOD => {
                let Some(params) =
                    decode_notification::<DidSaveTextDocumentParams>(notification.params)
                else {
                    return Ok(None);
                };
                self.refresh_live_preview(params.text_document.uri.as_str());
                Ok(None)
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
        // Closing the buffer is the only signal available that a preview is finished:
        // a browser tab closing is invisible from here.
        self.live_previews.remove(uri);
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
        let diagnostics = diagnostics(&self.state.index, &self.state.antora, &path)
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

    /// Re-render a saved document if it has a live preview on screen.
    ///
    /// A failed refresh is dropped: the preview is a convenience, and a save is the wrong
    /// moment to interrupt the author with a renderer error.
    fn refresh_live_preview(&mut self, uri: &str) {
        if !self.live_previews.contains(uri) {
            return;
        }
        let _ = refresh_preview(&self.state, self.renderer.as_ref(), self.sink.as_ref(), uri);
    }

    fn code_action_response(&self, params: &CodeActionParams) -> CodeActionResponse {
        let uri = params.text_document.uri.as_str();
        if self.state.documents.get(uri).is_none() {
            return Vec::new();
        }

        let live = if self.live_previews.contains(uri) {
            LivePreview::Active
        } else {
            LivePreview::Inactive
        };

        code_actions_for(uri, live)
            .into_iter()
            .map(|action| {
                CodeActionOrCommand::CodeAction(CodeAction {
                    title: action.title.clone(),
                    kind: Some(CodeActionKind::EMPTY),
                    command: Some(Command {
                        title: action.title,
                        command: action.command.to_owned(),
                        arguments: Some(vec![serde_json::Value::String(action.uri)]),
                    }),
                    ..CodeAction::default()
                })
            })
            .collect()
    }

    fn execute_command_response(&mut self, request: Request) -> Response {
        let id = request.id.clone();
        let params: ExecuteCommandParams = match serde_json::from_value(request.params) {
            Ok(params) => params,
            Err(error) => {
                return Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string())
            }
        };

        let command = params.command.clone();
        let mode = match command.as_str() {
            RENDER_PREVIEW_COMMAND => PreviewMode::Static,
            RENDER_LIVE_PREVIEW_COMMAND => PreviewMode::Live,
            STOP_LIVE_PREVIEW_COMMAND => PreviewMode::Static,
            other => {
                return Response::new_err(
                    id,
                    ErrorCode::InvalidParams as i32,
                    format!("unsupported command `{other}`"),
                )
            }
        };

        let Some(uri) = params.arguments.first().and_then(serde_json::Value::as_str) else {
            return Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                format!("`{RENDER_PREVIEW_COMMAND}` requires a document URI argument"),
            );
        };

        if command == STOP_LIVE_PREVIEW_COMMAND {
            // The artefact stays as it is; only the following stops.
            self.live_previews.remove(uri);
            return Response::new_ok(id, serde_json::Value::Null);
        }

        match render_preview(
            &self.state,
            self.renderer.as_ref(),
            self.sink.as_ref(),
            uri,
            mode,
        ) {
            Ok(artefact) => {
                if mode == PreviewMode::Live {
                    self.live_previews.insert(uri.to_owned());
                }
                Response::new_ok(id, artefact.display().to_string())
            }
            Err(error) => Response::new_err(id, ErrorCode::InternalError as i32, error.to_string()),
        }
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
        let target = definition_at_offset(
            &self.state.index,
            &self.state.antora,
            &current_path,
            document,
            offset,
        )?;
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

    fn completion_response(&self, params: CompletionParams) -> Option<CompletionResponse> {
        let document_uri = params.text_document_position.text_document.uri.as_str();
        let document = &self.state.documents.get(document_uri)?.document;
        let offset = self
            .encoding
            .offset(&document.text, params.text_document_position.position)?;
        let current_path = document_path(document_uri);
        let candidates = completion_at_offset(
            &self.state.index,
            &self.state.antora,
            &current_path,
            document,
            offset,
        );

        let items = candidates
            .into_iter()
            .filter_map(|candidate| self.completion_item(&document.text, candidate))
            .collect();
        Some(CompletionResponse::List(CompletionList {
            // The candidate set narrows as the target grows, so the client must ask again.
            is_incomplete: true,
            items,
        }))
    }

    fn completion_item(&self, text: &str, candidate: Candidate) -> Option<CompletionItem> {
        let range = self.encoding.range(text, candidate.range)?;
        Some(CompletionItem {
            label: candidate.label.clone(),
            kind: Some(match candidate.kind {
                CandidateKind::Page | CandidateKind::Resource => CompletionItemKind::FILE,
                CandidateKind::Family => CompletionItemKind::KEYWORD,
                CandidateKind::Directory => CompletionItemKind::FOLDER,
                CandidateKind::Anchor => CompletionItemKind::REFERENCE,
            }),
            detail: candidate.detail,
            sort_text: Some(candidate.sort_text),
            filter_text: Some(candidate.label.clone()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: candidate.label,
            })),
            ..CompletionItem::default()
        })
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
    use std::{fs, path::Path, str::FromStr, thread};

    use lsp_server::{Connection, Message, Notification, Request, RequestId};
    use lsp_types::{
        notification::{
            DidOpenTextDocument, Initialized, Notification as LspNotification, PublishDiagnostics,
        },
        request::{
            DocumentSymbolRequest, GotoDefinition, Initialize, Request as LspRequest, Shutdown,
        },
        ClientCapabilities, DiagnosticSeverity, DidOpenTextDocumentParams, DocumentSymbolParams,
        GotoDefinitionParams, GotoDefinitionResponse, InitializeParams, InitializedParams,
        Location, NumberOrString, PartialResultParams, Position, PublishDiagnosticsParams, Range,
        TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
        TextDocumentPositionParams, Uri, WorkDoneProgressParams, WorkspaceFolder,
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
    fn publishes_parser_diagnostics_for_duplicate_anchors() {
        let (server_connection, client_connection) = Connection::memory();
        let server = thread::spawn(move || run_connection(&server_connection));
        let uri: Uri = "file:///duplicates.adoc".parse().unwrap();

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
        client_connection.receiver.recv().unwrap();
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
                        text: "= Guide\n\n[[intro]]\n== Intro\n\n[[intro]]\n== Intro Again\n"
                            .to_owned(),
                    },
                },
            )))
            .unwrap();

        let Message::Notification(notification) = client_connection.receiver.recv().unwrap() else {
            panic!("expected a publishDiagnostics notification");
        };
        assert_eq!(notification.method, PublishDiagnostics::METHOD);
        let params: PublishDiagnosticsParams = serde_json::from_value(notification.params).unwrap();
        let duplicate = params
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code == Some(NumberOrString::String("adoc.duplicate-anchor".to_owned()))
            })
            .expect("duplicate anchor diagnostic was published");

        assert_eq!(duplicate.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(duplicate.range.start.line, 5);

        client_connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                Shutdown::METHOD.to_owned(),
                (),
            )))
            .unwrap();
        client_connection.receiver.recv().unwrap();
        client_connection
            .sender
            .send(Message::Notification(Notification::new(
                "exit".to_owned(),
                (),
            )))
            .unwrap();

        server.join().unwrap().unwrap();
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

    #[test]
    fn serves_filesystem_and_antora_definitions() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let (server_connection, client_connection) = Connection::memory();
        let server = thread::spawn(move || run_connection(&server_connection));

        client_connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                Initialize::METHOD.to_owned(),
                InitializeParams {
                    capabilities: ClientCapabilities::default(),
                    workspace_folders: Some(vec![WorkspaceFolder {
                        uri: file_uri(&fixtures),
                        name: "fixtures".to_owned(),
                    }]),
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

        let xref_path = fixtures.join("xrefs/index.adoc");
        let xref_uri = open_fixture(&client_connection, &xref_path);
        let location = request_definition(&client_connection, 2, xref_uri, Position::new(2, 10));
        assert!(uri_path(&location.uri).ends_with("xrefs/other.adoc"));
        assert_eq!(location.range.start.line, 2);

        let antora_page = fixtures.join("antora-single-component/modules/ROOT/pages/index.adoc");
        let antora_uri = open_fixture(&client_connection, &antora_page);
        let location = request_definition(&client_connection, 3, antora_uri, Position::new(2, 8));
        assert!(uri_path(&location.uri)
            .ends_with("antora-single-component/modules/security/pages/authentication.adoc"));

        let authentication =
            fixtures.join("antora-single-component/modules/security/pages/authentication.adoc");
        let authentication_uri = open_fixture(&client_connection, &authentication);
        let location = request_definition(
            &client_connection,
            4,
            authentication_uri,
            Position::new(2, 12),
        );
        assert!(uri_path(&location.uri)
            .ends_with("antora-single-component/modules/security/partials/token-note.adoc"));

        client_connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(5),
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

    fn open_fixture(connection: &Connection, path: &Path) -> Uri {
        let uri = file_uri(path);
        connection
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.to_owned(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: uri.clone(),
                        language_id: "asciidoc".to_owned(),
                        version: 1,
                        text: fs::read_to_string(path).unwrap(),
                    },
                },
            )))
            .unwrap();
        assert!(matches!(
            connection.receiver.recv().unwrap(),
            Message::Notification(ref notification)
                if notification.method == PublishDiagnostics::METHOD
        ));
        uri
    }

    fn request_definition(
        connection: &Connection,
        id: i32,
        uri: Uri,
        position: Position,
    ) -> Location {
        connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(id),
                GotoDefinition::METHOD.to_owned(),
                GotoDefinitionParams {
                    text_document_position_params: TextDocumentPositionParams::new(
                        TextDocumentIdentifier { uri },
                        position,
                    ),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                },
            )))
            .unwrap();
        let Message::Response(response) = connection.receiver.recv().unwrap() else {
            panic!("expected definition response");
        };
        let response: Option<GotoDefinitionResponse> =
            serde_json::from_value(response.response_result.unwrap()).unwrap();
        let Some(GotoDefinitionResponse::Scalar(location)) = response else {
            panic!("expected scalar definition location");
        };
        location
    }

    fn file_uri(path: &Path) -> Uri {
        Uri::from_str(url::Url::from_file_path(path).unwrap().as_str()).unwrap()
    }

    fn uri_path(uri: &Uri) -> std::path::PathBuf {
        url::Url::parse(uri.as_str())
            .unwrap()
            .to_file_path()
            .unwrap()
    }
}
