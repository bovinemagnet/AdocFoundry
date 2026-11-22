//! The transport over loopback TLS (COLLAB 4/8, #691; PRD §10 and design §12): engines meet
//! through a real listener, and raw peers that break the protocol on purpose are refused before
//! anything about the workspace reaches them. Every wait here is bounded.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::protocol::{decode, frame_length};
use adoc_engine::{
    commands::ids,
    protocol::{Credential, ErrorCode, Message, PeerId, Role, Secret, MAX_FRAME, PROTOCOL_VERSION},
    CommandArgs, DocumentId, Engine, Settings,
};
use adoc_share::{
    frames::{write_frame, write_message, ReadError, HANDSHAKE_FRAME},
    tls, Host, HostIdentity, Invitation, ShareError, Timings,
};
use tokio::{io::AsyncReadExt, net::TcpStream, runtime::Runtime, time::timeout};
use tokio_rustls::{client::TlsStream, TlsAcceptor, TlsConnector};

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

fn timings() -> Timings {
    Timings {
        handshake: Duration::from_secs(2),
        heartbeat: Duration::from_millis(200),
        link_timeout: Duration::from_secs(3),
        invite_lifetime: Duration::from_secs(60),
        admit_wait: Duration::from_secs(5),
    }
}

fn engine() -> Engine {
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    Engine::new(settings)
}

/// A host engine sharing `page.adoc` over a listener on loopback.
struct Hosting {
    _directory: tempfile::TempDir,
    host: Host,
    engine: Engine,
}

fn hosting(text: &str, timings: Timings) -> Hosting {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("page.adoc"), text).unwrap();
    let (host, link) = Host::start(LOOPBACK, "0.0.0", timings, None).unwrap();
    let mut engine = engine();
    engine
        .host_session(
            Box::new(link),
            directory.path(),
            &[PathBuf::from("page.adoc")],
            "Host",
        )
        .unwrap();
    Hosting {
        _directory: directory,
        host,
        engine,
    }
}

/// Pumps the engines until `done`, for at most ten seconds.
fn until(engines: &mut [&mut Engine], mut done: impl FnMut(&[&mut Engine]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for engine in engines.iter_mut() {
            engine.pump();
        }
        if done(engines) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting");
        std::thread::sleep(Duration::from_millis(5));
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

/// A peer that speaks the protocol by hand. It reads into a buffer of its own, since waiting
/// with a timeout must not lose part of a frame.
struct Raw {
    runtime: Runtime,
    stream: TlsStream<TcpStream>,
    buffer: Vec<u8>,
}

impl Raw {
    fn connect(invitation: &Invitation) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let stream = runtime.block_on(async {
            let tcp = TcpStream::connect(invitation.address).await.unwrap();
            TlsConnector::from(tls::client_config(invitation.fingerprint).unwrap())
                .connect(
                    rustls::pki_types::ServerName::try_from("adocfoundry.invalid").unwrap(),
                    tcp,
                )
                .await
                .unwrap()
        });
        Self {
            runtime,
            stream,
            buffer: Vec::new(),
        }
    }

    fn send(&mut self, message: &Message) {
        let _ = self
            .runtime
            .block_on(write_message(&mut self.stream, message));
    }

    fn send_bytes(&mut self, bytes: &[u8]) {
        let _ = self.runtime.block_on(write_frame(&mut self.stream, bytes));
    }

    /// The next message, `None` if nothing whole came within `wait`.
    fn recv(&mut self, wait: Duration) -> Option<Result<Message, ReadError>> {
        let deadline = Instant::now() + wait;
        loop {
            if self.buffer.len() >= 4 {
                let prefix: [u8; 4] = self.buffer[..4].try_into().unwrap();
                let length = match frame_length(prefix) {
                    Ok(length) => length,
                    Err(error) => return Some(Err(ReadError::Frame(error))),
                };
                if self.buffer.len() >= 4 + length {
                    let frame: Vec<u8> = self.buffer.drain(..4 + length).collect();
                    return Some(decode(&frame[4..]).map_err(ReadError::Frame));
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            let mut chunk = vec![0; 64 << 10];
            let stream = &mut self.stream;
            let read = self
                .runtime
                .block_on(async { timeout(left, AsyncReadExt::read(stream, &mut chunk)).await });
            match read {
                Err(_) => return None,
                Ok(Ok(0) | Err(_)) => return Some(Err(ReadError::Closed)),
                Ok(Ok(count)) => self.buffer.extend_from_slice(&chunk[..count]),
            }
        }
    }

    /// The next message other than a heartbeat.
    fn next(&mut self) -> Result<Message, ReadError> {
        loop {
            match self.recv(Duration::from_secs(5)).expect("nothing arrived") {
                Ok(Message::Heartbeat) => continue,
                other => return other,
            }
        }
    }

    fn hello(&mut self) {
        self.send(&Message::Hello {
            min: PROTOCOL_VERSION,
            max: PROTOCOL_VERSION,
            app: "raw".to_owned(),
        });
        assert!(matches!(self.next(), Ok(Message::Hello { .. })));
    }

    fn authenticate(&mut self, credential: Credential) {
        self.send(&Message::Authenticate {
            credential,
            name: "Raw".to_owned(),
            email: None,
        });
    }

    /// Whether the host closed the connection within a few seconds.
    fn closed(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.recv(Duration::from_millis(200)) {
                Some(Err(ReadError::Closed)) => return true,
                Some(Ok(_) | Err(ReadError::Frame(_))) | None => {}
            }
        }
        false
    }

    /// Whether the host closed the connection with an `Error` of `code`.
    fn refused_with(&mut self, code: ErrorCode) -> bool {
        loop {
            match self.next() {
                Ok(Message::Error { code: given, .. }) => return given == code && self.closed(),
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    }
}

#[test]
fn collab_loopback_session_syncs() {
    let mut hosting = hosting("= Page\n", timings());
    let invitation = hosting.host.invite().unwrap();
    assert_eq!(
        invitation.to_string().parse::<Invitation>().unwrap(),
        invitation
    );

    let connecting = adoc_share::connect(&invitation, timings()).unwrap();
    assert_eq!(connecting.safety_code(), hosting.host.safety_code());
    let (link, control) = connecting.authenticate("Ada", None, "0.0.0", None).unwrap();
    let mut guest = engine();
    guest.join_session(Box::new(link)).unwrap();

    let host = &mut hosting.engine;
    until(&mut [host, &mut guest], |engines| {
        !engines[0].session_knocks().is_empty()
    });
    let (peer, name) = host.session_knocks()[0].clone();
    assert_eq!(name, "Ada");
    host.admit_peer(peer).unwrap();
    host.set_peer_role(peer, Role::Editor).unwrap();
    until(&mut [host, &mut guest], |engines| {
        engines[1].session_manifest().len() == 1
    });
    assert_eq!(guest.session_manifest()[0].path, "page.adoc");
    guest.open_session_file(1).unwrap();
    until(&mut [host, &mut guest], |engines| {
        engines[1].session_document(1).is_some()
    });
    let host_doc = host.session_document(1).unwrap();
    let guest_doc = guest.session_document(1).unwrap();
    until(&mut [host, &mut guest], |engines| {
        engines[1]
            .session_me()
            .is_some_and(|(_, role)| role == Role::Editor)
    });

    type_at(host, host_doc, 7, "From the host.\n");
    type_at(&mut guest, guest_doc, 0, "// ");
    until(&mut [host, &mut guest], |engines| {
        text(engines[0], host_doc) == text(engines[1], guest_doc)
            && text(engines[0], host_doc).contains("From the host")
            && text(engines[0], host_doc).starts_with("// ")
    });

    // The link drops; the host edits while the guest is away; the guest resumes in the same
    // epoch with its token, and both converge.
    control.drop_connection();
    until(&mut [host, &mut guest], |engines| {
        !engines[1].session_connected()
            && engines[0]
                .participants()
                .iter()
                .all(|participant| !participant.connected)
    });
    type_at(host, host_doc, 0, "Away. ");
    control.resume();
    until(&mut [host, &mut guest], |engines| {
        engines[1].session_connected() && text(engines[1], guest_doc) == text(engines[0], host_doc)
    });
    until(&mut [host, &mut guest], |engines| {
        engines[1]
            .session_me()
            .is_some_and(|(_, role)| role == Role::Editor)
    });
    // Editing resumes once the resynchronisation is done.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        host.pump();
        guest.pump();
        let typed = guest.invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: guest_doc,
                range: SourceRange::new(0, 0),
                text: "Back. ".to_owned(),
            },
        );
        if typed.is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the guest never could edit again"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    until(&mut [host, &mut guest], |engines| {
        text(engines[0], host_doc) == text(engines[1], guest_doc)
            && text(engines[0], host_doc).starts_with("Back. Away. // ")
    });
    assert_eq!(host.shared_file(host_doc).unwrap().1, 1);
    assert_eq!(guest.shared_file(guest_doc).unwrap().1, 1);
    drop(control);
}

#[test]
fn collab_wrong_fingerprint_sends_nothing() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let impostor = HostIdentity::generate().unwrap();
    let acceptor = TlsAcceptor::from(tls::server_config(&impostor).unwrap());
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(LOOPBACK))
        .unwrap();
    let address = listener.local_addr().unwrap();
    // The server counts what it receives once TLS is up, until the client goes.
    let received = std::thread::spawn(move || {
        runtime.block_on(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let Ok(Ok(mut stream)) = timeout(Duration::from_secs(5), acceptor.accept(tcp)).await
            else {
                return 0;
            };
            let mut buffer = [0; 1024];
            let mut total = 0;
            while let Ok(Ok(read)) = timeout(
                Duration::from_secs(2),
                tokio::io::AsyncReadExt::read(&mut stream, &mut buffer),
            )
            .await
            {
                if read == 0 {
                    break;
                }
                total += read;
            }
            total
        })
    });
    let invited = HostIdentity::generate().unwrap();
    let invitation = Invitation {
        address,
        fingerprint: invited.fingerprint(),
        secret: Secret([7; 32]),
    };
    assert_eq!(
        adoc_share::connect(&invitation, timings()).unwrap_err(),
        ShareError::Fingerprint
    );
    assert_eq!(received.join().unwrap(), 0);
}

#[test]
fn collab_unauthorised_requests_are_refused() {
    let mut hosting = hosting("= Secret\n", timings());
    let invitation = hosting.host.invite().unwrap();

    // A secret the host never gave.
    let mut raw = Raw::connect(&invitation);
    raw.hello();
    raw.authenticate(Credential::Invitation(Secret([9; 32])));
    assert!(raw.refused_with(ErrorCode::Denied));
    // A resume token nobody holds.
    let mut raw = Raw::connect(&invitation);
    raw.hello();
    raw.authenticate(Credential::Resume(Secret([9; 32])));
    assert!(raw.refused_with(ErrorCode::Denied));
    // A message before the handshake.
    let mut raw = Raw::connect(&invitation);
    raw.send(&Message::OpenFile { file: 1 });
    assert!(raw.closed());

    // The invitation, used once, then again.
    let mut first = Raw::connect(&invitation);
    first.hello();
    first.authenticate(Credential::Invitation(invitation.secret.clone()));
    let mut again = Raw::connect(&invitation);
    again.hello();
    again.authenticate(Credential::Invitation(invitation.secret.clone()));
    assert!(again.refused_with(ErrorCode::Denied));
    until(&mut [&mut hosting.engine], |engines| {
        engines[0].session_knocks().len() == 1
    });

    // An expired invitation.
    let mut quick = hosting_with(Timings {
        invite_lifetime: Duration::from_millis(50),
        ..timings()
    });
    let stale = quick.host.invite().unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let mut late = Raw::connect(&stale);
    late.hello();
    late.authenticate(Credential::Invitation(stale.secret.clone()));
    assert!(late.refused_with(ErrorCode::Denied));
    quick.engine.pump();
    assert!(quick.engine.session_knocks().is_empty());
    drop(first);
}

/// The next message for `raw` other than a heartbeat or the roster, which the host sends
/// whenever who is in the session changes, pumping the host's engine meanwhile.
fn answer(hosting: &mut Hosting, raw: &mut Raw) -> Message {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        hosting.engine.pump();
        match raw.recv(Duration::from_millis(20)) {
            Some(Ok(Message::Heartbeat | Message::Roster { .. })) | None => {}
            Some(Ok(message)) => return message,
            Some(Err(error)) => panic!("{error:?}"),
        }
    }
    panic!("no answer");
}

fn hosting_with(timings: Timings) -> Hosting {
    hosting("= Page\n", timings)
}

/// A raw peer that showed a valid invitation, admitted by the host engine.
fn admitted(hosting: &mut Hosting) -> (Raw, PeerId) {
    let invitation = hosting.host.invite().unwrap();
    let mut raw = Raw::connect(&invitation);
    raw.hello();
    raw.authenticate(Credential::Invitation(invitation.secret));
    until(&mut [&mut hosting.engine], |engines| {
        !engines[0].session_knocks().is_empty()
    });
    let peer = hosting.engine.session_knocks()[0].0;
    hosting.engine.admit_peer(peer).unwrap();
    assert!(
        matches!(raw.next(), Ok(Message::Join { peer: joined, role: Role::Viewer, .. }) if joined == peer)
    );
    assert!(matches!(raw.next(), Ok(Message::Manifest { .. })));
    // A page outside any Antora component: no component is named.
    assert!(matches!(
        raw.next(),
        Ok(Message::Components { components }) if components.is_empty()
    ));
    (raw, peer)
}

#[test]
fn collab_nothing_is_sent_before_admit() {
    let mut hosting = hosting("= Secret\n", timings());
    let invitation = hosting.host.invite().unwrap();
    let mut raw = Raw::connect(&invitation);
    raw.hello();
    raw.authenticate(Credential::Invitation(invitation.secret));
    until(&mut [&mut hosting.engine], |engines| {
        !engines[0].session_knocks().is_empty()
    });
    // Nothing but heartbeats? Not even those: the connection is silent until Admit.
    assert!(raw.recv(Duration::from_millis(500)).is_none());
    let peer = hosting.engine.session_knocks()[0].0;
    hosting.engine.remove_peer(peer);
    assert!(raw.refused_with(ErrorCode::Denied));

    // A viewer's update reaches nothing; a file the manifest never offered closes it.
    let (mut raw, _) = admitted(&mut hosting);
    raw.send(&Message::OpenFile { file: 1 });
    assert!(matches!(
        answer(&mut hosting, &mut raw),
        Message::Snapshot { .. }
    ));
    // The file's state follows every snapshot.
    assert!(matches!(
        answer(&mut hosting, &mut raw),
        Message::FileState {
            conflict: false,
            ..
        }
    ));
    raw.send(&Message::OpenFile { file: 77 });
    assert!(matches!(
        answer(&mut hosting, &mut raw),
        Message::Error {
            code: ErrorCode::Protocol,
            ..
        }
    ));
    assert!(raw.closed());
}

#[test]
fn a_knock_not_answered_in_time_is_turned_away() {
    let mut hosting = hosting_with(Timings {
        admit_wait: Duration::from_millis(200),
        ..timings()
    });
    let invitation = hosting.host.invite().unwrap();
    let mut raw = Raw::connect(&invitation);
    raw.hello();
    raw.authenticate(Credential::Invitation(invitation.secret));
    assert!(raw.refused_with(ErrorCode::Denied));
    until(&mut [&mut hosting.engine], |engines| {
        engines[0].session_knocks().is_empty()
    });
}

#[test]
fn collab_frame_limits_close_the_connection() {
    let mut hosting = hosting("= Page\n", timings());
    let invitation = hosting.host.invite().unwrap();

    // Before authenticating, a frame over the handshake's limit.
    let mut raw = Raw::connect(&invitation);
    raw.send_bytes(&u32::try_from(HANDSHAKE_FRAME + 1).unwrap().to_be_bytes());
    assert!(raw.closed());

    // After Admit, a length over the protocol's limit, then a malformed frame.
    let (mut raw, peer) = admitted(&mut hosting);
    raw.send_bytes(&u32::try_from(MAX_FRAME + 1).unwrap().to_be_bytes());
    assert!(raw.closed());
    until(&mut [&mut hosting.engine], |engines| {
        engines[0]
            .participants()
            .iter()
            .any(|participant| participant.peer == peer && !participant.connected)
    });
    let (mut raw, _) = admitted(&mut hosting);
    raw.send_bytes(&[0, 0, 0, 5, 99, 0, 2, b'{', b'}']);
    assert!(raw.closed());

    // Silence: no heartbeat within the link's timeout.
    let mut hosting = hosting_with(Timings {
        link_timeout: Duration::from_millis(500),
        ..timings()
    });
    let (mut raw, _) = admitted(&mut hosting);
    assert!(raw.closed());

    // A connection that never says anything.
    let mut hosting = hosting_with(Timings {
        handshake: Duration::from_millis(300),
        ..timings()
    });
    let invitation = hosting.host.invite().unwrap();
    let mut raw = Raw::connect(&invitation);
    assert!(raw.closed());
    hosting.engine.pump();
}

#[test]
fn collab_slow_peer_never_blocks_host() {
    let mut hosting = hosting("= Page\n", timings());
    let (mut raw, peer) = admitted(&mut hosting);
    raw.send(&Message::OpenFile { file: 1 });
    assert!(matches!(
        answer(&mut hosting, &mut raw),
        Message::Snapshot { .. }
    ));
    // From now on the peer reads nothing, and the host keeps replacing a large block.
    let id = hosting.engine.session_document(1).unwrap();
    let block = 256 << 10;
    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut round = 0;
    loop {
        let filler = char::from(b'a' + (round % 26) as u8)
            .to_string()
            .repeat(block);
        let end = hosting.engine.document(id).unwrap().text.len();
        let edit = Instant::now();
        hosting
            .engine
            .invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(7, end),
                    text: filler,
                },
            )
            .unwrap();
        hosting.engine.pump();
        slowest = slowest.max(edit.elapsed());
        round += 1;
        let gone = hosting
            .engine
            .participants()
            .iter()
            .all(|participant| participant.peer != peer || !participant.connected);
        if gone {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the slow peer was never disconnected after {round} rounds"
        );
    }
    // It was the queue's limit that closed it: the bytes queued passed it, past what the
    // sockets' buffers took.
    assert!(
        round * block >= adoc_engine::protocol::MAX_QUEUED_BYTES,
        "closed after {round} rounds"
    );
    // The host's edits never waited on the network.
    assert!(slowest < Duration::from_secs(1), "an edit took {slowest:?}");
    // Reading at last, the peer finds what was queued before the queue overflowed, and the
    // end of the connection: told why if its full socket took the farewell in time.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "the connection stayed open");
        match raw.recv(Duration::from_secs(5)) {
            Some(Ok(Message::Error { code, .. })) => assert_eq!(code, ErrorCode::Slow),
            Some(Ok(_)) => {}
            Some(Err(_)) => break,
            None => panic!("the connection stayed open and silent"),
        }
    }
}

#[test]
fn stopping_closes_every_connection() {
    let mut hosting = hosting("= Page\n", timings());
    let invitation = hosting.host.invite().unwrap();
    let (link, control) = adoc_share::connect(&invitation, timings())
        .unwrap()
        .authenticate("Ada", None, "0.0.0", None)
        .unwrap();
    let mut guest = engine();
    guest.join_session(Box::new(link)).unwrap();
    until(&mut [&mut hosting.engine, &mut guest], |engines| {
        !engines[0].session_knocks().is_empty()
    });
    let peer = hosting.engine.session_knocks()[0].0;
    hosting.engine.admit_peer(peer).unwrap();
    until(&mut [&mut hosting.engine, &mut guest], |engines| {
        engines[1].session_manifest().len() == 1
    });
    hosting.host.stop();
    until(&mut [&mut guest], |engines| !engines[0].session_connected());
    assert!(!control.connected());
    assert!(hosting.host.invite().is_err());
    assert!(adoc_share::connect(&invitation, timings()).is_err());
}

#[test]
fn the_host_listens_only_where_told() {
    for everywhere in ["0.0.0.0:0", "[::]:0"] {
        assert!(matches!(
            Host::start(everywhere.parse().unwrap(), "0.0.0", timings(), None),
            Err(ShareError::Address(_))
        ));
    }
}

/// The knocks a host's link reports before an engine reads them, for at most ten seconds.
fn knock_on(link: &mut dyn adoc_engine::Link) -> (String, Option<String>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(adoc_engine::LinkEvent::Knock { name, email, .. }) = link.try_recv() {
            return (name, email);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no knock");
}

#[test]
fn the_host_is_told_the_guests_address() {
    for email in [Some("ada@example.org"), None] {
        let (mut host, mut link) = Host::start(LOOPBACK, "0.0.0", timings(), None).unwrap();
        let invitation = host.invite().unwrap();
        let (_guest, _control) = adoc_share::connect(&invitation, timings())
            .unwrap()
            .authenticate("Ada", email, "0.0.0", None)
            .unwrap();
        assert_eq!(
            knock_on(&mut link),
            ("Ada".to_owned(), email.map(str::to_owned))
        );
        host.stop();
    }
}

#[test]
fn the_host_says_it_speaks_version_two() {
    let hosting = hosting("= Page\n", timings());
    let invitation = hosting.host.invite().unwrap();
    let mut raw = Raw::connect(&invitation);
    raw.send(&Message::Hello {
        min: 1,
        max: 2,
        app: "raw".to_owned(),
    });
    assert!(matches!(
        raw.next(),
        Ok(Message::Hello { min: 2, max: 2, .. })
    ));
}

#[test]
fn an_invalid_address_is_refused_before_connecting() {
    let hosting = hosting("= Page\n", timings());
    let invitation = hosting.host.invite().unwrap();
    let refused = adoc_share::connect(&invitation, timings())
        .unwrap()
        .authenticate("Ada", Some("not an address"), "0.0.0", None);
    assert!(matches!(refused, Err(ShareError::Email)));
}
