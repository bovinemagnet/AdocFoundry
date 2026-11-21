//! The in-memory session the collaboration tests share: a host engine and guest engines over a
//! link that encodes and decodes every message as a frame, as the transport does, and can lose
//! what is in flight.

#![allow(dead_code)]

use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub use adoc_collab::testing::{join, wire, GuestLink, HostLink, Net};
use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids,
    protocol::{ErrorCode, Message, PeerId, Role},
    CommandArgs, DocumentId, Engine, LinkEvent, Settings,
};

pub fn engine() -> Engine {
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    Engine::new(settings)
}

pub struct Session {
    pub directory: tempfile::TempDir,
    pub net: Arc<Mutex<Net>>,
    pub host: Engine,
    pub guests: Vec<(PeerId, Engine)>,
}

impl Session {
    /// A host offering `pages` (name, text) from a folder of its own.
    pub fn new(pages: &[(&str, &str)]) -> Self {
        Self::with_host(pages, engine())
    }

    /// A host offering `pages`, as [`Session::new`], with an engine made by the caller.
    pub fn with_host(pages: &[(&str, &str)], mut host: Engine) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, text) in pages {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            files.push(PathBuf::from(name));
        }
        let net = Arc::new(Mutex::new(Net::default()));
        host.host_session(
            Box::new(HostLink(net.clone())),
            directory.path(),
            &files,
            "Host",
        )
        .unwrap();
        Self {
            directory,
            net,
            host,
            guests: Vec::new(),
        }
    }

    /// A guest knocking as `peer`; not yet admitted.
    pub fn knock(&mut self, peer: u32) -> PeerId {
        self.knock_with(peer, engine())
    }

    /// A guest knocking as `peer` with an engine made by the caller.
    pub fn knock_with(&mut self, peer: u32, mut guest: Engine) -> PeerId {
        let peer = PeerId(peer);
        guest
            .join_session(Box::new(GuestLink {
                net: self.net.clone(),
                me: peer,
            }))
            .unwrap();
        self.guests.push((peer, guest));
        self.net
            .lock()
            .unwrap()
            .to_host
            .push_back(LinkEvent::Knock {
                peer,
                name: format!("guest {}", peer.0),
            });
        self.settle();
        peer
    }

    /// A guest admitted, made an Editor if `edit`, with file 1 open.
    pub fn guest(&mut self, peer: u32, edit: bool) -> PeerId {
        let peer = self.knock(peer);
        self.host.admit_peer(peer).unwrap();
        self.settle();
        if edit {
            self.host.set_peer_role(peer, Role::Editor).unwrap();
        }
        self.engine(peer).open_session_file(1).unwrap();
        self.settle();
        peer
    }

    pub fn engine(&mut self, peer: PeerId) -> &mut Engine {
        &mut self
            .guests
            .iter_mut()
            .find(|(id, _)| *id == peer)
            .unwrap()
            .1
    }

    /// Pumps everyone until nothing is left in flight and nothing is due. Work due at once
    /// (comments still to tell a guest, say) is due at the moment it was asked for, so the time
    /// is read after asking. A deadline by an engine's own manual clock, which pumping does not
    /// reach, stays "due" by the system's time: once nothing is in flight for a few rounds, the
    /// session is at rest whatever such a deadline says.
    pub fn settle(&mut self) {
        let mut quiet = 0;
        for _ in 0..1000 {
            self.host.pump();
            for (_, guest) in &mut self.guests {
                guest.pump();
            }
            let net = self.net.lock().unwrap();
            let idle = net.to_host.is_empty() && net.to_guest.values().all(VecDeque::is_empty);
            drop(net);
            let rested = |engine: &Engine| {
                engine
                    .next_due()
                    .is_none_or(|due| due > std::time::Instant::now())
            };
            let due = !rested(&self.host) || self.guests.iter().any(|(_, guest)| !rested(guest));
            quiet = if idle { quiet + 1 } else { 0 };
            if idle && (!due || quiet >= 3) {
                self.assert_no_host_paths();
                return;
            }
        }
        panic!("the session never settled");
    }

    /// Nothing the host sent any guest names a folder of the host's: not the shared folder,
    /// nor any path above it (design §6, `collab_scope_never_leaks`).
    pub fn assert_no_host_paths(&self) {
        let root = self.directory.path().to_string_lossy().into_owned();
        let real = std::fs::canonicalize(self.directory.path())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| root.clone());
        let net = self.net.lock().unwrap();
        for reason in &net.reasons {
            assert!(
                !reason.contains(&root) && !reason.contains(&real),
                "the host gave a path of its own as a reason: {reason}"
            );
        }
        for (peer, messages) in &net.sent {
            for message in messages {
                let shown = format!("{message:?}");
                assert!(
                    !shown.contains(&root) && !shown.contains(&real),
                    "the host sent {peer:?} a path of its own: {shown}"
                );
            }
        }
    }

    pub fn host_doc(&self) -> DocumentId {
        self.host.session_document(1).unwrap()
    }

    pub fn host_text(&self) -> String {
        text(&self.host, self.host_doc())
    }

    pub fn guest_text(&mut self, peer: PeerId) -> String {
        let engine = self.engine(peer);
        let id = engine.session_document(1).unwrap();
        text(engine, id)
    }

    /// Sends `message` from `peer` to the host as a modified guest would.
    pub fn forge(&mut self, peer: PeerId, message: Message) {
        self.net
            .lock()
            .unwrap()
            .to_host
            .push_back(LinkEvent::Message {
                peer,
                message: wire(message),
            });
    }

    pub fn removed(&self) -> Vec<(PeerId, ErrorCode)> {
        self.net.lock().unwrap().removed.clone()
    }

    pub fn sent_to(&self, peer: PeerId) -> Vec<Message> {
        self.net
            .lock()
            .unwrap()
            .sent
            .get(&peer)
            .cloned()
            .unwrap_or_default()
    }

    /// The `client` of the latest snapshot of `file` the host sent `peer`, which an update from
    /// it names as its base.
    pub fn base(&self, peer: PeerId, file: u64) -> u64 {
        self.sent_to(peer)
            .into_iter()
            .rev()
            .find_map(|message| match message {
                Message::Snapshot {
                    file: sent, client, ..
                } if sent == file => Some(client),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// The link to `peer` drops: what the host sent it and the last `lost` messages it sent
    /// are lost; both sides hear it went; until it resumes, whatever either sends the other is
    /// lost.
    pub fn drop_link(&mut self, peer: PeerId, lost: usize) {
        let mut net = self.net.lock().unwrap();
        net.down.insert(peer);
        net.to_guest.entry(peer).or_default().clear();
        let mut kept = VecDeque::new();
        let mut dropped = 0;
        while let Some(event) = net.to_host.pop_back() {
            let from_peer =
                matches!(&event, LinkEvent::Message { peer: from, .. } if *from == peer);
            if from_peer && dropped < lost {
                dropped += 1;
                continue;
            }
            kept.push_front(event);
        }
        net.to_host = kept;
        net.to_host.push_back(LinkEvent::Gone {
            peer,
            reason: "dropped".to_owned(),
        });
        net.to_guest
            .entry(peer)
            .or_default()
            .push_back(LinkEvent::Gone {
                peer: PeerId::HOST,
                reason: "dropped".to_owned(),
            });
    }

    /// The guest reconnects with its resume token: the transport sends it a new `Join`.
    pub fn resume(&mut self, peer: PeerId) {
        let mut net = self.net.lock().unwrap();
        net.down.remove(&peer);
        net.to_host.push_back(LinkEvent::Resumed { peer });
        let queue = net.to_guest.entry(peer).or_default();
        queue.push_back(LinkEvent::Resumed { peer: PeerId::HOST });
        queue.push_back(join(peer));
    }
}

pub fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text_string()
}

pub fn type_at(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(at, at),
                text: typed.to_owned(),
            },
        )
        .unwrap();
}

pub fn guest_types(session: &mut Session, peer: PeerId, at: usize, typed: &str) {
    let engine = session.engine(peer);
    let id = engine.session_document(1).unwrap();
    type_at(engine, id, at, typed);
}
