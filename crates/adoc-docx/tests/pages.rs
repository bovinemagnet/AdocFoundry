//! Headers, footers, covers, sections and the table of contents: PRD §10, tests 11
//! (`docx_running_content_has_live_fields`) and 12 (`docx_covers_and_sections`).

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use adoc_docx::{
    write_docx, BookItem, DocxBook, DocxDocument, DocxOptions, DocxPage, Line, Margins, Paper,
    Running, Slots,
};
use common::{assert_valid_package, text, PACKAGE_RELATIONSHIPS, R, W};
use roxmltree::{Document, Node};

const IMAGES: &str = "../../tests/fixtures/image-viewer/modules/ROOT/images";

fn copy(name: &str, to: &Path) {
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(IMAGES)
            .join(name),
        to,
    )
    .unwrap();
}

/// A book of two pages, the first setting the covers and the title page's logo.
fn book(folder: &Path) -> DocxDocument {
    copy("screenshot.jpg", &folder.join("cover.jpg"));
    copy("still.gif", &folder.join("back.gif"));
    copy("logo.png", &folder.join("logo.png"));
    let page = |name: &str, text: &str| {
        let path = folder.join(name);
        fs::write(&path, text).unwrap();
        common::view(path.to_str().unwrap(), text)
    };
    let a = page(
        "a.adoc",
        "= Page A\n:front-cover-image: cover.jpg\n:back-cover-image: image:back.gif[]\n\
         :title-logo-image: image:logo.png[]\n\nText.\n\n== Section A1\n\nMore.\n",
    );
    let b = page("b.adoc", "= Page B\n\nText.\n");
    DocxDocument::Book(DocxBook {
        title: "The Manual".to_owned(),
        subtitle: Some("For Everyone".to_owned()),
        attributes: BTreeMap::from([
            ("author".to_owned(), "Paul Snow".to_owned()),
            ("revnumber".to_owned(), "2.1".to_owned()),
            ("revdate".to_owned(), "2026-11-16".to_owned()),
            ("toc".to_owned(), String::new()),
        ]),
        items: vec![
            BookItem::Part("Getting Started".to_owned()),
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
    })
}

fn options(folder: &Path) -> DocxOptions {
    DocxOptions {
        paper: Paper::Letter,
        landscape: true,
        margins: Margins {
            top: 1000,
            right: 1100,
            bottom: 1200,
            left: 1300,
            header: 500,
            footer: 600,
        },
        running: Running {
            header: Slots {
                odd: Line {
                    left: "{document-title}".to_owned(),
                    right: "{logo}".to_owned(),
                    ..Line::default()
                },
                even: None,
                first: None,
            },
            footer: Slots {
                odd: Line {
                    left: "{chapter-title}".to_owned(),
                    right: "{page-number} of {page-count}".to_owned(),
                    ..Line::default()
                },
                even: Some(Line {
                    left: "{page-number}".to_owned(),
                    ..Line::default()
                }),
                first: Some(Line {
                    centre: "{page-number}".to_owned(),
                    ..Line::default()
                }),
            },
            odd_even: true,
            logo: Some(folder.join("logo.png")),
        },
        ..DocxOptions::default()
    }
}

fn written(document: &DocxDocument, options: &DocxOptions) -> BTreeMap<String, Vec<u8>> {
    let mut bytes = Vec::new();
    let report = write_docx(document, options, &mut bytes).unwrap();
    assert_eq!(report.diagnostics, Vec::new());
    assert_valid_package(&bytes)
}

fn w<'a>(node: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    node.children().find(|child| child.has_tag_name((W, name)))
}

fn attribute<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.attribute((W, name))
}

fn text_of(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(|child| child.has_tag_name((W, "t")))
        .filter_map(|child| child.text())
        .collect()
}

/// The instructions of the fields in `xml`.
fn fields(xml: &str) -> Vec<String> {
    Document::parse(xml)
        .unwrap()
        .descendants()
        .filter(|node| node.has_tag_name((W, "instrText")))
        .filter_map(|node| node.text().map(|text| text.trim().to_owned()))
        .collect()
}

/// Each section's properties, in order: those that end a section in its last paragraph, then
/// the body's own.
fn sections<'a>(document: &'a Document<'a>) -> Vec<Node<'a, 'a>> {
    document
        .descendants()
        .filter(|node| node.has_tag_name((W, "sectPr")))
        .collect()
}

/// The part a section's header or footer reference of `kind` leads to.
fn reference(
    entries: &BTreeMap<String, Vec<u8>>,
    section: Node<'_, '_>,
    element: &str,
    kind: &str,
) -> Option<String> {
    let id = section
        .children()
        .find(|node| node.has_tag_name((W, element)) && attribute(*node, "type") == Some(kind))?
        .attribute((R, "id"))?;
    let relationships = Document::parse(text(entries, "word/_rels/document.xml.rels")).unwrap();
    let target = relationships
        .descendants()
        .find(|node| node.attribute("Id") == Some(id))?
        .attribute("Target")?;
    Some(format!("word/{target}"))
}

#[test]
fn docx_running_content_has_live_fields() {
    let folder = tempfile::tempdir().unwrap();
    let entries = written(&book(folder.path()), &options(folder.path()));
    let document = Document::parse(text(&entries, "word/document.xml")).unwrap();
    let main = sections(&document)[2];

    // The body's section names its headers and footers, a first page's and an even page's.
    let part = |element: &str, kind: &str| {
        reference(&entries, main, element, kind).unwrap_or_else(|| panic!("no {kind} {element}"))
    };
    let header = part("headerReference", "default");
    let even_header = part("headerReference", "even");
    let first_footer = part("footerReference", "first");
    let footer = part("footerReference", "default");
    let even_footer = part("footerReference", "even");
    assert!(w(main, "titlePg").is_some(), "the first page has its own");
    assert_eq!(attribute(w(main, "pgNumType").unwrap(), "start"), Some("1"));
    let settings = text(&entries, "word/settings.xml");
    assert!(settings.contains("<w:evenAndOddHeaders/>"), "{settings}");

    // Page numbers and counts are live fields; the chapter's title is one too.
    assert_eq!(
        fields(text(&entries, &footer)),
        ["STYLEREF \"heading 2\"", "PAGE", "NUMPAGES"]
    );
    assert_eq!(fields(text(&entries, &even_footer)), ["PAGE"]);
    assert_eq!(fields(text(&entries, &first_footer)), ["PAGE"]);
    let types = text(&entries, "[Content_Types].xml");
    for (part, kind) in [(&header, "header"), (&footer, "footer")] {
        assert!(types.contains(&format!(
            "PartName=\"/{part}\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml\""
        )));
    }

    // The header has the title at its left and the logo at its right, separated by tabs, an
    // even page's mirrored; the logo is a relationship of the header itself.
    let header_xml = Document::parse(text(&entries, &header)).unwrap();
    assert_eq!(text_of(header_xml.root_element()), "The Manual");
    let tabs = header_xml
        .descendants()
        .filter(|node| {
            node.has_tag_name((W, "tab")) && node.parent().unwrap().has_tag_name((W, "r"))
        })
        .count();
    assert_eq!(tabs, 2, "left, centre and right");
    let blip = header_xml
        .descendants()
        .find(|node| node.tag_name().name() == "blip")
        .expect("the logo");
    let header_rels = header.replace("word/", "word/_rels/") + ".rels";
    let rels = Document::parse(text(&entries, &header_rels)).unwrap();
    let target = rels
        .descendants()
        .find(|node| node.attribute("Id") == blip.attribute((R, "embed")))
        .and_then(|node| node.attribute("Target"))
        .unwrap();
    assert!(entries.contains_key(&format!("word/{target}")));
    let even = Document::parse(text(&entries, &even_header)).unwrap();
    let even_runs: Vec<String> = even
        .descendants()
        .filter(|node| node.has_tag_name((W, "r")))
        .map(|run| {
            if run
                .descendants()
                .any(|node| node.tag_name().name() == "blip")
            {
                "logo".to_owned()
            } else if w(run, "tab").is_some() {
                "tab".to_owned()
            } else {
                text_of(run)
            }
        })
        .collect();
    assert_eq!(even_runs, ["logo", "tab", "tab", "The Manual"]);

    // A table of contents: a field Word updates on opening, its entries written already, each
    // a link to its heading. `toclevels` (2) counts from the chapters, as Asciidoctor's does in a
    // book, and a part is a level above them.
    assert!(
        settings.contains("<w:updateFields w:val=\"true\"/>"),
        "{settings}"
    );
    assert!(fields(text(&entries, "word/document.xml"))
        .contains(&"TOC \\o \"1-3\" \\h \\z \\u".to_owned()));
    let entries_written: Vec<(String, String)> = document
        .descendants()
        .filter(|node| node.has_tag_name((W, "p")))
        .filter_map(|paragraph| {
            let style = attribute(w(w(paragraph, "pPr")?, "pStyle")?, "val")?;
            style
                .starts_with("TOC")
                .then(|| (style.to_owned(), text_of(paragraph)))
        })
        .collect();
    assert_eq!(
        entries_written,
        [
            ("TOCHeading".to_owned(), "Table of Contents".to_owned()),
            ("TOC1".to_owned(), "Getting Started".to_owned()),
            ("TOC2".to_owned(), "Page A".to_owned()),
            ("TOC3".to_owned(), "Section A1".to_owned()),
            ("TOC2".to_owned(), "Page B".to_owned()),
        ]
    );
    let links: Vec<&str> = document
        .descendants()
        .filter(|node| node.has_tag_name((W, "hyperlink")))
        .filter_map(|node| attribute(node, "anchor"))
        .filter(|anchor| anchor.starts_with("_Toc"))
        .collect();
    assert_eq!(links.len(), 4, "{links:?}");
}

#[test]
fn docx_covers_and_sections() {
    let folder = tempfile::tempdir().unwrap();
    let entries = written(&book(folder.path()), &options(folder.path()));
    let document = Document::parse(text(&entries, "word/document.xml")).unwrap();
    let sections = sections(&document);
    assert_eq!(
        sections.len(),
        4,
        "the front cover, the title page, the body and the back cover"
    );

    // Every section is Letter, landscape.
    for section in &sections {
        let size = w(*section, "pgSz").unwrap();
        assert_eq!(attribute(size, "w"), Some("15840"));
        assert_eq!(attribute(size, "h"), Some("12240"));
        assert_eq!(attribute(size, "orient"), Some("landscape"));
    }
    let margins = |section: Node<'_, '_>| {
        let margins = w(section, "pgMar").unwrap();
        ["top", "right", "bottom", "left", "header", "footer"]
            .map(|side| attribute(margins, side).unwrap().to_owned())
    };
    assert_eq!(
        margins(sections[2]),
        ["1000", "1100", "1200", "1300", "500", "600"]
    );
    assert_eq!(
        margins(sections[1]),
        ["1000", "1100", "1200", "1300", "500", "600"]
    );
    // A cover fills its page.
    for cover in [sections[0], sections[3]] {
        assert_eq!(margins(cover), ["0", "0", "0", "0", "0", "0"]);
    }

    // The covers and the title page show no running content: the cover and the title page
    // name none, and the back cover, which would otherwise take the body's, names empty ones.
    for section in [sections[0], sections[1]] {
        assert!(
            !section
                .children()
                .any(|node| node.tag_name().name().ends_with("Reference")),
            "no header or footer"
        );
    }
    for kind in ["default", "even", "first"] {
        for element in ["headerReference", "footerReference"] {
            let part = reference(&entries, sections[3], element, kind)
                .unwrap_or_else(|| panic!("the back cover's {kind} {element}"));
            let xml = Document::parse(text(&entries, &part)).unwrap();
            assert_eq!(text_of(xml.root_element()), "");
            assert!(!xml
                .descendants()
                .any(|node| node.tag_name().name() == "blip"));
        }
    }

    // What each section holds, by its paragraphs up to its properties.
    let paragraphs: Vec<Node> = document
        .descendants()
        .filter(|node| node.has_tag_name((W, "p")))
        .collect();
    let ends: Vec<usize> = paragraphs
        .iter()
        .enumerate()
        .filter(|(_, paragraph)| {
            w(**paragraph, "pPr").is_some_and(|properties| w(properties, "sectPr").is_some())
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(ends.len(), 3);
    let pictures = |range: std::ops::Range<usize>| -> Vec<String> {
        paragraphs[range]
            .iter()
            .flat_map(|paragraph| paragraph.descendants())
            .filter(|node| node.tag_name().name() == "docPr")
            .filter_map(|node| node.attribute("descr").map(str::to_owned))
            .collect()
    };
    assert_eq!(pictures(0..ends[0] + 1), ["Front cover"]);
    assert_eq!(pictures(ends[0] + 1..ends[1] + 1), ["Logo"]);
    assert_eq!(pictures(ends[2] + 1..paragraphs.len()), ["Back cover"]);

    // The title page: the title, subtitle, author, version and date.
    let title_page: Vec<String> = paragraphs[ends[0] + 1..=ends[1]]
        .iter()
        .map(|paragraph| text_of(*paragraph))
        .filter(|text| !text.is_empty())
        .collect();
    assert_eq!(
        title_page,
        [
            "The Manual",
            "For Everyone",
            "Paul Snow",
            "Version 2.1",
            "2026-11-16"
        ]
    );

    // The covers' pictures are embedded, of their own types.
    let media: BTreeSet<&str> = entries
        .keys()
        .map(String::as_str)
        .filter(|name| name.starts_with("word/media/"))
        .collect();
    assert_eq!(media.len(), 3, "{media:?}");
    let _ = PACKAGE_RELATIONSHIPS;
}

#[test]
fn an_article_is_one_section_with_the_running_content() {
    let document = common::article("= An Article\n\nText.\n\n== One\n\nMore.\n");
    let entries = written(&document, &DocxOptions::default());
    let xml = Document::parse(text(&entries, "word/document.xml")).unwrap();
    let sections = sections(&xml);
    assert_eq!(sections.len(), 1);
    let size = w(sections[0], "pgSz").unwrap();
    assert_eq!(attribute(size, "w"), Some("11906"));
    assert_eq!(attribute(size, "orient"), None);
    let footer = reference(&entries, sections[0], "footerReference", "default").unwrap();
    assert_eq!(fields(text(&entries, &footer)), ["PAGE"]);
    assert!(reference(&entries, sections[0], "footerReference", "even").is_some());
    assert!(reference(&entries, sections[0], "headerReference", "default").is_none());
    // No table of contents unless the page asks for one.
    assert!(!text(&entries, "word/settings.xml").contains("updateFields"));
}

/// Writes the book to `$ADOC_DOCX_OUT/book.docx`, for opening in Word or LibreOffice by hand:
/// `ADOC_DOCX_OUT=/tmp cargo test -p adoc-docx --test pages -- --ignored`.
#[test]
#[ignore = "writes a file for a manual check"]
fn write_sample() {
    let out = std::env::var_os("ADOC_DOCX_OUT").expect("ADOC_DOCX_OUT names a folder");
    let folder = tempfile::tempdir().unwrap();
    let mut bytes = Vec::new();
    write_docx(&book(folder.path()), &options(folder.path()), &mut bytes).unwrap();
    fs::write(Path::new(&out).join("book.docx"), bytes).unwrap();
}
