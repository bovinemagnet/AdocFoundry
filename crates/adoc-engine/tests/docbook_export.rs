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

/// `node`'s structural children as `element: title`, nested by indentation.
fn outline(node: roxmltree::Node<'_, '_>, depth: usize, out: &mut Vec<String>) {
    for child in node.children().filter(roxmltree::Node::is_element) {
        let name = child.tag_name().name();
        if matches!(name, "preface" | "part" | "chapter" | "section") {
            let title: String = child
                .children()
                .find(|child| child.has_tag_name((adoc_docbook::NAMESPACE, "title")))
                .map(|title| {
                    title
                        .descendants()
                        .filter(roxmltree::Node::is_text)
                        .filter_map(|node| node.text())
                        .collect()
                })
                .unwrap_or_default();
            out.push(format!("{}{name}: {title}", "  ".repeat(depth)));
            outline(child, depth + 1, out);
        }
    }
}

/// The document at `path`, checked, as its outline and its `info` element's text by child.
fn outline_and_info(path: &Path) -> (Vec<String>, Vec<(String, String)>) {
    let xml = fs::read_to_string(path).unwrap();
    adoc_docbook::testing::assert_valid(&xml);
    let document = adoc_docbook::testing::parse(&xml);
    let mut lines = Vec::new();
    outline(document.root_element(), 0, &mut lines);
    let info = document.root_element().first_element_child().expect("info");
    let info = info
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|child| {
            let text: String = child
                .descendants()
                .filter(roxmltree::Node::is_text)
                .filter_map(|node| node.text())
                .collect();
            (child.tag_name().name().to_owned(), text)
        })
        .collect();
    (lines, info)
}

/// PRD §11 test 7: a component version is a book that follows the plan PDF export follows:
/// the start page the navigation does not list is the preface, each navigation list's title a
/// part, top-level pages chapters, nested pages and headings sections, each page once. In a
/// workspace of several components and versions, the version asked for is the one exported,
/// with its own metadata.
#[test]
fn docbook_book_hierarchy_follows_the_plan() {
    let (directory, _root, mut engine) = book_workspace();
    let output = directory.path().join("guide.xml");
    engine
        .export_docbook_blocking(docbook(book(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let (lines, info) = outline_and_info(&output);
    assert_eq!(
        lines,
        [
            "preface: Welcome",
            "part: Getting Started",
            "  chapter: Install",
            "    section: Requirements",
            "    section: Configure",
            "      section: Options",
            "  chapter: Use",
            "part: Administration",
            "  chapter: Administering the Guide",
            "  chapter: Backup",
            "  chapter: Recovery",
            "    section: Restore",
            "  chapter: Empty Page",
        ]
    );
    assert_eq!(
        info,
        [
            ("title".to_owned(), "The Guide".to_owned()),
            ("releaseinfo".to_owned(), "1.0".to_owned()),
        ]
    );

    // Several components and versions: each version is its own book.
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().join("reference");
    copy_tree(&fixture("docx/reference"), &root);
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let manual = |version: &str| ExportTarget::Component {
        name: "manual".to_owned(),
        version: Some(version.to_owned()),
    };
    let output = workspace.path().join("manual-2.xml");
    engine
        .export_docbook_blocking(docbook(manual("2.0"), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let (lines, info) = outline_and_info(&output);
    let chapters: Vec<&str> = lines
        .iter()
        .filter(|line| !line.starts_with("    "))
        .map(String::as_str)
        .collect();
    assert_eq!(
        chapters,
        [
            "preface: The Manual",
            "part: Using the product",
            "  chapter: Installing",
            "  chapter: Media",
            "  chapter: Links",
            "part: Administration",
            "  chapter: Security",
        ],
        "{lines:#?}"
    );
    assert!(
        lines.contains(&"    section: Tables".to_owned()),
        "{lines:#?}"
    );
    assert!(info.contains(&("author".to_owned(), "Paul Snow".to_owned())));
    assert!(info.contains(&("releaseinfo".to_owned(), "2.0".to_owned())));
    let titles: Vec<&String> = lines.iter().collect();
    let unique: std::collections::BTreeSet<&String> = titles.iter().copied().collect();
    assert_eq!(unique.len(), titles.len(), "each page once: {lines:#?}");

    let output = workspace.path().join("manual-1.xml");
    engine
        .export_docbook_blocking(docbook(manual("1.0"), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let (lines, info) = outline_and_info(&output);
    assert_eq!(lines, ["chapter: The Old Manual"]);
    assert!(info.contains(&("releaseinfo".to_owned(), "1.0".to_owned())));
}

/// The reference fixture copied and indexed: several components and versions, pictures (PNG,
/// JPEG, SVG), a Mermaid diagram and formulas.
fn reference_workspace() -> (tempfile::TempDir, Engine) {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().join("reference");
    copy_tree(&fixture("docx/reference"), &root);
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (workspace, engine)
}

fn manual() -> ExportTarget {
    ExportTarget::Component {
        name: "manual".to_owned(),
        version: Some("2.0".to_owned()),
    }
}

/// Every `fileref` in the document at `output`.
fn filerefs(output: &Path) -> Vec<String> {
    let xml = fs::read_to_string(output).unwrap();
    adoc_docbook::testing::parse(&xml)
        .descendants()
        .filter_map(|node| node.attribute("fileref"))
        .map(str::to_owned)
        .collect()
}

/// PRD §11 test 12, through the engine: a book's pictures, its in-process diagram and its
/// drawn formulas are written once each into the assets folder beside the document, every
/// `fileref` leading there by a relative path, nothing naming the checkout; exporting again
/// with fewer pictures leaves only those; referred to where they are, nothing is copied.
#[test]
fn docbook_pictures_go_beside_the_document() {
    let (workspace, mut engine) = reference_workspace();
    let out = workspace.path().join("out");
    let output = out.join("manual.xml");
    let summary = engine
        .export_docbook_blocking(docbook(manual(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    document_text(&output);
    let assets = out.join("manual_assets");
    let refs = filerefs(&output);
    assert!(refs.len() >= 5, "{refs:?}");
    for fileref in &refs {
        assert!(fileref.starts_with("manual_assets/i"), "{fileref}");
        assert!(out.join(fileref).is_file(), "{fileref} is written");
    }
    let mut written = names(&assets);
    written.retain(|name| name.ends_with(".png") || name.ends_with(".jpg"));
    assert!(written.len() >= 2, "{written:?}");
    let distinct: std::collections::BTreeSet<&String> = refs.iter().collect();
    assert_eq!(
        names(&assets).len(),
        distinct.len(),
        "each picture once, and nothing else"
    );
    let xml = fs::read_to_string(&output).unwrap();
    assert!(!xml.contains(&*workspace.path().to_string_lossy()));
    assert!(
        summary
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("not as MathML")),
        "{:?}",
        summary.diagnostics
    );

    // Exported again as a page with one picture: the folder holds only that one.
    let root = workspace.path().join("reference/manual/modules/ROOT/pages");
    let page = root.join("one-picture.adoc");
    fs::write(&page, "= One Picture\n\nimage::logo.png[The logo]\n").unwrap();
    engine
        .export_docbook_blocking(docbook(ExportTarget::File(page.clone()), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(names(&assets), ["i1.png"]);
    assert_eq!(filerefs(&output), ["manual_assets/i1.png"]);

    // Referred to where they are: nothing copied, the old folder gone, paths from the document.
    let referenced = DocBookExportArgs {
        assets: Some(adoc_engine::DocBookAssets::Reference),
        ..docbook(ExportTarget::File(page), &output)
    };
    engine
        .export_docbook_blocking(referenced, WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!assets.exists());
    let refs = filerefs(&output);
    assert_eq!(refs.len(), 1);
    assert!(refs[0].starts_with("../reference/"), "{refs:?}");
    assert!(out.join(&refs[0]).is_file());
    adoc_docbook::testing::assert_valid_referenced(&fs::read_to_string(&output).unwrap());
}

/// PRD §11 test 16, through the engine: in the reference book, references between its pages
/// lead to their ids (an anchor in another component's page included), one to a component
/// outside the book is its text with a warning, a repeated footnote is a `footnoteref`, and
/// with the site's address known the outside reference leads there.
#[test]
fn docbook_internal_links_resolve() {
    let (workspace, mut engine) = reference_workspace();
    let output = workspace.path().join("manual.xml");
    let summary = engine
        .export_docbook_blocking(docbook(manual(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let xml = fs::read_to_string(&output).unwrap();
    adoc_docbook::testing::assert_valid(&xml);
    let document = adoc_docbook::testing::parse(&xml);
    let ids: std::collections::BTreeSet<&str> = document
        .descendants()
        .filter_map(|node| node.attribute(("http://www.w3.org/XML/1998/namespace", "id")))
        .collect();
    for id in [
        "manual-2.0-ROOT-index",
        "manual-2.0-ROOT-install",
        "manual-2.0-ROOT-install.requirements",
        "manual-2.0-admin-security",
    ] {
        assert!(ids.contains(id), "{id} in {ids:?}");
    }
    let links: Vec<(&str, String)> = document
        .descendants()
        .filter(|node| node.has_tag_name((adoc_docbook::NAMESPACE, "link")))
        .filter_map(|node| {
            let text: String = node
                .descendants()
                .filter(roxmltree::Node::is_text)
                .filter_map(|node| node.text())
                .collect();
            Some((node.attribute("linkend")?, text))
        })
        .collect();
    assert!(
        links.contains(&(
            "manual-2.0-ROOT-install.requirements",
            "the requirements".to_owned()
        )),
        "{links:?}"
    );
    assert!(
        links.contains(&("manual-2.0-admin-security", "security".to_owned())),
        "{links:?}"
    );
    assert!(document
        .descendants()
        .any(|node| node.has_tag_name((adoc_docbook::NAMESPACE, "footnoteref"))));
    assert!(
        summary.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("api::index.adoc leads outside the publication")),
        "{:?}",
        summary.diagnostics
    );
    assert!(xml.contains("the API reference"));

    // With the site's address known, the reference outside the book leads to its page there.
    fs::write(
        workspace.path().join("reference/.adocfoundry.toml"),
        "[pdf]\nsite_url = \"https://docs.example.com\"\n",
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&workspace.path().join("reference"))).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine
        .open_workspace(vec![workspace.path().join("reference")])
        .unwrap();
    assert!(engine.wait_for_workers(WAIT));
    engine
        .export_docbook_blocking(docbook(manual(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let xml = fs::read_to_string(&output).unwrap();
    assert!(
        xml.contains("xlink:href=\"https://docs.example.com/api/2.0/index.html\""),
        "{xml}"
    );
}

/// PRD §11 test 17: page, component and navigation profiles export the right documents;
/// `[docbook]` gives the defaults and a profile overrides them; invalid profiles are refused as
/// the settings are read; an output outside the workspace, a missing component and an assets
/// folder that is not the export's own fail explicitly.
#[test]
fn docbook_profiles_resolve_and_validate() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    fs::create_dir_all(root.join("taken_assets")).unwrap();
    fs::write(root.join("taken_assets/notes.txt"), "mine").unwrap();
    fs::write(
        root.join(".adocfoundry.toml"),
        r#"
[docbook]
assets = "reference"

[docbook.profiles.page]
page = "guide/modules/ROOT/pages/install.adoc"
output = "build/install.xml"
assets = "copy"

[docbook.profiles.handbook]
component = "1.0@guide"

[docbook.profiles.admin]
nav = ["guide/modules/admin/nav.adoc"]
output = "build/admin.xml"

[docbook.profiles.away]
page = "guide/modules/ROOT/pages/install.adoc"
output = "../away.xml"

[docbook.profiles.taken]
page = "guide/modules/ROOT/pages/install.adoc"
output = "taken.xml"

[docbook.profiles.wrong]
page = "guide/modules/ROOT/pages/install.adoc"
output = "wrong.dbk"

[docbook.profiles.odd]
page = "guide/modules/ROOT/pages/install.adoc"
theme = "default"

[docbook.profiles.bad-assets]
page = "guide/modules/ROOT/pages/install.adoc"
assets = "move"

[docbook.profiles.unknown]
component = "nowhere"
"#,
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    // Invalid profiles are refused when the settings are read.
    for name in [
        "docbook.profiles.wrong",
        "docbook.profiles.odd",
        "docbook.profiles.bad-assets",
    ] {
        assert!(
            settings
                .warnings
                .iter()
                .any(|warning| warning.contains(name)),
            "{name}: {:?}",
            settings.warnings
        );
    }
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(
        engine.docbook_profiles(),
        ["admin", "away", "handbook", "page", "taken", "unknown"]
    );

    // The page profile: its own output, and its own asset policy over `[docbook]`'s.
    let args = engine.docbook_profile_export_args("page").unwrap();
    assert_eq!(args.output, root.join("build/install.xml"));
    assert_eq!(args.assets, Some(adoc_engine::DocBookAssets::Copy));
    let summary = engine.export_docbook_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources, [page(&root, "ROOT/pages/install.adoc")]);
    assert!(document_text(&root.join("build/install.xml")).contains("Install"));

    // The component profile: `<name>.xml` at the root, the whole book, `[docbook]`'s policy.
    let args = engine.docbook_profile_export_args("handbook").unwrap();
    assert_eq!(args.output, root.join("handbook.xml"));
    assert_eq!(args.assets, None, "the default is applied on export");
    let summary = engine.export_docbook_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources.len(), 8);

    // The navigation profile: the start page as a preface, then the administration guide.
    let args = engine.docbook_profile_export_args("admin").unwrap();
    let summary = engine.export_docbook_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources[0], page(&root, "ROOT/pages/index.adoc"));
    let admin = root.join("guide/modules/admin");
    assert!(summary.sources[1..]
        .iter()
        .all(|source| source.starts_with(&admin)));

    // Through the command, as the app and MCP server export a profile.
    fs::remove_file(root.join("build/install.xml")).unwrap();
    engine
        .invoke(
            ids::EXPORT_DOCBOOK_PROFILE,
            CommandArgs::Profile("page".to_owned()),
        )
        .unwrap();
    assert!(engine.wait_for_workers(WAIT));
    for _ in 0..200 {
        engine.pump();
        if root.join("build/install.xml").is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(root.join("build/install.xml").is_file());

    // An output outside the workspace is refused; so, found without exporting, are a missing
    // component and an assets folder holding another's files.
    let error = engine.docbook_profile_export_args("away").unwrap_err();
    assert!(matches!(error, EngineError::Refused(_)), "{error:?}");
    assert!(!directory.path().join("away.xml").exists());
    assert!(engine.docbook_profile_problems("page").is_empty());
    let problems = engine.docbook_profile_problems("unknown");
    assert!(
        problems.iter().any(|problem| problem.contains("nowhere")),
        "{problems:?}"
    );
    let problems = engine.docbook_profile_problems("taken");
    assert!(
        problems.iter().any(|problem| problem.contains("notes.txt")),
        "{problems:?}"
    );
    assert!(engine.docbook_profile_export_args("missing").is_err());
    assert!(engine.docbook_profile_export_args("wrong").is_err());
}

/// PRD §11 test 18: one book published as PDF and as DocBook through their own profiles takes
/// the same pages in the same order, and neither format's settings change the other's output.
#[cfg(feature = "typst")]
#[test]
fn docbook_settings_leave_pdf_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    let open = |settings_text: &str| {
        fs::write(root.join(".adocfoundry.toml"), settings_text).unwrap();
        let mut settings = Settings::load(None, Some(&root)).unwrap();
        settings.git.enabled = false;
        let mut engine = Engine::new(settings);
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(WAIT));
        engine
    };
    let profiles = r#"
[pdf.profiles.guide]
component = "1.0@guide"
output = "guide.pdf"

[docbook.profiles.guide]
component = "1.0@guide"
output = "guide.xml"
"#;
    let mut engine = open(profiles);
    let pdf_settings = engine.settings().pdf.clone();
    let pdf_args = engine.profile_export_args("guide").unwrap();
    assert_eq!(pdf_args.output, root.join("guide.pdf"));
    let pdf = engine
        .export_blocking(pdf_args, WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let docbook_args = engine.docbook_profile_export_args("guide").unwrap();
    let profiled = engine
        .export_docbook_blocking(docbook_args, WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        profiled.sources, pdf.sources,
        "the same pages, in the same order"
    );
    let document = fs::read(root.join("guide.xml")).unwrap();

    // `[pdf]` changed leaves the DocBook as it was; `[docbook]` changed leaves the PDF's
    // settings and profile as they were.
    let mut engine = open(&format!(
        "{profiles}\n[pdf]\ntheme = \"default-with-font-fallbacks\"\nsite_url = \"\"\n"
    ));
    engine
        .export_docbook_blocking(engine.docbook_profile_export_args("guide").unwrap(), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(fs::read(root.join("guide.xml")).unwrap(), document);
    let engine = open(&format!("{profiles}\n[docbook]\nassets = \"reference\"\n"));
    let after = engine.profile_export_args("guide").unwrap();
    assert_eq!(after.output, root.join("guide.pdf"));
    assert_eq!(after.theme, None);
    assert_eq!(engine.settings().pdf, pdf_settings);
}
