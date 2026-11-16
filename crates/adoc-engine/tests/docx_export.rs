//! DOCX 2/8: the engine's DOCX export over the publication plan PDF export uses. PRD
//! `docs/prd/PRD-AdocFoundry_DOCX_Export.md` §10, tests 4 (`docx_and_pdf_select_the_same_pages`)
//! and 5 (`docx_failed_export_keeps_existing_output`).

#![cfg(feature = "docx")]

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, CommandArgs, DocxExportArgs, Engine, EngineError, EngineEvent, ExportTarget,
    Settings,
};

const WAIT: Duration = Duration::from_secs(120);
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

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
    book_workspace_with("")
}

/// The book fixture with `workspace_settings` as its `.adocfoundry.toml`.
fn book_workspace_with(workspace_settings: &str) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    fs::write(root.join(".adocfoundry.toml"), workspace_settings).unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    // Mermaid, and formulas as a default engine renders them.
    let diagrams = adoc_diagrams::Diagrams::with_providers(
        adoc_diagrams::DiagramCache::new(directory.path().join("cache")),
        vec![Box::new(adoc_diagrams::MermaidProvider::default())],
    );
    #[cfg(feature = "typst")]
    let diagrams = diagrams.with_provider(Box::new(adoc_engine::StemProvider));
    engine.set_diagrams(diagrams);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

fn docx(target: ExportTarget, output: &Path) -> DocxExportArgs {
    DocxExportArgs {
        target,
        output: output.to_path_buf(),
        attributes: Vec::new(),
        paper: None,
        landscape: None,
    }
}

fn book() -> ExportTarget {
    ExportTarget::Component {
        name: "guide".to_owned(),
        version: Some("1.0".to_owned()),
    }
}

/// The package's `word/document.xml`, read through its ZIP central directory.
fn document_xml(path: &Path) -> String {
    part(path, "word/document.xml")
}

/// A part of the package at `path`, read through its ZIP central directory.
fn part(path: &Path, wanted: &str) -> String {
    let bytes = fs::read(path).unwrap();
    let end = bytes.len() - 22;
    assert_eq!(&bytes[end..end + 4], b"PK\x05\x06");
    let read16 = |at: usize| usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
    let read32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let mut at = read32(end + 16);
    for _ in 0..read16(end + 10) {
        let (compressed, name_length) = (read32(at + 20), read16(at + 28));
        let local = read32(at + 42);
        let name = &bytes[at + 46..at + 46 + name_length];
        at += 46 + name_length + read16(at + 30) + read16(at + 32);
        if name == wanted.as_bytes() {
            let start = local + 30 + read16(local + 26) + read16(local + 28);
            let mut text = String::new();
            flate2::read::DeflateDecoder::new(&bytes[start..start + compressed])
                .read_to_string(&mut text)
                .unwrap();
            return text;
        }
    }
    panic!("no {wanted} in {}", path.display());
}

/// The document's paragraphs: each one's style and text.
fn paragraphs(path: &Path) -> Vec<(Option<String>, String)> {
    let xml = document_xml(path);
    let document = roxmltree::Document::parse(&xml).unwrap();
    document
        .descendants()
        .filter(|node| node.has_tag_name((W, "p")))
        .map(|paragraph| {
            let style = paragraph
                .descendants()
                .find(|node| node.has_tag_name((W, "pStyle")))
                .and_then(|node| node.attribute((W, "val")))
                .map(str::to_owned);
            let text = paragraph
                .descendants()
                .filter(|node| node.has_tag_name((W, "t")))
                .filter_map(|node| node.text())
                .collect();
            (style, text)
        })
        .collect()
}

/// The headings of a document, as (level, text).
fn headings(path: &Path) -> Vec<(String, String)> {
    paragraphs(path)
        .into_iter()
        .filter_map(|(style, text)| Some((style?, text)))
        .collect()
}

fn page(root: &Path, relative: &str) -> PathBuf {
    root.join("guide/modules").join(relative)
}

#[test]
fn exports_a_page_as_an_article() {
    let (directory, root, mut engine) = book_workspace();
    let id = engine
        .open(&page(&root, "ROOT/pages/install.adoc"))
        .unwrap();
    let output = directory.path().join("install.docx");
    let summary = engine
        .export_docx_blocking(docx(ExportTarget::Page(id), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(summary.output, output);
    assert_eq!(summary.pages, 1);
    assert_eq!(summary.sources, [page(&root, "ROOT/pages/install.adoc")]);
    let headings = headings(&output);
    assert_eq!(headings[0], ("Title".to_owned(), "Install".to_owned()));
    assert!(
        headings.contains(&("Heading1".to_owned(), "Requirements".to_owned())),
        "{headings:?}"
    );
}

#[test]
fn exports_a_component_as_a_book_in_navigation_order() {
    let (directory, root, mut engine) = book_workspace();
    let output = directory.path().join("guide.docx");
    let summary = engine
        .export_docx_blocking(docx(book(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    // The start page leads as a preface, then the two nav files' pages as parts, each page
    // once, the entry without a link a heading of its own.
    assert_eq!(
        summary.sources,
        [
            page(&root, "ROOT/pages/index.adoc"),
            page(&root, "ROOT/pages/install.adoc"),
            page(&root, "ROOT/pages/configure.adoc"),
            page(&root, "ROOT/pages/use.adoc"),
            page(&root, "admin/pages/index.adoc"),
            page(&root, "admin/pages/backup.adoc"),
            page(&root, "admin/pages/restore.adoc"),
            page(&root, "admin/pages/empty.adoc"),
        ]
    );
    assert_eq!(summary.pages, summary.sources.len());
    let headings = headings(&output);
    assert_eq!(headings[0], ("Title".to_owned(), "The Guide".to_owned()));
    let parts: Vec<&str> = headings
        .iter()
        .filter(|(style, _)| style == "Heading1")
        .map(|(_, text)| text.as_str())
        .collect();
    assert!(parts.contains(&"Getting Started"), "{headings:?}");
    assert!(parts.contains(&"Administration"), "{headings:?}");
    assert!(
        headings.contains(&("Heading2".to_owned(), "Recovery".to_owned())),
        "{headings:?}"
    );
}

/// Exports `target` to both formats and returns the pages each took, in order.
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
    let output = directory.join("out.docx");
    let word = engine
        .export_docx_blocking(docx(target, &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    (pdf.sources, word.sources, output)
}

#[cfg(feature = "typst")]
#[test]
fn docx_and_pdf_select_the_same_pages() {
    let (directory, root, mut engine) = book_workspace();
    let folder = directory.path();

    // An open page with unsaved text: both read the buffer, not the file.
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

    let (pdf, word, output) = both(&mut engine, folder, ExportTarget::Page(id));
    assert_eq!(pdf, std::slice::from_ref(&use_page));
    assert_eq!(word, pdf);
    let text: String = paragraphs(&output)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(text.contains("An unsaved sentence."), "{text}");
    assert!(!fs::read_to_string(&use_page).unwrap().contains("unsaved"));

    let (pdf, word, output) = both(&mut engine, folder, book());
    assert_eq!(pdf.len(), 8);
    assert_eq!(word, pdf);
    let text: String = paragraphs(&output)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(
        text.contains("An unsaved sentence."),
        "a book reads the open buffer too"
    );

    let (pdf, word, _) = both(
        &mut engine,
        folder,
        ExportTarget::File(page(&root, "admin/pages/backup.adoc")),
    );
    assert_eq!(word, pdf);

    // Navigation files in the order given: the administration guide first this time.
    let (pdf, word, output) = both(
        &mut engine,
        folder,
        ExportTarget::Navigation(vec![
            page(&root, "admin/nav.adoc"),
            page(&root, "ROOT/nav.adoc"),
        ]),
    );
    assert_eq!(pdf.first(), Some(&page(&root, "ROOT/pages/index.adoc")));
    assert_eq!(pdf.get(1), Some(&page(&root, "admin/pages/index.adoc")));
    assert_eq!(word, pdf);
    // The document holds the pages in that order.
    let titles: Vec<String> = headings(&output)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    let at = |title: &str| titles.iter().position(|found| found == title).unwrap();
    assert!(at("Backup") < at("Install"), "{titles:?}");
}

/// The bytes at `path`, or none.
fn contents(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

#[test]
fn docx_failed_export_keeps_existing_output() {
    let (directory, root, mut engine) = book_workspace();
    let folder = directory.path();
    let existing = folder.join("existing.docx");
    fs::write(&existing, b"the previous document").unwrap();
    let page_target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));

    // A target that does not exist.
    let error = engine
        .export_docx_blocking(
            docx(
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

    // An output that is not a `.docx` file is refused before anything is read.
    let pdf = folder.join("existing.pdf");
    fs::write(&pdf, b"a PDF").unwrap();
    let error = engine
        .export_docx_blocking(docx(page_target.clone(), &pdf), WAIT)
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidRequest(_)), "{error:?}");
    assert_eq!(contents(&pdf).unwrap(), b"a PDF");

    // A folder at the output.
    let folder_output = folder.join("folder.docx");
    fs::create_dir(&folder_output).unwrap();
    assert!(engine
        .export_docx_blocking(docx(page_target.clone(), &folder_output), WAIT)
        .is_err());
    assert!(folder_output.is_dir());

    // A link at the output is never written through.
    #[cfg(unix)]
    {
        let elsewhere = folder.join("elsewhere.docx");
        fs::write(&elsewhere, b"another document").unwrap();
        let link = folder.join("link.docx");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let error = engine
            .export_docx_blocking(docx(page_target.clone(), &link), WAIT)
            .unwrap_err();
        assert!(matches!(error, EngineError::InvalidRequest(_)), "{error:?}");
        assert_eq!(contents(&elsewhere).unwrap(), b"another document");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    // A folder that refuses the write: the document there is left as it was.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = folder.join("locked");
        fs::create_dir(&locked).unwrap();
        let inside = locked.join("out.docx");
        fs::write(&inside, b"the locked document").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = engine.export_docx_blocking(docx(page_target.clone(), &inside), WAIT);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "{result:?}");
        assert_eq!(contents(&inside).unwrap(), b"the locked document");
        let left: Vec<_> = fs::read_dir(&locked).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary file is left behind");
    }

    // A successful export replaces the document whole.
    engine
        .export_docx_blocking(docx(page_target, &existing), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(contents(&existing).unwrap().starts_with(b"PK\x03\x04"));
    let names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".tmp"))
        .collect();
    assert_eq!(
        names,
        Vec::<String>::new(),
        "no temporary file is left behind"
    );
}

#[test]
fn the_command_reports_when_the_document_is_written() {
    let (directory, root, mut engine) = book_workspace();
    let id = engine
        .open(&page(&root, "ROOT/pages/install.adoc"))
        .unwrap();
    let output = directory.path().join("command.docx");
    let events = engine.subscribe();
    engine
        .invoke(
            ids::EXPORT_DOCX_PAGE,
            CommandArgs::ExportDocx(docx(ExportTarget::Page(id), &output)),
        )
        .unwrap();
    // The command returns at once; the document is written on workers.
    assert!(engine.is_exporting());
    assert!(engine.wait_for_workers(WAIT));
    assert!(!engine.is_exporting());
    let mut finished: BTreeMap<PathBuf, _> = events
        .try_iter()
        .filter_map(|event| match event {
            EngineEvent::ExportFinished { output, result } => Some((output, result)),
            _ => None,
        })
        .collect();
    let result = finished.remove(&output).expect("the export finished");
    assert_eq!(result.unwrap().sources.len(), 1);
    assert!(fs::read(&output).unwrap().starts_with(b"PK\x03\x04"));
}

#[test]
fn references_out_of_the_book_use_the_site_address() {
    let (directory, _root, mut engine) =
        book_workspace_with("[pdf]\nsite_url = \"https://docs.example.com/\"\n");
    let output = directory.path().join("site.docx");
    let summary = engine
        .export_docx_blocking(docx(book(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let relationships = part(&output, "word/_rels/document.xml.rels");
    assert!(
        relationships.contains(
            r#"Target="https://docs.example.com/other/index.html" TargetMode="External""#
        ),
        "{relationships}"
    );
    // A reference between the book's pages is a link inside the document.
    let document = document_xml(&output);
    assert!(
        document.contains(r#"<w:hyperlink w:anchor="_a"#),
        "{document}"
    );
    let outside: Vec<&str> = summary
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .filter(|message| message.contains("outside the document"))
        .collect();
    assert_eq!(outside, Vec::<&str>::new());

    // Without a site, the reference is text, and reported.
    let (directory, _root, mut engine) = book_workspace();
    let output = directory.path().join("no-site.docx");
    let summary = engine
        .export_docx_blocking(docx(book(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!part(&output, "word/_rels/document.xml.rels").contains("other/index.html"));
    assert!(document_xml(&output).contains("other component"));
    assert!(
        summary.diagnostics.iter().any(|diagnostic| diagnostic.message
            == "the cross reference other::index.adoc leads outside the document and is exported as text"),
        "{:?}",
        summary.diagnostics
    );
}

#[test]
fn diagrams_are_rendered_and_embedded() {
    let (directory, _root, mut engine) = book_workspace();
    let output = directory.path().join("diagrams.docx");
    let summary = engine
        .export_docx_blocking(docx(book(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let document = document_xml(&output);
    assert!(document.contains("<wp:inline"), "the diagram is a picture");
    assert!(
        !document.contains("graph LR"),
        "the diagram's source is not shown"
    );
    let relationships = part(&output, "word/_rels/document.xml.rels");
    assert!(
        relationships.contains(r#"Target="media/image1.png""#),
        "{relationships}"
    );
    assert!(
        summary
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("diagram")),
        "{:?}",
        summary.diagnostics
    );
}

#[test]
fn an_image_outside_the_roots_is_not_embedded() {
    let (directory, root, mut engine) = book_workspace();
    let outside = directory.path().join("outside.png");
    fs::copy(
        fixture("image-viewer/modules/ROOT/images/logo.png"),
        &outside,
    )
    .unwrap();
    let page = page(&root, "ROOT/pages/pictures.adoc");
    fs::write(
        &page,
        format!("= Pictures\n\nimage::{}[Outside]\n", outside.display()),
    )
    .unwrap();
    engine.set_file_access(adoc_engine::FileAccess::within([root.clone()]));
    let output = root.join("pictures.docx");
    let summary = engine
        .export_docx_blocking(docx(ExportTarget::File(page), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!document_xml(&output).contains("<wp:inline"));
    assert!(document_xml(&output).contains("Outside"));
    assert!(
        summary
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("outside.png")),
        "{:?}",
        summary.diagnostics
    );
}

const PROFILES: &str = r#"
[docx]
paper = "letter"
[docx.margins]
top = "20mm"
[docx.header]
left = "{document-title}"
[docx.styles]
heading_colour = "1F3864"

[docx.profiles.article]
page = "guide/modules/ROOT/pages/install.adoc"

[docx.profiles.book]
component = "1.0@guide"
output = "build/guide.docx"
orientation = "landscape"
attributes = { author = "Paul Snow" }

[docx.profiles.selected]
nav = ["guide/modules/admin/nav.adoc", "guide/modules/ROOT/nav.adoc"]

[docx.profiles.escaping]
page = "guide/modules/ROOT/pages/install.adoc"
output = "../outside.docx"

[docx.profiles.missing]
component = "nothing"

[docx.profiles.pdf]
page = "guide/modules/ROOT/pages/install.adoc"
output = "manual.pdf"
"#;

#[test]
fn docx_profiles_resolve_and_validate() {
    let (_directory, root, mut engine) = book_workspace_with(PROFILES);
    // A profile whose output is not a `.docx` file is refused when read.
    assert!(
        engine
            .settings()
            .warnings
            .iter()
            .any(|warning| warning.contains("docx.profiles.pdf")
                && warning.contains("is not a .docx file")),
        "{:?}",
        engine.settings().warnings
    );

    let article = engine.docx_profile_export_args("article").unwrap();
    assert_eq!(
        article.target,
        ExportTarget::File(page(&root, "ROOT/pages/install.adoc"))
    );
    assert_eq!(
        article.output,
        root.join("article.docx"),
        "`<name>.docx` by default"
    );
    let book = engine.docx_profile_export_args("book").unwrap();
    assert_eq!(
        book.target,
        ExportTarget::Component {
            name: "guide".to_owned(),
            version: Some("1.0".to_owned()),
        }
    );
    assert_eq!(book.output, root.join("build/guide.docx"));
    assert_eq!(book.landscape, Some(true));
    assert_eq!(
        book.attributes,
        [("author".to_owned(), "Paul Snow".to_owned())]
    );
    let selected = engine.docx_profile_export_args("selected").unwrap();
    assert_eq!(
        selected.target,
        ExportTarget::Navigation(vec![
            page(&root, "admin/nav.adoc"),
            page(&root, "ROOT/nav.adoc"),
        ])
    );

    // Refused: an output outside the workspace, a profile that is not there; reported: content
    // the workspace does not have.
    assert!(matches!(
        engine.docx_profile_export_args("escaping"),
        Err(EngineError::Refused(_))
    ));
    assert!(matches!(
        engine.docx_profile_export_args("absent"),
        Err(EngineError::NotFound(_))
    ));
    assert_eq!(engine.docx_profile_problems("book"), Vec::<String>::new());
    assert_eq!(
        engine.docx_profile_problems("missing"),
        ["its component nothing is not in the workspace"]
    );
    assert_eq!(engine.docx_profile_problems("escaping").len(), 1);

    // Exported through the command, to the output it names.
    let events = engine.subscribe();
    engine
        .invoke(
            ids::EXPORT_DOCX_PROFILE,
            CommandArgs::Profile("book".to_owned()),
        )
        .unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let result = events
        .try_iter()
        .find_map(|event| match event {
            EngineEvent::ExportFinished { output, result } => Some((output, result)),
            _ => None,
        })
        .expect("the export finished");
    assert_eq!(result.0, root.join("build/guide.docx"));
    let summary = result.1.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(summary.sources.len(), 8);

    // `[docx]` and the profile shape the document: Letter, turned by the profile, the margin,
    // the header and the heading colour from the settings.
    let output = root.join("build/guide.docx");
    let document = document_xml(&output);
    assert!(
        document.contains(r#"<w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/>"#),
        "{document}"
    );
    assert!(document.contains(r#"w:top="1134""#));
    let header = part(&output, "word/header1.xml");
    assert!(header.contains("The Guide"), "{header}");
    let styles = part(&output, "word/styles.xml");
    assert!(styles.contains(r#"<w:color w:val="1F3864"/>"#));
    // The core properties take the profile's author.
    assert!(part(&output, "docProps/core.xml").contains("<dc:creator>Paul Snow</dc:creator>"));
}

#[cfg(feature = "typst")]
#[test]
fn a_pdf_profile_of_the_same_name_is_its_own() {
    let settings = format!("{PROFILES}\n[pdf.profiles.book]\ncomponent = \"1.0@guide\"\n");
    let (_directory, root, engine) = book_workspace_with(&settings);
    let pdf = engine.profile_export_args("book").unwrap();
    assert_eq!(pdf.output, root.join("book.pdf"), "still `<name>.pdf`");
    assert!(
        pdf.attributes.is_empty(),
        "the DOCX profile's attributes are its own"
    );
    let docx = engine.docx_profile_export_args("book").unwrap();
    assert_eq!(docx.output, root.join("build/guide.docx"));
    assert!(matches!(
        engine.profile_export_args("article"),
        Err(EngineError::NotFound(_))
    ));
}

/// A Word document cannot typeset maths, so the export renders its formulas as images, as the
/// preview does, rather than leaving them for a typesetter as a PDF does.
#[cfg(feature = "typst")]
#[test]
fn formulas_are_typeset_as_pictures() {
    let (directory, root, mut engine) = book_workspace();
    let page = root.join("guide/modules/ROOT/pages/maths.adoc");
    fs::write(
        &page,
        "= Maths\n:stem:\n\nThe area is stem:[A = pi r^2].\n\n[stem]\n++++\nsum_(i=1)^n i\n++++\n",
    )
    .unwrap();
    let id = engine.open(&page).unwrap();
    let output = directory.path().join("maths.docx");
    let summary = engine
        .export_docx_blocking(docx(ExportTarget::Page(id), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        summary
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("formula")),
        "{:?}",
        summary.diagnostics
    );
    let document = document_xml(&output);
    assert_eq!(document.matches("<wp:inline").count(), 2, "{document}");
    // Each source is its picture's description, for a screen reader, and not text of its own.
    for source in ["A = pi r^2", "sum_(i=1)^n i"] {
        assert!(
            document.contains(&format!("descr=\"{source}\"")),
            "{document}"
        );
        assert!(!document.contains(&format!(">{source}<")), "{document}");
    }
}
