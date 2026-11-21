//! The collaboration core (COLLAB 2/8, #689): one replica of one shared file's text per
//! participant, over the `yrs` CRDT, exchanging only encoded bytes. Byte edits in and out, in the
//! UTF-8 offsets every other crate uses; state vectors and updates for synchronising; undo that
//! touches only the local author's work; selections that follow their text; and the limits
//! everything is held to.
//!
//! No network, files, threads, processes, UI or parser: the engine owns replicas on its thread,
//! maps files to them and checks who may do what; the transport moves their bytes. The decisions
//! behind it are `docs/design/DESIGN-AdocFoundry_Collaboration.md`, the evidence
//! `docs/qa/spike-collab.md`.
//!
//! ```
//! use adoc_collab::{ByteEdit, Replica, ReplicaId, Sender};
//!
//! let mut host = Replica::new("= Page\n", ReplicaId::new(1).unwrap()).unwrap();
//! let mut guest = Replica::from_snapshot(&host.snapshot().unwrap(), ReplicaId::new(2).unwrap()).unwrap();
//! let typed = host.apply_local(&[ByteEdit::new(7, 7, "Text.\n")]).unwrap();
//! let received = guest.apply_remote(&typed.outbound.unwrap(), Sender::Host).unwrap();
//! assert_eq!(received.edits, vec![ByteEdit::new(7, 7, "Text.\n")]);
//! assert_eq!(guest.text(), "= Page\nText.\n");
//! ```

mod anchor;
mod edit;
mod link;
pub mod protocol;
mod replica;
#[cfg(feature = "test-support")]
pub mod testing;
mod wire;

use std::time::Duration;

pub use anchor::{Affinity, AwarenessThrottle, RelativeSelection};
pub use edit::{apply_edits, check_boundaries, check_order, length_after, normalise, ByteEdit};
pub use link::{Link, LinkError, LinkEvent};
pub use replica::{
    authors, AppliedChange, ChangeOrigin, EncodedUpdate, Replica, ReplicaId, Sender,
    StateVectorBytes,
};

/// The largest encoded update a replica accepts, checked before decoding. Room for a whole
/// file's snapshot: its text, at most [`MAX_SHARED_FILE`], and the history around it, which the
/// spike measured at about 5% after a long session.
pub const MAX_UPDATE: usize = 16 << 20;
/// The largest shared text, in bytes.
pub const MAX_SHARED_FILE: usize = 8 << 20;
/// The largest encoded end of a [`RelativeSelection`].
pub const MAX_ANCHOR: usize = 256;
/// The least time between two presence messages from one participant.
pub const AWARENESS_INTERVAL: Duration = Duration::from_millis(50);

/// What a replica refuses, or why it failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CollabError {
    /// An offset past the end or inside a character.
    InvalidBoundary(usize),
    /// Edits out of order, overlapping or backwards.
    Unordered,
    /// Bytes that are not an update or a state vector, or that `yrs` refused.
    MalformedUpdate(String),
    /// Something larger than its limit: the size given.
    TooLarge(usize),
    /// What the CRDT reports changing is not what was asked for, or does not add up; the
    /// replica is poisoned.
    ProjectionMismatch,
    /// The replica failed earlier and must be rebuilt in a new epoch.
    Poisoned,
    /// The replica holds an update it cannot integrate yet, so it encodes nothing until it is
    /// resynchronised or rebuilt: `yrs` can panic, or write items at clocks never used, encoding
    /// a document with something pending (y-crdt/y-crdt#720, #744).
    Pending,
    /// An update from a participant that is not what that participant could honestly send now:
    /// items under another participant's id, or not starting where the receiver's copy of the
    /// sender's work ends ([`Replica::apply_from`]).
    NotAdmitted(String),
}

impl std::fmt::Display for CollabError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBoundary(offset) => {
                write!(
                    formatter,
                    "offset {offset} is past the end or inside a character"
                )
            }
            Self::Unordered => formatter.write_str("edits are out of order or overlap"),
            Self::MalformedUpdate(reason) => write!(formatter, "malformed update: {reason}"),
            Self::TooLarge(size) => write!(formatter, "{size} bytes is over the limit"),
            Self::ProjectionMismatch => {
                formatter.write_str("the shared text and its changes disagree")
            }
            Self::Poisoned => formatter.write_str("the shared text must be rebuilt"),
            Self::NotAdmitted(reason) => write!(formatter, "update not admitted: {reason}"),
            Self::Pending => formatter.write_str("the shared text waits for an update"),
        }
    }
}

impl std::error::Error for CollabError {}
