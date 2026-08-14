use adoc_core::{Document, SourceRange};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbol {
    pub name: String,
    pub level: u8,
    pub range: SourceRange,
    pub selection_range: SourceRange,
}

#[must_use]
pub fn document_symbols(document: &Document) -> Vec<DocumentSymbol> {
    let mut symbols =
        Vec::with_capacity(document.sections.len() + usize::from(document.title.is_some()));
    if let Some(title) = &document.title {
        symbols.push(DocumentSymbol {
            name: title.text.clone(),
            level: 0,
            range: title.range,
            selection_range: title.selection_range,
        });
    }
    symbols.extend(document.sections.iter().map(|section| DocumentSymbol {
        name: section.title.clone(),
        level: section.level,
        range: section.range,
        selection_range: section.selection_range,
    }));
    symbols
}

#[cfg(test)]
mod tests {
    use adoc_parser::parse;

    use super::document_symbols;

    #[test]
    fn returns_title_and_sections_in_source_order() {
        let document = parse("file:///guide.adoc", "= Guide\n\n== Start\n\n=== Detail\n").document;
        let symbols = document_symbols(&document);

        assert_eq!(
            symbols
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            ["Guide", "Start", "Detail"]
        );
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| symbol.level)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }
}
