//! The seam between the transport and the engine (design §8): the engine owns a [`Link`] and
//! drains it a bounded number of events per `pump`; the transport fills it from the network and
//! wakes the engine. Neither waits on the other: receiving never blocks, and a send that would
//! have to wait is refused and the peer disconnected as slow.

use std::fmt;

use crate::protocol::{ErrorCode, FrameError, Message, PeerId};

/// What the transport reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkEvent {
    /// For a host: a connection showed a valid invitation and waits for [`Link::admit`] or
    /// [`Link::remove`]. Nothing about the workspace has been sent to it. `email` is the address
    /// the guest gave to be credited by in commits, already checked with
    /// [`crate::protocol::commit_email`]; the host never tells it anyone.
    Knock {
        peer: PeerId,
        name: String,
        email: Option<String>,
    },
    /// For a host: an admitted peer reconnected with its resume token. For a guest: the link to
    /// the host is back. Its `Join` follows as a message.
    Resumed { peer: PeerId },
    /// A message from an admitted peer (for a host) or from the host (for a guest), already
    /// decoded and held to the protocol's limits. Hello, Authenticate and heartbeats are the
    /// transport's and never arrive here.
    Message { peer: PeerId, message: Message },
    /// The connection to `peer` closed. A host keeps the peer, which may resume, until it is
    /// removed or sharing stops.
    Gone { peer: PeerId, reason: String },
}

/// Why a link refused something.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkError {
    /// No connection to that peer.
    Closed,
    /// The peer's queue is full; the transport disconnects it.
    Slow,
    /// The message breaks the protocol's limits and was not sent.
    Frame(FrameError),
}

impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("not connected"),
            Self::Slow => formatter.write_str("the peer is not keeping up"),
            Self::Frame(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LinkError {}

/// A session's transport as the engine's thread sees it. Every method returns at once.
pub trait Link: Send {
    /// The next event, if one has arrived.
    fn try_recv(&mut self) -> Option<LinkEvent>;

    /// Queues `message` for `peer` (a guest's only peer is [`PeerId::HOST`]).
    fn send(&mut self, peer: PeerId, message: Message) -> Result<(), LinkError>;

    /// For a host: lets a knocking peer in as a Viewer. The transport sends its `Join`, with
    /// the resume token only it holds; every message after that is the engine's.
    fn admit(&mut self, peer: PeerId) -> Result<(), LinkError>;

    /// Closes the connection to `peer` with `code`, and for a host forgets it: it cannot
    /// resume. A host uses it to deny a knock and to remove a peer.
    fn remove(&mut self, peer: PeerId, code: ErrorCode, message: &str);
}
