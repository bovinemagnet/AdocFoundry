//! Kindle export's writers under arbitrary pages: for any text, the page's book is written in
//! both formats without a panic, each file is read back whole by the independent readers of
//! `adoc_kindle::testing`, and every link inside it leads to a place the book holds. What it
//! looks for is the writers' offset, fragment and index arithmetic going wrong on input no
//! fixture has.

#![no_main]

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_kindle::{
    testing::{read_azw3, read_kfx},
    write_kindle, KindleDocument, KindleFormat, KindleOptions, KindlePage,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let path = Path::new("/fuzz/page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, &text));
    let view = adoc_render::build_view(path, &dom, &adoc_render::LocalResolver);
    let document = KindleDocument::Article(KindlePage {
        key: "p0".to_owned(),
        view,
        depth: 0,
    });
    let options = KindleOptions::default();

    let azw3 = write_kindle(&document, KindleFormat::Azw3, &options)
        .unwrap_or_else(|error| panic!("AZW3: {error}"));
    let read = read_azw3(&azw3.bytes).unwrap_or_else(|error| panic!("AZW3 read back: {error}"));
    for (_, href) in read.hrefs() {
        if href.starts_with("kindle:pos") {
            assert!(read.follow(&href).is_some(), "{href} leads nowhere");
        }
    }

    let kfx = write_kindle(&document, KindleFormat::Kfx, &options)
        .unwrap_or_else(|error| panic!("KFX: {error}"));
    let read = read_kfx(&kfx.bytes).unwrap_or_else(|error| panic!("KFX read back: {error}"));
    let chapters = read
        .chapters()
        .unwrap_or_else(|error| panic!("KFX chapters: {error}"));
    for block in chapters.iter().flatten() {
        for (_, _, anchor) in &block.links {
            assert!(read.anchor(*anchor).is_some(), "a link to no anchor");
        }
    }
});
