use std::time::{Duration, Instant};

use crate::TextEdit;

/// Consecutive single-character insertions closer together than this share one undo group.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(500);

/// Identifies one undoable user action. A group may span several documents.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UndoGroup(pub(crate) u64);

#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub group: UndoGroup,
    /// Edits that restore the text before this entry, in post-entry coordinates.
    pub inverse: Vec<TextEdit>,
    /// Edits that re-apply this entry, in pre-entry coordinates.
    pub forward: Vec<TextEdit>,
    pub version_before: u64,
    pub version_after: u64,
    pub(crate) typing: Option<Typing>,
}

/// The state of a run of typed characters that later keystrokes may extend.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Typing {
    pub(crate) last: Instant,
    /// Byte offset just after the typed run.
    pub(crate) end: usize,
}

/// Per-document undo history.
#[derive(Clone, Debug, Default)]
pub struct UndoStack {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
}

impl UndoStack {
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The number of undoable entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.undo.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    #[must_use]
    pub fn top_group(&self) -> Option<UndoGroup> {
        self.undo.last().map(|entry| entry.group)
    }

    #[must_use]
    pub fn redo_group(&self) -> Option<UndoGroup> {
        self.redo.last().map(|entry| entry.group)
    }

    pub(crate) fn push(&mut self, entry: UndoEntry) {
        self.redo.clear();
        self.undo.push(entry);
    }

    /// Extends the most recent entry with a typed character at `offset`, if the entry is a typing
    /// run that ends at `offset` and was last extended within the coalescing window.
    pub(crate) fn coalesce_typing(
        &mut self,
        offset: usize,
        text: &str,
        now: Instant,
        version_after: u64,
    ) -> bool {
        let Some(entry) = self.undo.last_mut() else {
            return false;
        };
        let Some(typing) = entry.typing.as_mut() else {
            return false;
        };
        if typing.end != offset || now.saturating_duration_since(typing.last) > COALESCE_WINDOW {
            return false;
        }
        let (Some(forward), Some(inverse)) = (entry.forward.first_mut(), entry.inverse.first_mut())
        else {
            return false;
        };
        forward.replacement.push_str(text);
        inverse.range.end += text.len();
        typing.end += text.len();
        typing.last = now;
        entry.version_after = version_after;
        self.redo.clear();
        true
    }

    pub(crate) fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo.pop()
    }

    pub(crate) fn push_redo(&mut self, mut entry: UndoEntry) {
        entry.typing = None;
        self.redo.push(entry);
    }

    pub(crate) fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo.pop()
    }

    pub(crate) fn push_undo_keep_redo(&mut self, entry: UndoEntry) {
        self.undo.push(entry);
    }
}
