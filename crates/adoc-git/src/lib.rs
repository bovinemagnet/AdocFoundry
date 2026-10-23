//! Git for AdocFoundry: the repository holding a workspace root, its branch, the status of its
//! files and the line changes of one file against `HEAD`.
//!
//! Everything goes through `gix` in-process; nothing runs the `git` executable. The engine owns
//! this crate and calls it from worker threads; no UI or engine type appears here.

mod diff;
mod error;
mod repository;
#[cfg(feature = "test-support")]
pub mod testing;

use std::{ops::Range, path::PathBuf};

use adoc_core::SourceRange;

pub use diff::line_hunks;
pub use error::GitError;
pub use repository::Repository;

/// What `HEAD` points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Head {
    /// A branch, by its short name (`main`, `release/1.0`).
    Branch(String),
    /// A commit, by its full hexadecimal id.
    Detached(String),
    /// A branch with no commits yet.
    Unborn,
}

impl std::fmt::Display for Head {
    /// As a status bar shows it: `main`, `detached @ 1a2b3c4` or `no commits yet`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Branch(name) => formatter.write_str(name),
            Self::Detached(id) => write!(formatter, "detached @ {}", &id[..id.len().min(7)]),
            Self::Unborn => formatter.write_str("no commits yet"),
        }
    }
}

/// The state of one file in the index (against `HEAD`) and in the working tree (against the
/// index), as `git status` reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStatus {
    /// The path relative to the working tree, with the platform's separators.
    pub path: PathBuf,
    pub index: Change,
    pub worktree: Change,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Change {
    Unmodified,
    Added,
    Modified,
    Deleted,
    /// Moved here from the given path.
    Renamed(PathBuf),
    Untracked,
    Ignored,
    Conflicted,
}

/// One run of changed lines in the current text of a file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hunk {
    pub kind: HunkKind,
    /// The 0-based lines of the current text; empty for a deletion, at the line after it.
    pub lines: Range<usize>,
    /// The same lines as bytes of the current text.
    pub range: SourceRange,
    /// The lines of the text in `HEAD` that this hunk replaces.
    pub head_lines: Range<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}
