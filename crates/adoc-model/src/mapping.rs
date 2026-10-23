//! Source edits, their inverses, and the mapping of positions through them.

use adoc_core::SourceRange;

/// A replacement of a byte range of the page, in the coordinates of the text before it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEdit {
    pub range: SourceRange,
    pub text: String,
}

impl SourceEdit {
    #[must_use]
    pub fn new(range: SourceRange, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
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

/// `source` with `edits` applied. The edits must be sorted, in bounds, on character boundaries
/// and must not overlap.
#[must_use]
pub fn apply_edits(source: &str, edits: &[SourceEdit]) -> String {
    let mut out = String::with_capacity(source.len());
    let mut last = 0;
    for edit in edits {
        out.push_str(&source[last..edit.range.start]);
        out.push_str(&edit.text);
        last = edit.range.end;
    }
    out.push_str(&source[last..]);
    out
}

/// The edits that turn the result of applying `edits` to `before` back into `before`, in the
/// coordinates of that result.
#[must_use]
pub fn invert(before: &str, edits: &[SourceEdit]) -> Vec<SourceEdit> {
    let mut delta: isize = 0;
    edits
        .iter()
        .map(|edit| {
            let start = edit.range.start.saturating_add_signed(delta);
            delta += edit.text.len() as isize - edit.range.len() as isize;
            SourceEdit::new(
                SourceRange::new(start, start + edit.text.len()),
                &before[edit.range.start..edit.range.end],
            )
        })
        .collect()
}

/// Which side of an insertion a position at it ends up on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Assoc {
    Before,
    After,
}

/// Maps source offsets through a sequence of edit batches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mapping {
    /// Each batch: `(start, old end, new length)` per edit, sorted, in that batch's
    /// pre-edit coordinates.
    batches: Vec<Vec<(usize, usize, usize)>>,
}

impl Mapping {
    #[must_use]
    pub fn new(edits: &[SourceEdit]) -> Self {
        let mut mapping = Self::default();
        mapping.push(edits);
        mapping
    }

    /// Appends a batch of edits made after the ones already mapped.
    pub fn push(&mut self, edits: &[SourceEdit]) {
        self.batches.push(
            edits
                .iter()
                .map(|edit| (edit.range.start, edit.range.end, edit.text.len()))
                .collect(),
        );
    }

    /// Where `offset` is after every batch. A position inside replaced text goes to the
    /// replacement's start (`Before`) or end (`After`); so does a position at an insertion.
    #[must_use]
    pub fn map(&self, offset: usize, assoc: Assoc) -> usize {
        self.batches
            .iter()
            .fold(offset, |offset, batch| map_batch(batch, offset, assoc))
    }

    /// `range` after every batch; it grows to take in text inserted at either end.
    #[must_use]
    pub fn map_range(&self, range: SourceRange) -> SourceRange {
        let start = self.map(range.start, Assoc::Before);
        let end = self.map(range.end, Assoc::After).max(start);
        SourceRange::new(start, end)
    }
}

fn map_batch(batch: &[(usize, usize, usize)], offset: usize, assoc: Assoc) -> usize {
    let mut delta: isize = 0;
    for &(start, end, new) in batch {
        if offset < start {
            break;
        }
        let shifted = |position: usize| position.saturating_add_signed(delta);
        if offset > end {
            delta += new as isize - (end - start) as isize;
            continue;
        }
        // At or inside the edit.
        let inside = offset > start && offset < end;
        let at_insertion = start == end;
        return if offset == start && !at_insertion {
            shifted(start)
        } else if offset == end && !at_insertion {
            shifted(start + new)
        } else if inside || at_insertion {
            match assoc {
                Assoc::Before => shifted(start),
                Assoc::After => shifted(start + new),
            }
        } else {
            shifted(offset)
        };
    }
    offset.saturating_add_signed(delta)
}

/// The smallest edit that turns `old` into `new`, both covering `range` of the page: the common
/// prefix and suffix are left alone, so unchanged lines stay unchanged.
#[must_use]
pub(crate) fn minimal_edit(range: SourceRange, old: &str, new: &str) -> Option<SourceEdit> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let longest = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(longest)
        .take_while(|(left, right)| left == right)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some(SourceEdit::new(
        SourceRange::new(range.start + prefix, range.start + old.len() - suffix),
        &new[prefix..new.len() - suffix],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, end: usize, text: &str) -> SourceEdit {
        SourceEdit::new(SourceRange::new(start, end), text)
    }

    #[test]
    fn edits_apply_and_invert_exactly() {
        let before = "hello brave world";
        let edits = [edit(0, 5, "goodbye"), edit(6, 11, ""), edit(17, 17, "!")];
        let after = apply_edits(before, &edits);
        assert_eq!(after, "goodbye  world!");
        assert_eq!(apply_edits(&after, &invert(before, &edits)), before);
    }

    #[test]
    fn mapping_follows_edits() {
        let mapping = Mapping::new(&[edit(2, 2, "XY"), edit(5, 8, "z")]);
        assert_eq!(mapping.map(1, Assoc::After), 1);
        assert_eq!(mapping.map(2, Assoc::Before), 2);
        assert_eq!(mapping.map(2, Assoc::After), 4);
        assert_eq!(mapping.map(4, Assoc::Before), 6);
        assert_eq!(
            mapping.map(5, Assoc::After),
            7,
            "the start of a replacement"
        );
        assert_eq!(mapping.map(6, Assoc::Before), 7, "inside a replacement");
        assert_eq!(mapping.map(6, Assoc::After), 8);
        assert_eq!(mapping.map(8, Assoc::Before), 8, "the end of a replacement");
        assert_eq!(mapping.map(10, Assoc::Before), 10);
        assert_eq!(
            mapping.map_range(SourceRange::new(2, 5)),
            SourceRange::new(2, 7)
        );

        let mut twice = Mapping::new(&[edit(0, 0, "ab")]);
        twice.push(&[edit(0, 1, "")]);
        assert_eq!(twice.map(3, Assoc::After), 4);
    }

    #[test]
    fn minimal_edits_leave_common_text_alone() {
        let range = SourceRange::new(10, 21);
        assert_eq!(
            minimal_edit(range, "one\ntwo\nend", "one\ntwos\nend"),
            Some(edit(17, 17, "s"))
        );
        assert_eq!(minimal_edit(range, "same", "same"), None);
        let edit = minimal_edit(SourceRange::new(0, 5), "héllo", "hèllo").unwrap();
        assert_eq!(edit.text, "è");
    }
}
