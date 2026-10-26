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
}
