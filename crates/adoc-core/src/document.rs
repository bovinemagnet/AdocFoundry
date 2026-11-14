use std::sync::Arc;

use crate::{Anchor, Diagnostic, ImageDirective, IncludeDirective, Reference, Section};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

impl SourceRange {
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

/// A range in a file.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Location {
    pub path: std::path::PathBuf,
    pub range: SourceRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentTitle {
    pub text: String,
    pub range: SourceRange,
    pub selection_range: SourceRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeDeclaration {
    pub name: String,
    pub value: Option<String>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Document {
    pub uri: String,
    /// The text parsed, shared by every copy of the document: the engine's parse of an open
    /// page and the index's entry for it hold the one allocation.
    pub text: Arc<str>,
    pub title: Option<DocumentTitle>,
    pub sections: Vec<Section>,
    pub attributes: Vec<AttributeDeclaration>,
    pub anchors: Vec<Anchor>,
    pub references: Vec<Reference>,
    pub includes: Vec<IncludeDirective>,
    pub images: Vec<ImageDirective>,
    /// The keyboard, button and menu macros in prose, in document order.
    pub ui_macros: Vec<UiMacro>,
    pub diagnostics: Vec<Diagnostic>,
}

/// A keyboard, button or menu macro (`kbd:[Ctrl+S]`, `btn:[Save]`, `menu:File[Save]`), which
/// Asciidoctor reads as one only while the `experimental` attribute is set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiMacro {
    /// `kbd`, `btn` or `menu`.
    pub name: &'static str,
    /// The whole macro, from its name to its closing bracket.
    pub range: SourceRange,
}

impl Document {
    #[must_use]
    pub fn new(uri: impl Into<String>, text: impl Into<Arc<str>>) -> Self {
        Self {
            uri: uri.into(),
            text: text.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn line_index(&self) -> LineIndex {
        LineIndex::new(&self.text)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineIndex {
    line_starts: Vec<usize>,
    text_len: usize,
}

impl LineIndex {
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(offset, _)| offset + 1));
        Self {
            line_starts,
            text_len: text.len(),
        }
    }

    /// The 0-based line and column of `offset`, the column in bytes.
    #[must_use]
    pub fn line_column(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text_len);
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        (line, offset - self.line_starts[line])
    }

    /// The 0-based line and column of `offset` in `text`, the text this index was built from,
    /// the column in characters, as editors and CI annotations count it.
    #[must_use]
    pub fn line_character(&self, text: &str, offset: usize) -> (usize, usize) {
        let offset = text.floor_char_boundary(offset.min(text.len()));
        let (line, byte) = self.line_column(offset);
        let column = text
            .get(offset - byte..offset)
            .map_or(byte, |before| before.chars().count());
        (line, column)
    }

    /// The offset at which 0-based `line` starts; `None` past the last line.
    #[must_use]
    pub fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::LineIndex;

    #[test]
    fn maps_offsets_to_zero_based_lines_and_columns() {
        let index = LineIndex::new("one\ntwo\n");

        assert_eq!(index.line_column(0), (0, 0));
        assert_eq!(index.line_column(5), (1, 1));
    }

    #[test]
    fn a_line_starts_after_the_line_ending_before_it() {
        // Issue #533: the start of a line is all a caller asks for, and is always a character
        // boundary, where a byte column need not be.
        let index = LineIndex::new("héllo\r\ntwo\n");
        assert_eq!(index.line_start(0), Some(0));
        assert_eq!(index.line_start(1), Some(8));
        assert_eq!(index.line_start(2), Some(12), "after the last line ending");
        assert_eq!(index.line_start(3), None);
        assert_eq!(LineIndex::new("").line_start(0), Some(0));
    }

    #[test]
    fn a_character_column_counts_characters_not_bytes() {
        // Issue #533: `héllo wörld çà ` is 15 characters and 19 bytes.
        let text = "= Page\n\nhéllo wörld çà xref:missing.adoc[]\n";
        let index = LineIndex::new(text);
        let xref = text.find("xref").unwrap();

        assert_eq!(index.line_column(xref), (2, 19));
        assert_eq!(index.line_character(text, xref), (2, 15));
        // Inside a character, the character it is in; past the end, the end.
        let inside = text.find('é').unwrap() + 1;
        assert_eq!(index.line_character(text, inside), (2, 1));
        assert_eq!(index.line_character(text, text.len() + 9), (3, 0));
    }
}
