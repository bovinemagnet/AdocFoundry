//! The AdocFoundry engine: the only entry point front ends use.
//!
//! It owns open documents, undo history, the command registry and settings, and publishes
//! [`EngineEvent`]s. Every user-visible action is a [`Command`] with a stable [`CommandId`].

mod clock;
mod command;
pub mod commands;
mod document;
mod edit;
mod engine;
mod error;
mod event;
mod language;
mod navigation;
mod preview;
mod search;
mod settings;
mod undo;
mod workspace;

pub use clock::{Clock, ManualClock, SystemClock};
pub use command::{
    Command, CommandArgs, CommandCategory, CommandContext, CommandId, CommandOutcome,
    CommandRegistry, CommandSpec, RegistryError,
};
pub use document::{DocumentId, OpenDocument};
pub use edit::TextEdit;
pub use engine::Engine;
pub use error::EngineError;
pub use event::EngineEvent;
pub use navigation::NavigationPosition;
pub use preview::{
    EditPreview, FileEdits, FileMove, PreviewId, SearchId, SearchMatch, SearchRequest,
};
pub use settings::{
    EditorSettings, Settings, ThemePreference, ToolSettings, WorkspaceSettings, USER_SETTINGS_FILE,
    WORKSPACE_SETTINGS_FILE,
};
pub use undo::{UndoEntry, UndoGroup, UndoStack, COALESCE_WINDOW};
pub use workspace::WATCH_DEBOUNCE;

pub use adoc_antora::{
    compare_component_versions, AntoraCoordinate, AntoraResourceId, NavError, NavItem, NavTree,
    ResourceFamily,
};
pub use adoc_lang::{
    CompletionItem, CompletionItemKind, DefinitionTarget, DocumentSymbol, DocumentSymbolKind,
    Hover, Location, WorkspaceState, WorkspaceSymbol, WorkspaceSymbolKind,
};
