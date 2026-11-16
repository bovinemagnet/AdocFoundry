//! Views and documents to write, and (from `adoc_docx::testing`) the package read back and
//! checked.

#![allow(dead_code)]

use std::path::Path;

pub use adoc_docx::testing::*;
use adoc_docx::{write_docx, DocxDocument, DocxOptions, DocxPage, DocxReport};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};

/// The view of `text` as a page at `path`.
pub fn view(path: &str, text: &str) -> adoc_render::ViewTree {
    let path = Path::new(path);
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

/// `text` as an article.
pub fn article(text: &str) -> DocxDocument {
    DocxDocument::Article(DocxPage {
        key: "p0".to_owned(),
        view: view("/docs/page.adoc", text),
        depth: 0,
    })
}

/// `document` written with the default options.
pub fn written(document: &DocxDocument) -> (Vec<u8>, DocxReport) {
    let mut bytes = Vec::new();
    let report = write_docx(document, &DocxOptions::default(), &mut bytes).unwrap();
    (bytes, report)
}
