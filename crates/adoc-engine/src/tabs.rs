//! Tab order and preview tabs. Browsing to a page shows it in one reusable preview tab, which the
//! next page browsed to replaces, until the page is edited, saved or kept open. Only a front end
//! that shows tabs turns preview tabs on ([`Engine::enable_preview_tabs`]), so the CLI, the
//! language server and the MCP server keep every document they open.

use std::path::Path;

use adoc_index::normalize_path;

use crate::{
    commands::{ids, register, spec},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, EngineEvent,
};

/// How a document is opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenDisposition {
    /// Kept open until closed: the Open File dialog, files named at start-up.
    Persistent,
    /// Browsed to: shown in the preview tab, replacing the page there, when preview tabs are on.
    Preview,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Tabs {
    /// The open documents in tab order.
    pub(crate) order: Vec<DocumentId>,
    /// The document in the preview tab.
    pub(crate) preview: Option<DocumentId>,
    /// Whether the front end shows preview tabs.
    pub(crate) enabled: bool,
}

impl Engine {
    /// Turns preview tabs on for a front end that shows documents as tabs; `[editor]
    /// preview_tabs` can still turn them off.
    pub fn enable_preview_tabs(&mut self) {
        self.tabs.enabled = true;
    }

    fn preview_tabs_on(&self) -> bool {
        self.tabs.enabled && self.settings.editor.preview_tabs
    }

    /// The open documents in tab order.
    #[must_use]
    pub fn tab_order(&self) -> &[DocumentId] {
        &self.tabs.order
    }

    /// Whether document `id` is in the preview tab, to be replaced by the next page browsed to.
    #[must_use]
    pub fn is_preview(&self, id: DocumentId) -> bool {
        self.tabs.preview == Some(id)
    }

    /// Opens the file at `path` as `disposition` says and activates it. A document already open
    /// is activated as it is: a kept tab never becomes a preview. Browsing reads the file before
    /// anything changes, so a file that cannot be read leaves the preview tab as it was; the page
    /// in the preview tab is then replaced in its place, unless it is edited or pinned in the
    /// rendered preview, which keep it open.
    pub fn open_as(
        &mut self,
        path: &Path,
        disposition: OpenDisposition,
    ) -> Result<DocumentId, EngineError> {
        let path = normalize_path(path);
        let browsing = disposition == OpenDisposition::Preview && self.preview_tabs_on();
        if !browsing || self.document_for_path(&path).is_some() {
            let id = self.open(&path)?;
            if disposition == OpenDisposition::Persistent {
                self.keep_open(id);
            }
            return Ok(id);
        }
        let text = self
            .file_access
            .read_to_string(&path)
            .map_err(|error| EngineError::io(&path, error))?;
        let old = self.tabs.preview.take();
        let id = self.insert_document(path, &text, false);
        self.tabs.preview = Some(id);
        let Some(old) = old else {
            self.set_active(id);
            return Ok(id);
        };
        let replaceable = self.document(old).is_some_and(|document| !document.dirty)
            && self.rendered.layout.pinned != Some(old);
        if !replaceable {
            self.emit(EngineEvent::DocumentKept(old));
            self.set_active(id);
            return Ok(id);
        }
        // The new page takes the old one's place in the tab strip.
        self.tabs.order.retain(|open| *open != id);
        let at = self
            .tabs
            .order
            .iter()
            .position(|open| *open == old)
            .unwrap_or(self.tabs.order.len());
        self.tabs.order.insert(at, id);
        self.emit(EngineEvent::PreviewTabReplaced { old, new: id });
        self.set_active(id);
        self.close(old)?;
        Ok(id)
    }

    /// Keeps document `id` open: it leaves the preview tab, where it stays as an ordinary tab.
    pub fn keep_open(&mut self, id: DocumentId) {
        if self.tabs.preview == Some(id) {
            self.tabs.preview = None;
            self.emit(EngineEvent::DocumentKept(id));
        }
    }

    /// Keeps the preview tab's document open when preview tabs have been turned off.
    pub(crate) fn settle_preview_tab(&mut self) {
        if !self.preview_tabs_on() {
            if let Some(id) = self.tabs.preview {
                self.keep_open(id);
            }
        }
    }
}

pub(crate) fn register_tabs(registry: &mut CommandRegistry) {
    use CommandCategory::Document;
    use CommandContext::{ActiveDocument, Always};
    register(
        registry,
        spec(
            ids::DOCUMENT_BROWSE,
            "Browse to File",
            Document,
            None,
            Always,
        ),
        |engine, args| {
            let CommandArgs::Path(path) = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::DOCUMENT_BROWSE,
                    expected: "a path",
                });
            };
            engine
                .open_as(&path, OpenDisposition::Preview)
                .map(CommandOutcome::Opened)
        },
    );
    register(
        registry,
        spec(
            ids::DOCUMENT_KEEP_OPEN,
            "Keep Open",
            Document,
            None,
            ActiveDocument,
        ),
        |engine, args| {
            let id = match args {
                CommandArgs::Document(id) => id,
                _ => engine
                    .active()
                    .ok_or_else(|| EngineError::NotFound("no document to keep open".to_owned()))?,
            };
            engine
                .document(id)
                .ok_or(EngineError::UnknownDocument(id))?;
            engine.keep_open(id);
            Ok(CommandOutcome::Done)
        },
    );
}
