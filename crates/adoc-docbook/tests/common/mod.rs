//! What the tests share: view trees made from AsciiDoc as the engine makes them, and the
//! documents read back.

#![allow(dead_code)]

use std::path::Path;

use adoc_docbook::{BookItem, DocBookDocument, Page, NAMESPACE};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};

pub fn view(path: &str, text: &str) -> adoc_render::ViewTree {
    let path = Path::new(path);
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

pub fn page(key: &str, path: &str, text: &str, depth: u8) -> Page {
    Page {
        key: key.to_owned(),
        view: view(path, text),
        depth,
    }
}

pub fn article(text: &str) -> DocBookDocument {
    DocBookDocument::Article(page("p0", "/docs/modules/ROOT/pages/guide.adoc", text, 0))
}

pub fn book_page(key: &str, name: &str, text: &str, depth: u8) -> BookItem {
    BookItem::Page(page(
        key,
        &format!("/docs/modules/ROOT/pages/{name}.adoc"),
        text,
        depth,
    ))
}

/// The DocBook elements named `name`, in document order.
pub fn elements<'a, 'input>(
    document: &'a roxmltree::Document<'input>,
    name: &'a str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> + 'a {
    document
        .descendants()
        .filter(move |node| node.has_tag_name((NAMESPACE, name)))
}

/// The text inside `node`, all of it.
pub fn text_of(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants()
        .filter(|node| node.is_text())
        .filter_map(|node| node.text())
        .collect()
}

/// The names of `node`'s child elements, in order.
pub fn children(node: roxmltree::Node<'_, '_>) -> Vec<String> {
    node.children()
        .filter(roxmltree::Node::is_element)
        .map(|child| child.tag_name().name().to_owned())
        .collect()
}
