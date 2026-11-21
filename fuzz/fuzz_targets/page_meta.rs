//! A page's metadata file (`.<page>.afd`, comments): any bytes a repository holds must be refused
//! or read without panicking, and a file read must be written again as one that reads back to
//! the same comments, so the engine never writes a file it would then refuse.
#![no_main]

use adoc_engine::page_meta::{parse, MetaProblem, PageMeta};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(meta) = parse(data) else {
        return;
    };
    if meta.newer.is_some() {
        return;
    }
    match meta.render(&meta.comments) {
        Ok(Some(text)) => {
            let again: PageMeta = parse(text.as_bytes()).expect("a file written is read");
            assert_eq!(again.comments, meta.comments);
        }
        // Written in the engine's own layout, a file near the limit may no longer fit.
        Ok(None) | Err(MetaProblem::TooMany) => {}
        Err(problem) => panic!("a file read is written: {problem}"),
    }
});
