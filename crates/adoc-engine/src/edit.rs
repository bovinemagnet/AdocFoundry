use adoc_core::SourceRange;
use ropey::Rope;

use crate::EngineError;

/// A replacement of a byte range in a document, in the coordinates of the text before the edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEdit {
    pub range: SourceRange,
    pub replacement: String,
}

impl TextEdit {
    #[must_use]
    pub fn new(range: SourceRange, replacement: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
        }
    }

    #[must_use]
    pub fn insert(offset: usize, text: impl Into<String>) -> Self {
        Self::new(SourceRange::new(offset, offset), text)
    }

    #[must_use]
    pub fn delete(range: SourceRange) -> Self {
        Self::new(range, String::new())
    }
}

/// Sorts `edits` by position and checks that they are in bounds, fall on character boundaries
/// and do not overlap.
pub(crate) fn validate(
    rope: &Rope,
    mut edits: Vec<TextEdit>,
) -> Result<Vec<TextEdit>, EngineError> {
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let len = rope.len();
    let mut previous_end = 0;
    for edit in &edits {
        let SourceRange { start, end } = edit.range;
        if start > end || end > len {
            return Err(EngineError::InvalidEdit(format!(
                "range {start}..{end} is outside the document (length {len})"
            )));
        }
        if !rope.is_char_boundary(start) || !rope.is_char_boundary(end) {
            return Err(EngineError::InvalidEdit(format!(
                "range {start}..{end} does not fall on character boundaries"
            )));
        }
        if start < previous_end {
            return Err(EngineError::InvalidEdit(format!(
                "range {start}..{end} overlaps a previous edit"
            )));
        }
        previous_end = end;
    }
    Ok(edits)
}

/// Applies validated, sorted `edits` to `rope` and returns the edits that undo them, expressed in
/// the coordinates of the edited text.
pub(crate) fn apply(rope: &mut Rope, edits: &[TextEdit]) -> Vec<TextEdit> {
    let mut inverse = Vec::with_capacity(edits.len());
    let mut delta: isize = 0;
    for edit in edits {
        let start = edit.range.start.saturating_add_signed(delta);
        let removed = rope.slice(edit.range.start..edit.range.end).to_string();
        inverse.push(TextEdit::new(
            SourceRange::new(start, start + edit.replacement.len()),
            removed,
        ));
        delta += edit.replacement.len() as isize - edit.range.len() as isize;
    }
    for edit in edits.iter().rev() {
        if !edit.range.is_empty() {
            rope.remove(edit.range.start..edit.range.end);
        }
        if !edit.replacement.is_empty() {
            rope.insert(edit.range.start, &edit.replacement);
        }
    }
    inverse
}

/// The single edit turning `before` into `after`: their common prefix and suffix left alone.
pub(crate) fn minimal(before: &str, after: &str) -> Option<TextEdit> {
    if before == after {
        return None;
    }
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let longest = before.len().min(after.len()) - prefix;
    let mut suffix = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take(longest)
        .take_while(|(left, right)| left == right)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    Some(TextEdit::new(
        SourceRange::new(prefix, before.len() - suffix),
        &after[prefix..after.len() - suffix],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, end: usize, text: &str) -> TextEdit {
        TextEdit::new(SourceRange::new(start, end), text)
    }

    #[test]
    fn applies_multiple_edits_and_inverts_them() {
        let mut rope = Rope::from_str("hello world");
        let edits = validate(&rope, vec![edit(6, 11, "there"), edit(0, 5, "hi")]).unwrap();
        let inverse = apply(&mut rope, &edits);
        assert_eq!(rope.to_string(), "hi there");

        let inverse = validate(&rope, inverse).unwrap();
        apply(&mut rope, &inverse);
        assert_eq!(rope.to_string(), "hello world");
    }

    #[test]
    fn rejects_out_of_bounds_overlapping_and_split_characters() {
        let rope = Rope::from_str("héllo");
        assert!(validate(&rope, vec![edit(0, 99, "")]).is_err());
        assert!(validate(&rope, vec![edit(0, 3, ""), edit(2, 4, "")]).is_err());
        assert!(validate(&rope, vec![edit(2, 2, "x")]).is_err());
    }
}
