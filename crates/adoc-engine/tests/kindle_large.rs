//! The Kindle export PRD, §12 test 22 (`kindle_large_book_is_bounded`): a large, picture-heavy
//! book exports in both formats within the budgets of `docs/qa/spike-kindle.md`, with the work
//! on workers rather than the front end's thread. A file of its own, so the process's peak
//! memory is these exports' alone.

#![cfg(feature = "kindle")]

use std::{
    fmt::Write as _,
    fs,
    path::Path,
    time::{Duration, Instant},
};

use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineEvent, ExportTarget, KindleExportArgs, KindleFormat,
    Settings,
};

/// The book's pages, each of ten sections with a list, a note, code and a table.
const PAGES: usize = 600;
/// Pages that show a picture of their own.
const PICTURES: usize = 50;

/// Writes a component of [`PAGES`] pages, listed in its navigation, under `folder`, with
/// [`PICTURES`] distinct pictures.
fn large_component(folder: &Path) {
    let root = folder.join("large");
    let pages = root.join("modules/ROOT/pages");
    let images = root.join("modules/ROOT/images");
    fs::create_dir_all(&pages).unwrap();
    fs::create_dir_all(&images).unwrap();
    fs::write(
        root.join("antora.yml"),
        "name: large\ntitle: A Large Book\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    let picture = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/docx/reference/manual/modules/ROOT/images/photo.jpg"),
    )
    .unwrap();
    let mut nav = String::new();
    for page in 0..PAGES {
        let mut text = format!("= Page {page}\n\nAn introduction to page {page}.\n\n");
        if page < PICTURES {
            // Distinct bytes each: a comment segment after the JPEG's start.
            let mut bytes = picture[..2].to_vec();
            bytes.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x06]);
            bytes.extend_from_slice(&(page as u32).to_be_bytes());
            bytes.extend_from_slice(&picture[2..]);
            fs::write(images.join(format!("picture-{page}.jpg")), bytes).unwrap();
            let _ = write!(text, "image::picture-{page}.jpg[Picture {page}]\n\n");
        }
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
fn kindle_large_book_is_bounded() {
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

    for format in [KindleFormat::Azw3, KindleFormat::Kfx] {
        // As the app exports: the command returns at once, and the front end pumps the engine
        // until the export finishes, each pump timed.
        let output = directory
            .path()
            .join(format!("large.{}", format.extension()));
        let started = Instant::now();
        engine
            .invoke(
                ids::EXPORT_KINDLE_COMPONENT,
                CommandArgs::ExportKindle(KindleExportArgs::new(
                    ExportTarget::Component {
                        name: "large".to_owned(),
                        version: Some("1.0".to_owned()),
                    },
                    format,
                    output.clone(),
                )),
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
            "{format:?}, {} pages: invoke {invoked:?}, longest pump {longest_pump:?}, whole \
             {elapsed:?}, {size} bytes, peak memory {:?} MB (before the exports {:?} MB)",
            summary.pages,
            peak.map(|bytes| bytes / 1_000_000),
            before.map(|bytes| bytes / 1_000_000),
        );
        assert_eq!(summary.pages, PAGES);
        assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);

        // The budgets of `docs/qa/spike-kindle.md`, for a debug build on a CI runner.
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
}
