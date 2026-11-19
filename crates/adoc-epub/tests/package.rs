//! EPUB 1/9 (#638): the OCF container and the package document, read back in process by the
//! crate's own reader (PRD §11 tests 1 to 3).

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_epub::{
    testing, write_book, write_epub, Book, BookItem, Document, EpubError, EpubOptions, Modified,
    Page, Plan,
};

fn view(path: &Path, text: &str) -> adoc_render::ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

fn article(text: &str) -> Document {
    Document::Article(Page {
        key: "p0".to_owned(),
        view: view(Path::new("/docs/page.adoc"), text),
        depth: 0,
    })
}

fn book(pages: &[(&str, &str)]) -> Document {
    let items = pages
        .iter()
        .enumerate()
        .map(|(index, (name, text))| {
            BookItem::Page(Page {
                key: format!("p{}", index + 1),
                view: view(&Path::new("/docs").join(name), text),
                depth: 1,
            })
        })
        .collect();
    Document::Book(Plan {
        title: "The Book".to_owned(),
        items,
        ..Plan::default()
    })
}

fn modified() -> Modified {
    // 2026-11-19T10:30:00Z
    Modified::from_unix_seconds(1_795_084_200).unwrap()
}

/// PRD §11 test 1.
#[test]
fn epub_package_is_valid_ocf() {
    let output = write_epub(
        &article("= A Page\n\nSome text.\n\n== A section\n\nMore.\n"),
        &EpubOptions::new(modified()),
    )
    .unwrap();
    assert_eq!(output.chapters, 1);
    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    assert_eq!(package.path, "EPUB/package.opf");
    assert_eq!(package.title, "A Page");
    assert_eq!(package.language, "en");
    assert!(
        package.identifier.starts_with("urn:uuid:"),
        "{}",
        package.identifier
    );
    assert_eq!(package.modified, "2026-11-19T10:30:00Z");
    assert_eq!(package.nav, "EPUB/nav.xhtml");
    assert_eq!(package.spine, ["c1"]);
    assert_eq!(
        package.manifest["c1"],
        (
            "EPUB/text/c1.xhtml".to_owned(),
            "application/xhtml+xml".to_owned(),
            None
        )
    );
    // The archive holds nothing else, `mimetype` first and the container before the package.
    let names: Vec<String> = testing::entries(&output.bytes)
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(
        names,
        [
            "mimetype",
            "META-INF/container.xml",
            "EPUB/package.opf",
            "EPUB/nav.xhtml",
            "EPUB/css/book.css",
            "EPUB/text/c1.xhtml"
        ]
    );
    // The chapter is XHTML, in its language, holding the page's text.
    let chapter = testing::parse("c1", testing::text(&files, "EPUB/text/c1.xhtml"));
    let html = chapter.root_element();
    assert!(html.has_tag_name((testing::XHTML, "html")));
    assert_eq!(
        html.attribute(("http://www.w3.org/XML/1998/namespace", "lang")),
        Some("en")
    );
    let text: String = chapter
        .descendants()
        .filter_map(|node| node.text())
        .collect();
    assert!(
        text.contains("Some text.") && text.contains("A section"),
        "{text}"
    );
    // The navigation leads to the chapter and its section.
    let nav = testing::text(&files, "EPUB/nav.xhtml");
    assert!(nav.contains("href=\"text/c1.xhtml\""), "{nav}");
    assert!(nav.contains("href=\"text/c1.xhtml#c1-a"), "{nav}");

    // A book: a chapter a page, in order, each in the spine.
    let output = write_epub(
        &book(&[
            ("one.adoc", "= One\n\nFirst."),
            ("two.adoc", "= Two\n\nSecond."),
        ]),
        &EpubOptions::new(modified()),
    )
    .unwrap();
    let (_, package) = testing::assert_valid_ocf(&output.bytes);
    assert_eq!(output.chapters, 2);
    assert_eq!(package.title, "The Book");
    assert_eq!(package.spine, ["c1", "c2"]);
}

/// PRD §11 test 2.
#[test]
fn epub_text_is_escaped() {
    let text = "= <Tags> & \"quotes\"\n:author: Zoë Ångström\n\n\
                Less < and > more & so on, 日本語, Ελληνικά, 🙂.\n";
    let mut options = EpubOptions::new(modified());
    options.book.creator = Some("A \"B\" <C> & D\u{1}".to_owned());
    let output = write_epub(&article(text), &options).unwrap();
    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    // Every XML file parsed; what was escaped reads back as written, and the one character
    // XML 1.0 forbids is replaced.
    assert_eq!(package.title, "<Tags> & \"quotes\"");
    let opf = testing::parse("opf", testing::text(&files, "EPUB/package.opf"));
    let creator = opf
        .descendants()
        .find(|node| node.has_tag_name((testing::DC, "creator")))
        .and_then(|node| node.text())
        .unwrap();
    assert_eq!(creator, "A \"B\" <C> & D\u{FFFD}");
    let chapter = testing::parse("c1", testing::text(&files, "EPUB/text/c1.xhtml"));
    let body: String = chapter
        .descendants()
        .filter_map(|node| node.text())
        .collect();
    assert!(
        body.contains("Less < and > more & so on, 日本語, Ελληνικά, 🙂."),
        "{body}"
    );
    // The raw markup never holds the text unescaped.
    assert!(!testing::text(&files, "EPUB/package.opf").contains("<Tags>"));

    // Malformed entries are refused: an unsafe name, and XML that is not well-formed, never
    // reach an archive.
    for name in ["../escape", "/abs", "a\\b", "", "a//b"] {
        assert!(testing::refused(name, b"<a/>"), "{name:?}");
    }
    assert!(testing::refused(
        "EPUB/text/c1.xhtml",
        b"<html><p>open</html>"
    ));
    assert!(testing::refused("EPUB/package.opf", &[0xFF]));
    assert!(!testing::refused("EPUB/text/c1.xhtml", b"<html/>"));
}

/// PRD §11 test 3.
#[test]
fn epub_output_is_deterministic() {
    let document = book(&[
        (
            "one.adoc",
            "= One\n\nFirst, with a footnote.footnote:[A note.]",
        ),
        ("two.adoc", "= Two\n\n== Part\n\nSecond."),
    ]);
    let mut options = EpubOptions::new(modified());
    options.book.identifier = Some("urn:uuid:12345678-1234-5678-1234-567812345678".to_owned());
    let first = write_epub(&document, &options).unwrap().bytes;
    let second = write_epub(&document, &options).unwrap().bytes;
    assert_eq!(first, second, "the same input gives the same bytes");
    let (_, package) = testing::assert_valid_ocf(&first);
    assert_eq!(
        package.identifier,
        "urn:uuid:12345678-1234-5678-1234-567812345678"
    );
    // Every file is dated with the modification time, to the even second.
    for entry in testing::entries(&first) {
        assert_eq!(
            entry.dos,
            ((10 << 11) | (30 << 5), (46 << 9) | (11 << 5) | 19),
            "{}",
            entry.name
        );
    }
    // Another time changes the dates and dcterms:modified, nothing else.
    let later = EpubOptions {
        modified: Modified::from_unix_seconds(1_795_084_200 + 86_400).unwrap(),
        ..options.clone()
    };
    let third = write_epub(&document, &later).unwrap().bytes;
    assert_ne!(first, third);
    let (_, later_package) = testing::assert_valid_ocf(&third);
    assert_eq!(later_package.modified, "2026-11-20T10:30:00Z");
    let strip = |bytes: &[u8]| -> Vec<(String, Vec<u8>)> {
        testing::entries(bytes)
            .into_iter()
            .filter(|entry| entry.name != "EPUB/package.opf")
            .map(|entry| (entry.name, entry.data))
            .collect()
    };
    assert_eq!(strip(&first), strip(&third));
    // Without an identifier given, the derived one is the same each time.
    let derived = |document: &Document| {
        let output = write_epub(document, &EpubOptions::new(modified())).unwrap();
        testing::assert_valid_ocf(&output.bytes).1.identifier
    };
    assert_eq!(derived(&document), derived(&document));
}

/// A book with nothing in it is still a book: one empty chapter, valid.
#[test]
fn an_empty_book_is_valid() {
    let document = Document::Book(Plan {
        title: "Empty".to_owned(),
        ..Plan::default()
    });
    let (book, _) = Book::build(&document, &adoc_epub::BookOptions::default());
    let bytes = write_book(&book, modified()).unwrap();
    let (_, package) = testing::assert_valid_ocf(&bytes);
    assert_eq!(package.spine, ["c1"]);
    // An error names what was wrong.
    assert!(EpubError::Invalid("x".to_owned())
        .to_string()
        .contains("not valid"));
}
