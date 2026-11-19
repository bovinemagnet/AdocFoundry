//! DocBook export's writer under arbitrary pages: for any text, the page's document is written
//! without a panic and passes the crate's own check (well-formed XML, the root, unique valid
//! ids, every `linkend` resolving, relative picture paths). What it looks for is the XML, the
//! ids and the links going wrong on input no fixture has. The RELAX NG schema is left to the
//! tests, being too slow to run on every input.

#![no_main]

use std::path::Path;

use adoc_docbook::{check, write_docbook, DocBookDocument, DocBookOptions, Page};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let path = Path::new("/fuzz/page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, &text));
    let view = adoc_render::build_view(path, &dom, &adoc_render::LocalResolver);
    let document = DocBookDocument::Article(Page {
        key: "fuzz-page".to_owned(),
        view,
        depth: 0,
    });
    let output = write_docbook(&document, &DocBookOptions::default())
        .unwrap_or_else(|error| panic!("DocBook: {error}"));
    assert_eq!(check(&output.xml), Vec::<String>::new());
});
