//! The last session and the folders opened recently, kept in the app's state folder
//! ([`crate::Settings::state_dir`]), never in a repository. On quitting the front end writes the
//! session ([`Engine::write_session`]): the workspace's roots, the open tabs in order with each
//! page's editor mode and caret, the tab in the preview tab, the active tab, the preview
//! layout and where the graph window was ([`Engine::graph_window`]). Launched with nothing to open, it brings that back ([`Engine::restore_session`]),
//! skipping files and folders that have gone. Every folder opened as a workspace is recorded in
//! [`RECENT_FOLDERS_FILE`] for `workspace.open_recent`.
//!
//! Only [`Engine::enable_sessions`] (the app) reads or writes either file: the CLI, the
//! language server and the MCP server open what they are told to.

use std::{collections::HashMap, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    commands::{ids, register, spec},
    engine::write_replacing,
    CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId, EditorMode,
    Engine, EngineError, OpenDisposition, TabItem,
};

/// The file the last session is kept in, in the state folder.
pub const SESSION_FILE: &str = "session.json";

/// The file the folders opened recently are kept in, in the state folder.
pub const RECENT_FOLDERS_FILE: &str = "recent-folders.json";

/// How many folders opened recently are remembered.
pub const RECENT_FOLDERS: usize = 10;

const FORMAT_VERSION: u64 = 1;

#[derive(Debug, Default)]
pub(crate) struct SessionState {
    enabled: bool,
    /// The folders opened recently, most recent first.
    recent: Vec<PathBuf>,
    /// Where the graph window was last.
    graph_window: Option<WindowPlacement>,
}

/// A window's size and place on screen, in logical pixels.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximised: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedSession {
    version: u64,
    roots: Vec<PathBuf>,
    tabs: Vec<SavedTab>,
    /// The active tab, by its place in `tabs`.
    active: Option<usize>,
    preview: SavedPreview,
    /// Absent from sessions written before the graph window, which still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    graph_window: Option<WindowPlacement>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedTab {
    path: PathBuf,
    /// A page's editor mode; `source` for an image.
    mode: SavedMode,
    /// A page's caret, as a byte offset.
    caret: usize,
    /// In the preview tab, to be replaced by the next file browsed to.
    preview: bool,
    /// A pinned tab; absent from sessions written before tabs could be pinned, which still load.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum SavedMode {
    Source,
    Visual,
    Split,
}

impl From<EditorMode> for SavedMode {
    fn from(mode: EditorMode) -> Self {
        match mode {
            EditorMode::Source => Self::Source,
            EditorMode::Visual => Self::Visual,
            EditorMode::Split => Self::Split,
        }
    }
}

impl From<SavedMode> for EditorMode {
    fn from(mode: SavedMode) -> Self {
        match mode {
            SavedMode::Source => Self::Source,
            SavedMode::Visual => Self::Visual,
            SavedMode::Split => Self::Split,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedPreview {
    visible: bool,
    detached: bool,
    scroll_sync: bool,
    pdf: bool,
    /// Where the preview was before the PDF was shown: `[visible, detached]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_pdf: Option<[bool; 2]>,
    /// The pinned page, by its place in `tabs`.
    pinned: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedFolders {
    version: u64,
    folders: Vec<PathBuf>,
}

pub(crate) fn register_session(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::WORKSPACE_OPEN_RECENT,
            "Open Recent Folder…",
            CommandCategory::Workspace,
            None,
            CommandContext::Always,
        ),
        |engine, _| Ok(CommandOutcome::RecentFolders(engine.recent_folders())),
    );
}

impl Engine {
    /// Keeps the last session and the folders opened recently in the state folder, for a
    /// front end that brings them back at its next launch.
    pub fn enable_sessions(&mut self) {
        self.session.enabled = true;
        self.session.recent = self
            .state_file(RECENT_FOLDERS_FILE)
            .and_then(|file| fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str::<SavedFolders>(&text).ok())
            .filter(|saved| saved.version == FORMAT_VERSION)
            .map(|saved| saved.folders)
            .unwrap_or_default();
    }

    /// The folders opened recently that are still there, most recent first.
    #[must_use]
    pub fn recent_folders(&self) -> Vec<PathBuf> {
        self.session
            .recent
            .iter()
            .filter(|folder| folder.is_dir())
            .cloned()
            .collect()
    }

    /// Records the workspace's roots as the folders opened most recently, the first root first.
    pub(crate) fn record_recent_folders(&mut self) {
        if !self.session.enabled {
            return;
        }
        for root in self.roots.iter().rev() {
            self.session.recent.retain(|folder| folder != root);
            self.session.recent.insert(0, root.clone());
        }
        self.session.recent.truncate(RECENT_FOLDERS);
        let saved = SavedFolders {
            version: FORMAT_VERSION,
            folders: self.session.recent.clone(),
        };
        // Written on the writer; a failure costs only the list.
        if let (Some(file), Ok(json)) = (
            self.state_file(RECENT_FOLDERS_FILE),
            serde_json::to_string_pretty(&saved),
        ) {
            self.write_later(file, move || Some(json));
        }
    }

    /// Writes the session for the next launch to bring back: the roots, the tabs in order with
    /// each page's editor mode and its caret from `carets` (the front end knows where each
    /// caret is), the preview tab, the active tab and the preview layout. Untitled pages, which
    /// have no file to reopen, are left out. Does nothing unless sessions are enabled.
    pub fn write_session(&self, carets: &HashMap<DocumentId, usize>) -> Result<(), EngineError> {
        if !self.session.enabled {
            return Ok(());
        }
        let mut tabs = Vec::new();
        let mut places: HashMap<TabItem, usize> = HashMap::new();
        for &item in &self.tabs.order {
            let (path, mode, caret) = match item {
                TabItem::Document(id) => {
                    let Some(document) = self.document(id).filter(|document| !document.untitled)
                    else {
                        continue;
                    };
                    let caret = carets.get(&id).copied().unwrap_or(0);
                    (document.path.clone(), self.editor_mode(id), caret)
                }
                TabItem::Image(id) => {
                    let Some(image) = self.image(id) else {
                        continue;
                    };
                    (image.path.clone(), EditorMode::Source, 0)
                }
            };
            places.insert(item, tabs.len());
            tabs.push(SavedTab {
                path,
                mode: mode.into(),
                caret,
                preview: self.is_preview(item),
                pinned: self.is_pinned(item),
            });
        }
        let layout = self.preview_layout();
        let saved = SavedSession {
            version: FORMAT_VERSION,
            roots: self.roots.clone(),
            tabs,
            active: self
                .active_item()
                .and_then(|item| places.get(&item).copied()),
            preview: SavedPreview {
                visible: layout.visible,
                detached: layout.detached,
                scroll_sync: layout.scroll_sync,
                pdf: layout.pdf,
                before_pdf: layout
                    .before_pdf
                    .map(|place| [place.visible, place.detached]),
                pinned: layout
                    .pinned
                    .and_then(|id| places.get(&TabItem::Document(id)).copied()),
            },
            graph_window: self.session.graph_window,
        };
        self.write_state_file(SESSION_FILE, &saved)
    }

    /// Brings back the session written last: opens the roots still there as the workspace,
    /// reopens the tabs whose files are still there with their editor modes, and restores the
    /// preview tab, the active tab and the preview layout. Returns where to put each reopened
    /// page's caret, within its text, for the front end to place; nothing when there is no
    /// session to restore or sessions are not enabled.
    pub fn restore_session(&mut self) -> HashMap<DocumentId, usize> {
        let mut carets = HashMap::new();
        if !self.session.enabled {
            return carets;
        }
        let Some(saved) = self
            .state_file(SESSION_FILE)
            .and_then(|file| fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str::<SavedSession>(&text).ok())
            .filter(|saved| saved.version == FORMAT_VERSION)
        else {
            return carets;
        };
        self.session.graph_window = saved.graph_window;
        let roots: Vec<PathBuf> = saved
            .roots
            .into_iter()
            .filter(|root| root.is_dir())
            .collect();
        if !roots.is_empty() {
            let _ = self.open_workspace(roots);
        }
        let mut opened: HashMap<usize, TabItem> = HashMap::new();
        self.restoring = true;
        for (place, tab) in saved.tabs.iter().enumerate() {
            if !tab.path.is_file() {
                continue;
            }
            let disposition = if tab.preview {
                OpenDisposition::Preview
            } else {
                OpenDisposition::Persistent
            };
            let Ok(item) = self.open_item(&tab.path, disposition) else {
                continue;
            };
            opened.insert(place, item);
            // Pinned tabs were saved first, so pinning each as it opens keeps their order.
            if tab.pinned {
                let _ = self.pin_tab(item);
            }
            let TabItem::Document(id) = item else {
                continue;
            };
            let Some(document) = self.document(id) else {
                continue;
            };
            let mode = EditorMode::from(tab.mode);
            // The models are built later: the active page's as the engine is next pumped,
            // every other page's when it is shown.
            if document.asciidoc && mode != EditorMode::Source {
                self.set_editor_mode_later(id, mode);
            }
            carets.insert(id, within_text(&self.documents[&id].text, tab.caret));
        }
        self.restoring = false;
        if !opened.is_empty() {
            self.refresh_open_diagnostics(None);
            self.audit_changed_everywhere();
        }
        if let Some(&item) = saved.active.and_then(|place| opened.get(&place)) {
            self.activate(item);
        }
        let mut layout = self.preview_layout();
        layout.visible = saved.preview.visible;
        layout.detached = saved.preview.detached;
        layout.scroll_sync = saved.preview.scroll_sync;
        // The PDF is a way of showing an open preview (#420).
        layout.pdf = cfg!(feature = "typst") && saved.preview.pdf && saved.preview.visible;
        layout.before_pdf = saved
            .preview
            .before_pdf
            .filter(|_| layout.pdf)
            .map(|[visible, detached]| crate::PreviewPlace { visible, detached });
        layout.pinned = saved
            .preview
            .pinned
            .and_then(|place| opened.get(&place))
            .and_then(|item| item.document());
        self.set_preview_layout(layout);
        carets
    }

    /// Where the graph window was when it was last moved, resized or closed, this session or,
    /// restored, the last.
    #[must_use]
    pub fn graph_window(&self) -> Option<WindowPlacement> {
        self.session.graph_window
    }

    /// Records where the graph window is, for [`Engine::write_session`] to keep. Writes
    /// nothing itself.
    pub fn set_graph_window(&mut self, placement: WindowPlacement) {
        self.session.graph_window = Some(placement);
    }

    fn state_file(&self, name: &str) -> Option<PathBuf> {
        self.settings.state_dir().map(|dir| dir.join(name))
    }

    fn write_state_file(&self, name: &str, value: &impl Serialize) -> Result<(), EngineError> {
        let Some(file) = self.state_file(name) else {
            return Ok(());
        };
        let json = serde_json::to_string_pretty(value)
            .map_err(|error| EngineError::InvalidRequest(error.to_string()))?;
        let written = file
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| write_replacing(&file, &json));
        written.map_err(|source| EngineError::Io { path: file, source })
    }
}

/// `caret` moved inside `text`: to its end when past it, and to the start of the character it
/// falls inside.
fn within_text(text: &ropey::Rope, caret: usize) -> usize {
    text.floor_char_boundary(caret.min(text.len()))
}
