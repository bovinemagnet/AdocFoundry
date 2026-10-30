//! The workspace window: title bar, sidebar, editor tabs, outline, search panel and status bar.
//!
//! Views render engine state and dispatch commands. Opening a tab follows the engine's active
//! tab, a page or an image, so link following, back/forward and palette commands all arrive the
//! same way.

use std::{
    cell::Cell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
};

use adoc_core::{LineIndex, SourceRange};
use adoc_engine::{
    commands::ids, count_words, CommandArgs, CommandOutcome, DocumentId, EditPreview, EditorMode,
    EngineEvent, ExportArgs, ExportSummary, ExportTarget, NavigationPosition, OpenDisposition,
    SidebarPanel, TabItem, ThemePreference,
};
use gpui::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    command::{Command, CommandItem, CommandState},
    h_flex,
    input::{Editor, EditorState, Input, InputState},
    list::ListItem,
    menu::{ContextMenuExt as _, PopupMenuItem},
    notification::Notification,
    resizable::{h_resizable, resizable_panel},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    tree::{tree, TreeEvent, TreeState},
    v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _, Theme, ThemeMode, TitleBar,
    WindowExt as _,
};

use crate::{
    app::{open_secondary_window, AppState, WindowKind},
    editor::{bindings::*, language, SourceEditorEvent, SourceEditorView},
    image_view::{ActualSize, FitToWindow, ImageView, ZoomIn, ZoomOut},
    menus::AppMenu,
    preview::{PreviewEvent, PreviewView},
    search_view::SearchView,
    sidebar,
    visual::{VisualEvent, VisualView},
};

/// Follows the link at `offset` in `document`: the engine opens and activates the target, and the
/// workspace view reacts to that. Images and attachments open in the system's default app.
pub fn follow_link(
    app: Entity<AppState>,
    document: DocumentId,
    offset: usize,
    window: &mut Window,
    cx: &mut App,
) {
    let outcome = app.update(cx, |app, cx| {
        app.invoke(
            ids::NAVIGATE_FOLLOW_LINK,
            CommandArgs::Offset {
                id: document,
                offset,
            },
            cx,
        )
    });
    match outcome {
        Ok(CommandOutcome::External(path)) => cx.open_with_system(&path),
        Ok(_) => {}
        Err(error) => window.push_notification(error.to_string(), cx),
    }
}

/// How long typing must pause before the outline and problems list are rebuilt.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SidebarTab {
    Navigation,
    Families,
    Files,
    Problems,
    Changes,
}

/// What a tab shows: a page's source editor, or an image.
#[derive(Clone)]
enum TabView {
    Editor(Entity<SourceEditorView>),
    Image(Entity<ImageView>),
}

impl TabView {
    fn item(&self, cx: &App) -> TabItem {
        match self {
            Self::Editor(editor) => TabItem::Document(editor.read(cx).document()),
            Self::Image(image) => TabItem::Image(image.read(cx).image()),
        }
    }

    fn editor(&self) -> Option<&Entity<SourceEditorView>> {
        match self {
            Self::Editor(editor) => Some(editor),
            Self::Image(_) => None,
        }
    }
}

/// The arguments naming `item` to the commands that act on tabs.
pub(crate) fn item_args(item: TabItem) -> CommandArgs {
    match item {
        TabItem::Document(id) => CommandArgs::Document(id),
        TabItem::Image(id) => CommandArgs::Image(id),
    }
}

pub struct WorkspaceView {
    app: Entity<AppState>,
    focus_handle: FocusHandle,
    tabs: Vec<TabView>,
    /// The visual editor of each document shown in visual or split mode.
    visuals: HashMap<DocumentId, Entity<VisualView>>,
    active_tab: usize,
    /// The tab strip's scroll, so the active tab can be kept in view.
    tab_scroll: gpui::ScrollHandle,
    /// Frames still to request the active tab's reveal on; see [`Self::reveal_active_tab`].
    tab_reveals: std::cell::Cell<u8>,
    /// The tab last right-clicked, which the tab strip's menu acts on; `None` over empty strip.
    tab_menu_target: Rc<Cell<Option<TabItem>>>,
    sidebar_tab: SidebarTab,
    navigation: Entity<TreeState>,
    families: Entity<TreeState>,
    /// The rows of Navigation and Modules as last built, kept to show again when a bulk
    /// operation opens or closes branches.
    nav_nodes: Vec<sidebar::Node>,
    family_nodes: Vec<sidebar::Node>,
    /// The file the sidebar trees last followed, with `[sidebar] follow_active`: they follow
    /// again only once another becomes active, so a branch closed over it stays closed.
    followed: Option<PathBuf>,
    /// The file Navigation and Modules each still have to reveal once a rebuilt tree has a row
    /// for it, as while the workspace is first indexed.
    nav_follow: Option<PathBuf>,
    families_follow: Option<PathBuf>,
    /// The Files panel: the folders and files on disk.
    files: Entity<crate::files_view::FilesView>,
    /// The panel menu opened from the keyboard, and what closes it.
    sidebar_menu: Option<(
        SidebarPanel,
        Entity<gpui_component::menu::PopupMenu>,
        Subscription,
    )>,
    problems: Entity<TreeState>,
    problem_count: usize,
    problems_stale: bool,
    outline: Entity<TreeState>,
    outline_stale: bool,
    /// True from an edit until typing pauses; the outline and problems wait for the pause.
    unsettled: bool,
    _settle: Option<gpui::Task<()>>,
    search: Entity<SearchView>,
    show_search: bool,
    palette: Entity<CommandState>,
    /// The ☰ application menu in the title bar.
    app_menu: Entity<AppMenu>,
    preview: Entity<PreviewView>,
    /// The preview's own window while it is detached.
    detached: Option<(
        gpui::WindowHandle<gpui_component::Root>,
        Entity<PreviewView>,
    )>,
    revealed: Option<NavigationPosition>,
    _subscriptions: Vec<Subscription>,
}

/// The panel whose branches the tree `id` of [`WorkspaceView::render_tree`] opens and closes.
fn expansion_panel(id: &str) -> Option<SidebarPanel> {
    match id {
        "nav" => Some(SidebarPanel::Navigation),
        "families" => Some(SidebarPanel::Modules),
        _ => None,
    }
}

/// The menu of a Navigation or Modules panel: commands that act on the whole panel, whichever
/// row was right-clicked, and never open a page.
fn expansion_menu(
    menu: gpui_component::menu::PopupMenu,
    panel: SidebarPanel,
    app: &Entity<AppState>,
    cx: &App,
) -> gpui_component::menu::PopupMenu {
    let name = match panel {
        SidebarPanel::Navigation => "Navigation",
        SidebarPanel::Modules => "Modules",
    };
    let can_collapse = app.read(cx).engine.can_collapse_one_level(panel);
    let item = |label: &'static str, command: adoc_engine::CommandId| {
        let app = app.clone();
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            app.update(cx, |app, cx| {
                let _ = app.invoke(command, CommandArgs::Panel(panel), cx);
            });
        })
    };
    menu.label(format!("All of {name}"))
        .item(item("Expand All", ids::SIDEBAR_EXPAND_ALL))
        .item(item("Collapse All", ids::SIDEBAR_COLLAPSE_ALL))
        .item(item("Collapse One Level", ids::SIDEBAR_COLLAPSE_ONE_LEVEL).disabled(!can_collapse))
        .separator()
        .item(item(
            "Reset to Configured Default",
            ids::SIDEBAR_RESET_EXPANSION,
        ))
}

/// What hovering a preview tab says.
pub(crate) const PREVIEW_TAB_TOOLTIP: &str = "Preview tab — edit or double-click to keep open.";

/// A sidebar row's label: one line, cut off with an ellipsis where the panel is too narrow,
/// with the whole text in a tooltip. The tree's rows have one fixed height, so they cannot wrap.
fn tree_label(tree: &'static str, index: usize, label: SharedString) -> impl IntoElement {
    let full = label.clone();
    div()
        .id(("tree-label", index))
        .debug_selector(move || format!("{tree}-row-{index}"))
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(label)
        .tooltip(move |window, cx| {
            #[cfg(test)]
            LAST_TREE_TOOLTIP.with(|last| *last.borrow_mut() = Some(full.clone()));
            Tooltip::new(full.clone()).build(window, cx)
        })
}

#[cfg(test)]
thread_local! {
    /// The text of the last sidebar tooltip built, for tests.
    pub(crate) static LAST_TREE_TOOLTIP: std::cell::RefCell<Option<SharedString>> =
        const { std::cell::RefCell::new(None) };
}

/// The engine's commands the palette offers: those that apply now, so the visual editor's are
/// listed only while the active document is shown visually.
pub(crate) fn palette_commands(engine: &adoc_engine::Engine) -> Vec<&adoc_engine::CommandSpec> {
    engine
        .commands()
        .all()
        .filter(|spec| engine.command_available(spec.id))
        .collect()
}

/// What the notification says about a finished export: the file and its pages with the first
/// warning, or the error, which names the file and line it comes from.
/// An export of `target` in `theme`, its output still to be chosen.
fn export_args(target: ExportTarget, theme: Option<String>) -> ExportArgs {
    ExportArgs {
        target,
        output: PathBuf::new(),
        theme,
        standards: Vec::new(),
        fonts_dir: None,
        typst: None,
        attributes: Vec::new(),
    }
}

pub(crate) fn export_message(output: &Path, result: &Result<ExportSummary, String>) -> String {
    let html = output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("html"));
    match result {
        Ok(summary) => {
            let warnings = summary.diagnostics.len();
            let name = output
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let mut message = if html {
                format!("Exported {name}")
            } else {
                format!("Exported {name} ({} pages)", summary.pages)
            };
            if warnings > 0 {
                message.push_str(&format!(
                    "; {warnings} warning{}: {}",
                    if warnings == 1 { "" } else { "s" },
                    summary.diagnostics[0]
                ));
            }
            message
        }
        Err(error) if html => format!("HTML export failed: {error}"),
        Err(error) => format!("PDF export failed: {error}"),
    }
}

impl WorkspaceView {
    pub fn new(
        app: Entity<AppState>,
        folder: Option<std::path::PathBuf>,
        files: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let preview = cx.new(|cx| PreviewView::new(app.clone(), false, window, cx));
        let navigation = cx.new(|cx| TreeState::new(cx));
        let families = cx.new(|cx| TreeState::new(cx));
        let subscriptions = vec![
            cx.subscribe_in(&app, window, Self::on_engine_event),
            cx.subscribe(&preview, Self::on_preview_event),
            Self::record_expansion(&navigation, SidebarPanel::Navigation, cx),
            Self::record_expansion(&families, SidebarPanel::Modules, cx),
            // `[editor] auto_save = "on_focus_change"` saves when the window loses focus.
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.app.update(cx, |app, _| app.engine.focus_lost());
                }
            }),
        ];
        let workspace = cx.weak_entity();
        let search = cx.new(|cx| SearchView::new(app.clone(), workspace, window, cx));
        let focus_handle = cx.focus_handle();
        let app_menu = cx.new(|_| AppMenu::new(app.clone(), focus_handle.clone()));
        let mut view = Self {
            focus_handle,
            tabs: Vec::new(),
            visuals: HashMap::new(),
            active_tab: 0,
            tab_scroll: gpui::ScrollHandle::new(),
            tab_reveals: std::cell::Cell::new(0),
            tab_menu_target: Rc::default(),
            sidebar_tab: SidebarTab::Navigation,
            navigation,
            families,
            nav_nodes: Vec::new(),
            family_nodes: Vec::new(),
            followed: None,
            nav_follow: None,
            families_follow: None,
            sidebar_menu: None,
            files: {
                let app = app.clone();
                let workspace = cx.weak_entity();
                cx.new(|cx| crate::files_view::FilesView::new(app, workspace, cx))
            },
            problems: cx.new(|cx| TreeState::new(cx)),
            problem_count: 0,
            problems_stale: true,
            outline: cx.new(|cx| TreeState::new(cx)),
            outline_stale: true,
            unsettled: false,
            _settle: None,
            search,
            show_search: false,
            palette: cx.new(|cx| CommandState::new(window, cx)),
            app_menu,
            preview,
            detached: None,
            revealed: None,
            app,
            _subscriptions: subscriptions,
        };
        view.apply_theme(window, cx);
        view.warn_about_settings(window, cx);
        if let Some(folder) = folder {
            view.open_folder(folder, window, cx);
        }
        for file in files {
            view.open_at(
                &file,
                SourceRange::default(),
                OpenDisposition::Persistent,
                window,
                cx,
            );
        }
        if let (Some(duration), Some(editor)) = (
            crate::typing_bench::requested(),
            view.active_editor().cloned(),
        ) {
            crate::typing_bench::start(editor, duration, window, cx);
        }
        // With no page open nothing else takes focus, and the workspace's keys need it.
        if window.focused(cx).is_none() {
            window.focus(&view.focus_handle, cx);
        }
        crate::updates::check_at_start_up(&view.app, window, cx);
        view.offer_recovery_at_start_up(window, cx);
        view
    }

    /// Offers what a session that ended left unsaved, once the window is drawn.
    fn offer_recovery_at_start_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.app.read(cx).engine.recoverable();
        if items.is_empty() {
            return;
        }
        let (app, workspace) = (self.app.clone(), cx.weak_entity());
        window.defer(cx, move |window, cx| {
            crate::history_view::show_recovery(app, workspace, items, window, cx);
        });
    }

    // --- engine events -----------------------------------------------------------------------

    fn on_engine_event(
        &mut self,
        _: &Entity<AppState>,
        event: &EngineEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EngineEvent::WorkspaceIndexed { .. } | EngineEvent::CatalogChanged { .. } => {
                self.rebuild_catalog_trees(cx);
                self.problems_stale = true;
            }
            EngineEvent::DiagnosticsUpdated { .. } => {
                self.problems_stale = true;
                self.settle_later(cx);
            }
            EngineEvent::NavigationChanged => self.follow_active(window, cx),
            EngineEvent::ExpansionChanged(panel) => self.show_tree(*panel, cx),
            EngineEvent::PreviewTabReplaced { old, new } => {
                self.replace_tab(*old, *new, window, cx);
            }
            EngineEvent::DocumentClosed(id) => {
                self.tabs
                    .retain(|tab| tab.item(cx) != TabItem::Document(*id));
                self.visuals.remove(id);
                self.follow_active(window, cx);
            }
            EngineEvent::ImageClosed(id) => {
                self.tabs.retain(|tab| tab.item(cx) != TabItem::Image(*id));
                self.follow_active(window, cx);
            }
            EngineEvent::EditorModeChanged(id) => self.sync_visual(*id, window, cx),
            EngineEvent::DocumentChanged { id, .. } if Some(*id) == self.active_document(cx) => {
                self.outline_stale = true;
                self.settle_later(cx);
            }
            EngineEvent::DocumentMoved { .. } => self.problems_stale = true,
            EngineEvent::DocumentSaved(id) => self.reload_settings_if_edited(*id, cx),
            EngineEvent::DocumentChangedOnDisk(id) => {
                let name = self.document_name(*id, cx);
                window.push_notification(
                    format!("{name} changed on disk; your unsaved edits were kept."),
                    cx,
                );
            }
            EngineEvent::SettingsChanged => {
                self.apply_theme(window, cx);
                self.warn_about_settings(window, cx);
            }
            EngineEvent::CommandFailed { command, error } => {
                window.push_notification(format!("{command}: {error}"), cx);
            }
            EngineEvent::PreviewLayoutChanged => {
                self.place_preview(cx);
                // Docking or detaching the preview changes the strip's width.
                self.reveal_active_tab(cx);
            }
            EngineEvent::GitPushed { message } => window.push_notification(message.clone(), cx),
            EngineEvent::ExportFinished { output, result } => {
                Self::export_finished(output, result, window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Creates or drops the visual editor of `id` to match its editor mode, and focuses the
    /// editor the active document now shows.
    fn sync_visual(&mut self, id: DocumentId, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.app.read(cx).engine.editor_mode(id);
        if mode == EditorMode::Source {
            self.visuals.remove(&id);
        } else if !self.visuals.contains_key(&id) {
            let app = self.app.clone();
            let visual = cx.new(|cx| VisualView::new(app, id, window, cx));
            self._subscriptions
                .push(cx.subscribe(&visual, Self::on_visual_event));
            self.visuals.insert(id, visual);
        }
        if self.active_document(cx) == Some(id) {
            self.focus_active_editor(window, cx);
        }
    }

    /// Focuses the visual editor in visual mode, else the source editor, or the image viewer.
    fn focus_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabView::Image(image)) = self.tabs.get(self.active_tab) {
            let focus = image.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            return;
        }
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        let id = editor.read(cx).document();
        let mode = self.app.read(cx).engine.editor_mode(id);
        let focus = match (mode, self.visuals.get(&id)) {
            (EditorMode::Visual, Some(visual)) => visual.read(cx).focus_handle(cx),
            _ => editor.read(cx).focus_handle(cx),
        };
        window.focus(&focus, cx);
    }

    /// Keeps the preview level with the caret of the active visual editor.
    fn on_visual_event(
        &mut self,
        visual: Entity<VisualView>,
        event: &VisualEvent,
        cx: &mut Context<Self>,
    ) {
        let VisualEvent::CaretMoved(offset) = *event;
        let id = visual.read(cx).document();
        if self.active_document(cx) != Some(id) {
            return;
        }
        // Side by side, the source follows the visual editor's caret.
        if self.app.read(cx).engine.editor_mode(id) == EditorMode::Split {
            if let Some(editor) = self.active_editor().cloned() {
                editor.update(cx, |editor, cx| editor.scroll_to_offset(offset, cx));
            }
        }
        let path = {
            let engine = &self.app.read(cx).engine;
            if !engine.preview_layout().scroll_sync {
                return;
            }
            let Some(document) = engine.document(id) else {
                return;
            };
            document.path.clone()
        };
        let previews = std::iter::once(self.preview.clone())
            .chain(self.detached.as_ref().map(|(_, view)| view.clone()));
        for preview in previews {
            preview.update(cx, |preview, cx| preview.reveal_source(&path, offset, cx));
        }
    }

    #[cfg(test)]
    pub fn visual_for_test(&self, id: DocumentId) -> Option<&Entity<VisualView>> {
        self.visuals.get(&id)
    }

    /// Opens the preview's own window when it is detached, and closes it when it docks or hides.
    fn place_preview(&mut self, cx: &mut Context<Self>) {
        let layout = self.app.read(cx).engine.preview_layout();
        let wants_window = layout.visible && layout.detached;
        match (&self.detached, wants_window) {
            (None, true) => {
                self.detached = open_secondary_window(self.app.clone(), WindowKind::Preview, cx)
                    .and_then(|(handle, view)| {
                        view.downcast::<PreviewView>()
                            .ok()
                            .map(|view| (handle, view))
                    });
                if let Some((_, view)) = &self.detached {
                    self._subscriptions
                        .push(cx.subscribe(view, Self::on_preview_event));
                }
            }
            (Some(_), false) => {
                if let Some((handle, _)) = self.detached.take() {
                    // Fails harmlessly when the window is already closing itself.
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            }
            _ => {}
        }
    }

    /// Keeps the preview level with the caret of the active editor.
    fn on_editor_event(
        &mut self,
        editor: Entity<SourceEditorView>,
        event: &SourceEditorEvent,
        cx: &mut Context<Self>,
    ) {
        let SourceEditorEvent::CaretMoved(offset) = *event;
        if self.active_editor() != Some(&editor) {
            return;
        }
        // Side by side, the visual editor follows the source caret.
        let id = editor.read(cx).document();
        if self.app.read(cx).engine.editor_mode(id) == EditorMode::Split {
            if let Some(visual) = self.visuals.get(&id).cloned() {
                visual.update(cx, |visual, cx| visual.reveal_offset(offset, cx));
            }
        }
        let path = {
            let engine = &self.app.read(cx).engine;
            if !engine.preview_layout().scroll_sync {
                return;
            }
            let Some(document) = engine.document(editor.read(cx).document()) else {
                return;
            };
            document.path.clone()
        };
        let previews = std::iter::once(self.preview.clone())
            .chain(self.detached.as_ref().map(|(_, view)| view.clone()));
        for preview in previews {
            preview.update(cx, |preview, cx| preview.reveal_source(&path, offset, cx));
        }
    }

    /// Scrolls the active editor to the block the user scrolled the preview to.
    fn on_preview_event(
        &mut self,
        _: Entity<PreviewView>,
        event: &PreviewEvent,
        cx: &mut Context<Self>,
    ) {
        let PreviewEvent::ScrolledTo(span) = event;
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        let same_file = {
            let engine = &self.app.read(cx).engine;
            engine.preview_layout().scroll_sync
                && engine
                    .document(editor.read(cx).document())
                    .is_some_and(|document| document.path == span.path)
        };
        if same_file {
            editor.update(cx, |editor, cx| {
                editor.scroll_to_offset(span.range.start, cx)
            });
        }
    }

    #[cfg(test)]
    pub fn active_editor_for_test(&self) -> Option<&Entity<SourceEditorView>> {
        self.active_editor()
    }

    #[cfg(test)]
    pub fn detached_preview(&self) -> Option<&Entity<PreviewView>> {
        self.detached.as_ref().map(|(_, view)| view)
    }

    fn rebuild_catalog_trees(&mut self, cx: &mut Context<Self>) {
        let (navigation, families) = {
            let engine = &self.app.read(cx).engine;
            (sidebar::navigation(engine), sidebar::families(engine))
        };
        self.app.update(cx, |app, _| {
            app.engine
                .resolve_expansion(SidebarPanel::Navigation, &sidebar::branches(&navigation));
            app.engine
                .resolve_expansion(SidebarPanel::Modules, &sidebar::branches(&families));
        });
        self.nav_nodes = navigation;
        self.family_nodes = families;
        self.show_tree(SidebarPanel::Navigation, cx);
        self.show_tree(SidebarPanel::Modules, cx);
        for panel in [SidebarPanel::Navigation, SidebarPanel::Modules] {
            if let Some(file) = self.pending_follow(panel).clone() {
                if self.reveal_in_tree(panel, &file, cx) {
                    *self.pending_follow(panel) = None;
                }
            }
        }
    }

    fn pending_follow(&mut self, panel: SidebarPanel) -> &mut Option<PathBuf> {
        match panel {
            SidebarPanel::Navigation => &mut self.nav_follow,
            SidebarPanel::Modules => &mut self.families_follow,
        }
    }

    /// With `[sidebar] follow_active`, reveals the active page or image in Files, Navigation and
    /// Modules when it is not the file they last followed, without taking focus.
    fn follow_in_sidebar(&mut self, cx: &mut Context<Self>) {
        let file = {
            let engine = &self.app.read(cx).engine;
            if !engine.settings().sidebar.follow_active {
                return;
            }
            match engine.active_item() {
                Some(TabItem::Document(id)) => engine.document(id).map(|document| &document.path),
                Some(TabItem::Image(id)) => engine.image(id).map(|image| &image.path),
                None => None,
            }
            .cloned()
        };
        let Some(file) = file else {
            return;
        };
        if self.followed.as_ref() == Some(&file) {
            return;
        }
        self.followed = Some(file.clone());
        // Deferred: a click in Files opens the file while Files is being updated.
        let files = self.files.clone();
        let revealed = file.clone();
        cx.defer(move |cx| {
            files.update(cx, |files, cx| files.reveal(&revealed, cx));
        });
        for panel in [SidebarPanel::Navigation, SidebarPanel::Modules] {
            let found = self.reveal_in_tree(panel, &file, cx);
            *self.pending_follow(panel) = (!found).then(|| file.clone());
        }
    }

    /// Selects the row of `panel` that opens `file`, keeping the selected row when it does,
    /// opening the branches above it and scrolling it into view. Without such a row the
    /// selection is cleared and the answer is false.
    fn reveal_in_tree(&mut self, panel: SidebarPanel, file: &Path, cx: &mut Context<Self>) -> bool {
        let (state, nodes) = match panel {
            SidebarPanel::Navigation => (self.navigation.clone(), &self.nav_nodes),
            SidebarPanel::Modules => (self.families.clone(), &self.family_nodes),
        };
        let selected = state
            .read(cx)
            .selected_item()
            .map(|item| item.id.to_string());
        let kept = selected
            .as_deref()
            .filter(|id| sidebar::opens_file(id, file));
        let ids = match kept {
            Some(id) => sidebar::path_to(nodes, id),
            None => sidebar::path_to_file(nodes, file),
        };
        let Some(ids) = ids else {
            state.update(cx, |tree, cx| tree.set_selected_index(None, cx));
            return false;
        };
        let branches: Vec<&str> = ids[..ids.len() - 1]
            .iter()
            .map(|id| sidebar::branch_key(id))
            .collect();
        self.app.update(cx, |app, _| {
            app.engine.open_branches(panel, &branches);
        });
        let items = sidebar::tree_items(nodes, &self.app.read(cx).engine, panel);
        let moved = kept.is_none();
        state.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            let index = ids.last().and_then(|id| tree.index_of(&id.into()));
            tree.set_selected_index(index, cx);
            if let (Some(index), true) = (index, moved) {
                tree.scroll_to_item(index, gpui::ScrollStrategy::Center);
            }
        });
        true
    }

    /// Records the branches the user opens and closes in `tree`, a panel's.
    fn record_expansion(
        tree: &Entity<TreeState>,
        panel: SidebarPanel,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe(tree, move |this, _, event: &TreeEvent, cx| {
            let (id, expanded) = match event {
                TreeEvent::Expanded(id) => (id, true),
                TreeEvent::Collapsed(id) => (id, false),
            };
            this.app.update(cx, |app, _| {
                app.engine
                    .set_branch_expanded(panel, sidebar::branch_key(id), expanded);
            });
        })
    }

    /// Shows `panel`'s rows as last built, each branch open or closed as the engine has it,
    /// keeping the selected row, or the nearest row above it still shown.
    fn show_tree(&mut self, panel: SidebarPanel, cx: &mut Context<Self>) {
        let (state, nodes) = match panel {
            SidebarPanel::Navigation => (self.navigation.clone(), &self.nav_nodes),
            SidebarPanel::Modules => (self.families.clone(), &self.family_nodes),
        };
        let items = sidebar::tree_items(nodes, &self.app.read(cx).engine, panel);
        let selected = state
            .read(cx)
            .selected_item()
            .map(|item| item.id.to_string());
        let path = selected.and_then(|id| sidebar::path_to(nodes, &id));
        state.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            let shown = path
                .iter()
                .flatten()
                .rev()
                .find_map(|id| tree.index_of(&id.into()));
            tree.set_selected_index(shown, cx);
        });
    }

    /// Defers the outline and problems until no edit has arrived for [`SETTLE`].
    fn settle_later(&mut self, cx: &mut Context<Self>) {
        self.unsettled = true;
        self._settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update(cx, |this, cx| {
                this.unsettled = false;
                cx.notify();
            });
        }));
    }

    fn refresh_problems(&mut self, cx: &mut Context<Self>) {
        if !self.problems_stale || self.unsettled {
            return;
        }
        self.problems_stale = false;
        let (items, count) = {
            let engine = &self.app.read(cx).engine;
            sidebar::problems(engine, engine.roots())
        };
        self.problem_count = count;
        self.problems
            .update(cx, |tree, cx| tree.set_items(items, cx));
    }

    /// The rows `panel` shows, each indented two spaces a level, with `+` after an open branch
    /// and `-` after a closed one.
    #[cfg(test)]
    pub fn tree_rows_for_test(&self, panel: SidebarPanel, cx: &App) -> Vec<String> {
        let state = match panel {
            SidebarPanel::Navigation => &self.navigation,
            SidebarPanel::Modules => &self.families,
        }
        .read(cx);
        (0..)
            .map_while(|ix| state.entry(ix))
            .map(|entry| {
                let mark = match (entry.is_folder(), entry.is_expanded()) {
                    (false, _) => "",
                    (true, true) => " +",
                    (true, false) => " -",
                };
                format!("{}{}{mark}", "  ".repeat(entry.depth()), entry.item().label)
            })
            .collect()
    }

    /// The position among the rows shown of the row selected in `panel`.
    #[cfg(test)]
    pub fn selected_index_for_test(&self, panel: SidebarPanel, cx: &App) -> Option<usize> {
        let state = match panel {
            SidebarPanel::Navigation => &self.navigation,
            SidebarPanel::Modules => &self.families,
        }
        .read(cx);
        let selected = state.selected_item()?.id.clone();
        (0..)
            .map_while(|ix| state.entry(ix))
            .position(|entry| entry.item().id == selected)
    }

    /// The label of the row selected in `panel`.
    #[cfg(test)]
    pub fn selected_row_for_test(&self, panel: SidebarPanel, cx: &App) -> Option<String> {
        let state = match panel {
            SidebarPanel::Navigation => &self.navigation,
            SidebarPanel::Modules => &self.families,
        }
        .read(cx);
        state.selected_item().map(|item| item.label.to_string())
    }

    /// Shows the Files panel and hands it over.
    #[cfg(test)]
    pub fn show_files_for_test(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<crate::files_view::FilesView> {
        self.sidebar_tab = SidebarTab::Files;
        cx.notify();
        self.files.clone()
    }

    /// Shows the Modules panel.
    #[cfg(test)]
    pub fn show_modules_for_test(&mut self, cx: &mut Context<Self>) {
        self.sidebar_tab = SidebarTab::Families;
        cx.notify();
    }

    /// Shows the Problems panel.
    #[cfg(test)]
    pub fn show_problems_for_test(&mut self, cx: &mut Context<Self>) {
        self.sidebar_tab = SidebarTab::Problems;
        self.problems_stale = true;
        cx.notify();
    }

    /// The labels of the Changes panel's rows, in order.
    #[cfg(test)]
    pub fn changes_for_test(&mut self, cx: &mut Context<Self>) -> Vec<String> {
        sidebar::change_rows(&self.app.read(cx).engine)
            .into_iter()
            .flat_map(|(_, rows)| rows.into_iter().map(|row| row.label))
            .collect()
    }

    /// The Changes panel: Stage All and Commit…, then a row per changed file with a box to
    /// stage it; a click on its name opens it at its first change.
    fn render_changes(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let groups = sidebar::change_rows(&self.app.read(cx).engine);
        let any = groups.iter().any(|(_, rows)| !rows.is_empty());
        let staged = groups
            .iter()
            .any(|(_, rows)| rows.iter().any(|row| row.staged));
        let mut list = v_flex().gap_0p5().px_1();
        for (group, rows) in groups {
            if let Some(name) = group {
                list = list.child(
                    div()
                        .px_1()
                        .pt_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(name),
                );
            }
            for (index, row) in rows.into_iter().enumerate() {
                let stage_path = row.path.clone();
                let open_path = row.path.clone();
                let selector = format!("change-row-{}", row.label);
                list = list.child(
                    h_flex()
                        .debug_selector(move || selector)
                        .gap_1()
                        .text_sm()
                        .child(
                            gpui_component::checkbox::Checkbox::new((
                                gpui::SharedString::from(row.path.display().to_string()),
                                index,
                            ))
                            .checked(row.staged)
                            .on_click(cx.listener(
                                move |this, checked: &bool, window, cx| {
                                    let command = if *checked {
                                        ids::GIT_STAGE
                                    } else {
                                        ids::GIT_UNSTAGE
                                    };
                                    this.invoke(
                                        command,
                                        CommandArgs::Path(stage_path.clone()),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        )
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "change-open-{}",
                                    row.path.display()
                                )))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .cursor_pointer()
                                .when(row.deleted, |label| {
                                    label.text_color(cx.theme().muted_foreground).line_through()
                                })
                                .child(row.label)
                                .when(!row.deleted, |label| {
                                    label.on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_first_change(&open_path, window, cx);
                                    }))
                                }),
                        ),
                );
            }
        }
        v_flex()
            .size_full()
            .gap_1()
            .child(
                h_flex()
                    .gap_1()
                    .p_1()
                    .child(
                        Button::new("stage-all")
                            .xsmall()
                            .ghost()
                            .label("Stage All")
                            .disabled(!any)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.invoke(ids::GIT_STAGE_ALL_ADOC, CommandArgs::None, window, cx);
                            })),
                    )
                    .child(
                        Button::new("commit")
                            .xsmall()
                            .label("Commit…")
                            .disabled(!staged)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_commit_dialog(String::new(), Vec::new(), window, cx);
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }

    /// Asks for a commit message, listing what will be committed, and commits (and pushes).
    /// `paths` are committed as they are saved; otherwise what is staged.
    pub fn open_commit_dialog(
        &mut self,
        message: String,
        paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files: Vec<String> = {
            let engine = &self.app.read(cx).engine;
            let roots = engine.roots();
            engine
                .changed_writer_files()
                .into_iter()
                .filter(|(path, status)| {
                    paths.contains(path) || status.index != adoc_engine::GitChange::Unmodified
                })
                .map(|(path, _)| sidebar::display_path(&path, roots))
                .collect()
        };
        let state = cx.new(|cx| {
            let mut state = gpui_component::input::TextareaState::new(window, cx);
            state.set_value(message, window, cx);
            state
        });
        let workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let summary = state
                .read(cx)
                .value()
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .count();
            let commit = |push: bool| {
                let (state, workspace, paths) = (state.clone(), workspace.clone(), paths.clone());
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                    let message = state.read(cx).value().to_string();
                    window.close_dialog(cx);
                    if let Some(workspace) = workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| {
                            workspace.commit(message, &paths, push, window, cx);
                        });
                    }
                }
            };
            dialog.title("Commit").width(px(560.)).child(
                v_flex()
                    .gap_2()
                    .child(gpui_component::input::Textarea::new(&state).h(px(120.)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if summary > 72 {
                                cx.theme().warning
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(format!("Summary: {summary} of 72 characters")),
                    )
                    .child(
                        v_flex()
                            .text_sm()
                            .children(files.iter().map(|file| div().child(file.clone()))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("commit-and-push")
                                    .label("Commit and Push")
                                    .on_click(commit(true)),
                            )
                            .child(
                                Button::new("commit-only")
                                    .primary()
                                    .label("Commit")
                                    .on_click(commit(false)),
                            ),
                    ),
            )
        });
    }

    /// Commits with `message`, saving and staging `paths` first, then pushes when asked.
    pub fn commit(
        &mut self,
        message: String,
        paths: &[std::path::PathBuf],
        push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<String> = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let args = CommandArgs::Custom(serde_json::json!({ "message": message, "paths": paths }));
        let Some(CommandOutcome::Committed { commit, saved }) =
            self.invoke(ids::GIT_COMMIT, args, window, cx)
        else {
            return;
        };
        let short = &commit[..commit.len().min(7)];
        let note = if saved.is_empty() {
            format!("Committed {short}")
        } else {
            let files = if saved.len() == 1 { "file" } else { "files" };
            format!("Saved {} {files} and committed {short}", saved.len())
        };
        window.push_notification(note, cx);
        if push {
            self.invoke(ids::GIT_PUSH, CommandArgs::None, window, cx);
        }
    }

    /// The component version and branch of the active page, above the navigation tree.
    fn version_header(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let engine = &self.app.read(cx).engine;
        let path = engine
            .active()
            .and_then(|id| engine.document(id))
            .map(|document| document.path.clone())?;
        let badge = engine.version_badge(&path)?;
        let label = match engine.git().head_for(&path) {
            Some(head) => format!("{badge} · {head}"),
            None => badge,
        };
        Some(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label)
                .into_any_element(),
        )
    }

    /// Offers the active page's other versions in the workspace, and opens the one chosen.
    fn on_open_in_version(
        &mut self,
        _: &OpenInVersion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let Some(id) = engine.active() else {
            return;
        };
        let others = engine.other_versions(id);
        if others.is_empty() {
            window.push_notification(
                "No other version of this page is in the workspace: check out its branch in \
                 another folder and add the folder to the workspace.",
                cx,
            );
            return;
        }
        let component = engine
            .document(id)
            .and_then(|document| engine.version_badge(&document.path))
            .and_then(|badge| badge.split_once('@').map(|(name, _)| name.to_owned()))
            .unwrap_or_default();
        let workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let buttons = others.iter().enumerate().map(|(index, (version, _))| {
                let version = version.clone();
                let label = format!("{component}@{}", version.as_deref().unwrap_or("latest"));
                let workspace = workspace.clone();
                Button::new(("version", index))
                    .label(label)
                    .ghost()
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let args = CommandArgs::Custom(serde_json::json!({
                            "document": id.get(),
                            "version": version,
                        }));
                        if let Some(workspace) = workspace.upgrade() {
                            workspace.update(cx, |workspace, cx| {
                                workspace.invoke(ids::NAVIGATE_OPEN_IN_VERSION, args, window, cx);
                            });
                        }
                    })
            });
            dialog
                .title("Open in Another Version")
                .child(v_flex().gap_1().children(buttons))
        });
    }

    /// Whether the sidebar offers the Changes panel: when a root is in a repository.
    fn has_changes_panel(&self, cx: &App) -> bool {
        !self.app.read(cx).engine.git().repositories().is_empty()
    }

    /// Opens `path` with the caret on its first change against `HEAD`.
    pub fn open_first_change(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_busy_preview(window, cx);
        let Some(CommandOutcome::Opened(id)) = self.invoke(
            ids::DOCUMENT_BROWSE,
            CommandArgs::Path(path.to_path_buf()),
            window,
            cx,
        ) else {
            return;
        };
        let first = self
            .app
            .read(cx)
            .engine
            .hunks_now(id)
            .ok()
            .and_then(|hunks| hunks.first().map(|hunk| hunk.range.start))
            .unwrap_or(0);
        self.open_at(
            path,
            SourceRange::new(first, first),
            OpenDisposition::Preview,
            window,
            cx,
        );
    }

    fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        if !self.outline_stale || self.unsettled {
            return;
        }
        self.outline_stale = false;
        let items = match self.active_document(cx) {
            Some(id) => sidebar::outline(&self.app.read(cx).engine, id),
            None => Vec::new(),
        };
        self.outline
            .update(cx, |tree, cx| tree.set_items(items, cx));
    }

    /// Shows a tab for the engine's active document and reveals the last navigation target.
    fn follow_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.outline_stale = true;
        let Some(item) = self.app.read(cx).engine.active_item() else {
            self.active_tab = 0;
            return;
        };
        let index = match self.tabs.iter().position(|tab| tab.item(cx) == item) {
            Some(index) => index,
            None => {
                let tab = self.new_tab(item, window, cx);
                self.tabs.push(tab);
                self.tabs.len() - 1
            }
        };
        self.active_tab = index;
        // However the document became active, its tab is brought into view.
        self.reveal_active_tab(cx);
        self.follow_in_sidebar(cx);
        let TabItem::Document(active) = item else {
            self.focus_active_editor(window, cx);
            return;
        };

        let position = self.app.read(cx).engine.navigation_position().cloned();
        let path = self
            .app
            .read(cx)
            .engine
            .document(active)
            .map(|document| document.path.clone());
        if let Some(position) = position {
            if self.revealed.as_ref() != Some(&position) && path.as_ref() == Some(&position.path) {
                let range = SourceRange::new(position.offset, position.offset);
                if let Some(editor) = self.tabs[index].editor().cloned() {
                    editor.update(cx, |editor, cx| editor.reveal(range, window, cx));
                }
                if let Some(visual) = self.visuals.get(&active).cloned() {
                    visual.update(cx, |visual, cx| visual.go_to(position.offset, cx));
                }
                self.revealed = Some(position);
                self.focus_active_editor(window, cx);
                return;
            }
        }
        self.focus_active_editor(window, cx);
    }

    fn new_tab(&mut self, item: TabItem, window: &mut Window, cx: &mut Context<Self>) -> TabView {
        let app = self.app.clone();
        match item {
            TabItem::Document(id) => {
                let editor = cx.new(|cx| SourceEditorView::new(app, id, false, window, cx));
                self._subscriptions
                    .push(cx.subscribe(&editor, Self::on_editor_event));
                TabView::Editor(editor)
            }
            TabItem::Image(id) => TabView::Image(cx.new(|cx| ImageView::new(app, id, cx))),
        }
    }

    /// Shows `new` in the tab of `old`, which the engine replaced in the preview tab. The view
    /// may already have made `new` a tab of its own, at the end, when it followed the engine
    /// before this event arrived: that tab moves to the old one's place.
    fn replace_tab(
        &mut self,
        old: TabItem,
        new: TabItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.item(cx) == old) else {
            return;
        };
        let tab = match self.tabs.iter().position(|tab| tab.item(cx) == new) {
            Some(made) => self.tabs.remove(made),
            None => self.new_tab(new, window, cx),
        };
        let index = index.min(self.tabs.len().saturating_sub(1));
        self.tabs[index] = tab;
        if let TabItem::Document(old) = old {
            self.visuals.remove(&old);
        }
        let active = self.app.read(cx).engine.active_item();
        if let Some(active) = self
            .tabs
            .iter()
            .position(|tab| Some(tab.item(cx)) == active)
        {
            self.active_tab = active;
        }
    }

    /// Keeps the page in the preview tab open while one of its editors holds what the engine
    /// has not seen yet: IME text being composed, or source text not yet sent.
    fn keep_busy_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let busy = {
            let engine = &self.app.read(cx).engine;
            let Some(id) = engine
                .tab_order()
                .iter()
                .copied()
                .find(|item| engine.is_preview(*item))
                .and_then(TabItem::document)
            else {
                return;
            };
            let composing = self
                .visuals
                .get(&id)
                .is_some_and(|visual| visual.read(cx).is_composing());
            let unsent = self.tabs.iter().filter_map(TabView::editor).any(|tab| {
                let tab = tab.read(cx);
                tab.document() == id
                    && engine
                        .document(id)
                        .is_some_and(|document| document.text != tab.widget_text(cx).as_str())
            });
            (composing || unsent).then_some(id)
        };
        if let Some(id) = busy {
            self.invoke(
                ids::DOCUMENT_KEEP_OPEN,
                CommandArgs::Document(id),
                window,
                cx,
            );
        }
    }

    /// The sidebar tree a palette or menu command opens and closes: the one shown, else
    /// Navigation.
    fn shown_panel(&self) -> SidebarPanel {
        match self.sidebar_tab {
            SidebarTab::Families => SidebarPanel::Modules,
            _ => SidebarPanel::Navigation,
        }
    }

    fn on_sidebar_command(
        &mut self,
        command: adoc_engine::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.shown_panel();
        self.invoke(command, CommandArgs::Panel(panel), window, cx);
    }

    fn on_expand_sidebar(
        &mut self,
        _: &ExpandSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_sidebar_command(ids::SIDEBAR_EXPAND_ALL, window, cx);
    }

    fn on_collapse_sidebar(
        &mut self,
        _: &CollapseSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_sidebar_command(ids::SIDEBAR_COLLAPSE_ALL, window, cx);
    }

    fn on_collapse_sidebar_level(
        &mut self,
        _: &CollapseSidebarLevel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_sidebar_command(ids::SIDEBAR_COLLAPSE_ONE_LEVEL, window, cx);
    }

    fn on_reset_sidebar(&mut self, _: &ResetSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.on_sidebar_command(ids::SIDEBAR_RESET_EXPANSION, window, cx);
    }

    /// Opens the shown panel's menu at the top of its tree, as a right-click would.
    fn on_sidebar_menu(&mut self, _: &SidebarMenu, window: &mut Window, cx: &mut Context<Self>) {
        let panel = match self.sidebar_tab {
            SidebarTab::Navigation => SidebarPanel::Navigation,
            SidebarTab::Families => SidebarPanel::Modules,
            _ => return,
        };
        let app = self.app.clone();
        let menu = gpui_component::menu::PopupMenu::build(window, cx, move |menu, _, cx| {
            expansion_menu(menu, panel, &app, cx)
        });
        let closed = cx.subscribe_in(&menu, window, |this, _, _: &gpui::DismissEvent, _, cx| {
            this.sidebar_menu = None;
            cx.notify();
        });
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.sidebar_menu = Some((panel, menu, closed));
        cx.notify();
    }

    /// Shows the active document or image in the Files panel, without moving focus from it.
    fn on_reveal_in_files(
        &mut self,
        _: &RevealInFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.active_item_path(cx) else {
            return;
        };
        if self.files.update(cx, |files, cx| files.reveal(&path, cx)) {
            self.sidebar_tab = SidebarTab::Files;
            cx.notify();
        } else {
            crate::files_view::outside_roots(&path, window, cx);
        }
    }

    fn on_keep_open(&mut self, _: &KeepOpen, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.active_item(cx) {
            self.invoke(ids::DOCUMENT_KEEP_OPEN, item_args(item), window, cx);
        }
    }

    fn on_reload_image(&mut self, _: &ReloadImage, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::IMAGE_RELOAD, CommandArgs::None, window, cx);
    }

    /// Hands a zoom command chosen from a menu or the palette to the active image's viewer,
    /// when focus is not in it to take it first.
    fn zoom_active_image(
        &mut self,
        action: Box<dyn gpui::Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(TabView::Image(image)) = self.tabs.get(self.active_tab).cloned() {
            let focus = image.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
        }
    }

    /// The documents of the tabs, in order: images are left out.
    #[cfg(test)]
    pub fn tab_documents(&self, cx: &App) -> Vec<DocumentId> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.item(cx).document())
            .collect()
    }

    /// What the tabs show, in order.
    #[cfg(test)]
    pub fn tab_items(&self, cx: &App) -> Vec<TabItem> {
        self.tabs.iter().map(|tab| tab.item(cx)).collect()
    }

    /// The viewer of the active tab, when it shows an image.
    #[cfg(test)]
    pub fn active_image_view(&self) -> Option<&Entity<ImageView>> {
        match self.tabs.get(self.active_tab) {
            Some(TabView::Image(image)) => Some(image),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    #[cfg(test)]
    pub fn preview(&self) -> &Entity<PreviewView> {
        &self.preview
    }

    fn active_editor(&self) -> Option<&Entity<SourceEditorView>> {
        self.tabs.get(self.active_tab).and_then(TabView::editor)
    }

    fn active_document(&self, cx: &App) -> Option<DocumentId> {
        self.active_editor()
            .map(|editor| editor.read(cx).document())
    }

    /// What the active tab shows.
    fn active_item(&self, cx: &App) -> Option<TabItem> {
        self.tabs.get(self.active_tab).map(|tab| tab.item(cx))
    }

    /// The file the active tab shows, a page's or an image's.
    fn active_item_path(&self, cx: &App) -> Option<std::path::PathBuf> {
        let engine = &self.app.read(cx).engine;
        match self.active_item(cx)? {
            TabItem::Document(id) => engine.document(id).map(|document| document.path.clone()),
            TabItem::Image(id) => engine.image(id).map(|image| image.path.clone()),
        }
    }

    /// A tab's file name, as its tab shows it.
    fn item_name(&self, item: TabItem, cx: &App) -> String {
        let engine = &self.app.read(cx).engine;
        let path = match item {
            TabItem::Document(id) => engine.document(id).map(|document| &document.path),
            TabItem::Image(id) => engine.image(id).map(|image| &image.path),
        };
        path.and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn document_name(&self, id: DocumentId, cx: &App) -> String {
        self.app
            .read(cx)
            .engine
            .document(id)
            .and_then(|document| document.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".to_owned())
    }

    // --- settings and theme ------------------------------------------------------------------

    fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.app.read(cx).engine.settings().editor.theme {
            ThemePreference::Auto => Theme::sync_system_appearance(Some(window), cx),
            ThemePreference::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            ThemePreference::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        }
    }

    fn warn_about_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let warnings = self.app.read(cx).engine.settings().warnings.clone();
        for warning in warnings {
            window.push_notification(warning, cx);
        }
    }

    fn reload_settings_if_edited(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        let is_settings = {
            let engine = &self.app.read(cx).engine;
            let path = engine.document(id).map(|document| document.path.clone());
            let settings = engine.settings();
            path.is_some_and(|path| {
                Some(path.as_path()) == settings.user_file()
                    || Some(path.as_path()) == settings.workspace_file()
            })
        };
        if is_settings {
            self.app.update(cx, |app, cx| {
                let _ = app.invoke(ids::SETTINGS_RELOAD, CommandArgs::None, cx);
            });
        }
    }

    // --- operations used by other views ------------------------------------------------------

    pub(crate) fn invoke(
        &mut self,
        command: adoc_engine::CommandId,
        args: CommandArgs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<CommandOutcome> {
        let outcome = self.app.update(cx, |app, cx| app.invoke(command, args, cx));
        match outcome {
            Ok(outcome) => Some(outcome),
            Err(adoc_engine::EngineError::SaveConflict { path }) => {
                self.offer_save_conflict(&path, window, cx);
                None
            }
            Err(error) => {
                window.push_notification(error.to_string(), cx);
                None
            }
        }
    }

    /// Asks what to do about a page that changed on disk since it was opened or saved: take the
    /// file's version (an undoable edit), or write the buffer's over it.
    fn offer_save_conflict(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.app.read(cx).engine.document_for_path(path) else {
            return;
        };
        let name = self.document_name(id, cx);
        let workspace = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let choice = |selector: &'static str, label: &'static str, command| {
                let workspace = workspace.clone();
                Button::new(selector)
                    .label(label)
                    .debug_selector(move || selector.to_owned())
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        if let Some(workspace) = workspace.upgrade() {
                            workspace.update(cx, |workspace, cx| {
                                workspace.invoke(command, CommandArgs::Document(id), window, cx);
                            });
                        }
                    })
            };
            dialog.title(format!("{name} changed on disk")).child(
                v_flex()
                    .gap_3()
                    .child(
                        "It changed since you opened or last saved it. Reload it to take \
                             the version on disk (Undo brings your edits back), or overwrite it \
                             with yours.",
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(choice(
                                "save-conflict-reload",
                                "Reload from Disk",
                                ids::DOCUMENT_RELOAD,
                            ))
                            .child(
                                choice(
                                    "save-conflict-overwrite",
                                    "Overwrite",
                                    ids::DOCUMENT_OVERWRITE,
                                )
                                .danger(),
                            ),
                    ),
            )
        });
    }

    fn open_folder(
        &mut self,
        folder: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(
            ids::WORKSPACE_OPEN_FOLDER,
            CommandArgs::Path(folder),
            window,
            cx,
        );
    }

    /// Opens `path` as `disposition` says and selects `range` in it: a page in a source tab, an
    /// image in an image tab, as the engine decides. Browsing shows it in the preview tab.
    pub fn open_at(
        &mut self,
        path: &Path,
        range: SourceRange,
        disposition: OpenDisposition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TabItem> {
        let command = match disposition {
            OpenDisposition::Persistent => ids::DOCUMENT_OPEN,
            OpenDisposition::Preview => {
                self.keep_busy_preview(window, cx);
                ids::DOCUMENT_BROWSE
            }
        };
        let item = match self.invoke(command, CommandArgs::Path(path.to_path_buf()), window, cx)? {
            CommandOutcome::Opened(id) => TabItem::Document(id),
            CommandOutcome::OpenedImage(id) => TabItem::Image(id),
            _ => return None,
        };
        self.app.update(cx, |app, cx| {
            app.engine.activate(item);
            app.drain(cx);
        });
        self.follow_active(window, cx);
        let TabItem::Document(id) = item else {
            return Some(item);
        };
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.reveal(range, window, cx));
        }
        if let Some(visual) = self.visuals.get(&id).cloned() {
            visual.update(cx, |visual, cx| visual.go_to(range.start, cx));
            self.focus_active_editor(window, cx);
        }
        Some(item)
    }

    /// Shows a multi-file change for confirmation; applying it is one undo step.
    pub fn show_preview(
        &mut self,
        preview: EditPreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = self.app.clone();
        let roots = app.read(cx).engine.roots().to_vec();
        let id = preview.id;
        let applied = Rc::new(Cell::new(false));
        let mut lines: Vec<SharedString> = Vec::new();
        for file_move in &preview.moves {
            lines.push(
                format!(
                    "Move {} → {}",
                    sidebar::display_path(&file_move.from, &roots),
                    sidebar::display_path(&file_move.to, &roots)
                )
                .into(),
            );
        }
        for file in &preview.edits {
            // The text the edits were computed against, which for an open page is its buffer,
            // not the file on disk.
            let text = &file.base;
            let index = LineIndex::new(text);
            for edit in &file.edits {
                let (line, _) = index.line_column(edit.range.start);
                let old = text.get(edit.range.start..edit.range.end).unwrap_or("");
                lines.push(
                    format!(
                        "{}:{}  {old} → {}",
                        sidebar::display_path(&file.path, &roots),
                        line + 1,
                        edit.replacement
                    )
                    .into(),
                );
            }
        }
        let title: SharedString = preview.title.into();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let app_ok = app.clone();
            let app_close = app.clone();
            let applied_ok = applied.clone();
            let applied_close = applied.clone();
            dialog
                .title(title.clone())
                .width(px(720.))
                .child(
                    v_flex()
                        .id("preview-lines")
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .gap_1()
                        .text_sm()
                        .font_family(cx.theme().mono_font_family.clone())
                        .children(lines.iter().cloned())
                        .when(lines.is_empty(), |list| list.child("Nothing to change.")),
                )
                .confirm()
                .ok_text("Apply")
                .on_ok(move |_, window, cx| {
                    let result = app_ok.update(cx, |app, cx| {
                        app.invoke(
                            ids::WORKSPACE_APPLY_PREVIEW,
                            CommandArgs::Custom(serde_json::json!({ "preview_id": id.get() })),
                            cx,
                        )
                    });
                    applied_ok.set(true);
                    if let Err(error) = result {
                        window.push_notification(error.to_string(), cx);
                    }
                    true
                })
                .on_close(move |_, _, cx| {
                    if !applied_close.get() {
                        app_close.update(cx, |app, _| app.engine.discard_preview(id));
                    }
                })
        });
    }

    // --- actions -----------------------------------------------------------------------------

    fn on_open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(folder) = paths.await.ok()?.ok()??.into_iter().next() else {
                return Some(());
            };
            this.update_in(cx, |this, window, cx| this.open_folder(folder, window, cx))
                .ok()
        })
        .detach();
    }

    fn on_open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let files = paths.await.ok()?.ok()??;
            this.update_in(cx, |this, window, cx| {
                for file in files {
                    this.open_at(
                        &file,
                        SourceRange::default(),
                        OpenDisposition::Persistent,
                        window,
                        cx,
                    );
                }
            })
            .ok()
        })
        .detach();
    }

    fn on_save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_document(cx) {
            self.invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id), window, cx);
        }
    }

    fn on_save_all(&mut self, _: &SaveAll, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::DOCUMENT_SAVE_ALL, CommandArgs::None, window, cx);
    }

    fn on_reload(&mut self, _: &Reload, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_document(cx) {
            self.invoke(ids::DOCUMENT_RELOAD, CommandArgs::Document(id), window, cx);
        }
    }

    fn on_overwrite(&mut self, _: &Overwrite, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_document(cx) {
            self.invoke(
                ids::DOCUMENT_OVERWRITE,
                CommandArgs::Document(id),
                window,
                cx,
            );
        }
    }

    /// Runs `command` (Close, or Close Others, Left, Right, Clean or All) on the tab of `item`,
    /// first asking once whether to discard the unsaved changes of any tab it would close.
    fn close_tabs(
        &mut self,
        command: adoc_engine::CommandId,
        item: TabItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let names: Vec<String> = engine
            .closed_by(command, item)
            .into_iter()
            .filter_map(TabItem::document)
            .filter(|closed| {
                engine
                    .document(*closed)
                    .is_some_and(|document| document.dirty)
            })
            .map(|closed| self.document_name(closed, cx))
            .collect();
        if names.is_empty() {
            self.invoke(command, item_args(item), window, cx);
            return;
        }
        let app = self.app.clone();
        let message = match &names[..] {
            [name] => format!("{name} has unsaved changes."),
            [rest @ .., last] => format!("{} and {last} have unsaved changes.", rest.join(", ")),
            [] => unreachable!(),
        };
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let app = app.clone();
            dialog
                .title("Discard unsaved changes?")
                .child(message.clone())
                .confirm()
                .ok_text("Discard")
                .on_ok(move |_, _, cx| {
                    app.update(cx, |app, cx| {
                        let _ = app.invoke(command, item_args(item), cx);
                    });
                    true
                })
        });
    }

    fn close_active(
        &mut self,
        command: adoc_engine::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = self.active_item(cx) {
            self.close_tabs(command, item, window, cx);
        }
    }

    fn on_close(&mut self, _: &CloseDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(ids::DOCUMENT_CLOSE, window, cx);
    }

    fn on_close_others(&mut self, _: &CloseOthers, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(ids::DOCUMENT_CLOSE_OTHERS, window, cx);
    }

    fn on_close_left(&mut self, _: &CloseLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(ids::DOCUMENT_CLOSE_LEFT, window, cx);
    }

    fn on_close_right(&mut self, _: &CloseRight, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(ids::DOCUMENT_CLOSE_RIGHT, window, cx);
    }

    fn on_close_clean(&mut self, _: &CloseClean, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(ids::DOCUMENT_CLOSE_CLEAN, window, cx);
    }

    fn on_close_all(&mut self, _: &CloseAll, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(ids::DOCUMENT_CLOSE_ALL, window, cx);
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.undo(false, cx));
        }
    }

    /// Runs a mark or insert command on the source editor's selection. The visual editor
    /// handles the same actions itself while it has focus, so this is reached from the source
    /// editor or the palette.
    fn source_selection_command(
        &mut self,
        command: adoc_engine::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        let document = editor.read(cx).document();
        if self.app.read(cx).engine.editor_mode(document) == EditorMode::Visual {
            return;
        }
        editor.update(cx, |editor, cx| {
            editor.run_on_selection(command, window, cx)
        });
    }

    fn on_toggle_strong(&mut self, _: &ToggleStrong, window: &mut Window, cx: &mut Context<Self>) {
        self.source_selection_command(ids::EDIT_TOGGLE_STRONG, window, cx);
    }

    fn on_toggle_emphasis(
        &mut self,
        _: &ToggleEmphasis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source_selection_command(ids::EDIT_TOGGLE_EMPHASIS, window, cx);
    }

    fn on_toggle_code(&mut self, _: &ToggleCode, window: &mut Window, cx: &mut Context<Self>) {
        self.source_selection_command(ids::EDIT_TOGGLE_CODE, window, cx);
    }

    fn on_toggle_highlight(
        &mut self,
        _: &ToggleHighlight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source_selection_command(ids::EDIT_TOGGLE_HIGHLIGHT, window, cx);
    }

    fn on_insert_table(&mut self, _: &InsertTable, window: &mut Window, cx: &mut Context<Self>) {
        self.source_selection_command(ids::EDIT_INSERT_TABLE, window, cx);
    }

    fn on_insert_code_block(
        &mut self,
        _: &InsertCodeBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source_selection_command(ids::EDIT_INSERT_CODE_BLOCK, window, cx);
    }

    fn on_insert_admonition(
        &mut self,
        _: &InsertAdmonition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source_selection_command(ids::EDIT_INSERT_ADMONITION, window, cx);
    }

    fn on_insert_link(&mut self, _: &InsertLink, window: &mut Window, cx: &mut Context<Self>) {
        self.source_selection_command(ids::EDIT_INSERT_LINK, window, cx);
    }

    fn on_insert_image(&mut self, _: &InsertImage, window: &mut Window, cx: &mut Context<Self>) {
        self.source_selection_command(ids::EDIT_INSERT_IMAGE, window, cx);
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.undo(true, cx));
        }
    }

    fn on_follow_link(&mut self, _: &FollowLink, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.follow_link_at_caret(window, cx));
        }
    }

    fn on_go_to_definition(
        &mut self,
        _: &GoToDefinition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_follow_link(&FollowLink, window, cx);
    }

    fn on_go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::NAVIGATE_BACK, CommandArgs::None, window, cx);
    }

    fn on_go_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::NAVIGATE_FORWARD, CommandArgs::None, window, cx);
    }

    fn on_peek_include(&mut self, _: &PeekInclude, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        let (id, offset) = {
            let editor = editor.read(cx);
            (editor.document(), editor.cursor(cx))
        };
        let Some(CommandOutcome::Location(location)) = self.invoke(
            ids::NAVIGATE_PEEK_INCLUDE,
            CommandArgs::Offset { id, offset },
            window,
            cx,
        ) else {
            return;
        };
        let text = {
            let engine = &self.app.read(cx).engine;
            engine
                .document_for_path(&location.path)
                .and_then(|id| engine.document(id))
                .map(|document| document.text.to_string())
                .or_else(|| std::fs::read_to_string(&location.path).ok())
                .unwrap_or_default()
        };
        let peek = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language::ASCIIDOC)
                .line_number(true)
                .default_value(text);
            state.set_readonly(true, cx);
            state
        });
        let roots = self.app.read(cx).engine.roots().to_vec();
        let title: SharedString = sidebar::display_path(&location.path, &roots).into();
        let path = location.path.clone();
        let workspace = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let path = path.clone();
            let workspace = workspace.clone();
            dialog
                .title(title.clone())
                .width(px(760.))
                .child(div().h(px(420.)).child(Editor::new(&peek).h(px(420.))))
                .confirm()
                .ok_text("Open")
                .on_ok(move |_, window, cx| {
                    if let Some(workspace) = workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_at(
                                &path,
                                SourceRange::default(),
                                OpenDisposition::Preview,
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
        });
    }

    fn on_find(&mut self, _: &FindInWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search = true;
        self.search
            .update(cx, |search, cx| search.focus(window, cx));
        cx.notify();
    }

    fn on_replace(&mut self, _: &ReplaceInWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        self.on_find(&FindInWorkspace, window, cx);
    }

    fn on_rename(&mut self, _: &RenamePage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_document(cx) else {
            return;
        };
        let Some(path) = self
            .app
            .read(cx)
            .engine
            .document(id)
            .map(|document| document.path.clone())
        else {
            return;
        };
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(path.display().to_string()));
        let workspace = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let input = input.clone();
            let workspace = workspace.clone();
            dialog
                .title("Rename Page")
                .width(px(640.))
                .child(
                    v_flex()
                        .gap_2()
                        .child("New path; every xref to the page is rewritten.")
                        .child(Input::new(&input)),
                )
                .confirm()
                .ok_text("Preview")
                .on_ok(move |_, window, cx| {
                    let new_path = std::path::PathBuf::from(input.read(cx).value().to_string());
                    let Some(workspace) = workspace.upgrade() else {
                        return true;
                    };
                    workspace.update(cx, |workspace, cx| {
                        if let Some(CommandOutcome::Preview(preview)) = workspace.invoke(
                            ids::PAGE_RENAME,
                            CommandArgs::Rename { id, new_path },
                            window,
                            cx,
                        ) {
                            let preview = preview.clone();
                            let handle = cx.weak_entity();
                            window.defer(cx, move |window, cx| {
                                if let Some(workspace) = handle.upgrade() {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.show_preview(preview, window, cx);
                                    });
                                }
                            });
                        }
                    });
                    true
                })
        });
    }

    fn on_reindex(&mut self, _: &Reindex, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::WORKSPACE_REINDEX, CommandArgs::None, window, cx);
    }

    fn on_refresh_git(&mut self, _: &RefreshGit, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::GIT_REFRESH, CommandArgs::None, window, cx);
    }

    /// The active document and the source caret, for commands that act at the caret.
    fn caret(&self, cx: &App) -> Option<(DocumentId, usize)> {
        let editor = self.active_editor()?.read(cx);
        Some((editor.document(), editor.cursor(cx)))
    }

    fn on_revert_change(&mut self, _: &RevertChange, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, offset)) = self.caret(cx) else {
            return;
        };
        self.invoke(
            ids::GIT_REVERT_HUNK,
            CommandArgs::Offset { id, offset },
            window,
            cx,
        );
    }

    fn on_format_table(&mut self, _: &FormatTable, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, offset)) = self.caret(cx) else {
            return;
        };
        self.invoke(
            ids::EDIT_FORMAT_TABLE,
            CommandArgs::Offset { id, offset },
            window,
            cx,
        );
    }

    fn on_next_change(&mut self, _: &NextChange, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to_change(ids::GIT_NEXT_CHANGE, window, cx);
    }

    fn on_previous_change(
        &mut self,
        _: &PreviousChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_to_change(ids::GIT_PREVIOUS_CHANGE, window, cx);
    }

    fn go_to_change(
        &mut self,
        command: adoc_engine::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, offset)) = self.caret(cx) else {
            return;
        };
        if let Some(CommandOutcome::Location(location)) =
            self.invoke(command, CommandArgs::Offset { id, offset }, window, cx)
        {
            self.open_at(
                &location.path,
                location.range,
                OpenDisposition::Persistent,
                window,
                cx,
            );
        }
    }

    fn on_toggle_blame(&mut self, _: &ToggleBlame, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.toggle_blame(window, cx));
        }
    }

    fn on_open_on_remote(&mut self, _: &OpenOnRemote, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(url) = self.remote_link(ids::NAVIGATE_OPEN_ON_REMOTE, window, cx) {
            cx.open_url(&url);
        }
    }

    fn on_copy_remote_link(
        &mut self,
        _: &CopyRemoteLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(url) = self.remote_link(ids::NAVIGATE_COPY_REMOTE_LINK, window, cx) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
            window.push_notification(format!("Copied {url}"), cx);
        }
    }

    /// The address of the caret's line on the hosting site, from `command`.
    fn remote_link(
        &mut self,
        command: adoc_engine::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let (id, offset) = self.caret(cx)?;
        match self.invoke(command, CommandArgs::Offset { id, offset }, window, cx)? {
            CommandOutcome::Url(url) => Some(url),
            _ => None,
        }
    }

    /// The active document's path, for commands that act on the file.
    fn active_path(&self, cx: &App) -> Option<std::path::PathBuf> {
        let engine = &self.app.read(cx).engine;
        engine
            .active()
            .and_then(|id| engine.document(id))
            .map(|document| document.path.clone())
    }

    fn on_stage_file(&mut self, _: &StageFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.active_path(cx) {
            self.invoke(ids::GIT_STAGE, CommandArgs::Path(path), window, cx);
        }
    }

    fn on_unstage_file(&mut self, _: &UnstageFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.active_path(cx) {
            self.invoke(ids::GIT_UNSTAGE, CommandArgs::Path(path), window, cx);
        }
    }

    fn on_stage_all(&mut self, _: &StageAll, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::GIT_STAGE_ALL_ADOC, CommandArgs::None, window, cx);
    }

    fn on_commit_changes(
        &mut self,
        _: &CommitChanges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_commit_dialog(String::new(), Vec::new(), window, cx);
    }

    /// Asks where to write the active page's PDF, then exports it on the engine's workers.
    fn on_export_page_pdf(
        &mut self,
        _: &ExportPagePdf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_page(ids::EXPORT_PDF_PAGE, "pdf", None, window, cx);
    }

    /// Asks where to write the active page as HTML, then exports it on the engine's workers.
    fn on_export_page_html(
        &mut self,
        _: &ExportPageHtml,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_page(ids::EXPORT_HTML_PAGE, "html", None, window, cx);
    }

    fn export_page(
        &mut self,
        command: adoc_engine::CommandId,
        extension: &str,
        theme: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let Some(id) = engine.active() else {
            window.push_notification("Open a page to export it.", cx);
            return;
        };
        let Some(path) = engine.document(id).map(|document| document.path.clone()) else {
            return;
        };
        let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = path
            .file_stem()
            .map(|stem| format!("{}.{extension}", stem.to_string_lossy()));
        self.export_to(
            command,
            export_args(ExportTarget::Page(id), theme),
            &directory,
            name,
            window,
            cx,
        );
    }

    /// Offers the bundled and workspace themes, then asks where to write the active page's PDF
    /// in the one chosen.
    fn on_export_page_pdf_with_theme(
        &mut self,
        _: &ExportPagePdfWithTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        if engine.active().is_none() {
            window.push_notification("Open a page to export it.", cx);
            return;
        }
        let themes = engine.pdf_themes();
        let labels = themes
            .iter()
            .map(|theme| match &theme.file {
                Some(file) => format!("{} — {}", theme.name, file.display()),
                None => format!("{} (bundled)", theme.name),
            })
            .collect();
        self.pick(
            "Choose a PDF theme…",
            labels,
            window,
            cx,
            move |this, index, window, cx| {
                let theme = themes[index].name.clone();
                this.export_page(ids::EXPORT_PDF_PAGE, "pdf", Some(theme), window, cx);
            },
        );
    }

    /// Offers the workspace's publication profiles, then exports the one chosen to its output.
    fn on_export_pdf_profile(
        &mut self,
        _: &ExportPdfProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profiles: Vec<String> = self
            .app
            .read(cx)
            .engine
            .settings()
            .pdf
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect();
        if profiles.is_empty() {
            window.push_notification(
                "No PDF profiles: add a [pdf.profiles.<name>] table to the workspace's .adocfoundry.toml.",
                cx,
            );
            return;
        }
        self.pick(
            "Export a PDF profile…",
            profiles.clone(),
            window,
            cx,
            move |this, index, window, cx| {
                this.invoke(
                    ids::EXPORT_PDF_PROFILE,
                    CommandArgs::Profile(profiles[index].clone()),
                    window,
                    cx,
                );
                cx.notify();
            },
        );
    }

    /// Shows `labels` in the palette's list and calls `chosen` with the index picked, once the
    /// list has closed and handed focus back.
    fn pick(
        &mut self,
        placeholder: &'static str,
        labels: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        chosen: impl Fn(&mut Self, usize, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let items: Vec<CommandItem> = labels
            .into_iter()
            .map(|label| CommandItem::new().label(label))
            .collect();
        let workspace = cx.weak_entity();
        let chosen = std::rc::Rc::new(chosen);
        let palette = self.palette.clone();
        palette.update(cx, |palette, cx| palette.set_query("", window, cx));
        let focus = palette.read(cx).focus_handle(cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            window.focus(&focus, cx);
            let workspace = workspace.clone();
            let chosen = chosen.clone();
            dialog.close_button(false).p_0().child(
                Command::new(&palette)
                    .bordered(false)
                    .placeholder(placeholder)
                    .max_h(px(420.))
                    .items(items.clone())
                    .on_confirm(move |index, window, cx| {
                        let index = index.row;
                        let workspace = workspace.clone();
                        let chosen = chosen.clone();
                        window.close_dialog(cx);
                        window.defer(cx, move |window, cx| {
                            let _ =
                                workspace.update(cx, |this, cx| chosen(this, index, window, cx));
                        });
                    }),
            )
        });
    }

    /// Asks where to write the active page's component as a book, then exports it.
    fn on_export_component_pdf(
        &mut self,
        _: &ExportComponentPdf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let antora = &engine.workspace().antora;
        let active = engine
            .active()
            .and_then(|id| engine.document(id))
            .and_then(|document| antora.context_for_path(&document.path))
            .map(|context| (context.component, context.version));
        let Some((name, version)) = active.or_else(|| {
            antora
                .components()
                .next()
                .map(|component| (component.name.clone(), component.version.clone()))
        }) else {
            window.push_notification("The workspace has no Antora component to export.", cx);
            return;
        };
        let directory = engine.roots().first().cloned().unwrap_or_default();
        let suggested = Some(format!("{name}.pdf"));
        self.export_to(
            ids::EXPORT_PDF_COMPONENT,
            export_args(ExportTarget::Component { name, version }, None),
            &directory,
            suggested,
            window,
            cx,
        );
    }

    /// Asks where to write `export`, starting in `directory` with `suggested`, then runs it
    /// with the path chosen as its output.
    fn export_to(
        &mut self,
        command: adoc_engine::CommandId,
        export: ExportArgs,
        directory: &Path,
        suggested: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chosen = cx.prompt_for_new_path(directory, suggested.as_deref());
        cx.spawn_in(window, async move |this, cx| {
            let output = chosen.await.ok()?.ok()??;
            this.update_in(cx, |this, window, cx| {
                this.invoke(
                    command,
                    CommandArgs::Export(ExportArgs { output, ..export }),
                    window,
                    cx,
                );
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Tells the user how an export went, with a button to open the file.
    fn export_finished(
        output: &Path,
        result: &Result<ExportSummary, String>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let message = export_message(output, result);
        match result {
            Ok(_) => {
                let path = output.to_path_buf();
                window.push_notification(
                    Notification::new().message(message).action(move |_, _, _| {
                        let path = path.clone();
                        Button::new("open-pdf")
                            .label("Open")
                            .on_click(move |_, _, cx| cx.open_with_system(&path))
                    }),
                    cx,
                );
            }
            Err(_) => window.push_notification(message, cx),
        }
    }

    fn on_push(&mut self, _: &Push, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::GIT_PUSH, CommandArgs::None, window, cx);
    }

    fn on_save_and_commit(
        &mut self,
        _: &SaveAndCommit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.app.read(cx).engine.active() else {
            return;
        };
        if let Some(CommandOutcome::CommitDraft { message, paths }) = self.invoke(
            ids::GIT_SAVE_AND_COMMIT,
            CommandArgs::Document(id),
            window,
            cx,
        ) {
            self.open_commit_dialog(message, paths, window, cx);
        }
    }

    fn on_show_changes(&mut self, _: &ShowChanges, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_changes_panel(cx) {
            window.push_notification("No workspace folder is in a Git repository.", cx);
            return;
        }
        self.invoke(ids::GIT_SHOW_CHANGES, CommandArgs::None, window, cx);
        self.sidebar_tab = SidebarTab::Changes;
        cx.notify();
    }

    fn on_check_for_updates(
        &mut self,
        _: &CheckForUpdates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(CommandOutcome::CheckForUpdates(feed)) =
            self.invoke(ids::APP_CHECK_FOR_UPDATES, CommandArgs::None, window, cx)
        {
            crate::updates::check(&self.app, &feed, true, window, cx);
        }
    }

    fn on_recover_unsaved(
        &mut self,
        _: &RecoverUnsaved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(CommandOutcome::Recoverable(items)) =
            self.invoke(ids::DOCUMENT_RECOVER_UNSAVED, CommandArgs::None, window, cx)
        else {
            return;
        };
        if items.is_empty() {
            window.push_notification("There are no unsaved changes to recover.", cx);
            return;
        }
        let workspace = cx.weak_entity();
        crate::history_view::show_recovery(self.app.clone(), workspace, items, window, cx);
    }

    fn on_local_history(&mut self, _: &LocalHistory, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_document(cx) else {
            return;
        };
        let Some(CommandOutcome::LocalHistory { document, versions }) = self.invoke(
            ids::DOCUMENT_LOCAL_HISTORY,
            CommandArgs::Document(id),
            window,
            cx,
        ) else {
            return;
        };
        if versions.is_empty() {
            window.push_notification(
                "This page has no saved versions yet: each save keeps one.",
                cx,
            );
            return;
        }
        let workspace = cx.weak_entity();
        crate::history_view::show_history(
            self.app.clone(),
            workspace,
            document,
            versions,
            window,
            cx,
        );
    }

    fn on_show_diagnostics(
        &mut self,
        _: &ShowDiagnostics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(CommandOutcome::Diagnostics(doctor)) =
            self.invoke(ids::APP_DIAGNOSTICS, CommandArgs::None, window, cx)
        {
            let font = self
                .app
                .read(cx)
                .engine
                .settings()
                .editor
                .font_family
                .clone();
            crate::diagnostics::show(*doctor, &font, window, cx);
        }
    }

    fn on_open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(CommandOutcome::External(path)) =
            self.invoke(ids::SETTINGS_OPEN, CommandArgs::None, window, cx)
        {
            self.open_at(
                &path,
                SourceRange::default(),
                OpenDisposition::Persistent,
                window,
                cx,
            );
        }
    }

    fn on_reload_settings(
        &mut self,
        _: &ReloadSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(ids::SETTINGS_RELOAD, CommandArgs::None, window, cx);
    }

    fn on_toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
        let mode = if cx.theme().mode.is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
        cx.notify();
    }

    fn on_open_mirror(&mut self, _: &OpenMirrorWindow, _: &mut Window, cx: &mut Context<Self>) {
        open_secondary_window(self.app.clone(), WindowKind::Mirror, cx);
    }

    fn on_toggle_preview(
        &mut self,
        _: &TogglePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None, window, cx);
    }

    fn on_detach_preview(
        &mut self,
        _: &DetachPreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(ids::VIEW_DETACH_PREVIEW, CommandArgs::None, window, cx);
    }

    fn on_toggle_preview_sync(
        &mut self,
        _: &TogglePreviewScrollSync,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(ids::VIEW_TOGGLE_SCROLL_SYNC, CommandArgs::None, window, cx);
    }

    fn on_toggle_pdf_preview(
        &mut self,
        _: &TogglePdfPreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None, window, cx);
    }

    fn on_pin_preview(&mut self, _: &PinPreview, window: &mut Window, cx: &mut Context<Self>) {
        self.invoke(ids::PREVIEW_PIN, CommandArgs::None, window, cx);
    }

    fn show_mode(
        &mut self,
        command: adoc_engine::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.active_document(cx) {
            self.invoke(command, CommandArgs::Document(id), window, cx);
        }
    }

    fn on_show_source(&mut self, _: &ShowSource, window: &mut Window, cx: &mut Context<Self>) {
        self.show_mode(ids::VIEW_SHOW_SOURCE, window, cx);
    }

    fn on_show_visual(&mut self, _: &ShowVisual, window: &mut Window, cx: &mut Context<Self>) {
        self.show_mode(ids::VIEW_SHOW_VISUAL, window, cx);
    }

    fn on_show_split(&mut self, _: &ShowSplit, window: &mut Window, cx: &mut Context<Self>) {
        self.show_mode(ids::VIEW_SHOW_SPLIT, window, cx);
    }

    /// The active document's editor as its mode shows it: source, visual, or both.
    fn editor_area(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if let Some(TabView::Image(image)) = self.tabs.get(self.active_tab) {
            return Some(image.clone().into_any_element());
        }
        let editor = self.active_editor().cloned()?;
        let id = editor.read(cx).document();
        let mode = self.app.read(cx).engine.editor_mode(id);
        let visual = self.visuals.get(&id).cloned();
        Some(match (mode, visual) {
            (EditorMode::Visual, Some(visual)) => visual.into_any_element(),
            (EditorMode::Split, Some(visual)) => h_resizable("source-visual")
                .child(resizable_panel().child(editor))
                .child(resizable_panel().child(visual))
                .into_any_element(),
            _ => editor.into_any_element(),
        })
    }

    /// Source, Visual and Split buttons for the active document.
    fn render_modes(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Only an AsciiDoc page has a visual editor; an image has none of them.
        let image = matches!(self.active_item(cx), Some(TabItem::Image(_)));
        let page = !image
            && self.active_document(cx).is_none_or(|id| {
                self.app
                    .read(cx)
                    .engine
                    .document(id)
                    .is_some_and(|document| document.asciidoc)
            });
        if !page {
            return div().into_any_element();
        }
        let mode = self
            .active_document(cx)
            .map(|id| self.app.read(cx).engine.editor_mode(id))
            .unwrap_or_default();
        let button =
            |id: &'static str, label: &'static str, shown: EditorMode, cx: &mut Context<Self>| {
                let command = match shown {
                    EditorMode::Source => ids::VIEW_SHOW_SOURCE,
                    EditorMode::Visual => ids::VIEW_SHOW_VISUAL,
                    EditorMode::Split => ids::VIEW_SHOW_SPLIT,
                };
                let button = Button::new(id).xsmall().label(label).on_click(
                    cx.listener(move |this, _, window, cx| this.show_mode(command, window, cx)),
                );
                if mode == shown {
                    button.primary()
                } else {
                    button.ghost()
                }
            };
        h_flex()
            .debug_selector(|| "editor-modes".to_owned())
            .gap_1()
            .px_2()
            .child(button("mode-source", "Source", EditorMode::Source, cx))
            .child(button("mode-visual", "Visual", EditorMode::Visual, cx))
            .child(button("mode-split", "Split", EditorMode::Split, cx))
            .into_any_element()
    }

    fn on_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        // Every item's action runs once the palette has closed and handed focus back, from
        // `on_confirm`. Dispatched from the palette's own field, inside the dialog layer that
        // `Root` draws beside this view, it would reach neither the workspace nor an editor.
        let mut actions: Vec<Box<dyn gpui::Action>> = Vec::new();
        let mut items: Vec<CommandItem> = palette_commands(&self.app.read(cx).engine)
            .into_iter()
            .filter_map(|spec| {
                actions.push(action_for(spec.id)?);
                Some(
                    CommandItem::new()
                        .label(spec.title)
                        .keywords([spec.id.as_str()]),
                )
            })
            .collect();
        let image = self.app.read(cx).engine.active_image().is_some();
        items.extend(
            UI_ACTIONS
                .iter()
                .enumerate()
                // The zoom commands only while an image is shown.
                .filter(|(index, _)| image || *index < IMAGE_UI_ACTIONS)
                .map(|(index, (title, _))| {
                    actions.push(ui_action(index));
                    CommandItem::new().label(*title)
                }),
        );
        let palette = self.palette.clone();
        palette.update(cx, |palette, cx| palette.set_query("", window, cx));
        let focus = palette.read(cx).focus_handle(cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            window.focus(&focus, cx);
            dialog.close_button(false).p_0().child(
                Command::new(&palette)
                    .bordered(false)
                    .placeholder("Run a command…")
                    .max_h(px(420.))
                    .items(items.clone())
                    .on_confirm({
                        let actions: Vec<Box<dyn gpui::Action>> =
                            actions.iter().map(|action| action.boxed_clone()).collect();
                        move |index, window, cx| {
                            let Some(action) = actions.get(index.row) else {
                                return;
                            };
                            let action = action.boxed_clone();
                            window.close_dialog(cx);
                            window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
                        }
                    }),
            )
        });
    }

    // --- rendering ---------------------------------------------------------------------------

    fn render_tree(
        &self,
        state: &Entity<TreeState>,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workspace = cx.weak_entity();
        let panel_space = div().id(id).size_full().child(
            tree(state, move |index, entry, _selected, _window, cx| {
                let item = entry.item().clone();
                let icon = if entry.is_folder() {
                    if entry.is_expanded() {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    }
                } else {
                    IconName::File
                };
                let workspace = workspace.clone();
                // Pages and the Outline's sections drag into an editor as xrefs.
                let drag = matches!(id, "nav" | "families" | "outline")
                    .then(|| sidebar::parse_target(&item.id))
                    .flatten()
                    .map(|target| sidebar::DraggedLink {
                        section: (id == "outline").then_some(target.range),
                        path: target.path,
                        label: item.label.clone(),
                    });
                ListItem::new(index)
                    .w_full()
                    .rounded(cx.theme().radius)
                    .py_0p5()
                    .px_2()
                    .pl(px(14.) * entry.depth() + px(8.))
                    .child(
                        h_flex()
                            .gap_2()
                            .w_full()
                            .min_w_0()
                            .text_sm()
                            .child(icon)
                            .child(tree_label(id, index, item.label.clone())),
                    )
                    .when_some(drag, |row, link| {
                        row.on_drag(link, |link, _, _, cx| cx.new(|_| link.clone()))
                    })
                    .on_click(move |event, window, cx| {
                        let Some(target) = sidebar::parse_target(&item.id) else {
                            return;
                        };
                        // A double-click keeps the page open; the outline only moves the caret.
                        let keep = event.click_count() > 1 && id != "outline";
                        if let Some(workspace) = workspace.upgrade() {
                            workspace.update(cx, |workspace, cx| {
                                workspace.open_at(
                                    &target.path,
                                    target.range,
                                    OpenDisposition::Preview,
                                    window,
                                    cx,
                                );
                                if keep {
                                    workspace.on_keep_open(&KeepOpen, window, cx);
                                }
                            });
                        }
                    })
            })
            .when_some(expansion_panel(id), |tree, panel| {
                let app = self.app.clone();
                tree.context_menu(move |_, _, menu, _, cx| expansion_menu(menu, panel, &app, cx))
            })
            .text_sm(),
        );
        // The panel's space below its rows offers the same menu.
        match expansion_panel(id) {
            Some(panel) => {
                let app = self.app.clone();
                let keyboard_menu = self
                    .sidebar_menu
                    .as_ref()
                    .filter(|(open, _, _)| *open == panel)
                    .map(|(_, menu, _)| menu.clone());
                panel_space
                    .relative()
                    .when_some(keyboard_menu, |panel_space, menu| {
                        panel_space.child(gpui::deferred(
                            gpui::anchored()
                                .snap_to_window_with_margin(px(8.))
                                .child(div().occlude().child(menu)),
                        ))
                    })
                    .context_menu(move |menu, _, cx| expansion_menu(menu, panel, &app, cx))
                    .into_any_element()
            }
            None => panel_space.into_any_element(),
        }
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_problems(cx);
        let git = self.has_changes_panel(cx);
        if !git && self.sidebar_tab == SidebarTab::Changes {
            self.sidebar_tab = SidebarTab::Navigation;
        }
        let mut panels = vec![
            (SidebarTab::Navigation, "Navigation".to_owned()),
            (SidebarTab::Families, "Modules".to_owned()),
            (SidebarTab::Files, "Files".to_owned()),
            (
                SidebarTab::Problems,
                format!("Problems ({})", self.problem_count),
            ),
        ];
        if git {
            panels.push((SidebarTab::Changes, "Changes".to_owned()));
        }
        let selected = panels
            .iter()
            .position(|(tab, _)| *tab == self.sidebar_tab)
            .unwrap_or(0);
        let content = match self.sidebar_tab {
            SidebarTab::Navigation => {
                let header = self.version_header(cx);
                v_flex()
                    .size_full()
                    .children(header)
                    .child(div().flex_1().min_h_0().child(self.render_tree(
                        &self.navigation.clone(),
                        "nav",
                        cx,
                    )))
                    .into_any_element()
            }
            SidebarTab::Families => self
                .render_tree(&self.families.clone(), "families", cx)
                .into_any_element(),
            SidebarTab::Files => self.files.clone().into_any_element(),
            SidebarTab::Problems => self
                .render_tree(&self.problems.clone(), "problems", cx)
                .into_any_element(),
            SidebarTab::Changes => self.render_changes(cx),
        };
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                TabBar::new("sidebar-tabs")
                    .underline()
                    .small()
                    .selected_index(selected)
                    .children(
                        panels
                            .iter()
                            .map(|(_, label)| Tab::new().label(label.clone())),
                    )
                    .on_click(cx.listener(move |this, index: &usize, _, cx| {
                        if let Some((tab, _)) = panels.get(*index) {
                            this.sidebar_tab = *tab;
                        }
                        cx.notify();
                    })),
            )
            .child(div().flex_1().min_h_0().child(content))
    }

    /// Scrolls the tab strip to the active tab over the next two frames. GPUI places a tracked
    /// item from the previous frame's layout, which a change of tabs or widths has just made
    /// stale, so the request is made again once a frame has been laid out after the change.
    fn reveal_active_tab(&self, cx: &mut Context<Self>) {
        self.tab_reveals.set(2);
        cx.notify();
    }

    /// Whether the active tab lies within the tab strip's visible width.
    #[cfg(test)]
    pub(crate) fn active_tab_visible_for_test(&self) -> Option<bool> {
        let viewport = self.tab_scroll.bounds();
        // Item bounds are laid out unscrolled; a tab wider than the strip shows its start.
        let tab = self.tab_scroll.bounds_for_item(self.active_tab)?;
        let left = tab.left() + self.tab_scroll.offset().x;
        let right = (left + tab.size.width).min(left + viewport.size.width);
        Some(left >= viewport.left() - px(1.) && right <= viewport.right() + px(1.))
    }

    fn render_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reveals = self.tab_reveals.get();
        if reveals > 0 {
            self.tab_reveals.set(reveals - 1);
            self.tab_scroll.scroll_to_item(self.active_tab);
            if reveals > 1 {
                window.request_animation_frame();
            }
        }
        let engine = &self.app.read(cx).engine;
        let target = self.tab_menu_target.clone();
        let tabs: Vec<Tab> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let id = tab.item(cx);
                let name = self.item_name(id, cx);
                let dirty = id
                    .document()
                    .and_then(|document| engine.document(document))
                    .is_some_and(|document| document.dirty);
                // An image's tab says so, and names the file in full on hover.
                let image_path = id
                    .image()
                    .and_then(|image| engine.image(image))
                    .map(|image| SharedString::from(image.path.display().to_string()));
                let preview = engine.is_preview(id);
                let target = target.clone();
                let aria = match (preview, image_path.is_some()) {
                    (true, true) => format!("{name}, image, preview tab"),
                    (true, false) => format!("{name}, preview tab"),
                    (false, true) => format!("{name}, image"),
                    (false, false) => name.clone(),
                };
                Tab::new()
                    .aria_label(aria)
                    .label(if dirty { format!("● {name}") } else { name })
                    .when(image_path.is_some(), |tab| {
                        tab.icon(gpui_kit_assets::IconName::Image)
                    })
                    .debug_selector(move || format!("editor-tab-{ix}"))
                    .on_mouse_down(gpui::MouseButton::Right, move |_, _, _| {
                        target.set(Some(id));
                    })
                    .when(!preview, |tab| match image_path.clone() {
                        Some(path) => tab.tooltip(move |window, cx| {
                            Tooltip::new(path.clone()).build(window, cx)
                        }),
                        None => tab,
                    })
                    // A preview tab: italic, and a double-click keeps it open.
                    .when(preview, |tab| {
                        let path = image_path.clone();
                        tab.italic()
                            .tooltip(move |window, cx| {
                                let path = path.clone();
                                Tooltip::element(move |_, _| {
                                    v_flex()
                                        .debug_selector(|| "preview-tab-tooltip".to_owned())
                                        .children(path.clone())
                                        .child(PREVIEW_TAB_TOOLTIP)
                                })
                                .build(window, cx)
                            })
                            // In the capture phase: the tab's contents take the bubbling one.
                            .capture_any_mouse_down(cx.listener(
                                move |this, event: &gpui::MouseDownEvent, window, cx| {
                                    if event.button == gpui::MouseButton::Left
                                        && event.click_count > 1
                                    {
                                        this.invoke(
                                            ids::DOCUMENT_KEEP_OPEN,
                                            item_args(id),
                                            window,
                                            cx,
                                        );
                                    }
                                },
                            ))
                    })
                    .suffix(
                        Button::new(("close-tab", ix))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tab_stop(false)
                            .debug_selector(move || format!("tab-close-{ix}"))
                            .tooltip("Close")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // Not also a click on the tab, which would select another.
                                cx.stop_propagation();
                                this.close_tabs(ids::DOCUMENT_CLOSE, id, window, cx);
                            })),
                    )
            })
            .collect();
        let (back, forward) = (engine.can_go_back(), engine.can_go_forward());
        let arrows = h_flex()
            .px_1()
            .child(
                Button::new("tab-bar-back")
                    .icon(IconName::ArrowLeft)
                    .ghost()
                    .xsmall()
                    .disabled(!back)
                    .tooltip("Go Back")
                    .debug_selector(|| "tab-bar-back".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.invoke(ids::NAVIGATE_BACK, CommandArgs::None, window, cx);
                    })),
            )
            .child(
                Button::new("tab-bar-forward")
                    .icon(IconName::ArrowRight)
                    .ghost()
                    .xsmall()
                    .disabled(!forward)
                    .tooltip("Go Forward")
                    .debug_selector(|| "tab-bar-forward".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.invoke(ids::NAVIGATE_FORWARD, CommandArgs::None, window, cx);
                    })),
            );
        let bar = TabBar::new("editor-tabs")
            .prefix(arrows)
            .track_scroll(&self.tab_scroll)
            .selected_index(self.active_tab)
            .children(tabs)
            .on_click(cx.listener(|this, index: &usize, window, cx| {
                let Some(item) = this.tabs.get(*index).map(|tab| tab.item(cx)) else {
                    return;
                };
                this.app.update(cx, |app, cx| {
                    app.engine.activate(item);
                    app.drain(cx);
                });
                this.follow_active(window, cx);
                cx.notify();
            }));
        let target = self.tab_menu_target.clone();
        let app = self.app.clone();
        let workspace = cx.entity().downgrade();
        div()
            .id("editor-tab-strip")
            // Cleared first, then set by the tab under the pointer, if any.
            .capture_any_mouse_down({
                let target = target.clone();
                move |_, _, _| target.set(None)
            })
            .child(bar)
            .context_menu(move |menu, _, cx| {
                let target = target.get();
                let engine = &app.read(cx).engine;
                let entries: &[(&str, adoc_engine::CommandId, Box<dyn gpui::Action>)] = &[
                    ("Close", ids::DOCUMENT_CLOSE, Box::new(CloseDocument)),
                    (
                        "Close Others",
                        ids::DOCUMENT_CLOSE_OTHERS,
                        Box::new(CloseOthers),
                    ),
                    ("Close Left", ids::DOCUMENT_CLOSE_LEFT, Box::new(CloseLeft)),
                    (
                        "Close Right",
                        ids::DOCUMENT_CLOSE_RIGHT,
                        Box::new(CloseRight),
                    ),
                    (
                        "Close Clean",
                        ids::DOCUMENT_CLOSE_CLEAN,
                        Box::new(CloseClean),
                    ),
                    ("Close All", ids::DOCUMENT_CLOSE_ALL, Box::new(CloseAll)),
                ];
                // Over the empty strip, only the commands that need no tab, on the active one.
                let tab = target.or_else(|| engine.active_item());
                let Some(tab) = tab else {
                    return menu;
                };
                let menu = match target {
                    Some(target) => {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new("Keep Open")
                                .action(Box::new(KeepOpen))
                                .disabled(!engine.is_preview(target))
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| {
                                        let _ = app.invoke(
                                            ids::DOCUMENT_KEEP_OPEN,
                                            item_args(target),
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .separator()
                    }
                    None => menu,
                };
                entries
                    .iter()
                    .filter(|(_, command, _)| {
                        target.is_some()
                            || matches!(
                                *command,
                                ids::DOCUMENT_CLOSE_CLEAN | ids::DOCUMENT_CLOSE_ALL
                            )
                    })
                    .fold(menu, |menu, (label, command, action)| {
                        let command = *command;
                        let workspace = workspace.clone();
                        menu.item(
                            PopupMenuItem::new(*label)
                                .action(action.boxed_clone())
                                .disabled(engine.closed_by(command, tab).is_empty())
                                .on_click(move |_, window, cx| {
                                    let _ = workspace.update(cx, |this, cx| {
                                        this.close_tabs(command, tab, window, cx);
                                    });
                                }),
                        )
                    })
            })
    }

    /// The zero-based line and column (in characters, as the source editor counts them) of the
    /// caret the user is working with: the visual editor's in Visual mode, or in Split mode while
    /// it has focus, otherwise the source editor's.
    /// The visual editor of document `id` when it, rather than the source editor, holds the
    /// caret: always in Visual mode, and in Split mode while it has focus.
    fn caret_visual(
        &self,
        id: DocumentId,
        window: &Window,
        cx: &App,
    ) -> Option<&Entity<VisualView>> {
        let engine = &self.app.read(cx).engine;
        self.visuals
            .get(&id)
            .filter(|visual| match engine.editor_mode(id) {
                EditorMode::Visual => true,
                EditorMode::Split => visual.read(cx).focus_handle(cx).is_focused(window),
                EditorMode::Source => false,
            })
    }

    pub(crate) fn caret_line_column(&self, window: &Window, cx: &App) -> Option<(u32, u32)> {
        let editor = self.active_editor()?.read(cx);
        let id = editor.document();
        let engine = &self.app.read(cx).engine;
        let Some(visual) = self.caret_visual(id, window, cx) else {
            return Some(editor.cursor_line_column(cx));
        };
        let text = &engine.document(id)?.text;
        let offset = visual.read(cx).selection().head.min(text.len());
        let line = text.byte_to_line_idx(offset, ropey::LineType::LF_CR);
        let start = text.line_to_byte_idx(line, ropey::LineType::LF_CR);
        let column = text.byte_to_char_idx(offset) - text.byte_to_char_idx(start);
        Some((
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(column).unwrap_or(u32::MAX),
        ))
    }

    /// The words of the active page, and of the selection while there is one: `120 words · 1
    /// min read`, or `12 of 120 words`.
    fn word_count(&self, window: &Window, cx: &App) -> Option<(String, String)> {
        let editor = self.active_editor()?.read(cx);
        let id = editor.document();
        let engine = &self.app.read(cx).engine;
        let count = engine.word_count(id)?;
        let selection = match self.caret_visual(id, window, cx) {
            Some(visual) => {
                let selection = visual.read(cx).selection();
                selection.start()..selection.end()
            }
            None => editor.selection(cx),
        };
        let selected = (!selection.is_empty())
            .then(|| engine.text_in(id, SourceRange::new(selection.start, selection.end)))
            .flatten()
            .map(|text| count_words(&text));
        let words = |count: usize| if count == 1 { "word" } else { "words" };
        Some(match selected {
            Some(selected) => (
                format!("status-words-{selected}-of-{}", count.words),
                format!("{selected} of {} {}", count.words, words(count.words)),
            ),
            None if count.minutes == 0 => (
                format!("status-words-{}", count.words),
                format!("{} {}", count.words, words(count.words)),
            ),
            None => (
                format!("status-words-{}", count.words),
                format!(
                    "{} {} · {} min read",
                    count.words,
                    words(count.words),
                    count.minutes
                ),
            ),
        })
    }

    fn render_status(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let caret = self.caret_line_column(window, cx);
        let words = self.word_count(window, cx);
        let engine = &self.app.read(cx).engine;
        let roots = engine.roots().to_vec();
        let (location, path, file) = match self.active_editor() {
            Some(editor) => {
                let editor = editor.read(cx);
                let id = editor.document();
                let document = engine.document(id);
                let position =
                    caret.map(|(line, column)| format!("Ln {}, Col {}", line + 1, column + 1));
                let path = document.map(|document| sidebar::display_path(&document.path, &roots));
                (
                    position,
                    path,
                    document.map(|document| document.path.clone()),
                )
            }
            // An image: its path, as for a page.
            None => match self.active_item(cx).and_then(TabItem::image) {
                Some(id) => {
                    let file = engine.image(id).map(|image| image.path.clone());
                    let path = file
                        .as_deref()
                        .map(|file| sidebar::display_path(file, &roots));
                    (None, path, file)
                }
                None => (None, None, None),
            },
        };
        // The branch of the active document's repository, else of the first root's.
        let repository = file
            .as_deref()
            .or(roots.first().map(std::path::PathBuf::as_path))
            .and_then(|path| engine.git().repository_for(path));
        let branch = repository.map(|repository| repository.head.to_string());
        // Commits not yet pushed: `main ↑2`.
        let ahead = repository
            .and_then(|repository| repository.ahead)
            .map(|ahead| format!(" ↑{ahead}"))
            .unwrap_or_default();
        // The component version of the active page: `product@2.0 · main`.
        let badge = file.as_deref().and_then(|path| engine.version_badge(path));
        let indexing = engine.is_indexing();
        StatusBar::new()
            .left(
                div()
                    .text_xs()
                    .child(path.unwrap_or_else(|| "No document".to_owned())),
            )
            .when_some(badge.clone(), |bar, badge| {
                let selector = format!("status-version-{badge}");
                bar.left(
                    div()
                        .debug_selector(move || selector)
                        .text_xs()
                        .child(badge),
                )
            })
            .when_some(branch, |bar, branch| {
                let selector = format!("status-branch-{branch}");
                let label = if badge.is_some() {
                    format!("· {branch}{ahead}")
                } else {
                    format!("⎇ {branch}{ahead}")
                };
                bar.left(
                    div()
                        .debug_selector(move || selector)
                        .text_xs()
                        .child(label),
                )
            })
            .when(indexing, |bar| bar.left(div().text_xs().child("Indexing…")))
            .when(engine.is_exporting(), |bar| {
                bar.left(div().text_xs().child("Exporting PDF…"))
            })
            .when_some(words, |bar, (selector, words)| {
                bar.right(
                    div()
                        .debug_selector(move || selector)
                        .text_xs()
                        .child(words),
                )
            })
            .right(
                div()
                    .text_xs()
                    .child(format!("{} problem(s)", self.problem_count)),
            )
            .right(div().text_xs().child(location.unwrap_or_default()))
    }
}

impl Focusable for WorkspaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_outline(cx);
        let (can_back, can_forward, title) = {
            let engine = &self.app.read(cx).engine;
            let title = engine
                .roots()
                .first()
                .and_then(|root| root.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "AdocFoundry".to_owned());
            (engine.can_go_back(), engine.can_go_forward(), title)
        };
        let editor = self.editor_area(cx);
        let has_tabs = !self.tabs.is_empty();
        let show_search = self.show_search;
        let layout = self.app.read(cx).engine.preview_layout();
        let docked_preview = (layout.visible && !layout.detached).then(|| self.preview.clone());
        let center = v_flex()
            .size_full()
            .when(has_tabs, |center| {
                center.child(
                    h_flex()
                        .items_center()
                        .child(div().flex_1().min_w_0().child(self.render_tabs(window, cx)))
                        .child(self.render_modes(cx)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .children(editor)
                    .when(!has_tabs, |empty| {
                        empty.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .text_color(cx.theme().muted_foreground)
                                .child("Open a folder or a page from the sidebar")
                                .child(
                                    Button::new("open-folder")
                                        .primary()
                                        .label("Open Folder…")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.on_open_folder(&OpenFolder, window, cx);
                                        })),
                                ),
                        )
                    }),
            )
            .when(show_search, |center| {
                center.child(div().h(px(280.)).child(self.search.clone()))
            })
            .into_any_element();

        v_flex()
            .id("workspace")
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_open_file))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_all))
            .on_action(cx.listener(Self::on_reload))
            .on_action(cx.listener(Self::on_overwrite))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_close_others))
            .on_action(cx.listener(Self::on_close_left))
            .on_action(cx.listener(Self::on_close_right))
            .on_action(cx.listener(Self::on_close_clean))
            .on_action(cx.listener(Self::on_close_all))
            .on_action(cx.listener(Self::on_keep_open))
            .on_action(cx.listener(Self::on_reload_image))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| {
                this.zoom_active_image(Box::new(ZoomIn), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| {
                this.zoom_active_image(Box::new(ZoomOut), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ActualSize, window, cx| {
                this.zoom_active_image(Box::new(ActualSize), window, cx);
            }))
            .on_action(cx.listener(|this, _: &FitToWindow, window, cx| {
                this.zoom_active_image(Box::new(FitToWindow), window, cx);
            }))
            .on_action(cx.listener(Self::on_expand_sidebar))
            .on_action(cx.listener(Self::on_collapse_sidebar))
            .on_action(cx.listener(Self::on_collapse_sidebar_level))
            .on_action(cx.listener(Self::on_reset_sidebar))
            .on_action(cx.listener(Self::on_sidebar_menu))
            .on_action(cx.listener(Self::on_reveal_in_files))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_toggle_strong))
            .on_action(cx.listener(Self::on_toggle_emphasis))
            .on_action(cx.listener(Self::on_toggle_code))
            .on_action(cx.listener(Self::on_toggle_highlight))
            .on_action(cx.listener(Self::on_insert_table))
            .on_action(cx.listener(Self::on_insert_code_block))
            .on_action(cx.listener(Self::on_insert_admonition))
            .on_action(cx.listener(Self::on_insert_link))
            .on_action(cx.listener(Self::on_insert_image))
            .on_action(cx.listener(Self::on_follow_link))
            .on_action(cx.listener(Self::on_go_to_definition))
            .on_action(cx.listener(Self::on_go_back))
            .on_action(cx.listener(Self::on_go_forward))
            .on_action(cx.listener(Self::on_peek_include))
            .on_action(cx.listener(Self::on_find))
            .on_action(cx.listener(Self::on_replace))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_reindex))
            .on_action(cx.listener(Self::on_refresh_git))
            .on_action(cx.listener(Self::on_revert_change))
            .on_action(cx.listener(Self::on_format_table))
            .on_action(cx.listener(Self::on_next_change))
            .on_action(cx.listener(Self::on_previous_change))
            .on_action(cx.listener(Self::on_show_changes))
            .on_action(cx.listener(Self::on_open_in_version))
            .on_action(cx.listener(Self::on_toggle_blame))
            .on_action(cx.listener(Self::on_open_on_remote))
            .on_action(cx.listener(Self::on_copy_remote_link))
            .on_action(cx.listener(Self::on_stage_file))
            .on_action(cx.listener(Self::on_unstage_file))
            .on_action(cx.listener(Self::on_stage_all))
            .on_action(cx.listener(Self::on_commit_changes))
            .on_action(cx.listener(Self::on_push))
            .on_action(cx.listener(Self::on_export_page_pdf))
            .on_action(cx.listener(Self::on_export_page_html))
            .on_action(cx.listener(Self::on_export_component_pdf))
            .on_action(cx.listener(Self::on_export_pdf_profile))
            .on_action(cx.listener(Self::on_export_page_pdf_with_theme))
            .on_action(cx.listener(Self::on_save_and_commit))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_check_for_updates))
            .on_action(cx.listener(Self::on_show_diagnostics))
            .on_action(cx.listener(Self::on_recover_unsaved))
            .on_action(cx.listener(Self::on_local_history))
            .on_action(cx.listener(Self::on_reload_settings))
            .on_action(cx.listener(Self::on_toggle_theme))
            .on_action(cx.listener(Self::on_open_mirror))
            .on_action(cx.listener(Self::on_palette))
            .on_action(cx.listener(Self::on_toggle_preview))
            .on_action(cx.listener(Self::on_detach_preview))
            .on_action(cx.listener(Self::on_toggle_preview_sync))
            .on_action(cx.listener(Self::on_toggle_pdf_preview))
            .on_action(cx.listener(Self::on_pin_preview))
            .on_action(cx.listener(Self::on_show_source))
            .on_action(cx.listener(Self::on_show_visual))
            .on_action(cx.listener(Self::on_show_split))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_1()
                        .child(self.app_menu.clone())
                        .child(
                            Button::new("back")
                                .ghost()
                                .xsmall()
                                .icon(IconName::ArrowLeft)
                                .tooltip("Go Back")
                                .disabled(!can_back)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_go_back(&GoBack, window, cx);
                                })),
                        )
                        .child(
                            Button::new("forward")
                                .ghost()
                                .xsmall()
                                .icon(IconName::ArrowRight)
                                .tooltip("Go Forward")
                                .disabled(!can_forward)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_go_forward(&GoForward, window, cx);
                                })),
                        )
                        .child(div().px_2().text_sm().child(title))
                        .child(
                            Button::new("palette")
                                .ghost()
                                .xsmall()
                                .label("Commands")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_palette(&CommandPalette, window, cx);
                                })),
                        ),
                ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("workspace-panels")
                        .child(
                            resizable_panel()
                                .size(px(280.))
                                .child(self.render_sidebar(cx)),
                        )
                        .child(
                            resizable_panel().child(match docked_preview {
                                Some(preview) => h_resizable("editor-preview")
                                    .child(resizable_panel().child(center))
                                    .child(resizable_panel().size(px(520.)).child(preview))
                                    .into_any_element(),
                                None => center,
                            }),
                        )
                        .child(
                            resizable_panel().size(px(240.)).child(
                                v_flex()
                                    .size_full()
                                    .bg(cx.theme().sidebar)
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Outline"),
                                    )
                                    .child(div().flex_1().min_h_0().child(self.render_tree(
                                        &self.outline.clone(),
                                        "outline",
                                        cx,
                                    ))),
                            ),
                        ),
                ),
            )
            .child(self.render_status(window, cx))
    }
}
