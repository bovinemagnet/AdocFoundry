use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use adoc_core::{DiagnosticSeverity as CoreDiagnosticSeverity, SourceRange};
use adoc_engine::{
    commands::ids, CommandArgs, DefinitionTarget, DocumentId, DocumentSymbolKind, Engine,
    EngineEvent, ExportArgs, ExportTarget, FileEdits, Settings, WorkspaceSymbolKind,
};
use adoc_lang::completion::{Candidate, CandidateKind};
use crossbeam_channel::RecvTimeoutError;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::{
    notification::{
        DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
        Notification as LspNotification, PublishDiagnostics, ShowMessage,
    },
    request::{
        CodeActionRequest, Completion, DocumentSymbolRequest, ExecuteCommand, GotoDefinition,
        HoverRequest, References, Rename, Request as LspRequest, WillRenameFiles,
        WorkspaceSymbolRequest,
    },
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse, Command,
    CompletionItem, CompletionItemKind, CompletionList, CompletionParams, CompletionResponse,
    CompletionTextEdit, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, ExecuteCommandParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    InitializeParams, InitializeResult, Location, MarkupContent, MarkupKind, MessageType,
    NumberOrString, PublishDiagnosticsParams, ReferenceParams, RenameFilesParams, RenameParams,
    ServerInfo, ShowMessageParams, SymbolInformation, SymbolKind, TextDocumentContentChangeEvent,
    TextEdit, Uri, WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};

use crate::{
    capabilities::{
        server_capabilities, ADD_TO_USER_COMMAND, ADD_TO_WORKSPACE_COMMAND,
        CHECK_WORKSPACE_COMMAND, EXPORT_PDF_COMMAND,
    },
    handlers::{
        code_actions::{
            code_actions_for, LivePreview, RENDER_LIVE_PREVIEW_COMMAND, RENDER_PREVIEW_COMMAND,
            STOP_LIVE_PREVIEW_COMMAND,
        },
        execute_command::{refresh_preview, render_preview, render_preview_with_engine},
    },
    position::PositionEncoding,
    preview::{BrowserSink, PreviewMode, PreviewSink, SystemLauncher},
    server::ServerError,
    state::document_path,
};

/// How long the server waits for a message before letting the engine catch up: worker
/// results, settled typing and file changes, whose diagnostics are then published.
const TICK: Duration = Duration::from_millis(50);

/// How long indexing the workspace may take before the server answers `initialize` anyway.
const INDEX_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a PDF export may take.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);

pub fn run_connection(connection: &Connection) -> Result<(), ServerError> {
    let (initialize_id, initialize_value) = connection.initialize_start()?;
    let initialize_params: InitializeParams = serde_json::from_value(initialize_value)?;
    let encoding = PositionEncoding::negotiate(&initialize_params.capabilities);
    let roots = workspace_roots(&initialize_params);
    // The files the app reads: the user settings file and the workspace's `.adocfoundry.toml`
    // (which `open_workspace` loads). Git is opt-in here.
    let settings = Settings::load(Settings::default_user_file().as_deref(), None)
        .unwrap_or_default()
        .with_git_opt_in();
    let mut engine = Engine::new(settings);
    engine.enable_spelling();
    let mut server = ProtocolServer::new(
        encoding,
        engine,
        Box::new(BrowserSink::new(
            std::env::temp_dir().join("adoc-ls-preview"),
            SystemLauncher,
        )),
    );
    server.open_workspace(roots);

    let result = InitializeResult {
        capabilities: server_capabilities(encoding, server.can_export()),
        server_info: Some(ServerInfo {
            name: "adoc-ls".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
        offset_encoding: None,
    };
    connection.initialize_finish(initialize_id, serde_json::to_value(result)?)?;

    let send = |message: Message| {
        connection
            .sender
            .send(message)
            .map_err(|_| ServerError::ChannelClosed)
    };
    loop {
        let notifications = match connection.receiver.recv_timeout(TICK) {
            Ok(Message::Request(request)) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                let (response, notifications) = server.handle_request(request);
                for notification in notifications {
                    send(notification.into())?;
                }
                send(response.into())?;
                Vec::new()
            }
            Ok(Message::Notification(notification)) => server.handle_notification(notification)?,
            Ok(Message::Response(_)) => Vec::new(),
            Err(RecvTimeoutError::Timeout) => server.tick(),
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        for notification in notifications {
            send(notification.into())?;
        }
    }
}

pub(crate) struct ProtocolServer {
    pub(crate) engine: Engine,
    events: std::sync::mpsc::Receiver<EngineEvent>,
    encoding: PositionEncoding,
    /// The client's URI and version of each document it has open.
    open: BTreeMap<DocumentId, (String, i32)>,
    sink: Box<dyn PreviewSink>,
    /// Documents with a live preview on screen, re-rendered when they are saved.
    live_previews: BTreeSet<String>,
}

impl ProtocolServer {
    pub(crate) fn new(
        encoding: PositionEncoding,
        mut engine: Engine,
        sink: Box<dyn PreviewSink>,
    ) -> Self {
        let events = engine.subscribe();
        Self {
            engine,
            events,
            encoding,
            open: BTreeMap::new(),
            sink,
            live_previews: BTreeSet::new(),
        }
    }

    /// Opens the workspace folders that exist and waits for them to be indexed, so the first
    /// requests already see every page.
    pub(crate) fn open_workspace(&mut self, roots: Vec<PathBuf>) {
        let roots: Vec<PathBuf> = roots.into_iter().filter(|root| root.is_dir()).collect();
        if roots.is_empty() || self.engine.open_workspace(roots).is_err() {
            return;
        }
        self.engine.wait_for_workers(INDEX_TIMEOUT);
        // Nothing is open yet, so there is nothing to publish.
        self.events.try_iter().for_each(drop);
    }

    /// Whether this build exports PDF: the engine has Typst.
    pub(crate) fn can_export(&self) -> bool {
        self.engine.commands().get(ids::EXPORT_PDF_PAGE).is_some()
    }

    /// Lets the engine apply worker results and settled changes, and publishes the
    /// diagnostics that changed.
    pub(crate) fn tick(&mut self) -> Vec<Notification> {
        if self.engine.pump() == 0 {
            return Vec::new();
        }
        self.published(None)
    }

    fn id_for(&self, uri: &str) -> Option<DocumentId> {
        let id = self.engine.document_for_path(&document_path(uri))?;
        self.open.contains_key(&id).then_some(id)
    }

    /// The parsed text of open document `id`, which diagnostics and positions refer to.
    fn text_of(&self, id: DocumentId) -> Option<&str> {
        Some(self.engine.document(id)?.parsed().text.as_str())
    }

    /// The text positions in the file at `path` refer to: its open buffer, else the indexed
    /// copy, else the file.
    fn file_text(&self, path: &Path) -> String {
        if let Some(id) = self.engine.document_for_path(path) {
            if let Some(document) = self.engine.document(id) {
                return document.parsed().text.clone();
            }
        }
        self.engine
            .workspace()
            .index
            .file(path)
            .map(|file| file.document.text.clone())
            .or_else(|| fs::read_to_string(path).ok())
            .unwrap_or_default()
    }

    fn handle_request(&mut self, request: Request) -> (Response, Vec<Notification>) {
        let response = match request.method.as_str() {
            DocumentSymbolRequest::METHOD => self
                .request_response::<DocumentSymbolParams, _>(request, |server, params| {
                    server.document_symbol_response(&params)
                }),
            GotoDefinition::METHOD => self
                .request_response::<GotoDefinitionParams, _>(request, |server, params| {
                    server.definition_response(&params)
                }),
            Completion::METHOD => self
                .request_response::<CompletionParams, _>(request, |server, params| {
                    server.completion_response(&params)
                }),
            WorkspaceSymbolRequest::METHOD => self
                .request_response::<WorkspaceSymbolParams, _>(request, |server, params| {
                    server.workspace_symbol_response(&params)
                }),
            HoverRequest::METHOD => self
                .request_response::<HoverParams, _>(request, |server, params| {
                    server.hover_response(&params)
                }),
            CodeActionRequest::METHOD => self
                .request_response::<CodeActionParams, _>(request, |server, params| {
                    server.code_action_response(&params)
                }),
            References::METHOD => self
                .request_response::<ReferenceParams, _>(request, |server, params| {
                    server.references_response(&params)
                }),
            Rename::METHOD => self.rename_response(request),
            WillRenameFiles::METHOD => self
                .request_response::<RenameFilesParams, _>(request, |server, params| {
                    server.will_rename_files_response(&params)
                }),
            ExecuteCommand::METHOD => return self.execute_command_response(request),
            _ => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request `{}`", request.method),
            ),
        };
        (response, Vec::new())
    }

    fn request_response<P, R>(
        &mut self,
        request: Request,
        handler: impl FnOnce(&mut Self, P) -> R,
    ) -> Response
    where
        P: serde::de::DeserializeOwned,
        R: serde::Serialize,
    {
        match serde_json::from_value(request.params) {
            Ok(params) => Response::new_ok(request.id, handler(self, params)),
            Err(error) => Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            ),
        }
    }

    pub(crate) fn handle_notification(
        &mut self,
        notification: Notification,
    ) -> Result<Vec<Notification>, ServerError> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Some(params) = decode_notification(notification.params) else {
                    return Ok(Vec::new());
                };
                Ok(self.did_open(params))
            }
            DidChangeTextDocument::METHOD => {
                let Some(params) = decode_notification(notification.params) else {
                    return Ok(Vec::new());
                };
                self.did_change(params)
            }
            DidSaveTextDocument::METHOD => {
                let Some(params) =
                    decode_notification::<DidSaveTextDocumentParams>(notification.params)
                else {
                    return Ok(Vec::new());
                };
                self.refresh_live_preview(params.text_document.uri.as_str());
                Ok(Vec::new())
            }
            DidCloseTextDocument::METHOD => {
                let Some(params) = decode_notification(notification.params) else {
                    return Ok(Vec::new());
                };
                Ok(self.did_close(params))
            }
            _ => Ok(Vec::new()),
        }
    }

    fn did_open(&mut self, params: DidOpenTextDocumentParams) -> Vec<Notification> {
        let uri = params.text_document.uri.as_str().to_owned();
        let id = self
            .engine
            .open_text(&document_path(&uri), &params.text_document.text);
        self.open.insert(id, (uri, params.text_document.version));
        self.published(Some(id))
    }

    /// Each change becomes one `edit.replace_range`, its range mapped through the text as the
    /// previous changes left it.
    fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Result<Vec<Notification>, ServerError> {
        let Some(id) = self.id_for(params.text_document.uri.as_str()) else {
            return Ok(Vec::new());
        };
        for change in params.content_changes {
            let text = self
                .engine
                .document(id)
                .map(adoc_engine::OpenDocument::text_string)
                .unwrap_or_default();
            let (range, replacement) = change_range(&text, change, self.encoding)?;
            self.engine
                .invoke(
                    ids::EDIT_REPLACE_RANGE,
                    CommandArgs::Text {
                        id,
                        range,
                        text: replacement,
                    },
                )
                .map_err(|error| ServerError::InvalidChange(error.to_string()))?;
        }
        if let Some((_, version)) = self.open.get_mut(&id) {
            *version = params.text_document.version;
        }
        Ok(self.published(Some(id)))
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) -> Vec<Notification> {
        let uri = params.text_document.uri.as_str();
        // Closing the buffer is the only signal available that a preview is finished:
        // a browser tab closing is invisible from here.
        self.live_previews.remove(uri);
        let Some(id) = self.id_for(uri) else {
            return Vec::new();
        };
        self.open.remove(&id);
        // The engine now holds the file as saved on disk, or not at all.
        let _ = self.engine.close(id);
        let mut notifications = vec![Notification::new(
            PublishDiagnostics::METHOD.to_owned(),
            PublishDiagnosticsParams::new(params.text_document.uri.clone(), Vec::new(), None),
        )];
        notifications.extend(self.published(None));
        notifications
    }

    /// Diagnostics for `changed`, then for every other open document whose diagnostics the
    /// engine says changed.
    fn published(&mut self, changed: Option<DocumentId>) -> Vec<Notification> {
        let mut ids: Vec<DocumentId> = changed.into_iter().collect();
        for event in self.events.try_iter() {
            let id = match event {
                EngineEvent::DiagnosticsUpdated { path } => self.engine.document_for_path(&path),
                EngineEvent::SpellingChanged(id) => Some(id),
                _ => None,
            };
            if let Some(id) = id.filter(|id| !ids.contains(id)) {
                ids.push(id);
            }
        }
        ids.into_iter()
            .filter_map(|id| self.diagnostics_notification(id))
            .collect()
    }

    fn diagnostics_notification(&self, id: DocumentId) -> Option<Notification> {
        let (uri, version) = self.open.get(&id)?;
        let uri = Uri::from_str(uri).ok()?;
        let document = self.engine.document(id)?;
        let mut diagnostics: Vec<Diagnostic> = document
            .diagnostics
            .iter()
            .filter_map(|diagnostic| {
                Some(Diagnostic {
                    range: self
                        .encoding
                        .range(&document.parsed().text, diagnostic.range)?,
                    severity: Some(match diagnostic.severity {
                        CoreDiagnosticSeverity::Information => DiagnosticSeverity::INFORMATION,
                        CoreDiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                        CoreDiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                    }),
                    code: Some(NumberOrString::String(diagnostic.code.as_str().to_owned())),
                    source: Some("adoc-ls".to_owned()),
                    message: diagnostic.message.clone(),
                    ..Diagnostic::default()
                })
            })
            .collect();
        let text = &document.parsed().text;
        diagnostics.extend(
            self.engine
                .misspellings(id)
                .iter()
                .filter_map(|misspelling| {
                    Some(Diagnostic {
                        range: self.encoding.range(text, misspelling.range)?,
                        severity: Some(DiagnosticSeverity::HINT),
                        code: Some(NumberOrString::String(
                            adoc_engine::SPELLING_CODE.to_owned(),
                        )),
                        source: Some("adoc-ls".to_owned()),
                        message: format!("Unknown word: {}", misspelling.word),
                        ..Diagnostic::default()
                    })
                }),
        );
        Some(Notification::new(
            PublishDiagnostics::METHOD.to_owned(),
            PublishDiagnosticsParams::new(uri, diagnostics, Some(*version)),
        ))
    }

    pub(crate) fn document_symbol_response(
        &self,
        params: &DocumentSymbolParams,
    ) -> Option<DocumentSymbolResponse> {
        let id = self.id_for(params.text_document.uri.as_str())?;
        let text = self.text_of(id)?;
        Some(DocumentSymbolResponse::Nested(
            self.engine
                .outline(id)
                .into_iter()
                .filter_map(|symbol| self.lsp_document_symbol(text, symbol))
                .collect(),
        ))
    }

    #[allow(deprecated)]
    fn lsp_document_symbol(
        &self,
        text: &str,
        symbol: adoc_engine::DocumentSymbol,
    ) -> Option<DocumentSymbol> {
        let children: Vec<_> = symbol
            .children
            .into_iter()
            .filter_map(|child| self.lsp_document_symbol(text, child))
            .collect();
        Some(DocumentSymbol {
            name: symbol.name,
            detail: symbol.detail,
            kind: match symbol.kind {
                DocumentSymbolKind::Document | DocumentSymbolKind::Page => SymbolKind::FILE,
                DocumentSymbolKind::Section | DocumentSymbolKind::Category => SymbolKind::NAMESPACE,
            },
            tags: None,
            deprecated: None,
            range: self.encoding.range(text, symbol.range)?,
            selection_range: self.encoding.range(text, symbol.selection_range)?,
            children: (!children.is_empty()).then_some(children),
        })
    }

    /// Re-render a saved document if it has a live preview on screen.
    ///
    /// A failed refresh is dropped: the preview is a convenience, and a save is the wrong
    /// moment to interrupt the author with a renderer error.
    fn refresh_live_preview(&mut self, uri: &str) {
        if !self.live_previews.contains(uri) {
            return;
        }
        let asciidoctor = self.asciidoctor();
        let renderer = asciidoctor
            .as_ref()
            .map(|renderer| renderer as &dyn adoc_render::Renderer);
        let _ = refresh_preview(&mut self.engine, renderer, self.sink.as_ref(), uri);
    }

    /// Asciidoctor, when it is configured or on `PATH`.
    fn asciidoctor(&self) -> Option<adoc_render::SystemAsciidoctor> {
        self.engine
            .asciidoctor_executable()
            .map(adoc_render::SystemAsciidoctor::new)
    }

    fn code_action_response(&self, params: &CodeActionParams) -> CodeActionResponse {
        let uri = params.text_document.uri.as_str();
        if self.id_for(uri).is_none() {
            return Vec::new();
        }

        let live = if self.live_previews.contains(uri) {
            LivePreview::Active
        } else {
            LivePreview::Inactive
        };

        let mut actions = self.spelling_actions(params);
        actions.extend(code_actions_for(uri, live).into_iter().map(|action| {
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
        }));
        actions
    }

    /// For each misspelling the range touches: a change to each suggestion, and adding the word
    /// to the workspace dictionary, or the user's when there is a user settings file.
    fn spelling_actions(&self, params: &CodeActionParams) -> Vec<CodeActionOrCommand> {
        let uri = &params.text_document.uri;
        let Some(id) = self.id_for(uri.as_str()) else {
            return Vec::new();
        };
        let Some(text) = self.text_of(id) else {
            return Vec::new();
        };
        let (Some(start), Some(end)) = (
            self.encoding.offset(text, params.range.start),
            self.encoding.offset(text, params.range.end),
        ) else {
            return Vec::new();
        };
        let user = self.engine.settings().user_file().is_some();
        let mut actions = Vec::new();
        for misspelling in self.engine.misspellings(id) {
            if misspelling.range.start > end || misspelling.range.end < start {
                continue;
            }
            let Some(range) = self.encoding.range(text, misspelling.range) else {
                continue;
            };
            for suggestion in &misspelling.suggestions {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Change to '{suggestion}'"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    edit: Some(WorkspaceEdit {
                        changes: Some(
                            [(uri.clone(), vec![TextEdit::new(range, suggestion.clone())])]
                                .into_iter()
                                .collect(),
                        ),
                        ..WorkspaceEdit::default()
                    }),
                    ..CodeAction::default()
                }));
            }
            let mut add = vec![(
                format!("Add '{}' to the workspace dictionary", misspelling.word),
                ADD_TO_WORKSPACE_COMMAND,
            )];
            if user {
                add.push((
                    format!("Add '{}' to the user dictionary", misspelling.word),
                    ADD_TO_USER_COMMAND,
                ));
            }
            for (title, command) in add {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: title.clone(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    command: Some(Command {
                        title,
                        command: command.to_owned(),
                        arguments: Some(vec![
                            serde_json::Value::String(uri.as_str().to_owned()),
                            serde_json::json!(misspelling.range.start),
                            serde_json::json!(misspelling.range.end),
                        ]),
                    }),
                    ..CodeAction::default()
                }));
            }
        }
        actions
    }

    pub(crate) fn execute_command_response(
        &mut self,
        request: Request,
    ) -> (Response, Vec<Notification>) {
        let id = request.id.clone();
        let params: ExecuteCommandParams = match serde_json::from_value(request.params) {
            Ok(params) => params,
            Err(error) => {
                return (
                    Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
                    Vec::new(),
                )
            }
        };
        let command = params.command.as_str();
        match command {
            EXPORT_PDF_COMMAND if self.can_export() => return self.export_pdf(id, &params),
            CHECK_WORKSPACE_COMMAND => return self.check_workspace(id),
            ADD_TO_WORKSPACE_COMMAND => return self.add_word(id, &params, false),
            ADD_TO_USER_COMMAND => return self.add_word(id, &params, true),
            _ => {}
        }
        let mode = match command {
            RENDER_PREVIEW_COMMAND | STOP_LIVE_PREVIEW_COMMAND => PreviewMode::Static,
            RENDER_LIVE_PREVIEW_COMMAND => PreviewMode::Live,
            other => {
                return (
                    Response::new_err(
                        id,
                        ErrorCode::InvalidParams as i32,
                        format!("unsupported command `{other}`"),
                    ),
                    Vec::new(),
                )
            }
        };

        let Some(uri) = params.arguments.first().and_then(serde_json::Value::as_str) else {
            return (
                Response::new_err(
                    id,
                    ErrorCode::InvalidParams as i32,
                    format!("`{command}` requires a document URI argument"),
                ),
                Vec::new(),
            );
        };

        if command == STOP_LIVE_PREVIEW_COMMAND {
            // The artefact stays as it is; only the following stops.
            self.live_previews.remove(uri);
            return (Response::new_ok(id, serde_json::Value::Null), Vec::new());
        }

        let rendered = match self.asciidoctor() {
            Some(renderer) => {
                render_preview(&self.engine, &renderer, self.sink.as_ref(), uri, mode)
            }
            None => render_preview_with_engine(&mut self.engine, self.sink.as_ref(), uri, mode),
        };
        let response = match rendered {
            Ok(artefact) => {
                if mode == PreviewMode::Live {
                    self.live_previews.insert(uri.to_owned());
                }
                Response::new_ok(id, artefact.display().to_string())
            }
            Err(error) => Response::new_err(id, ErrorCode::InternalError as i32, error.to_string()),
        };
        (response, Vec::new())
    }

    /// `adoc.spelling.addToWorkspace` and `adoc.spelling.addToUser`: `[uri, start, end]`, the
    /// word's byte offsets in the open document.
    fn add_word(
        &mut self,
        request: lsp_server::RequestId,
        params: &ExecuteCommandParams,
        user: bool,
    ) -> (Response, Vec<Notification>) {
        let arguments = &params.arguments;
        let (Some(uri), Some(start), Some(end)) = (
            arguments.first().and_then(serde_json::Value::as_str),
            arguments.get(1).and_then(serde_json::Value::as_u64),
            arguments.get(2).and_then(serde_json::Value::as_u64),
        ) else {
            return (
                Response::new_err(
                    request,
                    ErrorCode::InvalidParams as i32,
                    "expected a document URI and the start and end of a word".to_owned(),
                ),
                Vec::new(),
            );
        };
        let Some(id) = self.id_for(uri) else {
            return (
                Response::new_err(
                    request,
                    ErrorCode::InvalidParams as i32,
                    format!("{uri} is not open"),
                ),
                Vec::new(),
            );
        };
        let command = if user {
            ids::SPELLING_ADD_TO_USER
        } else {
            ids::SPELLING_ADD_TO_WORKSPACE
        };
        let range = SourceRange::new(start as usize, end as usize);
        match self
            .engine
            .invoke(command, CommandArgs::Range { id, range })
        {
            Ok(_) => (
                Response::new_ok(request, serde_json::Value::Null),
                self.published(None),
            ),
            Err(error) => (
                Response::new_err(request, ErrorCode::RequestFailed as i32, error.to_string()),
                Vec::new(),
            ),
        }
    }

    /// `adoc.exportPdf { uri, component?, output, theme?, standards? }`: the page, or its
    /// component version as a book, written to `output` and reported with `showMessage`.
    fn export_pdf(
        &mut self,
        id: lsp_server::RequestId,
        params: &ExecuteCommandParams,
    ) -> (Response, Vec<Notification>) {
        #[derive(serde::Deserialize)]
        struct Args {
            uri: String,
            #[serde(default)]
            component: bool,
            output: PathBuf,
            theme: Option<String>,
            #[serde(default)]
            standards: Vec<String>,
        }
        let failed = |message: String| {
            (
                Response::new_err(id.clone(), ErrorCode::InvalidParams as i32, message),
                Vec::new(),
            )
        };
        let Some(args) = params
            .arguments
            .first()
            .cloned()
            .and_then(|value| serde_json::from_value::<Args>(value).ok())
        else {
            return failed(format!(
                "`{EXPORT_PDF_COMMAND}` takes {{ uri, component?, output, theme?, standards? }}"
            ));
        };
        let mut standards = Vec::new();
        for name in &args.standards {
            match adoc_engine::pdf_standard(name) {
                Some(standard) => standards.push(standard),
                None => return failed(format!("unknown PDF standard `{name}`")),
            }
        }
        let path = document_path(&args.uri);
        let target = if args.component {
            let Some(context) = self.engine.workspace().antora.context_for_path(&path) else {
                return failed(format!("{} is not in an Antora component", path.display()));
            };
            ExportTarget::Component {
                name: context.component,
                version: context.version,
            }
        } else {
            let page = match self.engine.document_for_path(&path) {
                Some(page) => page,
                None => match self.engine.open(&path) {
                    Ok(page) => page,
                    Err(error) => return failed(error.to_string()),
                },
            };
            ExportTarget::Page(page)
        };
        let export = ExportArgs {
            target,
            output: args.output,
            theme: args.theme,
            standards,
            fonts_dir: None,
            typst: None,
            attributes: Vec::new(),
        };
        match self.engine.export_blocking(export, EXPORT_TIMEOUT) {
            Ok(summary) => {
                let mut message = format!(
                    "Exported {} ({} pages)",
                    summary.output.display(),
                    summary.pages
                );
                for diagnostic in &summary.diagnostics {
                    message.push_str(&format!("\n{diagnostic}"));
                }
                let typ = if summary.diagnostics.is_empty() {
                    MessageType::INFO
                } else {
                    MessageType::WARNING
                };
                (
                    Response::new_ok(id, summary.output.display().to_string()),
                    vec![Notification::new(
                        ShowMessage::METHOD.to_owned(),
                        ShowMessageParams { typ, message },
                    )],
                )
            }
            Err(error) => (
                Response::new_err(id, ErrorCode::RequestFailed as i32, error.to_string()),
                Vec::new(),
            ),
        }
    }

    /// `adoc.checkWorkspace`: every file's diagnostics published, open or not, and a count.
    fn check_workspace(&mut self, id: lsp_server::RequestId) -> (Response, Vec<Notification>) {
        let mut notifications = Vec::new();
        let mut problems = 0;
        let mut found = self.engine.workspace_diagnostics();
        found.retain(|(_, diagnostics)| !diagnostics.is_empty());
        let files = found.len();
        for (path, diagnostics) in found {
            if let Some(open) = self
                .engine
                .document_for_path(&path)
                .filter(|open| self.open.contains_key(open))
            {
                problems += diagnostics.len();
                notifications.extend(self.diagnostics_notification(open));
                continue;
            }
            let Some(uri) = path_to_uri(&path) else {
                continue;
            };
            let text = self.file_text(&path);
            problems += diagnostics.len();
            let diagnostics = diagnostics
                .into_iter()
                .filter_map(|diagnostic| {
                    Some(Diagnostic {
                        range: self.encoding.range(&text, diagnostic.range)?,
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
            notifications.push(Notification::new(
                PublishDiagnostics::METHOD.to_owned(),
                PublishDiagnosticsParams::new(uri, diagnostics, None),
            ));
        }
        (
            Response::new_ok(
                id,
                serde_json::json!({ "files": files, "problems": problems }),
            ),
            notifications,
        )
    }

    fn definition_response(&self, params: &GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let position = &params.text_document_position_params;
        let id = self.id_for(position.text_document.uri.as_str())?;
        let offset = self.encoding.offset(self.text_of(id)?, position.position)?;
        let location = match self.engine.definition(id, offset)? {
            DefinitionTarget::Location(location) => location,
            // An image or attachment: the file itself.
            DefinitionTarget::External(path) => adoc_engine::Location {
                path,
                range: adoc_core::SourceRange::default(),
            },
        };
        Some(GotoDefinitionResponse::Scalar(
            self.lsp_location(&location)?,
        ))
    }

    fn lsp_location(&self, location: &adoc_engine::Location) -> Option<Location> {
        let text = self.file_text(&location.path);
        Some(Location::new(
            path_to_uri(&location.path)?,
            self.encoding.range(&text, location.range)?,
        ))
    }

    fn references_response(&self, params: &ReferenceParams) -> Option<Vec<Location>> {
        let position = &params.text_document_position;
        let id = self.id_for(position.text_document.uri.as_str())?;
        let offset = self.encoding.offset(self.text_of(id)?, position.position)?;
        Some(
            self.engine
                .references(id, offset)
                .iter()
                .filter_map(|location| self.lsp_location(location))
                .collect(),
        )
    }

    /// `textDocument/rename` on an anchor: the anchor and every reference to it.
    fn rename_response(&mut self, request: Request) -> Response {
        let id = request.id.clone();
        let params: RenameParams = match serde_json::from_value(request.params) {
            Ok(params) => params,
            Err(error) => {
                return Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string())
            }
        };
        let position = &params.text_document_position;
        let edits = self
            .id_for(position.text_document.uri.as_str())
            .and_then(|document| {
                let offset = self
                    .encoding
                    .offset(self.text_of(document)?, position.position)?;
                self.engine
                    .anchor_rename_edits(document, offset, &params.new_name)
            });
        match edits {
            Some(edits) => Response::new_ok(id, self.workspace_edit(&edits)),
            None => Response::new_err(
                id,
                ErrorCode::RequestFailed as i32,
                "only an anchor id can be renamed here, to an id without spaces; rename a page \
                 by renaming its file"
                    .to_owned(),
            ),
        }
    }

    /// `workspace/willRenameFiles`: for each page moved, the edits to every xref and nav entry
    /// that names it. The client moves the files.
    fn will_rename_files_response(&mut self, params: &RenameFilesParams) -> Option<WorkspaceEdit> {
        let mut edits: Vec<FileEdits> = Vec::new();
        for rename in &params.files {
            let from = document_path(&rename.old_uri);
            let to = document_path(&rename.new_uri);
            if !adoc_index::is_asciidoc_path(&from) {
                continue;
            }
            let was_closed = self.engine.document_for_path(&from).is_none();
            let Ok(page) = self.engine.open(&from) else {
                continue;
            };
            if let Ok(preview) = self.engine.rename_preview(page, &to) {
                let preview_id = preview.id;
                edits.extend(preview.edits.clone());
                self.engine.discard_preview(preview_id);
            }
            if was_closed {
                let _ = self.engine.close(page);
            }
        }
        // Opening and closing pages to compute the edits changes nothing the client sees.
        self.events.try_iter().for_each(drop);
        (!edits.is_empty()).then(|| self.workspace_edit(&edits))
    }

    // `WorkspaceEdit` is keyed by `Uri`, which clippy counts as mutable; it is never mutated.
    #[allow(clippy::mutable_key_type)]
    fn workspace_edit(&self, files: &[FileEdits]) -> WorkspaceEdit {
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        for file in files {
            let Some(uri) = path_to_uri(&file.path) else {
                continue;
            };
            let text = self.file_text(&file.path);
            changes
                .entry(uri)
                .or_default()
                .extend(file.edits.iter().filter_map(|edit| {
                    Some(TextEdit {
                        range: self.encoding.range(&text, edit.range)?,
                        new_text: edit.replacement.clone(),
                    })
                }));
        }
        WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        }
    }

    #[allow(deprecated)]
    pub(crate) fn workspace_symbol_response(
        &self,
        params: &WorkspaceSymbolParams,
    ) -> Option<WorkspaceSymbolResponse> {
        let symbols = self
            .engine
            .workspace_symbols(&params.query)
            .into_iter()
            .filter_map(|symbol| {
                // The indexed text is the open buffer when the file is open.
                let text = &self
                    .engine
                    .workspace()
                    .index
                    .file(&symbol.path)?
                    .document
                    .text;
                Some(SymbolInformation {
                    name: symbol.name.clone(),
                    kind: match symbol.kind {
                        WorkspaceSymbolKind::Title => SymbolKind::FILE,
                        WorkspaceSymbolKind::Section => SymbolKind::NAMESPACE,
                        WorkspaceSymbolKind::Anchor => SymbolKind::KEY,
                        WorkspaceSymbolKind::Attribute => SymbolKind::VARIABLE,
                    },
                    tags: None,
                    deprecated: None,
                    location: Location::new(
                        path_to_uri(&symbol.path)?,
                        self.encoding.range(text, symbol.range)?,
                    ),
                    container_name: symbol.container.clone(),
                })
            })
            .collect();
        Some(WorkspaceSymbolResponse::Flat(symbols))
    }

    fn hover_response(&self, params: &HoverParams) -> Option<Hover> {
        let position = &params.text_document_position_params;
        let id = self.id_for(position.text_document.uri.as_str())?;
        let text = self.text_of(id)?;
        let offset = self.encoding.offset(text, position.position)?;
        let content = self.engine.hover(id, offset)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content.markdown,
            }),
            range: self.encoding.range(text, content.range),
        })
    }

    fn completion_response(&self, params: &CompletionParams) -> Option<CompletionResponse> {
        let position = &params.text_document_position;
        let id = self.id_for(position.text_document.uri.as_str())?;
        let text = self.text_of(id)?;
        let offset = self.encoding.offset(text, position.position)?;
        let items = self
            .engine
            .completions(id, offset)
            .into_iter()
            .filter_map(|candidate| self.completion_item(text, candidate))
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
                CandidateKind::Attribute => CompletionItemKind::VARIABLE,
            }),
            detail: candidate.detail,
            sort_text: Some(candidate.sort_text),
            filter_text: Some(candidate.label.clone()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: candidate.insert_text.unwrap_or(candidate.label),
            })),
            ..CompletionItem::default()
        })
    }
}

fn decode_notification<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Option<T> {
    serde_json::from_value(value).ok()
}

/// The byte range of `text` a change replaces, and its new text; a change without a range
/// replaces the whole document.
fn change_range(
    text: &str,
    change: TextDocumentContentChangeEvent,
    encoding: PositionEncoding,
) -> Result<(adoc_core::SourceRange, String), ServerError> {
    let Some(range) = change.range else {
        return Ok((adoc_core::SourceRange::new(0, text.len()), change.text));
    };
    let start = encoding
        .offset(text, range.start)
        .ok_or_else(|| ServerError::InvalidChange("invalid change start".to_owned()))?;
    let end = encoding
        .offset(text, range.end)
        .ok_or_else(|| ServerError::InvalidChange("invalid change end".to_owned()))?;
    if start > end {
        return Err(ServerError::InvalidChange(
            "change start is after its end".to_owned(),
        ));
    }
    Ok((adoc_core::SourceRange::new(start, end), change.text))
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
            DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Initialized,
            Notification as LspNotification, PublishDiagnostics,
        },
        request::{
            DocumentSymbolRequest, ExecuteCommand, GotoDefinition, Initialize,
            Request as LspRequest, Shutdown,
        },
        ClientCapabilities, CodeActionContext, CodeActionOrCommand, CodeActionParams, Diagnostic,
        DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
        DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
        ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse, InitializeParams,
        InitializedParams, Location, NumberOrString, PartialResultParams, Position,
        PublishDiagnosticsParams, Range, SymbolKind, TextDocumentContentChangeEvent,
        TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
        VersionedTextDocumentIdentifier, WorkDoneProgressParams, WorkspaceFolder,
        WorkspaceSymbolParams, WorkspaceSymbolResponse,
    };

    use adoc_engine::{Engine, Settings};

    use crate::{
        position::PositionEncoding,
        preview::{BrowserSink, Launcher, PreviewError},
    };

    use super::{change_range, run_connection, ProtocolServer};

    /// Writes previews without opening them.
    struct Unopened;

    impl Launcher for Unopened {
        fn launch(&self, _: &Path) -> Result<(), PreviewError> {
            Ok(())
        }
    }

    fn server_with(settings: Settings, previews: &Path) -> ProtocolServer {
        ProtocolServer::new(
            PositionEncoding::Utf16,
            Engine::new(settings),
            Box::new(BrowserSink::new(previews, Unopened)),
        )
    }

    fn server() -> ProtocolServer {
        server_with(
            Settings::default(),
            &std::env::temp_dir().join("adoc-ls-test-previews"),
        )
    }

    #[test]
    fn applies_incremental_utf16_changes() {
        let (range, text) = change_range(
            "A😀B\n",
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 1), Position::new(0, 3))),
                range_length: Some(2),
                text: "C".to_owned(),
            },
            PositionEncoding::Utf16,
        )
        .unwrap();

        assert_eq!((range.start, range.end, text.as_str()), (1, 5, "C"));
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
        // Opening a document republishes every open one, so publishes may precede the response.
        let response = connection
            .receiver
            .iter()
            .find_map(|message| match message {
                Message::Response(response) => Some(response),
                _ => None,
            })
            .expect("expected definition response");
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

    /// The `(uri, diagnostic codes)` of every publish a notification produces, in order.
    fn published(
        server: &mut ProtocolServer,
        method: &str,
        params: impl serde::Serialize,
    ) -> Vec<(String, Vec<String>)> {
        server
            .handle_notification(Notification::new(method.to_owned(), params))
            .unwrap()
            .into_iter()
            .map(|notification| {
                assert_eq!(notification.method, PublishDiagnostics::METHOD);
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(notification.params).unwrap();
                let codes = params
                    .diagnostics
                    .into_iter()
                    .filter_map(|diagnostic| match diagnostic.code {
                        Some(NumberOrString::String(code)) => Some(code),
                        _ => None,
                    })
                    .collect();
                (params.uri.as_str().to_owned(), codes)
            })
            .collect()
    }

    fn open(server: &mut ProtocolServer, uri: &str, text: &str) -> Vec<(String, Vec<String>)> {
        published(
            server,
            DidOpenTextDocument::METHOD,
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.parse().unwrap(),
                    language_id: "asciidoc".to_owned(),
                    version: 1,
                    text: text.to_owned(),
                },
            },
        )
    }

    const PAGE: &str = "file:///docs/page.adoc";
    const OTHER: &str = "file:///docs/other.adoc";

    #[test]
    fn republishes_other_open_documents_when_a_file_opens_or_changes() {
        let mut server = server();
        open(
            &mut server,
            PAGE,
            "= Page\n\nSee xref:other.adoc#details[].\n",
        );

        // Opening the target resolves the page's xref, so the page is published again.
        let after_open = open(&mut server, OTHER, "= Other\n\n[[details]]\n== Details\n");
        assert_eq!(after_open[0].0, OTHER, "the opened file comes first");
        assert!(
            after_open.contains(&(PAGE.to_owned(), Vec::new())),
            "{after_open:?}"
        );

        // Removing the anchor breaks the page's xref.
        let after_change = published(
            &mut server,
            DidChangeTextDocument::METHOD,
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(OTHER.parse().unwrap(), 2),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "= Other\n".to_owned(),
                }],
            },
        );
        assert_eq!(after_change[0].0, OTHER, "the changed file comes first");
        assert!(
            after_change.contains(&(PAGE.to_owned(), vec!["adoc.unresolved-anchor".to_owned()])),
            "{after_change:?}"
        );
    }

    #[test]
    fn republishes_other_open_documents_when_a_file_closes() {
        let mut server = server();
        open(
            &mut server,
            PAGE,
            "= Page\n\nSee xref:other.adoc#details[].\n",
        );
        open(&mut server, OTHER, "= Other\n\n[[details]]\n== Details\n");

        // `/docs/other.adoc` is not on disk, so closing it drops it from the index.
        let after_close = published(
            &mut server,
            DidCloseTextDocument::METHOD,
            DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier::new(OTHER.parse().unwrap()),
            },
        );

        assert_eq!(
            after_close,
            vec![
                (OTHER.to_owned(), Vec::new()),
                (
                    PAGE.to_owned(),
                    vec!["adoc.unresolved-xref-file".to_owned()]
                ),
            ]
        );
    }

    #[test]
    fn answers_workspace_symbol_requests_from_the_index() {
        // A real file URI on this platform, which the symbol's location is built from.
        let page = file_uri(&std::env::temp_dir().join("docs").join("page.adoc"));
        let mut server = server();
        open(
            &mut server,
            page.as_str(),
            "= Guide\n\n== Authentication Flow\n",
        );

        let Some(WorkspaceSymbolResponse::Flat(symbols)) =
            server.workspace_symbol_response(&WorkspaceSymbolParams {
                query: "auth".to_owned(),
                ..WorkspaceSymbolParams::default()
            })
        else {
            panic!("expected a flat symbol list");
        };

        assert_eq!(symbols.len(), 1, "{symbols:?}");
        let symbol = &symbols[0];
        assert_eq!(symbol.name, "Authentication Flow");
        assert_eq!(symbol.kind, SymbolKind::NAMESPACE);
        assert_eq!(symbol.container_name.as_deref(), Some("Guide"));
        assert_eq!(symbol.location.uri, page);
        assert_eq!(symbol.location.range.start.line, 2);
    }

    #[test]
    fn publishes_a_warning_for_files_under_an_invalid_descriptor() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-invalid-descriptor");
        let page = root.join("modules/ROOT/pages/index.adoc");
        let mut server = server();
        server.open_workspace(vec![root]);

        let published = open(&mut server, file_uri(&page).as_str(), "= Home\n");

        assert_eq!(
            published[0].1,
            vec!["adoc.antora.invalid-descriptor".to_owned()]
        );
    }

    fn document_symbols_of(server: &ProtocolServer, uri: &str) -> Vec<DocumentSymbol> {
        let Some(DocumentSymbolResponse::Nested(symbols)) =
            server.document_symbol_response(&DocumentSymbolParams {
                text_document: TextDocumentIdentifier::new(uri.parse().unwrap()),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
        else {
            panic!("expected nested document symbols");
        };
        symbols
    }

    #[test]
    fn outlines_a_navigation_file_by_its_list() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let nav = file_uri(&root.join("modules/ROOT/nav.adoc"));
        let mut server = server();
        server.open_workspace(vec![root]);
        open(
            &mut server,
            nav.as_str(),
            "* xref:index.adoc[Home]\n** xref:security:authentication.adoc[Authentication]\n",
        );

        let symbols = document_symbols_of(&server, nav.as_str());

        assert_eq!(symbols.len(), 1, "{symbols:?}");
        assert_eq!(symbols[0].name, "Home");
        assert_eq!(symbols[0].kind, SymbolKind::FILE);
        assert_eq!(symbols[0].detail.as_deref(), Some("index.adoc"));
        let children = symbols[0].children.as_deref().unwrap_or_default();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "Authentication");
        assert_eq!(symbols[0].range.end.line, 1);
    }

    #[test]
    fn nests_an_ordinary_document_s_sections() {
        let mut server = server();
        open(&mut server, PAGE, "= Guide\n\n== Start\n\n=== Detail\n");

        let symbols = document_symbols_of(&server, PAGE);

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::FILE);
        let start = &symbols[0].children.as_deref().unwrap_or_default()[0];
        assert_eq!(start.name, "Start");
        assert_eq!(start.kind, SymbolKind::NAMESPACE);
        assert_eq!(
            start.children.as_deref().unwrap_or_default()[0].name,
            "Detail"
        );
    }

    /// Tests that write and run an executable take this, so no other test forks while the
    /// script is open for writing (`ETXTBSY`), as `adoc-render`'s `spawn_guard` does there.
    fn spawn_guard() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Renders `uri` with `adoc.renderPreview` and reads the artefact written.
    fn rendered(server: &mut ProtocolServer, uri: &str) -> String {
        let (response, _) = server.execute_command_response(Request::new(
            RequestId::from(1),
            "workspace/executeCommand".to_owned(),
            serde_json::json!({ "command": "adoc.renderPreview", "arguments": [uri] }),
        ));
        let artefact = response.response_result.expect("rendered");
        fs::read_to_string(artefact.as_str().unwrap()).unwrap()
    }

    /// A server whose Asciidoctor is `asciidoctor`, which need not exist.
    fn server_rendering_with(asciidoctor: &Path, previews: &Path) -> ProtocolServer {
        let mut settings = Settings::default();
        settings.tools.asciidoctor = asciidoctor.display().to_string();
        server_with(settings, previews)
    }

    #[cfg(unix)]
    #[test]
    fn preview_uses_asciidoctor_when_available() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = spawn_guard();
        let directory = tempfile::tempdir().unwrap();
        let tool = directory.path().join("asciidoctor");
        fs::write(
            &tool,
            "#!/bin/sh\ncat > /dev/null\necho '<p>from asciidoctor</p>'\n",
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let mut server = server_rendering_with(&tool, &directory.path().join("previews"));
        let uri = file_uri(&directory.path().join("page.adoc"));
        open(&mut server, uri.as_str(), "= Page\n");

        assert!(rendered(&mut server, uri.as_str()).contains("from asciidoctor"));
    }

    #[test]
    fn preview_falls_back_to_the_engine() {
        let _guard = spawn_guard();
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("no-asciidoctor");
        let mut server = server_rendering_with(&missing, &directory.path().join("previews"));
        let uri = file_uri(&directory.path().join("page.adoc"));
        open(
            &mut server,
            uri.as_str(),
            "= From the Engine\n\nA paragraph.\n",
        );

        let html = rendered(&mut server, uri.as_str());
        assert!(html.contains("<html"), "a whole page: {html}");
        assert!(html.contains("From the Engine") && html.contains("A paragraph."));
    }

    #[test]
    fn preview_renders_typeset_stem() {
        let _guard = spawn_guard();
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("no-asciidoctor");
        let mut server = server_rendering_with(&missing, &directory.path().join("previews"));
        let uri = file_uri(&directory.path().join("page.adoc"));
        open(
            &mut server,
            uri.as_str(),
            "= Maths\n\n[stem]\n++++\nsqrt(4) = 2\n++++\n",
        );

        let html = rendered(&mut server, uri.as_str());
        assert!(html.contains("<img"), "{html}");
        assert!(html.contains("alt=\"sqrt(4) = 2\""), "{html}");
        assert!(html.contains(".svg"), "{html}");
    }

    fn spelling_server(directory: &Path) -> ProtocolServer {
        let user = directory.join("settings.toml");
        std::fs::write(&user, "").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap().with_git_opt_in();
        let mut engine = Engine::with_clock(
            settings,
            std::sync::Arc::new(adoc_engine::ManualClock::new()),
        );
        engine.enable_spelling();
        ProtocolServer::new(
            PositionEncoding::Utf16,
            engine,
            Box::new(BrowserSink::new(directory.join("previews"), Unopened)),
        )
    }

    /// The spelling diagnostics of each publish, by URI.
    fn spelling_hints(notifications: Vec<Notification>) -> Vec<(String, Vec<Diagnostic>)> {
        notifications
            .into_iter()
            .map(|notification| {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(notification.params).unwrap();
                let hints = params
                    .diagnostics
                    .into_iter()
                    .filter(|diagnostic| {
                        diagnostic.code
                            == Some(NumberOrString::String(
                                adoc_engine::SPELLING_CODE.to_owned(),
                            ))
                    })
                    .collect();
                (params.uri.as_str().to_owned(), hints)
            })
            .collect()
    }

    #[test]
    fn misspellings_are_published_as_hints_with_quick_fixes() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        let page = root.join("page.adoc");
        let text = "= Page\n\nWe recieve it.\n";
        std::fs::write(&page, text).unwrap();
        let mut server = spelling_server(&root);
        server.open_workspace(vec![root.clone()]);
        let uri = file_uri(&page);
        open(&mut server, uri.as_str(), text);
        server.tick();
        assert!(server
            .engine
            .wait_for_workers(std::time::Duration::from_secs(30)));
        let published = spelling_hints(server.published(None));
        let (_, hints) = published
            .iter()
            .find(|(published, _)| published == uri.as_str())
            .expect("a publish for the page");
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].severity, Some(DiagnosticSeverity::HINT));
        assert_eq!(hints[0].message, "Unknown word: recieve");
        assert_eq!(
            hints[0].range,
            Range::new(Position::new(2, 3), Position::new(2, 10))
        );

        let actions = server.code_action_response(&CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: hints[0].range,
            context: CodeActionContext::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        });
        let titles: Vec<String> = actions
            .iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title.clone()),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles.contains(&"Change to 'receive'".to_owned()),
            "{titles:?}"
        );
        assert!(titles.contains(&"Add 'recieve' to the workspace dictionary".to_owned()));
        assert!(titles.contains(&"Add 'recieve' to the user dictionary".to_owned()));
        let receive = actions
            .iter()
            .find_map(|action| match action {
                CodeActionOrCommand::CodeAction(action)
                    if action.title == "Change to 'receive'" =>
                {
                    action.edit.clone()
                }
                _ => None,
            })
            .unwrap();
        let edits = &receive.changes.unwrap()[&uri];
        assert_eq!(edits[0].new_text, "receive");
        assert_eq!(edits[0].range, hints[0].range);
    }

    #[test]
    fn the_add_to_workspace_command_writes_the_list() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        let page = root.join("page.adoc");
        let text = "= Page\n\nWe recieve it.\n";
        std::fs::write(&page, text).unwrap();
        let mut server = spelling_server(&root);
        server.open_workspace(vec![root.clone()]);
        let uri = file_uri(&page);
        open(&mut server, uri.as_str(), text);
        server.tick();
        assert!(server
            .engine
            .wait_for_workers(std::time::Duration::from_secs(30)));
        server.published(None);
        let start = text.find("recieve").unwrap();
        let (response, notifications) = server.execute_command_response(Request::new(
            RequestId::from(7),
            ExecuteCommand::METHOD.to_owned(),
            ExecuteCommandParams {
                command: crate::capabilities::ADD_TO_WORKSPACE_COMMAND.to_owned(),
                arguments: vec![
                    serde_json::json!(uri.as_str()),
                    serde_json::json!(start),
                    serde_json::json!(start + "recieve".len()),
                ],
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        ));
        assert!(
            response.response_result.is_ok(),
            "{:?}",
            response.response_result
        );
        assert_eq!(
            std::fs::read_to_string(root.join(adoc_engine::WORKSPACE_WORDS_FILE)).unwrap(),
            "recieve\n"
        );
        let published = spelling_hints(notifications);
        assert!(published
            .iter()
            .any(|(published, hints)| published == uri.as_str() && hints.is_empty()));
    }
}
