//! The package's structure: PRD §10, test 1 (`docx_package_is_valid_opc`).

mod common;

use std::collections::BTreeMap;

use adoc_docx::{BookItem, DocxBook, DocxDocument, DocxPage, DocxSeverity};
use common::{assert_valid_package, paragraphs, text, view, written};

const PAGE: &str = "= Release Notes\n:author: Paul Snow\n:description: What changed.\n\n\
                    The first paragraph.\n\n== Installing\n\nRun the installer.\n\n\
                    === On macOS\n\nOpen the DMG.\n";

#[test]
fn docx_package_is_valid_opc() {
    let (bytes, report) = written(&common::article(PAGE));
    let entries = assert_valid_package(&bytes);
    assert_eq!(report.diagnostics, Vec::new());

    assert_eq!(
        paragraphs(text(&entries, "word/document.xml")),
        [
            (Some("Title".to_owned()), "Release Notes".to_owned()),
            (None, "The first paragraph.".to_owned()),
            (Some("Heading1".to_owned()), "Installing".to_owned()),
            (None, "Run the installer.".to_owned()),
            (Some("Heading2".to_owned()), "On macOS".to_owned()),
            (None, "Open the DMG.".to_owned()),
        ]
    );

    let core = roxmltree::Document::parse(text(&entries, "docProps/core.xml")).unwrap();
    let field = |name: &str| {
        core.descendants()
            .find(|node| node.tag_name().name() == name)
            .and_then(|node| node.text())
            .map(str::to_owned)
    };
    assert_eq!(field("title").as_deref(), Some("Release Notes"));
    assert_eq!(field("creator").as_deref(), Some("Paul Snow"));
    assert_eq!(field("description").as_deref(), Some("What changed."));
    // No date: the same input gives the same bytes.
    assert_eq!(field("created"), None);
    assert_eq!(field("modified"), None);
}

#[test]
fn the_same_document_gives_the_same_bytes() {
    let document = common::article(PAGE);
    let (first, _) = written(&document);
    let (second, _) = written(&document);
    assert_eq!(first, second);
}

#[test]
fn a_book_s_pages_take_headings_at_their_depth() {
    let page = |key: &str, path: &str, text: &str, depth| {
        BookItem::Page(DocxPage {
            key: key.to_owned(),
            view: view(path, text),
            depth,
        })
    };
    let book = DocxDocument::Book(DocxBook {
        title: "The Manual".to_owned(),
        subtitle: Some("For everyone".to_owned()),
        attributes: BTreeMap::from([("author".to_owned(), "Paul Snow".to_owned())]),
        items: vec![
            BookItem::Part("Getting started".to_owned()),
            BookItem::Heading {
                title: "Basics".to_owned(),
                depth: 1,
            },
            page(
                "p0",
                "/docs/a.adoc",
                "= Install\n\nRun it.\n\n== Check\n\nLook.\n",
                2,
            ),
            page("p1", "/docs/b.adoc", "= Use\n\nType.\n", 1),
        ],
    });
    let (bytes, report) = written(&book);
    let entries = assert_valid_package(&bytes);
    assert_eq!(report.diagnostics, Vec::new());
    let heading = |level: u8| Some(format!("Heading{level}"));
    // A part is a level above the chapters: everything else moves down one.
    assert_eq!(
        paragraphs(text(&entries, "word/document.xml")),
        [
            (Some("Title".to_owned()), "The Manual".to_owned()),
            (Some("Subtitle".to_owned()), "For everyone".to_owned()),
            // The rest of the title page, and the paragraph ending its section.
            (Some("TitlePageDetail".to_owned()), "Paul Snow".to_owned()),
            (None, String::new()),
            (heading(1), "Getting started".to_owned()),
            (heading(2), "Basics".to_owned()),
            (heading(3), "Install".to_owned()),
            (None, "Run it.".to_owned()),
            (heading(4), "Check".to_owned()),
            (None, "Look.".to_owned()),
            (heading(2), "Use".to_owned()),
            (None, "Type.".to_owned()),
        ]
    );
}

#[test]
fn what_is_not_written_as_word_structure_yet_is_shown_as_text_and_reported() {
    // Images and formulas are embedded by DOCX 4/8; until then they are text, and reported.
    let (bytes, report) = written(&common::article(
        "= Page\n\nimage::missing.png[A diagram of the flow]\n\n[stem]\n++++\nsqrt(4)\n++++\n",
    ));
    let entries = assert_valid_package(&bytes);
    let texts: Vec<String> = paragraphs(text(&entries, "word/document.xml"))
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("A diagram of the flow")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("sqrt(4)")),
        "{texts:?}"
    );
    let warnings: Vec<(DocxSeverity, u32)> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.severity,
                diagnostic.source.as_ref().unwrap().line,
            )
        })
        .collect();
    assert_eq!(
        warnings,
        [(DocxSeverity::Warning, 3), (DocxSeverity::Warning, 5)]
    );
}

/// Writes the article and the book above to `$ADOC_DOCX_OUT`, for opening in Word or
/// LibreOffice by hand: `ADOC_DOCX_OUT=/tmp cargo test -p adoc-docx --test package -- --ignored`.
#[test]
#[ignore = "writes files for a manual check"]
fn write_samples() {
    let folder = std::env::var_os("ADOC_DOCX_OUT").expect("ADOC_DOCX_OUT names a folder");
    let folder = std::path::Path::new(&folder);
    let (article, _) = written(&common::article(PAGE));
    std::fs::write(folder.join("article.docx"), article).unwrap();
    let hostile = DocxDocument::Book(DocxBook {
        title: "Escaping <&> \"quotes\" \u{1}\u{FFFE} é 😀".to_owned(),
        subtitle: Some("tab\there".to_owned()),
        attributes: BTreeMap::new(),
        items: vec![BookItem::Part("A part".to_owned())],
    });
    let (book, _) = written(&hostile);
    std::fs::write(folder.join("escaping.docx"), book).unwrap();
}

#[test]
fn style_overrides_change_the_document_s_styles() {
    let options = adoc_docx::DocxOptions {
        styles: adoc_docx::StyleOverrides {
            body_font: Some("Georgia".to_owned()),
            heading_font: Some("Gill Sans <&>".to_owned()),
            code_font: Some("Menlo".to_owned()),
            body_size: Some(23),
            heading_colour: Some("1F3864".to_owned()),
            link_colour: Some("C00000".to_owned()),
        },
        ..adoc_docx::DocxOptions::default()
    };
    let mut bytes = Vec::new();
    adoc_docx::write_docx(&common::article(PAGE), &options, &mut bytes).unwrap();
    let entries = assert_valid_package(&bytes);
    let styles = roxmltree::Document::parse(text(&entries, "word/styles.xml")).unwrap();
    let w = common::W;
    let style = |id: &str| {
        styles
            .descendants()
            .find(|node| {
                node.has_tag_name((w, "style")) && node.attribute((w, "styleId")) == Some(id)
            })
            .unwrap()
    };
    let value = |node: roxmltree::Node, element: &str, attribute: &str| {
        node.descendants()
            .find(|child| child.has_tag_name((w, element)))
            .and_then(|child| child.attribute((w, attribute)))
            .map(str::to_owned)
    };
    let defaults = styles
        .descendants()
        .find(|node| node.has_tag_name((w, "rPrDefault")))
        .unwrap();
    assert_eq!(
        value(defaults, "rFonts", "ascii").as_deref(),
        Some("Georgia")
    );
    assert_eq!(value(defaults, "sz", "val").as_deref(), Some("23"));
    for level in ["Heading1", "Title"] {
        assert_eq!(
            value(style(level), "rFonts", "ascii").as_deref(),
            Some("Gill Sans <&>")
        );
    }
    assert_eq!(
        value(style("Heading1"), "color", "val").as_deref(),
        Some("1F3864")
    );
    assert_eq!(
        value(style("CodeChar"), "rFonts", "ascii").as_deref(),
        Some("Menlo")
    );
    assert_eq!(
        value(style("SourceCode"), "rFonts", "ascii").as_deref(),
        Some("Menlo")
    );
    assert_eq!(
        value(style("Hyperlink"), "color", "val").as_deref(),
        Some("C00000")
    );
}
