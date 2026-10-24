//! The AdocFoundry engine: the only entry point front ends use.
//!
//! It owns open documents, undo history, the command registry and settings, and publishes
//! [`EngineEvent`]s. Every user-visible action is a [`Command`] with a stable [`CommandId`].

mod clock;
mod command;
pub mod commands;
#[cfg(feature = "git")]
mod commit;
mod document;
mod edit;
mod engine;
mod error;
mod event;
mod export;
#[cfg(feature = "git")]
mod git;
// Without Git, the engine behaves as with `[git] enabled = false`.
#[cfg(not(feature = "git"))]
#[path = "git_off.rs"]
mod git;
mod language;
mod navigation;
mod preview;
mod rendered;
mod search;
mod settings;
#[cfg(feature = "typst")]
mod stem;
mod undo;
#[cfg(feature = "git")]
mod versions;
mod visual;
mod workspace;

pub use clock::{Clock, ManualClock, SystemClock};
pub use command::{
    Command, CommandArgs, CommandCategory, CommandContext, CommandId, CommandOutcome,
    CommandRegistry, CommandSpec, RegistryError,
};
pub use document::{DocumentId, OpenDocument};
pub use edit::TextEdit;
pub use engine::{Engine, INDEX_SETTLE};
pub use error::EngineError;
pub use event::EngineEvent;
pub use export::{ExportArgs, ExportDiagnostic, ExportSummary, ExportTarget};
#[cfg(feature = "git")]
pub use git::{GitRepository, GitState, GIT_SETTLE};
pub use navigation::NavigationPosition;
pub use preview::{
    EditPreview, FileEdits, FileMove, PreviewId, SearchId, SearchMatch, SearchRequest,
};
pub use rendered::{PreviewLayout, RenderedPreview, PREVIEW_SETTLE};
pub use settings::{
    pdf_standard, EditorSettings, GitSettings, PdfSettings, Settings, ThemePreference,
    ToolSettings, WorkspaceSettings, USER_SETTINGS_FILE, WORKSPACE_SETTINGS_FILE,
};
#[cfg(feature = "typst")]
pub use stem::StemProvider;

#[cfg(feature = "typst")]
pub use adoc_pdf::PdfStandard;
/// A PDF standard the output must conform to.
#[cfg(not(feature = "typst"))]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PdfStandard {
    /// PDF/A-2a: archival, tagged.
    A2a,
    /// PDF/UA-1: accessible.
    Ua1,
}
pub use undo::{UndoEntry, UndoGroup, UndoStack, COALESCE_WINDOW};
pub use visual::{EditorMode, VisualArgs, VisualDocument, VisualValue, MODEL_SETTLE};
pub use workspace::WATCH_DEBOUNCE;

pub use adoc_antora::{
    compare_component_versions, AntoraCoordinate, AntoraResourceId, NavError, NavItem, NavTree,
    ResourceFamily,
};
#[cfg(feature = "git")]
pub use adoc_git::{BlameLine, Change as GitChange, FileStatus, Head, Hunk, HunkKind};
pub use adoc_lang::{
    CompletionItem, CompletionItemKind, DefinitionTarget, DocumentSymbol, DocumentSymbolKind,
    Hover, Location, WorkspaceState, WorkspaceSymbol, WorkspaceSymbolKind,
};
pub use adoc_model::{next_word_end, previous_word_start, word_at};
pub use adoc_model::{
    Access, Assoc, Atom, BlockPath, BlockType, Mapping, Mark, MarkSet, Model, NewBlock, Node,
    NodeKind, Piece, ReadOnly, RowSection, Segment, SourceEdit, TextBlock, TextContext,
};
