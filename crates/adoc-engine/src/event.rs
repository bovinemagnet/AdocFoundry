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
    /// A new visual model of `id` is available from `Engine::visual`.
    ModelChanged {
        id: DocumentId,
        version: u64,
    },
    /// The document is now shown as source, visual or both.
    EditorModeChanged(DocumentId),
    #[cfg(feature = "git")]
    /// The repository holding workspace root `root` was found, or its `HEAD` moved to another
    /// branch or commit.
    GitHeadChanged {
        root: PathBuf,
        head: crate::Head,
    },
    #[cfg(feature = "git")]
    /// The status of the files in the repository holding `root` changed, or was found.
    GitStatusChanged {
        root: PathBuf,
    },
    #[cfg(feature = "git")]
    /// New line changes of an open document against `HEAD` are in `Engine::git`.
    GitHunksChanged(DocumentId),
    #[cfg(feature = "git")]
    /// The blame of an open document is ready from `Engine::blame_for`, or was forgotten.
    GitBlameChanged(DocumentId),
    #[cfg(feature = "git")]
    /// A push finished; `message` says what was pushed where.
    GitPushed {
        message: String,
    },
    /// A PDF export finished: the file written, its page count and warnings, or the error.
    ExportFinished {
        output: PathBuf,
        result: Result<crate::ExportSummary, String>,
    },
}
