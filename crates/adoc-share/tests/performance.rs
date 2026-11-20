//! Collaboration's costs (COLLAB 8/8, #695; PRD §9): `collab_update_sizes_stay_small` holds the
//! bytes an edit sends to a bound on every run, since CI gates on no wall-clock time;
//! `collab_performance_probe` measures the rest over loopback TLS on this machine and prints a
//! table, the source of every figure in `docs/qa/collab-performance.md`:
//!
//! ```sh
//! cargo test --release -p adoc-share --test performance -- --ignored --nocapture
//! ```
//!
//! Loopback is not a LAN: it shows the engines' and the transport's own costs, not a
//! network's.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, protocol::Role, CommandArgs, DocumentId, Engine, Settings};
use adoc_share::{Host, Timings};

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

fn timings() -> Timings {
    Timings {
        handshake: Duration::from_secs(5),
        heartbeat: Duration::from_millis(500),
        link_timeout: Duration::from_secs(10),
        invite_lifetime: Duration::from_secs(60),
        admit_wait: Duration::from_secs(10),
    }
}

fn engine() -> Engine {
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    Engine::new(settings)
}

fn insert(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) {
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

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text_string()
}

/// A page of `bytes` bytes of ordinary prose.
fn page(bytes: usize) -> String {
    let paragraph = "A sentence of ordinary prose, with a comma and a full stop. ";
    let mut text = String::from("= Page\n\n");
    while text.len() < bytes {
        text.push_str(paragraph);
        if text.len() % 600 < paragraph.len() {
            text.push_str("\n\n");
        }
    }
    text
}

/// Pumps until `done`, for at most 30 seconds.
fn until(engines: &mut [&mut Engine], mut done: impl FnMut(&[&mut Engine]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for engine in engines.iter_mut() {
            engine.pump();
        }
        if done(engines) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting");
        std::thread::yield_now();
    }
}

/// A host sharing `pages` (name, text) and one guest, an Editor, with every page open.
struct Pair {
    _directory: tempfile::TempDir,
    _sharing: Host,
    host: Engine,
    guest: Engine,
    control: adoc_share::GuestControl,
    /// How long the guest took from authenticating to holding every page.
    joined: Duration,
}

fn pair(pages: &[(&str, &str)]) -> Pair {
    let directory = tempfile::tempdir().unwrap();
    let mut files = Vec::new();
    for (name, text) in pages {
        fs::write(directory.path().join(name), text).unwrap();
        files.push(PathBuf::from(name));
    }
    let (sharing, link) = Host::start(LOOPBACK, "0.0.0", timings(), None).unwrap();
    let mut host = engine();
    host.host_session(Box::new(link), directory.path(), &files, "Host")
        .unwrap();
    let invitation = sharing.invite().unwrap();
    let start = Instant::now();
    let (link, control) = adoc_share::connect(&invitation, timings())
        .unwrap()
        .authenticate("Guest", "0.0.0", None)
        .unwrap();
    let mut guest = engine();
    guest.join_session(Box::new(link)).unwrap();
    until(&mut [&mut host, &mut guest], |engines| {
        !engines[0].session_knocks().is_empty()
    });
    let peer = host.session_knocks()[0].0;
    host.admit_peer(peer).unwrap();
    host.set_peer_role(peer, Role::Editor).unwrap();
    until(&mut [&mut host, &mut guest], |engines| {
        engines[1].session_manifest().len() == pages.len()
    });
    for file in 1..=pages.len() as u64 {
        guest.open_session_file(file).unwrap();
    }
    until(&mut [&mut host, &mut guest], |engines| {
        (1..=pages.len() as u64).all(|file| engines[1].session_document(file).is_some())
            && engines[1]
                .session_me()
                .is_some_and(|(_, role)| role == Role::Editor)
    });
    let joined = start.elapsed();
    Pair {
        _directory: directory,
        _sharing: sharing,
        host,
        guest,
        control,
        joined,
    }
}

/// The bytes the host's updates for document `id` came to since last asked.
fn sent_bytes(engine: &mut Engine, id: DocumentId) -> usize {
    engine
        .take_shared_updates(id)
        .iter()
        .map(|update| update.0.len())
        .sum()
}

/// What one typed character and one pasted paragraph send, against the text they add: an
/// edit's update stays a small constant over its text, whatever the page's size or history.
#[test]
fn collab_update_sizes_stay_small() {
    let mut host = engine();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    fs::write(&path, page(200_000)).unwrap();
    let id = host.open(&path).unwrap();
    host.share_document(
        id,
        adoc_engine::SharedFileId(1),
        adoc_engine::ReplicaId::new(7).unwrap(),
    )
    .unwrap();
    let paragraph = "A new paragraph pasted in one go, of some length. ".repeat(8);
    for round in 0..200 {
        let at = (round * 977) % text(&host, id).len();
        let at = text(&host, id)
            .char_indices()
            .map(|(at, _)| at)
            .find(|start| *start >= at)
            .unwrap_or(0);
        insert(&mut host, id, at, "x");
        let one = sent_bytes(&mut host, id);
        assert!(one <= 32, "one character sent {one} bytes");
        insert(&mut host, id, at, &paragraph);
        let many = sent_bytes(&mut host, id);
        assert!(
            many <= paragraph.len() + 32,
            "a paragraph of {} bytes sent {many}",
            paragraph.len()
        );
    }
}

fn percentile(samples: &mut [Duration], at: f64) -> Duration {
    samples.sort();
    samples[((samples.len() - 1) as f64 * at).round() as usize]
}

fn micros(duration: Duration) -> String {
    format!("{:.1} µs", duration.as_secs_f64() * 1e6)
}

/// Typing latency on one document, with collaboration (the host of a session) or without.
fn typing(shared: bool, size: usize) -> (Duration, Duration) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    fs::write(&path, page(size)).unwrap();
    let mut engine = engine();
    let id = engine.open(&path).unwrap();
    if shared {
        engine
            .share_document(
                id,
                adoc_engine::SharedFileId(1),
                adoc_engine::ReplicaId::new(7).unwrap(),
            )
            .unwrap();
    }
    let mut samples = Vec::new();
    let middle = text(&engine, id).len() / 2;
    let middle = text(&engine, id)[..middle]
        .char_indices()
        .last()
        .map_or(0, |(at, _)| at);
    for at in middle..middle + 2000 {
        let start = Instant::now();
        insert(&mut engine, id, at, "a");
        samples.push(start.elapsed());
        if shared {
            engine.take_shared_updates(id);
        }
    }
    (
        percentile(&mut samples, 0.5),
        percentile(&mut samples, 0.95),
    )
}

#[test]
#[ignore = "a measurement, run by hand"]
fn collab_performance_probe() {
    println!("| Measure | Small page (4 KiB) | 1 MiB document |");
    println!("| --- | --- | --- |");
    let (small_alone, small_alone_95) = typing(false, 4 << 10);
    let (large_alone, large_alone_95) = typing(false, 1 << 20);
    let (small_shared, small_shared_95) = typing(true, 4 << 10);
    let (large_shared, large_shared_95) = typing(true, 1 << 20);
    println!(
        "| Typing, alone, p50 / p95 | {} / {} | {} / {} |",
        micros(small_alone),
        micros(small_alone_95),
        micros(large_alone),
        micros(large_alone_95)
    );
    println!(
        "| Typing, hosting, p50 / p95 | {} / {} | {} / {} |",
        micros(small_shared),
        micros(small_shared_95),
        micros(large_shared),
        micros(large_shared_95)
    );

    let mut row = [String::new(), String::new(), String::new()];
    for (column, size) in [(0, 4 << 10), (1, 1 << 20)] {
        let body = page(size);
        let mut pair = pair(&[("page.adoc", &body)]);
        let host_doc = pair.host.session_document(1).unwrap();
        let guest_doc = pair.guest.session_document(1).unwrap();
        let snapshot = pair.host.shared_snapshot(host_doc).unwrap().0 .0.len();

        // Remote latency: a character typed by the host until the guest's text shows it.
        let mut samples = Vec::new();
        let rounds: usize = std::env::var("ADOC_PERF_ROUNDS")
            .ok()
            .and_then(|rounds| rounds.parse().ok())
            .unwrap_or(200);
        for n in 0..rounds {
            let before = text(&pair.host, host_doc);
            let at = before.len();
            let start = Instant::now();
            insert(
                &mut pair.host,
                host_doc,
                at,
                if n % 2 == 0 { "a" } else { "b" },
            );
            let wanted = text(&pair.host, host_doc);
            until(&mut [&mut pair.host, &mut pair.guest], |engines| {
                text(engines[1], guest_doc).len() == wanted.len()
            });
            samples.push(start.elapsed());
        }
        let p50 = percentile(&mut samples, 0.5);
        let p95 = percentile(&mut samples, 0.95);

        // Reconnection in the same epoch: the link drops and comes back.
        pair.control.drop_connection();
        until(&mut [&mut pair.host, &mut pair.guest], |engines| {
            !engines[1].session_connected()
        });
        let start = Instant::now();
        pair.control.resume();
        until(&mut [&mut pair.host, &mut pair.guest], |engines| {
            engines[1].session_connected()
                && engines[1].session_unacknowledged(1) == 0
                && text(engines[1], guest_doc) == text(engines[0], host_doc)
        });
        let reconnect = start.elapsed();

        if column == 0 {
            row[0] = format!(
                "| Remote update, loopback, p50 / p95 | {} / {}",
                micros(p50),
                micros(p95)
            );
            row[1] = format!(
                "| Initial snapshot | {} B, joined in {:.0} ms",
                snapshot,
                pair.joined.as_secs_f64() * 1e3
            );
            row[2] = format!(
                "| Reconnection, one page | {:.0} ms",
                reconnect.as_secs_f64() * 1e3
            );
        } else {
            row[0].push_str(&format!(" | {} / {} |", micros(p50), micros(p95)));
            row[1].push_str(&format!(
                " | {} B, joined in {:.0} ms |",
                snapshot,
                pair.joined.as_secs_f64() * 1e3
            ));
            row[2].push_str(&format!(" | {:.0} ms |", reconnect.as_secs_f64() * 1e3));
        }
    }
    for line in &row {
        println!("{line}");
    }

    // Bytes sent per character typed and per paragraph pasted, on a page with history.
    let mut host = engine();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    fs::write(&path, page(64 << 10)).unwrap();
    let id = host.open(&path).unwrap();
    host.share_document(
        id,
        adoc_engine::SharedFileId(1),
        adoc_engine::ReplicaId::new(7).unwrap(),
    )
    .unwrap();
    let paragraph = "A new paragraph pasted in one go, of some length. ".repeat(8);
    let (mut one, mut many) = (0, 0);
    for round in 0..100 {
        let at = (round * 613) % text(&host, id).len();
        insert(&mut host, id, at, "x");
        one += sent_bytes(&mut host, id);
        insert(&mut host, id, at, &paragraph);
        many += sent_bytes(&mut host, id);
    }
    println!(
        "| Update per character typed | {:.1} B | |",
        one as f64 / 100.0
    );
    println!(
        "| Update per {}-byte paragraph | {:.0} B | |",
        paragraph.len(),
        many as f64 / 100.0
    );

    // A long session: 20,000 single-character edits, half of them deletions.
    let start_size = host.shared_snapshot(id).unwrap().0 .0.len();
    let mut at = 100;
    for n in 0..20_000 {
        if n % 2 == 0 {
            insert(&mut host, id, at, "y");
        } else {
            host.invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(at, at + 1),
                    text: String::new(),
                },
            )
            .unwrap();
            at = (at + 37) % (text(&host, id).len() - 2);
        }
        host.take_shared_updates(id);
    }
    let end_size = host.shared_snapshot(id).unwrap().0 .0.len();
    println!("| Snapshot after 20,000 edits on a 64 KiB page | {start_size} B to {end_size} B | |");

    // Several shared files: joining with ten pages open.
    let pages: Vec<(String, String)> = (0..10)
        .map(|n| (format!("page{n}.adoc"), page(16 << 10)))
        .collect();
    let borrowed: Vec<(&str, &str)> = pages
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();
    let several = pair(&borrowed);
    println!(
        "| Joining with ten 16 KiB pages open | {:.0} ms | |",
        several.joined.as_secs_f64() * 1e3
    );
}
