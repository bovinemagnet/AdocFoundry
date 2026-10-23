//! Named things in a document that a workspace-wide search can find.

use std::path::{Path, PathBuf};

use adoc_core::{Document, SourceRange};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceSymbolKind {
    Title,
    Section,
    Anchor,
    Attribute,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: WorkspaceSymbolKind,
    pub path: PathBuf,
    /// The span to select when the symbol is chosen.
    pub range: SourceRange,
    /// The title of the document that declares the symbol.
    pub container: Option<String>,
}

/// A file's symbols in a fixed order: title, sections, anchors, attributes.
pub(crate) fn document_symbols(path: &Path, document: &Document) -> Vec<WorkspaceSymbol> {
    let container = document.title.as_ref().map(|title| title.text.clone());
    let symbol = |name: &str, kind, range| WorkspaceSymbol {
        name: name.to_owned(),
        kind,
        path: path.to_path_buf(),
        range,
        container: container.clone(),
    };

    let mut symbols = Vec::new();
    if let Some(title) = &document.title {
        symbols.push(symbol(
            &title.text,
            WorkspaceSymbolKind::Title,
            title.selection_range,
        ));
    }
    symbols.extend(document.sections.iter().map(|section| {
        symbol(
            &section.title,
            WorkspaceSymbolKind::Section,
            section.selection_range,
        )
    }));
    symbols.extend(
        document
            .anchors
            .iter()
            .map(|anchor| symbol(&anchor.id, WorkspaceSymbolKind::Anchor, anchor.range)),
    );
    // `:name!:` and `:!name:` unset an attribute rather than declaring one.
    symbols.extend(
        document
            .attributes
            .iter()
            .filter(|attribute| !attribute.name.starts_with('!') && !attribute.name.ends_with('!'))
            .map(|attribute| {
                symbol(
                    &attribute.name,
                    WorkspaceSymbolKind::Attribute,
                    attribute.range,
                )
            }),
    );
    symbols
}

/// The lower-cased runs of letters and digits in `text`, so that neither case nor
/// punctuation affects matching.
pub(crate) fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use adoc_parser::parse;

    use super::{document_symbols, words, WorkspaceSymbolKind};

    #[test]
    fn splits_names_into_lower_cased_words() {
        assert_eq!(
            words("Authentication Flow").collect::<Vec<_>>(),
            ["authentication", "flow"]
        );
        assert_eq!(
            words("page-component-name").collect::<Vec<_>>(),
            ["page", "component", "name"]
        );
        assert_eq!(
            words("API_v2: Setup").collect::<Vec<_>>(),
            ["api", "v2", "setup"]
        );
    }

    #[test]
    fn collects_each_kind_of_symbol_with_its_container() {
        let text =
            "= Guide\n:product-name: Widget\n:draft!:\n\n[[auth-flow]]\n== Authentication Flow\n";
        let document = parse("file:///docs/guide.adoc", text).document;

        let symbols = document_symbols(Path::new("/docs/guide.adoc"), &document);

        assert_eq!(
            symbols
                .iter()
                .map(|symbol| (symbol.name.as_str(), symbol.kind))
                .collect::<Vec<_>>(),
            [
                ("Guide", WorkspaceSymbolKind::Title),
                ("Authentication Flow", WorkspaceSymbolKind::Section),
                ("auth-flow", WorkspaceSymbolKind::Anchor),
                ("product-name", WorkspaceSymbolKind::Attribute),
            ]
        );
        assert!(symbols
            .iter()
            .all(|symbol| symbol.container.as_deref() == Some("Guide")));
    }
}
