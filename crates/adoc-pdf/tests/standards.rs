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
