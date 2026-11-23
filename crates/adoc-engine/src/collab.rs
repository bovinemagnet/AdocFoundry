//! Collaborative editing's engine side (COLLAB 3/8, #690; `docs/design/DESIGN-AdocFoundry_Collaboration.md`
//! §§1, 9, 10). A shared document keeps an `adoc-collab` replica beside its rope; the two are
//! two verified views of one text, never independent owners.
//!
//! Every local change to a shared document goes through `Engine::apply_edits`, which hands it
//! here: checked against the rope, applied to the replica, then to the rope, its update kept for
//! the transport. A participant's update comes in through [`Engine::apply_shared_update`] and
//! reaches the rope as one patch through `edit_document`, never echoed and never in local undo.
//! Undo and redo are the replica's, grouped where the engine's coalescing rules end a group.
//! There is no network code here: the transport (COLLAB 4/8) moves the bytes this hands out and
//! brings in.

use adoc_collab::{
    AppliedChange, ByteEdit, CollabError, EncodedUpdate, Replica, ReplicaId, Sender,
    StateVectorBytes, MAX_SHARED_FILE,
};

use crate::{
    edit,
    engine::{is_small, UndoMode},
    undo::{Typing, COALESCE_WINDOW},
    DocumentId, EditOrigin, Engine, EngineError, EngineEvent, TextEdit, UndoGroup, UndoStack,
};

/// A shared file's id in its session: opaque to guests, assigned by the host (design §7).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SharedFileId(pub u64);

/// This participant's part in a shared document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedRole {
    /// The host: owns the file, applies guests' updates and forwards them.
    Host,
    /// A guest, who may edit only while the host allows it.
    Guest { can_edit: bool },
}

/// What applying a participant's update did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedReceipt {
    /// Applied; anything it changed is in the rope.
    Applied,
    /// A guest's replica waits for something the host sent out of order, or is broken: the
    /// guest asks the host to resynchronise it (`Resync`, or a new snapshot).
    NeedsResync,
    /// The host found a guest's update left its replica waiting or inconsistent, which no honest
    /// guest causes: it rebuilt the file's replica from its own text in the new `epoch`. The
    /// caller drops that guest and sends everyone else a new snapshot (design §2).
    Rebuilt { epoch: u32 },
}

/// Whether a shared document's text is the text on disk (COLLAB 5/8).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SharedSaveState {
    /// The file holds this text: `revision` is its SHA-256 ([`revision_of`]).
    Saved { revision: String },
    /// Changes made since the last save are in the shared text only.
    Unsaved,
    /// The file was changed or removed outside the session, under the shared text: nothing is
    /// saved until the host keeps the shared text or takes the file's
    /// ([`Engine::resolve_disk_conflict`]).
    Conflict,
    /// A guest out of touch with the host: whether anything is saved is not known.
    Unknown,
}

/// How the host settles a change made on disk to a shared file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiskResolution {
    /// The shared text stays; the next save writes it over what is on disk.
    KeepShared,
    /// The file's text replaces the shared text in a new epoch: every participant starts again
    /// from it.
    ReloadDisk,
}

/// A saved text's revision: its SHA-256, in hexadecimal. A guest computes the same from its own
/// text to tell whether it is the saved one.
#[must_use]
pub fn revision_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A document shared in a session.
pub(crate) struct SharedDocument {
    replica: Replica,
    role: SharedRole,
    file: SharedFileId,
    epoch: u32,
    /// Updates of the local author's changes, waiting for the transport.
    outbox: Vec<EncodedUpdate>,
    /// The typing run the next keystroke may extend, as `UndoStack::coalesce_typing` decides.
    typing: Option<Typing>,
    /// The group of the last local change, so one command's several edits are one undo step.
    group: Option<UndoGroup>,
    /// The revisions of saves being written, by the document version each snapshot was.
    saving: std::collections::BTreeMap<u64, String>,
    /// The revision the file holds, as last written or read here.
    saved: Option<String>,
    /// The file changed or went outside the session: no save until the host resolves it.
    conflict: bool,
}

impl SharedDocument {
    /// Whether local changes wait to be sent.
    pub(crate) fn has_outbox(&self) -> bool {
        !self.outbox.is_empty()
    }

    pub(crate) fn is_guest(&self) -> bool {
        matches!(self.role, SharedRole::Guest { .. })
    }

    pub(crate) fn saved_revision(&self) -> Option<&str> {
        self.saved.as_deref()
    }

    fn new(replica: Replica, role: SharedRole, file: SharedFileId, epoch: u32) -> Self {
        Self {
            replica,
            role,
            file,
            epoch,
            outbox: Vec::new(),
            typing: None,
            group: None,
            saving: std::collections::BTreeMap::new(),
            saved: None,
            conflict: false,
        }
    }
}

fn refused(message: &str) -> EngineError {
    EngineError::Refused(message.to_owned())
}

fn collab_error(error: &CollabError) -> EngineError {
    match error {
        CollabError::InvalidBoundary(_) | CollabError::Unordered => {
            EngineError::InvalidEdit(error.to_string())
        }
        _ => EngineError::Refused(format!("shared document: {error}")),
    }
}

fn byte_edits(edits: &[TextEdit]) -> Vec<ByteEdit> {
    edits
        .iter()
        .map(|edit| ByteEdit::new(edit.range.start, edit.range.end, &edit.replacement))
        .collect()
}

fn text_edits(edits: &[ByteEdit]) -> Vec<TextEdit> {
    edits
        .iter()
        .map(|edit| TextEdit::new(edit.range, edit.text.clone()))
        .collect()
}

impl Engine {
    /// Shares open document `id` as `file`, this engine its host under `host` (design §5, §7).
    /// Its undo history is dropped: its byte ranges cannot be replayed across merged edits.
    pub fn share_document(
        &mut self,
        id: DocumentId,
        file: SharedFileId,
        host: ReplicaId,
    ) -> Result<(), EngineError> {
        self.share_document_at(id, file, host, 1)
    }

    /// [`Engine::share_document`] starting at `epoch`: a file shared again in a session goes on
    /// from the epoch after its last, so no update of the earlier sharing applies to it.
    pub(crate) fn share_document_at(
        &mut self,
        id: DocumentId,
        file: SharedFileId,
        host: ReplicaId,
        epoch: u32,
    ) -> Result<(), EngineError> {
        if self.shared.contains_key(&id) {
            return Err(refused("the document is already shared"));
        }
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        if document.untitled {
            return Err(refused("a document with no file is not shared"));
        }
        if document.text.len() > MAX_SHARED_FILE {
            return Err(refused("the document is too large to share"));
        }
        let replica =
            Replica::new(&document.text.to_string(), host).map_err(|error| collab_error(&error))?;
        let saved = (!document.dirty).then(|| revision_of(&document.text.to_string()));
        self.drop_undo_history(id);
        let mut shared = SharedDocument::new(replica, SharedRole::Host, file, epoch);
        shared.saved = saved;
        self.shared.insert(id, shared);
        Ok(())
    }

    /// Opens a shared file as a guest, from the host's `snapshot` of it in `epoch`, as a document
    /// with no file of its own named `name` (the host's root-relative path). It is never
    /// bootstrapped from anything but the snapshot.
    pub fn open_shared(
        &mut self,
        name: &str,
        file: SharedFileId,
        epoch: u32,
        snapshot: &EncodedUpdate,
        me: ReplicaId,
        can_edit: bool,
    ) -> Result<DocumentId, EngineError> {
        let replica = Replica::from_snapshot(snapshot, me).map_err(|error| collab_error(&error))?;
        let path = std::path::PathBuf::from(name);
        let id = self.insert_document(path.clone(), &replica.text(), true);
        // A shared page another includes: its preview reads it now.
        self.preview_source_changed(&path);
        self.shared.insert(
            id,
            SharedDocument::new(replica, SharedRole::Guest { can_edit }, file, epoch),
        );
        self.emit(EngineEvent::DocumentOpened(id));
        Ok(id)
    }

    /// A shared document's file, epoch and this participant's role in it.
    #[must_use]
    pub fn shared_file(&self, id: DocumentId) -> Option<(SharedFileId, u32, SharedRole)> {
        self.shared
            .get(&id)
            .map(|shared| (shared.file, shared.epoch, shared.role))
    }

    /// Lets a guest edit a shared document, or stops it, as the host's role change says. The host
    /// checks every update against the role it gave, whatever a guest does here.
    pub fn set_shared_editing(&mut self, id: DocumentId, can_edit: bool) {
        if let Some(shared) = self.shared.get_mut(&id) {
            if let SharedRole::Guest { .. } = shared.role {
                shared.role = SharedRole::Guest { can_edit };
            }
        }
    }

    /// The whole state of a shared document, for a guest joining or rebuilding, and its epoch.
    ///
    /// A snapshot checks itself (`adoc_collab::Replica::snapshot`): when the replica holds a
    /// state whose encoding would rebuild another text, or waits for something, the host
    /// rebuilds it from its own text first, in a new epoch, and snapshots that. A changed epoch
    /// tells the caller every guest needs the new snapshot.
    pub fn shared_snapshot(&mut self, id: DocumentId) -> Result<(EncodedUpdate, u32), EngineError> {
        let shared = self.shared.get(&id).ok_or_else(|| refused("not shared"))?;
        if let Ok(snapshot) = shared.replica.snapshot() {
            return Ok((snapshot, shared.epoch));
        }
        if shared.role != SharedRole::Host {
            return Err(refused("only the host gives a snapshot"));
        }
        if !matches!(self.broken(id), SharedReceipt::Rebuilt { .. }) {
            return Err(refused("the shared document could not be rebuilt"));
        }
        let shared = self.shared.get(&id).ok_or_else(|| refused("not shared"))?;
        let snapshot = shared
            .replica
            .snapshot()
            .map_err(|error| collab_error(&error))?;
        Ok((snapshot, shared.epoch))
    }

    /// A shared document's state vector, for a resynchronisation.
    pub fn shared_state_vector(&self, id: DocumentId) -> Result<StateVectorBytes, EngineError> {
        let shared = self.shared.get(&id).ok_or_else(|| refused("not shared"))?;
        Ok(shared.replica.state_vector())
    }

    /// What a participant whose state vector is `remote` lacks of a shared document.
    pub fn shared_missing_since(
        &self,
        id: DocumentId,
        remote: &StateVectorBytes,
    ) -> Result<EncodedUpdate, EngineError> {
        let shared = self.shared.get(&id).ok_or_else(|| refused("not shared"))?;
        shared
            .replica
            .missing_since(remote)
            .map_err(|error| collab_error(&error))
    }

    /// A shared document's text as its replica holds it, copied: for checks that it and the rope
    /// agree, not for every keystroke.
    #[must_use]
    pub fn shared_text(&self, id: DocumentId) -> Option<String> {
        self.shared.get(&id).map(|shared| shared.replica.text())
    }

    /// The updates of the local author's changes to a shared document since last taken, in the
    /// order made, for the transport to send.
    pub fn take_shared_updates(&mut self, id: DocumentId) -> Vec<EncodedUpdate> {
        self.shared
            .get_mut(&id)
            .map(|shared| std::mem::take(&mut shared.outbox))
            .unwrap_or_default()
    }

    /// Stops sharing document `id`: its replica and the history it held go, and its undo
    /// history starts empty, the session's byte ranges being no longer trusted.
    pub fn stop_sharing(&mut self, id: DocumentId) {
        if self.shared.remove(&id).is_some() {
            self.drop_undo_history(id);
        }
    }

    fn drop_undo_history(&mut self, id: DocumentId) {
        if let Some(state) = self.states.get_mut(&id) {
            state.undo = UndoStack::default();
        }
    }

    /// Applies a participant's update to shared document `id` in `epoch`, from `from`: a guest
    /// takes only the host's, the host only guests'. Refused updates change nothing (no rope,
    /// replica, undo or version); see [`SharedReceipt`] for what else may happen.
    pub fn apply_shared_update(
        &mut self,
        id: DocumentId,
        epoch: u32,
        update: &EncodedUpdate,
        from: Sender,
    ) -> Result<SharedReceipt, EngineError> {
        let shared = self
            .shared
            .get_mut(&id)
            .ok_or_else(|| refused("not shared"))?;
        if shared.epoch != epoch {
            return Err(refused("an update from an earlier epoch"));
        }
        let host = shared.role == SharedRole::Host;
        if host == matches!(from, Sender::Host) {
            return Err(refused("an update from the wrong side"));
        }
        let change = match shared.replica.apply_remote(update, from) {
            Ok(change) => change,
            Err(_) if shared.replica.is_poisoned() => return Ok(self.broken(id)),
            Err(error) => return Err(collab_error(&error)),
        };
        if !change.edits.is_empty() {
            let edits = text_edits(&change.edits);
            let defer = is_small(&edits);
            if self
                .edit_document(id, edits, defer, EditOrigin::Remote)
                .is_err()
            {
                return Ok(self.broken(id));
            }
            if let Some(shared) = self.shared.get_mut(&id) {
                // A remote change moves text under the local typing run: the next keystroke
                // starts a new undo group.
                shared.typing = None;
            }
        }
        if change.pending {
            return Ok(self.broken(id));
        }
        Ok(SharedReceipt::Applied)
    }

    /// The replica and the rope can no longer be trusted together. The host rebuilds the replica
    /// from its rope in a new epoch; a guest asks for a resynchronisation.
    fn broken(&mut self, id: DocumentId) -> SharedReceipt {
        let Some(shared) = self.shared.get(&id) else {
            return SharedReceipt::NeedsResync;
        };
        if shared.role != SharedRole::Host {
            return SharedReceipt::NeedsResync;
        }
        let text = self
            .documents
            .get(&id)
            .map(|document| document.text.to_string())
            .unwrap_or_default();
        let Ok(replica) = Replica::new(&text, ReplicaId::random()) else {
            self.stop_sharing(id);
            return SharedReceipt::NeedsResync;
        };
        let Some(shared) = self.shared.get_mut(&id) else {
            return SharedReceipt::NeedsResync;
        };
        shared.replica = replica;
        shared.epoch += 1;
        shared.outbox.clear();
        shared.typing = None;
        shared.group = None;
        let epoch = shared.epoch;
        SharedReceipt::Rebuilt { epoch }
    }

    /// A guest's shared document started again from the host's `snapshot` in `epoch`: the rope
    /// takes the snapshot's text as one remote patch, and the local history goes.
    pub fn reset_shared(
        &mut self,
        id: DocumentId,
        epoch: u32,
        snapshot: &EncodedUpdate,
        me: ReplicaId,
    ) -> Result<(), EngineError> {
        let role = self
            .shared
            .get(&id)
            .map(|shared| shared.role)
            .ok_or_else(|| refused("not shared"))?;
        if role == SharedRole::Host {
            return Err(refused("the host's replica is rebuilt, not reset"));
        }
        let replica = Replica::from_snapshot(snapshot, me).map_err(|error| collab_error(&error))?;
        let text = replica.text();
        let current = self
            .documents
            .get(&id)
            .map(|document| document.text.to_string())
            .unwrap_or_default();
        if let Some(edit) = edit::minimal(&current, &text) {
            self.edit_document(id, vec![edit], false, EditOrigin::Remote)?;
        }
        if let Some(shared) = self.shared.get_mut(&id) {
            shared.replica = replica;
            shared.epoch = epoch;
            shared.outbox.clear();
            shared.typing = None;
            shared.group = None;
        }
        self.drop_undo_history(id);
        Ok(())
    }

    /// The edit gateway for a shared document (from `Engine::apply_edits`): the edits are
    /// checked against the rope, applied to the replica, then to the rope, and the update kept
    /// for the transport. A refusal anywhere before the rope changes nothing.
    pub(crate) fn apply_shared_edits(
        &mut self,
        id: DocumentId,
        edits: Vec<TextEdit>,
        mode: UndoMode,
        typed: Option<(usize, String)>,
        defer: bool,
    ) -> Result<(), EngineError> {
        let shared = self.shared.get(&id).ok_or_else(|| refused("not shared"))?;
        if shared.role == (SharedRole::Guest { can_edit: false }) {
            return Err(refused("this shared document is read-only for you"));
        }
        if let UndoMode::Group(group) = mode {
            // The same command already changed another document: one file at a time while
            // shared, since peers could take only part of the change.
            let elsewhere = self
                .states
                .iter()
                .any(|(other, state)| *other != id && state.undo.top_group() == Some(group));
            let other_shared = self.shared_group == Some(group)
                && self
                    .shared
                    .get(&id)
                    .is_some_and(|shared| shared.group != Some(group));
            if elsewhere || other_shared {
                return Err(refused(
                    "a change to several files at once is not made while one of them is shared",
                ));
            }
            self.shared_group = Some(group);
        }
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        let forward = edit::validate(&document.text, edits)?;
        let now = self.clock.now();
        let Some(shared) = self.shared.get_mut(&id) else {
            return Err(refused("not shared"));
        };
        // Keystrokes coalesce as `UndoStack::coalesce_typing` would have them: a run of typing
        // at its own end within the window; one command's edits are one group.
        let continues = typed.as_ref().is_some_and(|(offset, _)| {
            shared.typing.as_ref().is_some_and(|typing| {
                typing.end == *offset
                    && now.saturating_duration_since(typing.last) <= COALESCE_WINDOW
            })
        });
        let same_group = matches!(mode, UndoMode::Group(group) if shared.group == Some(group));
        if !continues && !same_group {
            shared.replica.end_group();
        }
        let change: AppliedChange = shared
            .replica
            .apply_local(&byte_edits(&forward))
            .map_err(|error| collab_error(&error))?;
        shared.typing = typed.map(|(offset, text)| Typing {
            last: now,
            end: offset + text.len(),
        });
        shared.group = match mode {
            UndoMode::Group(group) => Some(group),
            UndoMode::Typing => None,
        };
        if let Some(update) = change.outbound {
            shared.outbox.push(update);
        }
        if self
            .edit_document(id, forward, defer, EditOrigin::Local)
            .is_err()
        {
            // The rope refused what it had accepted a moment before: never go on with the two
            // apart.
            self.broken(id);
            return Err(refused("the shared document had to be rebuilt"));
        }
        self.emit(EngineEvent::SharedUpdates(id));
        Ok(())
    }

    /// The host applies `edit` to shared document `id` for a participant (a guest's accepted
    /// suggestion, spec §8): checked against the rope, applied to the replica outside the
    /// host's undo history, sent to everyone like the host's own change, and taken by the rope
    /// as a participant's edit, so nobody's Undo reaches it. It ends what the host could redo, as
    /// any new edit of its own does: a Redo of the host's own accept of the same suggestion,
    /// undone and the suggestion reopened, would otherwise apply it a second time. A refusal
    /// before the rope changes nothing.
    pub(crate) fn apply_untracked_shared_edit(
        &mut self,
        id: DocumentId,
        edit: TextEdit,
    ) -> Result<(), EngineError> {
        let shared = self.shared.get(&id).ok_or_else(|| refused("not shared"))?;
        if shared.role != SharedRole::Host {
            return Err(refused("only the host applies a participant's change"));
        }
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        let forward = edit::validate(&document.text, vec![edit])?;
        let Some(shared) = self.shared.get_mut(&id) else {
            return Err(refused("not shared"));
        };
        let change = shared
            .replica
            .apply_untracked(&byte_edits(&forward))
            .map_err(|error| collab_error(&error))?;
        shared.replica.clear_redo();
        // As a participant's change does, it moves text under the host's typing run: the
        // host's next keystroke starts a new undo group.
        shared.typing = None;
        shared.group = None;
        if let Some(update) = change.outbound {
            shared.outbox.push(update);
        }
        if self
            .edit_document(id, forward, false, EditOrigin::Remote)
            .is_err()
        {
            // The rope refused what it had accepted a moment before: never go on with the two
            // apart.
            self.broken(id);
            return Err(refused("the shared document had to be rebuilt"));
        }
        self.emit(EngineEvent::SharedUpdates(id));
        Ok(())
    }

    /// A host's save of shared document `id` is starting with the text of document `version`:
    /// refused while a change on disk waits to be resolved, unless `overwrite`, the host's own
    /// choice to write over it.
    pub(crate) fn shared_save_starting(
        &mut self,
        id: DocumentId,
        version: u64,
        text: &str,
        overwrite: bool,
    ) -> Result<(), EngineError> {
        let Some(shared) = self.shared.get_mut(&id) else {
            return Ok(());
        };
        if shared.conflict && !overwrite {
            let path = self
                .documents
                .get(&id)
                .map(|document| document.path.clone())
                .unwrap_or_default();
            return Err(EngineError::SaveConflict { path });
        }
        shared.saving.insert(version, revision_of(text));
        Ok(())
    }

    /// A save of shared document `id`, of document `version`, finished: written (the file holds
    /// that snapshot's revision, whatever was typed since) or refused as changed on disk.
    pub(crate) fn shared_save_finished(
        &mut self,
        id: DocumentId,
        version: u64,
        written: bool,
        conflict: bool,
    ) {
        let Some(shared) = self.shared.get_mut(&id) else {
            return;
        };
        let revision = shared.saving.remove(&version);
        if written {
            if revision.is_some() {
                shared.saved = revision;
            }
            shared.conflict = false;
        } else if conflict {
            shared.conflict = true;
        }
    }

    /// The file of host-shared document `id` changed on disk to `on_disk` (`None`: removed),
    /// unlike what was last saved or read. It never flows in as an edit: the document enters a
    /// conflict the host resolves. Returns whether it is a host's shared document, which the
    /// caller then leaves alone.
    pub(crate) fn shared_changed_on_disk(&mut self, id: DocumentId, on_disk: Option<&str>) -> bool {
        let Some(shared) = self.shared.get_mut(&id) else {
            return false;
        };
        if shared.role != SharedRole::Host {
            return true;
        }
        let saved = self
            .states
            .get(&id)
            .zip(on_disk)
            .is_some_and(|(state, text)| {
                text.len() == state.saved_len
                    && crate::document::hash_text(text) == state.saved_hash
            });
        if saved {
            // Back to what was saved: nothing to resolve.
            if std::mem::take(&mut shared.conflict) {
                self.auto_save_settled(id);
                self.emit(EngineEvent::SessionChanged);
            }
            return true;
        }
        if !shared.conflict {
            shared.conflict = true;
            if let Some(state) = self.states.get_mut(&id) {
                state.changed_on_disk = true;
            }
            self.emit(EngineEvent::DocumentChangedOnDisk(id));
            self.emit(EngineEvent::SessionChanged);
        }
        true
    }

    /// This participant's selection from `anchor` to `head` in shared document `id`, relative
    /// to the text around it, to send as presence.
    pub(crate) fn shared_selection(
        &self,
        id: DocumentId,
        anchor: usize,
        head: usize,
    ) -> Option<adoc_collab::RelativeSelection> {
        let shared = self.shared.get(&id)?;
        let text = &self.documents.get(&id)?.text;
        let len = text.len();
        // Only offsets on character boundaries reach the replica.
        let on_boundary = |offset: usize| offset <= len && text.is_char_boundary(offset);
        if !on_boundary(anchor) || !on_boundary(head) {
            return None;
        }
        shared
            .replica
            .selection(anchor, head, adoc_collab::Affinity::After)
            .ok()
    }

    /// The range `range` of shared document `id` as a selection relative to the shared text, for
    /// a comment (#762): its start stays after text typed where it is, its end before, so text
    /// typed at either edge falls outside it, as a comment's byte range does here.
    pub(crate) fn shared_range(
        &self,
        id: DocumentId,
        range: adoc_core::SourceRange,
    ) -> Option<adoc_collab::RelativeSelection> {
        let shared = self.shared.get(&id)?;
        let text = &self.documents.get(&id)?.text;
        let len = text.len();
        let on_boundary = |offset: usize| offset <= len && text.is_char_boundary(offset);
        if !on_boundary(range.start) || !on_boundary(range.end) {
            return None;
        }
        let start = shared
            .replica
            .selection(range.start, range.start, adoc_collab::Affinity::After)
            .ok()?;
        let end = shared
            .replica
            .selection(range.end, range.end, adoc_collab::Affinity::Before)
            .ok()?;
        Some(adoc_collab::RelativeSelection {
            anchor: start.anchor,
            head: end.head,
        })
    }

    /// Where another participant's `selection` is now in shared document `id`, as byte offsets
    /// on character boundaries of its text.
    pub(crate) fn shared_resolve(
        &self,
        id: DocumentId,
        selection: &adoc_collab::RelativeSelection,
    ) -> Option<(usize, usize)> {
        let (anchor, head) = self.shared.get(&id)?.replica.resolve(selection)?;
        let text = &self.documents.get(&id)?.text;
        (text.is_char_boundary(anchor.min(text.len()))
            && text.is_char_boundary(head.min(text.len())))
        .then_some((anchor.min(text.len()), head.min(text.len())))
    }

    /// The revision host-shared document `id`'s file holds, as last written or read.
    pub(crate) fn shared_saved_revision(&self, id: DocumentId) -> Option<String> {
        self.shared.get(&id)?.saved_revision().map(str::to_owned)
    }

    /// Whether shared document `id`'s text is the text on disk, as this participant knows it.
    /// For a guest the session decides (`Engine::shared_save_state`).
    pub(crate) fn host_save_state(&self, id: DocumentId) -> Option<SharedSaveState> {
        let shared = self.shared.get(&id)?;
        if shared.role != SharedRole::Host {
            return None;
        }
        if shared.conflict {
            return Some(SharedSaveState::Conflict);
        }
        let dirty = self
            .documents
            .get(&id)
            .is_none_or(|document| document.dirty);
        Some(match (&shared.saved, dirty) {
            (Some(revision), false) => SharedSaveState::Saved {
                revision: revision.clone(),
            },
            _ => SharedSaveState::Unsaved,
        })
    }

    /// Settles a change made on disk to host-shared document `id` ([`DiskResolution`]).
    pub fn resolve_disk_conflict(
        &mut self,
        id: DocumentId,
        resolution: DiskResolution,
    ) -> Result<(), EngineError> {
        let shared = self.shared.get(&id).ok_or_else(|| refused("not shared"))?;
        if shared.role != SharedRole::Host {
            return Err(refused("only the host resolves its files"));
        }
        if !shared.conflict {
            return Err(refused("the file has not changed on disk"));
        }
        let path = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?
            .path
            .clone();
        let on_disk = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(EngineError::io(&path, error)),
        };
        match resolution {
            DiskResolution::KeepShared => {
                // What is on disk becomes the base the next save expects, so a further change
                // there conflicts again; the shared text stays, unsaved.
                if let Some(state) = self.states.get_mut(&id) {
                    match &on_disk {
                        Some(text) => state.mark_saved(text),
                        None => state.mark_saved(""),
                    }
                    state.changed_on_disk = false;
                }
                if let Some(shared) = self.shared.get_mut(&id) {
                    shared.conflict = false;
                    shared.saved = None;
                }
                if let Some(document) = self.documents.get_mut(&id) {
                    document.dirty = true;
                }
                self.auto_save_settled(id);
                self.auto_save_edited(id);
            }
            DiskResolution::ReloadDisk => {
                let text = on_disk.ok_or_else(|| refused("the file is no longer there"))?;
                if text.len() > MAX_SHARED_FILE {
                    return Err(refused("the file is now too large to share"));
                }
                let replica = Replica::new(&text, ReplicaId::random())
                    .map_err(|error| collab_error(&error))?;
                let current = self
                    .documents
                    .get(&id)
                    .map(|document| document.text.to_string())
                    .unwrap_or_default();
                if let Some(edit) = edit::minimal(&current, &text) {
                    self.edit_document(id, vec![edit], false, EditOrigin::Local)?;
                }
                if let Some(shared) = self.shared.get_mut(&id) {
                    shared.replica = replica;
                    shared.epoch += 1;
                    shared.outbox.clear();
                    shared.typing = None;
                    shared.group = None;
                    shared.saving.clear();
                    shared.conflict = false;
                    shared.saved = Some(revision_of(&text));
                }
                self.mark_saved(id, &text);
            }
        }
        self.emit(EngineEvent::SessionChanged);
        Ok(())
    }

    /// Undo or redo of a shared document: the local author's last group only, from the replica.
    pub(crate) fn shared_undo(&mut self, id: DocumentId, redo: bool) -> bool {
        let Some(shared) = self.shared.get_mut(&id) else {
            return false;
        };
        if shared.role == (SharedRole::Guest { can_edit: false }) {
            return false;
        }
        let change = if redo {
            shared.replica.redo_local()
        } else {
            shared.replica.undo_local()
        };
        let change = match change {
            Ok(Some(change)) => change,
            Ok(None) => return false,
            Err(_) => {
                self.broken(id);
                return false;
            }
        };
        shared.typing = None;
        shared.group = None;
        if let Some(update) = change.outbound {
            shared.outbox.push(update);
        }
        let origin = if redo {
            EditOrigin::Redo
        } else {
            EditOrigin::Undo
        };
        if self
            .edit_document(id, text_edits(&change.edits), false, origin)
            .is_err()
        {
            self.broken(id);
            return false;
        }
        self.emit(EngineEvent::SharedUpdates(id));
        true
    }

    /// Whether `id` is a guest's shared document, which only the host saves.
    pub(crate) fn is_shared_guest(&self, id: DocumentId) -> bool {
        self.shared
            .get(&id)
            .is_some_and(|shared| matches!(shared.role, SharedRole::Guest { .. }))
    }
}
