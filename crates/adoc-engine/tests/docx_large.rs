//! The DOCX export PRD, §10 test 19 (`docx_large_book_is_bounded`): a large book exports within
//! a time and memory budget, with the work on workers rather than the front end's thread. A
//! file of its own, so the process's peak memory is this export's alone.

#![cfg(feature = "docx")]

use std::{
    fmt::Write as _,
    fs,
    path::Path,
    time::{Duration, Instant},
};

use adoc_engine::{
    commands::ids, CommandArgs, DocxExportArgs, Engine, EngineEvent, ExportTarget, Settings,
};

/// The book's pages, each of ten sections with a list, a note, code and a table.
const PAGES: usize = 600;

/// Writes a component of [`PAGES`] pages, listed in its navigation, under `folder`.
fn large_component(folder: &Path) {
    let root = folder.join("large");
    let pages = root.join("modules/ROOT/pages");
    fs::create_dir_all(&pages).unwrap();
    fs::write(
        root.join("antora.yml"),
        "name: large\ntitle: A Large Book\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    let mut nav = String::new();
    for page in 0..PAGES {
        let mut text = format!("= Page {page}\n\nAn introduction to page {page}.\n\n");
        for section in 0..10 {
            let _ = write!(
                text,
                "== Section {section}\n\nA paragraph of the section, long enough to wrap onto a second line of the page, with *strong* and _emphasised_ words and a footnote.footnote:[Note {page}.{section}.]\n\n\
                 * one\n* two\n** two and a half\n\n\
                 NOTE: A note in section {section}.\n\n[source,rust]\n----\nfn section_{section}() {{}}\n----\n\n\
                 |===\n|Key |Value\n|one |1\n|two |2\n|===\n\n"
            );
        }
        if page > 0 {
            let _ = writeln!(text, "See xref:page-{}.adoc[the page before].", page - 1);
        }
        fs::write(pages.join(format!("page-{page}.adoc")), text).unwrap();
        let _ = writeln!(nav, "* xref:page-{page}.adoc[Page {page}]");
    }
    fs::write(root.join("modules/ROOT/nav.adoc"), nav).unwrap();
}

/// The process's peak resident memory in bytes, where the system reports it to a safe read:
/// `VmHWM` on Linux. Elsewhere `None`, and only time is checked.
fn peak_memory() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

#[test]
fn docx_large_book_is_bounded() {
    let directory = tempfile::tempdir().unwrap();
    large_component(directory.path());
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    let events = engine.subscribe();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(300)));
    let before = peak_memory();

    // As the app exports: the command returns at once, and the front end pumps the engine
    // until the export finishes, each pump timed.
    let output = directory.path().join("large.docx");
    let started = Instant::now();
    engine
        .invoke(
            ids::EXPORT_DOCX_COMPONENT,
            CommandArgs::ExportDocx(DocxExportArgs {
                target: ExportTarget::Component {
                    name: "large".to_owned(),
                    version: Some("1.0".to_owned()),
                },
                output: output.clone(),
                attributes: Vec::new(),
                paper: None,
                landscape: None,
                look: None,
            }),
        )
        .unwrap();
    let invoked = started.elapsed();
    let mut longest_pump = Duration::ZERO;
    let mut finished = None;
    while finished.is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "the export never finished"
        );
        let pump = Instant::now();
        engine.pump();
        longest_pump = longest_pump.max(pump.elapsed());
        finished = events.try_iter().find_map(|event| match event {
            EngineEvent::ExportFinished { result, .. } => Some(result),
            _ => None,
        });
        std::thread::sleep(Duration::from_millis(5));
    }
    let elapsed = started.elapsed();
    let summary = finished.unwrap().unwrap_or_else(|error| panic!("{error}"));
    let size = fs::metadata(&output).unwrap().len();
    let peak = peak_memory();
    eprintln!(
        "{} pages: invoke {invoked:?}, longest pump {longest_pump:?}, whole {elapsed:?}, \
         {size} bytes, peak memory {:?} MB (before the export {:?} MB)",
        summary.pages,
        peak.map(|bytes| bytes / 1_000_000),
        before.map(|bytes| bytes / 1_000_000),
    );
    assert_eq!(summary.pages, PAGES);
    assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);

    // The budgets, for a debug build on a CI runner. On a 2024 laptop the book takes 4.4 s,
    // the command 0.3 s and the longest pump 0.15 s, and peaks at 0.38 GB with the workspace
    // indexed; time and memory grow linearly with the pages (twice the book, twice of each).
    assert!(
        invoked < Duration::from_secs(3),
        "the command took {invoked:?}"
    );
    assert!(
        longest_pump < Duration::from_secs(3),
        "a pump took {longest_pump:?}: the front end would stall"
    );
    assert!(
        elapsed < Duration::from_secs(90),
        "the export took {elapsed:?}"
    );
    if let Some(peak) = peak {
        assert!(peak < 2_000_000_000, "peak memory {} MB", peak / 1_000_000);
    }
}
