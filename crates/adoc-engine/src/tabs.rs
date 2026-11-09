//! Tab order and preview tabs. Browsing to a page or an image shows it in one reusable preview
//! tab, which the next one browsed to replaces, until it is edited, saved or kept open. Only a
//! front end that shows tabs turns preview tabs on ([`Engine::enable_preview_tabs`]), so the CLI,
//! the language server and the MCP server keep everything they open. Tabs can be moved, as
//! dragging them does, and pinned (#82): pinned tabs sit first, and closing several tabs at
//! once leaves them open.

use std::{collections::HashSet, path::Path};

use adoc_index::normalize_path;

use crate::{
    commands::{ids, register, spec},
    images::is_image_path,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, EngineEvent, ImageId,
};

/// What a tab shows: a text document, or an image, which is never text.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TabItem {
    Document(DocumentId),
    Image(ImageId),
}

impl TabItem {
    #[must_use]
    pub fn document(self) -> Option<DocumentId> {
        match self {
            Self::Document(id) => Some(id),
            Self::Image(_) => None,
        }
    }

    #[must_use]
    pub fn image(self) -> Option<ImageId> {
        match self {
            Self::Image(id) => Some(id),
            Self::Document(_) => None,
        }
    }
}

impl From<DocumentId> for TabItem {
    fn from(id: DocumentId) -> Self {
        Self::Document(id)
    }
}

impl From<ImageId> for TabItem {
    fn from(id: ImageId) -> Self {
        Self::Image(id)
    }
}

impl PartialEq<DocumentId> for TabItem {
    fn eq(&self, other: &DocumentId) -> bool {
        *self == Self::Document(*other)
    }
}

/// How a document or an image is opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenDisposition {
    /// Kept open until closed: the Open File dialog, files named at start-up.
    Persistent,
    /// Browsed to: shown in the preview tab, replacing the page there, when preview tabs are on.
    Preview,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Tabs {
    /// The open documents and images in tab order, the pinned ones first.
    pub(crate) order: Vec<TabItem>,
    /// What the preview tab shows.
    pub(crate) preview: Option<TabItem>,
    /// Whether the front end shows preview tabs.
    pub(crate) enabled: bool,
    /// The pinned tabs, which closing several tabs at once leaves open. Not the page pinned in
    /// the rendered preview, which is `PreviewLayout::pinned`.
    pub(crate) pinned: HashSet<TabItem>,
}

impl Engine {
    /// Turns preview tabs on for a front end that shows documents as tabs; `[editor]
    /// preview_tabs` can still turn them off.
    pub fn enable_preview_tabs(&mut self) {
        self.tabs.enabled = true;
    }

    pub(crate) fn preview_tabs_on(&self) -> bool {
        self.tabs.enabled && self.settings.editor.preview_tabs
    }

    /// The open documents and images in tab order.
    #[must_use]
    pub fn tab_order(&self) -> &[TabItem] {
        &self.tabs.order
    }

    /// Whether `item` is in the preview tab, to be replaced by the next one browsed to.
    #[must_use]
    pub fn is_preview(&self, item: impl Into<TabItem>) -> bool {
        self.tabs.preview == Some(item.into())
    }

    /// The tab the user is working in: the active document, else the active image.
    #[must_use]
    pub fn active_item(&self) -> Option<TabItem> {
        self.active
            .map(TabItem::Document)
            .or(self.images.active.map(TabItem::Image))
    }

    /// Makes `item` the active tab.
    pub fn activate(&mut self, item: TabItem) {
        match item {
            TabItem::Document(id) => self.set_active(id),
            TabItem::Image(id) => self.set_active_image(id),
        }
    }

    /// Opens the file at `path` as what it is: an image in an image tab, anything else as a
    /// text document. Every way of opening a file goes through here, so a file opened from the
    /// Files view, a module's images, the Open dialog or a link opens the same way.
    pub fn open_item(
        &mut self,
        path: &Path,
        disposition: OpenDisposition,
    ) -> Result<TabItem, EngineError> {
        if is_image_path(path) {
            self.open_image_as(path, disposition).map(TabItem::Image)
        } else {
            self.open_as(path, disposition).map(TabItem::Document)
        }
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
        let text = self.read_text(&path)?;
        let old = self.tabs.preview.take();
        let id = self.insert_document(path, &text, false);
        self.take_preview_tab(old, TabItem::Document(id))?;
        Ok(id)
    }

    /// Puts `new`, just opened at the end of the tab strip, in the preview tab in place of `old`,
    /// which closes, unless it is a page that is edited or pinned in the rendered preview: that
    /// stays open as an ordinary tab. `new` is activated.
    pub(crate) fn take_preview_tab(
        &mut self,
        old: Option<TabItem>,
        new: TabItem,
    ) -> Result<(), EngineError> {
        self.tabs.preview = Some(new);
        let Some(old) = old else {
            self.activate(new);
            return Ok(());
        };
        let replaceable = match old {
            TabItem::Document(old) => {
                self.document(old).is_some_and(|document| !document.dirty)
                    && self.rendered.layout.pinned != Some(old)
            }
            TabItem::Image(old) => self.image(old).is_some(),
        };
        if !replaceable {
            self.emit(EngineEvent::TabKept(old));
            self.activate(new);
            return Ok(());
        }
        // The new tab takes the old one's place in the tab strip.
        self.tabs.order.retain(|open| *open != new);
        let at = self
            .tabs
            .order
            .iter()
            .position(|open| *open == old)
            .unwrap_or(self.tabs.order.len());
        self.tabs.order.insert(at, new);
        self.emit(EngineEvent::PreviewTabReplaced { old, new });
        self.activate(new);
        self.close_item(old)
    }

    /// Closes the document or image `item`.
    pub fn close_item(&mut self, item: TabItem) -> Result<(), EngineError> {
        match item {
            TabItem::Document(id) => self.close(id),
            TabItem::Image(id) => self.close_image(id),
        }
    }

    /// Keeps `item` open: it leaves the preview tab, where it stays as an ordinary tab.
    pub fn keep_open(&mut self, item: impl Into<TabItem>) {
        let item = item.into();
        if self.tabs.preview == Some(item) {
            self.tabs.preview = None;
            self.emit(EngineEvent::TabKept(item));
        }
    }

    /// Whether `item`'s tab is pinned: it sits with the other pinned tabs at the start of the
    /// strip, and closing several tabs at once leaves it open.
    #[must_use]
    pub fn is_pinned(&self, item: impl Into<TabItem>) -> bool {
        self.tabs.pinned.contains(&item.into())
    }

    /// How many tabs are pinned; they are the first so many of [`Engine::tab_order`].
    fn pinned_count(&self) -> usize {
        self.tabs.pinned.len()
    }

    fn open_tab(&self, item: TabItem) -> Result<usize, EngineError> {
        self.tabs
            .order
            .iter()
            .position(|open| *open == item)
            .ok_or_else(|| EngineError::NotFound(format!("{item:?} is not open")))
    }

    /// Pins `item`'s tab, last of the pinned ones; a preview tab is kept open.
    pub fn pin_tab(&mut self, item: TabItem) -> Result<(), EngineError> {
        let at = self.open_tab(item)?;
        if self.is_pinned(item) {
            return Ok(());
        }
        self.keep_open(item);
        let to = self.pinned_count();
        self.tabs.order.remove(at);
        self.tabs.order.insert(to, item);
        self.tabs.pinned.insert(item);
        self.emit(EngineEvent::TabsReordered);
        Ok(())
    }

    /// Unpins `item`'s tab, which goes first of the tabs that are not pinned.
    pub fn unpin_tab(&mut self, item: TabItem) -> Result<(), EngineError> {
        let at = self.open_tab(item)?;
        if !self.tabs.pinned.remove(&item) {
            return Ok(());
        }
        self.tabs.order.remove(at);
        let to = self.pinned_count();
        self.tabs.order.insert(to, item);
        self.emit(EngineEvent::TabsReordered);
        Ok(())
    }

    /// Moves `item`'s tab to place `to` of the strip, as dragging it there does, but only among
    /// tabs like it: a pinned tab stays among the pinned ones, and any other after them.
    pub fn move_tab(&mut self, item: TabItem, to: usize) -> Result<(), EngineError> {
        let at = self.open_tab(item)?;
        let pinned = self.pinned_count();
        // `at` is in its own group, so neither range is empty.
        let (first, last) = if self.is_pinned(item) {
            (0, pinned - 1)
        } else {
            (pinned, self.tabs.order.len() - 1)
        };
        let to = to.clamp(first, last);
        if to == at {
            return Ok(());
        }
        self.tabs.order.remove(at);
        self.tabs.order.insert(to, item);
        self.emit(EngineEvent::TabsReordered);
        Ok(())
    }

    /// Takes a closed tab out of the strip, the preview tab and the pinned tabs; returns where
    /// in the strip it was.
    pub(crate) fn forget_tab(&mut self, item: TabItem) -> Option<usize> {
        let at = self.tabs.order.iter().position(|open| *open == item);
        self.tabs.order.retain(|open| *open != item);
        if self.tabs.preview == Some(item) {
            self.tabs.preview = None;
        }
        self.tabs.pinned.remove(&item);
        at
    }

    /// Keeps the preview tab open when preview tabs have been turned off.
    pub(crate) fn settle_preview_tab(&mut self) {
        if !self.preview_tabs_on() {
            if let Some(item) = self.tabs.preview {
                self.keep_open(item);
            }
        }
    }

    /// After the active tab closed from position `closed_at`, makes the tab that took its
    /// place active, or the one before it when it was the last.
    pub(crate) fn activate_neighbour(&mut self, closed_at: usize) {
        let order = &self.tabs.order;
        match order
            .get(closed_at.min(order.len().saturating_sub(1)))
            .copied()
        {
            Some(TabItem::Document(id)) => {
                self.active = Some(id);
                if let Some(document) = self.documents.get(&id) {
                    self.recent.touch(&document.path);
                }
                self.refresh_visual(id);
            }
            Some(TabItem::Image(id)) => self.images.active = Some(id),
            None => {}
        }
        self.emit(EngineEvent::NavigationChanged);
    }
}

pub(crate) fn register_tabs(registry: &mut CommandRegistry) {
    use CommandCategory::Document;
    use CommandContext::{ActiveTab, Always};
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
                .open_item(&path, OpenDisposition::Preview)
                .map(CommandOutcome::opened)
        },
    );
    register(
        registry,
        spec(
            ids::DOCUMENT_KEEP_OPEN,
            "Keep Open",
            Document,
            None,
            ActiveTab,
        ),
        |engine, args| {
            let item = match args {
                CommandArgs::Document(id) => TabItem::Document(id),
                CommandArgs::Image(id) => TabItem::Image(id),
                _ => engine
                    .active_item()
                    .ok_or_else(|| EngineError::NotFound("no tab to keep open".to_owned()))?,
            };
            if !engine.tab_order().contains(&item) {
                return Err(EngineError::NotFound(format!("{item:?} is not open")));
            }
            engine.keep_open(item);
            Ok(CommandOutcome::Done)
        },
    );
    register(
        registry,
        spec(ids::DOCUMENT_PIN_TAB, "Pin Tab", Document, None, ActiveTab),
        |engine, args| {
            let item = tab_of(engine, ids::DOCUMENT_PIN_TAB, args)?;
            engine.pin_tab(item).map(|()| CommandOutcome::Done)
        },
    );
    register(
        registry,
        spec(
            ids::DOCUMENT_UNPIN_TAB,
            "Unpin Tab",
            Document,
            None,
            ActiveTab,
        ),
        |engine, args| {
            let item = tab_of(engine, ids::DOCUMENT_UNPIN_TAB, args)?;
            engine.unpin_tab(item).map(|()| CommandOutcome::Done)
        },
    );
    register(
        registry,
        spec(
            ids::DOCUMENT_MOVE_TAB,
            "Move Tab",
            Document,
            None,
            ActiveTab,
        ),
        |engine, args| {
            let CommandArgs::MoveTab { item, to } = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::DOCUMENT_MOVE_TAB,
                    expected: "a tab and the place to move it to",
                });
            };
            engine.move_tab(item, to).map(|()| CommandOutcome::Done)
        },
    );
}

/// The tab a command names, else the active one.
fn tab_of(
    engine: &Engine,
    command: crate::CommandId,
    args: CommandArgs,
) -> Result<TabItem, EngineError> {
    match args {
        CommandArgs::Document(id) => Ok(TabItem::Document(id)),
        CommandArgs::Image(id) => Ok(TabItem::Image(id)),
        CommandArgs::None => engine
            .active_item()
            .ok_or_else(|| EngineError::NotFound("no tab is open".to_owned())),
        _ => Err(EngineError::InvalidArguments {
            command,
            expected: "a document or image",
        }),
    }
}
