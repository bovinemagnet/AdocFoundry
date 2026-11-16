//! Document text in the package's XML: PRD §10, test 2 (`docx_text_is_escaped`).

mod common;

use std::collections::BTreeMap;

use adoc_docx::{BookItem, DocxBook, DocxDocument};
use common::{entries, paragraphs, text, written};

/// What XML 1.0 forbids in a document: every control character but tab, line feed and carriage
/// return, and the two non-characters at the end of the Basic Multilingual Plane.
fn forbidden(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}')
}

#[test]
fn docx_text_is_escaped() {
    let hostile =
        "<w:p>&amp; \"quoted\" 'single' ]]> <!-- --> \u{0}\u{1}\u{B}\u{1F}\u{FFFE}\u{FFFF} \
                   é ß 😀 \u{202E}rtl\u{202C} e\u{301} \u{7F}\u{85}";
    let book = DocxDocument::Book(DocxBook {
        title: hostile.to_owned(),
        subtitle: None,
        attributes: BTreeMap::from([
            ("author".to_owned(), hostile.to_owned()),
            ("description".to_owned(), hostile.to_owned()),
        ]),
        items: vec![BookItem::Part(format!("tab\tand\nline {hostile}"))],
    });
    let (bytes, _) = written(&book);
    let entries = entries(&bytes);
    for name in entries.keys() {
        roxmltree::Document::parse(text(&entries, name))
            .unwrap_or_else(|error| panic!("{name} is not well formed: {error}"));
    }
    let cleaned: String = hostile
        .chars()
        .map(|c| if forbidden(c) { '\u{FFFD}' } else { c })
        .collect();
    assert_eq!(
        paragraphs(text(&entries, "word/document.xml")),
        [
            (Some("Title".to_owned()), cleaned.clone()),
            // The author, on the title page, and the paragraph ending its section.
            (Some("TitlePageDetail".to_owned()), cleaned.clone()),
            (None, String::new()),
            (
                Some("Heading1".to_owned()),
                format!("tab\tand\nline {cleaned}")
            ),
        ]
    );
    let core = roxmltree::Document::parse(text(&entries, "docProps/core.xml")).unwrap();
    let creator = core
        .descendants()
        .find(|node| node.tag_name().name() == "creator")
        .and_then(|node| node.text());
    assert_eq!(creator, Some(cleaned.as_str()));
}

#[test]
fn every_character_survives_or_is_replaced() {
    // Every Unicode scalar value, in one paragraph: the part stays well formed, and each
    // character is kept or, where XML forbids it, replaced.
    let all: String = (0..=0x10_FFFF_u32)
        .filter_map(char::from_u32)
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let book = DocxDocument::Book(DocxBook {
        title: all.clone(),
        ..DocxBook::default()
    });
    let (bytes, _) = written(&book);
    let entries = entries(&bytes);
    let found = paragraphs(text(&entries, "word/document.xml"));
    let expected: String = all
        .chars()
        .map(|c| if forbidden(c) { '\u{FFFD}' } else { c })
        .collect();
    assert_eq!(found[0].1, expected);
}

#[test]
fn line_endings_become_breaks() {
    let book = DocxDocument::Book(DocxBook {
        title: "one\r\ntwo\rthree\nfour".to_owned(),
        ..DocxBook::default()
    });
    let (bytes, _) = written(&book);
    let entries = entries(&bytes);
    assert_eq!(
        paragraphs(text(&entries, "word/document.xml"))[0].1,
        "one\ntwo\nthree\nfour"
    );
}

#[test]
fn empty_documents_are_written() {
    for document in [
        common::article(""),
        common::article("= \n"),
        DocxDocument::Book(DocxBook::default()),
    ] {
        let (bytes, _) = written(&document);
        let entries = entries(&bytes);
        assert!(entries.contains_key("word/document.xml"));
    }
}
