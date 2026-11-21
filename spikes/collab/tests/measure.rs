//! COLLAB 1/8's measurements (#688 deliverable 1.5), a test binary of its own so the process's
//! memory is this session's alone. Ignored by default; run in release:
//! `cargo test --release --test measure -- --ignored --nocapture`, under `/usr/bin/time -l` (macOS)
//! or `/usr/bin/time -v` (Linux) for the peak.

use std::time::{Duration, Instant};

use adocfoundry_collab_spike::{apply_edits, Edit, Replica};

const LIMIT: usize = 64 << 20;

/// About `bytes` of AsciiDoc: sections, paragraphs, lists, code and a table, with some
/// non-ASCII text.
fn page(bytes: usize) -> String {
    let mut text = String::from("= A Large Page\n:toc:\n\n");
    let mut section = 0;
    while text.len() < bytes {
        section += 1;
        text.push_str(&format!(
            "== Section {section}\n\nA paragraph of the section with *strong* and _emphasised_ words, a \
             xref:other.adoc#s{section}[cross reference] and a café or two, déjà vu, 日本語.\n\n\
             * one\n* two\n** two and a half\n\n[source,rust]\n----\nfn section_{section}() {{}}\n----\n\n\
             |===\n|Key |Value\n|one |1\n|===\n\n"
        ));
    }
    text
}

fn percentile(times: &mut [Duration], p: usize) -> Duration {
    times.sort();
    times[(times.len() - 1) * p / 100]
}

/// The process's resident memory now, in MB, as `ps` reports it.
fn resident_mb() -> f64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

#[test]
#[ignore = "a measurement; run in release with --ignored --nocapture"]
fn measure_a_1_mib_session() {
    session(true);
}

/// The same session with no whole-text check: what `yrs` costs alone.
#[test]
#[ignore = "a measurement; run in release with --ignored --nocapture"]
fn measure_a_1_mib_session_yrs_alone() {
    session(false);
}

fn session(verify: bool) {
    println!("whole-text check: {verify}");
    let start_mb = resident_mb();
    let text = page(1 << 20);
    let mut host = Replica::new(1);
    let mut guest = Replica::new(2);
    host.set_verify_whole_text(verify);
    guest.set_verify_whole_text(verify);

    // Seeding and joining.
    let started = Instant::now();
    host.apply_local(&[Edit::new(0..0, &text)]).unwrap();
    let seeded = started.elapsed();
    let snapshot = host.snapshot();
    let started = Instant::now();
    guest.apply_remote(&snapshot, LIMIT).unwrap();
    let joined = started.elapsed();
    println!(
        "page {} bytes: seeded in {seeded:?}, snapshot {} bytes, joined in {joined:?}",
        text.len(),
        snapshot.len()
    );

    // A long session: typing at a caret that moves through the page, a word at a time, with a
    // backspace now and then; every keystroke one transaction sent to the guest.
    let mut rope = text.clone();
    let mut local = Vec::new();
    let mut remote = Vec::new();
    let mut bytes = 0usize;
    let mut keystrokes = 0usize;
    let words = ["the ", "page ", "café ", "日本 ", "and ", "*bold* "];
    let mut caret = rope.len() / 3;
    for keystroke in 0..20_000usize {
        if keystroke % 400 == 0 {
            // Move to another paragraph's end, at a character boundary.
            caret = (caret + 37_717) % rope.len();
            while !rope.is_char_boundary(caret) {
                caret += 1;
            }
            host.end_group();
        }
        let edit = if keystroke % 23 == 22 && caret > 0 {
            let mut start = caret - 1;
            while !rope.is_char_boundary(start) {
                start -= 1;
            }
            Edit::new(start..caret, "")
        } else {
            let word = words[keystroke % words.len()];
            let character = word.chars().nth(keystroke % word.chars().count()).unwrap();
            Edit::new(caret..caret, character.encode_utf8(&mut [0; 4]))
        };
        let started = Instant::now();
        let update = host.apply_local(std::slice::from_ref(&edit)).unwrap();
        local.push(started.elapsed());
        apply_edits(&mut rope, std::slice::from_ref(&edit));
        caret = edit.range.start + edit.text.len();
        if edit.text == " " {
            host.end_group();
        }
        bytes += update.len();
        keystrokes += 1;
        let started = Instant::now();
        let edits = guest.apply_remote(&update, LIMIT).unwrap();
        remote.push(started.elapsed());
        assert_eq!(edits.len(), 1);
    }
    assert_eq!(guest.text(), rope);
    println!(
        "{keystrokes} keystrokes: {:.1} update bytes each; local p50 {:?} p95 {:?}; remote p50 {:?} p95 {:?}",
        bytes as f64 / keystrokes as f64,
        percentile(&mut local, 50),
        percentile(&mut local, 95),
        percentile(&mut remote, 50),
        percentile(&mut remote, 95),
    );

    // A pasted paragraph, and a deleted one.
    let paragraph = "A pasted paragraph of about five hundred bytes. ".repeat(10);
    let update = host
        .apply_local(&[Edit::new(1000..1000, &paragraph)])
        .unwrap();
    guest.apply_remote(&update, LIMIT).unwrap();
    println!(
        "a {}-byte paste: update {} bytes",
        paragraph.len(),
        update.len()
    );
    let update = host
        .apply_local(&[Edit::new(1000..1000 + paragraph.len(), "")])
        .unwrap();
    guest.apply_remote(&update, LIMIT).unwrap();
    println!("deleting it: update {} bytes", update.len());

    // Reconnecting after missing 2,000 keystrokes.
    let before = guest.state_vector();
    for at in (5000..).take(2000) {
        host.apply_local(&[Edit::new(at..at, "x")]).unwrap();
    }
    let started = Instant::now();
    let missing = host.missing_since(&before).unwrap();
    let encoded = started.elapsed();
    let started = Instant::now();
    guest.apply_remote(&missing, LIMIT).unwrap();
    println!(
        "reconnect after 2,000 keystrokes: {} bytes, encoded in {encoded:?}, applied in {:?}",
        missing.len(),
        started.elapsed()
    );
    assert_eq!(guest.text(), host.text());

    // Undo of the last group and its update.
    let started = Instant::now();
    let (_, update) = host.undo().unwrap();
    println!(
        "an undo: {:?}, update {} bytes",
        started.elapsed(),
        update.len()
    );

    println!(
        "state after the session: {} bytes for {} bytes of text; resident {:.0} MB (from {:.0} MB)",
        host.state_size(),
        host.text().len(),
        resident_mb(),
        start_mb
    );
}

/// A small page (about 4 KiB) typed into for 2,000 keystrokes: what `yrs` costs where most
/// editing happens.
#[test]
#[ignore = "a measurement; run in release with --ignored --nocapture"]
fn measure_a_small_page() {
    let text = page(4 << 10);
    let mut host = Replica::new(1);
    let mut guest = Replica::new(2);
    host.set_verify_whole_text(false);
    guest.set_verify_whole_text(false);
    host.apply_local(&[Edit::new(0..0, &text)]).unwrap();
    let snapshot = host.snapshot();
    guest.apply_remote(&snapshot, LIMIT).unwrap();
    let mut rope = text.clone();
    let mut caret = rope.len() / 2;
    while !rope.is_char_boundary(caret) {
        caret += 1;
    }
    let (mut local, mut remote, mut bytes) = (Vec::new(), Vec::new(), 0usize);
    for keystroke in 0..2000usize {
        let character = ["a", "é", " ", "日"][keystroke % 4];
        let edit = Edit::new(caret..caret, character);
        let started = Instant::now();
        let update = host.apply_local(std::slice::from_ref(&edit)).unwrap();
        local.push(started.elapsed());
        apply_edits(&mut rope, std::slice::from_ref(&edit));
        caret += character.len();
        if character == " " {
            host.end_group();
        }
        bytes += update.len();
        let started = Instant::now();
        guest.apply_remote(&update, LIMIT).unwrap();
        remote.push(started.elapsed());
    }
    assert_eq!(guest.text(), rope);
    println!(
        "small page {} bytes, snapshot {} bytes; 2000 keystrokes: {:.1} update bytes each; local p50 {:?} p95 {:?}; remote p50 {:?} p95 {:?}; state {} bytes for {} bytes of text",
        text.len(),
        snapshot.len(),
        bytes as f64 / 2000.0,
        percentile(&mut local, 50),
        percentile(&mut local, 95),
        percentile(&mut remote, 50),
        percentile(&mut remote, 95),
        host.state_size(),
        rope.len()
    );
}
