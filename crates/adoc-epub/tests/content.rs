//! EPUB 3/9 (#640): AsciiDoc's blocks and inlines as semantic XHTML, read back in process
//! (PRD §11 tests 7 to 9).

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_epub::{testing, write_epub, Document, EpubOptions, Modified, Page, Severity};

fn view(path: &Path, text: &str) -> adoc_render::ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

fn article_of(view: adoc_render::ViewTree) -> Document {
    Document::Article(Page {
        key: "p0".to_owned(),
        view,
        depth: 0,
    })
}

fn options() -> EpubOptions {
    EpubOptions::new(Modified::from_unix_seconds(1_795_084_200).unwrap())
}

/// The first chapter's text, as written.
fn chapter(bytes: &[u8]) -> String {
    let (files, _) = testing::assert_valid_ocf(bytes);
    testing::text(&files, "EPUB/text/c1.xhtml").to_owned()
}

const XHTML: &str = testing::XHTML;

fn elements<'a, 'input>(
    document: &'a roxmltree::Document<'input>,
    name: &'a str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> + 'a {
    document
        .descendants()
        .filter(move |node| node.has_tag_name((XHTML, name)))
}

fn text_of(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants()
        .filter(|node| node.is_text())
        .filter_map(|node| node.text())
        .collect()
}

const PAGE: &str = r#"= Content
:experimental:

[#lists]
== Lists

* one
** one a
*** one a i
* two

. first
. second

[%interactive]
* [x] done
* [ ] to do

CPU:: The brain.
RAM:: The memory.

== A table

.Sizes
[cols="1,1,1",options="header,footer"]
|===
|Name |Size |Notes

2+|Spans two |Plain
.2+|Spans down |A |B
|C |D
|Total |3 |
|===

[cols="1,1"]
|===
a|
[cols="1,1"]
!===
!Inner !Table
!===
|Beside
|===

== Code and text

[source,rust]
----
fn main() {
    println!("<&>");
}
----

Unicode: 日本語, Ελληνικά, 🙂. See https://example.com[the site] and <<lists>>.
Press kbd:[Ctrl+C], then btn:[Save], then menu:File[Export > EPUB].

NOTE: A note.

WARNING: A warning.

.A sidebar
****
Set apart.
****

.An example
====
Shown.
====

[quote,Someone,Somewhere]
____
Quoted.
____

<<<

After the break.
"#;

/// PRD §11 test 7.
#[test]
fn epub_content_is_wellformed_xhtml() {
    let output = write_epub(
        &article_of(view(Path::new("/docs/content.adoc"), PAGE)),
        &options(),
    )
    .unwrap();
    let text = chapter(&output.bytes);
    let document = testing::parse("c1", &text);

    // A section a chapter, headings below the page's own.
    let section = elements(&document, "section").next().unwrap();
    assert_eq!(section.attribute((testing::OPS, "type")), Some("chapter"));
    assert_eq!(elements(&document, "h1").count(), 1);
    let h2: Vec<String> = elements(&document, "h2").map(text_of).collect();
    assert_eq!(h2, ["Lists", "A table", "Code and text"]);
    assert!(elements(&document, "h2").all(|h| h.attribute("id").is_some()));

    // Lists nest, ordered lists are ordered, a description list is one.
    let deepest = elements(&document, "li")
        .find(|li| text_of(*li).trim() == "one a i")
        .unwrap();
    let depth = deepest
        .ancestors()
        .filter(|node| node.has_tag_name((XHTML, "ul")))
        .count();
    assert_eq!(depth, 3);
    let ordered = elements(&document, "ol").next().unwrap();
    assert_eq!(elements(&document, "ol").count(), 1);
    assert!(text_of(ordered).contains("second"));
    assert!(
        text.contains("☑") && text.contains("☐"),
        "check lists keep their state"
    );
    let terms: Vec<String> = elements(&document, "dt")
        .map(|dt| text_of(dt).trim().to_owned())
        .collect();
    assert_eq!(terms, ["CPU", "RAM"]);

    // The complex table: a header, a footer, spans, and a table in a cell kept a table.
    let table = elements(&document, "table").next().unwrap();
    let groups: Vec<&str> = table
        .children()
        .filter(|node| node.is_element())
        .map(|node| node.tag_name().name())
        .collect();
    assert_eq!(groups, ["thead", "tbody", "tfoot"]);
    let headers: Vec<String> = table
        .descendants()
        .filter(|node| node.has_tag_name((XHTML, "th")))
        .map(|th| text_of(th).trim().to_owned())
        .collect();
    assert_eq!(headers, ["Name", "Size", "Notes"]);
    assert!(table
        .descendants()
        .any(|node| node.attribute("colspan") == Some("2")));
    assert!(table
        .descendants()
        .any(|node| node.attribute("rowspan") == Some("2")));
    let caption = elements(&document, "p")
        .find(|p| p.attribute("class") == Some("caption"))
        .map(text_of)
        .unwrap();
    assert!(caption.contains("Sizes"), "{caption}");
    let nested = elements(&document, "table")
        .find(|table| {
            table
                .ancestors()
                .skip(1)
                .any(|node| node.has_tag_name((XHTML, "td")))
        })
        .expect("a table inside a table's cell is a table");
    assert!(text_of(nested).contains("Inner"));

    // Code keeps its lines and its text, escaped and read back.
    let code = elements(&document, "pre").next().unwrap();
    assert_eq!(code.attribute("class"), Some("listing"));
    let listing = text_of(code);
    assert!(
        listing.contains("fn main() {\n    println!(\"<&>\");"),
        "{listing}"
    );

    // Unicode, links, keys, buttons and menus.
    let body = text_of(document.root_element());
    assert!(body.contains("日本語, Ελληνικά, 🙂"), "{body}");
    let links: Vec<(String, String)> = elements(&document, "a")
        .map(|a| {
            (
                a.attribute("href").unwrap_or_default().to_owned(),
                text_of(a),
            )
        })
        .collect();
    assert!(links.contains(&("https://example.com".to_owned(), "the site".to_owned())));
    let lists_id = elements(&document, "h2")
        .find(|h| text_of(*h) == "Lists")
        .and_then(|h| h.attribute("id"))
        .unwrap();
    assert!(
        links
            .iter()
            .any(|(href, _)| *href == format!("#{lists_id}")),
        "a reference in the chapter is a fragment: {links:?}"
    );
    let keys: Vec<String> = elements(&document, "kbd").map(text_of).collect();
    assert_eq!(keys, ["Ctrl", "C"]);
    let classes: Vec<(String, String)> = elements(&document, "span")
        .filter_map(|span| Some((span.attribute("class")?.to_owned(), text_of(span))))
        .collect();
    assert!(
        classes.contains(&("button".to_owned(), "Save".to_owned())),
        "{classes:?}"
    );
    assert!(
        classes.contains(&("menu".to_owned(), "File › Export › EPUB".to_owned())),
        "{classes:?}"
    );

    // Admonitions, a sidebar and an example are told apart; a quote is a quote.
    let divs: Vec<String> = elements(&document, "div")
        .filter_map(|div| div.attribute("class").map(str::to_owned))
        .collect();
    for class in [
        "admonition note",
        "admonition warning",
        "sidebar",
        "example",
    ] {
        assert!(divs.iter().any(|div| div == class), "{class} in {divs:?}");
    }
    let note = elements(&document, "div")
        .find(|div| div.attribute("class") == Some("admonition note"))
        .unwrap();
    assert_eq!(note.attribute("role"), Some("note"));
    assert!(text_of(note).contains("Note") && text_of(note).contains("A note."));
    let quote = elements(&document, "blockquote").next().unwrap();
    assert!(text_of(quote).contains("Quoted.") && text_of(quote).contains("Someone"));

    // A page break is a separator, the text after it kept.
    assert!(elements(&document, "hr").any(|hr| hr.attribute("class") == Some("page-break")));
    assert!(body.contains("After the break."));
}

/// PRD §11 test 8.
#[test]
fn epub_footnotes_link_both_ways() {
    let page = "= Notes\n\nFirst.footnote:once[The first note.] Second.footnote:[Another.]\n\n\
                Again.footnote:once[]\n";
    let output = write_epub(
        &article_of(view(Path::new("/docs/notes.adoc"), page)),
        &options(),
    )
    .unwrap();
    let text = chapter(&output.bytes);
    let document = testing::parse("c1", &text);
    let by_id = |id: &str| {
        document
            .descendants()
            .find(|node| node.attribute("id") == Some(id))
            .unwrap_or_else(|| panic!("nothing has the id {id}"))
    };
    let refs: Vec<roxmltree::Node<'_, '_>> = elements(&document, "a")
        .filter(|a| a.attribute((testing::OPS, "type")) == Some("noteref"))
        .collect();
    assert_eq!(refs.len(), 3, "every mark, the repeated one included");
    for mark in &refs {
        let href = mark.attribute("href").unwrap();
        let note = by_id(href.strip_prefix('#').unwrap());
        assert!(note.has_tag_name((XHTML, "aside")));
        assert_eq!(note.attribute((testing::OPS, "type")), Some("footnote"));
    }
    // The repeated mark leads to the same note, and has no id of its own.
    assert_eq!(refs[0].attribute("href"), refs[2].attribute("href"));
    assert!(refs[2].attribute("id").is_none());
    // Each note leads back to its first mark.
    let notes: Vec<roxmltree::Node<'_, '_>> = elements(&document, "aside").collect();
    assert_eq!(notes.len(), 2);
    for note in notes {
        let back = note
            .descendants()
            .find(|node| node.has_tag_name((XHTML, "a")))
            .and_then(|a| a.attribute("href"))
            .unwrap();
        let mark = by_id(back.strip_prefix('#').unwrap());
        assert_eq!(mark.attribute((testing::OPS, "type")), Some("noteref"));
    }
    assert!(text_of(document.root_element()).contains("The first note."));
}

/// PRD §11 test 9.
#[test]
fn epub_unsupported_nodes_warn() {
    let page = "= Odd\n\nBefore.\n\n++++\n<script>alert(1)</script><p onclick=\"x()\">Raw</p>\n++++\n\nAfter.\n";
    let path = Path::new("/docs/odd.adoc");
    let mut tree = view(path, page);
    // A block the renderer does not support, as a parser gives one.
    let mut unsupported = tree.blocks[0].clone();
    unsupported.kind = adoc_render::ViewKind::Unsupported {
        name: "widget".to_owned(),
        source: "widget::x[]".to_owned(),
    };
    tree.blocks.push(unsupported);
    let output = write_epub(&article_of(tree), &options()).unwrap();
    let text = chapter(&output.bytes);
    let document = testing::parse("c1", &text);

    // Passthrough is shown as text, never as markup: no script, no handler.
    assert!(elements(&document, "script").next().is_none());
    assert!(!document
        .descendants()
        .any(|node| node.attribute("onclick").is_some()));
    let body = text_of(document.root_element());
    assert!(body.contains("<script>alert(1)</script>"), "{body}");
    assert!(
        body.contains("widget::x[]"),
        "the unsupported block's source is kept"
    );
    assert!(body.contains("Before.") && body.contains("After."));

    // Each gives a located warning.
    let warnings: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .collect();
    let passthrough = warnings
        .iter()
        .find(|diagnostic| diagnostic.message.contains("passthrough"))
        .expect("a passthrough warning");
    let source = passthrough.source.as_ref().expect("located");
    assert_eq!(source.path, path);
    assert_eq!(source.line, 5);
    assert!(
        warnings
            .iter()
            .any(|diagnostic| diagnostic.message.contains("widget") && diagnostic.source.is_some()),
        "{warnings:?}"
    );
}
