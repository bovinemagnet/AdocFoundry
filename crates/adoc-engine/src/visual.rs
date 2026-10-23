//! The visual editor's model: for each document shown in visual or split mode, a model of its
//! text kept in step with every change, and the commands that edit it.
//!
//! A visual command turns a step on the model into source edits, applied to the document and
//! recorded in its undo history like any other edit. Steps that change no structure update the
//! model in place and are reconciled with a fresh parse once edits pause; anything else, and
//! any change from elsewhere, rebuilds the model from the source.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_dom::Dom;
use adoc_model::{apply_edits, Mark, MarkSet, Model, Step, StepResult};
use adoc_render::ViewTree;

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, EngineEvent, TextEdit,
};

/// A visual edit is reconciled with a fresh parse once edits pause this long; so is a model
/// that typing in the source editor made stale.
pub const MODEL_SETTLE: Duration = Duration::from_millis(100);

/// How a document's editor shows it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EditorMode {
    #[default]
    Source,
    Visual,
    /// Source and visual side by side.
    Split,
}

/// A document's visual model at one version, with the view tree built from the same parse for
/// drawing what the model keeps read-only (images, diagrams, included content).
#[derive(Clone, Debug, PartialEq)]
pub struct VisualDocument {
    pub document: DocumentId,
    pub version: u64,
    pub model: Model,
    pub view: ViewTree,
}

/// The arguments of a visual command.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualArgs {
    pub id: DocumentId,
    /// The document version the selection is in; a command for an older version is refused.
    pub version: u64,
    /// The selection, anchor to head, as source offsets.
    pub selection: SourceRange,
    /// The marks typed text carries.
    pub marks: MarkSet,
    pub value: VisualValue,
}

/// What a visual command works with besides the selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum VisualValue {
    #[default]
    None,
    Text(String),
    /// A link target and, optionally, its text.
    Link {
        target: String,
        text: Option<String>,
    },
}

struct Session {
    current: Arc<VisualDocument>,
    dom: Arc<Dom>,
    /// Every file the document was assembled from.
    sources: BTreeSet<PathBuf>,
    /// The document, or a file it includes, changed since the model was built.
    stale: bool,
    /// When to rebuild from a fresh parse.
    due: Option<Instant>,
    /// Diagrams the view is waiting for, by checksum.
    waiting: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct VisualState {
    modes: BTreeMap<DocumentId, EditorMode>,
    sessions: BTreeMap<DocumentId, Session>,
    /// The document a visual command is editing, whose model the command keeps current.
    applying: Option<DocumentId>,
}

impl VisualState {
    pub(crate) fn has_due(&self) -> bool {
        self.sessions.values().any(|session| session.due.is_some())
    }
}

impl Engine {
    #[must_use]
    pub fn editor_mode(&self, id: DocumentId) -> EditorMode {
        self.visual.modes.get(&id).copied().unwrap_or_default()
    }

    /// The visual model of `id`, while it is shown in visual or split mode.
    #[must_use]
    pub fn visual(&self, id: DocumentId) -> Option<Arc<VisualDocument>> {
        self.visual
            .sessions
            .get(&id)
            .map(|session| session.current.clone())
    }

    /// Rebuilds the visual model of `id` now if it is out of date.
    pub fn refresh_visual(&mut self, id: DocumentId) {
        if self
            .visual
            .sessions
            .get(&id)
            .is_some_and(|session| session.stale)
        {
            self.build_visual(id);
        }
    }

    pub(crate) fn set_editor_mode(&mut self, id: DocumentId, mode: EditorMode) {
        if self.editor_mode(id) == mode {
            return;
        }
        if mode == EditorMode::Source {
            self.visual.modes.remove(&id);
            self.visual.sessions.remove(&id);
        } else {
            self.visual.modes.insert(id, mode);
            if !self.visual.sessions.contains_key(&id) {
                self.build_visual(id);
            }
        }
        self.emit(EngineEvent::EditorModeChanged(id));
    }

    pub(crate) fn forget_visual(&mut self, id: DocumentId) {
        self.visual.modes.remove(&id);
        self.visual.sessions.remove(&id);
    }

    /// Parses `id` and builds its model and view now.
    fn build_visual(&mut self, id: DocumentId) {
        let parser = self.document_parser();
        let Some(input) = self.parse_input(id) else {
            return;
        };
        let (version, path, text) = (input.version, input.path.clone(), input.text.clone());
        let (dom, sources) = input.parse(parser.as_ref());
        let model = Model::build(&path, &text, &dom);
        let (view, wanted) = self.view_of(&path, &dom);
        let current = Arc::new(VisualDocument {
            document: id,
            version,
            model,
            view,
        });
        let unchanged = self.visual.sessions.get(&id).is_some_and(|session| {
            session.current.version == version && session.current.model == current.model
        });
        self.visual.sessions.insert(
            id,
            Session {
                current,
                dom: Arc::new(dom),
                sources,
                stale: false,
                due: None,
                waiting: wanted
                    .iter()
                    .map(adoc_diagrams::DiagramRequest::checksum)
                    .collect(),
            },
        );
        if !unchanged {
            self.emit(EngineEvent::ModelChanged { id, version });
        }
    }

    /// Marks the models assembled from `path` out of date, unless a visual command is
    /// editing that model itself.
    pub(crate) fn visual_source_changed(&mut self, path: &Path) {
        let due = self.clock.now() + MODEL_SETTLE;
        let applying = self.visual.applying;
        let documents = &self.documents;
        for (id, session) in &mut self.visual.sessions {
            if applying == Some(*id) {
                continue;
            }
            let own = documents
                .get(id)
                .is_some_and(|document| document.path == path);
            if own || session.sources.contains(path) {
                session.stale = true;
                session.due = Some(due);
            }
        }
    }

    /// Marks every model out of date, after a catalog or index change.
    pub(crate) fn visual_workspace_changed(&mut self) {
        let due = self.clock.now() + MODEL_SETTLE;
        for session in self.visual.sessions.values_mut() {
            session.stale = true;
            session.due = Some(due);
        }
    }

    /// Rebuilds every stale model now: after any command other than source typing.
    pub(crate) fn refresh_stale_visuals(&mut self) {
        let stale: Vec<DocumentId> = self
            .visual
            .sessions
            .iter()
            .filter(|(_, session)| session.stale)
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.build_visual(id);
        }
    }

    /// Rebuilds the models whose settle time has passed. Returns the number rebuilt.
    pub(crate) fn rebuild_due_visuals(&mut self) -> usize {
        let now = self.clock.now();
        let due: Vec<DocumentId> = self
            .visual
            .sessions
            .iter()
            .filter(|(_, session)| session.due.is_some_and(|due| due <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            self.build_visual(*id);
        }
        due.len()
    }

    /// Rebuilds the views waiting for a diagram that finished.
    pub(crate) fn visual_diagram_rendered(&mut self, checksum: &str) {
        let waiting: Vec<DocumentId> = self
            .visual
            .sessions
            .iter()
            .filter(|(_, session)| session.waiting.contains(checksum))
            .map(|(id, _)| *id)
            .collect();
        for id in waiting {
            let Some(session) = self.visual.sessions.get(&id) else {
                continue;
            };
            let (path, dom) = (session.current.model.path.clone(), session.dom.clone());
            let (view, wanted) = self.view_of(&path, &dom);
            if let Some(session) = self.visual.sessions.get_mut(&id) {
                let mut current = (*session.current).clone();
                current.view = view;
                session.current = Arc::new(current);
                session.waiting = wanted
                    .iter()
                    .map(adoc_diagrams::DiagramRequest::checksum)
                    .collect();
                let version = session.current.version;
                self.emit(EngineEvent::ModelChanged { id, version });
            }
        }
    }

    /// The current model of `args.id`, rebuilt first if a change from elsewhere left it stale;
    /// refused when the selection belongs to an older version of the document.
    fn visual_for(&mut self, args: &VisualArgs) -> Result<Arc<VisualDocument>, EngineError> {
        let version = self
            .document(args.id)
            .ok_or(EngineError::UnknownDocument(args.id))?
            .version;
        if args.version != version {
            return Err(EngineError::Refused(
                "the document changed; the edit was not applied".to_owned(),
            ));
        }
        self.refresh_visual(args.id);
        self.visual(args.id).ok_or_else(|| {
            EngineError::NotFound("the document is not shown in visual mode".to_owned())
        })
    }

    /// Applies a step's result to document `id`.
    fn apply_visual(
        &mut self,
        id: DocumentId,
        before: &str,
        result: StepResult,
        mode: UndoMode,
    ) -> Result<CommandOutcome, EngineError> {
        if result.edits.is_empty() {
            return Ok(CommandOutcome::Selection(result.selection));
        }
        // One edit, so the step is one undo entry: from the first change to the last.
        let edits = match result.edits.as_slice() {
            [edit] => vec![TextEdit::new(edit.range, edit.text.clone())],
            _ => whole_edit(before, &apply_edits(before, &result.edits))
                .into_iter()
                .collect(),
        };
        self.visual.applying = Some(id);
        let applied = self.apply_edits(id, edits, mode);
        self.visual.applying = None;
        applied?;
        let version = self.document(id).map_or(0, |document| document.version);
        let due = self.clock.now() + MODEL_SETTLE;
        match result.model {
            Some(model) => {
                if let Some(session) = self.visual.sessions.get_mut(&id) {
                    let mut current = (*session.current).clone();
                    current.version = version;
                    current.model = model;
                    session.current = Arc::new(current);
                    session.due = Some(due);
                }
                self.emit(EngineEvent::ModelChanged { id, version });
            }
            None => self.build_visual(id),
        }
        Ok(CommandOutcome::Selection(result.selection))
    }
}

/// The single edit turning `before` into `after`: their common prefix and suffix left alone.
fn whole_edit(before: &str, after: &str) -> Option<TextEdit> {
    if before == after {
        return None;
    }
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let longest = before.len().min(after.len()) - prefix;
    let mut suffix = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take(longest)
        .take_while(|(left, right)| left == right)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    Some(TextEdit::new(
        SourceRange::new(prefix, before.len() - suffix),
        &after[prefix..after.len() - suffix],
    ))
}

// --- commands --------------------------------------------------------------------------------

type Run = fn(&mut Engine, CommandArgs) -> Result<CommandOutcome, EngineError>;

pub(crate) fn register_visual(registry: &mut CommandRegistry) {
    use CommandCategory::{Edit, View};
    use CommandContext::{ActiveDocument, Selection};

    let commands: [(crate::CommandSpec, Run); 12] = [
        (
            spec(
                ids::VIEW_SHOW_SOURCE,
                "Show Source",
                View,
                Some("cmd-k s"),
                ActiveDocument,
            ),
            |engine, args| show(engine, args, EditorMode::Source),
        ),
        (
            spec(
                ids::VIEW_SHOW_VISUAL,
                "Show Visual Editor",
                View,
                Some("cmd-k w"),
                ActiveDocument,
            ),
            |engine, args| show(engine, args, EditorMode::Visual),
        ),
        (
            spec(
                ids::VIEW_SHOW_SPLIT,
                "Show Source and Visual Editor",
                View,
                Some("cmd-k d"),
                ActiveDocument,
            ),
            |engine, args| show(engine, args, EditorMode::Split),
        ),
        (
            spec(ids::VISUAL_INSERT_TEXT, "Type", Edit, None, Selection),
            visual_insert_text,
        ),
        (
            spec(
                ids::VISUAL_DELETE_BACKWARD,
                "Delete Backward",
                Edit,
                None,
                Selection,
            ),
            |engine, args| visual_delete(engine, args, false),
        ),
        (
            spec(
                ids::VISUAL_DELETE_FORWARD,
                "Delete Forward",
                Edit,
                None,
                Selection,
            ),
            |engine, args| visual_delete(engine, args, true),
        ),
        (
            spec(
                ids::EDIT_TOGGLE_STRONG,
                "Bold",
                Edit,
                Some("cmd-b"),
                Selection,
            ),
            |engine, args| toggle_mark(engine, args, ids::EDIT_TOGGLE_STRONG, Mark::Strong),
        ),
        (
            spec(
                ids::EDIT_TOGGLE_EMPHASIS,
                "Italic",
                Edit,
                Some("cmd-i"),
                Selection,
            ),
            |engine, args| toggle_mark(engine, args, ids::EDIT_TOGGLE_EMPHASIS, Mark::Emphasis),
        ),
        (
            spec(
                ids::EDIT_TOGGLE_CODE,
                "Code",
                Edit,
                Some("cmd-e"),
                Selection,
            ),
            |engine, args| toggle_mark(engine, args, ids::EDIT_TOGGLE_CODE, Mark::Code),
        ),
        (
            spec(
                ids::EDIT_TOGGLE_HIGHLIGHT,
                "Highlight",
                Edit,
                None,
                Selection,
            ),
            |engine, args| toggle_mark(engine, args, ids::EDIT_TOGGLE_HIGHLIGHT, Mark::Highlight),
        ),
        (
            spec(
                ids::EDIT_INSERT_LINK,
                "Insert Link",
                Edit,
                Some("cmd-shift-l"),
                Selection,
            ),
            insert_link,
        ),
        (
            spec(ids::EDIT_REMOVE_LINK, "Remove Link", Edit, None, Selection),
            remove_link,
        ),
    ];
    for (spec, run) in commands {
        register(registry, spec, run);
    }
}

fn show(
    engine: &mut Engine,
    args: CommandArgs,
    mode: EditorMode,
) -> Result<CommandOutcome, EngineError> {
    let id = match args {
        CommandArgs::Document(id) => id,
        _ => engine
            .active()
            .ok_or_else(|| EngineError::NotFound("no document is open".to_owned()))?,
    };
    if engine.document(id).is_none() {
        return Err(EngineError::UnknownDocument(id));
    }
    engine.set_editor_mode(id, mode);
    Ok(CommandOutcome::Done)
}

fn visual_args(command: crate::CommandId, args: CommandArgs) -> Result<VisualArgs, EngineError> {
    match args {
        CommandArgs::Visual(args) => Ok(args),
        _ => Err(EngineError::InvalidArguments {
            command,
            expected: "visual arguments",
        }),
    }
}

fn refused(error: adoc_model::StepError) -> EngineError {
    EngineError::Refused(error.to_string())
}

/// The selection as an ordered range.
fn ordered(selection: SourceRange) -> SourceRange {
    SourceRange::new(
        selection.start.min(selection.end),
        selection.start.max(selection.end),
    )
}

/// Types text over the selection: one undo step per run of typing.
fn visual_insert_text(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let args = visual_args(ids::VISUAL_INSERT_TEXT, args)?;
    let VisualValue::Text(text) = &args.value else {
        return Err(EngineError::InvalidArguments {
            command: ids::VISUAL_INSERT_TEXT,
            expected: "text",
        });
    };
    let visual = engine.visual_for(&args)?;
    let model = &visual.model;
    let range = ordered(args.selection);
    // Typing over a selection deletes it first; the two make one edit.
    let (model_after_delete, at) = if range.is_empty() {
        (None, range.start)
    } else {
        let deleted = model.apply(&Step::Delete { range }).map_err(refused)?;
        let at = deleted.selection.start;
        let rest = deleted.model.ok_or_else(|| {
            EngineError::Refused("select text inside one block to type over it".to_owned())
        })?;
        (Some(rest), at)
    };
    let base = model_after_delete.as_ref().unwrap_or(model);
    let inserted = base
        .apply(&Step::InsertText {
            at,
            text: text.clone(),
            marks: args.marks,
        })
        .map_err(refused)?;
    let edits = if range.is_empty() {
        inserted.edits.clone()
    } else {
        let after = apply_edits(&base.source, &inserted.edits);
        whole_edit(&model.source, &after)
            .map(|edit| vec![adoc_model::SourceEdit::new(edit.range, edit.replacement)])
            .unwrap_or_default()
    };
    let typed = range.is_empty()
        && text.chars().count() == 1
        && edits.len() == 1
        && edits[0].range.is_empty();
    let mode = if typed {
        UndoMode::Typing
    } else {
        UndoMode::Group(engine.new_group())
    };
    let source = model.source.clone();
    let result = StepResult {
        edits,
        selection: inserted.selection,
        model: inserted.model,
    };
    engine.apply_visual(args.id, &source, result, mode)
}

/// Deletes the selection, or the character before (or after) the caret.
fn visual_delete(
    engine: &mut Engine,
    args: CommandArgs,
    forward: bool,
) -> Result<CommandOutcome, EngineError> {
    let command = if forward {
        ids::VISUAL_DELETE_FORWARD
    } else {
        ids::VISUAL_DELETE_BACKWARD
    };
    let args = visual_args(command, args)?;
    let visual = engine.visual_for(&args)?;
    let model = &visual.model;
    let mut range = ordered(args.selection);
    if range.is_empty() {
        let Some((_, _, text)) = model.text_block_at(range.start) else {
            return Err(EngineError::Refused(
                "this text is not editable here; edit it in the source".to_owned(),
            ));
        };
        let display = text.display_offset(range.start).unwrap_or(0);
        let next = if forward {
            text.display[display..]
                .chars()
                .next()
                .map(|character| display + character.len_utf8())
        } else {
            text.display[..display]
                .chars()
                .next_back()
                .map(|character| display - character.len_utf8())
        };
        let Some(next) = next else {
            // At the edge of the block: joining blocks comes with block editing.
            return Ok(CommandOutcome::Selection(args.selection));
        };
        let stop = |display: usize| text.caret_stops(display).first().map(|(at, _)| *at);
        let (from, to) = if forward {
            (Some(range.start), stop(next))
        } else {
            (stop(next), Some(range.start))
        };
        let (Some(from), Some(to)) = (from, to) else {
            return Ok(CommandOutcome::Selection(args.selection));
        };
        range = SourceRange::new(from.min(to), from.max(to));
    }
    let result = model.apply(&Step::Delete { range }).map_err(refused)?;
    let source = model.source.clone();
    let group = engine.new_group();
    engine.apply_visual(args.id, &source, result, UndoMode::Group(group))
}

/// Runs `step` on the model of `args.id` as one undo step.
fn visual_step(
    engine: &mut Engine,
    args: &VisualArgs,
    step: Step,
) -> Result<CommandOutcome, EngineError> {
    let visual = engine.visual_for(args)?;
    let result = visual.model.apply(&step).map_err(refused)?;
    let source = visual.model.source.clone();
    let group = engine.new_group();
    engine.apply_visual(args.id, &source, result, UndoMode::Group(group))
}

/// Adds `mark` to the selection, or removes it where all of the selection has it.
fn toggle_mark(
    engine: &mut Engine,
    args: CommandArgs,
    command: crate::CommandId,
    mark: Mark,
) -> Result<CommandOutcome, EngineError> {
    let args = visual_args(command, args)?;
    let range = ordered(args.selection);
    visual_step(engine, &args, Step::ToggleMark { range, mark })
}

/// Makes the selection a link, or changes the link it is in.
fn insert_link(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let args = visual_args(ids::EDIT_INSERT_LINK, args)?;
    let VisualValue::Link { target, text } = args.value.clone() else {
        return Err(EngineError::InvalidArguments {
            command: ids::EDIT_INSERT_LINK,
            expected: "a link target",
        });
    };
    let range = ordered(args.selection);
    visual_step(
        engine,
        &args,
        Step::SetLink {
            range,
            target,
            text,
        },
    )
}

/// Replaces the link at the caret with its text.
fn remove_link(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let args = visual_args(ids::EDIT_REMOVE_LINK, args)?;
    let at = args.selection.end;
    visual_step(engine, &args, Step::RemoveLink { at })
}
