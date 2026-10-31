//! The AdocFoundry engine: the only entry point front ends use.
//!
//! It owns open documents, undo history, the command registry and settings, and publishes
//! [`EngineEvent`]s. Every user-visible action is a [`Command`] with a stable [`CommandId`].

mod about;
mod auto_save;
mod clock;
mod command;
pub mod commands;
#[cfg(feature = "git")]
mod commit;
mod doctor;
mod document;
mod edit;
mod engine;
mod error;
mod event;
mod expansion;
mod export;
mod files;
mod format_table;
#[cfg(feature = "git")]
mod git;
mod html_export;
mod images;
// Without Git, the engine behaves as with `[git] enabled = false`.
#[cfg(not(feature = "git"))]
#[path = "git_off.rs"]
mod git;
mod inspect;
mod language;
mod line_diff;
mod local_history;
mod navigation;
mod new_page;
#[cfg(feature = "typst")]
mod pdf_preview;
mod preview;
mod problems;
mod profiles;
mod quick_open;
mod recovery;
mod rendered;
pub mod report;
mod search;
pub use search::SEARCH_MATCH_LIMIT;
mod session;
mod settings;
mod spelling;
#[cfg(feature = "typst")]
mod stem;
mod tabs;
mod undo;
pub mod updates;
#[cfg(feature = "git")]
mod versions;
mod visual;
mod words;
mod workspace;
mod writer;

pub use about::{About, Notice, Notices, DOCUMENTATION};
#[cfg(feature = "typst")]
pub use adoc_pdf::{PageAnchor, PreviewPage};
pub use auto_save::AUTO_SAVE_DELAY;
pub use clock::{Clock, ManualClock, SystemClock};
pub use command::{
    Command, CommandArgs, CommandCategory, CommandContext, CommandId, CommandOutcome,
    CommandRegistry, CommandSpec, RegistryError,
};
pub use doctor::{Doctor, DoctorLine, DoctorReport, DoctorSection, PROBE_TIMEOUT};
pub use document::{DocumentId, OpenDocument};
pub use edit::TextEdit;
pub use engine::{Engine, INDEX_SETTLE};
pub use error::EngineError;
pub use event::EngineEvent;
pub use expansion::{Branch, SidebarPanel, EXPANSION_SAVE_SETTLE, EXPANSION_STATE_FILE};
pub use export::{ExportArgs, ExportDiagnostic, ExportSummary, ExportTarget};
pub use files::{DirectoryEntry, DirectoryError, EntryKind, Listing};
#[cfg(feature = "git")]
pub use git::{GitRepository, GitState, GIT_SETTLE};
pub use images::{is_image_path, ImageId, OpenImage, IMAGE_EXTENSIONS};
pub use inspect::resource_id_string;
pub use language::{SymbolMatch, SYMBOL_RESULTS};
pub use line_diff::{line_diff, DiffLine, DIFF_CONTEXT};
pub use local_history::{SavedVersion, HISTORY_DIR, HISTORY_MAX_PAGE, HISTORY_SWEEP_INTERVAL};
pub use navigation::NavigationPosition;
pub use new_page::{NewPage, PageModule};
#[cfg(feature = "typst")]
pub use pdf_preview::PdfPreview;
pub use preview::{
    EditPreview, FileEdits, FileMove, PreviewId, SearchId, SearchMatch, SearchRequest,
};
pub use problems::Problem;
pub use profiles::{PdfProfile, PdfThemeChoice, ProfileContent, BUNDLED_THEMES};
pub use quick_open::{QuickOpenItem, QUICK_OPEN_LIMIT};
pub use recovery::{Recoverable, RECOVERY_DIR, RECOVERY_SETTLE};
pub use rendered::{PreviewLayout, RenderedPreview, DIAGRAM_WORKERS, PREVIEW_SETTLE};
pub use session::{RECENT_FOLDERS, RECENT_FOLDERS_FILE, SESSION_FILE};
pub use settings::{
    pdf_standard, AutoSave, EditorSettings, ExpansionPolicy, GitSettings, HistorySettings,
    PdfSettings, Settings, SidebarSettings, SpellingSettings, ThemePreference, ToolSettings,
    UpdateSettings, WorkspaceSettings, USER_SETTINGS_FILE, WORKSPACE_SETTINGS_FILE,
};
pub use spelling::{
    Misspelling, MAX_SUGGESTED_WORDS, SPELLING_CODE, SPELLING_SETTLE, USER_WORDS_FILE,
    WORKSPACE_WORDS_FILE,
};
#[cfg(feature = "typst")]
pub use stem::StemProvider;
pub use tabs::{OpenDisposition, TabItem};
pub use words::{count_words, WordCount, WORDS_PER_MINUTE, WORD_COUNT_SETTLE};

pub use adoc_core::FileAccess;
pub use adoc_index::{is_asciidoc_path, normalize_path};
#[cfg(feature = "typst")]
pub use adoc_pdf::PdfStandard;
pub use adoc_render::{to_html, to_text, HtmlOptions, ViewTree};
/// A PDF standard the output must conform to.
#[cfg(not(feature = "typst"))]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PdfStandard {
    /// PDF/A-1b: archival, PDF 1.4.
    A1b,
    /// PDF/A-1a: archival, PDF 1.4, tagged.
    A1a,
    /// PDF/A-2a: archival, tagged.
    A2a,
    /// PDF/UA-1: accessible.
    Ua1,
}
pub use undo::{UndoEntry, UndoGroup, UndoStack, COALESCE_WINDOW};
pub use visual::{EditorMode, VisualArgs, VisualDocument, VisualValue, MODEL_SETTLE};
pub use workspace::WATCH_DEBOUNCE;

pub use adoc_antora::{
    compare_component_versions, AntoraCoordinate, AntoraResource, AntoraResourceId, NavError,
    NavItem, NavTree, ResourceFamily,
};
#[cfg(feature = "git")]
pub use adoc_git::{BlameLine, Change as GitChange, FileStatus, Head, Hunk, HunkKind};
pub use adoc_lang::{
    CompletionItem, CompletionItemKind, DefinitionTarget, DocumentSymbol, DocumentSymbolKind,
    Hover, Location, WorkspaceState, WorkspaceSymbol, WorkspaceSymbolKind, XrefLink,
};
pub use adoc_model::{next_word_end, previous_word_start, word_at};
pub use adoc_model::{
    Access, Assoc, Atom, BlockPath, BlockType, Mapping, Mark, MarkSet, Model, NewBlock, Node,
    NodeKind, Piece, ReadOnly, RowSection, Segment, SourceEdit, TextBlock, TextContext,
};
