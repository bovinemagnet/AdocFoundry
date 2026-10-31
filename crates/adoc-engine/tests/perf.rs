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
    let lines = engine.document(id).unwrap().parsed().text.lines().count();

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

/// The synthetic document open in a workspace of 500 linked pages, with `tabs` of those pages
/// open as well, as tabs are.
fn typing_workspace(tabs: usize) -> (tempfile::TempDir, Engine, adoc_engine::DocumentId) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.adoc");
    fs::write(&path, synthetic_document()).unwrap();
    fs::write(directory.path().join("other.adoc"), "= Other\n").unwrap();
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
    for page in 0..tabs {
        engine
            .open(&directory.path().join(format!("page-{page}.adoc")))
            .unwrap();
    }
    let id = engine.open(&path).unwrap();
    (directory, engine, id)
}

fn report(what: &str, mut samples: Vec<std::time::Duration>) {
    samples.sort();
    println!(
        "{what}: median {:?}, worst {:?}",
        samples[samples.len() / 2],
        samples[samples.len() - 1]
    );
}

/// Backspace held down: one-byte deletions through `edit.replace_range`, with 20 tabs open.
#[test]
#[ignore = "timing probe"]
fn backspace_loop() {
    let (_directory, mut engine, id) = typing_workspace(20);
    let mut samples = Vec::new();
    for _ in 0..200 {
        let end = engine.document(id).unwrap().text.len();
        let started = Instant::now();
        engine
            .invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(end - 1, end),
                    text: String::new(),
                },
            )
            .unwrap();
        samples.push(started.elapsed());
    }
    report("backspace (20 tabs)", samples);
}

/// Typing in the visual editor: one character at a time through `visual.insert_text`, with
/// 20 tabs open.
#[test]
#[ignore = "timing probe"]
fn visual_typing_loop() {
    let (_directory, mut engine, id) = typing_workspace(20);
    engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
        .unwrap();
    let start = engine
        .document(id)
        .unwrap()
        .text_string()
        .find("The ")
        .unwrap()
        + 4;
    let mut samples = Vec::new();
    for at in start..start + 200 {
        let args = adoc_engine::VisualArgs {
            id,
            version: engine.document(id).unwrap().version,
            selection: SourceRange::new(at, at),
            marks: adoc_engine::MarkSet::EMPTY,
            value: adoc_engine::VisualValue::Text("x".to_owned()),
        };
        let started = Instant::now();
        engine
            .invoke(ids::VISUAL_INSERT_TEXT, CommandArgs::Visual(args))
            .unwrap();
        samples.push(started.elapsed());
    }
    report("visual typing (20 tabs)", samples);
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
                attributes: Vec::new(),
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

#[test]
#[ignore = "timing probe"]
fn spelling_a_5000_line_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.adoc");
    fs::write(&path, synthetic_document()).unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.enable_spelling();
    let id = engine.open(&path).unwrap();
    let started = Instant::now();
    engine.pump();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(60)));
    let elapsed = started.elapsed();
    println!(
        "spelling: {elapsed:?} for {} misspellings (dictionary load included)",
        engine.misspellings(id).len()
    );
    assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
}

#[test]
#[ignore = "timing probe"]
fn pdf_preview_after_typing() {
    // Issue #62: an ordinary page's PDF preview follows typing within a second or two. The
    // reference article: four pages of every block kind, an image, a table and a footnote.
    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/pdf/reference/article.adoc");
    let directory = tempfile::tempdir().unwrap();
    let copy = directory.path().join("article.adoc");
    fs::copy(&page, &copy).unwrap();
    fs::create_dir_all(directory.path().join("images")).unwrap();
    for entry in fs::read_dir(page.parent().unwrap().join("images")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(
            entry.path(),
            directory.path().join("images").join(entry.file_name()),
        )
        .unwrap();
    }
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let events = engine.subscribe();
    let id = engine.open(&copy).unwrap();
    engine
        .invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None)
        .unwrap();
    let ready = |engine: &mut Engine| {
        let started = Instant::now();
        loop {
            engine.pump();
            engine.wait_for_workers(std::time::Duration::from_millis(20));
            if events
                .try_iter()
                .any(|event| matches!(event, adoc_engine::EngineEvent::PdfPreviewReady { .. }))
            {
                return started.elapsed();
            }
            assert!(started.elapsed().as_secs() < 30, "no PDF preview");
        }
    };
    println!("first PDF preview: {:?}", ready(&mut engine));
    for round in 0..5 {
        // Anything still arriving from the round before.
        engine.wait_for_workers(std::time::Duration::from_secs(5));
        let _ = events.try_iter().count();
        let started = Instant::now();
        engine
            .invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(0, 0),
                    text: "// edited\n".to_owned(),
                },
            )
            .unwrap();
        let edit = started.elapsed();
        let latency = ready(&mut engine);
        println!(
            "round {round}: edit {edit:?}, PDF preview {:?} after the edit",
            edit + latency
        );
    }
}

#[test]
#[ignore = "timing probe"]
fn quick_open_in_a_workspace_of_3000_pages() {
    let directory = tempfile::tempdir().unwrap();
    for module in 0..30 {
        let pages = directory.path().join(format!("module-{module}"));
        fs::create_dir(&pages).unwrap();
        for page in 0..100 {
            fs::write(
                pages.join(format!("topic-{page}.adoc")),
                format!("= Topic {page} of Module {module}\n"),
            )
            .unwrap();
        }
    }
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(60)));
    for query in ["", "t", "top", "topic 42", "mod 7 top 99"] {
        let started = Instant::now();
        let found = engine
            .quick_open(query, adoc_engine::QUICK_OPEN_LIMIT)
            .len();
        println!("{query:?}: {found} files in {:?}", started.elapsed());
    }
}
