use std::{
    cell::OnceCell,
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
    /// The text parsed, once something reads it after an edit (see [`OpenDocument::parsed`]).
    pub(crate) parsed: OnceCell<Document>,
    pub diagnostics: Vec<Diagnostic>,
    /// True for a document created with `open_untitled` that has never been saved.
    pub untitled: bool,
    /// False for a file that is not AsciiDoc, such as YAML or a script opened from the Files
    /// view: it is edited as text, and never indexed, diagnosed, previewed or shown visually.
    pub asciidoc: bool,
}

impl OpenDocument {
    /// Whether the document is in the workspace index: an AsciiDoc file on disk.
    pub(crate) fn indexed(&self) -> bool {
        self.asciidoc && !self.untitled
    }
}

pub(crate) fn uri_for(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

pub(crate) fn hash_text(text: &str) -> u64 {
    adoc_core::probe::hit(adoc_core::probe::Probe::HashText);
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl OpenDocument {
    /// The semantic parse of the text as it stands. An edit drops it, and the first read
    /// after parses the text again, so typing parses nothing until something asks: the index,
    /// once typing pauses, or a language feature.
    #[must_use]
    pub fn parsed(&self) -> &Document {
        self.parsed.get_or_init(|| {
            adoc_parser::parse(&uri_for(&self.path), &self.text.to_string()).document
        })
    }

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
    /// The saved text's length: text of another length is not it, without hashing.
    pub(crate) saved_len: usize,
    /// The file was reported changed on disk under unsaved edits; it is not reported again
    /// until the page is saved or reloaded.
    pub(crate) changed_on_disk: bool,
}

impl DocumentState {
    /// Records `text` as the text on disk.
    pub(crate) fn mark_saved(&mut self, text: &str) {
        self.saved_hash = hash_text(text);
        self.saved_len = text.len();
    }

    /// Whether `text` is the text on disk. Only text of the saved length is hashed.
    pub(crate) fn is_saved(&self, text: &Rope) -> bool {
        text.len() == self.saved_len && hash_text(&text.to_string()) == self.saved_hash
    }
}
