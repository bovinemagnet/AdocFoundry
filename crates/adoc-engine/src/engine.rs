use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc,
    },
};

use adoc_core::SourceRange;
use adoc_index::normalize_path;
use adoc_parser::parse;
use ropey::Rope;

use crate::{
    commands,
    document::{hash_text, uri_for, DocumentState},
    edit,
    undo::Typing,
    Clock, CommandArgs, CommandId, CommandOutcome, CommandRegistry, DocumentId, EngineError,
    EngineEvent, OpenDocument, Settings, SystemClock, TextEdit, UndoEntry, UndoGroup, UndoStack,
};

/// The headless editor engine: the single owner of workspace and document state.
///
/// The engine is single-threaded: a front end owns it and mutates it through `&mut self`. Long
/// work runs on worker threads whose results are applied by [`Engine::pump`].
pub struct Engine {
    settings: Settings,
    clock: Arc<dyn Clock>,
    registry: CommandRegistry,
    documents: BTreeMap<DocumentId, OpenDocument>,
    states: BTreeMap<DocumentId, DocumentState>,
    next_document: u64,
    next_group: u64,
    next_untitled: u64,
    active: Option<DocumentId>,
    subscribers: Vec<Sender<EngineEvent>>,
}

/// How an edit is recorded in undo history.
#[derive(Clone, Copy, Debug)]
pub(crate) enum UndoMode {
    /// A new entry in the given group.
    Group(UndoGroup),
    /// A typed character that may extend the previous typing run.
    Typing,
}

impl Engine {
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        Self::with_clock(settings, Arc::new(SystemClock))
    }

    /// Creates an engine with an injected clock, for deterministic tests of time-dependent
    /// behaviour.
    #[must_use]
    pub fn with_clock(settings: Settings, clock: Arc<dyn Clock>) -> Self {
        let mut registry = CommandRegistry::new();
        commands::register_builtin(&mut registry);
        Self {
            settings,
            clock,
            registry,
            documents: BTreeMap::new(),
            states: BTreeMap::new(),
            next_document: 1,
            next_group: 1,
            next_untitled: 1,
            active: None,
            subscribers: Vec::new(),
        }
    }

    // --- documents ---------------------------------------------------------------------------

    /// Opens the file at `path`, or returns the id of the document already open for it.
    pub fn open(&mut self, path: &Path) -> Result<DocumentId, EngineError> {
        let path = normalize_path(path);
        if let Some(id) = self.document_for_path(&path) {
            self.set_active(id);
            return Ok(id);
        }
        let text = fs::read_to_string(&path).map_err(|error| EngineError::io(&path, error))?;
        let id = self.insert_document(path, &text, false);
        self.set_active(id);
        Ok(id)
    }

    /// Opens a new, empty document that has no file on disk.
    pub fn open_untitled(&mut self) -> DocumentId {
        let path = PathBuf::from(format!("Untitled-{}.adoc", self.next_untitled));
        self.next_untitled += 1;
        let id = self.insert_document(path, "", true);
        self.set_active(id);
        id
    }

    fn insert_document(&mut self, path: PathBuf, text: &str, untitled: bool) -> DocumentId {
        let id = DocumentId(self.next_document);
        self.next_document += 1;
        let parsed = parse(&uri_for(&path), text).document;
        let diagnostics = parsed.diagnostics.clone();
        self.documents.insert(
            id,
            OpenDocument {
                id,
                path,
                text: Rope::from_str(text),
                version: 0,
                dirty: false,
                parsed,
                diagnostics,
                untitled,
            },
        );
        self.states.insert(
            id,
            DocumentState {
                undo: UndoStack::default(),
                saved_hash: hash_text(text),
            },
        );
        self.emit(EngineEvent::DocumentOpened(id));
        id
    }

    #[must_use]
    pub fn document(&self, id: DocumentId) -> Option<&OpenDocument> {
        self.documents.get(&id)
    }

    pub fn documents(&self) -> impl Iterator<Item = &OpenDocument> {
        self.documents.values()
    }

    /// The open document for `path`, if any.
    #[must_use]
    pub fn document_for_path(&self, path: &Path) -> Option<DocumentId> {
        let path = normalize_path(path);
        self.documents
            .values()
            .find(|document| document.path == path)
            .map(|document| document.id)
    }

    #[must_use]
    pub fn undo_stack(&self, id: DocumentId) -> Option<&UndoStack> {
        self.states.get(&id).map(|state| &state.undo)
    }

    pub fn close(&mut self, id: DocumentId) -> Result<(), EngineError> {
        self.documents
            .remove(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        self.states.remove(&id);
        if self.active == Some(id) {
            self.active = self.documents.keys().next_back().copied();
            self.emit(EngineEvent::NavigationChanged);
        }
        self.emit(EngineEvent::DocumentClosed(id));
        Ok(())
    }

    pub fn save(&mut self, id: DocumentId) -> Result<(), EngineError> {
        let document = self
            .documents
            .get_mut(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        if document.untitled {
            return Err(EngineError::NotFound(format!(
                "{} has never been saved and has no file",
                document.path.display()
            )));
        }
        let text = document.text.to_string();
        fs::write(&document.path, &text).map_err(|error| EngineError::io(&document.path, error))?;
        document.dirty = false;
        if let Some(state) = self.states.get_mut(&id) {
            state.saved_hash = hash_text(&text);
        }
        self.emit(EngineEvent::DocumentSaved(id));
        Ok(())
    }

    #[must_use]
    pub fn active(&self) -> Option<DocumentId> {
        self.active
    }

    pub fn set_active(&mut self, id: DocumentId) {
        if self.documents.contains_key(&id) && self.active != Some(id) {
            self.active = Some(id);
            self.emit(EngineEvent::NavigationChanged);
        }
    }

    // --- commands ----------------------------------------------------------------------------

    pub fn invoke(
        &mut self,
        command: CommandId,
        args: CommandArgs,
    ) -> Result<CommandOutcome, EngineError> {
        let handler = self
            .registry
            .shared(command)
            .ok_or(EngineError::UnknownCommand(command))?;
        let result = handler.run(self, args);
        if let Err(error) = &result {
            self.emit(EngineEvent::CommandFailed {
                command,
                error: error.to_string(),
            });
        }
        result
    }

    #[must_use]
    pub fn commands(&self) -> &CommandRegistry {
        &self.registry
    }

    /// Registers an additional command, for front ends and future integrations.
    pub fn commands_mut(&mut self) -> &mut CommandRegistry {
        &mut self.registry
    }

    // --- undo --------------------------------------------------------------------------------

    pub(crate) fn new_group(&mut self) -> UndoGroup {
        let group = UndoGroup(self.next_group);
        self.next_group += 1;
        group
    }

    /// Undoes the most recent group of `id`. When the group spans several documents, the group
    /// is undone in all of them.
    pub fn undo(&mut self, id: DocumentId) -> bool {
        let Some(group) = self
            .states
            .get(&id)
            .and_then(|state| state.undo.top_group())
        else {
            return false;
        };
        for document in self.documents_with_top_group(group, false) {
            let Some(entry) = self
                .states
                .get_mut(&document)
                .and_then(|state| state.undo.pop_undo())
            else {
                continue;
            };
            if self.edit_document(document, entry.inverse.clone()).is_ok() {
                if let Some(state) = self.states.get_mut(&document) {
                    state.undo.push_redo(entry);
                }
            }
        }
        true
    }

    /// Redoes the most recently undone group of `id`, across every document it spans.
    pub fn redo(&mut self, id: DocumentId) -> bool {
        let Some(group) = self
            .states
            .get(&id)
            .and_then(|state| state.undo.redo_group())
        else {
            return false;
        };
        for document in self.documents_with_top_group(group, true) {
            let Some(entry) = self
                .states
                .get_mut(&document)
                .and_then(|state| state.undo.pop_redo())
            else {
                continue;
            };
            if self.edit_document(document, entry.forward.clone()).is_ok() {
                if let Some(state) = self.states.get_mut(&document) {
                    state.undo.push_undo_keep_redo(entry);
                }
            }
        }
        true
    }

    fn documents_with_top_group(&self, group: UndoGroup, redo: bool) -> Vec<DocumentId> {
        self.states
            .iter()
            .filter(|(_, state)| {
                let top = if redo {
                    state.undo.redo_group()
                } else {
                    state.undo.top_group()
                };
                top == Some(group)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    // --- editing -----------------------------------------------------------------------------

    /// Applies `edits` to document `id` and records them in undo history.
    pub(crate) fn apply_edits(
        &mut self,
        id: DocumentId,
        edits: Vec<TextEdit>,
        mode: UndoMode,
    ) -> Result<(), EngineError> {
        let typed = match (&mode, edits.as_slice()) {
            (UndoMode::Typing, [edit]) if edit.range.is_empty() => {
                Some((edit.range.start, edit.replacement.clone()))
            }
            _ => None,
        };
        let applied = self.edit_document(id, edits)?;
        let now = self.clock.now();
        if let Some((offset, text)) = &typed {
            let coalesced = self.states.get_mut(&id).is_some_and(|state| {
                state
                    .undo
                    .coalesce_typing(*offset, text, now, applied.version_after)
            });
            if coalesced {
                return Ok(());
            }
        }
        let group = match mode {
            UndoMode::Group(group) => group,
            UndoMode::Typing => self.new_group(),
        };
        let typing = typed.map(|(offset, text)| Typing {
            last: now,
            end: offset + text.len(),
        });
        if let Some(state) = self.states.get_mut(&id) {
            state.undo.push(UndoEntry {
                group,
                inverse: applied.inverse,
                forward: applied.forward,
                version_before: applied.version_before,
                version_after: applied.version_after,
                typing,
            });
        }
        Ok(())
    }

    /// Applies edits without touching undo history: updates the rope, bumps the version,
    /// re-parses and publishes the change.
    fn edit_document(
        &mut self,
        id: DocumentId,
        edits: Vec<TextEdit>,
    ) -> Result<AppliedEdits, EngineError> {
        let document = self
            .documents
            .get_mut(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        let forward = edit::validate(&document.text, edits)?;
        let inverse = edit::apply(&mut document.text, &forward);
        let version_before = document.version;
        document.version += 1;
        let text = document.text.to_string();
        document.parsed = parse(&uri_for(&document.path), &text).document;
        document.diagnostics = document.parsed.diagnostics.clone();
        let saved_hash = self.states.get(&id).map_or(0, |state| state.saved_hash);
        document.dirty = hash_text(&text) != saved_hash;
        let version_after = document.version;
        let path = document.path.clone();

        self.emit(EngineEvent::DocumentChanged {
            id,
            version: version_after,
            edits: forward.clone(),
        });
        self.emit(EngineEvent::DiagnosticsUpdated { path });
        Ok(AppliedEdits {
            forward,
            inverse,
            version_before,
            version_after,
        })
    }

    // --- events ------------------------------------------------------------------------------

    /// Returns a receiver of every event published from now on. One per front end.
    pub fn subscribe(&mut self) -> Receiver<EngineEvent> {
        let (sender, receiver) = mpsc::channel();
        self.subscribers.push(sender);
        receiver
    }

    /// Applies results from worker threads and the file watcher. Returns the number applied.
    pub fn pump(&mut self) -> usize {
        0
    }

    pub(crate) fn emit(&mut self, event: EngineEvent) {
        self.subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }

    // --- settings ----------------------------------------------------------------------------

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn reload_settings(&mut self) -> Result<(), EngineError> {
        self.settings.reload()?;
        self.emit(EngineEvent::SettingsChanged);
        Ok(())
    }

    /// The text of document `id` in `range`.
    pub fn text_in(&self, id: DocumentId, range: SourceRange) -> Option<String> {
        let document = self.documents.get(&id)?;
        if range.end > document.text.len() || range.start > range.end {
            return None;
        }
        Some(document.text.slice(range.start..range.end).to_string())
    }
}

struct AppliedEdits {
    forward: Vec<TextEdit>,
    inverse: Vec<TextEdit>,
    version_before: u64,
    version_after: u64,
}
