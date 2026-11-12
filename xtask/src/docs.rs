//! Checks on the AsciiDoc of the documentation site in `src/docs`.

use std::{fs, path::Path};

/// Every `.adoc` file under `dir`, recursively.
fn adoc_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            adoc_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "adoc") {
            out.push(path);
        }
    }
}

/// The lines of `text` outside listing, literal and passthrough blocks, with their numbers.
fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    let mut fence: Option<&str> = None;
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_end();
        let is_fence = trimmed.len() >= 4
            && ["-", ".", "+"]
                .iter()
                .any(|c| trimmed.chars().all(|ch| ch.to_string() == *c));
        match fence {
            Some(open) if is_fence && trimmed == open => fence = None,
            Some(_) => {}
            None if is_fence => fence = Some(trimmed),
            None => lines.push((index + 1, line)),
        }
    }
    lines
}

/// Asciidoctor turns a backtick followed by an apostrophe into a closing curly quote, so
/// `` `x`'s `` loses the backtick that closes the code span and the rest of the line is
/// formatted wrongly. Write the possessive another way ("the `y` of `x`").
#[test]
fn no_code_span_is_followed_by_an_apostrophe() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/docs");
    let mut files = Vec::new();
    adoc_files(&docs, &mut files);
    files.sort();
    let mut found = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for (number, line) in prose_lines(&text) {
            if line.contains("`'") {
                let name = file.strip_prefix(&docs).unwrap().display();
                found.push(format!("{name}:{number}"));
            }
        }
    }
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn prose_lines_leave_out_listing_blocks() {
    let text = "a `x`'s\n----\nb `y`'s\n----\nc\n....\nd\n....\n";
    let numbers: Vec<usize> = prose_lines(text).iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, [1, 5]);
}
