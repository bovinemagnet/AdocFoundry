//! Memory probe for the index; ignored by default. Run it under `/usr/bin/time -l` (macOS) or
//! `/usr/bin/time -v` (Linux) and compare the peak with `PROBE_PAGES=0`:
//! `PROBE_PAGES=10000 /usr/bin/time -l cargo test --release -p adoc-index --test memory -- --ignored --nocapture`.

use std::{fmt::Write as _, path::PathBuf, time::Instant};

use adoc_index::WorkspaceIndex;

/// A page of about 8 KB: ten sections of prose, each with an anchor and two xrefs.
fn page(number: usize) -> String {
    let mut text = format!("= Page {number}\n:description: Page {number} of the probe.\n\n");
    for section in 0..10 {
        let _ = write!(
            text,
            "[[page-{number}-section-{section}]]\n== Section {section} of page {number}\n\n"
        );
        for _ in 0..6 {
            text.push_str(
                "A paragraph of prose long enough to look like documentation, with a few words \
                 of every length in it, so the page is of an ordinary size.\n\n",
            );
        }
        let _ = write!(
            text,
            "See xref:page-{}.adoc#page-{}-section-{section}[the next page] and \
             <<page-{number}-section-{}>>.\n\n",
            (number + 1) % 10_000,
            (number + 1) % 10_000,
            (section + 1) % 10
        );
    }
    text
}

#[test]
#[ignore = "memory probe"]
fn index_memory_probe() {
    let pages: usize = std::env::var("PROBE_PAGES")
        .ok()
        .and_then(|pages| pages.parse().ok())
        .unwrap_or(10_000);
    let mut index = WorkspaceIndex::new();
    let mut bytes = 0;
    let started = Instant::now();
    for number in 0..pages {
        let text = page(number);
        bytes += text.len();
        // As the workspace's index does, keeping each page's parse but not its text.
        index.index_file(PathBuf::from(format!("/probe/page-{number}.adoc")), &text);
    }
    println!(
        "{pages} pages, {:.1} MB of text, indexed in {:?}",
        bytes as f64 / 1e6,
        started.elapsed()
    );
    if pages > 0 {
        // An edit replaces one page's entry among all the others.
        let text = page(0);
        let started = Instant::now();
        for _ in 0..20 {
            index.index_file(PathBuf::from("/probe/page-0.adoc"), &text);
        }
        println!("replacing one page: {:?}", started.elapsed() / 20);
        for query in ["s", "section 3", "page 42", "zzz"] {
            let started = Instant::now();
            let found = index.symbols_matching(query).len();
            println!(
                "symbols matching {query:?}: {found} in {:?}",
                started.elapsed()
            );
        }
    }
    assert_eq!(index.files().count(), pages);
}
