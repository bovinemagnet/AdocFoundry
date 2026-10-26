use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc,
    },
};

use adoc_core::{FileAccess, SourceRange};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;
use adoc_parser::parse;
use ropey::Rope;

use crate::{
    commands::{self, ids},
    document::{hash_text, uri_for, DocumentState},
    edit,
    undo::Typing,
    workspace::{FileWatcher, WorkerMessage},
    Clock, CommandArgs, CommandContext, CommandId, CommandOutcome, CommandRegistry, DocumentId,
    EditorMode, EngineError, EngineEvent, OpenDocument, Settings, SystemClock, TextEdit, UndoEntry,
    UndoGroup, UndoStack,
};

/// The headless editor engine: the single owner of workspace and document state.
///
/// The engine is single-threaded: a front end owns it and mutates it through `&mut self`. Long
/// work runs on worker threads whose results are applied by [`Engine::pump`].
pub struct Engine {
    pub(crate) settings: Settings,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) registry: CommandRegistry,
    pub(crate) documents: BTreeMap<DocumentId, OpenDocument>,
    pub(crate) states: BTreeMap<DocumentId, DocumentState>,
    pub(crate) next_document: u64,
    pub(crate) next_group: u64,
    pub(crate) next_untitled: u64,
    pub(crate) active: Option<DocumentId>,
    pub(crate) subscribers: Vec<Sender<EngineEvent>>,
    pub(crate) roots: Vec<PathBuf>,
    /// Which files the engine reads on the front end's behalf; see [`Engine::set_file_access`].
    pub(crate) file_access: FileAccess,
    pub(crate) workspace: WorkspaceState,
    pub(crate) worker_sender: Sender<WorkerMessage>,
    pub(crate) worker_receiver: Receiver<WorkerMessage>,
    pub(crate) workers_running: usize,
    /// Whole-workspace index jobs in flight, a subset of `workers_running`.
    pub(crate) indexing: usize,
    pub(crate) watcher: Option<FileWatcher>,
    pub(crate) history: crate::navigation::History,
    pub(crate) previews: BTreeMap<crate::PreviewId, crate::EditPreview>,
    pub(crate) next_preview: u64,
    pub(crate) next_search: u64,
    pub(crate) group_moves: BTreeMap<UndoGroup, Vec<crate::FileMove>>,
    /// Documents whose typed edits are parsed but not yet in the index, with the time after
    /// which `pump` indexes them.
    pub(crate) pending_index: BTreeMap<DocumentId, std::time::Instant>,
    pub(crate) rendered: crate::rendered::PreviewState,
    pub(crate) visual: crate::visual::VisualState,
    pub(crate) git: crate::git::GitState,
    pub(crate) tabs: crate::tabs::Tabs,
    pub(crate) expansion: crate::expansion::Expansion,
    pub(crate) files: crate::files::Files,
    pub(crate) images: crate::images::Images,
    #[cfg(feature = "typst")]
    pub(crate) export: crate::export::ExportState,
}

/// Typed characters re-parse their document at once but re-index it, and refresh diagnostics,
/// only once typing pauses this long. Every other command indexes first.
pub const INDEX_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

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
        let (worker_sender, worker_receiver) = mpsc::channel();
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
            roots: Vec::new(),
            file_access: FileAccess::Any,
            workspace: WorkspaceState::new(),
            worker_sender,
            worker_receiver,
            workers_running: 0,
            indexing: 0,
            watcher: None,
            history: crate::navigation::History::default(),
            previews: BTreeMap::new(),
            next_preview: 1,
            next_search: 1,
            group_moves: BTreeMap::new(),
            pending_index: BTreeMap::new(),
            rendered: crate::rendered::PreviewState::default(),
            visual: crate::visual::VisualState::default(),
            git: crate::git::GitState::default(),
            tabs: crate::tabs::Tabs::default(),
            expansion: crate::expansion::Expansion::default(),
            files: crate::files::Files::default(),
            images: crate::images::Images::default(),
            #[cfg(feature = "typst")]
            export: crate::export::ExportState::default(),
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
        let text = self.read_text(&path)?;
        let id = self.insert_document(path, &text, false);
        self.set_active(id);
        Ok(id)
    }

    /// The text of the file at `path`, refusing a file that is not text: one holding NUL bytes,
    /// or not UTF-8, which is never decoded some other way.
    pub(crate) fn read_text(&self, path: &Path) -> Result<String, EngineError> {
        let not_text = || {
            EngineError::Refused(format!(
                "{} is not a text file AdocFoundry can edit (it is binary, or not UTF-8)",
                path.display()
            ))
        };
        let text = self
            .file_access
            .read_to_string(path)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::InvalidData => not_text(),
                _ => EngineError::io(path, error),
            })?;
        if text.contains('\0') {
            return Err(not_text());
        }
        Ok(text)
    }

    /// Opens the page at `path` with `text` as its buffer, as a language client opens its own
    /// buffer; the file need not exist. A document already open takes `text` as one edit.
    pub fn open_text(&mut self, path: &Path, text: &str) -> DocumentId {
        let path = normalize_path(path);
        if let Some(id) = self.document_for_path(&path) {
            let document = &self.documents[&id];
            if document.text != text {
                let len = document.text.len();
                let _ = self.invoke(
                    commands::ids::EDIT_REPLACE_RANGE,
                    CommandArgs::Text {
                        id,
                        range: SourceRange::new(0, len),
                        text: text.to_owned(),
                    },
                );
            }
            return id;
        }
        let id = self.insert_document(path.clone(), text, false);
        // Unsaved when it differs from the file, or there is none.
        let saved = fs::read_to_string(&path).unwrap_or_default();
        self.mark_saved(id, &saved);
        id
    }

    /// Opens a new, empty document that has no file on disk.
    pub fn open_untitled(&mut self) -> DocumentId {
        let path = PathBuf::from(format!("Untitled-{}.adoc", self.next_untitled));
        self.next_untitled += 1;
        let id = self.insert_document(path, "", true);
        self.set_active(id);
        id
    }

    pub(crate) fn insert_document(
        &mut self,
        path: PathBuf,
        text: &str,
        untitled: bool,
    ) -> DocumentId {
        let id = DocumentId(self.next_document);
        self.next_document += 1;
        let asciidoc = untitled || adoc_index::is_asciidoc_path(&path);
        let parsed = parse(&uri_for(&path), text).document;
        let diagnostics = if untitled {
            parsed.diagnostics.clone()
        } else if !asciidoc {
            Vec::new()
        } else {
            self.workspace
                .replace_document(path.clone(), parsed.clone());
            adoc_lang::diagnostics(&self.workspace, &path)
        };
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
                asciidoc,
            },
        );
        self.tabs.order.push(crate::TabItem::Document(id));
        self.states.insert(
            id,
            DocumentState {
                undo: UndoStack::default(),
                saved_hash: hash_text(text),
            },
        );
        self.emit(EngineEvent::DocumentOpened(id));
        if asciidoc && !untitled {
            self.refresh_open_diagnostics(None);
        }
        if !untitled {
            self.schedule_hunks(id, false);
        }
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

    /// Closes document `id`, discarding unsaved edits. The index reverts to the file on disk.
    pub fn close(&mut self, id: DocumentId) -> Result<(), EngineError> {
        let document = self
            .documents
            .remove(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        self.states.remove(&id);
        let item = crate::TabItem::Document(id);
        self.tabs.order.retain(|open| *open != item);
        if self.tabs.preview == Some(item) {
            self.tabs.preview = None;
        }
        self.pending_index.remove(&id);
        self.forget_preview(id);
        self.forget_visual(id);
        self.forget_hunks(id);
        if self.rendered.layout.pinned == Some(id) {
            self.rendered.layout.pinned = None;
            self.emit(EngineEvent::PreviewLayoutChanged);
        }
        if document.indexed() {
            match fs::read_to_string(&document.path) {
                Ok(text) => {
                    self.workspace.index.index_source(&document.path, &text);
                }
                Err(_) => self.workspace.remove_document(&document.path),
            }
            self.refresh_open_diagnostics(None);
        }
        if self.active == Some(id) {
            self.active = None;
            self.activate_last_tab();
        }
        self.sync_preview_target();
        self.emit(EngineEvent::DocumentClosed(id));
        Ok(())
    }

    /// The tabs, in order, that `command` (`document.close` or one of Close Others, Left,
    /// Right, Clean and All) would close if invoked on the tab of `item`, a document or an
    /// image: none if it is not open. Front ends ask this first, to confirm discarding the
    /// unsaved ones.
    #[must_use]
    pub fn closed_by(
        &self,
        command: CommandId,
        item: impl Into<crate::TabItem>,
    ) -> Vec<crate::TabItem> {
        let item = item.into();
        let open = self.tabs.order.iter().copied();
        let Some(at) = self.tabs.order.iter().position(|open| *open == item) else {
            return Vec::new();
        };
        match command {
            ids::DOCUMENT_CLOSE => vec![item],
            ids::DOCUMENT_CLOSE_OTHERS => open.filter(|other| *other != item).collect(),
            ids::DOCUMENT_CLOSE_LEFT => open.take(at).collect(),
            ids::DOCUMENT_CLOSE_RIGHT => open.skip(at + 1).collect(),
            _ => self.closed_by_any(command),
        }
    }

    /// The tabs, in order, Close Clean or Close All would close, whichever tab they are invoked
    /// on. An image is always clean.
    #[must_use]
    pub fn closed_by_any(&self, command: CommandId) -> Vec<crate::TabItem> {
        let open = self.tabs.order.iter().copied();
        let clean = |item: &crate::TabItem| match item {
            crate::TabItem::Document(id) => self.documents.get(id).is_some_and(|d| !d.dirty),
            crate::TabItem::Image(_) => true,
        };
        match command {
            ids::DOCUMENT_CLOSE_CLEAN => open.filter(clean).collect(),
            ids::DOCUMENT_CLOSE_ALL => open.collect(),
            _ => Vec::new(),
        }
    }

    /// Writes document `id` to its file, unless the file changed on disk since the engine last
    /// read or wrote it: then nothing is written and the error is [`EngineError::SaveConflict`].
    pub fn save(&mut self, id: DocumentId) -> Result<(), EngineError> {
        self.write_document(id, false)
    }

    /// Writes document `id` to its file even when the file changed on disk, losing that change.
    pub fn overwrite(&mut self, id: DocumentId) -> Result<(), EngineError> {
        self.write_document(id, true)
    }

    /// Replaces document `id`'s text with its file's, as one undo step, leaving it clean.
    pub fn reload(&mut self, id: DocumentId) -> Result<(), EngineError> {
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        let path = document.path.clone();
        let text = self.read_text(&path)?;
        let Some(document) = self.documents.get(&id) else {
            return Err(EngineError::UnknownDocument(id));
        };
        if document.text != text.as_str() {
            let whole = SourceRange::new(0, document.text.len());
            let group = self.new_group();
            self.apply_edits(
                id,
                vec![TextEdit::new(whole, text.clone())],
                UndoMode::Group(group),
            )?;
        }
        self.mark_saved(id, &text);
        Ok(())
    }

    fn write_document(&mut self, id: DocumentId, overwrite: bool) -> Result<(), EngineError> {
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        if document.untitled {
            return Err(EngineError::NotFound(format!(
                "{} has never been saved and has no file",
                document.path.display()
            )));
        }
        let path = document.path.clone();
        if !overwrite {
            let saved = self.states.get(&id).map(|state| state.saved_hash);
            match fs::read_to_string(&path) {
                Ok(on_disk) if Some(hash_text(&on_disk)) == saved => {}
                // Deleted outside: writing it again loses nothing.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                // Changed, or no longer text.
                _ => return Err(EngineError::SaveConflict { path }),
            }
        }
        let text = document.text.to_string();
        write_replacing(&path, &text).map_err(|error| EngineError::io(&path, error))?;
        let Some(document) = self.documents.get_mut(&id) else {
            return Err(EngineError::UnknownDocument(id));
        };
        document.dirty = false;
        if let Some(state) = self.states.get_mut(&id) {
            state.saved_hash = hash_text(&text);
        }
        self.emit(EngineEvent::DocumentSaved(id));
        self.keep_open(id);
        self.schedule_hunks(id, false);
        self.forget_blame(id);
        Ok(())
    }

    /// Records `text` as the content on disk, so the document is clean when it matches.
    pub(crate) fn mark_saved(&mut self, id: DocumentId, text: &str) {
        if let Some(state) = self.states.get_mut(&id) {
            state.saved_hash = hash_text(text);
        }
        if let Some(document) = self.documents.get_mut(&id) {
            document.dirty = document.text != text;
        }
    }

    #[must_use]
    pub fn active(&self) -> Option<DocumentId> {
        self.active
    }

    /// Makes document `id` the active tab, in place of any active image.
    pub fn set_active(&mut self, id: DocumentId) {
        if self.documents.contains_key(&id)
            && (self.active != Some(id) || self.images.active.is_some())
        {
            self.images.active = None;
            self.active = Some(id);
            self.emit(EngineEvent::NavigationChanged);
            self.sync_preview_target();
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
        if command != commands::ids::EDIT_INSERT_TEXT {
            self.flush_index();
        }
        let result = handler.run(self, args);
        // Source typing rebuilds visual models once it pauses; anything else, now.
        let typing = [
            commands::ids::EDIT_INSERT_TEXT,
            commands::ids::EDIT_REPLACE_RANGE,
        ]
        .contains(&command);
        if !typing {
            self.refresh_stale_visuals();
        }
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

    /// Whether command `id` applies now, by its [`CommandContext`]: a document command needs an
    /// active document (none is while an image is the active tab), a tab command any active tab, a visual editor command one shown visually or split, and an Antora command
    /// an Antora component in the workspace. Front ends use it to offer only what applies.
    #[must_use]
    pub fn command_available(&self, id: CommandId) -> bool {
        let Some(command) = self.registry.get(id) else {
            return false;
        };
        match command.spec().when {
            CommandContext::Always => true,
            CommandContext::ActiveDocument => self.active().is_some(),
            CommandContext::ActiveTab => self.active_item().is_some(),
            CommandContext::ActiveImage => self.active_image().is_some(),
            CommandContext::Selection => self
                .active()
                .is_some_and(|document| self.editor_mode(document) != EditorMode::Source),
            CommandContext::Antora => self.workspace.antora.components().next().is_some(),
        }
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
            if self
                .edit_document(document, entry.inverse.clone(), false)
                .is_ok()
            {
                if let Some(state) = self.states.get_mut(&document) {
                    state.undo.push_redo(entry);
                }
            }
        }
        self.replay_moves(group, true);
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
            if self
                .edit_document(document, entry.forward.clone(), false)
                .is_ok()
            {
                if let Some(state) = self.states.get_mut(&document) {
                    state.undo.push_undo_keep_redo(entry);
                }
            }
        }
        self.replay_moves(group, false);
        true
    }

    /// Undoes `group` in every document where it is the latest entry, keeping nothing of it in
    /// the redo history: for a change that failed part-way and must leave no trace.
    pub(crate) fn discard_group(&mut self, group: UndoGroup) {
        for document in self.documents_with_top_group(group, false) {
            let Some(entry) = self
                .states
                .get_mut(&document)
                .and_then(|state| state.undo.pop_undo())
            else {
                continue;
            };
            let _ = self.edit_document(document, entry.inverse, false);
        }
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
        let applied = self.edit_document(id, edits, typed.is_some())?;
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
    /// re-parses and publishes the change. With `defer_index`, re-indexing and diagnostics wait
    /// for [`INDEX_SETTLE`].
    fn edit_document(
        &mut self,
        id: DocumentId,
        edits: Vec<TextEdit>,
        defer_index: bool,
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
        let saved_hash = self.states.get(&id).map_or(0, |state| state.saved_hash);
        document.dirty = hash_text(&text) != saved_hash;
        let version_after = document.version;
        let path = document.path.clone();
        let untitled = document.untitled;
        let indexed = document.indexed();
        if untitled {
            document.diagnostics = document.parsed.diagnostics.clone();
        } else if !indexed {
            // Text that is not AsciiDoc: nothing to index or diagnose.
        } else if defer_index {
            let deadline = self.clock.now() + INDEX_SETTLE;
            self.pending_index.insert(id, deadline);
        } else {
            self.pending_index.remove(&id);
            let parsed = document.parsed.clone();
            self.workspace.replace_document(path.clone(), parsed);
        }

        self.emit(EngineEvent::DocumentChanged {
            id,
            version: version_after,
            edits: forward.clone(),
        });
        // An edited page is kept open, even once undone.
        self.keep_open(id);
        self.preview_source_changed(&path);
        self.visual_source_changed(&path);
        if !untitled {
            self.schedule_hunks(id, defer_index);
        }
        if untitled {
            self.emit(EngineEvent::DiagnosticsUpdated { path });
        } else if indexed && !defer_index {
            self.refresh_open_diagnostics(Some(&path));
        }
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

    pub(crate) fn emit(&mut self, event: EngineEvent) {
        self.subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }

    // --- settings ----------------------------------------------------------------------------

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Indexes every document whose typed edits are waiting for a pause, and refreshes
    /// diagnostics. Returns the number indexed.
    pub(crate) fn flush_index(&mut self) -> usize {
        self.index_pending(None)
    }

    /// Indexes pending documents whose deadline is at or before `due`, or all when `None`.
    pub(crate) fn index_pending(&mut self, due: Option<std::time::Instant>) -> usize {
        let ready: Vec<DocumentId> = self
            .pending_index
            .iter()
            .filter(|(_, deadline)| due.is_none_or(|due| **deadline <= due))
            .map(|(id, _)| *id)
            .collect();
        for id in &ready {
            self.pending_index.remove(id);
            let Some(document) = self.documents.get(id) else {
                continue;
            };
            let (path, parsed) = (document.path.clone(), document.parsed.clone());
            self.workspace.replace_document(path.clone(), parsed);
            self.refresh_open_diagnostics(Some(&path));
        }
        ready.len()
    }

    /// The workspace index, Antora catalog and descriptor problems.
    #[must_use]
    pub fn workspace(&self) -> &WorkspaceState {
        &self.workspace
    }

    /// Recomputes the diagnostics of every open document, publishing `DiagnosticsUpdated` for
    /// each whose diagnostics changed and always for `changed`.
    pub(crate) fn refresh_open_diagnostics(&mut self, changed: Option<&Path>) {
        let mut updated = Vec::new();
        for document in self
            .documents
            .values_mut()
            .filter(|document| document.indexed())
        {
            let fresh = adoc_lang::diagnostics(&self.workspace, &document.path);
            if fresh != document.diagnostics || changed == Some(document.path.as_path()) {
                document.diagnostics = fresh;
                updated.push(document.path.clone());
            }
        }
        for path in updated {
            self.emit(EngineEvent::DiagnosticsUpdated { path });
        }
    }

    pub fn reload_settings(&mut self) -> Result<(), EngineError> {
        let git_was_enabled = self.settings.git.enabled;
        self.settings.reload()?;
        self.emit(EngineEvent::SettingsChanged);
        self.settle_preview_tab();
        if self.settings.git.enabled != git_was_enabled {
            self.discover_git();
        }
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

/// Writes `text` to `path` through a temporary file in the same directory, renamed over the
/// original, so a failed write never leaves a truncated file. The original's permissions are
/// kept, and a symbolic link is kept as a link: its target is written.
///
/// The temporary file has an unpredictable name and is created exclusively, never through an
/// existing path, so a link planted in a shared directory cannot redirect the write.
pub(crate) fn write_replacing(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let target = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let directory = target.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".adocfoundry-save-")
        .tempfile_in(directory)?;
    temporary.write_all(text.as_bytes())?;
    if let Ok(metadata) = fs::metadata(&target) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())?;
    }
    temporary.as_file().sync_all()?;
    // Dropping a temporary file that was not persisted removes it.
    temporary.persist(&target).map_err(|error| error.error)?;
    Ok(())
}
