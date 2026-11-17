//! Views, documents and books to write in tests.

#![allow(dead_code)]

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_kindle::{BookItem, KindleDocument, KindlePage, KindlePlan};

/// The view of `text` as a page at `path`. Cross references resolve to files beside it that
/// exist.
pub fn view(path: &Path, text: &str) -> adoc_render::ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

/// `text` as an article.
pub fn article(text: &str) -> KindleDocument {
    KindleDocument::Article(KindlePage {
        key: "p0".to_owned(),
        view: view(Path::new("/docs/page.adoc"), text),
        depth: 0,
    })
}

/// The pages written into `folder` as files, then read as a book in that order, each at
/// depth 1.
pub fn book(folder: &Path, title: &str, pages: &[(&str, &str)]) -> KindleDocument {
    for (name, text) in pages {
        std::fs::write(folder.join(name), text).unwrap();
    }
    let items = pages
        .iter()
        .enumerate()
        .map(|(index, (name, text))| {
            BookItem::Page(KindlePage {
                key: format!("p{}", index + 1),
                view: view(&folder.join(name), text),
                depth: 1,
            })
        })
        .collect();
    KindleDocument::Book(KindlePlan {
        title: title.to_owned(),
        items,
        ..KindlePlan::default()
    })
}
