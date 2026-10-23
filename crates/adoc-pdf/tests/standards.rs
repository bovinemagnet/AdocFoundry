//! WP 4.6: the index, PDF/A-2a and PDF/UA-1, and the PDF's metadata.

mod common;

use std::path::Path;

use adoc_pdf::{export, to_typst, typeset_pages, PdfOptions, PdfStandard};
use adoc_render::{ViewInline, ViewKind};

fn options(standards: &[PdfStandard]) -> PdfOptions {
    PdfOptions {
        standards: standards.to_vec(),
        date: Some((2026, 10, 24)),
        ..PdfOptions::default()
    }
}

#[test]
fn index_terms_reach_the_dom() {
    let view = common::view_of_text(
        Path::new("/doc/page.adoc"),
        "= Page\n\nA ((flow term)) and a hidden one.(((primary, secondary, tertiary)))\n",
    );
    let ViewKind::Paragraph(content) = &view.blocks[0].kind else {
        panic!("{:?}", view.blocks[0].kind)
    };
    let terms: Vec<(&Vec<String>, &Vec<ViewInline>)> = content
        .iter()
        .filter_map(|inline| match inline {
            ViewInline::IndexTerm { terms, content } => Some((terms, content)),
            _ => None,
        })
        .collect();
    assert_eq!(terms.len(), 2, "{content:?}");
    assert_eq!(terms[0].0, &["flow term"]);
    assert_eq!(ViewInline::plain_text(terms[0].1), "flow term");
    assert_eq!(terms[1].0, &["primary", "secondary", "tertiary"]);
    assert!(terms[1].1.is_empty(), "a concealed term shows nothing");
    // The text reads as it did before index terms were kept.
    assert_eq!(
        ViewInline::plain_text(content),
        "A flow term and a hidden one."
    );
}

#[test]
fn index_lists_terms_with_pages() {
    let text = "= Guide\n\n== One\n\nAbout ((apples)) and ((Bananas)).\n\n<<<\n\n== Two\n\nMore ((apples)).(((fruit, cherries)))\n\n[index]\n== Index\n";
    let view = common::view_of_text(Path::new("/doc/guide.adoc"), text);
    let typst = to_typst(
        &common::article(view),
        &common::default_theme(),
        &options(&[]),
    );
    let pages = typeset_pages(&typst, &common::default_theme(), &options(&[]))
        .unwrap_or_else(|error| panic!("{error}"));
    let last = pages.last().unwrap().text();
    let squeezed: String = last.chars().filter(|c| !c.is_whitespace()).collect();
    // Letters, terms in order, and the pages each is on, once each.
    for expected in [
        "A",
        "apples,1,2",
        "B",
        "Bananas,1",
        "F",
        "fruit",
        "cherries,2",
    ] {
        assert!(squeezed.contains(expected), "{expected:?} not in {last:?}");
    }
    let apples = squeezed.find("apples").unwrap();
    let bananas = squeezed.find("Bananas").unwrap();
    let fruit = squeezed.find("fruit").unwrap();
    assert!(apples < bananas && bananas < fruit);
}

#[test]
fn pdf_a_2a_export_is_conformant() {
    let path = common::fixtures().join("pdf/reference/article.adoc");
    let document = common::article(common::view_of(&path));
    let output = export(
        &document,
        &common::default_theme(),
        &options(&[PdfStandard::A2a]),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let bytes = String::from_utf8_lossy(&output.bytes);
    assert!(
        bytes.contains("<pdfaid:part>2</pdfaid:part>"),
        "no PDF/A part"
    );
    assert!(bytes.contains("<pdfaid:conformance>A</pdfaid:conformance>"));
    // Both standards at once.
    let output = export(
        &document,
        &common::default_theme(),
        &options(&[PdfStandard::A2a, PdfStandard::Ua1]),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let bytes = String::from_utf8_lossy(&output.bytes);
    assert!(bytes.contains("pdfaid:part") && bytes.contains("pdfuaid:part"));
}

#[test]
fn pdf_ua_1_requires_alt_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        common::fixtures().join("pdf/reference/images/pipeline.svg"),
        dir.path().join("export-pipeline.svg"),
    )
    .unwrap();
    let page = dir.path().join("page.adoc");
    std::fs::write(&page, "= Page\n\nimage::export-pipeline.svg[]\n").unwrap();
    let document = common::article(common::view_of(&page));
    // An image without alt text is given its file name, as Asciidoctor does.
    let typst = to_typst(
        &document,
        &common::default_theme(),
        &options(&[PdfStandard::Ua1]),
    );
    assert!(
        typst.source.contains("alt: \"export pipeline\""),
        "{}",
        typst.source
    );
    let output = export(
        &document,
        &common::default_theme(),
        &options(&[PdfStandard::Ua1]),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(String::from_utf8_lossy(&output.bytes).contains("<pdfuaid:part>1</pdfuaid:part>"));
}

#[test]
fn metadata_comes_from_attributes() {
    let text = "= The Title\nAda Writer\n:description: What the document is about.\n:keywords: pdf, typst, asciidoc\n:lang: en-GB\n\nBody.\n";
    let document = common::article(common::view_of_text(Path::new("/doc/meta.adoc"), text));
    let output = export(
        &document,
        &common::default_theme(),
        &options(&[PdfStandard::A2a]),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let bytes = String::from_utf8_lossy(&output.bytes);
    for expected in [
        "The Title",
        "Ada Writer",
        "What the document is about.",
        "typst",
    ] {
        assert!(
            bytes.contains(expected),
            "{expected:?} is not in the metadata"
        );
    }
}

/// The structure tree in reading order: each element's depth, type and title or alt text.
fn tag_tree(bytes: &[u8]) -> Vec<(usize, String, String)> {
    use hayro_syntax::object::{Dict, Name, Object, String as PdfString};

    fn text(bytes: &[u8]) -> String {
        match bytes.strip_prefix(&[0xfe, 0xff]) {
            Some(utf16) => String::from_utf16_lossy(
                &utf16
                    .chunks(2)
                    .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
                    .collect::<Vec<_>>(),
            ),
            None => String::from_utf8_lossy(bytes).into_owned(),
        }
    }
    fn walk(object: Object<'_>, depth: usize, out: &mut Vec<(usize, String, String)>) {
        match object {
            Object::Array(kids) => kids.iter::<Object>().for_each(|kid| walk(kid, depth, out)),
            Object::Dict(element) => {
                // Marked-content and object references have no structure type.
                let Some(kind) = element.get::<Name>(b"S") else {
                    return;
                };
                let label = [&b"T"[..], b"Alt"]
                    .iter()
                    .find_map(|key| element.get::<PdfString>(*key))
                    .map_or_else(String::new, |label| text(label.as_bytes()));
                out.push((depth, kind.as_str().to_owned(), label));
                if let Some(kids) = element.get::<Object>(b"K") {
                    walk(kids, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let pdf = hayro_syntax::Pdf::new(bytes.to_vec()).unwrap();
    let catalog: Dict = pdf.xref().get(pdf.xref().root_id()).unwrap();
    let root: Dict = catalog.get(b"StructTreeRoot").expect("a structure tree");
    let mut out = Vec::new();
    walk(root.get::<Object>(b"K").unwrap(), 0, &mut out);
    out
}

#[test]
fn the_tag_tree_reads_in_order() {
    // What a screen reader announces: headings in order, lists as lists with their labels,
    // tables with header cells, and images by their alternative text.
    let path = common::fixtures().join("pdf/reference/article.adoc");
    let document = common::article(common::view_of(&path));
    let output = export(
        &document,
        &common::default_theme(),
        &options(&[PdfStandard::Ua1]),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let tree = tag_tree(&output.bytes);
    let headings: Vec<&str> = tree
        .iter()
        .filter(|(_, kind, _)| kind.starts_with('H') && kind.len() == 2)
        .map(|(_, _, title)| title.as_str())
        .collect();
    assert_eq!(headings[0], "Exporting Documentation to PDF");
    assert!(headings.contains(&"2. Lists") && headings.contains(&"6. Tables"));
    let kinds: Vec<&str> = tree.iter().map(|(_, kind, _)| kind.as_str()).collect();
    let lists = kinds.iter().filter(|kind| **kind == "L").count();
    assert!(lists >= 4, "{lists} lists: {kinds:?}");
    for (index, (depth, kind, _)) in tree.iter().enumerate() {
        if kind == "LI" {
            let children: Vec<&str> = tree[index + 1..]
                .iter()
                .take_while(|(inner, _, _)| inner > depth)
                .filter(|(inner, _, _)| *inner == depth + 1)
                .map(|(_, kind, _)| kind.as_str())
                .collect();
            assert_eq!(children, ["Lbl", "LBody"], "list item {index}");
        }
    }
    assert!(kinds.contains(&"Table") && kinds.contains(&"TH") && kinds.contains(&"TD"));
    assert!(tree
        .iter()
        .any(|(_, kind, alt)| kind == "Figure" && alt == "Pipeline"));
    // The order: lists come under their heading, before the tables.
    let position = |wanted: &str| {
        tree.iter()
            .position(|(_, _, title)| title == wanted)
            .unwrap()
    };
    let first_list = kinds.iter().position(|kind| *kind == "L").unwrap();
    assert!(position("2. Lists") < first_list && first_list < position("6. Tables"));
}
