//! Registers the AsciiDoc tree-sitter grammars with gpui-component's language registry.
//!
//! tree-sitter here is for highlighting, folding and injections only; every semantic question
//! goes to the engine.

use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};

/// The name the block grammar is registered under; editors select it by this name.
pub const ASCIIDOC: &str = "asciidoc";
/// The inline grammar, injected into paragraphs, titles, table cells and macro targets.
pub const ASCIIDOC_INLINE: &str = "asciidoc_inline";

pub const HIGHLIGHTS: &str = include_str!("../../languages/asciidoc/highlights.scm");
pub const INJECTIONS: &str = include_str!("../../languages/asciidoc/injections.scm");
#[cfg(test)]
const OUTLINE: &str = include_str!("../../languages/asciidoc/outline.scm");
#[cfg(test)]
const FOLDS: &str = include_str!("../../languages/asciidoc/folds.scm");
pub const INLINE_HIGHLIGHTS: &str = include_str!("../../languages/asciidoc_inline/highlights.scm");

/// Languages a `[source,<lang>]` block may inject, by the names gpui-component knows them by.
const SOURCE_LANGUAGES: &[&str] = &[
    "rust",
    "java",
    "javascript",
    "typescript",
    "python",
    "go",
    "json",
    "yaml",
    "toml",
    "bash",
    "sql",
    "html",
];

/// The highlighting for a document: AsciiDoc for a page, otherwise the grammar gpui-component
/// knows by the file's extension (`toml`, `yml`, `json`, `sh` and the rest), else plain text.
pub fn for_document(path: &std::path::Path, asciidoc: bool) -> String {
    if asciidoc {
        return ASCIIDOC.to_owned();
    }
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // An unknown name is plain text.
    gpui_component::highlighter::Language::from_str(&extension)
        .name()
        .to_owned()
}

pub fn register() {
    let registry = LanguageRegistry::singleton();
    registry.register(
        ASCIIDOC_INLINE,
        &LanguageConfig::new(
            ASCIIDOC_INLINE,
            tree_sitter_asciidoc_inline::language(),
            Vec::new(),
            INLINE_HIGHLIGHTS,
            "",
            "",
        ),
    );
    let mut injected = vec![ASCIIDOC_INLINE.into()];
    injected.extend(SOURCE_LANGUAGES.iter().map(|language| (*language).into()));
    registry.register(
        ASCIIDOC,
        &LanguageConfig::new(
            ASCIIDOC,
            tree_sitter_asciidoc::language(),
            injected,
            HIGHLIGHTS,
            INJECTIONS,
            "",
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiles(language: &tree_sitter::Language, name: &str, source: &str) {
        if let Err(error) = tree_sitter::Query::new(language, source) {
            panic!("{name} does not compile against the grammar: {error}");
        }
    }

    #[test]
    fn queries_compile_against_the_grammars() {
        let block = tree_sitter_asciidoc::language();
        compiles(&block, "highlights.scm", HIGHLIGHTS);
        compiles(&block, "injections.scm", INJECTIONS);
        compiles(&block, "outline.scm", OUTLINE);
        compiles(&block, "folds.scm", FOLDS);
        compiles(
            &tree_sitter_asciidoc_inline::language(),
            "inline highlights.scm",
            INLINE_HIGHLIGHTS,
        );
    }

    #[test]
    fn injections_name_the_registered_inline_language() {
        assert!(INJECTIONS.contains(&format!("\"{ASCIIDOC_INLINE}\"")));
    }

    // Every piece of prose carries markup, as only prose with markup is injected.
    const PAGE: &str = "\
= Page Title in *bold*

== Section One in _italics_

A paragraph's first line in `code`
and its second line, with ^super^ script.

Plain words and nothing else.

\"Quoted words with *stress*
over two lines\"
-- An attribution in _italics_

* an item for <<anchor>>
. a step to xref:other.adoc[]

NOTE: an admonition about {product}

.A block title at https://example.com
|===
| a cell with #highlight#
|===

[source,rust]
let starred = *value*;
";

    /// One of each way inline markup can begin, each of which the inline grammar highlights.
    const MARKUP: &[&str] = &[
        "*strong*",
        "**un**constrained",
        "_italic_",
        "`monospace`",
        "#highlight#",
        "^super^",
        "~sub~",
        "+passthrough+",
        "pass:[text]",
        "<<anchor>>",
        "<<anchor,label>>",
        "xref:page.adoc[label]",
        "[.role]#text#",
        "kbd:[Ctrl+C]",
        "footnote:[a note]",
        "stem:[x^2]",
        "((term))",
        "(((primary,secondary)))",
        "[[id]]",
        "{product}",
        "{counter:step}",
        "line +\nbreak",
        "\\*escaped*",
        "(C)",
        "(R)",
        "(TM)",
        "...",
        "->",
        "=>",
        "<-",
        "<=",
        "\"`double`\"",
        "'`single`'",
        "https://example.com",
        "https://example.com[label]",
        "www.example.com",
        "user@example.com",
    ];

    fn highlighted_by_the_inline_grammar(text: &str) -> bool {
        use tree_sitter::StreamingIterator;
        let inline = tree_sitter_asciidoc_inline::language();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&inline).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let query = tree_sitter::Query::new(&inline, INLINE_HIGHLIGHTS).unwrap();
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut captures = cursor.captures(&query, tree.root_node(), text.as_bytes());
        captures.next().is_some()
    }

    #[test]
    fn every_kind_of_markup_is_highlighted_by_the_inline_grammar() {
        for markup in MARKUP {
            assert!(
                highlighted_by_the_inline_grammar(&format!("A {markup} here.\n")),
                "{markup:?} is not inline markup",
            );
        }
        assert!(!highlighted_by_the_inline_grammar(
            "Plain words and nothing else.\n"
        ));
    }

    // Prose without markup has nothing to highlight, so injecting it would only spend
    // gpui-component's limit on injections per document.
    #[test]
    fn prose_with_markup_is_injected_and_plain_prose_is_not() {
        for markup in MARKUP {
            let page = format!("A paragraph with {markup} in it.\n");
            assert!(
                !inline_injections(&page).is_empty(),
                "{markup:?} is not injected",
            );
        }
        let at = PAGE.find("Plain words").unwrap();
        assert!(!inline_injections(PAGE)
            .iter()
            .any(|range| range.start <= at && at < range.end));
    }

    /// The byte ranges the block grammar's injections hand to the inline grammar.
    fn inline_injections(text: &str) -> Vec<std::ops::Range<usize>> {
        use tree_sitter::StreamingIterator;
        let block = tree_sitter_asciidoc::language();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&block).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let query = tree_sitter::Query::new(&block, INJECTIONS).unwrap();
        let content = query.capture_index_for_name("injection.content").unwrap();
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
        let mut ranges = Vec::new();
        while let Some(found) = matches.next() {
            let inline = query
                .property_settings(found.pattern_index)
                .iter()
                .any(|setting| {
                    &*setting.key == "injection.language"
                        && setting.value.as_deref() == Some(ASCIIDOC_INLINE)
                });
            if inline {
                ranges.extend(
                    found
                        .captures
                        .iter()
                        .filter(|capture| capture.index == content)
                        .map(|capture| capture.node.byte_range()),
                );
            }
        }
        ranges
    }

    // gpui-component parses every injection separately, up to a limit per document, so an
    // injection inside another is parsed twice and brings the limit closer.
    #[test]
    fn no_inline_injection_lies_inside_another() {
        let ranges = inline_injections(PAGE);
        for (i, outer) in ranges.iter().enumerate() {
            for (j, inner) in ranges.iter().enumerate() {
                assert!(
                    i == j || inner.start >= outer.end || inner.end <= outer.start,
                    "{:?} overlaps {:?}",
                    &PAGE[inner.clone()],
                    &PAGE[outer.clone()],
                );
            }
        }
    }

    #[test]
    fn every_line_of_prose_is_injected() {
        let ranges = inline_injections(PAGE);
        for prose in [
            "Page Title",
            "Section One",
            "first line",
            "second line",
            "Quoted words",
            "An attribution",
            "an item",
            "a step",
            "an admonition",
            "A block title",
            "a cell",
        ] {
            let at = PAGE.find(prose).unwrap();
            assert!(
                ranges
                    .iter()
                    .any(|range| range.start <= at && at + prose.len() <= range.end),
                "{prose:?} is not injected",
            );
        }
    }

    // A paragraph is injected whole, so a span broken over two of its lines reaches the
    // inline grammar in one piece and is highlighted as one span.
    #[test]
    fn a_span_over_two_lines_of_a_paragraph_is_highlighted() {
        use tree_sitter::StreamingIterator;
        let page = "A paragraph with *strong words\nover two lines* in it.\n";
        let span = page.find("*strong").unwrap()..page.find(" in it").unwrap();
        let injected = inline_injections(page)
            .into_iter()
            .find(|range| range.start <= span.start && span.end <= range.end)
            .expect("the span is not injected in one piece");
        let text = &page[injected.clone()];
        let inline = tree_sitter_asciidoc_inline::language();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&inline).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let query = tree_sitter::Query::new(&inline, INLINE_HIGHLIGHTS).unwrap();
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut captures = cursor.captures(&query, tree.root_node(), text.as_bytes());
        let mut strong = false;
        while let Some((found, index)) = captures.next() {
            let capture = found.captures[*index];
            let range = capture.node.byte_range();
            strong |= query.capture_names()[capture.index as usize] == "emphasis.strong"
                && injected.start + range.start == span.start
                && injected.start + range.end == span.end;
        }
        assert!(strong, "{:?} is not highlighted as one span", &page[span]);
    }

    #[test]
    fn source_paragraphs_are_not_prose() {
        let at = PAGE.find("*value*").unwrap();
        assert!(!inline_injections(PAGE)
            .iter()
            .any(|range| range.start <= at && at < range.end));
    }
}
