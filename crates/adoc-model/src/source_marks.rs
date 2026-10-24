//! Marks toggled in source text, for the source editor's Bold, Italic, Code and Highlight:
//! each line of the selection wrapped in the mark's delimiters, or unwrapped where every line
//! already has them. Unlike the visual editor's steps this works on any text, read-only to the
//! model or not, since what is edited is plainly the source.

use adoc_core::SourceRange;

use crate::{scan::is_word, Assoc, Mapping, Mark, SourceEdit};

/// The edits a toggle makes and the selection after them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceToggle {
    pub edits: Vec<SourceEdit>,
    pub selection: SourceRange,
}

/// Where a line's mark is: its delimiters' ranges.
struct Marked {
    open: SourceRange,
    close: SourceRange,
}

/// Toggles `mark` over `selection` of `text`.
///
/// Each line of the selection, trimmed of spaces, is a segment. When every segment already has
/// the mark, its delimiters (just inside or just outside the segment) are removed; otherwise
/// every segment without it is wrapped, in the constrained form (`*word*`) where the segment
/// stands between non-word characters and the unconstrained form (`un**break**able`)
/// otherwise. An empty selection gets an empty mark with the caret inside it.
#[must_use]
pub fn toggle_mark_in_source(text: &str, selection: SourceRange, mark: Mark) -> SourceToggle {
    let selection = SourceRange::new(
        selection.start.min(text.len()),
        selection.end.min(text.len()),
    );
    let delimiter = mark.delimiter();
    let segments = segments(text, selection);
    if segments.is_empty() {
        let caret = selection.start;
        let k = if fits_constrained(text, caret, caret) {
            1
        } else {
            2
        };
        let pair = delimiter.to_string().repeat(2 * k);
        let edits = vec![SourceEdit::insert(caret, pair)];
        let inside = caret + k * delimiter.len_utf8();
        return SourceToggle {
            edits,
            selection: SourceRange::new(inside, inside),
        };
    }
    let marked: Vec<Option<Marked>> = segments
        .iter()
        .map(|segment| marked(text, *segment, delimiter))
        .collect();
    let mut edits = Vec::new();
    if marked.iter().all(Option::is_some) {
        for found in marked.iter().flatten() {
            edits.push(SourceEdit::delete(found.open));
            edits.push(SourceEdit::delete(found.close));
        }
    } else {
        for (segment, found) in segments.iter().zip(&marked) {
            if found.is_some() {
                continue;
            }
            let k = if fits_constrained(text, segment.start, segment.end) {
                1
            } else {
                2
            };
            let delimiters = delimiter.to_string().repeat(k);
            edits.push(SourceEdit::insert(segment.start, delimiters.clone()));
            edits.push(SourceEdit::insert(segment.end, delimiters));
        }
    }
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let mapping = Mapping::new(&edits);
    let (first, last) = (segments[0], segments[segments.len() - 1]);
    // One line keeps its text selected, as in the visual editor; several keep their marks in
    // the selection, so toggling again finds them.
    let selection = if segments.len() == 1 {
        SourceRange::new(
            mapping.map(first.start, Assoc::After),
            mapping.map(last.end, Assoc::Before),
        )
    } else {
        let start = marked[0]
            .as_ref()
            .map_or(first.start, |found| found.open.start.min(first.start));
        let end = marked[marked.len() - 1]
            .as_ref()
            .map_or(last.end, |found| found.close.end.max(last.end));
        SourceRange::new(
            mapping.map(start, Assoc::Before),
            mapping.map(end, Assoc::After),
        )
    };
    SourceToggle { edits, selection }
}

/// The selection's lines, each trimmed of spaces; empty lines are left out.
fn segments(text: &str, selection: SourceRange) -> Vec<SourceRange> {
    let mut segments = Vec::new();
    let mut start = selection.start;
    for line in text[selection.start..selection.end].split_inclusive('\n') {
        let end = start + line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let leading = content.len() - content.trim_start().len();
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            let segment_start = start + leading;
            segments.push(SourceRange::new(
                segment_start,
                segment_start + trimmed.len(),
            ));
        }
        start = end;
    }
    segments
}

/// The mark around `segment`, when it has one: the same count of `delimiter` at each end, just
/// inside or just outside it, with text between.
fn marked(text: &str, segment: SourceRange, delimiter: char) -> Option<Marked> {
    let width = delimiter.len_utf8();
    for k in [2, 1] {
        let run = delimiter.to_string().repeat(k);
        let length = k * width;
        let open = if text[segment.start..segment.end].starts_with(&run) {
            SourceRange::new(segment.start, segment.start + length)
        } else if text[..segment.start].ends_with(&run) {
            SourceRange::new(segment.start - length, segment.start)
        } else {
            continue;
        };
        let close = if text[open.end.max(segment.start)..segment.end].ends_with(&run)
            && segment.end - length >= open.end
        {
            SourceRange::new(segment.end - length, segment.end)
        } else if text[segment.end..].starts_with(&run) {
            SourceRange::new(segment.end, segment.end + length)
        } else {
            continue;
        };
        if close.start > open.end {
            return Some(Marked { open, close });
        }
    }
    None
}

/// Whether a mark around `text[start..end]` may take the constrained form: no word character
/// just outside it.
fn fits_constrained(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}
