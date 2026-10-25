use std::path::PathBuf;

use adoc_core::SourceRange;

use crate::TextEdit;

/// Identifies one workspace search request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SearchId(pub(crate) u64);

impl SearchId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SearchRequest {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Restricts the search to files beneath this directory, such as one component's root.
    pub scope: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub range: SourceRange,
    /// Zero-based line of the match start.
    pub line: usize,
    /// The full text of that line, without its line ending.
    pub line_text: String,
}

/// Identifies a pending multi-file change awaiting `workspace.apply_preview`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PreviewId(pub(crate) u64);

impl PreviewId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEdits {
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
    /// The text the edits were computed against: what a confirmation shows, and what the file
    /// must still hold when the preview is applied.
    pub base: String,
}

/// A multi-file change shown to the user before it is applied as one undo group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditPreview {
    pub id: PreviewId,
    pub title: String,
    pub moves: Vec<FileMove>,
    pub edits: Vec<FileEdits>,
}
