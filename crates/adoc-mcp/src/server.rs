//! The tools and resources. Each tool locks the engine, does its work through the engine's
//! commands and queries, and answers with JSON text, or a tool error carrying the engine's
//! message.

use std::{
    path::{Path, PathBuf},
    sync::{mpsc::Receiver, Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use adoc_core::{DiagnosticSeverity, SourceRange};
use adoc_engine::{
    commands::ids, report::display_path, CommandArgs, CommandOutcome, DefinitionTarget, Engine,
    EngineEvent, ExportArgs, ExportTarget, NavItem, SearchRequest,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock, Implementation, ListResourceTemplatesResult,
        ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ResourceTemplate,
        ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::paths::confine;

/// The tools that change files, listed and callable only with `--allow-write`.
const WRITE_TOOLS: [&str; 3] = ["edit_page", "rename_page", "export_pdf"];

/// At most this many items in a list result; the total is stated when there are more.
const MAX_ITEMS: usize = 500;

/// At most this many bytes of a text result; the total is stated when there are more.
const MAX_TEXT: usize = 200_000;

/// How long a render, search, Git status or export may take.
const TIMEOUT: Duration = Duration::from_secs(120);

const ASCIIDOC: &str = "text/asciidoc";

struct State {
    engine: Engine,
    events: Receiver<EngineEvent>,
    roots: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct Server {
    state: Arc<Mutex<State>>,
    tool_router: ToolRouter<Self>,
}

/// A tool's answer: JSON on success, the message as a tool error otherwise.
type Answer = Result<CallToolResult, ErrorData>;

fn answer(result: Result<Value, String>) -> Answer {
    Ok(match result {
        Ok(value) => CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&value).unwrap_or_default(),
        )]),
        Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
    })
}

/// `items`, cut to [`MAX_ITEMS`], under `key`, with the total when cut.
fn limited<T: serde::Serialize>(key: &str, items: Vec<T>) -> Value {
    let total = items.len();
    let shown: Vec<T> = items.into_iter().take(MAX_ITEMS).collect();
    let mut value = json!({ key: shown });
    if total > MAX_ITEMS {
        value["truncated"] = json!({ "shown": MAX_ITEMS, "total": total });
    }
    value
}

/// `text`, cut to [`MAX_TEXT`] bytes at a character boundary, and its total length when cut.
fn limited_text(text: &str) -> (String, Option<usize>) {
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
}

#[derive(Deserialize, JsonSchema)]
struct SymbolsArgs {
    /// Words the symbols' names start with.
    query: String,
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
    fn confine(&self, path: &str) -> Result<PathBuf, String> {
        confine(&self.roots, path)
    }

    fn root(&self) -> &Path {
        &self.roots[0]
    }

    /// A path as the results show it: relative to the first root.
    fn shown(&self, path: &Path) -> String {
        display_path(path, self.root())
    }

    /// The text of the page at `path`: its buffer when open, else the file.
    fn text(&self, path: &Path) -> Result<String, String> {
        if let Some(id) = self.engine.document_for_path(path) {
            if let Some(document) = self.engine.document(id) {
                return Ok(document.text_string());
            }
        }
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", self.shown(path)))
    }

    /// Opens `path` if it is not open; returns its id and whether this opened it.
    fn open(&mut self, path: &Path) -> Result<(adoc_engine::DocumentId, bool), String> {
        if let Some(id) = self.engine.document_for_path(path) {
            return Ok((id, false));
        }
        let id = self.engine.open(path).map_err(|error| error.to_string())?;
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
    pub fn new(mut engine: Engine, roots: Vec<PathBuf>, allow_write: bool) -> Self {
        let events = engine.subscribe();
        let mut tool_router = Self::tool_router();
        if !allow_write {
            for name in WRITE_TOOLS {
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
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[tool_router]
impl Server {
    #[tool(
        description = "List each Antora component version: name, version, title, start page and nav files."
    )]
    async fn list_components(&self) -> Answer {
        let state = self.state();
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
                    "nav": component.nav.iter().map(|nav| state.shown(&component.root.join(nav))).collect::<Vec<_>>(),
                })
            })
            .collect();
        answer(Ok(limited("components", components)))
    }

    #[tool(
        description = "The navigation trees of a component version, with each entry's page path and resource id."
    )]
    async fn get_nav(&self, Parameters(args): Parameters<GetNavArgs>) -> Answer {
        let state = self.state();
        let result = state
            .engine
            .nav_trees(&args.component, args.version.as_deref())
            .map_err(|error| format!("{error:?}"))
            .map(|trees| {
                let trees: Vec<Value> = trees
                    .iter()
                    .map(|tree| {
                        json!({
                            "nav_file": state.shown(&tree.path),
                            "items": tree.items.iter().map(|item| state.nav_item(item)).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                json!({ "trees": trees })
            });
        answer(result)
    }

    #[tool(
        description = "Read a page by path or Antora resource id: its text (the unsaved buffer if it has one), title, attributes and includes."
    )]
    async fn read_page(&self, Parameters(args): Parameters<ReadPageArgs>) -> Answer {
        let state = self.state();
        answer((|| {
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
                        .map_err(|error| error.to_string())?;
                    state.confine(&path.to_string_lossy())?
                }
                (None, None) => return Err("give a `path` or a `resource_id`".to_owned()),
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
        })())
    }

    #[tool(
        description = "What an xref, include or image target resolves to from a page, or that it does not."
    )]
    async fn resolve(&self, Parameters(args): Parameters<ResolveArgs>) -> Answer {
        let state = self.state();
        answer((|| {
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
        })())
    }

    #[tool(
        description = "The workspace's diagnostics, as `adocfoundry-cli check --format json` gives them, and whether any meets `fail_on`."
    )]
    async fn check(&self, Parameters(args): Parameters<CheckArgs>) -> Answer {
        let state = self.state();
        answer((|| {
            let threshold = match args.fail_on.as_deref().unwrap_or("error") {
                "error" => DiagnosticSeverity::Error,
                "warning" => DiagnosticSeverity::Warning,
                other => return Err(format!("unknown fail_on level `{other}`")),
            };
            let scope = args
                .scope
                .as_deref()
                .map(|scope| state.confine(scope))
                .transpose()?;
            let entries = state.engine.check_report(state.root(), scope.as_deref());
            let failed = entries.iter().any(|entry| entry.meets(threshold));
            let mut value = limited("diagnostics", entries);
            value["failed"] = json!(failed);
            Ok(value)
        })())
    }

    #[tool(
        description = "Workspace symbols (titles, sections, anchors, attributes) matching a query."
    )]
    async fn symbols(&self, Parameters(args): Parameters<SymbolsArgs>) -> Answer {
        let state = self.state();
        let symbols: Vec<Value> = state
            .engine
            .workspace_symbols(&args.query)
            .into_iter()
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
        answer(Ok(limited("symbols", symbols)))
    }

    #[tool(description = "Search the workspace's pages, with the line of each match.")]
    async fn search(&self, Parameters(args): Parameters<SearchArgs>) -> Answer {
        let mut state = self.state();
        answer((|| {
            let scope = args
                .scope
                .as_deref()
                .map(|scope| state.confine(scope))
                .transpose()?;
            state.events.try_iter().for_each(drop);
            state
                .engine
                .invoke(
                    ids::WORKSPACE_SEARCH,
                    CommandArgs::Search(SearchRequest {
                        query: args.query.clone(),
                        regex: args.regex,
                        case_sensitive: args.case_sensitive,
                        whole_word: args.whole_word,
                        scope,
                    }),
                )
                .map_err(|error| error.to_string())?;
            let deadline = Instant::now() + TIMEOUT;
            let mut matches = Vec::new();
            loop {
                state.engine.pump();
                let mut done = false;
                for event in state.events.try_iter().collect::<Vec<_>>() {
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
                if done {
                    break;
                }
                if Instant::now() > deadline {
                    return Err("the search did not finish".to_owned());
                }
                std::thread::sleep(Duration::from_millis(5));
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
        })())
    }

    #[tool(
        description = "Render a page through the engine: plain text from its view tree, or HTML."
    )]
    async fn render_page(&self, Parameters(args): Parameters<RenderPageArgs>) -> Answer {
        let mut state = self.state();
        answer((|| {
            let path = state.confine(&args.path)?;
            let html = match args.format.as_deref().unwrap_or("text") {
                "text" => false,
                "html" => true,
                other => return Err(format!("unknown format `{other}`; use `text` or `html`")),
            };
            let (id, opened) = state.open(&path)?;
            let rendered = state.engine.render_blocking(id, TIMEOUT);
            if opened {
                let _ = state.engine.close(id);
            }
            let preview = rendered.ok_or_else(|| format!("{} did not render", args.path))?;
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
        })())
    }

    #[tool(
        description = "Each root's Git branch and changed AsciiDoc files, as `adocfoundry-cli git-status --format json` gives them."
    )]
    async fn git_status(&self) -> Answer {
        let mut state = self.state();
        let roots = state.roots.clone();
        let reports: Vec<Value> = roots
            .iter()
            .map(|root| match state.engine.git_status_report(root, TIMEOUT) {
                Ok(report) => json!({ "root": root, "status": report }),
                Err(error) => json!({ "root": root, "error": error.to_string() }),
            })
            .collect();
        answer(Ok(json!({ "roots": reports })))
    }

    #[tool(
        description = "Edit a page: each edit replaces text that occurs exactly once, or a byte range, through the engine's edit command, in order. With `save`, the page is written to disk."
    )]
    async fn edit_page(&self, Parameters(args): Parameters<EditPageArgs>) -> Answer {
        let mut state = self.state();
        answer((|| {
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
                                    return Err(format!(
                                        "edit {}: `old_text` is not in the page",
                                        number + 1
                                    ))
                                }
                                _ => {
                                    return Err(format!(
                                        "edit {}: `old_text` occurs {} times; give more context",
                                        number + 1,
                                        found.len()
                                    ))
                                }
                            }
                        }
                        (None, Some(range)) => SourceRange::new(range.start, range.end),
                        _ => {
                            return Err(format!(
                                "edit {}: give either `old_text` or `range`",
                                number + 1
                            ))
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
                        .map_err(|error| format!("edit {}: {error}", number + 1))?;
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
                    .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
                    .map_err(|error| error.to_string())?;
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
        })())
    }

    #[tool(
        description = "Rename a page, rewriting the xrefs, nav entries and includes that name it, and save every file changed."
    )]
    async fn rename_page(&self, Parameters(args): Parameters<RenamePageArgs>) -> Answer {
        let mut state = self.state();
        answer((|| {
            let path = state.confine(&args.path)?;
            let parent = path.parent().unwrap_or_else(|| Path::new(""));
            let new_path = state.confine(&parent.join(&args.new_name).to_string_lossy())?;
            let (id, _) = state.open(&path)?;
            let CommandOutcome::Preview(preview) = state
                .engine
                .invoke(
                    ids::PAGE_RENAME,
                    CommandArgs::Rename {
                        id,
                        new_path: new_path.clone(),
                    },
                )
                .map_err(|error| error.to_string())?
            else {
                return Err("the rename produced no change".to_owned());
            };
            state
                .engine
                .invoke(
                    ids::WORKSPACE_APPLY_PREVIEW,
                    CommandArgs::Custom(json!({ "preview_id": preview.id.get() })),
                )
                .map_err(|error| error.to_string())?;
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
                        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(document))
                        .map_err(|error| error.to_string())?;
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
        })())
    }

    #[tool(description = "Export a page as an article, or a component version as a book, to PDF.")]
    async fn export_pdf(&self, Parameters(args): Parameters<ExportPdfArgs>) -> Answer {
        let mut state = self.state();
        answer((|| {
            let output = state.confine(&args.output)?;
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
                _ => return Err("give either a `path` or a `component`".to_owned()),
            };
            let mut standards = Vec::new();
            for name in &args.standards {
                standards.push(
                    adoc_engine::pdf_standard(name)
                        .ok_or_else(|| format!("unknown PDF standard `{name}`"))?,
                );
            }
            let summary = state
                .engine
                .export_blocking(
                    ExportArgs {
                        target,
                        output,
                        theme: args.theme.clone(),
                        standards,
                        fonts_dir: None,
                        typst: None,
                    },
                    TIMEOUT,
                )
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "output": state.shown(&summary.output),
                "pages": summary.pages,
                "warnings": summary.diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>(),
            }))
        })())
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

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("adoc-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "An Antora documentation workspace in AsciiDoc. Pages are resources; tools read, \
             check, search and render them, and edit them when the server allows writing.",
        )
    }

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let state = self.state();
        let mut resources: Vec<Resource> = state
            .engine
            .workspace()
            .index
            .files()
            .map(|file| {
                let name = state.shown(&file.path);
                let mut resource =
                    Resource::new(file_uri(&file.path), name).with_mime_type(ASCIIDOC);
                if let Some(title) = &file.document.title {
                    resource = resource.with_title(title.text.clone());
                }
                resource
            })
            .collect();
        resources.sort_by(|a, b| a.uri.cmp(&b.uri));
        Ok(ListResourcesResult::with_all_items(resources))
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
        let state = self.state();
        let not_found = |message: String| ErrorData::resource_not_found(message, None);
        let path = if let Some(id) = request.uri.strip_prefix("antora:") {
            state
                .engine
                .resolve_resource_id(id, None)
                .map_err(|error| not_found(error.to_string()))?
        } else if let Some(path) = request.uri.strip_prefix("file://") {
            PathBuf::from(path)
        } else {
            return Err(not_found(format!("unknown resource {}", request.uri)));
        };
        let path = state.confine(&path.to_string_lossy()).map_err(not_found)?;
        let text = state.text(&path).map_err(not_found)?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, request.uri.clone()).with_mime_type(ASCIIDOC)
        ])
        .into())
    }
}
