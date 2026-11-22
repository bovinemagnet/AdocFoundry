//! Collaboration's transport (COLLAB 4/8, #691; `docs/design/DESIGN-AdocFoundry_Collaboration.md`
//! §§3, 5, 6, 8): framed TLS 1.3 over TCP between a host and its guests, in process, with no
//! server of anyone else's.
//!
//! It authenticates and moves frames; it decides nothing about the workspace. The host listens
//! on one chosen address with a certificate made for the session ([`HostIdentity`]), hands out
//! single-use invitations ([`Invitation`]) and reports each guest who shows one as a knock; the
//! engine decides who comes in, what they may open and edit, and what every message does, over
//! the [`Link`](adoc_collab::Link) this crate gives it ([`HostLink`], [`GuestLink`]). A guest
//! checks the host's certificate against the invitation before sending anything, and sends its
//! secret only once the person has compared the safety code.
//!
//! Every queue is bounded: events wait for the engine (backpressure on the peer that sent them),
//! and frames for a peer wait in a queue the engine fills without waiting, a full one closing the
//! connection as slow. The transport runs on threads of its own, each with a current-thread
//! runtime; it reads no file and runs no program.

mod conn;
pub mod frames;
mod guest;
mod host;
mod identity;
mod invite;
pub mod tls;

use std::{fmt, sync::Arc, time::Duration};

use adoc_collab::protocol::{HANDSHAKE_TIMEOUT, HEARTBEAT_INTERVAL, INVITE_LIFETIME, LINK_TIMEOUT};

pub use guest::{connect, Connecting, GuestControl, GuestLink};
pub use host::{Host, HostLink};
pub use identity::{Fingerprint, HostIdentity, SafetyCode};
pub use invite::Invitation;

/// Called, from the transport's thread, whenever an event arrives for the engine: it should
/// only wake the front end, which then pumps the engine.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The most events waiting for the engine; a peer whose message would be one more waits.
pub const EVENT_QUEUE: usize = 256;

/// The most connections a host holds that have not been admitted: more are closed at once.
pub const MAX_PENDING: usize = 8;

/// The transport's times, the protocol's by default; tests shorten them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timings {
    /// From connecting to authenticating.
    pub handshake: Duration,
    /// How often a quiet link sends a heartbeat.
    pub heartbeat: Duration,
    /// How long a link may be silent before it is down.
    pub link_timeout: Duration,
    /// How long an invitation stays valid.
    pub invite_lifetime: Duration,
    /// How long a knock waits for the host to admit it.
    pub admit_wait: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            handshake: HANDSHAKE_TIMEOUT,
            heartbeat: HEARTBEAT_INTERVAL,
            link_timeout: LINK_TIMEOUT,
            invite_lifetime: INVITE_LIFETIME,
            admit_wait: INVITE_LIFETIME,
        }
    }
}

/// Why the transport could not do something.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShareError {
    /// A socket or the system failed.
    Io(String),
    /// TLS could not be set up or failed.
    Tls(String),
    /// The host's certificate is not the one the invitation names: nothing was sent.
    Fingerprint,
    /// Not an invitation this version reads.
    Invitation,
    /// A display name the protocol does not allow.
    Name,
    /// An e-mail address the protocol does not allow.
    Email,
    /// An address the host will not listen on.
    Address(String),
    /// The other side took too long.
    TimedOut,
    /// Sharing has stopped.
    Stopped,
}

impl fmt::Display for ShareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::Tls(message) | Self::Address(message) => {
                formatter.write_str(message)
            }
            Self::Fingerprint => {
                formatter.write_str("the host is not the one the invitation names")
            }
            Self::Invitation => formatter.write_str("not an invitation"),
            Self::Name => formatter.write_str("that name cannot be used"),
            Self::Email => formatter.write_str("that is not an e-mail address"),
            Self::TimedOut => formatter.write_str("the host took too long to answer"),
            Self::Stopped => formatter.write_str("sharing has stopped"),
        }
    }
}

impl std::error::Error for ShareError {}
