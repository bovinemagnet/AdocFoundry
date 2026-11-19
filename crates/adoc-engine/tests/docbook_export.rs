//! DocBook export through the engine, over the publication plan the other formats use
//! (`docs/prd/PRD-AdocFoundry_DocBook_Export.md`, DocBook 2/9). Every document written is
//! checked against the DocBook 5.2.1 RELAX NG schema.

#![cfg(feature = "docbook")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, docbook_assets_folder, CommandArgs, DocBookExportArgs, Engine, EngineError,
    ExportFormat, ExportTarget, ProtectionRefusal, Settings,
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

/// The book fixture copied to a temporary directory and indexed, with `settings` changed.
fn workspace_with(change: impl FnOnce(&mut Settings)) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    change(&mut settings);
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

fn book_workspace() -> (tempfile::TempDir, PathBuf, Engine) {
    workspace_with(|_| {})
}

fn page(root: &Path, relative: &str) -> PathBuf {
    root.join("guide/modules").join(relative)
}

fn book() -> ExportTarget {
    ExportTarget::Component {
        name: "guide".to_owned(),
        version: Some("1.0".to_owned()),
    }
}

fn docbook(target: ExportTarget, output: &Path) -> DocBookExportArgs {
    DocBookExportArgs::new(target, output.to_path_buf())
}

/// The document written at `path`, checked against the schema, and its text.
fn document_text(path: &Path) -> String {
    let xml = fs::read_to_string(path).unwrap();
    adoc_docbook::testing::assert_valid(&xml);
    adoc_docbook::testing::parse(&xml)
        .descendants()
        .filter(roxmltree::Node::is_text)
        .filter_map(|node| node.text())
        .collect()
}

/// An output that is not an `.xml` file is refused before anything is read, and nothing is
/// written.
#[test]
fn docbook_outputs_must_be_xml_files() {
    let (directory, root, mut engine) = book_workspace();
    let target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));
    for name in ["book.dbk", "book.pdf", "no-extension", "book.xml.zip"] {
        let output = directory.path().join(name);
        let result = engine.export_docbook_blocking(docbook(target.clone(), &output), WAIT);
        assert!(
            matches!(result, Err(EngineError::InvalidRequest(_))),
            "{name}: {result:?}"
        );
        assert!(!output.exists(), "{name}");
    }
    // Any letter case of the extension is XML.
    let output = directory.path().join("book.XML");
    engine
        .export_docbook_blocking(docbook(target, &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(document_text(&output).contains("Install"));
    assert_eq!(
        docbook_assets_folder(&output),
        directory.path().join("book_assets")
    );
}

/// The pages a PDF and a DocBook document of `target` hold, and the document written.
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
    let output = directory.join("out.xml");
    let summary = engine
        .export_docbook_blocking(docbook(target, &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(summary.output, output);
    (pdf.sources, summary.sources, output)
}

/// PRD §11 test 4: for an article, a component version as a book and navigation files given
/// in an order, DocBook takes the pages PDF takes, in the same order, unsaved buffers included.
/// The DocBook export reaches neither `adoc-pdf` nor Typst (`docbook_builds_alone`).
#[cfg(feature = "typst")]
#[test]
fn docbook_and_pdf_select_the_same_pages() {
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

    let (pdf, docbook, output) = both(&mut engine, folder, ExportTarget::Page(id));
    assert_eq!(pdf, std::slice::from_ref(&use_page));
    assert_eq!(docbook, pdf);
    assert!(document_text(&output).contains("An unsaved sentence."));
    assert!(!fs::read_to_string(&use_page).unwrap().contains("unsaved"));

    let (pdf, docbook, output) = both(&mut engine, folder, book());
    assert_eq!(pdf.len(), 8);
    assert_eq!(docbook, pdf);
    assert!(
        document_text(&output).contains("An unsaved sentence."),
        "a book reads the open buffer too"
    );

    let (pdf, docbook, _) = both(
        &mut engine,
        folder,
        ExportTarget::File(page(&root, "admin/pages/backup.adoc")),
    );
    assert_eq!(docbook, pdf);

    let (pdf, docbook, output) = both(
        &mut engine,
        folder,
        ExportTarget::Navigation(vec![
            page(&root, "admin/nav.adoc"),
            page(&root, "ROOT/nav.adoc"),
        ]),
    );
    assert_eq!(pdf.first(), Some(&page(&root, "ROOT/pages/index.adoc")));
    assert_eq!(docbook, pdf);
    // The book holds the pages in that order.
    let text = document_text(&output);
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(at("Backup") < at("Install"));
}

fn contents(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

/// What `folder` holds, by name.
fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// PRD §11 test 6: a failed export, a link, a wrong extension, a folder, a folder that refuses
/// the write and an assets folder that is not the export's own each leave the existing
/// document and its folder as they were; a successful export replaces both, leaving nothing
/// staged.
#[test]
fn docbook_failed_export_keeps_existing_output() {
    let (directory, root, mut engine) = book_workspace();
    let folder = directory.path();
    let existing = folder.join("existing.xml");
    fs::write(&existing, b"the previous document").unwrap();
    let page_target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));

    // A target that does not exist.
    let error = engine
        .export_docbook_blocking(
            docbook(
                ExportTarget::Component {
                    name: "missing".to_owned(),
                    version: None,
                },
                &existing,
            ),
            WAIT,
        )
        .unwrap_err();
    assert!(matches!(error, EngineError::NotFound(_)), "{error:?}");
    assert_eq!(contents(&existing).unwrap(), b"the previous document");

    // A folder at the output.
    let folder_output = folder.join("folder.xml");
    fs::create_dir(&folder_output).unwrap();
    assert!(engine
        .export_docbook_blocking(docbook(page_target.clone(), &folder_output), WAIT)
        .is_err());
    assert!(folder_output.is_dir());

    // An assets folder holding a file the export did not write is never replaced.
    let assets = folder.join("existing_assets");
    fs::create_dir(&assets).unwrap();
    fs::write(assets.join("i1.png"), b"a picture").unwrap();
    fs::write(assets.join("notes.txt"), b"the user's notes").unwrap();
    let error = engine
        .export_docbook_blocking(docbook(page_target.clone(), &existing), WAIT)
        .unwrap_err();
    assert!(
        matches!(&error, EngineError::InvalidRequest(message) if message.contains("notes.txt")),
        "{error:?}"
    );
    assert_eq!(contents(&existing).unwrap(), b"the previous document");
    assert_eq!(names(&assets), ["i1.png", "notes.txt"]);
    // Nor is one holding a folder, or a file where the folder would be.
    fs::remove_file(assets.join("notes.txt")).unwrap();
    fs::create_dir(assets.join("i2.png")).unwrap();
    assert!(engine
        .export_docbook_blocking(docbook(page_target.clone(), &existing), WAIT)
        .is_err());
    fs::remove_dir(assets.join("i2.png")).unwrap();
    let stray = folder.join("stray.xml");
    fs::write(folder.join("stray_assets"), b"a file").unwrap();
    assert!(engine
        .export_docbook_blocking(docbook(page_target.clone(), &stray), WAIT)
        .is_err());
    assert!(!stray.exists());

    #[cfg(unix)]
    {
        // A link at the output, or as its assets folder, is never written through.
        let elsewhere = folder.join("elsewhere.xml");
        fs::write(&elsewhere, b"another document").unwrap();
        let link = folder.join("link.xml");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let error = engine
            .export_docbook_blocking(docbook(page_target.clone(), &link), WAIT)
            .unwrap_err();
        assert!(matches!(error, EngineError::InvalidRequest(_)), "{error:?}");
        assert_eq!(contents(&elsewhere).unwrap(), b"another document");

        let pictures = folder.join("pictures");
        fs::create_dir(&pictures).unwrap();
        let linked = folder.join("linked.xml");
        std::os::unix::fs::symlink(&pictures, folder.join("linked_assets")).unwrap();
        let error = engine
            .export_docbook_blocking(docbook(page_target.clone(), &linked), WAIT)
            .unwrap_err();
        assert!(matches!(error, EngineError::InvalidRequest(_)), "{error:?}");
        assert!(!linked.exists());

        // A folder that refuses the write: the document there is left as it was, and nothing
        // is staged in it.
        use std::os::unix::fs::PermissionsExt;
        let locked = folder.join("locked");
        fs::create_dir(&locked).unwrap();
        let inside = locked.join("out.xml");
        fs::write(&inside, b"the locked document").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = engine.export_docbook_blocking(docbook(page_target.clone(), &inside), WAIT);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "{result:?}");
        assert_eq!(contents(&inside).unwrap(), b"the locked document");
        assert_eq!(names(&locked), ["out.xml"], "nothing staged is left");
    }

    // A successful export replaces the document whole. The page has no pictures, so the
    // assets folder left by an earlier export, holding only what an export writes, goes, and
    // nothing staged is left.
    let summary = engine
        .export_docbook_blocking(docbook(page_target, &existing), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(summary.pages, 1);
    assert!(!summary.protected);
    assert!(document_text(&existing).contains("Install"));
    assert!(
        !assets.exists(),
        "the old pictures went with the old document"
    );
    let staged: Vec<String> = names(folder)
        .into_iter()
        .filter(|name| name.starts_with(".adocfoundry-docbook-") || name.starts_with(".tmp"))
        .collect();
    assert_eq!(staged, Vec::<String>::new());
}

/// A page that asks to be read-only is refused before anything is read: DocBook carries no
/// editing restriction (READONLY PRD).
#[test]
fn docbook_refuses_a_read_only_page() {
    let (directory, root, mut engine) = book_workspace();
    let protected = page(&root, "ROOT/pages/install.adoc");
    let text = fs::read_to_string(&protected).unwrap();
    let (title, rest) = text.split_once('\n').unwrap();
    fs::write(
        &protected,
        format!("{title}\n:export-read-only: true\n{rest}"),
    )
    .unwrap();
    let id = engine.open(&protected).unwrap();
    let output = directory.path().join("install.xml");
    let refused = engine.invoke(
        ids::EXPORT_DOCBOOK_PAGE,
        CommandArgs::ExportDocBook(docbook(ExportTarget::Page(id), &output)),
    );
    match refused {
        Err(EngineError::ExportProtection(refusal)) => {
            assert!(
                matches!(
                    *refusal,
                    ProtectionRefusal::UnsupportedFormat {
                        format: ExportFormat::DocBook,
                        ..
                    }
                ),
                "{refusal:?}"
            );
            assert!(refusal.to_string().contains("DocBook"), "{refusal}");
        }
        other => panic!("{other:?}"),
    }
    assert!(engine.wait_for_workers(WAIT));
    assert!(!output.exists());
}

/// PRD §5.4: a DocBook export starts no program. A Graphviz diagram the cache does not hold
/// keeps its source, with a warning, and the `dot` configured is never run; the same page
/// exported to PDF would run it.
#[cfg(unix)]
#[test]
fn docbook_starts_no_diagram_program() {
    let tools = tempfile::tempdir().unwrap();
    let marker = tools.path().join("ran");
    let dot = tools.path().join("dot");
    fs::write(
        &dot,
        format!("#!/bin/sh\ntouch '{}'\necho '<svg/>'\n", marker.display()),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dot, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (directory, root, mut engine) = workspace_with(|settings| {
        settings.tools.dot = dot.display().to_string();
    });
    let diagram_page = page(&root, "ROOT/pages/diagram.adoc");
    fs::write(
        &diagram_page,
        "= Diagram\n\n[graphviz]\n....\ndigraph { a -> b }\n....\n",
    )
    .unwrap();
    let output = directory.path().join("diagram.xml");
    let summary = engine
        .export_docbook_blocking(docbook(ExportTarget::File(diagram_page), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(engine.wait_for_workers(WAIT));
    assert!(!marker.exists(), "dot was run");
    assert!(
        summary
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("starts no program")),
        "{:?}",
        summary.diagnostics
    );
    assert!(document_text(&output).contains("digraph { a -> b }"));
}
