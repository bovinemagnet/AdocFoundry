//! Rich content as Word structures: PRD §10, tests 6 (`docx_rich_content_uses_word_structures`),
//! 7 (`docx_internal_links_reach_bookmarks`) and 8 (`docx_unsupported_nodes_warn`).

mod common;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use adoc_docx::{
    write_docx, BookItem, DocxBook, DocxDocument, DocxOptions, DocxPage, DocxSeverity,
};
use adoc_render::{SourceSpan, ViewBlock, ViewKind, ViewTree};
use common::{article, assert_valid_package, paragraphs, text, written, W};
use roxmltree::{Document, Node};

const RICH: &str = r#"= Rich Content
:experimental:

A paragraph with *bold*, _italic_, `code`, #marked#, ^super^ and ~sub~ text, +
a hard break, kbd:[Ctrl+S], btn:[Save] and menu:File[Save As].
A footnote.footnote:note[The note text.] And again.footnote:note[]

* One
* Two
** Two point one
. First
. Second
+
A paragraph in the item.

//
* [ ] To do
* [x] Done

Term:: Its definition.

[source,rust]
----
fn main() {
    println!("<&>"); // <1>
}
----
<1> Prints.

[cols="1,1,1",options="header"]
|===
|A |B |C
2+|Spans two |Three
.2+|Down two |b |c
|e |f
|===

NOTE: Mind the gap.

____
A quotation.
____

****
A sidebar.
****

====
An example.
====
"#;

fn parse(xml: &str) -> Document<'_> {
    Document::parse(xml).unwrap()
}

fn w<'a>(node: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    node.children().find(|child| child.has_tag_name((W, name)))
}

fn attribute<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.attribute((W, name))
}

/// The text of `node`: its `w:t` elements, tabs and breaks.
fn text_of(node: Node<'_, '_>) -> String {
    let mut out = String::new();
    for child in node.descendants() {
        if child.has_tag_name((W, "t")) {
            out.push_str(child.text().unwrap_or_default());
        } else if child.has_tag_name((W, "tab")) {
            out.push('\t');
        } else if child.has_tag_name((W, "br")) {
            out.push('\n');
        }
    }
    out
}

/// The runs whose text is `text`, with their properties.
fn run_with<'a>(document: &'a Document<'a>, text: &str) -> Node<'a, 'a> {
    document
        .descendants()
        .filter(|node| node.has_tag_name((W, "r")))
        .find(|run| text_of(*run) == text)
        .unwrap_or_else(|| panic!("no run of {text:?}"))
}

/// The paragraphs of `style`, as their text.
fn styled(document: &Document<'_>, style: &str) -> Vec<String> {
    document
        .descendants()
        .filter(|node| node.has_tag_name((W, "p")))
        .filter(|paragraph| {
            paragraph
                .descendants()
                .find(|node| node.has_tag_name((W, "pStyle")))
                .and_then(|node| attribute(node, "val"))
                == Some(style)
        })
        .map(text_of)
        .collect()
}

/// The table whose style is `style`.
fn table<'a>(document: &'a Document<'a>, style: &str) -> Node<'a, 'a> {
    document
        .descendants()
        .filter(|node| node.has_tag_name((W, "tbl")))
        .find(|table| {
            table
                .descendants()
                .find(|node| node.has_tag_name((W, "tblStyle")))
                .and_then(|node| attribute(node, "val"))
                == Some(style)
        })
        .unwrap_or_else(|| panic!("no table in the style {style}"))
}

#[test]
fn docx_rich_content_uses_word_structures() {
    let (bytes, report) = written(&article(RICH));
    let entries = assert_valid_package(&bytes);
    assert_eq!(report.diagnostics, Vec::new());
    let xml = text(&entries, "word/document.xml");
    let document = parse(xml);

    // Inline formatting is run properties and character styles.
    let has = |run: Node<'_, '_>, property: &str| {
        w(run, "rPr").is_some_and(|properties| w(properties, property).is_some())
    };
    let property = |run: Node<'_, '_>, property: &str| {
        w(run, "rPr")
            .and_then(|properties| w(properties, property))
            .and_then(|node| attribute(node, "val"))
            .map(str::to_owned)
    };
    assert!(has(run_with(&document, "bold"), "b"));
    assert!(has(run_with(&document, "italic"), "i"));
    assert_eq!(
        property(run_with(&document, "code"), "rStyle").as_deref(),
        Some("CodeChar")
    );
    assert_eq!(
        property(run_with(&document, "marked"), "highlight").as_deref(),
        Some("yellow")
    );
    assert_eq!(
        property(run_with(&document, "super"), "vertAlign").as_deref(),
        Some("superscript")
    );
    assert_eq!(
        property(run_with(&document, "sub"), "vertAlign").as_deref(),
        Some("subscript")
    );
    assert_eq!(
        property(run_with(&document, "Ctrl"), "rStyle").as_deref(),
        Some("Keyboard")
    );
    let first = document
        .descendants()
        .find(|node| node.has_tag_name((W, "p")) && text_of(*node).starts_with("A paragraph"))
        .unwrap();
    assert!(
        text_of(first).contains(" text,\na hard break"),
        "{:?}",
        text_of(first)
    );
    assert!(text_of(first).contains("[Save]"));
    assert!(text_of(first).contains("File › Save As"));

    // Lists are Word numbering: bullets and an ordered list, nested by level.
    let numbering = parse(text(&entries, "word/numbering.xml"));
    let formats: BTreeMap<String, BTreeSet<String>> = numbering
        .descendants()
        .filter(|node| node.has_tag_name((W, "abstractNum")))
        .map(|definition| {
            let formats = definition
                .descendants()
                .filter(|node| node.has_tag_name((W, "numFmt")))
                .filter_map(|node| attribute(node, "val").map(str::to_owned))
                .collect();
            (
                attribute(definition, "abstractNumId").unwrap().to_owned(),
                formats,
            )
        })
        .collect();
    assert!(
        formats.values().any(|formats| formats.contains("bullet")),
        "{formats:?}"
    );
    assert!(
        formats.values().any(|formats| !formats.contains("bullet")),
        "an ordered list: {formats:?}"
    );
    let levels: Vec<(String, String)> = document
        .descendants()
        .filter(|node| node.has_tag_name((W, "numPr")))
        .map(|numbering| {
            let paragraph = numbering
                .ancestors()
                .find(|node| node.has_tag_name((W, "p")))
                .unwrap();
            let level = attribute(w(numbering, "ilvl").unwrap(), "val")
                .unwrap()
                .to_owned();
            (level, text_of(paragraph))
        })
        .collect();
    assert_eq!(
        levels,
        [
            ("0".to_owned(), "One".to_owned()),
            ("0".to_owned(), "Two".to_owned()),
            ("1".to_owned(), "Two point one".to_owned()),
            ("2".to_owned(), "First".to_owned()),
            ("2".to_owned(), "Second".to_owned()),
        ]
    );
    let item_paragraph = document
        .descendants()
        .find(|node| node.has_tag_name((W, "p")) && text_of(*node) == "A paragraph in the item.")
        .unwrap();
    let indent = w(w(item_paragraph, "pPr").unwrap(), "ind").expect("indented under its item");
    assert!(attribute(indent, "left").unwrap().parse::<u32>().unwrap() > 0);
    // A checklist shows its boxes.
    let texts: Vec<String> = paragraphs(xml).into_iter().map(|(_, text)| text).collect();
    assert!(texts.contains(&"☐\tTo do".to_owned()), "{texts:?}");
    assert!(texts.contains(&"☑\tDone".to_owned()), "{texts:?}");

    // A description list.
    assert_eq!(styled(&document, "DescriptionTerm"), ["Term"]);
    assert!(texts.contains(&"Its definition.".to_owned()));

    // Code keeps its whitespace and characters, with its callouts.
    let code = styled(&document, "SourceCode");
    assert_eq!(code, ["fn main() {\n    println!(\"<&>\"); (1)\n}"]);
    assert!(texts.contains(&"(1)\tPrints.".to_owned()), "{texts:?}");

    // A real table: a header row repeated, a cell across two columns, one down two rows.
    let grid = table(&document, "AdocTable");
    let columns = w(grid, "tblGrid")
        .unwrap()
        .children()
        .filter(|node| node.is_element())
        .count();
    assert_eq!(columns, 3);
    let rows: Vec<Node> = grid
        .children()
        .filter(|node| node.has_tag_name((W, "tr")))
        .collect();
    assert_eq!(rows.len(), 4);
    assert!(w(w(rows[0], "trPr").unwrap(), "tblHeader").is_some());
    for row in &rows {
        let width: usize = row
            .children()
            .filter(|node| node.has_tag_name((W, "tc")))
            .map(|cell| {
                w(cell, "tcPr")
                    .and_then(|properties| w(properties, "gridSpan"))
                    .and_then(|span| attribute(span, "val"))
                    .map_or(1, |span| span.parse().unwrap())
            })
            .sum();
        assert_eq!(width, 3, "every row fills the grid");
    }
    let merge = |row: Node<'_, '_>, index: usize| {
        let cell = row
            .children()
            .filter(|node| node.has_tag_name((W, "tc")))
            .nth(index)
            .unwrap();
        let merge = w(cell, "tcPr").and_then(|properties| w(properties, "vMerge"));
        (
            merge.map(|merge| attribute(merge, "val").unwrap_or("continue").to_owned()),
            text_of(cell),
        )
    };
    assert_eq!(
        merge(rows[2], 0),
        (Some("restart".to_owned()), "Down two".to_owned())
    );
    assert_eq!(
        merge(rows[3], 0),
        (Some("continue".to_owned()), String::new())
    );
    assert_eq!(merge(rows[3], 1), (None, "e".to_owned()));

    // Footnotes: one Word footnote, and the second reference a field naming it.
    let footnotes = parse(text(&entries, "word/footnotes.xml"));
    let notes: Vec<String> = footnotes
        .descendants()
        .filter(|node| node.has_tag_name((W, "footnote")) && attribute(*node, "type").is_none())
        .map(text_of)
        .collect();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("The note text."), "{notes:?}");
    let references = document
        .descendants()
        .filter(|node| node.has_tag_name((W, "footnoteReference")))
        .count();
    assert_eq!(references, 1);
    let instruction: String = document
        .descendants()
        .filter(|node| node.has_tag_name((W, "instrText")))
        .filter_map(|node| node.text())
        .collect();
    let bookmark = instruction
        .split_whitespace()
        .skip_while(|word| *word != "NOTEREF")
        .nth(1)
        .expect("a NOTEREF field");
    assert!(
        document
            .descendants()
            .any(|node| node.has_tag_name((W, "bookmarkStart"))
                && attribute(node, "name") == Some(bookmark)),
        "the field names the bookmark around the first reference"
    );

    // Containers hold their blocks, editable.
    let note = table(&document, "AdocAdmonition");
    let cells: Vec<String> = note
        .descendants()
        .filter(|node| node.has_tag_name((W, "tc")))
        .map(text_of)
        .collect();
    assert_eq!(cells, ["Note", "Mind the gap."]);
    assert_eq!(styled(&document, "Quote"), ["A quotation."]);
    assert_eq!(text_of(table(&document, "AdocSidebar")), "A sidebar.");
    assert_eq!(text_of(table(&document, "AdocExample")), "An example.");
}

/// Pages on disk, so their references resolve as the workspace's would: every file is
/// written before any view is built.
fn pages(folder: &Path, pages: &[(&str, &str)]) -> Vec<(PathBuf, ViewTree)> {
    for (name, text) in pages {
        std::fs::write(folder.join(name), text).unwrap();
    }
    pages
        .iter()
        .map(|(name, text)| {
            let path = folder.join(name);
            (path.clone(), common::view(path.to_str().unwrap(), text))
        })
        .collect()
}

#[test]
fn docx_internal_links_reach_bookmarks() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path();
    let mut written_pages = pages(
        folder,
        &[
            (
                "a.adoc",
                "= Page A\n\nSee xref:b.adoc#details[the details], <<local,here>>, xref:b.adoc[page B], \
                 xref:c.adoc[page C], xref:missing.adoc[nothing] and https://example.com[a site].\n\n[#local]\n== Local Section\n\nText.\n",
            ),
            ("b.adoc", "= Page B\n\n[#details]\n== Details\n\nMore.\n"),
            ("c.adoc", "= Page C\n"),
        ],
    )
    .into_iter();
    let (_, a) = written_pages.next().unwrap();
    let (_, b) = written_pages.next().unwrap();
    let (c_path, _) = written_pages.next().unwrap();
    let book = DocxDocument::Book(DocxBook {
        title: "Links".to_owned(),
        items: vec![
            BookItem::Page(DocxPage {
                key: "p1".to_owned(),
                view: a,
                depth: 1,
            }),
            BookItem::Page(DocxPage {
                key: "p2".to_owned(),
                view: b,
                depth: 1,
            }),
        ],
        ..DocxBook::default()
    });
    let options = DocxOptions {
        page_urls: HashMap::from([(c_path, "https://docs.example.com/c.html".to_owned())]),
        ..DocxOptions::default()
    };
    let mut bytes = Vec::new();
    let report = write_docx(&book, &options, &mut bytes).unwrap();
    // Every internal link leads to a bookmark: `assert_valid_package` checks it.
    let entries = assert_valid_package(&bytes);
    let document = parse(text(&entries, "word/document.xml"));

    // Each internal link reaches the bookmark of what it names.
    let bookmark_text = |name: &str| {
        let start = document
            .descendants()
            .find(|node| {
                node.has_tag_name((W, "bookmarkStart")) && attribute(*node, "name") == Some(name)
            })
            .unwrap();
        text_of(
            start
                .ancestors()
                .find(|node| node.has_tag_name((W, "p")))
                .unwrap(),
        )
    };
    let links: Vec<(String, Option<String>, Option<String>)> = document
        .descendants()
        .filter(|node| node.has_tag_name((W, "hyperlink")))
        .map(|link| {
            (
                text_of(link),
                attribute(link, "anchor").map(bookmark_text),
                link.attribute((common::R, "id")).map(str::to_owned),
            )
        })
        .collect();
    let internal: Vec<(&str, &str)> = links
        .iter()
        .filter_map(|(text, target, _)| Some((text.as_str(), target.as_deref()?)))
        .collect();
    assert_eq!(
        internal,
        [
            ("the details", "Details"),
            ("here", "Local Section"),
            ("page B", "Page B"),
        ]
    );

    // A page outside the book goes to its published address; a web address stays one.
    let relationships = parse(text(&entries, "word/_rels/document.xml.rels"));
    let external: BTreeMap<&str, (&str, Option<&str>)> = relationships
        .descendants()
        .filter(|node| node.attribute("TargetMode") == Some("External"))
        .map(|node| {
            (
                node.attribute("Id").unwrap(),
                (node.attribute("Target").unwrap(), node.attribute("Type")),
            )
        })
        .collect();
    let target = |text: &str| {
        let (_, _, id) = links.iter().find(|(found, _, _)| found == text).unwrap();
        external[id.as_deref().unwrap()].0
    };
    assert_eq!(target("page C"), "https://docs.example.com/c.html");
    assert_eq!(target("a site"), "https://example.com");
    for (_, kind) in external.values() {
        assert_eq!(
            *kind,
            Some("http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink")
        );
    }

    // A reference that leads nowhere is text, and reported where it is.
    assert!(!links.iter().any(|(text, _, _)| text == "nothing"));
    let texts: String = paragraphs(text(&entries, "word/document.xml"))
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(texts.contains("nothing"));
    let warnings: Vec<(DocxSeverity, u32, &str)> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.severity,
                diagnostic.source.as_ref().unwrap().line,
                diagnostic.message.as_str(),
            )
        })
        .collect();
    assert_eq!(
        warnings,
        [(
            DocxSeverity::Warning,
            3,
            "the cross reference missing.adoc leads to nothing and is exported as text"
        )]
    );
}

#[test]
fn a_link_that_is_not_a_web_or_mail_address_is_text() {
    // The parser reads none of these as links, but a view may still hold one.
    let mut view = common::view("/docs/page.adoc", "= Page\n\nText.\n");
    let ViewKind::Paragraph(content) = &mut view.blocks[0].kind else {
        panic!("a paragraph");
    };
    content.push(adoc_render::ViewInline::Link {
        url: "javascript:alert(1)".to_owned(),
        content: vec![adoc_render::ViewInline::Text("a script".to_owned())],
    });
    let document = DocxDocument::Article(DocxPage {
        key: "p0".to_owned(),
        view,
        depth: 0,
    });
    let (bytes, report) = written(&document);
    let entries = assert_valid_package(&bytes);
    let document = parse(text(&entries, "word/document.xml"));
    assert!(!document
        .descendants()
        .any(|node| node.has_tag_name((W, "hyperlink"))));
    assert!(text_of(document.root_element()).contains("a script"));
    let messages: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["the link javascript:alert(1) is not a web or mail address and is exported as text"]
    );
}

fn block(line: u32, kind: ViewKind) -> ViewBlock {
    ViewBlock {
        span: SourceSpan {
            path: PathBuf::from("/docs/page.adoc"),
            range: adoc_core::SourceRange::default(),
            line,
            include_chain: Vec::new(),
        },
        id: None,
        roles: Vec::new(),
        style: None,
        title: Vec::new(),
        caption: None,
        attributes: BTreeMap::new(),
        options: BTreeSet::new(),
        kind,
    }
}

#[test]
fn docx_unsupported_nodes_warn() {
    let view = ViewTree {
        source: PathBuf::from("/docs/page.adoc"),
        blocks: vec![
            block(
                3,
                ViewKind::Unsupported {
                    name: "widget".to_owned(),
                    source: "widget::x[]".to_owned(),
                },
            ),
            block(
                5,
                ViewKind::Media {
                    kind: adoc_render::MediaKind::Video,
                    targets: vec!["intro.mp4".to_owned()],
                },
            ),
        ],
        ..ViewTree::default()
    };
    let document = DocxDocument::Article(DocxPage {
        key: "p0".to_owned(),
        view,
        depth: 0,
    });
    let (bytes, report) = written(&document);
    let entries = assert_valid_package(&bytes);
    let texts: Vec<String> = paragraphs(text(&entries, "word/document.xml"))
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    // Shown, not dropped.
    assert!(
        texts.iter().any(|text| text.contains("widget::x[]")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("intro.mp4")),
        "{texts:?}"
    );
    let warnings: Vec<(DocxSeverity, u32, &str)> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.severity,
                diagnostic.source.as_ref().unwrap().line,
                diagnostic.message.as_str(),
            )
        })
        .collect();
    assert_eq!(
        warnings,
        [
            (
                DocxSeverity::Warning,
                3,
                "an unsupported block (widget) is exported as its source"
            ),
            (
                DocxSeverity::Warning,
                5,
                "a video or audio block is exported as its address"
            ),
        ]
    );
}

/// Writes the rich page to `$ADOC_DOCX_OUT/rich.docx`, for opening in Word or LibreOffice by
/// hand: `ADOC_DOCX_OUT=/tmp cargo test -p adoc-docx --test content -- --ignored`.
#[test]
#[ignore = "writes a file for a manual check"]
fn write_sample() {
    let folder = std::env::var_os("ADOC_DOCX_OUT").expect("ADOC_DOCX_OUT names a folder");
    let (bytes, _) = written(&article(RICH));
    std::fs::write(Path::new(&folder).join("rich.docx"), bytes).unwrap();
}
