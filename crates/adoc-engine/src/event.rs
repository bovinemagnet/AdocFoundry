use std::path::PathBuf;

use crate::{CommandId, DocumentId, SearchId, SearchMatch, TextEdit};

/// A change in engine state that front ends react to.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    WorkspaceIndexed {
        roots: Vec<PathBuf>,
        files: usize,
    },
    DocumentOpened(DocumentId),
    DocumentChanged {
        id: DocumentId,
        version: u64,
        edits: Vec<TextEdit>,
    },
    DocumentSaved(DocumentId),
    DocumentClosed(DocumentId),
    /// The file behind an open document was moved, by a rename or its undo.
    DocumentMoved {
        id: DocumentId,
        from: PathBuf,
        to: PathBuf,
    },
    /// The file behind an open document changed on disk while it had unsaved edits.
    DocumentChangedOnDisk(DocumentId),
    DiagnosticsUpdated {
        path: PathBuf,
    },
    CatalogChanged {
        descriptor: PathBuf,
    },
    /// Back/forward availability or the active document changed.
    NavigationChanged,
    SearchResults {
        request: SearchId,
        results: Vec<SearchMatch>,
        done: bool,
    },
    CommandFailed {
        command: CommandId,
        error: String,
    },
    SettingsChanged,
    /// A new preview of `id` is available from `Engine::rendered_preview`.
    PreviewReady {
        id: DocumentId,
        version: u64,
    },
    /// The preview was shown, hidden, detached, docked, pinned or unpinned, or scroll sync
    /// was toggled.
    PreviewLayoutChanged,
}
