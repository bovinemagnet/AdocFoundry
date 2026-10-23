//! Who last changed each line: blame of the file in `HEAD`, carried over to the current text.

use crate::{Hunk, HunkKind};

/// The commit that last changed one line of the current text, or that it is not committed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlameLine {
    /// The 0-based line in the current text.
    pub line: usize,
    /// The commit's short id; `None` for a line not committed yet.
    pub commit: Option<String>,
    pub author: String,
    /// When the commit was made, in seconds since the Unix epoch.
    pub time: i64,
    /// The first line of the commit message, or `Not committed`.
    pub summary: String,
}

impl BlameLine {
    pub(crate) fn uncommitted(line: usize) -> Self {
        Self {
            line,
            commit: None,
            author: String::new(),
            time: 0,
            summary: "Not committed".to_owned(),
        }
    }
}

/// The blame of each of `lines` lines of the current text, from the blame of the file in
/// `HEAD` and the hunks between `HEAD` and the current text: lines inside an added or modified
/// hunk are not committed, and every other line takes the blame of its line in `HEAD`.
#[must_use]
pub fn blame_through(head: &[BlameLine], hunks: &[Hunk], lines: usize) -> Vec<BlameLine> {
    (0..lines)
        .map(|line| {
            let changed = hunks
                .iter()
                .any(|hunk| hunk.kind != HunkKind::Deleted && hunk.lines.contains(&line));
            if changed {
                return BlameLine::uncommitted(line);
            }
            // Every hunk wholly before this line moves it by the lines it added or removed.
            let shift: isize = hunks
                .iter()
                .filter(|hunk| hunk.lines.end <= line)
                .map(|hunk| hunk.head_lines.len() as isize - hunk.lines.len() as isize)
                .sum();
            usize::try_from(line as isize + shift)
                .ok()
                .and_then(|head_line| head.get(head_line))
                .map_or_else(
                    || BlameLine::uncommitted(line),
                    |blamed| BlameLine {
                        line,
                        ..blamed.clone()
                    },
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_hunks;

    #[test]
    fn lines_keep_their_blame_around_insertions_changes_and_deletions() {
        let head_text = "a\nb\nc\nd\ne\n";
        let head: Vec<BlameLine> = (0..5)
            .map(|line| BlameLine {
                line,
                commit: Some(format!("c{line}")),
                author: String::new(),
                time: 0,
                summary: String::new(),
            })
            .collect();
        // `b` deleted, `x` inserted after `c`, `e` changed.
        let current = "a\nc\nx\nd\nE\n";
        let hunks = line_hunks(head_text, current);
        let blamed: Vec<Option<String>> = blame_through(&head, &hunks, 5)
            .into_iter()
            .map(|line| line.commit)
            .collect();
        assert_eq!(
            blamed,
            [
                Some("c0".to_owned()),
                Some("c2".to_owned()),
                None,
                Some("c3".to_owned()),
                None
            ]
        );
    }
}
