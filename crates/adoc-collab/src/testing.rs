//! An in-memory link for tests (feature `test-support`): a host and its guests meet over a
//! [`Net`] that encodes and decodes every message as a frame, as the transport does, and lets a
//! test lose what is in flight, hold the host's reads, or send what a modified guest would.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use crate::{
    protocol::{decode, ErrorCode, Message, PeerId, Role, Secret, SessionId},
    Link, LinkError, LinkEvent,
};

/// Everything in flight between a host and its guests.
#[derive(Default)]
pub struct Net {
    pub to_host: VecDeque<LinkEvent>,
    pub to_guest: BTreeMap<PeerId, VecDeque<LinkEvent>>,
    /// Every message the host sent each peer, in order.
    pub sent: BTreeMap<PeerId, Vec<Message>>,
    pub removed: Vec<(PeerId, ErrorCode)>,
    pub admitted: Vec<PeerId>,
    /// While set, the host reads nothing: what guests send stays in flight.
    pub hold_host: bool,
}

/// `message` as the other side reads it: encoded as a frame and decoded again.
#[must_use]
pub fn wire(message: Message) -> Message {
    let frame = message.encode().unwrap();
    decode(&frame[4..]).unwrap()
}

/// The host's end of a [`Net`].
pub struct HostLink(pub Arc<Mutex<Net>>);

impl Link for HostLink {
    fn try_recv(&mut self) -> Option<LinkEvent> {
        let mut net = self.0.lock().unwrap();
        if net.hold_host {
            return None;
        }
        net.to_host.pop_front()
    }

    fn send(&mut self, peer: PeerId, message: Message) -> Result<(), LinkError> {
        let message = wire(message);
        let mut net = self.0.lock().unwrap();
        net.sent.entry(peer).or_default().push(message.clone());
        net.to_guest
            .entry(peer)
            .or_default()
            .push_back(LinkEvent::Message {
                peer: PeerId::HOST,
                message,
            });
        Ok(())
    }

    fn admit(&mut self, peer: PeerId) -> Result<(), LinkError> {
        let mut net = self.0.lock().unwrap();
        net.admitted.push(peer);
        net.to_guest.entry(peer).or_default().push_back(join(peer));
        Ok(())
    }

    fn remove(&mut self, peer: PeerId, code: ErrorCode, _: &str) {
        let mut net = self.0.lock().unwrap();
        net.removed.push((peer, code));
        net.to_guest
            .entry(peer)
            .or_default()
            .push_back(LinkEvent::Gone {
                peer: PeerId::HOST,
                reason: "removed".to_owned(),
            });
    }
}

/// The `Join` the transport sends a guest it lets in.
#[must_use]
pub fn join(peer: PeerId) -> LinkEvent {
    LinkEvent::Message {
        peer: PeerId::HOST,
        message: Message::Join {
            peer,
            role: Role::Viewer,
            session: SessionId([1; 16]),
            resume: Secret([2; 32]),
        },
    }
}

/// Guest `me`'s end of a [`Net`].
pub struct GuestLink {
    pub net: Arc<Mutex<Net>>,
    pub me: PeerId,
}

impl Link for GuestLink {
    fn try_recv(&mut self) -> Option<LinkEvent> {
        self.net
            .lock()
            .unwrap()
            .to_guest
            .get_mut(&self.me)?
            .pop_front()
    }

    fn send(&mut self, _: PeerId, message: Message) -> Result<(), LinkError> {
        let message = wire(message);
        self.net
            .lock()
            .unwrap()
            .to_host
            .push_back(LinkEvent::Message {
                peer: self.me,
                message,
            });
        Ok(())
    }

    fn admit(&mut self, _: PeerId) -> Result<(), LinkError> {
        Err(LinkError::Closed)
    }

    fn remove(&mut self, _: PeerId, _: ErrorCode, _: &str) {}
}
