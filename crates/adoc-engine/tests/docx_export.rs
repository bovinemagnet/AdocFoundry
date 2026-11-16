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

/// A copy of the reference book (`tests/fixtures/docx/reference`), its settings included, at
/// `folder`, opened as the CLI opens a workspace.
fn reference_workspace(folder: &Path) -> (PathBuf, Engine) {
    let root = folder.join("reference");
    copy_tree(&fixture("docx/reference"), &root);
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (root, engine)
}

fn manual() -> ExportTarget {
    ExportTarget::Component {
        name: "manual".to_owned(),
        version: Some("2.0".to_owned()),
    }
}

/// The DOCX export PRD, §10 test 18: a book of two components and two versions, with nested
/// partials and every construct the export writes, passes every structural check, holds what
/// its pages hold and nothing else, and is the same bytes wherever and however often it is
/// exported.
#[test]
fn docx_reference_book_is_complete() {
    let directory = tempfile::tempdir().unwrap();
    let (root, mut engine) = reference_workspace(&directory.path().join("a"));
    let output = directory.path().join("manual.docx");
    let summary = engine
        .export_docx_blocking(docx(manual(), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let bytes = fs::read(&output).unwrap();
    let entries = adoc_docx::testing::assert_valid_package(&bytes);
    let text = |name: &str| adoc_docx::testing::text(&entries, name).to_owned();
    let document = text("word/document.xml");

    // Version 2.0's pages, in navigation order, the start page first; nothing of 1.0's or of
    // the other component's.
    let sources: Vec<String> = summary
        .sources
        .iter()
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        sources,
        [
            "manual/modules/ROOT/pages/index.adoc",
            "manual/modules/ROOT/pages/install.adoc",
            "manual/modules/ROOT/pages/tables.adoc",
            "manual/modules/ROOT/pages/media.adoc",
            "manual/modules/ROOT/pages/links.adoc",
            "manual/modules/admin/pages/security.adoc",
        ]
    );
    assert_eq!(summary.pages, 6);
    for absent in ["OLD-VERSION-MARKER", "API-COMPONENT-MARKER"] {
        assert!(!document.contains(absent), "{absent}");
    }

    // One warning, for the reference out of the book, placed where it was written; nothing is
    // lost without one.
    let warnings: Vec<String> = summary
        .diagnostics
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(summary.diagnostics.len(), 1, "{warnings:?}");
    let (path, line) = summary.diagnostics[0].location.clone().unwrap();
    assert_eq!(path, root.join("manual/modules/ROOT/pages/links.adoc"));
    assert_eq!(line, 4);
    assert!(warnings[0].contains("api::index.adoc"), "{warnings:?}");

    // Real Word structures: the title, parts, chapters and sections as heading styles, and
    // everything a page holds in its own style.
    let paragraphs = adoc_docx::testing::paragraphs(&document);
    let has = |style: &str, text: &str| {
        paragraphs
            .iter()
            .any(|(found, found_text)| found.as_deref() == Some(style) && found_text == text)
    };
    for (style, text) in [
        ("Title", "The Manual"),
        ("Heading1", "Using the product"),
        ("Heading1", "Administration"),
        ("Heading2", "Installing"),
        ("Heading2", "Media"),
        ("Heading2", "Links"),
        ("Heading2", "Security"),
        ("Heading3", "Requirements"),
        ("Heading3", "Tables"),
        ("Heading3", "Keys"),
        ("Caption", "Table 1. Supported platforms"),
        ("Caption", "Figure 2. A photograph"),
        ("Caption", "Starting the service"),
        ("TOCHeading", "Table of Contents"),
    ] {
        assert!(has(style, text), "no {style} {text:?}: {paragraphs:?}");
    }
    let styled = |style: &str| {
        paragraphs
            .iter()
            .filter(|(found, _)| found.as_deref() == Some(style))
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
    };
    assert!(styled("SourceCode")
        .iter()
        .any(|text| text.starts_with("systemctl start product")));
    // Markup characters escaped, Unicode as written.
    assert!(document.contains("Ünïcødé, \"quotes\""));
    assert!(document.contains("&amp; &lt;angle brackets&gt;, and 3 &gt; 2."));
    assert!(
        document.contains("NESTED-PARTIAL-MARKER"),
        "the nested partial"
    );
    for style in [
        "AdocAdmonition",
        "AdocSidebar",
        "AdocExample",
        "Quote",
        "DescriptionTerm",
    ] {
        assert!(document.contains(&format!("w:val=\"{style}\"")), "{style}");
    }
    // Lists numbered by Word, nested ones by letter; the table's cells merged on its grid.
    assert!(document.matches("<w:numPr>").count() >= 5);
    assert!(text("word/numbering.xml").contains("w:val=\"lowerLetter\""));
    assert!(document.contains("<w:gridSpan w:val=\"2\"/>"));
    assert!(document.contains("<w:vMerge w:val=\"restart\"/>"));

    // Links: two between pages of the book, to bookmarks the validator found; one to the web.
    let internal: Vec<&str> = document
        .match_indices("w:anchor=\"_a")
        .map(|(at, _)| &document[at..at + 14])
        .collect();
    assert_eq!(internal.len(), 2, "{internal:?}");
    let relationships = text("word/_rels/document.xml.rels");
    assert!(relationships.contains(r#"Target="https://example.com/product" TargetMode="External""#));
    // A footnote written once and referred to twice.
    assert_eq!(
        text("word/footnotes.xml")
            .matches("The note, written once.")
            .count(),
        1
    );
    assert!(document.contains("NOTEREF _f1"));

    // Pictures: covers, the title page's logo, PNG, JPEG, SVG, a diagram and two formulas, each
    // file once, whatever uses it, and every one in the package itself.
    let media: Vec<&String> = entries
        .keys()
        .filter(|name| name.starts_with("word/media/"))
        .collect();
    assert_eq!(media.len(), 7, "{media:?}");
    assert!(media.iter().any(|name| name.ends_with(".jpeg")));
    for alt in [
        "Front cover",
        "Back cover",
        "Logo",
        "The product logo",
        "A photograph",
        "The pipeline drawn as SVG",
        "A = pi r^2",
        "sum_(i=1)^n i = (n(n+1))/2",
    ] {
        assert!(document.contains(&format!("descr=\"{alt}\"")), "{alt}");
    }
    for (name, data) in &entries {
        assert!(
            !data.windows(5).any(|window| window == b"file:"),
            "{name} refers to a file outside the package"
        );
    }

    // Sections: front cover, title page, the body numbered from 1 with its first page's own
    // header, back cover; A4 with the settings' margins.
    assert_eq!(document.matches("<w:sectPr").count(), 4);
    assert_eq!(document.matches("<w:pgNumType w:start=\"1\"/>").count(), 1);
    assert!(document.contains("<w:titlePg/>"));
    assert!(document.contains("<w:pgSz w:w=\"11906\" w:h=\"16838\"/>"));
    assert!(document.contains("w:top=\"1134\""));
    assert!(document.contains("TOC \\o \"1-3\" \\h \\z \\u"));
    let settings = text("word/settings.xml");
    assert!(settings.contains("<w:evenAndOddHeaders/>"));
    assert!(settings.contains("<w:updateFields w:val=\"true\"/>"));

    // Running content: odd and even headers with the logo and the chapter's title, footers
    // with live page numbers, and a first page's header with the document's title.
    let parts = |prefix: &str| {
        entries
            .keys()
            .filter(|name| name.starts_with(prefix) && name.ends_with(".xml"))
            .map(|name| text(name))
            .collect::<Vec<_>>()
    };
    let headers = parts("word/header");
    assert_eq!(
        headers
            .iter()
            .filter(|header| header.contains("STYLEREF \"heading 2\"")
                && header.contains("<w:drawing>"))
            .count(),
        2
    );
    assert!(headers.iter().any(|header| header.contains(">The Manual<")));
    let footers = parts("word/footer");
    assert_eq!(
        footers
            .iter()
            .filter(|footer| footer.contains(" PAGE ") && footer.contains(" NUMPAGES "))
            .count(),
        2
    );
    assert!(entries
        .keys()
        .any(|name| name.starts_with("word/_rels/header")));

    // Properties, and the settings' heading colour.
    let core = text("docProps/core.xml");
    assert!(core.contains("<dc:title>The Manual</dc:title>"));
    assert!(core.contains("<dc:creator>Paul Snow</dc:creator>"));
    assert!(
        text("word/styles.xml").contains("w:val=\"1F4E79\"")
            || text("word/styles.xml").contains("w:val=\"1f4e79\"")
    );

    // The same bytes again, and from a copy of the workspace somewhere else.
    let again = directory.path().join("again.docx");
    engine
        .export_docx_blocking(docx(manual(), &again), WAIT)
        .unwrap();
    assert!(
        fs::read(&again).unwrap() == bytes,
        "a second export differs"
    );
    let (_elsewhere, mut other) = reference_workspace(&directory.path().join("b/c"));
    let moved = directory.path().join("moved.docx");
    other
        .export_docx_blocking(docx(manual(), &moved), WAIT)
        .unwrap();
    assert!(
        fs::read(&moved).unwrap() == bytes,
        "the workspace's place changes the document"
    );
}

/// An export that runs out of time is a failure, and a failure leaves the output as it was,
/// even when the time runs out while the document is being written: the writer must not
/// replace the file afterwards. Timeouts from a few milliseconds up, until one is long enough,
/// so that some end in each stage.
#[test]
fn a_timed_out_export_leaves_the_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("long");
    let pages = root.join("modules/ROOT/pages");
    fs::create_dir_all(&pages).unwrap();
    fs::write(
        root.join("antora.yml"),
        "name: long\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    let mut nav = String::new();
    for page in 0..60 {
        let sections: String = (0..20)
            .map(|section| format!("== Section {section}\n\nText of {page}.{section}.footnote:[A note.]\n\n|===\n|a |b\n|===\n\n"))
            .collect();
        fs::write(
            pages.join(format!("p{page}.adoc")),
            format!("= Page {page}\n\n{sections}"),
        )
        .unwrap();
        nav.push_str(&format!("* xref:p{page}.adoc[]\n"));
    }
    fs::write(root.join("modules/ROOT/nav.adoc"), nav).unwrap();
    let mut settings = Settings::default().with_git_opt_in();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let output = directory.path().join("long.docx");
    let target = || ExportTarget::Component {
        name: "long".to_owned(),
        version: Some("1.0".to_owned()),
    };

    let mut timeout = Duration::from_millis(2);
    let mut failures = 0;
    loop {
        fs::write(&output, "the earlier document").unwrap();
        match engine.export_docx_blocking(docx(target(), &output), timeout) {
            Ok(_) => break,
            Err(error) => {
                failures += 1;
                assert!(error.to_string().contains("in time"), "{error}");
                // Whatever was still running has finished; the file is untouched.
                assert!(engine.wait_for_workers(WAIT));
                assert_eq!(
                    fs::read_to_string(&output).unwrap_or_default(),
                    "the earlier document",
                    "replaced after timing out at {timeout:?}"
                );
                let leftovers: Vec<_> = fs::read_dir(directory.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name())
                    .filter(|name| name != "long" && name != "long.docx")
                    .collect();
                assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
            }
        }
        assert!(timeout < WAIT, "never finished");
        timeout = timeout.mul_f64(1.25);
    }
    assert!(failures > 0, "no timeout was short enough to test");
    assert!(fs::read(&output).unwrap().starts_with(b"PK\x03\x04"));
}

/// Problems in a page are warnings at the file and line they come from, in the page itself or
/// in a partial it includes, and the document is written with a visible stand-in for each: a
/// missing image, a diagram nothing renders, a video, and source cut short.
#[test]
fn docx_problems_are_located() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    let pages = root.join("guide/modules/ROOT");
    fs::create_dir_all(pages.join("partials")).unwrap();
    fs::write(
        pages.join("partials/broken.adoc"),
        "A partial.\n\nimage::also-missing.png[Also missing]\n",
    )
    .unwrap();
    let page = pages.join("pages/problems.adoc");
    fs::write(
        &page,
        "= Problems\n\nimage::missing.png[Missing]\n\n[graphviz]\n....\ndigraph { a -> b }\n....\n\n\
         video::intro.mp4[]\n\ninclude::partial$broken.adoc[]\n\n[source,rust]\n----\nfn cut_short() {\n",
    )
    .unwrap();
    // Every page written before the workspace is read, so the partial is in its catalog.
    let mut settings = Settings::default().with_git_opt_in();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.set_diagrams(adoc_diagrams::Diagrams::with_providers(
        adoc_diagrams::DiagramCache::new(directory.path().join("cache")),
        vec![Box::new(adoc_diagrams::MermaidProvider::default())],
    ));
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let id = engine.open(&page).unwrap();
    let output = directory.path().join("problems.docx");
    let summary = engine
        .export_docx_blocking(docx(ExportTarget::Page(id), &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let located: Vec<(String, u32, String)> = summary
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let (path, line) = diagnostic
                .location
                .clone()
                .unwrap_or_else(|| panic!("{diagnostic} has no place"));
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, line, diagnostic.message.clone())
        })
        .collect();
    let at = |name: &str, line: u32, words: &str| {
        located.iter().any(|(found, found_line, message)| {
            found == name && *found_line == line && message.contains(words)
        })
    };
    assert!(at("problems.adoc", 3, "missing.png"), "{located:?}");
    assert!(at("problems.adoc", 5, "graphviz"), "{located:?}");
    assert!(at("problems.adoc", 10, "video"), "{located:?}");
    assert!(at("broken.adoc", 3, "also-missing.png"), "{located:?}");

    // Each has a stand-in, and the source cut short is kept as code.
    let text: String = paragraphs(&output)
        .into_iter()
        .map(|(_, text)| text + "\n")
        .collect();
    for shown in [
        "Missing",
        "digraph { a -> b }",
        "intro.mp4",
        "Also missing",
        "fn cut_short() {",
    ] {
        assert!(text.contains(shown), "{shown} is not shown: {text}");
    }
    adoc_docx::testing::assert_valid_package(&fs::read(&output).unwrap());
}
