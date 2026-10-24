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

/// A book's export, for its time and peak memory: 300 pages of sections, admonitions, code and
/// tables in one component's navigation. Run it under `/usr/bin/time -l` (macOS) or
/// `/usr/bin/time -v` (Linux) to read the peak; a 1,833-page documentation book peaked at
/// 9.3 GB and took 13 s once blocks known to fit stopped being measured and running titles
/// stopped filtering every heading on every page.
#[test]
#[ignore = "memory probe"]
fn book_memory_probe() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("probe");
    let pages = root.join("modules/ROOT/pages");
    fs::create_dir_all(&pages).unwrap();
    fs::write(
        root.join("antora.yml"),
        "name: probe\ntitle: Probe\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    let mut nav = String::new();
    for page in 0..300 {
        let mut text = format!("= Page {page}\n\nAn introduction to page {page}.\n\n");
        for section in 0..10 {
            let _ = write!(
                text,
                "== Section {section}\n\nA paragraph of the section, long enough to wrap onto a second line of the page.\n\n\
                 NOTE: A note in section {section}.\n\n[source,rust]\n----\nfn section_{section}() {{}}\n----\n\n\
                 |===\n|Key |Value\n|one |1\n|two |2\n|===\n\n"
            );
        }
        fs::write(pages.join(format!("page-{page}.adoc")), text).unwrap();
        let _ = writeln!(nav, "* xref:page-{page}.adoc[Page {page}]");
    }
    fs::write(root.join("modules/ROOT/nav.adoc"), nav).unwrap();

    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(120)));
    let started = Instant::now();
    let summary = engine
        .export_blocking(
            adoc_engine::ExportArgs {
                target: adoc_engine::ExportTarget::Component {
                    name: "probe".to_owned(),
                    version: Some("1.0".to_owned()),
                },
                output: directory.path().join("probe.pdf"),
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
            },
            std::time::Duration::from_secs(600),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    eprintln!(
        "book of {} pages exported in {:.1} s",
        summary.pages,
        started.elapsed().as_secs_f64()
    );
}
