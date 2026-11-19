//! DocBook 3/9 (#651): an article with its metadata, and a book's preface, parts, chapters and
//! sections as its navigation nests them (PRD §5.1, §11 tests 7 and 8). Every document is
//! checked against the DocBook 5.2.1 RELAX NG schema.

mod common;

use adoc_docbook::{
    testing, write_docbook, Book, BookItem, DocBookDocument, DocBookOptions, Severity,
};
use common::{article, book_page, children, elements, page, text_of};

fn written(document: &DocBookDocument) -> adoc_docbook::DocBookOutput {
    let output = write_docbook(document, &DocBookOptions::default()).expect("written");
    testing::assert_valid(&output.xml);
    output
}

/// The title of `node`'s `title` child.
fn title(node: roxmltree::Node<'_, '_>) -> String {
    node.children()
        .find(|child| child.has_tag_name((adoc_docbook::NAMESPACE, "title")))
        .map(text_of)
        .unwrap_or_default()
}

/// `node`'s structural children as `element: title`, with what they hold, indented.
fn outline(node: roxmltree::Node<'_, '_>, depth: usize, out: &mut Vec<String>) {
    for child in node.children().filter(roxmltree::Node::is_element) {
        let name = child.tag_name().name();
        if matches!(name, "preface" | "part" | "chapter" | "section") {
            out.push(format!("{}{name}: {}", "  ".repeat(depth), title(child)));
            outline(child, depth + 1, out);
        }
    }
}

fn outline_of(xml: &str) -> Vec<String> {
    let document = testing::parse(xml);
    let mut out = Vec::new();
    outline(document.root_element(), 0, &mut out);
    out
}

fn heading(title: &str, depth: u8) -> BookItem {
    BookItem::Heading {
        title: title.to_owned(),
        depth,
    }
}

#[test]
fn a_book_has_a_preface_parts_chapters_and_sections() {
    let book = DocBookDocument::Book(Book {
        title: "The Guide".to_owned(),
        items: vec![
            BookItem::Preface(page(
                "p1",
                "/docs/modules/ROOT/pages/index.adoc",
                "= Welcome\n\nHello.\n",
                1,
            )),
            BookItem::Part("Getting Started".to_owned()),
            book_page(
                "p2",
                "install",
                "= Install\n\nRun it.\n\n== Requirements\n\nNone.\n",
                1,
            ),
            book_page("p3", "configure", "= Configure\n\nSet it.\n", 2),
            book_page("p4", "deep", "= Deep\n\nDeeper.\n", 3),
            book_page("p5", "use", "= Use\n\nUse it.\n", 1),
            BookItem::Part("Administration".to_owned()),
            book_page("p6", "backup", "= Backup\n\nBack up.\n", 1),
            heading("Recovery", 1),
            book_page("p7", "restore", "= Restore\n\nRestore.\n", 2),
            heading("Nothing Here", 1),
            BookItem::Part("Empty".to_owned()),
            heading("Still Nothing", 1),
        ],
        ..Book::default()
    });
    let output = written(&book);
    assert_eq!(
        outline_of(&output.xml),
        [
            "preface: Welcome",
            "part: Getting Started",
            "  chapter: Install",
            "    section: Requirements",
            "    section: Configure",
            "      section: Deep",
            "  chapter: Use",
            "part: Administration",
            "  chapter: Backup",
            "  chapter: Recovery",
            "    section: Restore",
        ]
    );
    let warnings: Vec<&str> = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    // An empty part is reported once, not again for the empty heading inside it.
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("\u{201c}Nothing Here\u{201d} has no pages"));
    assert!(warnings[1].contains("part \u{201c}Empty\u{201d} has no pages"));
}

/// With no navigation list titles, a top-level heading of the navigation is a part, and the
/// pages under it its chapters; a page the navigation puts first at a depth below the top is a
/// chapter; a later preface is a chapter.
#[test]
fn headings_without_parts_are_parts() {
    let book = DocBookDocument::Book(Book {
        title: "Manual".to_owned(),
        items: vec![
            book_page("p1", "deep-first", "= Deep First\n\nText.\n", 2),
            heading("Basics", 1),
            book_page("p2", "a", "= A\n\nText.\n", 2),
            heading("Topics", 2),
            book_page("p3", "b", "= B\n\nText.\n", 3),
            book_page("p4", "c", "= C\n\nText.\n", 1),
            BookItem::Preface(page("p5", "/docs/late.adoc", "= Late\n\nText.\n", 1)),
        ],
        ..Book::default()
    });
    let output = written(&book);
    assert_eq!(
        outline_of(&output.xml),
        [
            "chapter: Deep First",
            "part: Basics",
            "  chapter: A",
            "  chapter: Topics",
            "    section: B",
            "chapter: C",
            "chapter: Late",
        ]
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

/// A page with nothing of its own but pages nested under it needs no empty paragraph; one with
/// neither gets one.
#[test]
fn empty_pages_stay_docbook() {
    let book = DocBookDocument::Book(Book {
        title: "Manual".to_owned(),
        items: vec![
            book_page("p1", "parent", "= Parent\n", 1),
            book_page("p2", "child", "= Child\n", 2),
        ],
        ..Book::default()
    });
    let output = written(&book);
    let document = testing::parse(&output.xml);
    let chapter = elements(&document, "chapter").next().unwrap();
    assert_eq!(children(chapter), ["title", "section"]);
    let section = elements(&document, "section").next().unwrap();
    assert_eq!(children(section), ["title", "para"]);
}

#[test]
fn docbook_article_has_metadata() {
    let output = written(&article(
        "= The Guide: Getting the Most from It\n\
         Jane Q Doe <jane@example.com>; John Roe\n\
         v1.2, 2026-11-19: First release\n\
         :keywords: install, setup , \n\
         :description: How to install and set up the product.\n\
         :lang: en-GB\n\
         \n\
         Text.\n",
    ));
    let document = testing::parse(&output.xml);
    let info = elements(&document, "info").next().unwrap();
    assert_eq!(
        children(info),
        [
            "title",
            "subtitle",
            "author",
            "author",
            "date",
            "revhistory",
            "keywordset",
            "abstract"
        ]
    );
    let text = |name: &str| text_of(elements(&document, name).next().unwrap());
    assert_eq!(text("title"), "The Guide");
    assert_eq!(text("subtitle"), "Getting the Most from It");
    assert_eq!(text("firstname"), "Jane");
    assert_eq!(text("othername"), "Q");
    assert_eq!(text("surname"), "Doe");
    assert_eq!(text("email"), "jane@example.com");
    let authors: Vec<String> = elements(&document, "personname").map(text_of).collect();
    assert_eq!(authors, ["JaneQDoe", "JohnRoe"]);
    assert_eq!(text("revnumber"), "1.2");
    assert_eq!(text("date"), "2026-11-19");
    assert_eq!(text("revremark"), "First release");
    let keywords: Vec<String> = elements(&document, "keyword").map(text_of).collect();
    assert_eq!(keywords, ["install", "setup"]);
    assert_eq!(text("abstract"), "How to install and set up the product.");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

    // Nothing is made up: a page with no header has only its title.
    let output = written(&article("= Plain\n\nText.\n"));
    let document = testing::parse(&output.xml);
    let info = elements(&document, "info").next().unwrap();
    assert_eq!(children(info), ["title"]);

    // A version with no date is the release, which a revision cannot hold without one.
    let output = written(&article("= Plain\n:revnumber: 3.0\n\nText.\n"));
    let document = testing::parse(&output.xml);
    assert_eq!(
        text_of(elements(&document, "releaseinfo").next().unwrap()),
        "3.0"
    );
    assert_eq!(elements(&document, "revhistory").count(), 0);

    // A title with markup keeps it whole; a missing title is the file's name, with a warning.
    let output = written(&article("= The *Bold*: Part\n\nText.\n"));
    let document = testing::parse(&output.xml);
    assert_eq!(
        text_of(elements(&document, "title").next().unwrap()),
        "The Bold: Part"
    );
    assert_eq!(elements(&document, "subtitle").count(), 0);
    let output = written(&article("Text with no title.\n"));
    let document = testing::parse(&output.xml);
    assert_eq!(
        text_of(elements(&document, "title").next().unwrap()),
        "guide"
    );
    assert_eq!(output.diagnostics[0].severity, Severity::Warning);
    assert!(output.diagnostics[0].message.contains("no title"));
}

/// A book's metadata comes from its attributes: the component's, as the plan gives them.
#[test]
fn a_book_has_its_metadata() {
    let output = written(&DocBookDocument::Book(Book {
        title: "The Manual".to_owned(),
        subtitle: Some("For Administrators".to_owned()),
        attributes: [
            ("author", "Paul Snow"),
            ("revnumber", "2.0"),
            ("lang", "fr"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect(),
        items: vec![book_page("p1", "a", "= A\n\nText.\n", 1)],
    }));
    let document = testing::parse(&output.xml);
    let info = elements(&document, "info").next().unwrap();
    assert_eq!(
        children(info),
        ["title", "subtitle", "author", "releaseinfo"]
    );
    assert_eq!(
        text_of(elements(&document, "personname").next().unwrap()),
        "Paul Snow"
    );
    assert_eq!(
        document
            .root_element()
            .attribute(("http://www.w3.org/XML/1998/namespace", "lang")),
        Some("fr")
    );
}
