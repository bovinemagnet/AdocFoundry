//! Every AsciiDoc page of the repository's fixtures and documentation, written as an article
//! and checked against the DocBook 5.2.1 RELAX NG schema and the export's own check (its
//! Schematron exclusions included): what the hand-written tests do not think of, the corpus
//! may hold.

use std::path::{Path, PathBuf};

use adoc_docbook::{testing, write_docbook, DocBookDocument, DocBookOptions, Page};

fn pages(folder: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            pages(&path, out);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "adoc")
        {
            out.push(path);
        }
    }
}

#[test]
fn every_fixture_and_documentation_page_is_valid_docbook() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    pages(&repository.join("tests/fixtures"), &mut found);
    pages(&repository.join("src/docs"), &mut found);
    found.sort();
    assert!(found.len() > 200, "{} pages", found.len());
    let mut failures = Vec::new();
    for path in &found {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let view = common_view(path, &text);
        let document = DocBookDocument::Article(Page {
            key: "page".to_owned(),
            view,
            depth: 0,
        });
        match write_docbook(&document, &DocBookOptions::default()) {
            Ok(output) => {
                let problems = testing::schema_problems(&output.xml);
                if !problems.is_empty() {
                    failures.push(format!("{}: {}", path.display(), problems.join("; ")));
                }
            }
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} pages:\n{}",
        failures.len(),
        found.len(),
        failures.join("\n")
    );
}

fn common_view(path: &Path, text: &str) -> adoc_render::ViewTree {
    use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}
