//! One participant's copy of one shared file's text, over a `yrs` document.

use std::{
    collections::HashSet,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex},
};

use yrs::{
    types::Delta,
    undo::{Options as UndoOptions, UndoManager},
    updates::{decoder::Decode, encoder::Encode},
    Any, ClientID, Doc, GetString, Observable, OffsetKind, Options, Origin, Out, ReadTxn,
    StateVector, Text, TextRef, Transact, Update,
};

use crate::{
    edit::{apply_edits, check_boundaries, check_order, length_after, normalise},
    wire::{check_state_vector_shape, check_update_shape},
    ByteEdit, CollabError, MAX_SHARED_FILE, MAX_UPDATE,
};

/// The name of the one shared text in each file's document.
const TEXT: &str = "text";
/// Transaction origins. Only [`LOCAL`] is tracked by the undo manager.
const LOCAL: &str = "adoc-collab:local";
const REMOTE: &str = "adoc-collab:remote";
const INITIAL: &str = "adoc-collab:initial";

/// A participant's identity in one file's replica: `yrs`'s client id, at most 53 bits and never
/// zero. The host assigns it (design §7) and checks a guest's updates against it ([`authors`]).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplicaId(u64);

impl ReplicaId {
    /// The largest id: `yrs` keeps client ids to 53 bits, the integers a JavaScript peer could
    /// hold exactly.
    pub const MAX: u64 = (1 << 53) - 1;

    /// The id `value`, if it is between 1 and [`ReplicaId::MAX`].
    #[must_use]
    pub fn new(value: u64) -> Option<Self> {
        (1..=Self::MAX).contains(&value).then_some(Self(value))
    }

    /// A random id, which the host checks against those already given in the session.
    #[must_use]
    pub fn random() -> Self {
        loop {
            if let Some(id) = Self::new(ClientID::random().get()) {
                return id;
            }
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A replica's state vector, encoded: what it holds from each participant.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StateVectorBytes(pub Vec<u8>);

/// An update, encoded (`yrs`'s lib0 v1 encoding), to send to other replicas.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EncodedUpdate(pub Vec<u8>);

/// What made a change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeOrigin {
    /// The local author's edit.
    Local,
    /// Another participant's update.
    Remote,
    /// The local author's undo.
    Undo,
    /// The local author's redo.
    Redo,
}

/// Who sent an update, which decides what it may hold (design §6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sender {
    /// The host, to a guest: anyone's items, forwarded in order.
    Host,
    /// A guest, to the host: that guest's own items only.
    Guest(ReplicaId),
}

/// What a change did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedChange {
    pub origin: ChangeOrigin,
    /// The byte edits it made, one batch in pre-change coordinates ([`ByteEdit`]), for the
    /// engine's rope. Empty when nothing visible changed.
    pub edits: Vec<ByteEdit>,
    /// The update to send to the other replicas: always for the local author's changes, never
    /// for a remote one.
    pub outbound: Option<EncodedUpdate>,
    /// Whether the replica now holds something it cannot integrate until something else arrives
    /// (`yrs`'s pending updates). A guest then asks the host to resynchronise the file; the host
    /// treats it as the sender's violation and rebuilds the replica from its own text (design
    /// §2).
    pub pending: bool,
}

/// One participant's copy of one shared file's text.
///
/// The text is held exactly as given: line endings, a byte order mark and Unicode normalisation
/// are never rewritten. A replica that fails in a way that may have changed it (a panic inside
/// `yrs`, a projection mismatch, a remote update that makes the text too large) is poisoned:
/// every later call is refused with [`CollabError::Poisoned`], and the caller builds a new
/// replica in a new epoch.
pub struct Replica {
    id: ReplicaId,
    doc: Doc,
    text: TextRef,
    undo: UndoManager<()>,
    /// What each committed transaction changed, since last taken.
    observed: Arc<Mutex<Vec<Vec<ByteEdit>>>>,
    /// The updates of transactions this replica made itself, since last taken.
    outgoing: Arc<Mutex<Vec<Vec<u8>>>>,
    poisoned: bool,
}

impl std::fmt::Debug for Replica {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the text: no document content in logs (PRD §5.5).
        formatter
            .debug_struct("Replica")
            .field("id", &self.id)
            .field("len", &self.len())
            .field("poisoned", &self.poisoned)
            .finish()
    }
}

/// The byte edits, in pre-transaction coordinates, that a text delta describes, or `None` when
/// it inserts something other than text (which only a modified peer could send).
fn delta_to_edits(delta: &[Delta]) -> Option<Vec<ByteEdit>> {
    let mut edits: Vec<ByteEdit> = Vec::new();
    let mut at = 0usize;
    let push =
        |edits: &mut Vec<ByteEdit>, start: usize, end: usize, text: &str| match edits.last_mut() {
            Some(last) if last.range.end == start => {
                last.range.end = end;
                last.text.push_str(text);
            }
            _ => edits.push(ByteEdit::new(start, end, text)),
        };
    for change in delta {
        match change {
            Delta::Retain(length, _) => at += *length as usize,
            Delta::Deleted(length) => {
                let end = at + *length as usize;
                push(&mut edits, at, end, "");
                at = end;
            }
            Delta::Inserted(Out::Any(Any::String(text)), _) => push(&mut edits, at, at, text),
            Delta::Inserted(..) => return None,
        }
    }
    Some(edits)
}

impl Replica {
    fn empty(id: ReplicaId) -> Self {
        let mut options = Options::with_client_id(ClientID::new(id.get()));
        options.offset_kind = OffsetKind::Bytes;
        let doc = Doc::with_options(options);
        let text = doc.get_or_insert_text(TEXT);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let seen = observed.clone();
        text.observe("adoc-collab:bridge", move |txn, event| {
            let edits = delta_to_edits(event.delta(txn));
            // `None` (something other than text) is recorded as an impossible batch, which the
            // caller's projection check refuses.
            let edits = edits.unwrap_or_else(|| vec![ByteEdit::new(1, 0, "")]);
            if let Ok(mut observed) = seen.lock() {
                observed.push(edits);
            }
        });
        let outgoing = Arc::new(Mutex::new(Vec::new()));
        let sent = outgoing.clone();
        let remote = Origin::from(REMOTE);
        doc.observe_update_v1("adoc-collab:outgoing", move |txn, event| {
            if txn.origin() != Some(&remote) {
                if let Ok(mut sent) = sent.lock() {
                    sent.push(event.update.clone());
                }
            }
        })
        .expect("a new document has no open transaction");
        let mut tracked = HashSet::new();
        tracked.insert(Origin::from(LOCAL));
        // Groups end only where the caller says (`end_group`), as the engine's coalescing rules
        // decide, never by `yrs`'s clock.
        let mut undo = UndoManager::with_options(UndoOptions {
            capture_timeout_millis: u64::MAX,
            tracked_origins: tracked,
            ..UndoOptions::default()
        });
        undo.expand_scope(&doc, &text);
        Self {
            id,
            doc,
            text,
            undo,
            observed,
            outgoing,
            poisoned: false,
        }
    }

    /// The host's replica of a file whose text is `text`. The text is not in the undo history.
    pub fn new(text: &str, id: ReplicaId) -> Result<Self, CollabError> {
        if text.len() > MAX_SHARED_FILE {
            return Err(CollabError::TooLarge(text.len()));
        }
        let replica = Self::empty(id);
        if !text.is_empty() {
            let mut txn = replica.doc.transact_mut_with(INITIAL);
            replica.text.insert(&mut txn, 0, text);
        }
        replica.take_observed();
        replica.take_outgoing();
        if replica.len() != text.len() {
            return Err(CollabError::ProjectionMismatch);
        }
        Ok(replica)
    }

    /// A guest's replica of a file, from the host's snapshot of it. The guest never creates the
    /// text itself, so it can never be bootstrapped twice.
    pub fn from_snapshot(snapshot: &EncodedUpdate, id: ReplicaId) -> Result<Self, CollabError> {
        let mut replica = Self::empty(id);
        let change = replica.apply_remote(snapshot, Sender::Host)?;
        if change.pending {
            return Err(CollabError::MalformedUpdate(
                "the snapshot depends on something it does not hold".to_owned(),
            ));
        }
        Ok(replica)
    }

    #[must_use]
    pub fn id(&self) -> ReplicaId {
        self.id
    }

    /// The text's length in bytes, without copying it.
    #[must_use]
    pub fn len(&self) -> usize {
        self.text.len(&self.doc.transact()) as usize
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole text, copied: for tests, resynchronisation and saving, not every keystroke.
    #[must_use]
    pub fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }

    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Whether `yrs` holds updates or deletions it cannot integrate until others arrive.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.doc.transact().has_missing_updates()
    }

    fn usable(&self) -> Result<(), CollabError> {
        if self.poisoned {
            Err(CollabError::Poisoned)
        } else {
            Ok(())
        }
    }

    fn poison(&mut self, error: CollabError) -> CollabError {
        self.poisoned = true;
        error
    }

    fn take_observed(&self) -> Vec<ByteEdit> {
        let batches = self
            .observed
            .lock()
            .map(|mut observed| std::mem::take(&mut *observed))
            .unwrap_or_default();
        // One transaction commits one batch; more would be a projection the caller can't trust.
        match batches.len() {
            0 => Vec::new(),
            1 => batches.into_iter().next().unwrap_or_default(),
            _ => vec![ByteEdit::new(1, 0, "")],
        }
    }

    fn take_outgoing(&self) -> Option<EncodedUpdate> {
        let updates = self
            .outgoing
            .lock()
            .map(|mut outgoing| std::mem::take(&mut *outgoing))
            .unwrap_or_default();
        updates.into_iter().last().map(EncodedUpdate)
    }

    /// Applies the local author's `edits` (ordered, non-overlapping, in the text's current
    /// coordinates) as one transaction in the local undo history. The caller has checked every
    /// offset against its own copy of the text (the engine's `TextEdit::validate`), since `yrs`
    /// moves an offset inside a character without a word; this replica checks order, bounds and
    /// size. A refused batch changes nothing and sends nothing.
    pub fn apply_local(&mut self, edits: &[ByteEdit]) -> Result<AppliedChange, CollabError> {
        self.usable()?;
        let len = self.len();
        check_order(edits, len)?;
        let after = length_after(len, edits);
        if after > MAX_SHARED_FILE {
            return Err(CollabError::TooLarge(after));
        }
        let wanted = normalise(edits);
        if wanted.is_empty() {
            return Ok(AppliedChange {
                origin: ChangeOrigin::Local,
                edits: Vec::new(),
                outbound: None,
                pending: false,
            });
        }
        let applied = catch_unwind(AssertUnwindSafe(|| {
            let mut txn = self.doc.transact_mut_with(LOCAL);
            for edit in wanted.iter().rev() {
                let start = edit.range.start as u32;
                let removed = (edit.range.end - edit.range.start) as u32;
                if removed > 0 {
                    self.text.remove_range(&mut txn, start, removed);
                }
                if !edit.text.is_empty() {
                    self.text.insert(&mut txn, start, &edit.text);
                }
            }
        }));
        if applied.is_err() {
            return Err(self.poison(CollabError::ProjectionMismatch));
        }
        let observed = self.take_observed();
        if observed != wanted || self.len() != after {
            return Err(self.poison(CollabError::ProjectionMismatch));
        }
        Ok(AppliedChange {
            origin: ChangeOrigin::Local,
            edits: observed,
            outbound: self.take_outgoing(),
            pending: false,
        })
    }

    /// Applies an update from `sender`, at most [`MAX_UPDATE`] bytes, and gives the byte edits
    /// it made.
    ///
    /// The update is admitted first, unread by `yrs` until it is: every participant's items in
    /// it must form one unbroken run starting exactly where this replica's copy of that
    /// participant's work ends, and an update from a guest may hold only that guest's items. In
    /// the star every link is ordered and the host forwards only what it has applied, so an
    /// honest update always passes; a duplicate, a reordering, a gap or a forgery does not
    /// ([`CollabError::NotAdmitted`]). The rule is not only against forgery: `yrs` 0.28.2
    /// integrates a block overlapping items it already holds into freed memory (found by the
    /// `collab_update` fuzz target), so such an update must never reach it. Deletions carry no
    /// author and are admitted; who may delete is the engine's to check.
    ///
    /// Bytes that do not decode change nothing. An update `yrs` refuses after decoding changes
    /// nothing either, or poisons the replica if it did. An update that would make the text
    /// larger than [`MAX_SHARED_FILE`] poisons it: there is no undoing a `yrs` transaction.
    ///
    /// The text after the update is compared whole with the text before it and the byte edits
    /// reported, since a hostile update can change text the edits do not show: `yrs` counts
    /// positions in UTF-16 units, and an update naming a position inside a character makes it
    /// split the character into replacement characters. A mismatch poisons the replica. The
    /// comparison copies the text twice (about 90 µs for 1 MiB), which a remote update can
    /// afford and a keystroke, checked by its delta alone in [`Replica::apply_local`], could not.
    pub fn apply_remote(
        &mut self,
        update: &EncodedUpdate,
        sender: Sender,
    ) -> Result<AppliedChange, CollabError> {
        self.usable()?;
        if update.0.len() > MAX_UPDATE {
            return Err(CollabError::TooLarge(update.0.len()));
        }
        check_update_shape(&update.0)?;
        let decoded = catch_unwind(|| Update::decode_v1(&update.0));
        let decoded = match decoded {
            Ok(Ok(decoded)) => decoded,
            Ok(Err(error)) => return Err(CollabError::MalformedUpdate(error.to_string())),
            Err(_) => return Err(CollabError::MalformedUpdate("unreadable".to_owned())),
        };
        self.admit(&decoded, sender)?;
        let text = self.text();
        let before = self.doc.transact().state_vector();
        let applied = catch_unwind(AssertUnwindSafe(|| {
            let mut txn = self.doc.transact_mut_with(REMOTE);
            txn.apply_update(decoded)
        }));
        let edits = self.take_observed();
        self.take_outgoing();
        match applied {
            Err(_) => return Err(self.poison(CollabError::ProjectionMismatch)),
            Ok(Err(error)) => {
                let unchanged = edits.is_empty()
                    && self.doc.transact().state_vector() == before
                    && self.text() == text;
                let error = CollabError::MalformedUpdate(error.to_string());
                return Err(if unchanged { error } else { self.poison(error) });
            }
            Ok(Ok(())) => {}
        }
        let after = self.len();
        if after > MAX_SHARED_FILE {
            return Err(self.poison(CollabError::TooLarge(after)));
        }
        if !edits.is_empty() {
            if check_boundaries(&text, &edits).is_err() {
                return Err(self.poison(CollabError::ProjectionMismatch));
            }
            let mut expected = text;
            apply_edits(&mut expected, &edits);
            if expected != self.text() {
                return Err(self.poison(CollabError::ProjectionMismatch));
            }
        } else if after != text.len() || self.text() != text {
            return Err(self.poison(CollabError::ProjectionMismatch));
        }
        Ok(AppliedChange {
            origin: ChangeOrigin::Remote,
            edits,
            outbound: None,
            pending: self.has_pending(),
        })
    }

    /// Whether a decoded update is what `sender` could honestly send now ([`Replica::apply_remote`]).
    fn admit(&self, update: &Update, sender: Sender) -> Result<(), CollabError> {
        let held = self.doc.transact().state_vector();
        for (client, ranges) in update.insertions(true).iter() {
            if let Sender::Guest(author) = sender {
                if client.get() != author.get() {
                    return Err(CollabError::NotAdmitted(format!(
                        "items under {} from {}",
                        client.get(),
                        author.get()
                    )));
                }
            }
            let start = held.get(client);
            let runs: Vec<_> = ranges.iter().collect();
            if !matches!(runs.as_slice(), [run] if run.start == start && run.end > run.start) {
                return Err(CollabError::NotAdmitted(format!(
                    "items under {} not one run from {start}",
                    client.get()
                )));
            }
        }
        let lower = update.state_vector_lower();
        if let Some((client, clock)) = lower
            .iter()
            .find(|(client, clock)| **clock != held.get(client))
        {
            return Err(CollabError::NotAdmitted(format!(
                "blocks under {} from {clock}, not {}",
                client.get(),
                held.get(client)
            )));
        }
        Ok(())
    }

    /// Ends the local author's current undo group: the next local edit starts a new one. The
    /// engine calls it where its own coalescing rules end a group.
    pub fn end_group(&mut self) {
        self.undo.reset();
    }

    fn undo_or_redo(&mut self, origin: ChangeOrigin) -> Result<Option<AppliedChange>, CollabError> {
        self.usable()?;
        let len = self.len();
        let done = catch_unwind(AssertUnwindSafe(|| match origin {
            ChangeOrigin::Undo => self.undo.undo_blocking(),
            _ => self.undo.redo_blocking(),
        }));
        let edits = self.take_observed();
        let outbound = self.take_outgoing();
        match done {
            Err(_) => Err(self.poison(CollabError::ProjectionMismatch)),
            Ok(false) => Ok(None),
            Ok(true) => {
                if check_order(&edits, len).is_err() || length_after(len, &edits) != self.len() {
                    return Err(self.poison(CollabError::ProjectionMismatch));
                }
                Ok(Some(AppliedChange {
                    origin,
                    edits,
                    outbound,
                    pending: false,
                }))
            }
        }
    }

    /// Undoes the local author's last group, and only the local author's: other participants'
    /// edits, made between or inside, stay. `None` when there is nothing to undo.
    pub fn undo_local(&mut self) -> Result<Option<AppliedChange>, CollabError> {
        self.undo_or_redo(ChangeOrigin::Undo)
    }

    /// Redoes the local author's last undone group.
    pub fn redo_local(&mut self) -> Result<Option<AppliedChange>, CollabError> {
        self.undo_or_redo(ChangeOrigin::Redo)
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// This replica's state vector.
    #[must_use]
    pub fn state_vector(&self) -> StateVectorBytes {
        StateVectorBytes(self.doc.transact().state_vector().encode_v1())
    }

    /// Everything this replica holds that a replica whose state vector is `remote` lacks. A
    /// replica holding a pending update encodes nothing ([`CollabError::Pending`]), and the
    /// encoding runs inside the panic guard.
    pub fn missing_since(&self, remote: &StateVectorBytes) -> Result<EncodedUpdate, CollabError> {
        self.usable()?;
        if self.has_pending() {
            return Err(CollabError::Pending);
        }
        if remote.0.len() > MAX_UPDATE {
            return Err(CollabError::TooLarge(remote.0.len()));
        }
        check_state_vector_shape(&remote.0)?;
        let remote = catch_unwind(|| StateVector::decode_v1(&remote.0))
            .map_err(|_| CollabError::MalformedUpdate("unreadable".to_owned()))?
            .map_err(|error| CollabError::MalformedUpdate(error.to_string()))?;
        catch_unwind(AssertUnwindSafe(|| {
            self.doc.transact().encode_state_as_update_v1(&remote)
        }))
        .map(EncodedUpdate)
        .map_err(|_| CollabError::ProjectionMismatch)
    }

    /// The whole state, which a new replica starts from ([`Replica::from_snapshot`]), checked by
    /// doing just that: a snapshot that would rebuild another text is refused with
    /// [`CollabError::ProjectionMismatch`], and the caller rebuilds the replica from its own text
    /// (the host) before anyone joins from it.
    ///
    /// The check is not idle: an update a modified participant made, accepted and applied with
    /// every check passing, can leave `yrs` holding a state whose encoding rebuilds a different
    /// text (found by the `collab_update` fuzz target). Every participant already connected
    /// agrees; anyone joining or resynchronising from the snapshot would not. It costs a replica
    /// built from the snapshot, about a millisecond for 1 MiB, once a join or resynchronisation.
    pub fn snapshot(&self) -> Result<EncodedUpdate, CollabError> {
        let snapshot = self.missing_since(&StateVectorBytes(StateVector::default().encode_v1()))?;
        let check = if self.id.get() == ReplicaId::MAX {
            ReplicaId::MAX - 1
        } else {
            ReplicaId::MAX
        };
        let copy = Replica::from_snapshot(&snapshot, ReplicaId(check))
            .map_err(|_| CollabError::ProjectionMismatch)?;
        if copy.text() != self.text() {
            return Err(CollabError::ProjectionMismatch);
        }
        Ok(snapshot)
    }

    pub(crate) fn doc(&self) -> &Doc {
        &self.doc
    }

    pub(crate) fn text_ref(&self) -> &TextRef {
        &self.text
    }
}

/// The participants whose items `update` creates, read without applying it. The host refuses
/// a guest's update that creates any but the guest's own (design §6). A deletion creates none;
/// a guest's undo of a deletion and its redo create the guest's own.
pub fn authors(update: &EncodedUpdate) -> Result<Vec<ReplicaId>, CollabError> {
    if update.0.len() > MAX_UPDATE {
        return Err(CollabError::TooLarge(update.0.len()));
    }
    check_update_shape(&update.0)?;
    let decoded = catch_unwind(|| Update::decode_v1(&update.0))
        .map_err(|_| CollabError::MalformedUpdate("unreadable".to_owned()))?
        .map_err(|error| CollabError::MalformedUpdate(error.to_string()))?;
    let mut ids = Vec::new();
    for (client, _) in decoded.insertions(true).iter() {
        // An id outside the range a replica can have names no participant: refused.
        ids.push(ReplicaId::new(client.get()).ok_or_else(|| {
            CollabError::MalformedUpdate("an author that is no replica".to_owned())
        })?);
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}
