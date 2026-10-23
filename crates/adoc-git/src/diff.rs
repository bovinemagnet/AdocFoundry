//! Line hunks between two texts.

use adoc_core::{LineIndex, SourceRange};
use gix::diff::blob::{Algorithm, Diff, InternedInput};

use crate::{Hunk, HunkKind};

/// The lines of `current` that differ from `head`, as Git's Myers diff finds them.
#[must_use]
pub fn line_hunks(head: &str, current: &str) -> Vec<Hunk> {
    let input = InternedInput::new(head, current);
    let diff = Diff::compute(Algorithm::Myers, &input);
    let index = LineIndex::new(current);
    let line_start = |line: usize| index.offset(line, 0).unwrap_or(current.len());
    diff.hunks()
        .map(|hunk| {
            let head_lines = hunk.before.start as usize..hunk.before.end as usize;
            let lines = hunk.after.start as usize..hunk.after.end as usize;
            let kind = if head_lines.is_empty() {
                HunkKind::Added
            } else if lines.is_empty() {
                HunkKind::Deleted
            } else {
                HunkKind::Modified
            };
            let range = SourceRange::new(line_start(lines.start), line_start(lines.end));
            Hunk {
                kind,
                lines,
                range,
                head_lines,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_texts_have_no_hunks() {
        assert!(line_hunks("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn a_new_file_is_one_added_hunk() {
        let hunks = line_hunks("", "a\nb\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Added);
        assert_eq!(hunks[0].lines, 0..2);
        assert_eq!(hunks[0].range, SourceRange::new(0, 4));
    }

    #[test]
    fn a_deletion_at_the_end_points_past_the_text() {
        let hunks = line_hunks("a\nb\n", "a\n");
        assert_eq!(hunks[0].kind, HunkKind::Deleted);
        assert_eq!(hunks[0].lines, 1..1);
        assert_eq!(hunks[0].range, SourceRange::new(2, 2));
        assert_eq!(hunks[0].head_lines, 1..2);
    }
}
