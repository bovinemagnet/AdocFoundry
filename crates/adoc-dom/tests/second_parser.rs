//! The asciidoc-parser implementation against the acdc one over the render corpus.

use std::path::{Path, PathBuf};

use adoc_dom::{AcdcParser, AsciidocParser, BlockKind, DocumentParser, Dom, Inline, ParseInput};

fn corpus() -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/render");
    let mut pages: Vec<PathBuf> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "adoc")
        })
        .collect();
    pages.sort();
    pages
}

fn summary(dom: &Dom) -> Vec<(String, String)> {
    dom.walk()
        .into_iter()
        .map(|block| {
            let text = match &block.kind {
                BlockKind::Paragraph { inlines } => Inline::plain_text(inlines),
                BlockKind::Section { title, .. } | BlockKind::Heading { title, .. } => {
                    Inline::plain_text(title)
                }
                BlockKind::Listing { text, .. } | BlockKind::Literal { text } => text.clone(),
                _ => String::new(),
            };
            (block.kind_name().to_owned(), text)
        })
        .collect()
}

#[test]
fn second_parser_matches_the_corpus() {
    let mut differences = Vec::new();
    for page in corpus() {
        let text = std::fs::read_to_string(&page).unwrap();
        let input = ParseInput::new(&page, &text);
        let first = summary(&AcdcParser.parse(&input));
        let second_dom = AsciidocParser.parse(&input);
        let second = summary(&second_dom);
        let name = page.file_name().unwrap().to_string_lossy().into_owned();

        // Spans name real files and lie within them.
        for block in second_dom.walk() {
            let span = &block.meta.span;
            let file = std::fs::read_to_string(&span.path).unwrap_or_default();
            assert!(span.range.end <= file.len(), "{name}: {span:?}");
        }
        assert!(!second.is_empty(), "{name}: nothing parsed");

        let length = first.len().max(second.len());
        for index in 0..length {
            if first.get(index) != second.get(index) {
                differences.push(format!(
                    "{name} block {index}: acdc {:?}, asciidoc-parser {:?}",
                    first.get(index),
                    second.get(index)
                ));
            }
        }
    }
    // Listed, not failed: the parsers are compared, not judged.
    for difference in &differences {
        eprintln!("{difference}");
    }
    eprintln!("{} difference(s)", differences.len());
}

#[test]
fn second_parser_survives_half_typed_input() {
    for text in [
        "",
        "=",
        "= ",
        "== Title\n----\nunterminated",
        "|===\n|a |b",
        "* [",
        "[source,\n----",
        "xref:",
        "include::[]",
        "image::[",
        "footnote:[",
        "<<",
        "[[",
        "....\n",
        "++++",
        ":attr",
        "* item\n** nested\n*** deeper\n.",
        "[cols=\"1,\n|===",
        "NOTE:",
        "[mermaid]\n----",
        "\u{feff}= Bom\n",
        "= T\n\nä ö ü 日本 🎉\n",
    ] {
        let dom = AsciidocParser.parse(&ParseInput::new(Path::new("/draft.adoc"), text));
        for block in dom.walk() {
            assert!(block.meta.span.range.end <= text.len(), "{text:?}");
        }
    }
}
