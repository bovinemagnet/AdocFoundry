//! DocBook 1/9 (#649): the root, its namespaces, version and `info`, escaping and determinism
//! (PRD §11 tests 1 to 3). Every document is checked against the DocBook 5.2.1 RELAX NG schema.

mod common;

use adoc_docbook::{
    testing, write_docbook, Book, BookItem, DocBookDocument, DocBookOptions, Severity, NAMESPACE,
    VERSION, XLINK,
};
use common::{article, book_page, children, elements, text_of};

fn written(document: &DocBookDocument) -> adoc_docbook::DocBookOutput {
    let output = write_docbook(document, &DocBookOptions::default()).expect("written");
    testing::assert_valid(&output.xml);
    output
}

#[test]
fn docbook_root_is_wellformed_and_versioned() {
    let output = written(&article(
        "= A Guide\n\nThe first paragraph.\n\n== Usage\n\nUse it.\n",
    ));
    let document = testing::parse(&output.xml);
    let root = document.root_element();
    assert!(root.has_tag_name((NAMESPACE, "article")));
    assert_eq!(root.attribute("version"), Some(VERSION));
    assert_eq!(root.lookup_namespace_uri(Some("xlink")), Some(XLINK));
    assert_eq!(
        children(root),
        ["info", "para", "section"],
        "info comes first"
    );
    let info = root.first_element_child().unwrap();
    assert_eq!(children(info), ["title"]);
    assert_eq!(text_of(info), "A Guide");
    assert!(output
        .xml
        .starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

    let book = DocBookDocument::Book(Book {
        title: "The Manual".to_owned(),
        items: vec![
            BookItem::Part("Getting Started".to_owned()),
            book_page("p1", "install", "= Install\n\nRun it.\n", 1),
            book_page("p2", "use", "= Use\n\nUse it.\n", 1),
            BookItem::Part("Reference".to_owned()),
            book_page("p3", "options", "= Options\n\n== Flags\n\nNone.\n", 1),
        ],
        ..Book::default()
    });
    let output = written(&book);
    let document = testing::parse(&output.xml);
    let root = document.root_element();
    assert!(root.has_tag_name((NAMESPACE, "book")));
    assert_eq!(root.attribute("version"), Some(VERSION));
    assert_eq!(children(root), ["info", "part", "part"]);
    let parts: Vec<_> = elements(&document, "part").collect();
    assert_eq!(children(parts[0]), ["title", "chapter", "chapter"]);
    assert_eq!(children(parts[1]), ["title", "chapter"]);
    let chapters: Vec<String> = elements(&document, "chapter")
        .map(|chapter| text_of(chapter.first_element_child().unwrap()))
        .collect();
    assert_eq!(chapters, ["Install", "Use", "Options"]);
}

#[test]
fn a_document_has_a_language_when_it_names_one() {
    let output = written(&article("= Guide\n:lang: en-GB\n\nText.\n"));
    let document = testing::parse(&output.xml);
    assert_eq!(
        document
            .root_element()
            .attribute(("http://www.w3.org/XML/1998/namespace", "lang")),
        Some("en-GB")
    );
}

#[test]
fn docbook_text_is_escaped() {
    let text = "= <Tags> & \"Quotes\" ]]>\n\n\
                Markup <b>bold</b> &amp; entities &lt; <!DOCTYPE x [<!ENTITY e \"y\">]> &e;\n\n\
                Unicode: 日本語, Ελληνικά, עברית, 😀.\n\n\
                Control: a\u{1}b\u{7}c.\n\n\
                ++++\n<evil xmlns=\"x\"/><?pi?>\n++++\n\n\
                Inline pass:[<b>raw</b>] text.\n";
    let output = written(&article(text));
    assert!(!output.xml.contains("<!DOCTYPE"), "no DOCTYPE is written");
    assert!(
        !output.xml.contains("<evil"),
        "passthrough is never raw XML"
    );
    assert!(
        !output.xml.contains("<b>"),
        "inline passthrough is never raw XML"
    );
    assert!(!output.xml.contains("<?pi"));
    let document = testing::parse(&output.xml);
    let title = text_of(elements(&document, "title").next().unwrap());
    assert!(
        title.starts_with("<Tags> & ") && title.ends_with(" ]]>"),
        "{title}"
    );
    let all = text_of(document.root_element());
    assert!(all.contains("日本語, Ελληνικά, עברית, 😀"), "{all}");
    assert!(
        all.contains("<!DOCTYPE x"),
        "markup in text stays text: {all}"
    );
    assert!(
        all.contains("a\u{FFFD}b\u{FFFD}c"),
        "what XML forbids is replaced: {all}"
    );
    assert!(
        all.contains("<evil"),
        "the passthrough's text is kept: {all}"
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("passthrough")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn an_empty_or_untitled_document_is_still_docbook() {
    let output = written(&article(""));
    let document = testing::parse(&output.xml);
    assert_eq!(
        text_of(elements(&document, "title").next().unwrap()),
        "guide"
    );
    let warning = &output.diagnostics[0];
    assert_eq!(warning.severity, Severity::Warning);
    assert!(warning.message.contains("no title"), "{}", warning.message);
    assert_eq!(
        warning.source.as_ref().map(|source| source.line),
        Some(1),
        "the warning names the page"
    );

    let output = written(&article("= Guide\n\n== Empty\n"));
    assert_eq!(output.diagnostics, []);

    // A book of no pages, and a part with none.
    let output = written(&DocBookDocument::Book(Book {
        title: "Empty".to_owned(),
        items: vec![BookItem::Part("Nothing".to_owned())],
        ..Book::default()
    }));
    assert!(output.diagnostics[0].message.contains("no pages"));
    assert_eq!(
        children(testing::parse(&output.xml).root_element()),
        ["info"]
    );
}

#[test]
fn docbook_output_is_deterministic() {
    let document = DocBookDocument::Book(Book {
        title: "Manual".to_owned(),
        items: vec![
            book_page(
                "p1",
                "a",
                "= A\n\nOne *two* _three_ `four`.\n\n== B\n\nFive.\n",
                1,
            ),
            book_page("p2", "b", "= B\n\nSix^7^ ~8~.\n", 1),
        ],
        ..Book::default()
    });
    let first = written(&document);
    let second = written(&document);
    assert_eq!(first.xml, second.xml);
    assert_eq!(first.diagnostics, second.diagnostics);
}
