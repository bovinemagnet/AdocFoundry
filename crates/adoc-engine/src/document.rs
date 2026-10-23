use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
};

use adoc_core::{Diagnostic, Document};
use ropey::Rope;

/// Identifies an open document for the lifetime of the engine.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DocumentId(pub(crate) u64);

impl DocumentId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A document the engine has open. Front ends mirror it and never own their own copy.
#[derive(Clone, Debug)]
pub struct OpenDocument {
    pub id: DocumentId,
    /// May not exist on disk, for an untitled document.
    pub path: PathBuf,
    pub text: Rope,
    pub version: u64,
    pub dirty: bool,
    pub parsed: Document,
    pub diagnostics: Vec<Diagnostic>,
    /// True for a document created with `open_untitled` that has never been saved.
    pub untitled: bool,
}

pub(crate) fn uri_for(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

pub(crate) fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl OpenDocument {
    /// The document text as a string.
    #[must_use]
    pub fn text_string(&self) -> String {
        self.text.to_string()
    }
}

/// Engine-private bookkeeping for an open document.
#[derive(Clone, Debug, Default)]
pub(crate) struct DocumentState {
    pub(crate) undo: crate::UndoStack,
    pub(crate) saved_hash: u64,
}
