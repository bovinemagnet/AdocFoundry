//! This repository's own documentation, read by the line parser.

use std::{
    fs,
    path::{Path, PathBuf},
};

use adoc_core::DiagnosticCode;

fn adoc_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            adoc_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "adoc") {
            out.push(path);
        }
    }
}

/// Issue #593: no page of the documentation site has a code span that does not close where it
/// seems to, which would be formatted wrongly on the published site.
#[test]
fn the_documentation_has_no_broken_code_spans() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/docs");
    let mut files = Vec::new();
    adoc_files(&docs, &mut files);
    files.sort();
    assert!(!files.is_empty());
    let mut found = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let line_index = adoc_core::LineIndex::new(&text);
        for diagnostic in adoc_parser::parse("file:///page.adoc", &text)
            .document
            .diagnostics
        {
            if diagnostic.code == DiagnosticCode::BrokenCodeSpan {
                let line = line_index.line_column(diagnostic.range.start).0 + 1;
                let name = file.strip_prefix(&docs).unwrap().display();
                found.push(format!("{name}:{line}: {}", diagnostic.message));
            }
        }
    }
    assert_eq!(found, Vec::<String>::new());
}
