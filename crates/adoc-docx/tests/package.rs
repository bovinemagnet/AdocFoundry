//! The package's structure: PRD §10, test 1 (`docx_package_is_valid_opc`).

mod common;

use std::collections::{BTreeMap, BTreeSet};

use adoc_docx::{BookItem, DocxBook, DocxDocument, DocxPage, DocxSeverity};
use common::{entries, paragraphs, text, view, written, PACKAGE_RELATIONSHIPS, R, W};

const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const CORE_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const STYLES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";

const PAGE: &str = "= Release Notes\n:author: Paul Snow\n:description: What changed.\n\n\
                    The first paragraph.\n\n== Installing\n\nRun the installer.\n\n\
                    === On macOS\n\nOpen the DMG.\n";

/// The part a relationship in `source`'s relationships part points at, as a package name.
fn target_of(source: &str, target: &str) -> String {
    let base = source.rsplit_once('/').map_or("", |(folder, _)| folder);
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for segment in target.split('/') {
        match segment {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            segment => parts.push(segment),
        }
    }
    parts.join("/")
}

/// The relationships part of `part` (`word/_rels/document.xml.rels` for `word/document.xml`;
/// `_rels/.rels` for the package).
fn relationships_of(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((folder, name)) => format!("{folder}/_rels/{name}.rels"),
        None if part.is_empty() => "_rels/.rels".to_owned(),
        None => format!("_rels/{part}.rels"),
    }
}

/// Every check a reader makes of the package, against its bytes.
fn assert_valid_package(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let entries = entries(bytes);
    let names: BTreeSet<&str> = entries.keys().map(String::as_str).collect();
    for required in [
        "[Content_Types].xml",
        "_rels/.rels",
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "word/styles.xml",
        "docProps/core.xml",
    ] {
        assert!(names.contains(required), "{required} is missing: {names:?}");
    }

    // Every part is well-formed XML.
    for name in &names {
        roxmltree::Document::parse(text(&entries, name))
            .unwrap_or_else(|error| panic!("{name} is not well formed: {error}"));
    }

    // Every part has a content type, by extension or by name, and every override names a part.
    let types_text = text(&entries, "[Content_Types].xml");
    let types = roxmltree::Document::parse(types_text).unwrap();
    assert_eq!(
        types.root_element().tag_name().namespace(),
        Some(CONTENT_TYPES)
    );
    let defaults: BTreeMap<&str, &str> = types
        .descendants()
        .filter(|node| node.has_tag_name((CONTENT_TYPES, "Default")))
        .map(|node| {
            (
                node.attribute("Extension").unwrap(),
                node.attribute("ContentType").unwrap(),
            )
        })
        .collect();
    let overrides: BTreeMap<&str, &str> = types
        .descendants()
        .filter(|node| node.has_tag_name((CONTENT_TYPES, "Override")))
        .map(|node| {
            (
                node.attribute("PartName").unwrap(),
                node.attribute("ContentType").unwrap(),
            )
        })
        .collect();
    for part in overrides.keys() {
        let name = part.strip_prefix('/').expect("a part name starts with /");
        assert!(
            names.contains(name),
            "an override names {part}, not in the package"
        );
    }
    for name in names.iter().filter(|name| **name != "[Content_Types].xml") {
        let by_name = overrides.get(format!("/{name}").as_str());
        let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
        assert!(
            by_name.is_some() || defaults.contains_key(extension),
            "{name} has no content type"
        );
    }
    assert_eq!(
        overrides["/word/document.xml"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
    );
    assert_eq!(
        overrides["/word/styles.xml"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"
    );
    assert_eq!(
        overrides["/docProps/core.xml"],
        "application/vnd.openxmlformats-package.core-properties+xml"
    );
    assert_eq!(
        defaults["rels"],
        "application/vnd.openxmlformats-package.relationships+xml"
    );

    // Every relationship has a unique id and a target in the package.
    let mut by_type = BTreeMap::new();
    for rels in names.iter().filter(|name| name.ends_with(".rels")) {
        let source = match *rels {
            "_rels/.rels" => String::new(),
            rels => {
                let (folder, file) = rels.rsplit_once("/_rels/").unwrap();
                format!("{folder}/{}", file.strip_suffix(".rels").unwrap())
            }
        };
        assert_eq!(relationships_of(&source), *rels);
        assert!(
            source.is_empty() || names.contains(source.as_str()),
            "{rels} has no part"
        );
        let xml = roxmltree::Document::parse(text(&entries, rels)).unwrap();
        let mut ids = BTreeSet::new();
        for relationship in xml
            .descendants()
            .filter(|node| node.has_tag_name((PACKAGE_RELATIONSHIPS, "Relationship")))
        {
            let id = relationship.attribute("Id").unwrap();
            assert!(ids.insert(id.to_owned()), "{rels}: {id} twice");
            let target = relationship.attribute("Target").unwrap();
            if relationship.attribute("TargetMode") != Some("External") {
                let part = target_of(&source, target);
                assert!(
                    names.contains(part.as_str()),
                    "{rels}: {id} leads to {part}, not in the package"
                );
                by_type.insert(
                    (
                        source.clone(),
                        relationship.attribute("Type").unwrap().to_owned(),
                    ),
                    part,
                );
            }
        }
    }
    assert_eq!(
        by_type[&(String::new(), OFFICE_DOCUMENT.to_owned())],
        "word/document.xml"
    );
    assert_eq!(
        by_type[&(String::new(), CORE_PROPERTIES.to_owned())],
        "docProps/core.xml"
    );
    assert_eq!(
        by_type[&("word/document.xml".to_owned(), STYLES.to_owned())],
        "word/styles.xml"
    );

    // The document's root and body, and every `r:id` it uses has a relationship.
    let document = roxmltree::Document::parse(text(&entries, "word/document.xml")).unwrap();
    assert!(document.root_element().has_tag_name((W, "document")));
    let body = document
        .root_element()
        .children()
        .find(|node| node.has_tag_name((W, "body")))
        .expect("a body");
    assert!(
        body.last_element_child()
            .unwrap()
            .has_tag_name((W, "sectPr")),
        "the body ends with its section's properties"
    );
    let document_rels =
        roxmltree::Document::parse(text(&entries, "word/_rels/document.xml.rels")).unwrap();
    let ids: BTreeSet<&str> = document_rels
        .descendants()
        .filter_map(|node| node.attribute("Id"))
        .collect();
    for node in document.descendants() {
        for attribute in node.attributes().filter(|a| a.namespace() == Some(R)) {
            assert!(
                ids.contains(attribute.value()),
                "r:{} {} has no relationship",
                attribute.name(),
                attribute.value()
            );
        }
    }

    // Every paragraph style used is defined.
    let styles = roxmltree::Document::parse(text(&entries, "word/styles.xml")).unwrap();
    let defined: BTreeSet<&str> = styles
        .descendants()
        .filter(|node| node.has_tag_name((W, "style")))
        .filter_map(|node| node.attribute((W, "styleId")))
        .collect();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name((W, "pStyle")))
    {
        let style = node.attribute((W, "val")).unwrap();
        assert!(
            defined.contains(style),
            "the style {style} is used and not defined"
        );
    }
    entries
}

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
    let (bytes, report) = written(&common::article(
        "= Page\n\n* one\n* two\n\n[source,rust]\n----\nfn main() {}\n----\n",
    ));
    let entries = assert_valid_package(&bytes);
    let texts: Vec<String> = paragraphs(text(&entries, "word/document.xml"))
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(texts.iter().any(|text| text.contains("one")), "{texts:?}");
    assert!(
        texts.iter().any(|text| text.contains("fn main() {}")),
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
        [(DocxSeverity::Warning, 3), (DocxSeverity::Warning, 6)]
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
