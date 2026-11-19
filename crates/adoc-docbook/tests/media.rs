//! DocBook 5/9 (#653): pictures copied once and referred to by relative paths, diagrams and
//! formulas, and what cannot be shown reported with its description kept (PRD §5.4, §11 tests
//! 12 to 14). Every document is checked against the DocBook 5.2.1 RELAX NG schema.

mod common;

use std::{fs, path::Path};

use adoc_core::FileAccess;
use adoc_docbook::{testing, write_docbook, AssetPolicy, DocBookDocument, DocBookOptions, Page};
use adoc_render::{DiagramView, FormulaImage, ViewBlock, ViewInline, ViewKind};
use common::{children, elements, text_of};

const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];
const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>";

/// A page at `folder/page.adoc` with `text`, its images found beside it.
fn page_in(folder: &Path, text: &str) -> Page {
    let path = folder.join("page.adoc");
    fs::write(&path, text).unwrap();
    Page {
        key: "p0".to_owned(),
        view: common::view(path.to_str().unwrap(), text),
        depth: 0,
    }
}

fn options(policy: AssetPolicy) -> DocBookOptions {
    DocBookOptions {
        access: FileAccess::Any,
        assets: policy,
    }
}

fn copy() -> AssetPolicy {
    AssetPolicy::Copy {
        folder: "guide_assets".to_owned(),
    }
}

fn written(page: Page, options: &DocBookOptions) -> adoc_docbook::DocBookOutput {
    let output = write_docbook(&DocBookDocument::Article(page), options).expect("written");
    testing::assert_valid(&output.xml);
    output
}

fn filerefs(xml: &str) -> Vec<String> {
    let document = testing::parse(xml);
    document
        .descendants()
        .filter_map(|node| node.attribute("fileref"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn docbook_assets_are_copied_and_relative() {
    let folder = tempfile::tempdir().unwrap();
    let dir = folder.path();
    fs::write(dir.join("logo.png"), PNG).unwrap();
    fs::write(dir.join("same.png"), PNG).unwrap();
    fs::write(dir.join("photo.jpg"), JPEG).unwrap();
    fs::write(dir.join("drawing.svg"), SVG).unwrap();
    let page = page_in(
        dir,
        "= Pictures\n\n.The logo\nimage::logo.png[The logo,200]\n\nimage::photo.jpg[A photo]\n\n\
         image::drawing.svg[A drawing]\n\nThe same picture again image:same.png[Same] inline, and \
         image:logo.png[Logo] once more.\n",
    );
    let output = written(page, &options(copy()));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    // Identical content is stored once, named in the order first used.
    let names: Vec<&str> = output
        .assets
        .iter()
        .map(|asset| asset.name.as_str())
        .collect();
    assert_eq!(names, ["i1.png", "i2.jpg", "i3.svg"]);
    assert_eq!(output.assets[0].bytes, PNG);
    assert_eq!(
        filerefs(&output.xml),
        [
            "guide_assets/i1.png",
            "guide_assets/i2.jpg",
            "guide_assets/i3.svg",
            "guide_assets/i1.png",
            "guide_assets/i1.png"
        ]
    );
    // Nothing names where the pictures came from.
    let checkout = dir.to_string_lossy();
    assert!(!output.xml.contains(checkout.as_ref()));
    let document = testing::parse(&output.xml);
    let figure = elements(&document, "figure").next().unwrap();
    assert_eq!(children(figure), ["title", "mediaobject"]);
    let media = elements(&document, "mediaobject").next().unwrap();
    assert_eq!(children(media), ["imageobject", "textobject"]);
    assert_eq!(text_of(media), "The logo");
    let data = elements(&document, "imagedata").next().unwrap();
    assert_eq!(data.attribute("contentwidth"), Some("200"));
    assert_eq!(elements(&document, "informalfigure").count(), 2);
    assert_eq!(elements(&document, "inlinemediaobject").count(), 2);

    // Referred to where they are, nothing is copied and paths climb from the document.
    let out = dir.join("out");
    let output = write_docbook(
        &DocBookDocument::Article(page_in(dir, "= Pictures\n\nimage::logo.png[The logo]\n")),
        &options(AssetPolicy::Reference { folder: out }),
    )
    .expect("written");
    testing::assert_valid_referenced(&output.xml);
    assert!(output.assets.is_empty());
    assert_eq!(filerefs(&output.xml), ["../logo.png"]);
}

#[test]
fn docbook_missing_media_is_reported() {
    let folder = tempfile::tempdir().unwrap();
    let dir = folder.path();
    fs::write(dir.join("notes.png"), b"not a picture").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    fs::write(elsewhere.path().join("outside.png"), PNG).unwrap();
    let outside = elsewhere.path().join("outside.png");
    let page = page_in(
        dir,
        &format!(
            "= Missing\n\n.A caption\nimage::gone.png[What it showed]\n\n\
             image::notes.png[Not a picture]\n\n\
             image::https://example.com/remote.png[Remote]\n\n\
             image::{}[Outside]\n",
            outside.display()
        ),
    );
    let options = DocBookOptions {
        access: FileAccess::within([dir.to_path_buf()]),
        assets: copy(),
    };
    let output = written(page, &options);
    assert!(output.assets.is_empty());
    let document = testing::parse(&output.xml);
    // The caption and the description stay where the picture would be.
    let figure = elements(&document, "figure").next().unwrap();
    assert_eq!(text_of(figure), "A captionWhat it showed");
    assert_eq!(filerefs(&output.xml), ["https://example.com/remote.png"]);
    let messages: Vec<&str> = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(messages.len(), 4, "{messages:?}");
    assert!(
        messages[0].contains("gone.png is not found"),
        "{messages:?}"
    );
    assert!(
        messages[1].contains("not a PNG, JPEG, GIF or SVG"),
        "{messages:?}"
    );
    assert!(
        messages[2].contains("by its address, not copied"),
        "{messages:?}"
    );
    assert!(messages[3].contains("could not be read"), "{messages:?}");
    let lines: Vec<u32> = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.source.as_ref().unwrap().line)
        .collect();
    // Each is located at its block, its title's line included.
    assert_eq!(lines, [3, 6, 8, 10]);
}

/// A view with `kinds` as its blocks, each spanning line 3 of the page.
fn view_with(kinds: Vec<(Vec<ViewInline>, ViewKind)>) -> Page {
    let mut view = common::view(
        "/docs/modules/ROOT/pages/guide.adoc",
        "= Formulas\n\nText.\n",
    );
    let span = view.blocks[0].span.clone();
    for (title, kind) in kinds {
        view.blocks.push(ViewBlock {
            span: span.clone(),
            id: None,
            roles: Vec::new(),
            style: None,
            title,
            caption: None,
            attributes: Default::default(),
            options: Default::default(),
            kind,
        });
    }
    Page {
        key: "p0".to_owned(),
        view,
        depth: 0,
    }
}

#[test]
fn docbook_formulas_have_a_fallback() {
    let folder = tempfile::tempdir().unwrap();
    let drawn = folder.path().join("formula.svg");
    fs::write(&drawn, SVG).unwrap();
    let page = view_with(vec![
        (
            vec![ViewInline::Text("Euler".to_owned())],
            ViewKind::Stem {
                latex: true,
                source: "e^{i\\pi} + 1 = 0".to_owned(),
                rendered: DiagramView::Svg(drawn.clone()),
            },
        ),
        (
            Vec::new(),
            ViewKind::Stem {
                latex: false,
                source: "sum_(i=1)^n i".to_owned(),
                rendered: DiagramView::Failed {
                    kind: "stem-asciimath".to_owned(),
                    message: "no Typst".to_owned(),
                    source: "sum_(i=1)^n i".to_owned(),
                },
            },
        ),
        (
            Vec::new(),
            ViewKind::Paragraph(vec![
                ViewInline::Text("Inline ".to_owned()),
                ViewInline::Stem {
                    latex: false,
                    source: "a^2".to_owned(),
                    rendered: Some(FormulaImage {
                        path: drawn,
                        width: 10.0,
                        height: 10.0,
                        baseline: 8.0,
                    }),
                },
                ViewInline::Text(" and ".to_owned()),
                ViewInline::Stem {
                    latex: false,
                    source: "b^2".to_owned(),
                    rendered: None,
                },
            ]),
        ),
    ]);
    let output = written(page, &options(copy()));
    let document = testing::parse(&output.xml);
    let equation = elements(&document, "equation").next().unwrap();
    assert_eq!(children(equation), ["title", "alt", "mediaobject"]);
    let informal = elements(&document, "informalequation").next().unwrap();
    assert_eq!(
        children(informal),
        ["mathphrase"],
        "no picture, still the source"
    );
    let inline: Vec<Vec<String>> = elements(&document, "inlineequation")
        .map(children)
        .collect();
    assert_eq!(
        inline,
        [
            vec!["alt".to_owned(), "inlinemediaobject".to_owned()],
            vec!["mathphrase".to_owned()]
        ]
    );
    // A formula drawn keeps its source as its alternative; one not drawn is its source.
    let drawn: Vec<String> = elements(&document, "alt").map(text_of).collect();
    assert_eq!(drawn, ["e^{i\\pi} + 1 = 0", "a^2"]);
    let math: Vec<String> = elements(&document, "phrase")
        .filter(|phrase| phrase.attribute("role") == Some("math"))
        .map(text_of)
        .collect();
    assert_eq!(math, drawn);
    let sources: Vec<String> = elements(&document, "mathphrase").map(text_of).collect();
    assert_eq!(sources, ["sum_(i=1)^n i", "b^2"]);
    // The picture drawn of a formula is stored once.
    assert_eq!(output.assets.len(), 1);
    // One warning for the page says what was lost.
    assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
    assert!(output.diagnostics[0]
        .message
        .contains("4 formulas are written"));
    assert!(output.diagnostics[0].message.contains("not as MathML"));
}

#[test]
fn diagrams_are_pictures_or_their_source() {
    let folder = tempfile::tempdir().unwrap();
    let drawn = folder.path().join("diagram.svg");
    fs::write(&drawn, SVG).unwrap();
    let page = view_with(vec![
        (
            vec![ViewInline::Text("Flow".to_owned())],
            ViewKind::Diagram(DiagramView::Svg(drawn)),
        ),
        (
            Vec::new(),
            ViewKind::Diagram(DiagramView::Failed {
                kind: "graphviz".to_owned(),
                message: "this export starts no program".to_owned(),
                source: "digraph { a -> b }".to_owned(),
            }),
        ),
        (
            Vec::new(),
            ViewKind::Diagram(DiagramView::Pending {
                kind: "mermaid".to_owned(),
                source: "graph LR; a-->b".to_owned(),
            }),
        ),
        (
            Vec::new(),
            ViewKind::Media {
                kind: adoc_dom::MediaKind::Video,
                targets: vec![
                    "https://example.com/v.mp4".to_owned(),
                    "local.mp4".to_owned(),
                ],
            },
        ),
    ]);
    let output = written(page, &options(copy()));
    let document = testing::parse(&output.xml);
    let figure = elements(&document, "figure").next().unwrap();
    assert_eq!(text_of(figure), "FlowA diagram");
    assert_eq!(
        filerefs(&output.xml),
        ["guide_assets/i1.svg", "https://example.com/v.mp4"]
    );
    let sources: Vec<String> = elements(&document, "programlisting").map(text_of).collect();
    assert_eq!(sources, ["digraph { a -> b }", "graph LR; a-->b"]);
    let messages: Vec<&str> = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    // A failed diagram is reported by the export, as for every format; a pending one and a
    // local video here.
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].contains("not drawn in time"));
    assert!(messages[1].contains("not copied"));
}
