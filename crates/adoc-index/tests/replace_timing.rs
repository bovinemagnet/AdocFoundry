//! Timing probe for `WorkspaceIndex::replace`; ignored by default. Run with
//! `PROBE_FILE=<file.adoc> cargo test --release -p adoc-index --test replace_timing -- --ignored --nocapture`.

use std::{path::PathBuf, time::Instant};

use adoc_index::WorkspaceIndex;

#[test]
#[ignore = "timing probe"]
fn replace_a_large_document() {
    let text = std::fs::read_to_string(std::env::var("PROBE_FILE").unwrap()).unwrap();
    let path = PathBuf::from("/probe/synthetic.adoc");
    let mut index = WorkspaceIndex::new();
    index.index_source(&path, &text);
    let document = adoc_parser::parse("file:///probe/synthetic.adoc", &text).document;
    let mut samples = Vec::new();
    for _ in 0..50 {
        let copy = document.clone();
        let started = Instant::now();
        index.replace(path.clone(), copy);
        samples.push(started.elapsed());
    }
    samples.sort();
    let started = Instant::now();
    for _ in 0..50 {
        let _ = document.clone();
    }
    println!(
        "replace median {:?} worst {:?}; document clone {:?}",
        samples[25],
        samples[49],
        started.elapsed() / 50
    );
}
