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
}

impl SharedDocument {
    /// Whether local changes wait to be sent.
    pub(crate) fn has_outbox(&self) -> bool {
        !self.outbox.is_empty()
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
        self.drop_undo_history(id);
        self.shared.insert(
            id,
            SharedDocument {
                replica,
                role: SharedRole::Host,
                file,
                epoch,
                outbox: Vec::new(),
                typing: None,
                group: None,
            },
        );
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
        let id = self.insert_document(std::path::PathBuf::from(name), &replica.text(), true);
        self.shared.insert(
            id,
            SharedDocument {
                replica,
                role: SharedRole::Guest { can_edit },
                file,
                epoch,
                outbox: Vec::new(),
                typing: None,
                group: None,
            },
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
