//! Byte edits: the one shape every change to a shared text takes on its way in and out of a
//! replica. `TextEdit` is the engine's (L3) and out of this crate's reach, so the engine converts
//! (design §3).

use adoc_core::SourceRange;

use crate::CollabError;

/// A replacement of the bytes `range` by `text`.
///
/// A batch of edits is ordered and non-overlapping, and every range is in the coordinates of the
/// text **before** the batch, as the engine's `TextEdit`s are: applying a batch from its last
/// edit to its first gives the text after it ([`apply_edits`]).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ByteEdit {
    pub range: SourceRange,
    pub text: String,
}

impl ByteEdit {
    #[must_use]
    pub fn new(start: usize, end: usize, text: &str) -> Self {
        Self {
            range: SourceRange::new(start, end),
            text: text.to_owned(),
        }
    }

    /// An edit that changes nothing: an empty range replaced by nothing.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.range.start == self.range.end && self.text.is_empty()
    }
}

/// Applies a batch of `edits` to `text`. The batch must be valid for it ([`check_boundaries`]).
pub fn apply_edits(text: &mut String, edits: &[ByteEdit]) {
    for edit in edits.iter().rev() {
        text.replace_range(edit.range.start..edit.range.end, &edit.text);
    }
}

/// Checks that a batch is ordered and non-overlapping, with every range forwards and its ends
/// inside a text of `len` bytes.
pub fn check_order(edits: &[ByteEdit], len: usize) -> Result<(), CollabError> {
    let mut end = 0;
    for edit in edits {
        if edit.range.start > edit.range.end || edit.range.start < end {
            return Err(CollabError::Unordered);
        }
        if edit.range.end > len {
            return Err(CollabError::InvalidBoundary(edit.range.end));
        }
        end = edit.range.end;
    }
    Ok(())
}

/// Checks that a batch is ordered and non-overlapping and that every offset falls on a
/// character boundary of `text`. `yrs` does not refuse an offset inside a character: it moves
/// the edit to the next boundary without a word (`docs/qa/spike-collab.md`, finding 2).
pub fn check_boundaries(text: &str, edits: &[ByteEdit]) -> Result<(), CollabError> {
    check_order(edits, text.len())?;
    for edit in edits {
        for offset in [edit.range.start, edit.range.end] {
            if !text.is_char_boundary(offset) {
                return Err(CollabError::InvalidBoundary(offset));
            }
        }
    }
    Ok(())
}

/// A batch in its one canonical form, as a replica reports it: no-op edits dropped and edits
/// that touch merged.
#[must_use]
pub fn normalise(edits: &[ByteEdit]) -> Vec<ByteEdit> {
    let mut out: Vec<ByteEdit> = Vec::with_capacity(edits.len());
    for edit in edits.iter().filter(|edit| !edit.is_noop()) {
        match out.last_mut() {
            Some(last) if last.range.end == edit.range.start => {
                last.range.end = edit.range.end;
                last.text.push_str(&edit.text);
            }
            _ => out.push(edit.clone()),
        }
    }
    out
}

/// The length in bytes of a text of `len` bytes after `edits`.
#[must_use]
pub fn length_after(len: usize, edits: &[ByteEdit]) -> usize {
    edits.iter().fold(len, |len, edit| {
        len - (edit.range.end - edit.range.start) + edit.text.len()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_apply_in_pre_edit_coordinates() {
        let mut text = "one two three".to_owned();
        let edits = [ByteEdit::new(0, 3, "1"), ByteEdit::new(8, 13, "3")];
        check_boundaries(&text, &edits).unwrap();
        assert_eq!(length_after(text.len(), &edits), 7);
        apply_edits(&mut text, &edits);
        assert_eq!(text, "1 two 3");
    }

    #[test]
    fn disorder_overlap_and_split_characters_are_refused() {
        let text = "é👋";
        assert_eq!(
            check_boundaries(text, &[ByteEdit::new(1, 1, "x")]),
            Err(CollabError::InvalidBoundary(1))
        );
        assert_eq!(
            check_boundaries(text, &[ByteEdit::new(2, 4, "")]),
            Err(CollabError::InvalidBoundary(4))
        );
        assert_eq!(
            check_boundaries(text, &[ByteEdit::new(0, 7, "")]),
            Err(CollabError::InvalidBoundary(7))
        );
        assert_eq!(
            check_order(&[ByteEdit::new(2, 2, "a"), ByteEdit::new(0, 0, "b")], 6),
            Err(CollabError::Unordered)
        );
        assert_eq!(
            check_order(&[ByteEdit::new(0, 3, ""), ByteEdit::new(2, 4, "")], 6),
            Err(CollabError::Unordered)
        );
        assert_eq!(
            check_order(&[ByteEdit::new(3, 2, "")], 6),
            Err(CollabError::Unordered)
        );
    }

    #[test]
    fn normalising_drops_noops_and_merges_neighbours() {
        let edits = [
            ByteEdit::new(0, 0, ""),
            ByteEdit::new(1, 2, "a"),
            ByteEdit::new(2, 4, "b"),
            ByteEdit::new(6, 6, "c"),
        ];
        assert_eq!(
            normalise(&edits),
            vec![ByteEdit::new(1, 4, "ab"), ByteEdit::new(6, 6, "c")]
        );
    }
}
