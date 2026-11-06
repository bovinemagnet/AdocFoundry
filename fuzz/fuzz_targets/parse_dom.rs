//! `AcdcParser`, the full parser behind the preview, export and visual editor: it returns for
//! any text, on a thread with the stack the engine's workers have, and every block span in the
//! page is a range of the page's text on character boundaries.
//!
//! `catch_unwind` in `adoc-dom` keeps a panic in acdc-parser from taking the app down, but not
//! a stack overflow or an abort, which is what this target looks for. libFuzzer's panic hook
//! aborts on any panic, caught or not, so a panic the guard would catch is reported too: the
//! page would fall back to its source, and the panic is still a bug to report or fix.
//!
//! Run with `-rss_limit_mb=4096`, so memory blow-ups are reported as such.

#![no_main]

use std::{path::Path, sync::Arc};

use adoc_dom::{AcdcParser, DocumentParser, NoIncludes, ParseInput};
use libfuzzer_sys::fuzz_target;

/// The stack of a thread from `std::thread::spawn`, which the engine's workers parse on.
const WORKER_STACK: usize = 2 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn(move || check(&text))
        .expect("a parsing thread")
        .join()
        .expect("the parse returned");
});

fn check(text: &str) {
    let path = Path::new("fuzz.adoc");
    let dom = AcdcParser.parse(&ParseInput {
        path,
        text,
        attributes: Vec::new(),
        includes: Arc::new(NoIncludes),
    });
    for block in dom.walk() {
        let span = &block.meta.span;
        if span.path != path || !span.include_chain.is_empty() {
            continue;
        }
        let range = span.range;
        assert!(
            range.start <= range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end),
            "the {} block's span {range:?} is not a range of the {} bytes of text",
            block.kind_name(),
            text.len()
        );
    }
}
