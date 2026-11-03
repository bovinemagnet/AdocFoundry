//! Staged changes (MCP PRD §7.5): `preview_changes` works out a change and checks it without
//! touching a file or a buffer; `apply_changes` makes it, only while every file still holds
//! what the preview saw. Previews belong to the session that made them (`Previews`), under
//! unguessable ids, a few at a time, for a while: see the limits below.
//!
//! What the server guarantees: the preview writes nothing; an apply against a file that changed
//! since, a file to make that now exists, or a path that now leads outside the roots changes
//! nothing; the change is one undo group in the engine. Saving is not atomic across files: each
//! changed page is saved in turn, and a save that fails is reported with what was and was not
//! saved, the change staying in the engine's buffers.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use adoc_core::{DiagnosticSeverity, LineIndex, SourceRange};
use adoc_engine::{
    commands::ids, line_diff, text_hash, ChangeSet, CommandArgs, DiffLine, EngineError, NewPage,
    StagedFile, TextEdit,
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    responses::{ErrorCode, Failure},
    schemas,
    server::{Answer, Server, State},
};

/// How long a preview may be applied after it is made.
pub const PREVIEW_TTL: Duration = Duration::from_secs(15 * 60);

/// How many previews a session keeps at once; the oldest goes first.
pub const MAX_PREVIEWS: usize = 16;

/// How many bytes of text a session's previews hold at most.
pub const MAX_STAGED_BYTES: usize = 4 * 1024 * 1024;

/// How many operations one preview takes.
pub const MAX_OPERATIONS: usize = 64;

const SCHEMA_VERSION: u32 = 1;

/// What a file must hold when a preview is applied: its hash then, or nothing.
#[derive(Clone, Debug)]
struct Expected {
    path: PathBuf,
    /// `None` for a file the change makes, which must still not exist.
    hash: Option<String>,
}

struct Stored {
    changes: ChangeSet,
    expected: Vec<Expected>,
    made: Instant,
    bytes: usize,
}

/// One session's previews and the answers its applies gave, by idempotency key.
#[derive(Default)]
pub(crate) struct Previews {
    stored: HashMap<String, Stored>,
    answered: HashMap<String, (Instant, Result<Value, Failure>)>,
}

impl Previews {
    fn expire(&mut self, now: Instant) {
        self.stored
            .retain(|_, stored| now.duration_since(stored.made) < PREVIEW_TTL);
        self.answered
            .retain(|_, (when, _)| now.duration_since(*when) < PREVIEW_TTL);
    }

    fn bytes(&self) -> usize {
        self.stored.values().map(|stored| stored.bytes).sum()
    }

    /// Keeps `stored` under a new id, dropping the oldest previews to stay within the limits.
    fn keep(&mut self, stored: Stored) -> Result<String, Failure> {
        if stored.bytes > MAX_STAGED_BYTES {
            return Err(Failure::new(
                ErrorCode::ResultTooLarge,
                format!(
                    "the change holds {} bytes of text, over the {MAX_STAGED_BYTES} a session keeps",
                    stored.bytes
                ),
            ));
        }
        while self.stored.len() >= MAX_PREVIEWS || self.bytes() + stored.bytes > MAX_STAGED_BYTES {
            let Some(oldest) = self
                .stored
                .iter()
                .min_by_key(|(_, stored)| stored.made)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.stored.remove(&oldest);
        }
        let id = preview_id()?;
        self.stored.insert(id.clone(), stored);
        Ok(id)
    }
}

/// 16 random bytes in hex, which no one guesses.
fn preview_id() -> Result<String, Failure> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| Failure::internal(format!("no random preview id: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Deserialize, JsonSchema)]
struct Range {
    start: usize,
    end: usize,
}

/// One operation of a change.
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Operation {
    /// Replace text in an existing file: `old_text` that occurs exactly once, or a byte
    /// `range`.
    Replace {
        path: Option<String>,
        resource_id: Option<String>,
        old_text: Option<String>,
        range: Option<Range>,
        new_text: String,
        /// The SHA-256 (hex) the file must hold now, as `get_page_context` or an earlier
        /// preview gives it; checked again when the change is applied.
        expected_hash: Option<String>,
    },
    /// Make a page in a module's `pages` folder, as New Page does, with an entry in the
    /// module's navigation file when `add_to_navigation`.
    CreatePage {
        component: String,
        /// `~` for the versionless version; needed when the component has several.
        version: Option<String>,
        module: String,
        title: String,
        /// The file's path in the `pages` folder; named from the title without it.
        file_name: Option<String>,
        #[serde(default)]
        add_to_navigation: bool,
    },
    /// Make a partial in a module's `partials` folder.
    CreatePartial {
        component: String,
        version: Option<String>,
        module: String,
        file_name: String,
        #[serde(default)]
        text: String,
    },
    /// Rename or move a page, rewriting the xrefs, navigation entries and includes that name
    /// it, as Rename Page does.
    Rename {
        path: Option<String>,
        resource_id: Option<String>,
        /// The new file name, or a path relative to the page's folder.
        new_name: String,
        expected_hash: Option<String>,
    },
}

#[derive(Deserialize, JsonSchema)]
struct PreviewArgs {
    /// The operations, applied in order as one change.
    operations: Vec<Operation>,
}

#[derive(Deserialize, JsonSchema)]
struct ApplyArgs {
    /// The `preview_id` `preview_changes` gave, in this session.
    preview_id: String,
    /// A key of the client's choosing: applying again with it gives the first answer rather
    /// than applying twice.
    idempotency_key: Option<String>,
    /// Write every changed page to disk; true by default. Otherwise the change stays in the
    /// server's buffers, where `read_page` shows it.
    save: Option<bool>,
}

/// The module root of `component`'s `version` (or its only version) and `module`.
fn module_root(
    state: &State,
    component: &str,
    version: Option<&str>,
    module: &str,
) -> Result<PathBuf, Failure> {
    let catalog = &state.engine.workspace().antora;
    let version = match version {
        Some("~") => None,
        Some(version) => Some(version.to_owned()),
        None => {
            let versions = catalog.versions_of(component);
            match versions.as_slice() {
                [] => {
                    return Err(Failure::not_found(format!(
                        "no component `{component}` in the workspace"
                    )))
                }
                [only] => only.version.clone(),
                several => {
                    let names: Vec<String> = several
                        .iter()
                        .map(|descriptor| descriptor.version.clone().unwrap_or_else(|| "~".into()))
                        .collect();
                    return Err(Failure::invalid(format!(
                        "`{component}` has several versions ({}); give the `version`",
                        names.join(", ")
                    )));
                }
            }
        }
    };
    catalog
        .module(component, version.as_deref(), module)
        .map(|module| module.root.clone())
        .ok_or_else(|| {
            Failure::not_found(format!(
                "no module `{module}` in `{component}` at version `{}`",
                version.as_deref().unwrap_or("~")
            ))
        })
}

/// A plain relative file name, with no `..`, root or drive.
fn plain(name: &str) -> Result<PathBuf, Failure> {
    let path = PathBuf::from(name);
    if name.is_empty()
        || !path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err(Failure::invalid(format!(
            "`{name}` is not a file name inside the folder"
        )));
    }
    Ok(path)
}

fn check_hash(state: &State, path: &Path, expected: Option<&String>) -> Result<(), Failure> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let now = state.engine.current_text(path).map(|text| text_hash(&text));
    if now.as_ref() != Some(expected) {
        return Err(Failure::new(
            ErrorCode::Conflict,
            format!(
                "{} does not hold the text `expected_hash` names",
                state.shown(path)
            ),
        ));
    }
    Ok(())
}

/// The change one operation makes, worked out against the workspace as it is.
fn stage(state: &mut State, operation: &Operation) -> Result<ChangeSet, Failure> {
    let engine_failure = |error: EngineError| match error {
        EngineError::InvalidRequest(_) | EngineError::InvalidEdit(_) => {
            Failure::invalid(error.to_string())
        }
        other => Failure::from(other),
    };
    match operation {
        Operation::Replace {
            path,
            resource_id,
            old_text,
            range,
            new_text,
            expected_hash,
        } => {
            let path = state.target(path.as_deref(), resource_id.as_deref(), None)?;
            check_hash(state, &path, expected_hash.as_ref())?;
            let text = state.engine.current_text(&path).ok_or_else(|| {
                Failure::not_found(format!("{} cannot be read", state.shown(&path)))
            })?;
            let range = match (old_text, range) {
                (Some(old), None) => {
                    let found: Vec<usize> =
                        text.match_indices(old.as_str()).map(|(at, _)| at).collect();
                    match found.as_slice() {
                        [at] => SourceRange::new(*at, at + old.len()),
                        [] => {
                            return Err(Failure::new(
                                ErrorCode::Conflict,
                                format!("`old_text` is not in {}", state.shown(&path)),
                            ))
                        }
                        _ => {
                            return Err(Failure::invalid(format!(
                                "`old_text` occurs {} times in {}; give more context",
                                found.len(),
                                state.shown(&path)
                            )))
                        }
                    }
                }
                (None, Some(range)) => SourceRange::new(range.start, range.end),
                _ => return Err(Failure::invalid("give either `old_text` or `range`")),
            };
            state
                .engine
                .stage_edits(&path, vec![TextEdit::new(range, new_text.clone())])
                .map_err(engine_failure)
        }
        Operation::CreatePage {
            component,
            version,
            module,
            title,
            file_name,
            add_to_navigation,
        } => {
            let root = module_root(state, component, version.as_deref(), module)?;
            let file = file_name.as_deref().map(plain).transpose()?;
            if let Some(file) = &file {
                state.confine(&root.join("pages").join(file).to_string_lossy())?;
            }
            state
                .engine
                .stage_new_page(&NewPage {
                    module: root,
                    title: title.clone(),
                    add_to_nav: *add_to_navigation,
                    file,
                })
                .map_err(engine_failure)
        }
        Operation::CreatePartial {
            component,
            version,
            module,
            file_name,
            text,
        } => {
            let root = module_root(state, component, version.as_deref(), module)?;
            let path = root.join("partials").join(plain(file_name)?);
            let path = state.confine(&path.to_string_lossy())?;
            state
                .engine
                .stage_new_partial(&path, text)
                .map_err(engine_failure)
        }
        Operation::Rename {
            path,
            resource_id,
            new_name,
            expected_hash,
        } => {
            let path = state.target(path.as_deref(), resource_id.as_deref(), None)?;
            check_hash(state, &path, expected_hash.as_ref())?;
            let parent = path.parent().unwrap_or_else(|| Path::new(""));
            let new_path = state.confine(&parent.join(new_name).to_string_lossy())?;
            state
                .engine
                .stage_rename(&path, &new_path)
                .map_err(engine_failure)
        }
    }
}

/// `before` and `after` as unified diff lines, `-` removed, `+` added, ` ` context.
fn diff(before: &str, after: &str) -> String {
    let mut out = String::new();
    for line in line_diff(before, after) {
        let (mark, text) = match line {
            DiffLine::Removed(text) => ('-', text),
            DiffLine::Added(text) => ('+', text),
            DiffLine::Same(text) => (' ', text),
            DiffLine::Skipped(count) => {
                out.push_str(&format!("@@ {count} unchanged lines @@\n"));
                continue;
            }
        };
        out.push(mark);
        out.push_str(&text);
        out.push('\n');
    }
    out
}

fn severity(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Information => "info",
    }
}

/// A staged file as the preview reports it.
fn file_report(state: &State, file: &StagedFile) -> Value {
    let lines = LineIndex::new(&file.after);
    let diagnostics: Vec<Value> = file
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let (line, column) = lines.line_column(diagnostic.range.start);
            json!({
                "severity": severity(diagnostic.severity),
                "code": diagnostic.code.as_str(),
                "message": diagnostic.message,
                "line": line + 1,
                "column": column + 1,
            })
        })
        .collect();
    let kind = match (&file.before, &file.moved_to) {
        (None, _) => "create",
        (Some(_), Some(_)) => "move",
        (Some(_), None) => "edit",
    };
    json!({
        "kind": kind,
        "path": state.shown(&file.path),
        "to": file.moved_to.as_deref().map(|to| state.shown(to)),
        "resource_id": state
            .engine
            .resource_id(file.moved_to.as_deref().unwrap_or(&file.path)),
        "before_hash": file.before.as_deref().map(text_hash),
        "after_hash": text_hash(&file.after),
        "diff": diff(file.before.as_deref().unwrap_or_default(), &file.after),
        "diagnostics": diagnostics,
    })
}

fn unix_seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[tool_router(router = changes_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Work out a change (text replacements, new pages and partials, renames) and check it, without writing anything: each file's diff, hashes and diagnostics after, and a `preview_id` that `apply_changes` makes it with while every file still holds what the preview saw. The preview belongs to this session and expires after 15 minutes.",
        output_schema = schemas::preview_changes(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn preview_changes(&self, Parameters(args): Parameters<PreviewArgs>) -> Answer {
        let previews = self.previews.clone();
        let allow_write = self.allow_write();
        self.run(move |state| {
            if args.operations.is_empty() {
                return Err(Failure::invalid("give at least one operation"));
            }
            if args.operations.len() > MAX_OPERATIONS {
                return Err(Failure::invalid(format!(
                    "{} operations; a preview takes at most {MAX_OPERATIONS}",
                    args.operations.len()
                )));
            }
            let mut changes = ChangeSet::default();
            for (number, operation) in args.operations.iter().enumerate() {
                let staged = stage(state, operation).map_err(|failure| {
                    Failure::new(
                        failure.code,
                        format!("operation {}: {}", number + 1, failure.message),
                    )
                })?;
                changes.merge(staged).map_err(|error| {
                    Failure::new(
                        ErrorCode::InvalidArgument,
                        format!("operation {}: {error}", number + 1),
                    )
                })?;
            }
            let files = state.engine.check_staged(&changes).map_err(Failure::from)?;
            let expected: Vec<Expected> = files
                .iter()
                .map(|file| Expected {
                    path: file.path.clone(),
                    hash: file.before.as_deref().map(text_hash),
                })
                .collect();
            let bytes = changes.bytes();
            let now = Instant::now();
            let mut previews = crate::server::lock_previews(&previews);
            previews.expire(now);
            let id = previews.keep(Stored {
                changes,
                expected,
                made: now,
                bytes,
            })?;
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "preview_id": id,
                "expires_at": unix_seconds(SystemTime::now() + PREVIEW_TTL),
                "expires_in_seconds": PREVIEW_TTL.as_secs(),
                "workspace_revision": state.engine.workspace_revision(),
                "files": files.iter().map(|file| file_report(state, file)).collect::<Vec<_>>(),
                "can_apply": allow_write,
                "reason": (!allow_write).then_some(
                    "this server was started without --allow-write, so apply_changes is not offered"
                ),
            }))
        })
        .await
    }

    #[tool(
        description = "Make a change `preview_changes` worked out, as one undo step, only if every file still holds what the preview saw, every file to make is still absent and every path is still inside the roots; otherwise nothing changes. With `save` (the default) each changed page is then written to disk; a failed save is reported with what was and was not saved.",
        output_schema = schemas::apply_changes(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn apply_changes(&self, Parameters(args): Parameters<ApplyArgs>) -> Answer {
        let previews = self.previews.clone();
        self.run(move |state| {
            let now = Instant::now();
            let stored = {
                let mut previews = crate::server::lock_previews(&previews);
                previews.expire(now);
                if let Some(key) = &args.idempotency_key {
                    if let Some((_, answered)) = previews.answered.get(key) {
                        return answered.clone();
                    }
                }
                previews.stored.remove(&args.preview_id)
            };
            let Some(stored) = stored else {
                return Err(Failure::new(
                    ErrorCode::StalePreview,
                    "no preview with that id in this session: it was applied, expired, or made \
                     in another session; preview the change again",
                ));
            };
            let answer = apply(state, &stored, args.save.unwrap_or(true));
            if let Some(key) = args.idempotency_key {
                crate::server::lock_previews(&previews)
                    .answered
                    .insert(key, (now, answer.clone()));
            }
            answer
        })
        .await
    }
}

/// Checks `stored` against the workspace as it is now, then applies and saves it.
fn apply(state: &mut State, stored: &Stored, save: bool) -> Result<Value, Failure> {
    // Every path again, now: a folder may have become a link out of the roots since.
    for path in stored.changes.paths() {
        state.confine(&path.to_string_lossy())?;
    }
    for expected in &stored.expected {
        match &expected.hash {
            None => {
                if expected.path.exists()
                    || state.engine.document_for_path(&expected.path).is_some()
                {
                    return Err(Failure::new(
                        ErrorCode::Conflict,
                        format!(
                            "{} exists now; preview the change again",
                            state.shown(&expected.path)
                        ),
                    ));
                }
            }
            Some(hash) => {
                let now = state
                    .engine
                    .current_text(&expected.path)
                    .map(|text| text_hash(&text));
                if now.as_ref() != Some(hash) {
                    return Err(Failure::new(
                        ErrorCode::StalePreview,
                        format!(
                            "{} has changed since the preview; preview the change again",
                            state.shown(&expected.path)
                        ),
                    ));
                }
            }
        }
    }
    state
        .engine
        .apply_staged(&stored.changes)
        .map_err(|error| match error {
            EngineError::PreviewStale { .. } => {
                Failure::new(ErrorCode::StalePreview, error.to_string())
            }
            EngineError::InvalidRequest(_) => Failure::new(ErrorCode::Conflict, error.to_string()),
            other => Failure::from(other),
        })?;
    let mut changed: Vec<PathBuf> = stored
        .changes
        .creates
        .iter()
        .map(|create| create.path.clone())
        .chain(stored.changes.edits.iter().map(|file| {
            stored
                .changes
                .moves
                .iter()
                .find(|file_move| file_move.from == file.path)
                .map_or(file.path.clone(), |file_move| file_move.to.clone())
        }))
        .chain(
            stored
                .changes
                .moves
                .iter()
                .map(|file_move| file_move.to.clone()),
        )
        .collect();
    changed.sort();
    changed.dedup();
    let shown: Vec<String> = changed.iter().map(|path| state.shown(path)).collect();
    let mut saved = Vec::new();
    let mut failed = Vec::new();
    if save {
        for path in &changed {
            let Some(id) = state.engine.document_for_path(path) else {
                continue;
            };
            match state
                .engine
                .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
            {
                Ok(_) => saved.push(state.shown(path)),
                Err(error) => {
                    failed.push(json!({ "path": state.shown(path), "message": error.to_string() }))
                }
            }
        }
    }
    if !failed.is_empty() {
        return Err(Failure::new(
            ErrorCode::InternalError,
            format!(
                "the change was made, but {} of {} pages could not be saved; it stays in the \
                 server's buffers",
                failed.len(),
                saved.len() + failed.len()
            ),
        )
        .with_details(json!({
            "applied": true,
            "saved": saved,
            "failed": failed,
            "recovery": "The unsaved pages keep the change: fix the cause and save them with \
                         `edit_page` and `save: true` (no edits), or read them with `read_page`.",
        })));
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "applied": true,
        "saved": save,
        "changed": shown,
        "resource_ids": changed.iter().map(|path| state.engine.resource_id(path)).collect::<Vec<_>>(),
        "workspace_revision": state.engine.workspace_revision(),
    }))
}
