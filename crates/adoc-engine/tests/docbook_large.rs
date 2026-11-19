//! The DocBook export PRD, §11 test 25 (`docbook_large_book_is_bounded`): books of about 100
//! and 1,000 pages export within the budgets the e-book exporters are held to, with the work on
//! workers rather than the front end's thread, and the larger takes not much more than ten
//! times the smaller: assembling a book is not superlinear in its pages. A file of its own, so
//! the process's peak memory is these exports' alone.

#![cfg(feature = "docbook")]

use std::{
    fmt::Write as _,
    fs,
    path::Path,
    time::{Duration, Instant},
};

use adoc_engine::{
    commands::ids, CommandArgs, DocBookExportArgs, Engine, EngineEvent, ExportTarget, Settings,
};

/// Pages that show a picture of their own, in each book.
const PICTURES: usize = 25;

/// Writes a component `name` of `pages` pages, listed in its navigation, under `folder`, with
/// [`PICTURES`] distinct pictures.
fn component(folder: &Path, name: &str, pages: usize) {
    let root = folder.join(name);
    let pages_folder = root.join("modules/ROOT/pages");
    let images = root.join("modules/ROOT/images");
    fs::create_dir_all(&pages_folder).unwrap();
    fs::create_dir_all(&images).unwrap();
    fs::write(
        root.join("antora.yml"),
        format!("name: {name}\ntitle: A Book of {pages} Pages\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n"),
    )
    .unwrap();
    let picture = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/docx/reference/manual/modules/ROOT/images/photo.jpg"),
    )
    .unwrap();
    let mut nav = String::new();
    for page in 0..pages {
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
        for section in 0..3 {
            let _ = write!(
                text,
                "[#s{section}]\n== Section {section}\n\nA paragraph of the section, with *strong* and _emphasised_ words and a footnote.footnote:[Note {page}.{section}.]\n\n\
                 * one\n* two\n** two and a half\n\n\
                 NOTE: A note in section {section}.\n\n[source,rust]\n----\nfn section_{section}() {{}} // <1>\n----\n<1> A function.\n\n\
                 [%header]\n|===\n|Key |Value\n|one |1\n|two |2\n|===\n\n"
            );
        }
        if page > 0 {
            let _ = writeln!(text, "See xref:page-{}.adoc#s1[the page before].", page - 1);
        }
        fs::write(pages_folder.join(format!("page-{page}.adoc")), text).unwrap();
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

/// Exports component `name` as the app does (the command returns at once, the front end pumps
/// until the export finishes), checks the document, and gives how long it took, how long the
/// longest pump took, and how long the command took.
fn export(
    engine: &mut Engine,
    events: &std::sync::mpsc::Receiver<EngineEvent>,
    folder: &Path,
    name: &str,
    pages: usize,
) -> (Duration, Duration, Duration) {
    let output = folder.join(format!("{name}.xml"));
    let started = Instant::now();
    engine
        .invoke(
            ids::EXPORT_DOCBOOK_COMPONENT,
            CommandArgs::ExportDocBook(DocBookExportArgs::new(
                ExportTarget::Component {
                    name: name.to_owned(),
                    version: Some("1.0".to_owned()),
                },
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
    assert_eq!(summary.pages, pages);
    assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);
    let xml = fs::read_to_string(&output).unwrap();
    assert_eq!(adoc_docbook::check(&xml), Vec::<String>::new());
    let document = adoc_docbook::testing::parse(&xml);
    let chapters = document
        .descendants()
        .filter(|node| node.has_tag_name((adoc_docbook::NAMESPACE, "chapter")))
        .count();
    assert_eq!(chapters, pages);
    assert_eq!(
        fs::read_dir(folder.join(format!("{name}_assets")))
            .unwrap()
            .count(),
        PICTURES
    );
    eprintln!(
        "DocBook, {pages} pages: invoke {invoked:?}, longest pump {longest_pump:?}, whole \
         {elapsed:?}, {} bytes",
        xml.len()
    );
    (elapsed, longest_pump, invoked)
}

#[test]
fn docbook_large_book_is_bounded() {
    let directory = tempfile::tempdir().unwrap();
    component(directory.path(), "hundred", 100);
    component(directory.path(), "thousand", 1000);
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    let events = engine.subscribe();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(300)));
    let before = peak_memory();

    let (small, small_pump, small_invoke) =
        export(&mut engine, &events, directory.path(), "hundred", 100);
    let (large, large_pump, large_invoke) =
        export(&mut engine, &events, directory.path(), "thousand", 1000);
    let peak = peak_memory();
    eprintln!(
        "peak memory {:?} MB (before the exports {:?} MB)",
        peak.map(|bytes| bytes / 1_000_000),
        before.map(|bytes| bytes / 1_000_000),
    );

    // The budgets the e-book exporters are held to (`docs/qa/spike-kindle.md`), for a debug
    // build on a CI runner.
    for invoked in [small_invoke, large_invoke] {
        assert!(
            invoked < Duration::from_secs(3),
            "the command took {invoked:?}"
        );
    }
    for pump in [small_pump, large_pump] {
        assert!(
            pump < Duration::from_secs(3),
            "a pump took {pump:?}: the front end would stall"
        );
    }
    assert!(
        large < Duration::from_secs(180),
        "1,000 pages took {large:?}"
    );
    // Ten times the pages in not much more than ten times the time, with room for a runner's
    // noise: nothing grows with the square of the pages.
    assert!(
        large < small * 25 + Duration::from_secs(5),
        "100 pages took {small:?} and 1,000 took {large:?}"
    );
    if let Some(peak) = peak {
        assert!(peak < 2_000_000_000, "peak memory {} MB", peak / 1_000_000);
    }
}
