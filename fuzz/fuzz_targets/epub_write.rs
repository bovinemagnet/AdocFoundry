//! EPUB export's writer under arbitrary pages: for any text, the page's book is written without a
//! panic, passes the crate's own validator, and is read back whole by the independent reader of
//! `adoc_epub::testing`, every link and picture reference resolving. What it looks for is the
//! XHTML, the ids and the links going wrong on input no fixture has.

#![no_main]

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_epub::{testing, validate, write_epub, Document, EpubOptions, Modified, Page};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let path = Path::new("/fuzz/page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, &text));
    let view = adoc_render::build_view(path, &dom, &adoc_render::LocalResolver);
    let document = Document::Article(Page {
        key: "p0".to_owned(),
        view,
        depth: 0,
    });
    let modified = Modified::from_unix_seconds(1_795_046_400).expect("a datable time");
    let output = write_epub(&document, &EpubOptions::new(modified))
        .unwrap_or_else(|error| panic!("EPUB: {error}"));
    assert_eq!(validate(&output.bytes), Vec::<String>::new());
    testing::assert_links_resolve(&output.bytes);
});
