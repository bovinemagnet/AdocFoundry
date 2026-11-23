//! Carets and selections that follow their text, sent as presence (awareness): ephemeral, never
//! part of the shared document, and throttled.

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    time::{Duration, Instant},
};

use yrs::{
    updates::{decoder::Decode, encoder::Encode},
    Assoc, IndexedSequence, StickyIndex, Transact,
};

use crate::{CollabError, Replica, AWARENESS_INTERVAL, MAX_ANCHOR};

/// Which side of an insertion at its place an anchor stays on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Affinity {
    /// Text inserted at the anchor goes after it: the anchor stays before.
    Before,
    /// Text inserted at the anchor goes before it: the anchor moves after, as a caret does.
    After,
}

/// A selection's two ends as positions relative to the text around them, encoded to send.
/// Resolved against any replica of the same file, it gives byte offsets there, however the
/// text changed meanwhile. Replicas holding the same updates agree, except where text was undone
/// and redone after the selection was made: `yrs` follows an undone item to its redone copy only
/// on the replica that redid (#786). So one replica's resolution is no proof of another's; to be
/// sure another places it alike, make the selection again from the text as it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelativeSelection {
    pub anchor: Vec<u8>,
    pub head: Vec<u8>,
}

impl RelativeSelection {
    /// A selection read from the wire: each end at most [`MAX_ANCHOR`] bytes and decodable.
    pub fn from_wire(anchor: Vec<u8>, head: Vec<u8>) -> Result<Self, CollabError> {
        for end in [&anchor, &head] {
            if end.len() > MAX_ANCHOR {
                return Err(CollabError::TooLarge(end.len()));
            }
            catch_unwind(|| StickyIndex::decode_v1(end))
                .map_err(|_| CollabError::MalformedUpdate("unreadable".to_owned()))?
                .map_err(|error| CollabError::MalformedUpdate(error.to_string()))?;
        }
        Ok(Self { anchor, head })
    }
}

impl Replica {
    /// The selection from `anchor` to `head` (byte offsets of this replica's text, on character
    /// boundaries the caller has checked), relative to the text around it.
    pub fn selection(
        &self,
        anchor: usize,
        head: usize,
        affinity: Affinity,
    ) -> Result<RelativeSelection, CollabError> {
        let len = self.len();
        let assoc = match affinity {
            Affinity::Before => Assoc::Before,
            Affinity::After => Assoc::After,
        };
        let encode = |offset: usize| {
            if offset > len {
                return Err(CollabError::InvalidBoundary(offset));
            }
            catch_unwind(AssertUnwindSafe(|| {
                let txn = self.doc().transact();
                self.text_ref()
                    .sticky_index(&txn, offset as u32, assoc)
                    .map(|index| index.encode_v1())
            }))
            .map_err(|_| CollabError::ProjectionMismatch)?
            .ok_or(CollabError::InvalidBoundary(offset))
        };
        Ok(RelativeSelection {
            anchor: encode(anchor)?,
            head: encode(head)?,
        })
    }

    /// Where `selection`'s ends are now in this replica's text, as byte offsets; `None` if
    /// either end is too long, unreadable, belongs to text this replica does not hold, or falls
    /// past the end. A selection comes from another participant, so it is read inside the panic
    /// guard; the caller checks the offsets against its rope before drawing anything there.
    #[must_use]
    pub fn resolve(&self, selection: &RelativeSelection) -> Option<(usize, usize)> {
        let len = self.len();
        let end = |bytes: &Vec<u8>| -> Option<usize> {
            if bytes.len() > MAX_ANCHOR {
                return None;
            }
            let offset = catch_unwind(AssertUnwindSafe(|| {
                let index = StickyIndex::decode_v1(bytes).ok()?;
                let txn = self.doc().transact();
                index.get_offset(&txn).map(|offset| offset.index as usize)
            }))
            .ok()??;
            (offset <= len).then_some(offset)
        };
        Some((end(&selection.anchor)?, end(&selection.head)?))
    }
}

/// Paces presence: at most one message per [`AWARENESS_INTERVAL`], the latest state winning.
/// The caller offers every change and sends when told to; nothing here reads a clock.
#[derive(Debug)]
pub struct AwarenessThrottle {
    interval: Duration,
    last_sent: Option<Instant>,
    waiting: bool,
}

impl Default for AwarenessThrottle {
    fn default() -> Self {
        Self::new(AWARENESS_INTERVAL)
    }
}

impl AwarenessThrottle {
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_sent: None,
            waiting: false,
        }
    }

    /// A change at `now`: true if it may be sent now (and is counted as sent); otherwise it
    /// waits, to be sent at [`AwarenessThrottle::due`].
    pub fn offer(&mut self, now: Instant) -> bool {
        match self.last_sent {
            Some(last) if now.saturating_duration_since(last) < self.interval => {
                self.waiting = true;
                false
            }
            _ => {
                self.last_sent = Some(now);
                self.waiting = false;
                true
            }
        }
    }

    /// When the waiting change may be sent, if one waits.
    #[must_use]
    pub fn due(&self) -> Option<Instant> {
        self.waiting
            .then(|| self.last_sent.map(|last| last + self.interval))
            .flatten()
    }

    /// True if a waiting change may be sent at `now`, counting it as sent.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.due() {
            Some(due) if now >= due => {
                self.last_sent = Some(now);
                self.waiting = false;
                true
            }
            _ => false,
        }
    }
}
