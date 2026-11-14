use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
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
    CompletionTextEdit, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, InitializeParams, InitializeResult, Location, MarkupContent, MarkupKind,
    MessageType, NumberOrString, PublishDiagnosticsParams, ReferenceParams, RenameFilesParams,
    RenameParams, ServerInfo, ShowMessageParams, SymbolInformation, SymbolKind,
    TextDocumentContentChangeEvent, TextEdit, Uri, WorkspaceEdit, WorkspaceSymbolParams,
    WorkspaceSymbolResponse,
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
    position::{Lines, PositionEncoding},
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

/// The settings the server runs with: the files the app reads, the user settings file at
/// `user_file` and the workspace's `.adocfoundry.toml` (which `open_workspace` loads). Git is
/// opt-in here, and auto-save always off: the client owns its buffers and saves them. A file
/// that does not parse gives the defaults, still naming the file, so a reload reads it again,
/// and the problem to show.
fn lsp_settings(user_file: Option<&Path>) -> (Settings, Option<String>) {
    let (settings, problem) = Settings::load_or_default(user_file, None);
    (
        settings.with_git_opt_in().without_auto_save(),
        problem.map(|error| format!("adoc-ls is using the default settings: {error}")),
    )
}

/// A `window/showMessage` warning.
fn warning(message: String) -> Notification {
    Notification::new(
        ShowMessage::METHOD.to_owned(),
        ShowMessageParams {
            typ: MessageType::WARNING,
            message,
        },
    )
}

pub fn run_connection(connection: &Connection) -> Result<(), ServerError> {
    let (initialize_id, initialize_value) = connection.initialize_start()?;
    let initialize_params: InitializeParams = serde_json::from_value(initialize_value)?;
    let encoding = PositionEncoding::negotiate(&initialize_params.capabilities);
    let roots = workspace_roots(&initialize_params);
    let (settings, settings_problem) = lsp_settings(Settings::default_user_file().as_deref());
    let mut engine = Engine::new(settings);
    engine.enable_spelling();
    let mut server = ProtocolServer::new(
        encoding,
        engine,
        Box::new(BrowserSink::private(SystemLauncher)),
    );
    let mut problems: Vec<String> = settings_problem.into_iter().collect();
    problems.extend(server.open_workspace(roots));

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
    for problem in problems {
        send(warning(problem).into())?;
    }
    loop {
        let mut notifications = match connection.receiver.recv_timeout(TICK) {
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
        // A steady stream of messages never leaves the connection quiet for a tick: what is
        // due is applied between them.
        if server.due(Instant::now()) {
            notifications.extend(server.tick());
        }
        for notification in notifications {
            send(notification.into())?;
        }
    }
}

/// A document a request uses, and whether the request opened it (see
/// [`ProtocolServer::borrow_document`]).
struct Borrowed {
    id: DocumentId,
    opened: bool,
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
    /// Closed files the last `adoc.checkWorkspace` published problems for, cleared by the
    /// next check once they have none.
    checked: BTreeSet<PathBuf>,
    /// Set from any thread when a worker or the watcher sends the engine something to apply.
    woken: Arc<AtomicBool>,
}

impl ProtocolServer {
    pub(crate) fn new(
        encoding: PositionEncoding,
        mut engine: Engine,
        sink: Box<dyn PreviewSink>,
    ) -> Self {
        let events = engine.subscribe();
        let woken = Arc::new(AtomicBool::new(false));
        let wake = Arc::clone(&woken);
        engine.set_waker(move || wake.store(true, Ordering::Relaxed));
        Self {
            engine,
            events,
            encoding,
            open: BTreeMap::new(),
            sink,
            live_previews: BTreeSet::new(),
            checked: BTreeSet::new(),
            woken,
        }
    }

    /// Opens the workspace folders that exist and waits for them to be indexed, so the first
    /// requests already see every page. Returns why the workspace could not be opened.
    /// Opens and indexes `roots`, and gives the problems to show the user: why the workspace
    /// could not be opened, or what the settings left out (a settings file that does not
    /// parse is skipped, the folder still opening, so it can be fixed).
    pub(crate) fn open_workspace(&mut self, roots: Vec<PathBuf>) -> Vec<String> {
        let roots: Vec<PathBuf> = roots.into_iter().filter(|root| root.is_dir()).collect();
        if roots.is_empty() {
            return Vec::new();
        }
        if let Err(error) = self.engine.open_workspace(roots) {
            return vec![format!("adoc-ls could not open the workspace: {error}")];
        }
        self.engine.wait_for_workers(INDEX_TIMEOUT);
        // Nothing is open yet, so there is nothing to publish.
        self.events.try_iter().for_each(drop);
        self.engine
            .settings()
            .warnings
            .iter()
            .map(|warning| format!("adoc-ls: {warning}"))
            .collect()
    }

    /// Whether this build exports PDF: the engine has Typst.
    pub(crate) fn can_export(&self) -> bool {
        self.engine.commands().get(ids::EXPORT_PDF_PAGE).is_some()
    }

    /// Lets the engine apply worker results and settled changes, and publishes the
    /// diagnostics that changed.
    pub(crate) fn tick(&mut self) -> Vec<Notification> {
        self.woken.store(false, Ordering::Relaxed);
        self.engine.pump();
        // Also what changed while a request was answered: a command that indexes typing left
        // pending diagnoses the page, and its publish waits for nothing else.
        self.published(None)
    }

    /// The engine's document for `path`, opened from disk when the engine does not have it.
    /// A request that opened it closes it before answering (when `opened`), so the engine holds
    /// only what the client has open.
    fn borrow_document(&mut self, path: &Path) -> Result<Borrowed, adoc_engine::EngineError> {
        if let Some(id) = self.engine.document_for_path(path) {
            return Ok(Borrowed { id, opened: false });
        }
        let id = self.engine.open(path)?;
        Ok(Borrowed { id, opened: true })
    }

    /// Whether the engine has something to do at `now`: a deadline it keeps has passed, or a
    /// worker or the watcher has sent it something since the last tick.
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.woken.load(Ordering::Relaxed) || self.engine.next_due().is_some_and(|due| due <= now)
    }

    fn id_for(&self, uri: &str) -> Option<DocumentId> {
        let id = self.engine.document_for_path(&document_path(uri))?;
        self.open.contains_key(&id).then_some(id)
    }

    /// The parsed text of open document `id`, which diagnostics and positions refer to.
    fn text_of(&self, id: DocumentId) -> Option<&str> {
        Some(&self.engine.document(id)?.parsed().text)
    }

    /// The text positions in the file at `path` refer to: its open buffer, else the indexed
    /// copy, else, for a file the index does not hold, the file. A file changed on disk since
    /// it was indexed gives no text, so the index's offsets map to no position rather than to
    /// the wrong one.
    fn file_text(&self, path: &Path) -> String {
        if let Some(id) = self.engine.document_for_path(path) {
            if let Some(document) = self.engine.document(id) {
                return document.parsed().text.to_string();
            }
        }
        match self.engine.workspace().index.file(path) {
            Some(file) => file
                .text()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default(),
            None => fs::read_to_string(path).unwrap_or_default(),
        }
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
            WillRenameFiles::METHOD => {
                let id = request.id.clone();
                let response = match serde_json::from_value::<RenameFilesParams>(request.params) {
                    Ok(params) => match self.will_rename_files_response(&params) {
                        Ok(edit) => Response::new_ok(id, edit),
                        Err(message) => {
                            Response::new_err(id, ErrorCode::RequestFailed as i32, message)
                        }
                    },
                    Err(error) => {
                        Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string())
                    }
                };
                // Reading a page that changed on disk can change other open pages' diagnostics.
                return (response, self.published(None));
            }
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
            .open_client_text(&document_path(&uri), &params.text_document.text);
        self.open.insert(id, (uri, params.text_document.version));
        self.published(Some(id))
    }

    /// Each change becomes one `edit.replace_range`, its range mapped through the text as the
    /// previous changes left it, which is kept here with its lines as they are applied rather
    /// than copied from the engine and scanned for each.
    fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Result<Vec<Notification>, ServerError> {
        let Some(id) = self.id_for(params.text_document.uri.as_str()) else {
            return Ok(Vec::new());
        };
        let mut text = Lines::owned(
            self.engine
                .document(id)
                .map(adoc_engine::OpenDocument::text_string)
                .unwrap_or_default(),
        );
        for change in params.content_changes {
            // A change that cannot be applied is reported and skipped: one bad notification
            // never ends the server, and LSP offers no way to ask for the whole text again.
            let applied =
                change_range(&text, change, self.encoding).and_then(|(range, replacement)| {
                    self.engine
                        .invoke(
                            ids::EDIT_REPLACE_RANGE,
                            CommandArgs::Text {
                                id,
                                range,
                                text: replacement.clone(),
                            },
                        )
                        .map_err(|error| ServerError::InvalidChange(error.to_string()))?;
                    Ok((range, replacement))
                });
            match applied {
                Ok((range, replacement)) => text.replace(range, &replacement),
                Err(error) => eprintln!(
                    "adoc-ls: skipped a change to {}: {error}",
                    params.text_document.uri.as_str()
                ),
            }
        }
        if let Some((_, version)) = self.open.get_mut(&id) {
            *version = params.text_document.version;
        }
        Ok(self.published_after_edit(id))
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
        for id in self.updated() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        self.notifications_for(ids)
    }

    /// After an edit to `edited`, diagnostics for every open document the engine says changed,
    /// `edited` first. A typed edit's diagnosis waits until typing pauses, and until then the
    /// document holds the previous text's diagnostics, which are not the new version's: it is
    /// published once diagnosed.
    fn published_after_edit(&mut self, edited: DocumentId) -> Vec<Notification> {
        let mut ids = self.updated();
        if let Some(index) = ids.iter().position(|id| *id == edited) {
            ids[..=index].rotate_right(1);
        }
        self.notifications_for(ids)
    }

    /// The documents whose diagnostics or misspellings the engine says changed since last
    /// asked, each once, in the order it said so.
    fn updated(&mut self) -> Vec<DocumentId> {
        let mut ids: Vec<DocumentId> = Vec::new();
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
        ids
    }

    fn notifications_for(&self, ids: Vec<DocumentId>) -> Vec<Notification> {
        ids.into_iter()
            .filter_map(|id| self.diagnostics_notification(id))
            .collect()
    }

    fn diagnostics_notification(&self, id: DocumentId) -> Option<Notification> {
        let (uri, version) = self.open.get(&id)?;
        let uri = Uri::from_str(uri).ok()?;
        let document = self.engine.document(id)?;
        let lines = Lines::new(&document.parsed().text);
        let mut diagnostics: Vec<Diagnostic> = document
            .diagnostics
            .iter()
            .filter_map(|diagnostic| self.lsp_diagnostic(&uri, &lines, diagnostic))
            .collect();
        diagnostics.extend(
            self.engine
                .misspellings(id)
                .iter()
                .filter_map(|misspelling| {
                    Some(Diagnostic {
                        range: self.encoding.range(&lines, misspelling.range)?,
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

    /// `diagnostic`, in the file at `uri` whose text is `lines`, as LSP has it; none when its
    /// range is not in that text.
    fn lsp_diagnostic(
        &self,
        uri: &Uri,
        lines: &Lines<'_>,
        diagnostic: &adoc_core::Diagnostic,
    ) -> Option<Diagnostic> {
        Some(Diagnostic {
            range: self.encoding.range(lines, diagnostic.range)?,
            severity: Some(match diagnostic.severity {
                CoreDiagnosticSeverity::Information => DiagnosticSeverity::INFORMATION,
                CoreDiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                CoreDiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
            }),
            code: Some(NumberOrString::String(diagnostic.code.as_str().to_owned())),
            source: Some("adoc-ls".to_owned()),
            message: diagnostic.message.clone(),
            related_information: (!diagnostic.related.is_empty()).then(|| {
                diagnostic
                    .related
                    .iter()
                    .filter_map(|related| {
                        Some(DiagnosticRelatedInformation {
                            location: Location::new(
                                uri.clone(),
                                self.encoding.range(lines, related.range)?,
                            ),
                            message: related.message.clone(),
                        })
                    })
                    .collect()
            }),
            ..Diagnostic::default()
        })
    }

    pub(crate) fn document_symbol_response(
        &self,
        params: &DocumentSymbolParams,
    ) -> Option<DocumentSymbolResponse> {
        let id = self.id_for(params.text_document.uri.as_str())?;
        let lines = Lines::new(self.text_of(id)?);
        Some(DocumentSymbolResponse::Nested(
            self.engine
                .outline(id)
                .into_iter()
                .filter_map(|symbol| self.lsp_document_symbol(&lines, symbol))
                .collect(),
        ))
    }

    #[allow(deprecated)]
    fn lsp_document_symbol(
        &self,
        lines: &Lines<'_>,
        symbol: adoc_engine::DocumentSymbol,
    ) -> Option<DocumentSymbol> {
        let children: Vec<_> = symbol
            .children
            .into_iter()
            .filter_map(|child| self.lsp_document_symbol(lines, child))
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
            range: self.encoding.range(lines, symbol.range)?,
            selection_range: self.encoding.range(lines, symbol.selection_range)?,
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
        // Only the kinds asked for: an action of kind `a.b` is one of `a`.
        if let Some(only) = &params.context.only {
            actions.retain(|action| {
                let kind = match action {
                    CodeActionOrCommand::CodeAction(action) => action.kind.as_ref(),
                    CodeActionOrCommand::Command(_) => None,
                }
                .map_or("", CodeActionKind::as_str);
                only.iter().any(|wanted| {
                    kind == wanted.as_str()
                        || kind.strip_prefix(wanted.as_str()).is_some_and(|rest| {
                            !wanted.as_str().is_empty() && rest.starts_with('.')
                        })
                })
            });
        }
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
        let lines = Lines::new(text);
        let (Some(start), Some(end)) = (
            self.encoding.offset(&lines, params.range.start),
            self.encoding.offset(&lines, params.range.end),
        ) else {
            return Vec::new();
        };
        let user = self.engine.settings().user_file().is_some();
        let mut actions = Vec::new();
        for misspelling in self.engine.misspellings(id) {
            if misspelling.range.start > end || misspelling.range.end < start {
                continue;
            }
            let Some(range) = self.encoding.range(&lines, misspelling.range) else {
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
        // The offsets are the page's when the action was listed. Misspellings move with edits,
        // so a range that is no longer one names other text, which never goes in a word list.
        if !self
            .engine
            .misspellings(id)
            .iter()
            .any(|misspelling| misspelling.range == range)
        {
            return (
                Response::new_err(
                    request,
                    ErrorCode::RequestFailed as i32,
                    "the page has changed since the word was found; ask for the action again"
                        .to_owned(),
                ),
                Vec::new(),
            );
        }
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
        let mut opened = None;
        let target = if args.component {
            let Some(context) = self.engine.workspace().antora.context_for_path(&path) else {
                return failed(format!("{} is not in an Antora component", path.display()));
            };
            ExportTarget::Component {
                name: context.component,
                version: context.version,
            }
        } else {
            match self.borrow_document(&path) {
                Ok(page) => {
                    opened = page.opened.then_some(page.id);
                    ExportTarget::Page(page.id)
                }
                Err(error) => return failed(error.to_string()),
            }
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
        let exported = self.engine.export_blocking(export, EXPORT_TIMEOUT);
        if let Some(page) = opened {
            let _ = self.engine.close(page);
        }
        match exported {
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

    /// `adoc.checkWorkspace`: every file's diagnostics published, open or not, and a count. A
    /// closed file an earlier check published problems for gets an empty list once it has none.
    fn check_workspace(&mut self, id: lsp_server::RequestId) -> (Response, Vec<Notification>) {
        let mut notifications = Vec::new();
        let mut problems = 0;
        let mut checked = BTreeSet::new();
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
            let lines = Lines::new(&text);
            problems += diagnostics.len();
            let diagnostics = diagnostics
                .iter()
                .filter_map(|diagnostic| self.lsp_diagnostic(&uri, &lines, diagnostic))
                .collect();
            notifications.push(Notification::new(
                PublishDiagnostics::METHOD.to_owned(),
                PublishDiagnosticsParams::new(uri, diagnostics, None),
            ));
            checked.insert(path);
        }
        let earlier = std::mem::replace(&mut self.checked, checked);
        for path in earlier.difference(&self.checked) {
            // An open file's diagnostics, spelling hints included, are published as it changes.
            let open = self
                .engine
                .document_for_path(path)
                .is_some_and(|open| self.open.contains_key(&open));
            if let Some(uri) = path_to_uri(path).filter(|_| !open) {
                notifications.push(Notification::new(
                    PublishDiagnostics::METHOD.to_owned(),
                    PublishDiagnosticsParams::new(uri, Vec::new(), None),
                ));
            }
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
        let offset = self
            .encoding
            .offset(&Lines::new(self.text_of(id)?), position.position)?;
        let location = match self.engine.definition(id, offset)? {
            DefinitionTarget::Location(location) => location,
            // An image or attachment: the file itself, at its start, which needs no reading.
            DefinitionTarget::External(path) => {
                return Some(GotoDefinitionResponse::Scalar(Location::new(
                    path_to_uri(&path)?,
                    lsp_types::Range::default(),
                )))
            }
        };
        Some(GotoDefinitionResponse::Scalar(
            self.lsp_locations(std::slice::from_ref(&location)).pop()?,
        ))
    }

    /// `locations` as LSP has them, each file's text read once; a location whose range is not
    /// in its file's text is left out.
    fn lsp_locations(&self, locations: &[adoc_engine::Location]) -> Vec<Location> {
        let texts: HashMap<&Path, String> = locations
            .iter()
            .map(|location| location.path.as_path())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|path| (path, self.file_text(path)))
            .collect();
        let lines: HashMap<&Path, Lines<'_>> = texts
            .iter()
            .map(|(path, text)| (*path, Lines::new(text)))
            .collect();
        locations
            .iter()
            .filter_map(|location| {
                Some(Location::new(
                    path_to_uri(&location.path)?,
                    self.encoding
                        .range(lines.get(location.path.as_path())?, location.range)?,
                ))
            })
            .collect()
    }

    fn references_response(&self, params: &ReferenceParams) -> Option<Vec<Location>> {
        let position = &params.text_document_position;
        let id = self.id_for(position.text_document.uri.as_str())?;
        let offset = self
            .encoding
            .offset(&Lines::new(self.text_of(id)?), position.position)?;
        Some(self.lsp_locations(&self.engine.references(id, offset)))
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
                    .offset(&Lines::new(self.text_of(document)?), position.position)?;
                self.engine
                    .anchor_rename_edits(document, offset, &params.new_name)
            });
        match edits.map(|edits| self.workspace_edit(&edits)) {
            Some(Ok(edit)) => Response::new_ok(id, edit),
            Some(Err(message)) => Response::new_err(id, ErrorCode::RequestFailed as i32, message),
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
    /// that names it. The client moves the files. Refused, with the reason, when the edits
    /// cannot be worked out, so the client never moves a page leaving its links behind unsaid.
    fn will_rename_files_response(
        &mut self,
        params: &RenameFilesParams,
    ) -> Result<Option<WorkspaceEdit>, String> {
        let mut edits: Vec<FileEdits> = Vec::new();
        let mut refused = None;
        for rename in &params.files {
            let from = document_path(&rename.old_uri);
            let to = document_path(&rename.new_uri);
            if !adoc_index::is_asciidoc_path(&from) {
                continue;
            }
            let Ok(page) = self.borrow_document(&from) else {
                continue;
            };
            match self.engine.rename_preview(page.id, &to) {
                Ok(preview) => {
                    let preview_id = preview.id;
                    edits.extend(preview.edits.clone());
                    self.engine.discard_preview(preview_id);
                }
                Err(error) => {
                    refused.get_or_insert(error.to_string());
                }
            }
            if page.opened {
                let _ = self.engine.close(page.id);
            }
        }
        if let Some(reason) = refused {
            return Err(reason);
        }
        if edits.is_empty() {
            return Ok(None);
        }
        self.workspace_edit(&edits).map(Some)
    }

    // `WorkspaceEdit` is keyed by `Uri`, which clippy counts as mutable; it is never mutated.
    /// The engine's edits as LSP edits, each file's ranges mapped through `base`, the text they
    /// are offsets into. Refused, naming the file, when that is not the text the client edits:
    /// a file changed on disk since it was indexed (whose `base` is then empty), or an open
    /// buffer the index has not caught up with.
    #[allow(clippy::mutable_key_type)]
    fn workspace_edit(&self, files: &[FileEdits]) -> Result<WorkspaceEdit, String> {
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        for file in files {
            let Some(uri) = path_to_uri(&file.path) else {
                continue;
            };
            let buffer_differs = self
                .engine
                .document_for_path(&file.path)
                .and_then(|id| self.engine.document(id))
                .is_some_and(|document| document.text != file.base.as_str());
            let lines = Lines::new(&file.base);
            let mapped: Option<Vec<TextEdit>> = file
                .edits
                .iter()
                .map(|edit| {
                    Some(TextEdit {
                        range: self.encoding.range(&lines, edit.range)?,
                        new_text: edit.replacement.clone(),
                    })
                })
                .collect();
            match mapped.filter(|_| !buffer_differs) {
                Some(edits) => changes.entry(uri).or_default().extend(edits),
                None => {
                    return Err(format!(
                        "{} changed since it was read; try again in a moment",
                        file.path.display()
                    ))
                }
            }
        }
        Ok(WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        })
    }

    #[allow(deprecated)]
    pub(crate) fn workspace_symbol_response(
        &self,
        params: &WorkspaceSymbolParams,
    ) -> Option<WorkspaceSymbolResponse> {
        let found = self.engine.workspace_symbols(&params.query);
        // Each file's text once: the open buffer, else the file as indexed. The index keeps
        // no text of its own for files read from disk.
        let texts: HashMap<&Path, String> = found
            .iter()
            .map(|symbol| &*symbol.path)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|path| (path, self.file_text(path)))
            .collect();
        let lines: HashMap<&Path, Lines<'_>> = texts
            .iter()
            .map(|(path, text)| (*path, Lines::new(text)))
            .collect();
        let symbols = found
            .iter()
            .filter_map(|symbol| {
                let lines = lines.get(&*symbol.path)?;
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
                        self.encoding.range(lines, symbol.range)?,
                    ),
                    container_name: symbol.container.as_deref().map(str::to_owned),
                })
            })
            .collect();
        Some(WorkspaceSymbolResponse::Flat(symbols))
    }

    fn hover_response(&self, params: &HoverParams) -> Option<Hover> {
        let position = &params.text_document_position_params;
        let id = self.id_for(position.text_document.uri.as_str())?;
        let lines = Lines::new(self.text_of(id)?);
        let offset = self.encoding.offset(&lines, position.position)?;
        let content = self.engine.hover(id, offset)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content.markdown,
            }),
            range: self.encoding.range(&lines, content.range),
        })
    }

    fn completion_response(&self, params: &CompletionParams) -> Option<CompletionResponse> {
        let position = &params.text_document_position;
        let id = self.id_for(position.text_document.uri.as_str())?;
        let lines = Lines::new(self.text_of(id)?);
        let offset = self.encoding.offset(&lines, position.position)?;
        let items = self
            .engine
            .completions(id, offset)
            .into_iter()
            .filter_map(|candidate| self.completion_item(&lines, candidate))
            .collect();
        Some(CompletionResponse::List(CompletionList {
            // The candidate set narrows as the target grows, so the client must ask again.
            is_incomplete: true,
            items,
        }))
    }

    fn completion_item(&self, lines: &Lines<'_>, candidate: Candidate) -> Option<CompletionItem> {
        let range = self.encoding.range(lines, candidate.range)?;
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
    text: &Lines<'_>,
    change: TextDocumentContentChangeEvent,
    encoding: PositionEncoding,
) -> Result<(adoc_core::SourceRange, String), ServerError> {
    let Some(range) = change.range else {
        return Ok((
            adoc_core::SourceRange::new(0, text.text().len()),
            change.text,
        ));
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

/// The workspace folders the client sends, else its root URI, else its root path.
#[allow(deprecated)]
fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    if let Some(folders) = params
        .workspace_folders
        .as_ref()
        .filter(|folders| !folders.is_empty())
    {
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
            DocumentSymbolRequest, ExecuteCommand, GotoDefinition, Initialize, Rename,
            Request as LspRequest, Shutdown,
        },
        ClientCapabilities, CodeActionContext, CodeActionOrCommand, CodeActionParams, Diagnostic,
        DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
        DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
        ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse, InitializeParams,
        InitializedParams, Location, NumberOrString, PartialResultParams, Position,
        PublishDiagnosticsParams, Range, RenameParams, SymbolKind, TextDocumentContentChangeEvent,
        TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
        VersionedTextDocumentIdentifier, WorkDoneProgressParams, WorkspaceEdit, WorkspaceFolder,
        WorkspaceSymbolParams, WorkspaceSymbolResponse,
    };

    use adoc_engine::{Engine, Settings};

    use crate::{
        position::{Lines, PositionEncoding},
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
            &Lines::new("A😀B\n"),
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
    fn each_change_in_a_notification_maps_through_the_text_the_last_one_left() {
        let mut server = server();
        open(&mut server, PAGE, "first\n");
        let change =
            |range: Option<((u32, u32), (u32, u32))>, text: &str| TextDocumentContentChangeEvent {
                range: range.map(|((start_line, start), (end_line, end))| {
                    Range::new(
                        Position::new(start_line, start),
                        Position::new(end_line, end),
                    )
                }),
                range_length: None,
                text: text.to_owned(),
            };
        server
            .handle_notification(Notification::new(
                DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier::new(PAGE.parse().unwrap(), 2),
                    content_changes: vec![
                        // The whole text, then lines that exist only after the change before.
                        change(None, "a\r\nb\n"),
                        change(Some(((0, 0), (0, 0))), "x\ny\n"),
                        change(Some(((2, 0), (2, 1))), "😀"),
                        change(Some(((2, 2), (2, 2))), "z"),
                        // Joining two lines, then a line's end before its `\r\n`.
                        change(Some(((1, 1), (2, 0))), ""),
                        change(Some(((1, 9), (2, 0))), "-"),
                        // Past the last line: the end of the text.
                        change(Some(((5, 0), (5, 0))), "!"),
                        // Inside the surrogate pair: skipped.
                        change(Some(((1, 2), (1, 2))), "?"),
                    ],
                },
            ))
            .unwrap();

        let id = server.id_for(PAGE).unwrap();
        assert_eq!(
            server.engine.document(id).unwrap().text_string(),
            "x\ny😀z-b\n!"
        );
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
        // The first declaration, for the client to link to.
        let related = duplicate
            .related_information
            .as_ref()
            .expect("the first declaration is related");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].location.uri, uri);
        assert_eq!(related[0].location.range.start.line, 2);

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
    fn renames_an_anchor_in_the_text_just_typed() {
        let page = file_uri(&std::env::temp_dir().join("docs").join("rename.adoc"));
        let mut server = server();
        open(&mut server, page.as_str(), "[[here]]\n\nSee <<here>>.\n");
        // A small change, so re-indexing waits for typing to pause.
        server
            .handle_notification(Notification::new(
                DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier::new(page.clone(), 2),
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(2, 0), Position::new(2, 0))),
                        range_length: None,
                        text: "X".to_owned(),
                    }],
                },
            ))
            .unwrap();

        let (response, _) = server.handle_request(Request::new(
            RequestId::from(1),
            Rename::METHOD.to_owned(),
            RenameParams {
                text_document_position: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(page.clone()),
                    Position::new(0, 3),
                ),
                new_name: "there".to_owned(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        ));
        let edit: WorkspaceEdit =
            serde_json::from_value(response.response_result.unwrap()).unwrap();
        let mut edits = edit.changes.unwrap().remove(&page).unwrap();
        edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));
        let ranges: Vec<Range> = edits.iter().map(|edit| edit.range).collect();
        assert_eq!(
            ranges,
            [
                Range::new(Position::new(0, 2), Position::new(0, 6)),
                Range::new(Position::new(2, 7), Position::new(2, 11)),
            ],
            "the anchor and the reference in `XSee <<here>>.`"
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
    fn finds_workspace_symbols_in_pages_that_are_not_open() {
        let directory = tempfile::tempdir().unwrap();
        let page = directory.path().join("page.adoc");
        // Lines before the section, with a non-BMP character, so its position is worked out
        // from the file's text.
        fs::write(&page, "= 😀 Guide\n\nIntro.\n\n== Authentication Flow\n").unwrap();
        let mut server = server();
        server.open_workspace(vec![directory.path().to_path_buf()]);

        let Some(WorkspaceSymbolResponse::Flat(symbols)) =
            server.workspace_symbol_response(&WorkspaceSymbolParams {
                query: "auth".to_owned(),
                ..WorkspaceSymbolParams::default()
            })
        else {
            panic!("expected a flat symbol list");
        };

        assert_eq!(symbols.len(), 1, "{symbols:?}");
        assert_eq!(symbols[0].name, "Authentication Flow");
        // The title, after `== `.
        assert_eq!(symbols[0].location.range.start, Position::new(4, 3));
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

    use crate::spawn_guard;

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

    /// The `(uri, diagnostic codes)` of each publish, in order.
    fn publishes(notifications: Vec<Notification>) -> Vec<(String, Vec<String>)> {
        notifications
            .into_iter()
            .filter(|notification| notification.method == PublishDiagnostics::METHOD)
            .map(|notification| {
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

    #[test]
    fn checking_the_workspace_again_clears_a_closed_file_that_was_fixed() {
        let directory = tempfile::tempdir().unwrap();
        let page = directory.path().join("page.adoc");
        fs::write(&page, "= Page\n\nSee xref:missing.adoc[].\n").unwrap();
        let mut server = server();
        server.open_workspace(vec![directory.path().to_path_buf()]);
        let uri = file_uri(&page).as_str().to_owned();

        let (_, first) = server.check_workspace(RequestId::from(1));
        assert_eq!(
            publishes(first),
            [(uri.clone(), vec!["adoc.unresolved-xref-file".to_owned()])]
        );

        fs::write(directory.path().join("missing.adoc"), "= Found\n").unwrap();
        server.engine.reindex();
        assert!(server
            .engine
            .wait_for_workers(std::time::Duration::from_secs(10)));
        server.tick();

        let (response, second) = server.check_workspace(RequestId::from(2));
        assert_eq!(
            response.response_result.unwrap(),
            serde_json::json!({ "files": 0, "problems": 0 })
        );
        assert_eq!(publishes(second), [(uri, Vec::new())], "cleared");
    }

    #[test]
    fn checking_the_workspace_again_leaves_a_file_opened_since_to_its_own_publishes() {
        let directory = tempfile::tempdir().unwrap();
        let page = directory.path().join("page.adoc");
        fs::write(&page, "= Page\n\nSee xref:missing.adoc[].\n").unwrap();
        let mut server = server();
        server.open_workspace(vec![directory.path().to_path_buf()]);
        let uri = file_uri(&page).as_str().to_owned();
        let (_, first) = server.check_workspace(RequestId::from(1));
        assert_eq!(publishes(first).len(), 1);

        // Opened and fixed in the client: its own publishes, spelling hints and all, cover it.
        open(&mut server, &uri, "= Page\n");
        let (_, second) = server.check_workspace(RequestId::from(2));
        assert_eq!(publishes(second), []);
    }

    #[test]
    fn a_change_is_never_saved_for_the_client_whatever_the_settings_say() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("settings.toml");
        fs::write(&user, "[editor]\nauto_save = \"after_delay\"\n").unwrap();
        let path = directory.path().join("page.adoc");
        fs::write(&path, "= Page\n").unwrap();
        let clock = adoc_engine::ManualClock::new();
        let mut server = ProtocolServer::new(
            PositionEncoding::Utf16,
            Engine::with_clock(
                super::lsp_settings(Some(&user)).0,
                std::sync::Arc::new(clock.clone()),
            ),
            Box::new(BrowserSink::new(
                directory.path().join("previews"),
                Unopened,
            )),
        );
        let uri = file_uri(&path);
        open(&mut server, uri.as_str(), "= Page\n");

        server
            .handle_notification(Notification::new(
                DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier::new(uri, 2),
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(1, 0), Position::new(1, 0))),
                        range_length: None,
                        text: "\nTyped.\n".to_owned(),
                    }],
                },
            ))
            .unwrap();
        clock.advance(adoc_engine::AUTO_SAVE_DELAY * 2);
        server.tick();
        assert!(server
            .engine
            .wait_for_workers(std::time::Duration::from_secs(10)));

        assert_eq!(fs::read_to_string(&path).unwrap(), "= Page\n");
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

    /// The ranges of each publish for `uri` with its version, in order.
    fn published_ranges(
        notifications: Vec<Notification>,
        uri: &str,
    ) -> Vec<(Option<i32>, Vec<Range>)> {
        notifications
            .into_iter()
            .filter(|notification| notification.method == PublishDiagnostics::METHOD)
            .map(|notification| {
                serde_json::from_value::<PublishDiagnosticsParams>(notification.params).unwrap()
            })
            .filter(|params| params.uri.as_str() == uri)
            .map(|params| {
                (
                    params.version,
                    params
                        .diagnostics
                        .into_iter()
                        .map(|diagnostic| diagnostic.range)
                        .collect(),
                )
            })
            .collect()
    }

    /// A typed character defers the page's diagnostics until typing pauses; until then the
    /// previous text's are not published again under the new version at shifted places.
    #[test]
    fn a_typed_change_publishes_only_diagnostics_of_the_text_it_made() {
        let clock = adoc_engine::ManualClock::new();
        let mut server = ProtocolServer::new(
            PositionEncoding::Utf16,
            Engine::with_clock(Settings::default(), std::sync::Arc::new(clock.clone())),
            Box::new(BrowserSink::new(
                std::env::temp_dir().join("adoc-ls-test-previews"),
                Unopened,
            )),
        );
        open(&mut server, PAGE, "= Guide\n\nSee <<missing>>.\n");
        let warning = Range::new(Position::new(2, 4), Position::new(2, 15));

        let typed = server
            .handle_notification(Notification::new(
                DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier::new(PAGE.parse().unwrap(), 2),
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(0, 0), Position::new(0, 0))),
                        range_length: None,
                        text: "X".to_owned(),
                    }],
                },
            ))
            .unwrap();
        for (_, ranges) in published_ranges(typed, PAGE) {
            assert_eq!(ranges, [warning], "a stale publish");
        }

        clock.advance(adoc_engine::INDEX_SETTLE * 2);
        let mut settled = server.tick();
        assert!(server
            .engine
            .wait_for_workers(std::time::Duration::from_secs(10)));
        settled.extend(server.tick());
        assert_eq!(
            published_ranges(settled, PAGE),
            [(Some(2), vec![warning])],
            "published once diagnosed"
        );
    }

    /// `adoc.spelling.addToWorkspace [uri, start, end]`.
    fn add_to_workspace(
        server: &mut ProtocolServer,
        uri: &Uri,
        start: usize,
        end: usize,
    ) -> lsp_server::Response {
        server
            .execute_command_response(Request::new(
                RequestId::from(7),
                ExecuteCommand::METHOD.to_owned(),
                ExecuteCommandParams {
                    command: crate::capabilities::ADD_TO_WORKSPACE_COMMAND.to_owned(),
                    arguments: vec![
                        serde_json::json!(uri.as_str()),
                        serde_json::json!(start),
                        serde_json::json!(end),
                    ],
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            ))
            .0
    }

    /// A page with its misspellings checked, in a workspace.
    fn checked_page(text: &str) -> (tempfile::TempDir, std::path::PathBuf, Uri, ProtocolServer) {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        let page = root.join("page.adoc");
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
        (directory, root, uri, server)
    }

    #[test]
    fn adding_a_word_at_offsets_inside_a_character_answers_an_error() {
        let (_directory, root, uri, mut server) = checked_page("= Page\n\né qwzxy\n");

        // Byte 9 is inside `é`.
        let response = add_to_workspace(&mut server, &uri, 9, 10);

        assert!(response.response_result.is_err(), "{response:?}");
        assert!(!root.join(adoc_engine::WORKSPACE_WORDS_FILE).exists());
        // And the server goes on answering.
        assert!(!document_symbols_of(&server, uri.as_str()).is_empty());
    }

    #[test]
    fn adding_a_word_from_a_stale_range_adds_nothing() {
        let text = "= Page\n\nWe recieve it.\n";
        let (_directory, root, uri, mut server) = checked_page(text);
        let start = text.find("recieve").unwrap();
        // The action was listed; then `é` is typed before the word.
        server
            .handle_notification(Notification::new(
                DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier::new(uri.clone(), 2),
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(2, 3), Position::new(2, 3))),
                        range_length: None,
                        text: "é".to_owned(),
                    }],
                },
            ))
            .unwrap();

        let response = add_to_workspace(&mut server, &uri, start, start + "recieve".len());

        assert!(response.response_result.is_err(), "{response:?}");
        assert!(!root.join(adoc_engine::WORKSPACE_WORDS_FILE).exists());
    }

    /// `adoc.exportPdf { uri, output }`.
    fn export(server: &mut ProtocolServer, page: &Path, output: &Path) -> lsp_server::Response {
        server
            .execute_command_response(Request::new(
                RequestId::from(9),
                ExecuteCommand::METHOD.to_owned(),
                ExecuteCommandParams {
                    command: crate::capabilities::EXPORT_PDF_COMMAND.to_owned(),
                    arguments: vec![serde_json::json!({
                        "uri": file_uri(page).as_str(),
                        "output": output,
                    })],
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            ))
            .0
    }

    #[test]
    fn exporting_a_page_the_client_has_not_opened_leaves_it_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        let page = root.join("old.adoc");
        fs::write(&page, "= Old\n\nA paragraph.\n").unwrap();
        let mut server = server();
        server.open_workspace(vec![root.clone()]);

        let response = export(&mut server, &page, &root.join("old.pdf"));
        assert!(response.response_result.is_ok(), "{response:?}");
        assert_eq!(server.engine.document_for_path(&page), None);

        // And so on failure: the output is a folder.
        let folder = root.join("folder.pdf");
        fs::create_dir(&folder).unwrap();
        let response = export(&mut server, &page, &folder);
        assert!(response.response_result.is_err(), "{response:?}");
        assert_eq!(server.engine.document_for_path(&page), None);
    }

    /// A user settings file that does not parse gives the defaults, still reading that file,
    /// so the watcher picks up its correction, and the problem is reported.
    #[test]
    fn a_malformed_user_settings_file_is_kept_and_reported() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("settings.toml");
        fs::write(&user, "[spelling\n").unwrap();

        let (settings, problem) = super::lsp_settings(Some(&user));

        assert_eq!(settings.user_file(), Some(user.as_path()));
        let problem = problem.expect("a problem to show");
        assert!(problem.contains("settings.toml"), "{problem}");
    }

    const LINKING: &str = "= L\n\nSee xref:target.adoc[].\n";

    /// A workspace of `target.adoc` and `linking.adoc`, which links to it, indexed.
    fn renaming_workspace() -> (tempfile::TempDir, std::path::PathBuf, ProtocolServer) {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        fs::write(root.join("target.adoc"), "= Target\n").unwrap();
        fs::write(root.join("linking.adoc"), LINKING).unwrap();
        let mut server = server();
        server.open_workspace(vec![root.clone()]);
        (directory, root, server)
    }

    /// `workspace/willRenameFiles` for `target.adoc` to `renamed.adoc`.
    fn will_rename_target(server: &mut ProtocolServer, root: &Path) -> lsp_server::Response {
        server
            .handle_request(Request::new(
                RequestId::from(11),
                lsp_types::request::WillRenameFiles::METHOD.to_owned(),
                lsp_types::RenameFilesParams {
                    files: vec![lsp_types::FileRename {
                        old_uri: file_uri(&root.join("target.adoc")).as_str().to_owned(),
                        new_uri: file_uri(&root.join("renamed.adoc")).as_str().to_owned(),
                    }],
                },
            ))
            .0
    }

    #[test]
    fn renaming_a_file_named_by_a_file_changed_on_disk_answers_an_error() {
        let (_directory, root, mut server) = renaming_workspace();
        // Rewritten, and not read again yet: the index's offsets name nothing in it.
        fs::write(root.join("linking.adoc"), format!("= Longer\n\n{LINKING}")).unwrap();

        let response = will_rename_target(&mut server, &root);

        let error = response.response_result.expect_err("refused, not null");
        assert!(error.message.contains("linking.adoc"), "{error:?}");
    }

    #[test]
    fn renaming_a_file_just_after_typing_in_a_page_naming_it_edits_the_typed_text() {
        let (_directory, root, mut server) = renaming_workspace();
        let uri = file_uri(&root.join("linking.adoc"));
        open(&mut server, uri.as_str(), LINKING);
        server
            .handle_notification(Notification::new(
                DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier::new(uri.clone(), 2),
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(0, 0), Position::new(0, 0))),
                        range_length: None,
                        text: "X".to_owned(),
                    }],
                },
            ))
            .unwrap();

        let response = will_rename_target(&mut server, &root);

        let edit: WorkspaceEdit =
            serde_json::from_value(response.response_result.expect("edits")).unwrap();
        let edits = &edit.changes.unwrap()[&uri];
        assert_eq!(edits.len(), 1, "{edits:?}");
        // `target.adoc` in `XSee xref:target.adoc[].` is on line 2, characters 9 to 20.
        assert_eq!(
            edits[0].range,
            Range::new(Position::new(2, 9), Position::new(2, 20))
        );
        assert_eq!(edits[0].new_text, "renamed.adoc");
    }

    /// Opening the page to rename reads it as it is on disk now, which can change other open
    /// pages' diagnostics: they are published, not dropped.
    #[test]
    fn renaming_a_file_publishes_what_reading_it_changed() {
        let (_directory, root, mut server) = renaming_workspace();
        fs::write(root.join("target.adoc"), "= Target\n\n[[part]]\n== Part\n").unwrap();
        server.engine.reindex();
        assert!(server
            .engine
            .wait_for_workers(std::time::Duration::from_secs(10)));
        let uri = file_uri(&root.join("linking.adoc"));
        open(
            &mut server,
            uri.as_str(),
            "= L\n\nSee xref:target.adoc#part[].\n",
        );
        // The anchor goes on disk, and nothing has read the file again yet.
        fs::write(root.join("target.adoc"), "= Target\n").unwrap();

        let (_, notifications) = server.handle_request(Request::new(
            RequestId::from(11),
            lsp_types::request::WillRenameFiles::METHOD.to_owned(),
            lsp_types::RenameFilesParams {
                files: vec![lsp_types::FileRename {
                    old_uri: file_uri(&root.join("target.adoc")).as_str().to_owned(),
                    new_uri: file_uri(&root.join("renamed.adoc")).as_str().to_owned(),
                }],
            },
        ));

        assert_eq!(
            publishes(notifications),
            [(
                uri.as_str().to_owned(),
                vec!["adoc.unresolved-anchor".to_owned()]
            )]
        );
    }

    #[allow(deprecated)]
    #[test]
    fn an_empty_list_of_workspace_folders_falls_back_to_the_root_uri() {
        let root = std::env::temp_dir().join("docs");
        let params = InitializeParams {
            workspace_folders: Some(Vec::new()),
            root_uri: Some(file_uri(&root)),
            ..InitializeParams::default()
        };

        assert_eq!(super::workspace_roots(&params), [root]);
    }

    /// The engine is pumped after a message once something it keeps is due, not only after
    /// the connection has been quiet for a tick.
    #[test]
    fn the_engine_is_due_once_typing_settles() {
        use adoc_engine::Clock as _;

        let clock = adoc_engine::ManualClock::new();
        let mut server = ProtocolServer::new(
            PositionEncoding::Utf16,
            Engine::with_clock(Settings::default(), std::sync::Arc::new(clock.clone())),
            Box::new(BrowserSink::new(
                std::env::temp_dir().join("adoc-ls-test-previews"),
                Unopened,
            )),
        );
        open(&mut server, PAGE, "= Guide\n");
        server.tick();
        assert!(!server.due(clock.now()));
        server
            .handle_notification(Notification::new(
                DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier::new(PAGE.parse().unwrap(), 2),
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(0, 0), Position::new(0, 0))),
                        range_length: None,
                        text: "X".to_owned(),
                    }],
                },
            ))
            .unwrap();
        assert!(!server.due(clock.now()), "typing has not settled");

        clock.advance(adoc_engine::INDEX_SETTLE * 2);

        assert!(server.due(clock.now()));
    }

    use lsp_types::CodeActionKind;

    /// The titles of the code actions offered on line 2 of `text` for `only`.
    fn code_action_titles(text: &str, only: Option<Vec<CodeActionKind>>) -> Vec<String> {
        let (_directory, _root, uri, server) = checked_page(text);
        server
            .code_action_response(&CodeActionParams {
                text_document: TextDocumentIdentifier { uri },
                range: Range::new(Position::new(2, 3), Position::new(2, 3)),
                context: CodeActionContext {
                    only,
                    ..CodeActionContext::default()
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .into_iter()
            .map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => action.title,
                CodeActionOrCommand::Command(command) => command.title,
            })
            .collect()
    }

    #[test]
    fn code_actions_are_only_of_the_kinds_asked_for() {
        let text = "= Page\n\nWe recieve it.\n";
        let all = code_action_titles(text, None);
        assert!(all.iter().any(|title| title.contains("preview")), "{all:?}");
        assert!(all.contains(&"Change to 'receive'".to_owned()), "{all:?}");

        let fixes = code_action_titles(text, Some(vec![CodeActionKind::QUICKFIX]));
        assert!(
            fixes.contains(&"Change to 'receive'".to_owned()),
            "{fixes:?}"
        );
        assert!(
            !fixes.iter().any(|title| title.contains("preview")),
            "{fixes:?}"
        );

        assert_eq!(
            code_action_titles(text, Some(vec![CodeActionKind::REFACTOR])),
            Vec::<String>::new()
        );
    }
}
