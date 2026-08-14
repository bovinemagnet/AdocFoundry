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
    pub text: String,
    pub title: Option<DocumentTitle>,
    pub sections: Vec<Section>,
    pub attributes: Vec<AttributeDeclaration>,
    pub anchors: Vec<Anchor>,
    pub references: Vec<Reference>,
    pub includes: Vec<IncludeDirective>,
    pub images: Vec<ImageDirective>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Document {
    #[must_use]
    pub fn new(uri: impl Into<String>, text: impl Into<String>) -> Self {
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

    #[must_use]
    pub fn line_column(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text_len);
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        (line, offset - self.line_starts[line])
    }

    #[must_use]
    pub fn offset(&self, line: usize, column: usize) -> Option<usize> {
        let start = *self.line_starts.get(line)?;
        let line_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text_len);
        Some((start + column).min(line_end))
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
        assert_eq!(index.offset(1, 2), Some(6));
        assert_eq!(index.offset(9, 0), None);
    }
}
