//! The workspace window: title bar, sidebar, editor tabs, outline, search panel and status bar.
//!
//! Views render engine state and dispatch commands. Opening a tab follows the engine's active
//! document, so link following, back/forward and palette commands all arrive the same way.

use std::{cell::Cell, collections::HashMap, path::Path, rc::Rc};

use adoc_core::{LineIndex, SourceRange};
use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DocumentId, EditPreview, EditorMode, EngineEvent,
    ExportArgs, ExportSummary, ExportTarget, NavigationPosition, ThemePreference,
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
    notification::Notification,
    resizable::{h_resizable, resizable_panel},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    tree::{tree, TreeState},
    v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _, Theme, ThemeMode, TitleBar,
    WindowExt as _,
};

use crate::{
    app::{open_secondary_window, AppState, WindowKind},
    editor::{bindings::*, language, SourceEditorEvent, SourceEditorView},
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
    Problems,
    Changes,
}

pub struct WorkspaceView {
    app: Entity<AppState>,
    focus_handle: FocusHandle,
    tabs: Vec<Entity<SourceEditorView>>,
    /// The visual editor of each document shown in visual or split mode.
    visuals: HashMap<DocumentId, Entity<VisualView>>,
    active_tab: usize,
    /// The tab strip's scroll, so the active tab can be kept in view.
    tab_scroll: gpui::ScrollHandle,
    /// Frames still to request the active tab's reveal on; see [`Self::reveal_active_tab`].
    tab_reveals: std::cell::Cell<u8>,
    sidebar_tab: SidebarTab,
    navigation: Entity<TreeState>,
    families: Entity<TreeState>,
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
    preview: Entity<PreviewView>,
    /// The preview's own window while it is detached.
    detached: Option<(
        gpui::WindowHandle<gpui_component::Root>,
        Entity<PreviewView>,
    )>,
    revealed: Option<NavigationPosition>,
    _subscriptions: Vec<Subscription>,
}

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
pub(crate) fn export_message(output: &Path, result: &Result<ExportSummary, String>) -> String {
    match result {
        Ok(summary) => {
            let warnings = summary.diagnostics.len();
            let name = output
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let mut message = format!("Exported {name} ({} pages)", summary.pages);
            if warnings > 0 {
                message.push_str(&format!(
                    "; {warnings} warning{}: {}",
                    if warnings == 1 { "" } else { "s" },
                    summary.diagnostics[0]
                ));
            }
            message
        }
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
        let subscriptions = vec![
            cx.subscribe_in(&app, window, Self::on_engine_event),
            cx.subscribe(&preview, Self::on_preview_event),
        ];
        let workspace = cx.weak_entity();
        let search = cx.new(|cx| SearchView::new(app.clone(), workspace, window, cx));
        let mut view = Self {
            focus_handle: cx.focus_handle(),
            tabs: Vec::new(),
            visuals: HashMap::new(),
            active_tab: 0,
            tab_scroll: gpui::ScrollHandle::new(),
            tab_reveals: std::cell::Cell::new(0),
            sidebar_tab: SidebarTab::Navigation,
            navigation: cx.new(|cx| TreeState::new(cx)),
            families: cx.new(|cx| TreeState::new(cx)),
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
            view.open_at(&file, SourceRange::default(), window, cx);
        }
        if let (Some(duration), Some(editor)) = (
            crate::typing_bench::requested(),
            view.active_editor().cloned(),
        ) {
            crate::typing_bench::start(editor, duration, window, cx);
        }
        view
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
            EngineEvent::DocumentClosed(id) => {
                self.tabs.retain(|tab| tab.read(cx).document() != *id);
                self.visuals.remove(id);
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

    /// Focuses the visual editor in visual mode, else the source editor.
    fn focus_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.navigation
            .update(cx, |tree, cx| tree.set_items(navigation, cx));
        self.families
            .update(cx, |tree, cx| tree.set_items(families, cx));
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
        let Some(CommandOutcome::Opened(id)) = self.invoke(
            ids::DOCUMENT_OPEN,
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
        self.open_at(path, SourceRange::new(first, first), window, cx);
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
        let Some(active) = self.app.read(cx).engine.active() else {
            self.active_tab = 0;
            return;
        };
        let index = match self
            .tabs
            .iter()
            .position(|tab| tab.read(cx).document() == active)
        {
            Some(index) => index,
            None => {
                let app = self.app.clone();
                let editor = cx.new(|cx| SourceEditorView::new(app, active, false, window, cx));
                self._subscriptions
                    .push(cx.subscribe(&editor, Self::on_editor_event));
                self.tabs.push(editor);
                self.tabs.len() - 1
            }
        };
        self.active_tab = index;
        // However the document became active, its tab is brought into view.
        self.reveal_active_tab(cx);

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
                self.tabs[index].update(cx, |editor, cx| editor.reveal(range, window, cx));
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

    #[cfg(test)]
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    #[cfg(test)]
    pub fn preview(&self) -> &Entity<PreviewView> {
        &self.preview
    }

    fn active_editor(&self) -> Option<&Entity<SourceEditorView>> {
        self.tabs.get(self.active_tab)
    }

    fn active_document(&self, cx: &App) -> Option<DocumentId> {
        self.active_editor()
            .map(|editor| editor.read(cx).document())
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

    fn invoke(
        &mut self,
        command: adoc_engine::CommandId,
        args: CommandArgs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<CommandOutcome> {
        let outcome = self.app.update(cx, |app, cx| app.invoke(command, args, cx));
        match outcome {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                window.push_notification(error.to_string(), cx);
                None
            }
        }
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

    /// Opens `path` and selects `range` in it.
    pub fn open_at(
        &mut self,
        path: &Path,
        range: SourceRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(CommandOutcome::Opened(id)) = self.invoke(
            ids::DOCUMENT_OPEN,
            CommandArgs::Path(path.to_path_buf()),
            window,
            cx,
        ) else {
            return;
        };
        self.app.update(cx, |app, cx| {
            app.engine.set_active(id);
            app.drain(cx);
        });
        self.follow_active(window, cx);
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.reveal(range, window, cx));
        }
        if let Some(visual) = self.visuals.get(&id).cloned() {
            visual.update(cx, |visual, cx| visual.go_to(range.start, cx));
            self.focus_active_editor(window, cx);
        }
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
            let text = std::fs::read_to_string(&file.path).unwrap_or_default();
            let index = LineIndex::new(&text);
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
                    this.open_at(&file, SourceRange::default(), window, cx);
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

    fn on_close(&mut self, _: &CloseDocument, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_document(cx) else {
            return;
        };
        let dirty = self
            .app
            .read(cx)
            .engine
            .document(id)
            .is_some_and(|document| document.dirty);
        if !dirty {
            self.invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(id), window, cx);
            return;
        }
        let app = self.app.clone();
        let name = self.document_name(id, cx);
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let app = app.clone();
            dialog
                .title("Discard unsaved changes?")
                .child(format!("{name} has unsaved changes."))
                .confirm()
                .ok_text("Discard")
                .on_ok(move |_, _, cx| {
                    app.update(cx, |app, cx| {
                        let _ = app.invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(id), cx);
                    });
                    true
                })
        });
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.undo(false, cx));
        }
    }

    /// Toggles a mark in the source editor. The visual editor handles the same actions itself
    /// while it has focus, so this is reached from the source editor or the palette.
    fn toggle_source_mark(
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
        editor.update(cx, |editor, cx| editor.toggle_mark(command, window, cx));
    }

    fn on_toggle_strong(&mut self, _: &ToggleStrong, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_source_mark(ids::EDIT_TOGGLE_STRONG, window, cx);
    }

    fn on_toggle_emphasis(
        &mut self,
        _: &ToggleEmphasis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_source_mark(ids::EDIT_TOGGLE_EMPHASIS, window, cx);
    }

    fn on_toggle_code(&mut self, _: &ToggleCode, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_source_mark(ids::EDIT_TOGGLE_CODE, window, cx);
    }

    fn on_toggle_highlight(
        &mut self,
        _: &ToggleHighlight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_source_mark(ids::EDIT_TOGGLE_HIGHLIGHT, window, cx);
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
                            workspace.open_at(&path, SourceRange::default(), window, cx);
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
            self.open_at(&location.path, location.range, window, cx);
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
            .map(|stem| format!("{}.pdf", stem.to_string_lossy()));
        self.export_to(ExportTarget::Page(id), &directory, name, window, cx);
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
            ExportTarget::Component { name, version },
            &directory,
            suggested,
            window,
            cx,
        );
    }

    fn export_to(
        &mut self,
        target: ExportTarget,
        directory: &Path,
        suggested: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = match target {
            ExportTarget::Page(_) => ids::EXPORT_PDF_PAGE,
            ExportTarget::Component { .. } => ids::EXPORT_PDF_COMPONENT,
        };
        let chosen = cx.prompt_for_new_path(directory, suggested.as_deref());
        cx.spawn_in(window, async move |this, cx| {
            let output = chosen.await.ok()?.ok()??;
            this.update_in(cx, |this, window, cx| {
                this.invoke(
                    command,
                    CommandArgs::Export(ExportArgs {
                        target,
                        output,
                        theme: None,
                        standards: Vec::new(),
                        fonts_dir: None,
                        typst: None,
                    }),
                    window,
                    cx,
                );
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Tells the user how an export went, with a button to open the PDF.
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

    fn on_open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(CommandOutcome::External(path)) =
            self.invoke(ids::SETTINGS_OPEN, CommandArgs::None, window, cx)
        {
            self.open_at(&path, SourceRange::default(), window, cx);
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
    fn render_modes(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .gap_1()
            .px_2()
            .child(button("mode-source", "Source", EditorMode::Source, cx))
            .child(button("mode-visual", "Visual", EditorMode::Visual, cx))
            .child(button("mode-split", "Split", EditorMode::Split, cx))
    }

    fn on_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        // The palette dispatches an item's action from its own field, where only workspace
        // actions are handled; the visual editor's run once the palette has closed and handed
        // focus back, from `on_confirm`.
        let mut visual_actions: Vec<Option<Box<dyn gpui::Action>>> = Vec::new();
        let mut items: Vec<CommandItem> = palette_commands(&self.app.read(cx).engine)
            .into_iter()
            .filter_map(|spec| {
                let action = action_for(spec.id)?;
                let item = CommandItem::new()
                    .label(spec.title)
                    .keywords([spec.id.as_str()]);
                Some(if in_visual_editor(spec.id) {
                    visual_actions.push(Some(action));
                    item
                } else {
                    visual_actions.push(None);
                    item.action(action)
                })
            })
            .collect();
        items.extend(UI_ACTIONS.iter().enumerate().map(|(index, (title, _))| {
            visual_actions.push(None);
            CommandItem::new().label(*title).action(ui_action(index))
        }));
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
                        let visual_actions: Vec<Option<Box<dyn gpui::Action>>> = visual_actions
                            .iter()
                            .map(|action| action.as_ref().map(|action| action.boxed_clone()))
                            .collect();
                        move |index, window, cx| {
                            let Some(Some(action)) = visual_actions.get(index.row) else {
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
        div().id(id).size_full().child(
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
                    .on_click(move |_, window, cx| {
                        let Some(target) = sidebar::parse_target(&item.id) else {
                            return;
                        };
                        if let Some(workspace) = workspace.upgrade() {
                            workspace.update(cx, |workspace, cx| {
                                workspace.open_at(&target.path, target.range, window, cx);
                            });
                        }
                    })
            })
            .text_sm(),
        )
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
        let tabs: Vec<Tab> = self
            .tabs
            .iter()
            .map(|tab| {
                let id = tab.read(cx).document();
                let (name, dirty) = engine
                    .document(id)
                    .map(|document| {
                        let name = document
                            .path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        (name, document.dirty)
                    })
                    .unwrap_or_default();
                Tab::new().label(if dirty { format!("● {name}") } else { name })
            })
            .collect();
        TabBar::new("editor-tabs")
            .track_scroll(&self.tab_scroll)
            .selected_index(self.active_tab)
            .children(tabs)
            .on_click(cx.listener(|this, index: &usize, window, cx| {
                let Some(id) = this.tabs.get(*index).map(|tab| tab.read(cx).document()) else {
                    return;
                };
                this.app.update(cx, |app, cx| {
                    app.engine.set_active(id);
                    app.drain(cx);
                });
                this.follow_active(window, cx);
                cx.notify();
            }))
    }

    /// The zero-based line and column (in characters, as the source editor counts them) of the
    /// caret the user is working with: the visual editor's in Visual mode, or in Split mode while
    /// it has focus, otherwise the source editor's.
    pub(crate) fn caret_line_column(&self, window: &Window, cx: &App) -> Option<(u32, u32)> {
        let editor = self.active_editor()?.read(cx);
        let id = editor.document();
        let engine = &self.app.read(cx).engine;
        let visual = self
            .visuals
            .get(&id)
            .filter(|visual| match engine.editor_mode(id) {
                EditorMode::Visual => true,
                EditorMode::Split => visual.read(cx).focus_handle(cx).is_focused(window),
                EditorMode::Source => false,
            });
        let Some(visual) = visual else {
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

    fn render_status(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let caret = self.caret_line_column(window, cx);
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
            None => (None, None, None),
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
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_toggle_strong))
            .on_action(cx.listener(Self::on_toggle_emphasis))
            .on_action(cx.listener(Self::on_toggle_code))
            .on_action(cx.listener(Self::on_toggle_highlight))
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
            .on_action(cx.listener(Self::on_export_component_pdf))
            .on_action(cx.listener(Self::on_save_and_commit))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_reload_settings))
            .on_action(cx.listener(Self::on_toggle_theme))
            .on_action(cx.listener(Self::on_open_mirror))
            .on_action(cx.listener(Self::on_palette))
            .on_action(cx.listener(Self::on_toggle_preview))
            .on_action(cx.listener(Self::on_detach_preview))
            .on_action(cx.listener(Self::on_toggle_preview_sync))
            .on_action(cx.listener(Self::on_pin_preview))
            .on_action(cx.listener(Self::on_show_source))
            .on_action(cx.listener(Self::on_show_visual))
            .on_action(cx.listener(Self::on_show_split))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_1()
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
