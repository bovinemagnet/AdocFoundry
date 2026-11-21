//! COLLAB 1/8 (#688): a throwaway prototype of the replica `adoc-collab` will hold, over `yrs`,
//! to measure what the CRDT does before COLLAB 2/8 designs the real one. One shared text per
//! replica, positions in UTF-8 bytes as everywhere in AdocFoundry, every change a list of byte
//! edits in pre-edit coordinates (the engine's `TextEdit` shape), and only encoded bytes leave a
//! replica.

use std::{
    collections::HashSet,
    ops::Range,
    sync::{Arc, Mutex},
};

use yrs::{
    types::Delta,
    undo::{Options as UndoOptions, UndoManager},
    updates::{decoder::Decode, encoder::Encode},
    Assoc, ClientID, Doc, GetString, IndexedSequence, Observable, OffsetKind, Options, Origin, Out,
    ReadTxn, StateVector, StickyIndex, Text, TextRef, Transact, Update,
};

/// The name of the one shared text in each document.
const TEXT: &str = "text";
/// The origin of the transactions that apply a peer's update.
const REMOTE: &str = "remote";
/// The origin of the transactions that apply a local author's edits.
const LOCAL: &str = "local";

/// A replacement of the bytes `range` (in the text before the batch it belongs to) by `text`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn new(range: Range<usize>, text: &str) -> Self {
        Self {
            range,
            text: text.to_owned(),
        }
    }
}

/// What the replica refuses.
#[derive(Debug, Eq, PartialEq)]
pub enum SpikeError {
    /// An offset is past the end or inside a character.
    InvalidBoundary(usize),
    /// Edits overlap or are out of order.
    Unordered,
    /// The bytes are not an update, or `yrs` refused to apply them.
    MalformedUpdate(String),
    /// The bytes are larger than the limit given.
    TooLarge(usize),
    /// What `yrs` reported changing is not what was asked for, or does not rebuild its text.
    ProjectionMismatch,
}

/// Applies `edits` (ordered, non-overlapping, in pre-edit coordinates) to `text`.
pub fn apply_edits(text: &mut String, edits: &[Edit]) {
    for edit in edits.iter().rev() {
        text.replace_range(edit.range.clone(), &edit.text);
    }
}

/// Checks that `edits` are ordered, non-overlapping and on character boundaries of `text`.
pub fn validate(text: &str, edits: &[Edit]) -> Result<(), SpikeError> {
    let mut end = 0;
    for edit in edits {
        if edit.range.start < end || edit.range.start > edit.range.end {
            return Err(SpikeError::Unordered);
        }
        for offset in [edit.range.start, edit.range.end] {
            if !text.is_char_boundary(offset) {
                return Err(SpikeError::InvalidBoundary(offset));
            }
        }
        end = edit.range.end;
    }
    Ok(())
}

/// `edits` with empty ones dropped, as a delta reports them.
fn normalise(edits: &[Edit]) -> Vec<Edit> {
    edits
        .iter()
        .filter(|edit| !(edit.range.is_empty() && edit.text.is_empty()))
        .cloned()
        .collect()
}

/// The byte edits, in pre-edit coordinates, that a text delta describes; adjacent deletions and
/// insertions at one place are one edit.
fn delta_to_edits(delta: &[Delta]) -> Vec<Edit> {
    let mut edits: Vec<Edit> = Vec::new();
    let mut at = 0usize;
    let push = |edits: &mut Vec<Edit>, range: Range<usize>, text: &str| match edits.last_mut() {
        Some(last) if last.range.end == range.start => {
            last.range.end = range.end;
            last.text.push_str(text);
        }
        _ => edits.push(Edit::new(range, text)),
    };
    for change in delta {
        match change {
            Delta::Retain(length, _) => at += *length as usize,
            Delta::Deleted(length) => {
                push(&mut edits, at..at + *length as usize, "");
                at += *length as usize;
            }
            Delta::Inserted(value, _) => {
                let text = match value {
                    Out::Any(yrs::Any::String(text)) => text.to_string(),
                    other => panic!("not text: {other:?}"),
                };
                push(&mut edits, at..at, &text);
            }
        }
    }
    edits
}

/// The clients whose items an encoded update creates, read without applying it: the host
/// refuses a guest's update that creates any but the guest's own.
pub fn authors(update: &[u8]) -> Result<Vec<u64>, SpikeError> {
    let update = Update::decode_v1(update)
        .map_err(|error| SpikeError::MalformedUpdate(error.to_string()))?;
    let mut clients: Vec<u64> = update
        .insertions(true)
        .iter()
        .map(|(client, _)| client.get())
        .collect();
    clients.sort_unstable();
    clients.dedup();
    Ok(clients)
}

/// One participant's copy of a shared text.
pub struct Replica {
    doc: Doc,
    text: TextRef,
    undo: UndoManager<()>,
    /// What each committed transaction changed, as byte edits, since last taken.
    changes: Arc<Mutex<Vec<Vec<Edit>>>>,
    /// The updates of transactions this replica made itself, since last taken.
    outgoing: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Whether each change is checked against the whole text (two copies of it), boundaries
    /// included.
    verify_whole_text: bool,
}

impl Replica {
    /// An empty replica for the participant `client`. The undo manager groups nothing by time:
    /// a group ends where [`Replica::end_group`] says, so tests never depend on a clock.
    pub fn new(client: u64) -> Self {
        let mut options = Options::with_client_id(ClientID::new(client));
        options.offset_kind = OffsetKind::Bytes;
        let doc = Doc::with_options(options);
        let text = doc.get_or_insert_text(TEXT);
        let changes = Arc::new(Mutex::new(Vec::new()));
        let seen = changes.clone();
        text.observe("bridge", move |txn, event| {
            seen.lock().unwrap().push(delta_to_edits(event.delta(txn)));
        });
        let outgoing = Arc::new(Mutex::new(Vec::new()));
        let sent = outgoing.clone();
        let remote = Origin::from(REMOTE);
        doc.observe_update_v1("outgoing", move |txn, event| {
            if txn.origin() != Some(&remote) {
                sent.lock().unwrap().push(event.update.clone());
            }
        })
        .expect("no transaction is open");
        let mut tracked = HashSet::new();
        tracked.insert(Origin::from(LOCAL));
        let mut undo = UndoManager::with_options(UndoOptions {
            capture_timeout_millis: u64::MAX,
            tracked_origins: tracked,
            ..UndoOptions::default()
        });
        undo.expand_scope(&doc, &text);
        Self {
            doc,
            text,
            undo,
            changes,
            outgoing,
            verify_whole_text: true,
        }
    }

    /// Turns the whole-text check of every change off, to measure what `yrs` costs alone.
    pub fn set_verify_whole_text(&mut self, verify: bool) {
        self.verify_whole_text = verify;
    }

    pub fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }

    fn take_changes(&self) -> Vec<Edit> {
        let batches = std::mem::take(&mut *self.changes.lock().unwrap());
        assert!(
            batches.len() <= 1,
            "one transaction, one batch: {batches:?}"
        );
        batches.into_iter().next().unwrap_or_default()
    }

    fn take_outgoing(&self) -> Vec<u8> {
        let mut updates = std::mem::take(&mut *self.outgoing.lock().unwrap());
        assert!(updates.len() <= 1, "one transaction, one update");
        updates.pop().unwrap_or_default()
    }

    /// Applies the local author's `edits` as one transaction and gives the update to send. A
    /// refused batch changes nothing and sends nothing.
    pub fn apply_local(&mut self, edits: &[Edit]) -> Result<Vec<u8>, SpikeError> {
        // Without the whole-text check the boundaries are the caller's to check, as the
        // engine's `TextEdit::validate` checks them against its rope.
        let before = if self.verify_whole_text {
            let before = self.text();
            validate(&before, edits)?;
            before
        } else {
            String::new()
        };
        {
            let mut txn = self.doc.transact_mut_with(LOCAL);
            for edit in edits.iter().rev() {
                let start = edit.range.start as u32;
                let length = (edit.range.end - edit.range.start) as u32;
                if length > 0 {
                    self.text.remove_range(&mut txn, start, length);
                }
                if !edit.text.is_empty() {
                    self.text.insert(&mut txn, start, &edit.text);
                }
            }
        }
        let applied = self.take_changes();
        if self.verify_whole_text {
            let mut rebuilt = before;
            apply_edits(&mut rebuilt, &applied);
            if rebuilt != self.text() {
                return Err(SpikeError::ProjectionMismatch);
            }
        } else if applied != normalise(edits) {
            return Err(SpikeError::ProjectionMismatch);
        }
        Ok(self.take_outgoing())
    }

    /// Applies a peer's update, at most `limit` bytes, and gives the byte edits it made, in
    /// pre-edit coordinates, for the engine's rope.
    pub fn apply_remote(&mut self, update: &[u8], limit: usize) -> Result<Vec<Edit>, SpikeError> {
        if update.len() > limit {
            return Err(SpikeError::TooLarge(update.len()));
        }
        let decoded = Update::decode_v1(update)
            .map_err(|error| SpikeError::MalformedUpdate(error.to_string()))?;
        let before = if self.verify_whole_text {
            self.text()
        } else {
            String::new()
        };
        {
            let mut txn = self.doc.transact_mut_with(REMOTE);
            txn.apply_update(decoded)
                .map_err(|error| SpikeError::MalformedUpdate(error.to_string()))?;
        }
        let edits = self.take_changes();
        if self.verify_whole_text {
            let mut rebuilt = before;
            validate(&rebuilt, &edits)?;
            apply_edits(&mut rebuilt, &edits);
            if rebuilt != self.text() {
                return Err(SpikeError::ProjectionMismatch);
            }
        }
        Ok(edits)
    }

    /// Ends the local author's current undo group.
    pub fn end_group(&mut self) {
        self.undo.reset();
    }

    /// Undoes the local author's last group: the byte edits it made and the update to send.
    pub fn undo(&mut self) -> Option<(Vec<Edit>, Vec<u8>)> {
        self.undo
            .undo_blocking()
            .then(|| (self.take_changes(), self.take_outgoing()))
    }

    /// Redoes the local author's last undone group.
    pub fn redo(&mut self) -> Option<(Vec<Edit>, Vec<u8>)> {
        self.undo
            .redo_blocking()
            .then(|| (self.take_changes(), self.take_outgoing()))
    }

    /// This replica's state vector, encoded.
    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// Everything this replica holds that a peer whose state vector is `remote` lacks.
    pub fn missing_since(&self, remote: &[u8]) -> Result<Vec<u8>, SpikeError> {
        let remote = StateVector::decode_v1(remote)
            .map_err(|error| SpikeError::MalformedUpdate(error.to_string()))?;
        Ok(self.doc.transact().encode_state_as_update_v1(&remote))
    }

    /// The whole state, as a snapshot a new replica starts from.
    pub fn snapshot(&self) -> Vec<u8> {
        self.missing_since(&StateVector::default().encode_v1())
            .expect("an empty state vector")
    }

    /// A position that follows the text around `offset`, encoded to send as a peer's caret.
    pub fn anchor(&self, offset: usize, assoc: Assoc) -> Vec<u8> {
        let txn = self.doc.transact();
        self.text
            .sticky_index(&txn, offset as u32, assoc)
            .expect("an offset inside the text")
            .encode_v1()
    }

    /// Where an encoded anchor is now, in bytes.
    pub fn resolve(&self, anchor: &[u8]) -> Option<usize> {
        let anchor = StickyIndex::decode_v1(anchor).ok()?;
        let txn = self.doc.transact();
        anchor.get_offset(&txn).map(|offset| offset.index as usize)
    }

    /// Whether `update` can integrate here without waiting for anything, as far as can be seen
    /// without applying it: for each client, its items start no later than this replica's clock,
    /// and its deletions fall within what this replica holds or the update adds. An item whose
    /// origin is unknown here is not visible this way; [`Replica::has_pending`] after applying
    /// catches it.
    pub fn precheck(&self, update: &[u8]) -> Result<bool, SpikeError> {
        let update = Update::decode_v1(update)
            .map_err(|error| SpikeError::MalformedUpdate(error.to_string()))?;
        let here = self.doc.transact().state_vector();
        let lower = update.state_vector_lower();
        if lower
            .iter()
            .any(|(client, clock)| *clock > here.get(client))
        {
            return Ok(false);
        }
        let upper = update.state_vector();
        for (client, ranges) in update.delete_set().iter() {
            let known = here.get(client).max(upper.get(client));
            if ranges.iter().any(|range| range.end > known) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether `yrs` holds updates or deletions it cannot integrate until others arrive.
    pub fn has_pending(&self) -> bool {
        self.doc.transact().has_missing_updates()
    }

    /// The encoded size of the whole state.
    pub fn state_size(&self) -> usize {
        self.snapshot().len()
    }
}
