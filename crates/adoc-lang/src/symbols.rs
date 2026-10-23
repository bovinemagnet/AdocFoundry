use adoc_core::{Document, SourceRange};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentSymbolKind {
    Document,
    Section,
    /// A navigation entry linking to a page.
    Page,
    /// A navigation title or an entry without a link.
    Category,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: DocumentSymbolKind,
    pub level: u8,
    pub range: SourceRange,
    pub selection_range: SourceRange,
    pub children: Vec<DocumentSymbol>,
}

/// The document title with its sections nested beneath it by level.
#[must_use]
pub fn document_symbols(document: &Document) -> Vec<DocumentSymbol> {
    let title = document.title.iter().map(|title| DocumentSymbol {
        name: title.text.clone(),
        detail: None,
        kind: DocumentSymbolKind::Document,
        level: 0,
        range: title.range,
        selection_range: title.selection_range,
        children: Vec::new(),
    });
    let sections = document.sections.iter().map(|section| DocumentSymbol {
        name: section.title.clone(),
        detail: None,
        kind: DocumentSymbolKind::Section,
        level: section.level,
        range: section.range,
        selection_range: section.selection_range,
        children: Vec::new(),
    });
    nest(title.chain(sections))
}

/// A navigation file's titles and list items, nested by list level, each linking entry
/// carrying its `xref:` target as detail.
#[must_use]
pub fn nav_symbols(document: &Document) -> Vec<DocumentSymbol> {
    nest(
        adoc_antora::parse_nav(&document.text)
            .into_iter()
            .map(|entry| {
                let range = SourceRange::new(entry.start, entry.end);
                DocumentSymbol {
                    name: entry.text,
                    kind: if entry.target.is_some() {
                        DocumentSymbolKind::Page
                    } else {
                        DocumentSymbolKind::Category
                    },
                    detail: entry.target,
                    level: entry.level,
                    range,
                    selection_range: range,
                    children: Vec::new(),
                }
            }),
    )
}

/// Nest a flat, document-ordered list by level: each symbol becomes a child of the nearest
/// preceding symbol with a lower level.
///
/// A parent's range is widened to its last descendant's, since an outline expects each
/// symbol's range to contain its children's.
fn nest(flat: impl IntoIterator<Item = DocumentSymbol>) -> Vec<DocumentSymbol> {
    let mut roots = Vec::new();
    let mut open: Vec<DocumentSymbol> = Vec::new();
    for symbol in flat {
        while open.last().is_some_and(|last| last.level >= symbol.level) {
            close(&mut open, &mut roots);
        }
        open.push(symbol);
    }
    while !open.is_empty() {
        close(&mut open, &mut roots);
    }
    roots
}

fn close(open: &mut Vec<DocumentSymbol>, roots: &mut Vec<DocumentSymbol>) {
    let Some(mut symbol) = open.pop() else {
        return;
    };
    if let Some(last) = symbol.children.last() {
        symbol.range.end = symbol.range.end.max(last.range.end);
    }
    match open.last_mut() {
        Some(parent) => parent.children.push(symbol),
        None => roots.push(symbol),
    }
}

#[cfg(test)]
mod tests {
    use adoc_core::SourceRange;
    use adoc_parser::parse;

    use super::{document_symbols, nav_symbols, DocumentSymbol, DocumentSymbolKind};

    /// `name(child, child(grandchild))`, so a tree reads as one string.
    fn shape(symbols: &[DocumentSymbol]) -> String {
        symbols
            .iter()
            .map(|symbol| {
                if symbol.children.is_empty() {
                    symbol.name.clone()
                } else {
                    format!("{}({})", symbol.name, shape(&symbol.children))
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[test]
    fn nests_sections_under_the_title_and_each_other() {
        let text = "= Guide\n\n== Start\n\n=== Detail\n\n== Next\n";
        let symbols = document_symbols(&parse("file:///guide.adoc", text).document);

        assert_eq!(shape(&symbols), "Guide(Start(Detail), Next)");
        assert_eq!(symbols[0].kind, DocumentSymbolKind::Document);
        assert_eq!(symbols[0].children[0].kind, DocumentSymbolKind::Section);
    }

    #[test]
    fn widens_each_parent_to_cover_its_children() {
        let text = "= Guide\n\n== Start\n\n=== Detail\n\n== Next\n";
        let symbols = document_symbols(&parse("file:///guide.adoc", text).document);
        let guide = &symbols[0];
        let start = &guide.children[0];
        let detail = &start.children[0];
        let next = &guide.children[1];

        assert_eq!(start.range.end, detail.range.end);
        assert_eq!(guide.range.end, next.range.end);
        // The selection stays on the parent's own line.
        assert_eq!(guide.selection_range, SourceRange::new(2, 7));
    }

    #[test]
    fn outlines_a_navigation_file_by_list_level() {
        let text = ".Getting Started\n* xref:index.adoc[Introduction]\n** xref:install.adoc[Install]\n* Guides\n";
        let symbols = nav_symbols(&parse("file:///nav.adoc", text).document);

        assert_eq!(
            shape(&symbols),
            "Getting Started(Introduction(Install), Guides)"
        );
        let introduction = &symbols[0].children[0];
        assert_eq!(introduction.kind, DocumentSymbolKind::Page);
        assert_eq!(introduction.detail.as_deref(), Some("index.adoc"));
        assert_eq!(symbols[0].children[1].kind, DocumentSymbolKind::Category);
        assert_eq!(symbols[0].range.end, text.len() - 1);
    }
}
