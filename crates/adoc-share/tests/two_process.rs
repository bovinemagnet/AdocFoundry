//! The release suite across two processes (COLLAB 8/8, #695; PRD §10,
//! `collab_two_process_release_suite`): a host engine here and a guest engine in a process of
//! its own, meeting over loopback TLS with a pinned fingerprint, as two copies of the app do.
//! The guest is this test binary run again on `guest_process`, which does nothing unless told
//! to by its environment, since no shipped binary may carry collaboration but the app. The
//! invitation reaches it on its standard input, never its command line; it answers on standard
//! output, each answer a line beginning `@ `. Every wait is bounded, the guest is killed if the
//! test fails, and the host's listener is shown closed at the end.
//!
//! What runs here, and which tests hold the rest of the coverage matrix, is listed in
//! `docs/qa/collab.md`.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, protocol::Role, CommandArgs, DocumentId, Engine, Settings};
use adoc_share::{Host, Invitation, Timings};

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
/// How long any one step may take, well inside the ten minutes CI allows a test.
const STEP: Duration = Duration::from_secs(30);
/// Set in the guest's environment.
const GUEST: &str = "ADOC_SHARE_GUEST_PROCESS";

fn timings() -> Timings {
    Timings {
        handshake: Duration::from_secs(5),
        heartbeat: Duration::from_millis(200),
        link_timeout: Duration::from_secs(3),
        invite_lifetime: Duration::from_secs(120),
        admit_wait: Duration::from_secs(30),
    }
}

fn engine() -> Engine {
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    Engine::new(settings)
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text_string()
}

fn insert(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) -> bool {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(at, at),
                text: typed.to_owned(),
            },
        )
        .is_ok()
}

/// Text on one line of the pipe: its bytes in hexadecimal.
fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(hex: &str) -> String {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect();
    String::from_utf8(bytes).unwrap()
}

/// Tests that start a process take this, as the repository's process tests do (the `ETXTBSY`
/// workaround).
fn spawn_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: Mutex<()> = Mutex::new(());
    GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The guest process, killed if still running when dropped.
struct Guest {
    child: Child,
    input: ChildStdin,
    answers: Receiver<String>,
}

impl Guest {
    fn start(invitation: &Invitation) -> Self {
        let mut child = {
            let _guard = spawn_guard();
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "guest_process",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(GUEST, "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap()
        };
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (send, answers) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                // The test harness starts the guest's line with the test's name, unended.
                if let Some(at) = line.find("@ ") {
                    if send.send(line[at + 2..].to_owned()).is_err() {
                        break;
                    }
                }
            }
        });
        writeln!(input, "{invitation}").unwrap();
        Self {
            child,
            input,
            answers,
        }
    }

    fn send(&mut self, command: &str) {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
    }

    /// The guest's next answer, pumping the host meanwhile.
    fn answer(&mut self, host: &mut Engine) -> String {
        let deadline = Instant::now() + STEP;
        loop {
            host.pump();
            if let Ok(answer) = self.answers.recv_timeout(Duration::from_millis(5)) {
                return answer;
            }
            assert!(
                Instant::now() < deadline,
                "the guest did not answer in time"
            );
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "the guest process ended early"
            );
        }
    }

    fn ask(&mut self, host: &mut Engine, command: &str) -> String {
        self.send(command);
        self.answer(host)
    }

    /// Waits for the guest to end by itself, within a step.
    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + STEP;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the guest process did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Pumps `host` until `done`, within a step.
fn until(host: &mut Engine, mut done: impl FnMut(&Engine) -> bool) {
    let deadline = Instant::now() + STEP;
    while !done(host) {
        assert!(Instant::now() < deadline, "timed out waiting on the host");
        host.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn collab_two_process_release_suite() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("one.adoc"), "= One\n\nFirst.\n").unwrap();
    fs::write(directory.path().join("two.adoc"), "= Two ✓\n").unwrap();
    let (mut sharing, link) = Host::start(LOOPBACK, "0.0.0", timings(), None).unwrap();
    let address = sharing.address();
    let mut host = engine();
    host.host_session(
        Box::new(link),
        directory.path(),
        &[PathBuf::from("one.adoc"), PathBuf::from("two.adoc")],
        "Host",
    )
    .unwrap();
    let invitation = sharing.invite().unwrap();

    // Handshake: the fingerprint pinned, the same safety code both sides, nothing sent
    // before the host admits the guest.
    let mut guest = Guest::start(&invitation);
    assert_eq!(
        guest.answer(&mut host),
        format!("code {}", sharing.safety_code())
    );
    until(&mut host, |host| !host.session_knocks().is_empty());
    let (peer, name) = host.session_knocks()[0].clone();
    assert_eq!(name, "Guest");
    host.admit_peer(peer).unwrap();
    host.set_peer_role(peer, Role::Editor).unwrap();

    // Listing: the two pages, by path from the shared folder only.
    assert_eq!(
        guest.ask(&mut host, "manifest"),
        "manifest one.adoc two.adoc"
    );

    // Several pages edited at once from both sides converge, Unicode included.
    assert_eq!(guest.ask(&mut host, "open 1"), "ok");
    assert_eq!(guest.ask(&mut host, "open 2"), "ok");
    assert_eq!(guest.ask(&mut host, "editor"), "ok");
    let one = host.session_document(1).unwrap();
    let two = host.session_document(2).unwrap();
    assert!(insert(&mut host, one, 0, "Host: "));
    assert_eq!(
        guest.ask(&mut host, &format!("type 2 0 {}", hex("😀 "))),
        "ok"
    );
    assert_eq!(
        guest.ask(&mut host, &format!("type 1 0 {}", hex("// "))),
        "ok"
    );
    until(&mut host, |host| {
        text(host, two).starts_with("😀 ") && text(host, one).contains("// ")
    });
    let expected_one = text(&host, one);
    assert_eq!(
        guest.ask(&mut host, &format!("expect 1 {}", hex(&expected_one))),
        "ok"
    );
    let expected_two = format!("expect 2 {}", hex(&text(&host, two)));
    assert_eq!(guest.ask(&mut host, &expected_two), "ok");
    assert!(expected_one.contains("Host: ") && expected_one.contains("// "));

    // Optimistic updates are acknowledged: nothing of the guest's is left waiting.
    assert_eq!(guest.ask(&mut host, "unacked 1"), "unacked 0");

    // Revocation: a Viewer's edit is refused there, and the host's text stays.
    host.set_peer_role(peer, Role::Viewer).unwrap();
    assert_eq!(guest.ask(&mut host, "viewer"), "ok");
    let before = text(&host, one);
    assert_eq!(guest.ask(&mut host, "try 1"), "refused");
    for _ in 0..20 {
        host.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(text(&host, one), before);

    // Reconnection: the link drops, the host edits meanwhile, the guest resumes in the same
    // epoch and catches up, then edits again once an Editor.
    assert_eq!(guest.ask(&mut host, "drop"), "ok");
    until(&mut host, |host| {
        host.participants()
            .iter()
            .any(|participant| participant.peer == peer && !participant.connected)
    });
    assert!(insert(&mut host, one, 0, "Away. "));
    assert_eq!(guest.ask(&mut host, "resume"), "ok");
    let caught_up = format!("expect 1 {}", hex(&text(&host, one)));
    assert_eq!(guest.ask(&mut host, &caught_up), "ok");
    host.set_peer_role(peer, Role::Editor).unwrap();
    assert_eq!(guest.ask(&mut host, "editor"), "ok");
    assert_eq!(
        guest.ask(&mut host, &format!("type 1 0 {}", hex("Back. "))),
        "ok"
    );
    until(&mut host, |host| {
        text(host, one).starts_with("Back. Away. ")
    });
    assert_eq!(host.shared_file(one).unwrap().1, 1);

    // Shutdown: stopping closes the guest's connection; the guest ends cleanly; nothing
    // listens any more.
    sharing.stop();
    assert_eq!(guest.ask(&mut host, "gone"), "ok");
    guest.send("quit");
    assert!(guest.wait().success());
    assert!(
        TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_err(),
        "the host still listens"
    );
}

/// The guest's side of `collab_two_process_release_suite`, run in a process of its own. Does
/// nothing unless its environment says it is that process.
#[test]
#[ignore = "run by collab_two_process_release_suite as its guest"]
fn guest_process() {
    if std::env::var_os(GUEST).is_none() {
        return;
    }
    // Commands arrive on a thread of their own, so the engine is pumped between them; the
    // standard input closing (the test gone) ends the guest.
    let (send, commands) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let invitation: Invitation = commands.recv_timeout(STEP).unwrap().trim().parse().unwrap();
    let say = |answer: &str| {
        let mut out = std::io::stdout().lock();
        writeln!(out, "@ {answer}").unwrap();
        out.flush().unwrap();
    };

    let connecting = adoc_share::connect(&invitation, timings()).unwrap();
    say(&format!("code {}", connecting.safety_code()));
    let (link, control) = connecting
        .authenticate("Guest", None, "0.0.0", None)
        .unwrap();
    let mut guest = engine();
    guest.join_session(Box::new(link)).unwrap();

    // Pumps until `done`, within a step; whether it came.
    let within = |guest: &mut Engine, done: &mut dyn FnMut(&mut Engine) -> bool| {
        let deadline = Instant::now() + STEP;
        loop {
            guest.pump();
            if done(guest) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let answer = |done: bool| say(if done { "ok" } else { "timeout" });

    loop {
        let command = match commands.recv_timeout(Duration::from_millis(5)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                guest.pump();
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["manifest"] => {
                within(&mut guest, &mut |guest| guest.session_manifest().len() == 2);
                let paths: Vec<String> = guest
                    .session_manifest()
                    .into_iter()
                    .map(|entry| entry.path)
                    .collect();
                say(&format!("manifest {}", paths.join(" ")));
            }
            ["open", file] => {
                let file: u64 = file.parse().unwrap();
                let opened = guest.open_session_file(file).is_ok()
                    && within(&mut guest, &mut |guest| {
                        guest.session_document(file).is_some()
                    });
                answer(opened);
            }
            ["editor"] => answer(within(&mut guest, &mut |guest| {
                guest
                    .session_me()
                    .is_some_and(|(_, role)| role == Role::Editor)
            })),
            ["viewer"] => answer(within(&mut guest, &mut |guest| {
                guest
                    .session_me()
                    .is_some_and(|(_, role)| role == Role::Viewer)
            })),
            ["type", file, at, typed] => {
                let id = guest.session_document(file.parse().unwrap()).unwrap();
                let typed = unhex(typed);
                let at: usize = at.parse().unwrap();
                // Editing resumes once a resynchronisation is done.
                answer(within(&mut guest, &mut |guest| {
                    insert(guest, id, at, &typed)
                }));
            }
            ["try", file] => {
                let id = guest.session_document(file.parse().unwrap()).unwrap();
                let before = text(&guest, id);
                let refused = !insert(&mut guest, id, 0, "PWNED ") && text(&guest, id) == before;
                say(if refused { "refused" } else { "edited" });
            }
            ["expect", file, wanted] => {
                let id = guest.session_document(file.parse().unwrap()).unwrap();
                let wanted = unhex(wanted);
                answer(within(&mut guest, &mut |guest| text(guest, id) == wanted));
            }
            ["unacked", file] => {
                let file: u64 = file.parse().unwrap();
                within(&mut guest, &mut |guest| {
                    guest.session_unacknowledged(file) == 0
                });
                say(&format!("unacked {}", guest.session_unacknowledged(file)));
            }
            ["drop"] => {
                control.drop_connection();
                answer(within(&mut guest, &mut |guest| !guest.session_connected()));
            }
            ["resume"] => {
                control.resume();
                answer(within(&mut guest, &mut |guest| guest.session_connected()));
            }
            ["gone"] => answer(within(&mut guest, &mut |guest| !guest.session_connected())),
            ["quit"] => break,
            _ => say("unknown"),
        }
    }
    drop(control);
}
