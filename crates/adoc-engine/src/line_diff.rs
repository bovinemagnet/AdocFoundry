//! Line differences between two texts, shown when recovering unsaved edits and when comparing
//! a page with a version from its local history.

use similar::{ChangeTag, TextDiff};

/// Unchanged lines shown on each side of a change.
pub const DIFF_CONTEXT: usize = 3;

/// One line of a difference, from the older text to the newer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffLine {
    /// A line both texts share, shown around a change.
    Same(String),
    /// A line only the older text has.
    Removed(String),
    /// A line only the newer text has.
    Added(String),
    /// This many unchanged lines, left out.
    Skipped(usize),
}

/// The changed lines from `old` to `new`, each change with [`DIFF_CONTEXT`] unchanged lines
/// around it and the unchanged lines between changes counted. Empty when the texts are the
/// same.
#[must_use]
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let diff = TextDiff::from_lines(old, new);
    let mut lines = Vec::new();
    let mut shown_to = 0;
    for group in diff.grouped_ops(DIFF_CONTEXT) {
        let Some(start) = group.first().map(|op| op.old_range().start) else {
            continue;
        };
        if start > shown_to {
            lines.push(DiffLine::Skipped(start - shown_to));
        }
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change
                    .value()
                    .trim_end_matches('\n')
                    .trim_end_matches('\r')
                    .to_owned();
                lines.push(match change.tag() {
                    ChangeTag::Equal => DiffLine::Same(text),
                    ChangeTag::Delete => DiffLine::Removed(text),
                    ChangeTag::Insert => DiffLine::Added(text),
                });
            }
            shown_to = op.old_range().end;
        }
    }
    let total = diff.old_slices().len();
    if !lines.is_empty() && total > shown_to {
        lines.push(DiffLine::Skipped(total - shown_to));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(count: usize) -> String {
        (1..=count).map(|line| format!("line {line}\n")).collect()
    }

    #[test]
    fn the_same_text_has_no_difference() {
        assert!(line_diff("a\nb\n", "a\nb\n").is_empty());
        assert!(line_diff("", "").is_empty());
    }

    #[test]
    fn a_changed_line_is_shown_with_its_context_and_the_rest_counted() {
        let old = numbered(20);
        let new = old.replace("line 10\n", "line ten\n");
        assert_eq!(
            line_diff(&old, &new),
            vec![
                DiffLine::Skipped(6),
                DiffLine::Same("line 7".into()),
                DiffLine::Same("line 8".into()),
                DiffLine::Same("line 9".into()),
                DiffLine::Removed("line 10".into()),
                DiffLine::Added("line ten".into()),
                DiffLine::Same("line 11".into()),
                DiffLine::Same("line 12".into()),
                DiffLine::Same("line 13".into()),
                DiffLine::Skipped(7),
            ]
        );
    }

    #[test]
    fn changes_far_apart_are_separated_by_the_lines_between() {
        let old = numbered(30);
        let new = old
            .replace("line 2\n", "line two\n")
            .replace("line 28\n", "");
        let diff = line_diff(&old, &new);
        assert_eq!(diff[0], DiffLine::Same("line 1".into()));
        assert!(diff.contains(&DiffLine::Skipped(19)), "{diff:?}");
        assert!(diff.contains(&DiffLine::Removed("line 28".into())));
        assert_eq!(diff.last(), Some(&DiffLine::Same("line 30".into())));
    }

    #[test]
    fn line_endings_are_not_part_of_a_line() {
        assert_eq!(
            line_diff("", "first\r\nsecond"),
            vec![
                DiffLine::Added("first".into()),
                DiffLine::Added("second".into()),
            ]
        );
    }
}
