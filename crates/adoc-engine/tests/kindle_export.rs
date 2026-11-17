//! Kindle export through the engine, over the publication plan PDF and Word export use
//! (`docs/prd/PRD-AdocFoundry_Kindle_Export.md`).

#![cfg(feature = "kindle")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineError, ExportTarget, KindleExportArgs, KindleFormat,
    Settings,
};

const WAIT: Duration = Duration::from_secs(120);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The book fixture copied to a temporary directory and indexed.
fn book_workspace() -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

fn page(root: &Path, relative: &str) -> PathBuf {
    root.join("guide/modules").join(relative)
}

/// An output whose extension is not the format's is refused before anything is read, and
/// nothing is written in either format's place.
#[test]
fn kindle_outputs_must_carry_their_format_extension() {
    let (directory, root, mut engine) = book_workspace();
    let target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));
    for (format, name) in [
        (KindleFormat::Kfx, "book.azw3"),
        (KindleFormat::Azw3, "book.kfx"),
        (KindleFormat::Azw3, "book.epub"),
        (KindleFormat::Kfx, "no-extension"),
    ] {
        let output = directory.path().join(name);
        let result = engine.export_kindle_blocking(
            KindleExportArgs::new(target.clone(), format, output.clone()),
            WAIT,
        );
        assert!(
            matches!(result, Err(EngineError::InvalidRequest(_))),
            "{name}: {result:?}"
        );
        assert!(!output.exists(), "{name}");
    }
}

fn book() -> ExportTarget {
    ExportTarget::Component {
        name: "guide".to_owned(),
        version: Some("1.0".to_owned()),
    }
}

/// The pages a PDF and an AZW3 of `target` hold, and the AZW3 written.
#[cfg(feature = "typst")]
fn both(
    engine: &mut Engine,
    directory: &Path,
    target: ExportTarget,
) -> (Vec<PathBuf>, Vec<PathBuf>, PathBuf) {
    let pdf = engine
        .export_blocking(
            adoc_engine::ExportArgs {
                target: target.clone(),
                output: directory.join("out.pdf"),
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
                attributes: Vec::new(),
            },
            WAIT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let output = directory.join("out.azw3");
    let kindle = engine
        .export_kindle_blocking(
            KindleExportArgs::new(target, KindleFormat::Azw3, output.clone()),
            WAIT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    (pdf.sources, kindle.sources, output)
}

/// The book's text as a reader finds it.
fn book_text(path: &Path) -> String {
    adoc_kindle::testing::read_azw3(&fs::read(path).unwrap())
        .unwrap()
        .plain_text()
}

/// PRD §12 test 1: for an article, a component version as a book and navigation files given
/// in an order, Kindle takes the pages PDF takes, in the same order, unsaved buffers included.
/// The Kindle export reaches neither `adoc-pdf` nor Typst (`kindle_builds_alone`).
#[cfg(feature = "typst")]
#[test]
fn kindle_and_pdf_select_the_same_pages() {
    let (directory, root, mut engine) = book_workspace();
    let folder = directory.path();

    let use_page = page(&root, "ROOT/pages/use.adoc");
    let id = engine.open(&use_page).unwrap();
    let end = engine.document(id).unwrap().text.to_string().len();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(end, end),
                text: "\nAn unsaved sentence.\n".to_owned(),
            },
        )
        .unwrap();

    let (pdf, kindle, output) = both(&mut engine, folder, ExportTarget::Page(id));
    assert_eq!(pdf, std::slice::from_ref(&use_page));
    assert_eq!(kindle, pdf);
    assert!(book_text(&output).contains("An unsaved sentence."));
    assert!(!fs::read_to_string(&use_page).unwrap().contains("unsaved"));

    let (pdf, kindle, output) = both(&mut engine, folder, book());
    assert_eq!(pdf.len(), 8);
    assert_eq!(kindle, pdf);
    assert!(
        book_text(&output).contains("An unsaved sentence."),
        "a book reads the open buffer too"
    );

    let (pdf, kindle, _) = both(
        &mut engine,
        folder,
        ExportTarget::File(page(&root, "admin/pages/backup.adoc")),
    );
    assert_eq!(kindle, pdf);

    let (pdf, kindle, output) = both(
        &mut engine,
        folder,
        ExportTarget::Navigation(vec![
            page(&root, "admin/nav.adoc"),
            page(&root, "ROOT/nav.adoc"),
        ]),
    );
    assert_eq!(pdf.first(), Some(&page(&root, "ROOT/pages/index.adoc")));
    assert_eq!(kindle, pdf);
    // The book holds the pages in that order.
    let text = book_text(&output);
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(at("Backup") < at("Install"));
}

/// A failed Kindle export leaves an existing file as it was.
#[test]
fn a_refused_kindle_export_keeps_the_existing_file() {
    let (directory, root, mut engine) = book_workspace();
    let existing = directory.path().join("existing.kfx");
    fs::write(&existing, b"the previous book").unwrap();
    let result = engine.export_kindle_blocking(
        KindleExportArgs::new(
            ExportTarget::File(page(&root, "ROOT/pages/install.adoc")),
            KindleFormat::Kfx,
            existing.clone(),
        ),
        WAIT,
    );
    // Until KFX is written, the export fails as unsupported; either way, never as AZW3.
    if result.is_err() {
        assert_eq!(fs::read(&existing).unwrap(), b"the previous book");
    } else {
        assert_eq!(&fs::read(&existing).unwrap()[..4], b"CONT");
    }
}
