//! A collaboration session through the engine (COLLAB 4/8, #691; design §§2, 5, 7, 8, 11):
//! a host engine and guest engines meet over an in-memory link that encodes and decodes every
//! message as a frame, as the transport does, and can lose what is in flight. Guests are held to
//! what the host's session allows, whatever they send.

#![cfg(feature = "collab")]

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids,
    protocol::{decode, ErrorCode, Message, PeerId, Role, SaveOutcome},
    CommandArgs, DocumentId, EncodedUpdate, Engine, Link, LinkError, LinkEvent, ReplicaId,
    Settings, SESSION_EVENTS_PER_PUMP,
};

fn engine() -> Engine {
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    Engine::new(settings)
}

/// Everything in flight between a host and its guests.
#[derive(Default)]
struct Net {
    to_host: VecDeque<LinkEvent>,
    to_guest: BTreeMap<PeerId, VecDeque<LinkEvent>>,
    /// Every message the host sent each peer, in order.
    sent: BTreeMap<PeerId, Vec<Message>>,
    removed: Vec<(PeerId, ErrorCode)>,
    admitted: Vec<PeerId>,
    /// While set, the host reads nothing: what guests send stays in flight.
    hold_host: bool,
}

fn wire(message: Message) -> Message {
    let frame = message.encode().unwrap();
    decode(&frame[4..]).unwrap()
}

struct HostLink(Arc<Mutex<Net>>);

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

fn join(peer: PeerId) -> LinkEvent {
    LinkEvent::Message {
        peer: PeerId::HOST,
        message: Message::Join {
            peer,
            role: Role::Viewer,
            session: adoc_engine::protocol::SessionId([1; 16]),
            resume: adoc_engine::protocol::Secret([2; 32]),
        },
    }
}

struct GuestLink {
    net: Arc<Mutex<Net>>,
    me: PeerId,
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

struct Session {
    directory: tempfile::TempDir,
    net: Arc<Mutex<Net>>,
    host: Engine,
    guests: Vec<(PeerId, Engine)>,
}

impl Session {
    /// A host offering `pages` (name, text) from a folder of its own.
    fn new(pages: &[(&str, &str)]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, text) in pages {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            files.push(PathBuf::from(name));
        }
        let net = Arc::new(Mutex::new(Net::default()));
        let mut host = engine();
        host.host_session(Box::new(HostLink(net.clone())), directory.path(), &files)
            .unwrap();
        Self {
            directory,
            net,
            host,
            guests: Vec::new(),
        }
    }

    /// A guest knocking as `peer`; not yet admitted.
    fn knock(&mut self, peer: u32) -> PeerId {
        let peer = PeerId(peer);
        let mut guest = engine();
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
    fn guest(&mut self, peer: u32, edit: bool) -> PeerId {
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

    fn engine(&mut self, peer: PeerId) -> &mut Engine {
        &mut self
            .guests
            .iter_mut()
            .find(|(id, _)| *id == peer)
            .unwrap()
            .1
    }

    /// Pumps everyone until nothing is left in flight.
    fn settle(&mut self) {
        for _ in 0..1000 {
            self.host.pump();
            for (_, guest) in &mut self.guests {
                guest.pump();
            }
            let net = self.net.lock().unwrap();
            let idle = net.to_host.is_empty() && net.to_guest.values().all(VecDeque::is_empty);
            drop(net);
            if idle
                && self
                    .host
                    .next_due()
                    .is_none_or(|due| due > std::time::Instant::now())
            {
                return;
            }
        }
        panic!("the session never settled");
    }

    fn host_doc(&self) -> DocumentId {
        self.host.session_document(1).unwrap()
    }

    fn host_text(&self) -> String {
        text(&self.host, self.host_doc())
    }

    fn guest_text(&mut self, peer: PeerId) -> String {
        let engine = self.engine(peer);
        let id = engine.session_document(1).unwrap();
        text(engine, id)
    }

    /// Sends `message` from `peer` to the host as a modified guest would.
    fn forge(&mut self, peer: PeerId, message: Message) {
        self.net
            .lock()
            .unwrap()
            .to_host
            .push_back(LinkEvent::Message {
                peer,
                message: wire(message),
            });
    }

    fn removed(&self) -> Vec<(PeerId, ErrorCode)> {
        self.net.lock().unwrap().removed.clone()
    }

    fn sent_to(&self, peer: PeerId) -> Vec<Message> {
        self.net
            .lock()
            .unwrap()
            .sent
            .get(&peer)
            .cloned()
            .unwrap_or_default()
    }

    /// The link to `peer` drops: what the host sent it and the last `lost` messages it sent
    /// are lost; both sides hear it went.
    fn drop_link(&mut self, peer: PeerId, lost: usize) {
        let mut net = self.net.lock().unwrap();
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
    fn resume(&mut self, peer: PeerId) {
        let mut net = self.net.lock().unwrap();
        net.to_host.push_back(LinkEvent::Resumed { peer });
        let queue = net.to_guest.entry(peer).or_default();
        queue.push_back(LinkEvent::Resumed { peer: PeerId::HOST });
        queue.push_back(join(peer));
    }
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text_string()
}

fn type_at(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) {
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

fn guest_types(session: &mut Session, peer: PeerId, at: usize, typed: &str) {
    let engine = session.engine(peer);
    let id = engine.session_document(1).unwrap();
    type_at(engine, id, at, typed);
}

#[test]
fn collab_engines_meet_through_a_session() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    assert_eq!(session.guest_text(ada), "= Page\n");

    let id = session.host_doc();
    type_at(&mut session.host, id, 7, "Host.\n");
    session.settle();
    guest_types(&mut session, ada, 0, "// ");
    session.settle();
    assert_eq!(session.host_text(), "// = Page\nHost.\n");
    assert_eq!(session.guest_text(ada), session.host_text());
    assert!(session.removed().is_empty());
}

#[test]
fn two_guests_converge_through_the_host() {
    let mut session = Session::new(&[("page.adoc", "abc\n")]);
    let ada = session.guest(1, true);
    let bob = session.guest(2, true);
    for round in 0..20 {
        guest_types(&mut session, ada, 0, "a");
        guest_types(&mut session, bob, 1, "b");
        let id = session.host_doc();
        let end = session.host_text().len();
        type_at(&mut session.host, id, end, &round.to_string());
        if round % 3 == 0 {
            session.settle();
        }
    }
    session.settle();
    let host = session.host_text();
    assert_eq!(session.guest_text(ada), host);
    assert_eq!(session.guest_text(bob), host);
    assert_eq!(host.matches('a').count(), 21);
    assert_eq!(host.matches('b').count(), 21);
    assert!(session.removed().is_empty());
}

#[test]
fn collab_nothing_is_sent_before_admit() {
    let mut session = Session::new(&[("page.adoc", "= Secret\n")]);
    let peer = session.knock(1);
    assert_eq!(
        session.host.session_knocks(),
        [(peer, "guest 1".to_owned())]
    );
    assert!(session.sent_to(peer).is_empty());
    assert!(session.net.lock().unwrap().admitted.is_empty());
    // A message from a connection not yet admitted is not read.
    session.forge(peer, Message::OpenFile { file: 1 });
    session.settle();
    assert!(session.sent_to(peer).is_empty());

    session.host.remove_peer(peer);
    session.settle();
    assert!(session.sent_to(peer).is_empty());
    assert_eq!(session.removed(), [(peer, ErrorCode::Denied)]);
    assert!(session.host.participants().is_empty());
}

#[test]
fn collab_unauthorised_requests_are_refused() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let viewer = session.guest(1, false);
    let before = session.host_text();

    // A Viewer's guest engine refuses to edit; a modified one sends an update anyway.
    let engine = session.engine(viewer);
    let id = engine.session_document(1).unwrap();
    assert!(engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .is_err());
    let Some(Message::Snapshot { client, update, .. }) = session
        .sent_to(viewer)
        .into_iter()
        .find(|message| matches!(message, Message::Snapshot { .. }))
    else {
        panic!("no snapshot");
    };
    let mut forged =
        adoc_collab::Replica::from_snapshot(&update, ReplicaId::new(client).unwrap()).unwrap();
    let change = forged
        .apply_local(&[adoc_collab::ByteEdit::new(0, 0, "PWNED ")])
        .unwrap();
    session.forge(
        viewer,
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 1,
            update: change.outbound.unwrap(),
        },
    );
    session.settle();
    assert_eq!(session.host_text(), before);
    assert!(session.sent_to(viewer).iter().any(|message| matches!(
        message,
        Message::Error {
            code: ErrorCode::Unauthorised,
            ..
        }
    )));
    // The guest started again from the host's text.
    assert_eq!(session.guest_text(viewer), before);

    // A Viewer may not save.
    session.forge(
        viewer,
        Message::SaveRequest {
            file: 1,
            epoch: 1,
            request: 9,
        },
    );
    session.settle();
    assert!(session.sent_to(viewer).contains(&Message::SaveResult {
        request: 9,
        outcome: SaveOutcome::PermissionDenied,
    }));
    assert!(session.removed().is_empty());

    // A file id the manifest never offered closes the connection.
    session.forge(viewer, Message::OpenFile { file: 99 });
    session.settle();
    assert_eq!(session.removed(), [(viewer, ErrorCode::Protocol)]);
    assert!(session.host.participants().is_empty());
}

#[test]
fn messages_only_the_host_sends_close_the_connection() {
    for message in [
        Message::Join {
            peer: PeerId(5),
            role: Role::Editor,
            session: adoc_engine::protocol::SessionId([0; 16]),
            resume: adoc_engine::protocol::Secret([0; 32]),
        },
        Message::RoleChanged {
            peer: PeerId(1),
            role: Role::Editor,
        },
        Message::UpdateAck {
            file: 1,
            epoch: 1,
            seq: 1,
        },
        Message::Update {
            file: 2,
            epoch: 1,
            seq: 1,
            update: EncodedUpdate(vec![0]),
        },
    ] {
        let mut session = Session::new(&[("page.adoc", "= Page\n")]);
        let peer = session.guest(1, true);
        session.forge(peer, message.clone());
        session.settle();
        assert_eq!(
            session.removed(),
            [(peer, ErrorCode::Protocol)],
            "{message:?}"
        );
    }
}

#[test]
fn collab_forged_author_is_refused() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let before = session.host_text();
    let Some(Message::Snapshot { update, .. }) = session
        .sent_to(ada)
        .into_iter()
        .find(|message| matches!(message, Message::Snapshot { .. }))
    else {
        panic!("no snapshot");
    };
    // Items written under a client id the host never gave this guest.
    let mut forged =
        adoc_collab::Replica::from_snapshot(&update, ReplicaId::new(424_242).unwrap()).unwrap();
    let change = forged
        .apply_local(&[adoc_collab::ByteEdit::new(0, 0, "forged ")])
        .unwrap();
    session.forge(
        ada,
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 1,
            update: change.outbound.unwrap(),
        },
    );
    session.settle();
    assert_eq!(session.host_text(), before);
    assert_eq!(session.removed(), [(ada, ErrorCode::Protocol)]);
}

#[test]
fn updates_out_of_order_close_the_connection() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 0, "a");
    session.settle();
    let before = session.host_text();
    // The next update must be number 2.
    session.forge(
        ada,
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 7,
            update: EncodedUpdate(vec![0]),
        },
    );
    session.settle();
    assert_eq!(session.host_text(), before);
    assert_eq!(session.removed(), [(ada, ErrorCode::Protocol)]);
}

#[test]
fn a_role_taken_away_while_edits_are_in_flight() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let before = session.host_text();
    // Typed and sent, but not yet read by the host when it takes editing away.
    guest_types(&mut session, ada, 0, "a");
    guest_types(&mut session, ada, 1, "b");
    session.engine(ada).pump();
    session.host.set_peer_role(ada, Role::Viewer).unwrap();
    session.settle();
    assert_eq!(session.host_text(), before);
    assert_eq!(session.guest_text(ada), before);
    assert!(session.removed().is_empty());

    // Given editing back, the guest's edits count again.
    session.host.set_peer_role(ada, Role::Editor).unwrap();
    session.settle();
    guest_types(&mut session, ada, 0, "c");
    session.settle();
    assert_eq!(session.host_text(), format!("c{before}"));
    assert_eq!(session.guest_text(ada), session.host_text());
}

#[test]
fn a_reconnected_guest_delivers_each_edit_once() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 7, "one ");
    session.settle();
    // Three more edits are sent; the link drops with the last two still in flight, and the
    // host's own edit never reaches the guest.
    guest_types(&mut session, ada, 11, "two ");
    guest_types(&mut session, ada, 15, "three ");
    guest_types(&mut session, ada, 21, "four ");
    session.engine(ada).pump();
    assert_eq!(session.net.lock().unwrap().to_host.len(), 3);
    session.net.lock().unwrap().hold_host = true;
    let id = session.host_doc();
    type_at(&mut session.host, id, 0, "// ");
    session.host.pump();
    session.drop_link(ada, 2);
    session.net.lock().unwrap().hold_host = false;
    session.settle();
    assert!(!session.engine(ada).session_connected());
    // The host has "two" only; the guest never had the host's "// ".
    assert_eq!(session.host_text(), "// = Page\none two ");
    assert_eq!(session.guest_text(ada), "= Page\none two three four ");
    // A disconnected guest cannot edit.
    let engine = session.engine(ada);
    let doc = engine.session_document(1).unwrap();
    assert!(engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: doc,
                range: SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .is_err());

    session.resume(ada);
    session.settle();
    let host = session.host_text();
    assert_eq!(host, "// = Page\none two three four ");
    assert_eq!(session.guest_text(ada), host);
    assert!(session.engine(ada).session_connected());
    assert!(session.removed().is_empty());

    // Editing works again after the resynchronisation.
    guest_types(&mut session, ada, 0, "!");
    session.settle();
    assert_eq!(session.host_text(), format!("!{host}"));
}

#[test]
fn a_file_closed_by_the_host_is_shared_again_in_a_new_epoch() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let id = session.host_doc();
    session.host.close(id).unwrap();
    guest_types(&mut session, ada, 0, "x");
    session.settle();
    // The update of the old epoch is spent; both start again from the host's file.
    assert_eq!(session.host_text(), "= Page\n");
    assert_eq!(session.guest_text(ada), "= Page\n");
    assert!(session.removed().is_empty());
    guest_types(&mut session, ada, 0, "y");
    session.settle();
    assert_eq!(session.host_text(), "y= Page\n");
}

#[test]
fn collab_manifest_offers_only_what_may_be_shared() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    let outside = directory.path().join("outside.adoc");
    fs::write(&outside, "= Outside\n").unwrap();
    for (name, text) in [
        ("modules/ROOT/pages/index.adoc", "= Index\n"),
        ("modules/ROOT/partials/part.adoc", "Part.\n"),
        (".git/config.adoc", "x"),
        ("modules/ROOT/pages/.draft.adoc", "x"),
        ("node_modules/pkg/readme.adoc", "x"),
        ("modules/ROOT/pages/notes.txt", "x"),
    ] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("modules/ROOT/pages/link.adoc")).unwrap();

    let offer = |files: &[&str]| {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        let net = Arc::new(Mutex::new(Net::default()));
        engine().host_session(Box::new(HostLink(net)), &root, &files)
    };
    let manifest = offer(&[
        "modules/ROOT/pages/index.adoc",
        "modules/ROOT/partials/part.adoc",
    ])
    .unwrap();
    let shown: Vec<(&str, adoc_engine::protocol::FileKind)> = manifest
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect();
    assert_eq!(
        shown,
        [
            (
                "modules/ROOT/pages/index.adoc",
                adoc_engine::protocol::FileKind::Page
            ),
            (
                "modules/ROOT/partials/part.adoc",
                adoc_engine::protocol::FileKind::Partial
            ),
        ]
    );
    let mut refused = vec![
        ".git/config.adoc",
        "modules/ROOT/pages/.draft.adoc",
        "node_modules/pkg/readme.adoc",
        "modules/ROOT/pages/notes.txt",
        "modules/ROOT/pages/missing.adoc",
        "../outside.adoc",
        "modules/../../outside.adoc",
    ];
    let absolute = outside.display().to_string();
    refused.push(&absolute);
    #[cfg(unix)]
    refused.push("modules/ROOT/pages/link.adoc");
    for file in refused {
        assert!(offer(&[file]).is_err(), "{file} was offered");
    }
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_link_after_sharing_is_not_opened() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let outside = session
        .directory
        .path()
        .parent()
        .unwrap()
        .join(format!("outside-{}.adoc", std::process::id()));
    fs::write(&outside, "= Secret\n").unwrap();
    let page = session.directory.path().join("page.adoc");
    fs::remove_file(&page).unwrap();
    std::os::unix::fs::symlink(&outside, &page).unwrap();
    let peer = session.knock(1);
    session.host.admit_peer(peer).unwrap();
    session.settle();
    session.engine(peer).open_session_file(1).unwrap();
    session.settle();
    fs::remove_file(&outside).unwrap();
    assert!(!session
        .sent_to(peer)
        .iter()
        .any(|message| matches!(message, Message::Snapshot { .. })));
    assert!(session.host.session_document(1).is_none());
    let _ = Path::new(&page);
}

#[test]
fn a_session_drains_a_bounded_number_of_events_a_pump() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    for _ in 0..(SESSION_EVENTS_PER_PUMP + 10) {
        guest_types(&mut session, ada, 0, "a");
        session.engine(ada).pump();
    }
    let waiting = session.net.lock().unwrap().to_host.len();
    assert!(waiting > SESSION_EVENTS_PER_PUMP);
    assert_eq!(session.host.pump(), SESSION_EVENTS_PER_PUMP);
    // More wait: the next pump is due at once.
    assert!(session
        .host
        .next_due()
        .is_some_and(|due| due <= std::time::Instant::now()));
    session.settle();
    assert_eq!(session.guest_text(ada), session.host_text());
    assert!(session
        .host_text()
        .starts_with(&"a".repeat(SESSION_EVENTS_PER_PUMP + 10)));
}
