//! Timing probes for the keystroke path. Ignored by default; run with
//! `cargo test --release -p adoc-engine --test perf -- --ignored --nocapture`.

use std::{fmt::Write as _, fs, time::Instant};

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, CommandArgs, Engine, Settings};

/// A 5,000-line document with sections, xrefs, includes, attributes and blocks.
fn synthetic_document() -> String {
    let mut text = String::from("= Synthetic\n:product: Foundry\n\n");
    let mut section = 0;
    while text.lines().count() < 5_000 {
        section += 1;
        let _ = write!(
            text,
            "== Section {section}\n\nSee xref:other.adoc#part-{section}[part {section}] and <<section-{}>>.\n\
             The {{product}} value is used here.\n\n[source,rust]\n----\nfn main() {{}}\n----\n\n\
             * item one\n* item two\n\n",
            section.max(2) - 1
        );
    }
    text
}

#[test]
#[ignore = "timing probe"]
fn keystroke_on_a_5000_line_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.adoc");
    fs::write(&path, synthetic_document()).unwrap();
    fs::write(directory.path().join("other.adoc"), "= Other\n").unwrap();
    // A workspace of 500 further pages linking to one another.
    for page in 0..500 {
        fs::write(
            directory.path().join(format!("page-{page}.adoc")),
            format!(
                "= Page {page}\n\n== Body\n\nSee xref:page-{}.adoc#body[next] and xref:missing-{page}.adoc[].\n",
                (page + 1) % 500
            ),
        )
        .unwrap();
    }

    let mut engine = Engine::new(Settings::default());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    engine.wait_for_workers(std::time::Duration::from_secs(30));
    let id = engine.open(&path).unwrap();
    let lines = engine.document(id).unwrap().parsed.text.lines().count();

    let mut samples = Vec::new();
    for offset in (100..20_100).step_by(100) {
        let started = Instant::now();
        engine
            .invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(offset, offset),
                    text: "x".to_owned(),
                },
            )
            .unwrap();
        samples.push(started.elapsed());
    }
    samples.sort();
    let median = samples[samples.len() / 2];
    let worst = samples[samples.len() - 1];
    println!(
        "{lines} lines: keystroke (re-parse, index, diagnostics) median {median:?}, worst {worst:?}"
    );

    let started = Instant::now();
    let diagnostics = engine.workspace_diagnostics();
    println!(
        "workspace diagnostics: {:?} for {} file(s)",
        started.elapsed(),
        diagnostics.len()
    );
}
