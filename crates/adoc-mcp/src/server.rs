//! The tools and resources. Each tool locks the engine, does its work through the engine's
//! commands and queries, and answers with JSON, as text and as structured content, or a tool
//! error carrying a stable code and the engine's message (`responses.rs`). A tool waits for the
//! engine for at most [`LOCK_WAIT`], and only [`MAX_WAITING`] requests wait at once, so a
//! stalled request answers others with `timeout` or `unavailable` rather than holding them.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::Receiver,
        Arc, Mutex, MutexGuard, TryLockError,
    },
    time::{Duration, Instant},
};

use adoc_core::{DiagnosticSeverity, SourceRange};
use adoc_engine::{
    commands::ids, report::display_path, to_svg_laid_out, CommandArgs, CommandOutcome,
    DefinitionTarget, Engine, EngineEvent, ExportArgs, ExportTarget, GraphFilter, GraphScope,
    NavItem, SearchRequest, SvgStyle,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult,
        GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
        Resource, ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    paths::confine,
    responses::{answer, for_session, page, paged, structured, ErrorCode, Failure},
    schemas,
};

/// The tools that change files, listed and callable only with `--allow-write`.
const WRITE_TOOLS: [&str; 8] = [
    "edit_page",
    "rename_page",
    "export_pdf",
    "apply_changes",
    "create_page",
    "create_partial",
    "create_module",
    "create_project",
];

/// The tools that run the project's own build tools, listed and callable only with
/// `--allow-build`, which `--allow-write` never implies.
const BUILD_TOOLS: [&str; 1] = ["build_site"];

/// What the server was started allowing beyond reading.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Permissions {
    /// `--allow-write`: the tools in [`WRITE_TOOLS`].
    pub write: bool,
    /// `--allow-build`: the tools in [`BUILD_TOOLS`].
    pub build: bool,
}

/// At most this many items in a list result that is not paged; the total is stated, and
/// `incomplete` set, when there are more.
const MAX_ITEMS: usize = crate::responses::PAGE;

/// At most this many bytes of a text result; the total is stated when there are more.
pub(crate) const MAX_TEXT: usize = 200_000;

/// How long a render, search, Git status or export may take.
const TIMEOUT: Duration = Duration::from_secs(120);

/// How often the engine is pumped between tool calls.
const IDLE_PUMP: Duration = Duration::from_millis(250);

/// How long a request waits for another to finish with the engine before answering `timeout`.
const LOCK_WAIT: Duration = Duration::from_secs(30);

/// How many requests may wait for the engine at once; more are answered `unavailable`.
const MAX_WAITING: usize = 16;

const ASCIIDOC: &str = "text/asciidoc";

pub(crate) struct State {
    pub(crate) engine: Engine,
    events: Receiver<EngineEvent>,
    pub(crate) roots: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct Server {
    pub(crate) state: Arc<Mutex<State>>,
    tool_router: ToolRouter<Self>,
    /// [`LOCK_WAIT`], shorter in tests.
    lock_wait: Duration,
    /// The requests waiting for or using the engine.
    waiting: Arc<AtomicUsize>,
    permissions: Permissions,
    /// This session's staged changes: each session (an HTTP session, or the one over stdio)
    /// has its own, though all share the engine.
    pub(crate) previews: Arc<Mutex<crate::changes::Previews>>,
    /// This session's resource subscriptions.
    subscriptions: Arc<Mutex<crate::subscriptions::Subscriptions>>,
    /// Every session's, for the idle pump to check.
    registry: Arc<Mutex<crate::subscriptions::Registry>>,
}

/// A session's previews, poisoned or not.
pub(crate) fn lock_previews(
    previews: &Mutex<crate::changes::Previews>,
) -> MutexGuard<'_, crate::changes::Previews> {
    previews
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A tool's answer: JSON on success, a tool error with its code otherwise.
pub(crate) type Answer = Result<CallToolResult, ErrorData>;

/// `items`, cut to [`MAX_ITEMS`], under `key`, with the total and `incomplete` when cut.
fn limited<T: serde::Serialize>(key: &str, items: Vec<T>) -> Value {
    let total = items.len();
    let shown: Vec<T> = items.into_iter().take(MAX_ITEMS).collect();
    let mut value = json!({ key: shown });
    if total > MAX_ITEMS {
        value["truncated"] = json!({ "shown": MAX_ITEMS, "total": total });
        value["incomplete"] = json!(true);
    }
    value
}

/// `text`, cut to [`MAX_TEXT`] bytes at a character boundary, and its total length when cut.
pub(crate) fn limited_text(text: &str) -> (String, Option<usize>) {
    if text.len() <= MAX_TEXT {
        return (text.to_owned(), None);
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), Some(text.len()))
}

#[derive(Deserialize, JsonSchema)]
struct GetNavArgs {
    /// The component's name.
    component: String,
    /// The version; the latest when absent.
    version: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadPageArgs {
    /// The page's path, absolute or relative to the first root.
    path: Option<String>,
    /// Or its Antora resource id, `version@component:module:family$path`.
    resource_id: Option<String>,
    /// The page a partial resource id is read from.
    from: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ResolveArgs {
    /// An xref, include or image macro as written, or a bare xref target.
    target: String,
    /// The page the target is written in.
    from: String,
}

#[derive(Deserialize, JsonSchema)]
struct CheckArgs {
    /// Only files beneath this directory.
    scope: Option<String>,
    /// `error` (the default) or `warning`: the severity that makes `failed` true.
    fail_on: Option<String>,
    /// The `next_cursor` of the page before, to list the next; the first page without it.
    cursor: Option<String>,
    /// At most this many diagnostics in the page, up to 500 (the default).
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct SymbolsArgs {
    /// Words the symbols' names start with.
    query: String,
    /// The `next_cursor` of the page before, to list the next; the first page without it.
    cursor: Option<String>,
    /// At most this many symbols in the page, up to 500 (the default).
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    whole_word: bool,
    /// Only files beneath this directory.
    scope: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct LinkGraphArgs {
    /// `workspace` (the default), `page` for a page and its neighbours, or `path` for the
    /// shortest path from the page `path` to the page `to`.
    scope: Option<String>,
    /// The page of a `page` scope, or where a `path` scope starts; absolute or relative to the
    /// first root.
    path: Option<String>,
    /// Where a `path` scope ends.
    to: Option<String>,
    /// Whether a `path` scope follows links only in their direction, as a reader does; true by
    /// default, false to follow them either way.
    directed: Option<bool>,
    /// How many links from the page a `page` scope reaches, 1 (the default) to 3.
    depth: Option<u8>,
    /// Show partials and include links; true by default.
    partials: Option<bool>,
    /// Show navigation files and their links; true by default.
    nav: Option<bool>,
    /// Show pages with no links at all; true by default.
    orphans: Option<bool>,
    /// Show targets that do not resolve; false by default.
    unresolved: Option<bool>,
    /// `json` (the default), or `svg` for the graph laid out and drawn as an SVG image.
    format: Option<String>,
    /// Draw the SVG for a dark background; false by default.
    dark: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct RenderPageArgs {
    path: String,
    /// `text` (the default) or `html`.
    format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct Range {
    /// Byte offset of the first byte replaced.
    start: usize,
    /// Byte offset just past the last byte replaced.
    end: usize,
}

#[derive(Deserialize, JsonSchema)]
struct Edit {
    /// Text that must occur exactly once in the page; it is replaced.
    old_text: Option<String>,
    /// Or the byte range replaced.
    range: Option<Range>,
    new_text: String,
}

#[derive(Deserialize, JsonSchema)]
struct EditPageArgs {
    path: String,
    /// Applied in order, each against the text the previous ones left.
    edits: Vec<Edit>,
    /// Write the page to disk afterwards; otherwise the edits stay in the engine's buffer.
    #[serde(default)]
    save: bool,
}

#[derive(Deserialize, JsonSchema)]
struct RenamePageArgs {
    path: String,
    /// The new file name, or a path relative to the page's directory.
    new_name: String,
}

#[derive(Deserialize, JsonSchema)]
struct ExportPdfArgs {
    /// The page to export as an article.
    path: Option<String>,
    /// Or a component version to export as a book: `name` or `version@name`.
    component: Option<String>,
    /// The PDF to write, inside a root.
    output: String,
    theme: Option<String>,
    /// `pdf-a-1b`, `pdf-a-1a`, `pdf-a-2a`, `pdf-ua-1`.
    #[serde(default)]
    standards: Vec<String>,
}

impl State {
    /// `index_incomplete` while a whole index of the workspace is running, so a list made from
    /// the index is never taken for the whole workspace.
    pub(crate) fn indexed(&self) -> Result<(), Failure> {
        if self.engine.is_indexing_workspace() {
            return Err(Failure::new(
                ErrorCode::IndexIncomplete,
                "the workspace is still being indexed; try again shortly",
            ));
        }
        Ok(())
    }

    pub(crate) fn confine(&self, path: &str) -> Result<PathBuf, Failure> {
        confine(&self.roots, path)
    }

    /// The file a tool names by exactly one of `path` or `resource_id` (read from the page
    /// `from` when given), inside the roots. Naming both, or neither, is refused rather than
    /// one chosen.
    pub(crate) fn target(
        &self,
        path: Option<&str>,
        resource_id: Option<&str>,
        from: Option<&str>,
    ) -> Result<PathBuf, Failure> {
        match (path, resource_id) {
            (Some(path), None) => self.confine(path),
            (None, Some(id)) => {
                let from = from.map(|from| self.confine(from)).transpose()?;
                let path = self
                    .engine
                    .resolve_resource_id(id, from.as_deref())
                    .map_err(|error| match error {
                        adoc_engine::EngineError::InvalidRequest(_) => Failure::from(error),
                        _ => Failure::not_found(error.to_string()),
                    })?;
                self.confine(&path.to_string_lossy())
            }
            (Some(_), Some(_)) => Err(Failure::invalid(
                "give a `path` or a `resource_id`, not both",
            )),
            (None, None) => Err(Failure::invalid("give a `path` or a `resource_id`")),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.roots[0]
    }

    /// A path as the results show it: relative to the first root.
    pub(crate) fn shown(&self, path: &Path) -> String {
        display_path(path, self.root())
    }

    /// The text of the page at `path`: its buffer when open, else the file.
    pub(crate) fn text(&self, path: &Path) -> Result<String, Failure> {
        if let Some(id) = self.engine.document_for_path(path) {
            if let Some(document) = self.engine.document(id) {
                return Ok(document.text_string());
            }
        }
        std::fs::read_to_string(path).map_err(|error| {
            let message = format!("{}: {error}", self.shown(path));
            match error.kind() {
                std::io::ErrorKind::NotFound => Failure::not_found(message),
                _ => Failure::internal(message),
            }
        })
    }

    /// Opens `path` if it is not open; returns its id and whether this opened it.
    fn open(&mut self, path: &Path) -> Result<(adoc_engine::DocumentId, bool), Failure> {
        if let Some(id) = self.engine.document_for_path(path) {
            return Ok((id, false));
        }
        let id = self.engine.open(path)?;
        Ok((id, true))
    }

    fn nav_item(&self, item: &NavItem) -> Value {
        let resource_id = item.target.as_ref().map(adoc_engine::resource_id_string);
        let page = resource_id
            .as_deref()
            .and_then(|id| self.engine.resolve_resource_id(id, None).ok())
            .map(|path| self.shown(&path));
        json!({
            "title": item.title,
            "resource_id": resource_id,
            "page": page,
            "children": item.children.iter().map(|child| self.nav_item(child)).collect::<Vec<_>>(),
        })
    }
}

impl Server {
    /// Pumps the engine every [`IDLE_PUMP`] while no tool is using it, so files changed on disk
    /// are applied between calls, and drops the events no tool reads, so neither the watcher's
    /// channel nor the events pile up on a long-running server.
    pub async fn pump_while_idle(self) {
        let mut interval = tokio::time::interval(IDLE_PUMP);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let (state, registry) = (self.state.clone(), self.registry.clone());
            let due = tokio::task::spawn_blocking(move || {
                let revision = {
                    let mut state = match state.try_lock() {
                        Ok(state) => state,
                        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                        // A tool has the engine, and pumps it as it needs.
                        Err(std::sync::TryLockError::WouldBlock) => return Vec::new(),
                    };
                    state.engine.pump();
                    state.events.try_iter().for_each(drop);
                    state.engine.workspace_revision()
                };
                // Read with the engine free: subscribed files on disk, if the workspace moved.
                crate::subscriptions::due(&registry, revision, Instant::now())
            })
            .await
            .unwrap_or_default();
            crate::subscriptions::announce(due).await;
        }
    }
}

/// The engine, once it is free, or `None` when `wait` passes first.
fn lock_within(state: &Mutex<State>, wait: Duration) -> Option<MutexGuard<'_, State>> {
    let deadline = Instant::now() + wait;
    loop {
        match state.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) if Instant::now() >= deadline => return None,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Counts a request among those waiting for the engine while it lives.
struct Waiting(Arc<AtomicUsize>);

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Server {
    pub fn new(mut engine: Engine, roots: Vec<PathBuf>, permissions: Permissions) -> Self {
        let events = engine.subscribe();
        let mut tool_router = Self::tool_router()
            + Self::discovery_router()
            + Self::analysis_router()
            + Self::changes_router()
            + Self::creation_router()
            + Self::publication_router()
            + Self::impact_router()
            + Self::build_router();
        if !permissions.write {
            for name in WRITE_TOOLS {
                tool_router.remove_route(name);
            }
        }
        if !permissions.build {
            for name in BUILD_TOOLS {
                tool_router.remove_route(name);
            }
        }
        Self {
            state: Arc::new(Mutex::new(State {
                engine,
                events,
                roots,
            })),
            tool_router,
            lock_wait: LOCK_WAIT,
            waiting: Arc::default(),
            permissions,
            previews: Arc::default(),
            subscriptions: Arc::default(),
            registry: Arc::default(),
        }
    }

    /// The server for a new session: the same engine and tools, and previews of its own.
    #[must_use]
    pub fn for_session(&self) -> Self {
        Self {
            previews: Arc::default(),
            subscriptions: Arc::default(),
            ..self.clone()
        }
    }

    pub(crate) fn allow_write(&self) -> bool {
        self.permissions.write
    }

    /// Runs `work` with the engine on the runtime's blocking threads, so a long render, search
    /// or export never holds up the runtime: other requests and sessions are answered meanwhile,
    /// and wait only for the engine itself, and that for at most [`LOCK_WAIT`].
    pub(crate) async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut State) -> T + Send + 'static,
    ) -> Result<T, Failure> {
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= MAX_WAITING {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(Failure::new(
                ErrorCode::Unavailable,
                format!("{MAX_WAITING} requests are already waiting for the engine; try again"),
            ));
        }
        let waiting = Waiting(self.waiting.clone());
        let (state, wait) = (self.state.clone(), self.lock_wait);
        tokio::task::spawn_blocking(move || {
            let _waiting = waiting;
            let mut state = lock_within(&state, wait).ok_or_else(|| {
                Failure::timeout(format!(
                    "another request has had the engine for over {} s; try again",
                    wait.as_secs_f32()
                ))
            })?;
            // Changes on disk and the graph's due updates are applied first, so every tool
            // reads the workspace as it is, not as it was at the last idle pump.
            state.engine.pump();
            Ok(work(&mut state))
        })
        .await
        .map_err(|error| Failure::internal(error.to_string()))?
    }

    /// Runs a tool's `work` as [`Server::blocking`] does and answers with what it gives.
    pub(crate) async fn run(
        &self,
        work: impl FnOnce(&mut State) -> Result<Value, Failure> + Send + 'static,
    ) -> Answer {
        Ok(answer(self.blocking(work).await.and_then(|result| result)))
    }
}

#[tool_router]
impl Server {
    #[tool(
        description = "List each Antora component version: name, version, title, start page and nav files.",
        output_schema = schemas::list_components(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_components(&self) -> Answer {
        self.run(|state| {
            let components: Vec<Value> = state
                .engine
                .workspace()
                .antora
                .components()
                .map(|component| {
                    json!({
                        "name": component.name,
                        "version": component.version,
                        "title": component.title,
                        "display_version": component.display_version,
                        "start_page": component.start_page,
                        "nav": component
                            .nav
                            .iter()
                            .map(|nav| state.shown(&component.root.join(nav)))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(limited("components", components))
        })
        .await
    }

    #[tool(
        description = "The navigation trees of a component version, with each entry's page path and resource id.",
        output_schema = schemas::get_nav(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_nav(&self, Parameters(args): Parameters<GetNavArgs>) -> Answer {
        self.run(move |state| {
            // No version means the component's latest, as Antora's resource ids read it; the
            // catalog itself takes no version for the versionless (`~`) one only.
            let version = match args.version {
                Some(version) => Some(version),
                None => state
                    .engine
                    .workspace()
                    .antora
                    .latest_version(&args.component)
                    .and_then(|latest| latest.version.clone()),
            };
            let trees = state
                .engine
                .nav_trees(&args.component, version.as_deref())
                .map_err(|error| match error {
                    adoc_engine::NavError::UnknownComponent { .. } => {
                        Failure::not_found(format!("{error:?}"))
                    }
                    _ => Failure::internal(format!("{error:?}")),
                })?;
            let trees: Vec<Value> = trees
                .iter()
                .map(|tree| {
                    json!({
                        "nav_file": state.shown(&tree.path),
                        "items": tree.items.iter().map(|item| state.nav_item(item)).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({ "trees": trees }))
        })
        .await
    }

    #[tool(
        description = "Read a page by path or Antora resource id: its text (the unsaved buffer if it has one), title, attributes and includes.",
        output_schema = schemas::read_page(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn read_page(&self, Parameters(args): Parameters<ReadPageArgs>) -> Answer {
        self.run(move |state| {
            let path = match (&args.path, &args.resource_id) {
                (Some(path), _) => state.confine(path)?,
                (None, Some(id)) => {
                    let from = args
                        .from
                        .as_deref()
                        .map(|from| state.confine(from))
                        .transpose()?;
                    let path = state
                        .engine
                        .resolve_resource_id(id, from.as_deref())
                        .map_err(|error| match error {
                            adoc_engine::EngineError::InvalidRequest(_) => Failure::from(error),
                            _ => Failure::not_found(error.to_string()),
                        })?;
                    state.confine(&path.to_string_lossy())?
                }
                (None, None) => return Err(Failure::invalid("give a `path` or a `resource_id`")),
            };
            let text = state.text(&path)?;
            let file = state.engine.workspace().index.file(&path);
            let document = file.map(|file| &file.document);
            let includes: Vec<Value> = document
                .map(|document| {
                    document
                        .includes
                        .iter()
                        .map(|include| {
                            let resolved = state
                                .engine
                                .resolve_target(&path, &format!("include::{}[]", include.target));
                            let resolves_to = match resolved {
                                Some(DefinitionTarget::Location(location)) => {
                                    Some(state.shown(&location.path))
                                }
                                Some(DefinitionTarget::External(path)) => Some(state.shown(&path)),
                                None => None,
                            };
                            json!({ "target": include.target, "resolves_to": resolves_to })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let attributes: Vec<Value> = document
                .map(|document| {
                    document
                        .attributes
                        .iter()
                        .map(
                            |attribute| json!({ "name": attribute.name, "value": attribute.value }),
                        )
                        .collect()
                })
                .unwrap_or_default();
            let (shown_text, total) = limited_text(&text);
            let mut value = json!({
                "path": state.shown(&path),
                "resource_id": state.engine.resource_id(&path),
                "title": document.and_then(|document| document.title.as_ref()).map(|title| title.text.clone()),
                "attributes": attributes,
                "includes": includes,
                "text": shown_text,
            });
            if let Some(total) = total {
                value["truncated"] = json!({ "shown_bytes": MAX_TEXT, "total_bytes": total });
            }
            Ok(value)
        })
        .await
    }

    #[tool(
        description = "What an xref, include or image target resolves to from a page, or that it does not.",
        output_schema = schemas::resolve(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn resolve(&self, Parameters(args): Parameters<ResolveArgs>) -> Answer {
        self.run(move |state| {
            let from = state.confine(&args.from)?;
            Ok(match state.engine.resolve_target(&from, &args.target) {
                Some(DefinitionTarget::Location(location)) => {
                    let text = state.text(&location.path).unwrap_or_default();
                    let line = text[..location.range.start.min(text.len())]
                        .matches('\n')
                        .count()
                        + 1;
                    json!({
                        "resolves": true,
                        "path": state.shown(&location.path),
                        "line": line,
                        "resource_id": state.engine.resource_id(&location.path),
                    })
                }
                Some(DefinitionTarget::External(path)) => json!({
                    "resolves": true,
                    "path": state.shown(&path),
                    "external": true,
                }),
                None => json!({
                    "resolves": false,
                    "reason": format!("`{}` names no page, file or anchor from {}", args.target, state.shown(&from)),
                }),
            })
        })
        .await
    }

    #[tool(
        description = "The workspace's diagnostics, as `adocfoundry-cli check --format json` gives them, a page at a time, and whether any (on any page) meets `fail_on`.",
        output_schema = schemas::check(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check(&self, Parameters(args): Parameters<CheckArgs>) -> Answer {
        self.run(move |state| {
            let fail_on = args.fail_on.as_deref().unwrap_or("error");
            let threshold = match fail_on {
                "error" => DiagnosticSeverity::Error,
                "warning" => DiagnosticSeverity::Warning,
                other => return Err(Failure::invalid(format!("unknown fail_on level `{other}`"))),
            };
            let scope = args
                .scope
                .as_deref()
                .map(|scope| state.confine(scope))
                .transpose()?;
            state.indexed()?;
            let revision = state.engine.workspace_revision();
            let entries = state.engine.check_report(state.root(), scope.as_deref());
            let failed = entries.iter().any(|entry| entry.meets(threshold));
            let query = format!(
                "check\u{1f}{}\u{1f}{fail_on}",
                scope
                    .as_deref()
                    .map(|scope| state.shown(scope))
                    .unwrap_or_default()
            );
            let page = page(
                entries,
                &query,
                revision,
                args.cursor.as_deref(),
                args.limit,
            )?;
            let mut value = paged("diagnostics", page, revision);
            value["failed"] = json!(failed);
            Ok(value)
        })
        .await
    }

    #[tool(
        description = "Workspace symbols (titles, sections, anchors, attributes) matching a query, a page at a time.",
        output_schema = schemas::symbols(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn symbols(&self, Parameters(args): Parameters<SymbolsArgs>) -> Answer {
        self.run(move |state| {
            state.indexed()?;
            let revision = state.engine.workspace_revision();
            let symbols = state.engine.workspace_symbols(&args.query);
            let query = format!("symbols\u{1f}{}", args.query);
            let page = page(
                symbols,
                &query,
                revision,
                args.cursor.as_deref(),
                args.limit,
            )?;
            let symbols: Vec<Value> = page
                .items
                .iter()
                .map(|symbol| {
                    let line = state
                        .text(&symbol.path)
                        .map(|text| {
                            text[..symbol.range.start.min(text.len())]
                                .matches('\n')
                                .count()
                                + 1
                        })
                        .ok();
                    json!({
                        "name": symbol.name,
                        "kind": format!("{:?}", symbol.kind).to_lowercase(),
                        "path": state.shown(&symbol.path),
                        "line": line,
                        "container": symbol.container,
                    })
                })
                .collect();
            let page = crate::responses::Page {
                items: symbols,
                next_cursor: page.next_cursor,
                total: page.total,
            };
            Ok(paged("symbols", page, revision))
        })
        .await
    }

    #[tool(
        description = "Search the workspace's pages, with the line of each match.",
        output_schema = schemas::search(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search(&self, Parameters(args): Parameters<SearchArgs>) -> Answer {
        self.run(move |state| {
            let scope = args
                .scope
                .as_deref()
                .map(|scope| state.confine(scope))
                .transpose()?;
            state.events.try_iter().for_each(drop);
            state.engine.invoke(
                ids::WORKSPACE_SEARCH,
                CommandArgs::Search(SearchRequest {
                    query: args.query.clone(),
                    regex: args.regex,
                    case_sensitive: args.case_sensitive,
                    whole_word: args.whole_word,
                    scope,
                }),
            )?;
            // The search runs on one worker, which reports its matches in batches; this waits
            // for its last, not for other workers such as a site build.
            let mut matches = Vec::new();
            let mut done = false;
            let events = &state.events;
            state.engine.wait_until(Instant::now() + TIMEOUT, |_| {
                for event in events.try_iter() {
                    if let EngineEvent::SearchResults {
                        results,
                        done: finished,
                        ..
                    } = event
                    {
                        matches.extend(results);
                        done |= finished;
                    }
                }
                done
            });
            if !done {
                return Err(Failure::timeout("the search did not finish"));
            }
            let matches: Vec<Value> = matches
                .iter()
                .map(|found| {
                    json!({
                        "path": state.shown(&found.path),
                        "line": found.line + 1,
                        "text": found.line_text,
                    })
                })
                .collect();
            Ok(limited("matches", matches))
        })
        .await
    }

    #[tool(
        description = "Render a page through the engine: plain text from its view tree, or HTML.",
        output_schema = schemas::render_page(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn render_page(&self, Parameters(args): Parameters<RenderPageArgs>) -> Answer {
        self.run(move |state| {
            let path = state.confine(&args.path)?;
            let html = match args.format.as_deref().unwrap_or("text") {
                "text" => false,
                "html" => true,
                other => {
                    return Err(Failure::invalid(format!(
                        "unknown format `{other}`; use `text` or `html`"
                    )))
                }
            };
            let (id, opened) = state.open(&path)?;
            let rendered = state.engine.render_blocking(id, TIMEOUT);
            if opened {
                let _ = state.engine.close(id);
            }
            let preview = rendered
                .ok_or_else(|| Failure::timeout(format!("{} did not render", args.path)))?;
            let output = if html {
                adoc_engine::to_html(
                    &preview.view,
                    &adoc_engine::HtmlOptions { standalone: true },
                )
            } else {
                adoc_engine::to_text(&preview.view)
            };
            let (output, total) = limited_text(&output);
            let mut value = json!({ "path": state.shown(&path), "content": output });
            if let Some(total) = total {
                value["truncated"] = json!({ "shown_bytes": MAX_TEXT, "total_bytes": total });
            }
            Ok(value)
        })
        .await
    }

    #[tool(
        description = "How pages link: the pages, partials and navigation files of the workspace, or of a page and its neighbours, with the xrefs, includes and navigation entries between them, as `adocfoundry-cli graph` gives them; with `scope: path`, the shortest path between two pages; with `format: svg`, drawn as an SVG image.",
        output_schema = schemas::link_graph(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn link_graph(&self, Parameters(args): Parameters<LinkGraphArgs>) -> Answer {
        self.run(move |state| {
            let defaults = GraphFilter::default();
            let filter = GraphFilter {
                partials: args.partials.unwrap_or(defaults.partials),
                nav: args.nav.unwrap_or(defaults.nav),
                orphans: args.orphans.unwrap_or(defaults.orphans),
                unresolved: args.unresolved.unwrap_or(defaults.unresolved),
                components: None,
            };
            let svg = match args.format.as_deref().unwrap_or("json") {
                "json" => None,
                "svg" => Some(SvgStyle {
                    colour_by: state.engine.settings().graph.colour_by,
                    dark: args.dark.unwrap_or(false),
                }),
                other => return Err(Failure::invalid(format!("unknown format `{other}`"))),
            };
            let not_indexed = |state: &State, page: &Path| {
                Failure::not_found(format!("{} is not an indexed page", state.shown(page)))
            };
            if args.scope.as_deref() == Some("path") {
                let (Some(from), Some(to)) = (&args.path, &args.to) else {
                    return Err(Failure::invalid("a `path` scope needs a `path` and a `to`"));
                };
                let (from, to) = (state.confine(from)?, state.confine(to)?);
                for page in [&from, &to] {
                    if state.engine.workspace().index.file(page).is_none() {
                        return Err(not_indexed(state, page));
                    }
                }
                let directed = args.directed.unwrap_or(true);
                let engine = &state.engine;
                let Some(report) =
                    engine.graph_path_report(state.root(), &filter, &from, &to, directed)
                else {
                    return Ok(json!({ "path": null }));
                };
                if let Some(style) = svg {
                    let (snapshot, _) = engine
                        .graph_path_view(&filter, &from, &to, directed)
                        .unwrap_or_default();
                    return Ok(json!({ "svg": to_svg_laid_out(&snapshot, &style) }));
                }
                return serde_json::to_value(report)
                    .map_err(|error| Failure::internal(error.to_string()));
            }
            let scope = match (args.scope.as_deref().unwrap_or("workspace"), &args.path) {
                ("workspace", _) => GraphScope::Workspace,
                ("page", Some(path)) => GraphScope::Local {
                    path: state.confine(path)?,
                    depth: args.depth.unwrap_or(1).clamp(1, 3),
                },
                ("page", None) => return Err(Failure::invalid("a `page` scope needs a `path`")),
                (other, _) => return Err(Failure::invalid(format!("unknown scope `{other}`"))),
            };
            if let GraphScope::Local { path, .. } = &scope {
                if state.engine.workspace().index.file(path).is_none() {
                    return Err(not_indexed(state, path));
                }
            }
            if let Some(style) = svg {
                let svg = state.engine.graph_svg(&scope, &filter, &style);
                if svg.len() > MAX_TEXT {
                    return Err(Failure::new(
                        ErrorCode::ResultTooLarge,
                        format!(
                            "the SVG is {} bytes, over the {MAX_TEXT} allowed; draw a page's \
                             neighbourhood with `scope: page`",
                            svg.len()
                        ),
                    ));
                }
                return Ok(json!({ "svg": svg }));
            }
            let report = state.engine.graph_report(state.root(), &scope, &filter);
            let nodes = limited("nodes", report.nodes);
            let edges = limited("edges", report.edges);
            let mut value = json!({
                "revision": report.revision,
                "nodes": nodes["nodes"],
                "edges": edges["edges"],
            });
            for (part, cut) in [("nodes", &nodes), ("edges", &edges)] {
                if let Some(truncated) = cut.get("truncated") {
                    value["truncated"][part] = truncated.clone();
                    value["incomplete"] = json!(true);
                }
            }
            Ok(value)
        })
        .await
    }

    #[tool(
        description = "Each root's Git branch and changed AsciiDoc files, as `adocfoundry-cli git-status --format json` gives them.",
        output_schema = schemas::git_status(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn git_status(&self) -> Answer {
        self.run(|state| {
            let roots = state.roots.clone();
            let reports: Vec<Value> = roots
                .iter()
                .map(|root| match state.engine.git_status_report(root, TIMEOUT) {
                    Ok(report) => json!({ "root": root, "status": report }),
                    Err(error) => json!({ "root": root, "error": error.to_string() }),
                })
                .collect();
            Ok(json!({ "roots": reports }))
        })
        .await
    }

    #[tool(
        description = "Edit a page: each edit replaces text that occurs exactly once, or a byte range, through the engine's edit command, in order. With `save`, the page is written to disk.",
        output_schema = schemas::edit_page(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn edit_page(&self, Parameters(args): Parameters<EditPageArgs>) -> Answer {
        self.run(move |state| {
            let path = state.confine(&args.path)?;
            let (id, _) = state.open(&path)?;
            let mut applied = 0;
            let result = (|| {
                for (number, edit) in args.edits.iter().enumerate() {
                    let text = state
                        .engine
                        .document(id)
                        .map(adoc_engine::OpenDocument::text_string)
                        .unwrap_or_default();
                    let range = match (&edit.old_text, &edit.range) {
                        (Some(old), None) => {
                            let found: Vec<usize> =
                                text.match_indices(old.as_str()).map(|(at, _)| at).collect();
                            match found.as_slice() {
                                [at] => SourceRange::new(*at, at + old.len()),
                                [] => {
                                    return Err(Failure::new(
                                        ErrorCode::Conflict,
                                        format!(
                                            "edit {}: `old_text` is not in the page",
                                            number + 1
                                        ),
                                    ))
                                }
                                _ => {
                                    return Err(Failure::invalid(format!(
                                        "edit {}: `old_text` occurs {} times; give more context",
                                        number + 1,
                                        found.len()
                                    )))
                                }
                            }
                        }
                        (None, Some(range)) => SourceRange::new(range.start, range.end),
                        _ => {
                            return Err(Failure::invalid(format!(
                                "edit {}: give either `old_text` or `range`",
                                number + 1
                            )))
                        }
                    };
                    state
                        .engine
                        .invoke(
                            ids::EDIT_REPLACE_RANGE,
                            CommandArgs::Text {
                                id,
                                range,
                                text: edit.new_text.clone(),
                            },
                        )
                        .map_err(|error| {
                            let failure = Failure::from(error);
                            Failure::new(
                                failure.code,
                                format!("edit {}: {}", number + 1, failure.message),
                            )
                        })?;
                    applied += 1;
                }
                Ok(())
            })();
            if let Err(error) = result {
                // All or nothing: the edits already made are undone.
                for _ in 0..applied {
                    state.engine.undo(id);
                }
                return Err(error);
            }
            if args.save {
                state
                    .engine
                    .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))?;
            }
            let length = state
                .engine
                .document(id)
                .map_or(0, |document| document.text.len());
            Ok(json!({
                "path": state.shown(&path),
                "edits": applied,
                "saved": args.save,
                "length": length,
            }))
        })
        .await
    }

    #[tool(
        description = "Rename a page, rewriting the xrefs, nav entries and includes that name it, and save every file changed.",
        output_schema = schemas::rename_page(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn rename_page(&self, Parameters(args): Parameters<RenamePageArgs>) -> Answer {
        self.run(move |state| {
            let path = state.confine(&args.path)?;
            let parent = path.parent().unwrap_or_else(|| Path::new(""));
            let new_path = state.confine(&parent.join(&args.new_name).to_string_lossy())?;
            let (id, _) = state.open(&path)?;
            let CommandOutcome::Preview(preview) = state.engine.invoke(
                ids::PAGE_RENAME,
                CommandArgs::Rename {
                    id,
                    new_path: new_path.clone(),
                },
            )?
            else {
                return Err(Failure::invalid("the rename produced no change"));
            };
            state.engine.invoke(
                ids::WORKSPACE_APPLY_PREVIEW,
                CommandArgs::Custom(json!({ "preview_id": preview.id.get() })),
            )?;
            let mut changed = Vec::new();
            for file in preview
                .edits
                .iter()
                .map(|file| &file.path)
                .chain([&new_path])
            {
                let file = if *file == path { &new_path } else { file };
                if let Some(document) = state.engine.document_for_path(file) {
                    state
                        .engine
                        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(document))?;
                }
                changed.push(state.shown(file));
            }
            changed.sort();
            changed.dedup();
            Ok(json!({
                "from": state.shown(&path),
                "to": state.shown(&new_path),
                "changed": changed,
            }))
        })
        .await
    }

    #[tool(
        description = "Export a page as an article, or a component version as a book, to PDF.",
        output_schema = schemas::export_pdf(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn export_pdf(&self, Parameters(args): Parameters<ExportPdfArgs>) -> Answer {
        self.run(move |state| {
            let output = state.confine(&args.output)?;
            // The name is the client's: a link planted there is refused rather than written
            // through, as the batch export does.
            if std::fs::symlink_metadata(&output).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(Failure::invalid(format!(
                    "{} is a link, which is not written through",
                    args.output
                )));
            }
            let target = match (&args.path, &args.component) {
                (Some(path), None) => {
                    let path = state.confine(path)?;
                    ExportTarget::Page(state.open(&path)?.0)
                }
                (None, Some(component)) => {
                    let (version, name) = match component.split_once('@') {
                        Some((version, name)) => (Some(version.to_owned()), name.to_owned()),
                        None => (None, component.clone()),
                    };
                    ExportTarget::Component { name, version }
                }
                _ => return Err(Failure::invalid("give either a `path` or a `component`")),
            };
            let mut standards = Vec::new();
            for name in &args.standards {
                standards.push(
                    adoc_engine::pdf_standard(name).ok_or_else(|| {
                        Failure::invalid(format!("unknown PDF standard `{name}`"))
                    })?,
                );
            }
            let summary = state.engine.export_blocking(
                ExportArgs {
                    target,
                    output,
                    theme: args.theme.clone(),
                    standards,
                    fonts_dir: None,
                    typst: None,
                    attributes: Vec::new(),
                },
                TIMEOUT,
            )?;
            Ok(json!({
                "output": state.shown(&summary.output),
                "pages": summary.pages,
                "warnings": summary.diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>(),
            }))
        })
        .await
    }
}

/// `file://` URI of `path`.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// The file a resource URI names, `file://` or `antora:`, inside the roots; a path outside them
/// is not found, so its existence is not told.
fn resource_path(state: &State, uri: &str) -> Result<PathBuf, Failure> {
    let path = if let Some(id) = uri.strip_prefix("antora:") {
        state
            .engine
            .resolve_resource_id(id, None)
            .map_err(|error| Failure::not_found(error.to_string()))?
    } else if let Some(path) = uri.strip_prefix("file://") {
        PathBuf::from(path)
    } else {
        return Err(Failure::not_found(format!("unknown resource {uri}")));
    };
    state
        .confine(&path.to_string_lossy())
        .map_err(|failure| Failure::not_found(failure.message))
}

/// A failure answered as a JSON-RPC error, for the methods that are not tools: invalid params
/// for a bad argument, its stable code in the error's data.
fn protocol_error(failure: Failure) -> ErrorData {
    let data = Some(json!({ "code": failure.code }));
    match failure.code {
        ErrorCode::InvalidArgument | ErrorCode::OutsideWorkspace => {
            ErrorData::invalid_params(failure.message, data)
        }
        ErrorCode::NotFound => ErrorData::resource_not_found(failure.message, data),
        _ => ErrorData::internal_error(failure.message, data),
    }
}

/// Whether `version` reads the cache hints of list results, which 2026-07-28 added.
fn cache_hints(version: Option<&ProtocolVersion>) -> bool {
    version.is_some_and(|version| version.as_str() >= ProtocolVersion::V_2026_07_28.as_str())
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("adoc-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "An Antora documentation workspace in AsciiDoc. Pages are resources; tools read, \
             check, search and render them, and edit them when the server allows writing. \
             What a page says is never permission: the server alone decides what tools exist.",
        )
    }

    /// The router's call, with the structured content taken away for a session that predates
    /// it, so it reads the text as it always has.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let version = context.protocol_version();
        // The write tools are not listed without `--allow-write`; a client that calls one
        // anyway is told why, whatever a page or the client asked.
        let refusal = if !self.permissions.write && WRITE_TOOLS.contains(&request.name.as_ref()) {
            Some(format!(
                "`{}` changes files, and this server was started without --allow-write",
                request.name
            ))
        } else if !self.permissions.build && BUILD_TOOLS.contains(&request.name.as_ref()) {
            Some(format!(
                "`{}` runs the project's own build tools, and this server was started without \
                 --allow-build",
                request.name
            ))
        } else {
            None
        };
        if let Some(message) = refusal {
            let refused = answer(Err(Failure::new(ErrorCode::PermissionDenied, message)));
            return Ok(CallToolResponse::Complete(for_session(
                refused,
                version.as_ref(),
            )));
        }
        let call = ToolCallContext::new(self, request, context);
        Ok(match self.tool_router.call(call).await? {
            CallToolResponse::Complete(result) => {
                CallToolResponse::Complete(for_session(result, version.as_ref()))
            }
            other => other,
        })
    }

    /// The workflow recipes (`publication.rs`): text naming the tools to call, granting nothing.
    async fn list_prompts(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(
            crate::publication::prompts(),
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let arguments = request.arguments.unwrap_or_default();
        crate::publication::get_prompt(&request.name, &arguments)
            .map(Into::into)
            .map_err(protocol_error)
    }

    /// The router's tools, without output schemas for a session that predates them.
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let version = context.protocol_version();
        let mut tools = self.tool_router.list_all();
        if !structured(version.as_ref()) {
            for tool in &mut tools {
                tool.output_schema = None;
            }
        }
        let mut result = ListToolsResult::with_all_items(tools);
        if cache_hints(version.as_ref()) {
            result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Public);
        }
        Ok(result)
    }

    /// The indexed files, [`crate::responses::PAGE`] at a time, ordered by Antora resource id
    /// and then path (files outside any component first); `next_cursor` resumes the list while
    /// the workspace revision stands.
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let cursor = request.and_then(|request| request.cursor);
        let page = self
            .blocking(move |state| {
                state.indexed()?;
                let revision = state.engine.workspace_revision();
                let mut files: Vec<(String, String, Resource)> = state
                    .engine
                    .workspace()
                    .index
                    .files()
                    .map(|file| {
                        let path = state.shown(&file.path);
                        let mut resource = Resource::new(file_uri(&file.path), path.clone())
                            .with_mime_type(ASCIIDOC);
                        if let Some(title) = &file.document.title {
                            resource = resource.with_title(title.text.clone());
                        }
                        let id = state.engine.resource_id(&file.path).unwrap_or_default();
                        (id, path, resource)
                    })
                    .collect();
                files.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
                let resources = files.into_iter().map(|(_, _, resource)| resource).collect();
                page(resources, "resources", revision, cursor.as_deref(), None)
            })
            .await
            .and_then(|page| page)
            .map_err(protocol_error)?;
        let mut result = ListResourcesResult::with_all_items(page.items);
        result.next_cursor = page.next_cursor;
        Ok(result)
    }

    /// Subscribes this session to a resource: it is told when the file changes on disk (never
    /// of a change only in a buffer, which every session shares).
    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: rmcp::model::SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let uri = request.uri.clone();
        let path = self
            .blocking(move |state| resource_path(state, &request.uri))
            .await
            .and_then(|path| path)
            .map_err(protocol_error)?;
        crate::subscriptions::subscribe(
            &self.registry,
            &self.subscriptions,
            context.peer.clone(),
            uri,
            path,
        );
        Ok(())
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: rmcp::model::UnsubscribeRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        crate::subscriptions::unsubscribe(&self.subscriptions, &request.uri);
        Ok(())
    }

    async fn list_resource_templates(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("antora:{resource_id}", "Antora resource")
                .with_description(
                    "A page or partial by its Antora resource id, `version@component:module:family$path`.",
                )
                .with_mime_type(ASCIIDOC),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.blocking(move |state| {
            let path = resource_path(state, &request.uri)?;
            let text = state.text(&path)?;
            Ok(ReadResourceResult::new(vec![
                ResourceContents::text(text, request.uri.clone()).with_mime_type(ASCIIDOC)
            ])
            .into())
        })
        .await
        .and_then(|result| result)
        .map_err(protocol_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        let root = adoc_engine::normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/references"),
        );
        let mut engine = Engine::new(adoc_engine::Settings::default().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        Server::new(engine, vec![root], Permissions::default())
    }

    fn code(result: &CallToolResult) -> Value {
        result
            .structured_content
            .as_ref()
            .map(|value| value["error"]["code"].clone())
            .unwrap_or_default()
    }

    #[test]
    fn mcp_timeout_does_not_block_other_clients_forever() {
        let mut server = server();
        server.lock_wait = Duration::from_millis(200);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();

        // A stalled request has the engine and keeps it.
        let (held, is_held) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let state = server.state.clone();
        let stalled = std::thread::spawn(move || {
            let _engine = state.lock().unwrap();
            held.send(()).unwrap();
            let _ = released.recv();
        });
        is_held.recv().unwrap();

        let started = Instant::now();
        let answer = runtime.block_on(server.list_components()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(answer.is_error, Some(true));
        assert_eq!(code(&answer), json!("timeout"), "{answer:?}");

        // So many requests waiting that another is turned away at once.
        server.waiting.store(MAX_WAITING, Ordering::SeqCst);
        let answer = runtime.block_on(server.list_components()).unwrap();
        assert_eq!(code(&answer), json!("unavailable"));
        server.waiting.store(0, Ordering::SeqCst);

        release.send(()).unwrap();
        stalled.join().unwrap();
        let answer = runtime.block_on(server.list_components()).unwrap();
        assert_eq!(answer.is_error, Some(false), "{answer:?}");
        assert_eq!(
            server.waiting.load(Ordering::SeqCst),
            0,
            "every wait counted out"
        );
    }
}
