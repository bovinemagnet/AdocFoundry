//! The source editor: gpui-component's code editor mirroring one engine document.
//!
//! The widget owns a display buffer for performance. Every widget edit is sent to the engine as
//! `edit.insert_text` or `edit.replace_range`; every engine change from elsewhere (undo, replace,
//! rename, a reload from disk) is applied back to the widget. The engine rope is the truth.

pub mod bindings;
pub mod blame;
pub mod changes;
pub mod language;
mod providers;
pub mod sync;

use std::{cell::RefCell, rc::Rc};

use adoc_core::{DiagnosticSeverity, SourceRange};
use adoc_engine::{commands::ids, CommandArgs, CommandId, CommandOutcome, DocumentId, EngineEvent};
use gpui::{
    div, prelude::FluentBuilder as _, relative, AppContext as _, Context, Entity, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window,
};
use gpui_component::{
    highlighter::{Diagnostic, DiagnosticSeverity as WidgetSeverity},
    input::{Editor, EditorState, InputEvent, RopeExt as _, TabSize},
    ActiveTheme as _, WindowExt as _,
};

use crate::{app::AppState, sidebar::DraggedLink};
use providers::EngineProviders;

/// Sends the difference between the engine's text and `widget_text` to the engine as one edit.
pub fn sync_widget_to_engine(
    app: &mut AppState,
    document: DocumentId,
    widget_text: &str,
    cx: &mut Context<AppState>,
) {
    let Some(open) = app.engine.document(document) else {
        return;
    };
    let engine_text = open.text.to_string();
    let Some((range, replacement)) = sync::diff(&engine_text, widget_text) else {
        return;
    };
    let command = if range.is_empty() {
        ids::EDIT_INSERT_TEXT
    } else {
        ids::EDIT_REPLACE_RANGE
    };
    let args = CommandArgs::Text {
        id: document,
        range: SourceRange::new(range.start, range.end),
        text: replacement.to_owned(),
    };
    if let Err(error) = app.invoke(command, args, cx) {
        // The widget and engine disagree in a way an edit cannot express; the engine wins
        // when the editor next reconciles.
        eprintln!("adocfoundry: edit rejected: {error}");
    }
}

/// The character boundary under `point` nearest to it, among the rows the editor has laid out.
fn offset_at(state: &EditorState, point: gpui::Point<gpui::Pixels>) -> Option<usize> {
    let rows = state.visible_row_range()?;
    let map = state.display_map();
    let text = state.text();
    let first = map.display_row_to_buffer_line(rows.start);
    let last = map
        .display_row_to_buffer_line(rows.end.saturating_sub(1).max(rows.start))
        .min(text.lines_len().saturating_sub(1));
    let mut nearest: Option<(gpui::Pixels, usize)> = None;
    for line in first..=last {
        let (start, end) = (text.line_start_offset(line), text.line_end_offset(line));
        let line_text = text.slice(start..end).to_string();
        let boundaries = line_text.char_indices().map(|(at, _)| start + at);
        for offset in boundaries.chain([end]) {
            let Some(bounds) = state.range_to_bounds(&(offset..offset)) else {
                continue;
            };
            if point.y < bounds.top() || point.y >= bounds.bottom() {
                continue;
            }
            let distance = (bounds.left() - point.x).abs();
            if nearest.is_none_or(|(closest, _)| distance < closest) {
                nearest = Some((distance, offset));
            }
        }
    }
    nearest.map(|(_, offset)| offset)
}

/// What the editor tells the window around it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEditorEvent {
    /// The caret moved to this byte offset.
    CaretMoved(usize),
}

pub struct SourceEditorView {
    app: Entity<AppState>,
    document: DocumentId,
    caret: Option<usize>,
    state: Entity<EditorState>,
    /// The change markers as last painted.
    marks: Rc<RefCell<Vec<changes::GutterMark>>>,
    /// The change whose card is open, with its text in `HEAD` and where it was clicked.
    open_change: Option<(adoc_engine::Hunk, String, gpui::Point<gpui::Pixels>)>,
    /// Whether the blame column is shown.
    show_blame: bool,
    /// The gutter line the pointer rests on, and the blame tooltip once it has rested long enough.
    hover_line: Option<usize>,
    hover_tip: Option<(String, gpui::Point<gpui::Pixels>)>,
    _hover: Option<gpui::Task<()>>,
    _diagnostics: Option<gpui::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Diagnostics are redrawn once typing pauses for this long, not on every keystroke.
const DIAGNOSTICS_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
/// Lines left above a place navigated to, so it is read in context.
const REVEAL_MARGIN: usize = 3;
/// How long the pointer rests on the gutter before the line's blame shows.
const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

impl SourceEditorView {
    pub fn new(
        app: Entity<AppState>,
        document: DocumentId,
        readonly: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (text, soft_wrap, tab_width, language) = {
            let engine = &app.read(cx).engine;
            let text = engine
                .document(document)
                .map(|open| open.text.to_string())
                .unwrap_or_default();
            let editor = &engine.settings().editor;
            let language = engine
                .document(document)
                .map(|open| language::for_document(&open.path, open.asciidoc))
                .unwrap_or_else(|| language::ASCIIDOC.to_owned());
            (text, editor.soft_wrap, editor.tab_width as usize, language)
        };
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .folding(true)
                .soft_wrap(soft_wrap)
                .tab_size(TabSize {
                    tab_size: tab_width,
                    hard_tabs: false,
                })
                .default_value(text);
            state.set_readonly(readonly, cx);
            state
        });

        let providers = Rc::new(EngineProviders::new(app.downgrade(), document));
        state.update(cx, |state, cx| {
            let lsp = state.lsp_mut();
            lsp.definition_provider = Some(providers.clone());
            lsp.hover_provider = Some(providers.clone());
            if !readonly {
                lsp.completion_provider = Some(providers.clone());
            }
            let follow_from = providers.clone();
            let follow_app = app.downgrade();
            // A modifier-click on a link: follow it through the engine so history is kept.
            lsp.show_document = Some(Rc::new(move |_, window, cx| {
                let offset = follow_from.last_definition_offset();
                if let Some(app) = follow_app.upgrade() {
                    crate::workspace_view::follow_link(app, document, offset, window, cx);
                }
                true
            }));
            cx.notify();
        });

        let mut subscriptions = vec![cx.subscribe(&state, |this, state, event, cx| {
            if matches!(event, InputEvent::Change) {
                let text = state.read(cx).text().to_string();
                let document = this.document;
                this.app.update(cx, |app, cx| {
                    sync_widget_to_engine(app, document, &text, cx)
                });
            }
        })];
        subscriptions.push(cx.subscribe_in(
            &app,
            window,
            |this, _, event, window, cx| match event {
                EngineEvent::DocumentChanged { id, .. } if *id == this.document => {
                    this.reconcile(window, cx);
                }
                EngineEvent::GitHunksChanged(id) | EngineEvent::GitBlameChanged(id)
                    if *id == this.document =>
                {
                    cx.notify();
                }
                EngineEvent::SettingsChanged => cx.notify(),
                EngineEvent::DiagnosticsUpdated { path } if this.is_path(path, cx) => {
                    this._diagnostics = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(DIAGNOSTICS_DELAY).await;
                        let _ = this.update(cx, |this, cx| this.show_diagnostics(cx));
                    }));
                }
                _ => {}
            },
        ));

        subscriptions.push(cx.observe(&state, |this: &mut Self, state, cx| {
            let caret = state.read(cx).cursor();
            if this.caret != Some(caret) {
                this.caret = Some(caret);
                cx.emit(SourceEditorEvent::CaretMoved(caret));
            }
            // The change markers follow the editor's scrolling.
            cx.notify();
        }));

        let mut view = Self {
            app,
            document,
            caret: None,
            state,
            marks: Rc::default(),
            open_change: None,
            show_blame: false,
            hover_line: None,
            hover_tip: None,
            _hover: None,
            _diagnostics: None,
            _subscriptions: subscriptions,
        };
        view.show_diagnostics(cx);
        view
    }

    pub fn document(&self) -> DocumentId {
        self.document
    }

    fn is_path(&self, path: &std::path::Path, cx: &Context<Self>) -> bool {
        self.app
            .read(cx)
            .engine
            .document(self.document)
            .is_some_and(|open| open.path == path)
    }

    #[cfg(test)]
    pub fn state_for_test(&self) -> &Entity<EditorState> {
        &self.state
    }

    /// The caret's zero-based line and column (in UTF-16 code units, as the widget counts).
    pub fn cursor_line_column(&self, cx: &gpui::App) -> (u32, u32) {
        let position = self.state.read(cx).cursor_position();
        (position.line, position.character)
    }

    /// The widget's current text.
    pub fn widget_text(&self, cx: &gpui::App) -> String {
        self.state.read(cx).text().to_string()
    }

    /// The caret's byte offset.
    pub fn cursor(&self, cx: &gpui::App) -> usize {
        self.state.read(cx).cursor()
    }

    /// The selection, as byte offsets.
    pub fn selection(&self, cx: &gpui::App) -> std::ops::Range<usize> {
        self.state.read(cx).selected_range()
    }

    /// Runs `command` on the selection through the engine (a mark such as `edit.toggle_strong`,
    /// or an insert such as `edit.insert_table`), then selects what the engine says to: the
    /// marked text, or where typing goes next.
    pub fn run_on_selection(
        &mut self,
        command: CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = self.selection(cx);
        let document = self.document;
        let outcome = self.app.update(cx, |app, cx| {
            app.invoke(
                command,
                CommandArgs::Range {
                    id: document,
                    range: SourceRange::new(selection.start, selection.end),
                },
                cx,
            )
        });
        if let Ok(CommandOutcome::Selection(selection)) = outcome {
            self.reconcile(window, cx);
            self.state.update(cx, |state, cx| {
                let len = state.text().len();
                state.set_selected_range(selection.start.min(len)..selection.end.min(len), cx);
            });
        }
    }

    /// Inserts an xref to the page or section dragged in from the sidebar at the text under
    /// the pointer, or at the caret when there is none, and puts the caret after it.
    fn drop_link(&mut self, link: &DraggedLink, window: &mut Window, cx: &mut Context<Self>) {
        let engine = &self.app.read(cx).engine;
        let Some(xref) = engine.xref_to(self.document, &link.path, link.section) else {
            window.push_notification(link.unlinkable(), cx);
            return;
        };
        let at = offset_at(self.state.read(cx), window.mouse_position())
            .unwrap_or_else(|| self.cursor(cx));
        let text = xref.source();
        let document = self.document;
        let inserted = self.app.update(cx, |app, cx| {
            app.invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id: document,
                    range: SourceRange::new(at, at),
                    text: text.clone(),
                },
                cx,
            )
        });
        if let Err(error) = inserted {
            window.push_notification(error.to_string(), cx);
            return;
        }
        self.reconcile(window, cx);
        let caret = at + text.len();
        self.state.update(cx, |state, cx| {
            state.set_selected_range(caret..caret, cx);
            state.focus(window, cx);
        });
    }

    /// Places the caret at `range` and focuses the editor.
    pub fn reveal(&self, range: SourceRange, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let len = state.text().len();
            state.set_selected_range(range.start.min(len)..range.end.min(len), cx);
            state.focus(window, cx);
        });
        // The editor would scroll only far enough to bring the caret to an edge, which can
        // leave it just out of view; the place navigated to goes near the top instead.
        self.scroll_to_line_of(range.start, REVEAL_MARGIN, cx);
    }

    /// Scrolls so the line holding `offset` is at the top, leaving the caret where it is.
    pub fn scroll_to_offset(&self, offset: usize, cx: &mut Context<Self>) {
        self.scroll_to_line_of(offset, 0, cx);
    }

    /// Scrolls so the line holding `offset` is `margin` lines below the top.
    fn scroll_to_line_of(&self, offset: usize, margin: usize, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let Some(line_height) = state.line_height() else {
                return;
            };
            let text = state.text().clone();
            let line = text.offset_to_position(offset.min(text.len())).line as usize;
            let row = state
                .display_map()
                .buffer_line_to_display_row(line)
                .saturating_sub(margin);
            let x = state.scroll_offset().x;
            state.set_scroll_offset(gpui::point(x, -(line_height * row as f32)), cx);
        });
    }

    /// The first source line shown, for tests of scroll sync.
    #[cfg(test)]
    pub fn top_line(&self, cx: &gpui::App) -> Option<usize> {
        let state = self.state.read(cx);
        let line_height = state.line_height()?;
        let row = (-state.scroll_offset().y / line_height).round() as usize;
        Some(state.display_map().display_row_to_buffer_line(row))
    }

    /// Applies the engine's text to the widget when they differ, as one replacement.
    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine_text) = self
            .app
            .read(cx)
            .engine
            .document(self.document)
            .map(|open| open.text.to_string())
        else {
            return;
        };
        self.state.update(cx, |state, cx| {
            let widget_text = state.text().to_string();
            if let Some((range, replacement)) = sync::diff(&widget_text, &engine_text) {
                let replacement = replacement.to_owned();
                state.set_selected_range(range, cx);
                state.replace(replacement, window, cx);
            }
        });
    }

    fn show_diagnostics(&mut self, cx: &mut Context<Self>) {
        let diagnostics = self
            .app
            .read(cx)
            .engine
            .document(self.document)
            .map(|open| open.diagnostics.clone())
            .unwrap_or_default();
        self.state.update(cx, |state, cx| {
            let text = state.text().clone();
            let Some(set) = state.diagnostics_mut() else {
                return;
            };
            set.clear();
            set.extend(diagnostics.into_iter().map(|diagnostic| {
                let start = text.offset_to_position(diagnostic.range.start.min(text.len()));
                let end = text.offset_to_position(diagnostic.range.end.min(text.len()));
                Diagnostic::new(start..end, diagnostic.message)
                    .with_severity(match diagnostic.severity {
                        DiagnosticSeverity::Error => WidgetSeverity::Error,
                        DiagnosticSeverity::Warning => WidgetSeverity::Warning,
                        DiagnosticSeverity::Information => WidgetSeverity::Info,
                    })
                    .with_code(diagnostic.code.as_str())
            }));
            cx.notify();
        });
    }

    /// Opens the card of the `index`th change, clicked at `position`.
    pub fn open_change(
        &mut self,
        index: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let Some(hunk) = engine.git().hunks_for(self.document).get(index).cloned() else {
            return;
        };
        let head = engine.head_text(self.document, &hunk).unwrap_or_default();
        self.open_change = Some((hunk, head, position));
        cx.notify();
    }

    fn close_change(&mut self, cx: &mut Context<Self>) {
        if self.open_change.take().is_some() {
            cx.notify();
        }
    }

    /// Replaces the open change with its text in `HEAD`, as one undo step.
    pub(crate) fn revert_open_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((hunk, _, _)) = self.open_change.take() else {
            return;
        };
        let args = CommandArgs::Offset {
            id: self.document,
            offset: hunk.range.start,
        };
        let result = self
            .app
            .update(cx, |app, cx| app.invoke(ids::GIT_REVERT_HUNK, args, cx));
        if let Err(error) = result {
            gpui_component::WindowExt::push_notification(window, error.to_string(), cx);
        }
        cx.notify();
    }

    /// Shows or hides the blame column, asking the engine for the blame when shown.
    pub fn toggle_blame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document;
        let result = self.app.update(cx, |app, cx| {
            app.invoke(ids::GIT_SHOW_BLAME, CommandArgs::Document(document), cx)
        });
        match result {
            Ok(_) => {
                self.show_blame = !self.show_blame;
                cx.notify();
            }
            Err(error) => {
                gpui_component::WindowExt::push_notification(window, error.to_string(), cx)
            }
        }
    }

    #[cfg(test)]
    pub fn shows_blame(&self) -> bool {
        self.show_blame
    }

    /// The pointer moved over gutter line `line` (or off the gutter): after a rest of 400 ms
    /// there, when `[git] blame_on_hover` is set, the line's blame shows in a tooltip.
    pub fn hover_gutter(
        &mut self,
        line: Option<usize>,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if line == self.hover_line {
            return;
        }
        self.hover_line = line;
        if self.hover_tip.take().is_some() {
            cx.notify();
        }
        self._hover = None;
        let Some(line) = line else {
            return;
        };
        if !self.app.read(cx).engine.settings().git.blame_on_hover {
            return;
        }
        let document = self.document;
        self.app
            .update(cx, |app, _| app.engine.request_blame(document));
        self._hover = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.hover_line != Some(line) {
                    return;
                }
                let blame = this.app.read(cx).engine.blame_for(document);
                if let Some(blamed) = blame.and_then(|blame| blame.get(line).cloned()) {
                    let text = blame::describe(&blamed, blame::now());
                    this.hover_tip = Some((text, position));
                    cx.notify();
                }
            });
        }));
    }

    /// The blame tooltip's text, once shown.
    #[cfg(test)]
    pub fn hover_tip_for_test(&self) -> Option<String> {
        self.hover_tip.as_ref().map(|(text, _)| text.clone())
    }

    /// The change markers as last painted: their kinds and bounds.
    #[cfg(test)]
    pub fn marks_for_test(&self) -> Vec<changes::GutterMark> {
        self.marks.borrow().clone()
    }

    /// The text in `HEAD` shown by the open change card.
    #[cfg(test)]
    pub fn open_change_for_test(&self) -> Option<String> {
        self.open_change.as_ref().map(|(_, head, _)| head.clone())
    }

    /// Types `text` at the caret as a keystroke would, for the typing benchmark.
    pub fn type_for_benchmark(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.insert(text.to_owned(), window, cx));
    }

    /// Follows the link under the caret.
    pub fn follow_link_at_caret(&self, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.cursor(cx);
        crate::workspace_view::follow_link(self.app.clone(), self.document, offset, window, cx);
    }

    /// Runs the engine's `edit.undo` or `edit.redo` for this document.
    pub fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let document = self.document;
        let command = if redo { ids::EDIT_REDO } else { ids::EDIT_UNDO };
        self.app.update(cx, |app, cx| {
            let _ = app.invoke(command, CommandArgs::Document(document), cx);
        });
    }
}

impl SourceEditorView {
    /// The card of an open change: the lines it replaced in `HEAD`, with Revert and Copy.
    fn change_card(
        &self,
        hunk: &adoc_engine::Hunk,
        head: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::{
            prelude::FluentBuilder as _, InteractiveElement as _, StatefulInteractiveElement as _,
        };
        use gpui_component::{
            button::{Button, ButtonVariants as _},
            h_flex, v_flex, Sizable as _,
        };
        let title = match hunk.kind {
            adoc_engine::HunkKind::Added => "Added since the last commit",
            adoc_engine::HunkKind::Modified => "Before this change",
            adoc_engine::HunkKind::Deleted => "Deleted since the last commit",
        };
        let lines: Vec<String> = head.lines().map(str::to_owned).collect();
        let copy = head.to_owned();
        gpui::deferred(
            gpui::anchored().position(position).snap_to_window().child(
                v_flex()
                    .id("change-card")
                    .debug_selector(|| "change-card".to_owned())
                    .occlude()
                    .gap_2()
                    .p_2()
                    .max_w(gpui::px(560.))
                    .bg(cx.theme().popover)
                    .text_color(cx.theme().popover_foreground)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .shadow_md()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_change(cx)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(title),
                    )
                    .when(!lines.is_empty(), |card| {
                        card.child(
                            v_flex()
                                .id("change-head")
                                .max_h(gpui::px(240.))
                                .overflow_y_scroll()
                                .p_1()
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .font_family(
                                    self.app
                                        .read(cx)
                                        .engine
                                        .settings()
                                        .editor
                                        .font_family
                                        .clone(),
                                )
                                .text_sm()
                                .children(lines.into_iter().map(|line| {
                                    div().child(if line.is_empty() {
                                        " ".to_owned()
                                    } else {
                                        line
                                    })
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("revert-change")
                                    .small()
                                    .primary()
                                    .label("Revert Change")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.revert_open_change(window, cx);
                                    })),
                            )
                            .when(!copy.is_empty(), |row| {
                                row.child(
                                    Button::new("copy-change")
                                        .small()
                                        .ghost()
                                        .label("Copy")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                copy.clone(),
                                            ));
                                            this.close_change(cx);
                                        })),
                                )
                            }),
                    ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

impl gpui::EventEmitter<SourceEditorEvent> for SourceEditorView {}

impl Focusable for SourceEditorView {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }
}

impl Render for SourceEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font_family = self
            .app
            .read(cx)
            .engine
            .settings()
            .editor
            .font_family
            .clone();
        let font_size = self.app.read(cx).engine.settings().editor.font_size as f32;
        let engine = &self.app.read(cx).engine;
        let hunks = if engine.settings().git.show_markers {
            engine.git().hunks_for(self.document).to_vec()
        } else {
            Vec::new()
        };
        let colours = changes::MarkColours {
            added: cx.theme().success,
            modified: cx.theme().info,
            deleted: cx.theme().danger,
        };
        let blame = self
            .show_blame
            .then(|| engine.blame_for(self.document))
            .flatten();
        let hover = engine.settings().git.blame_on_hover;
        let card = self
            .open_change
            .as_ref()
            .map(|(hunk, head, position)| self.change_card(hunk, head, *position, cx));
        let tip = self.hover_tip.as_ref().map(|(text, position)| {
            gpui::deferred(
                gpui::anchored()
                    .position(*position + gpui::point(gpui::px(12.), gpui::px(12.)))
                    .snap_to_window()
                    .child(
                        div()
                            .debug_selector(|| "blame-tip".to_owned())
                            .px_2()
                            .py_1()
                            .text_xs()
                            .bg(cx.theme().popover)
                            .text_color(cx.theme().popover_foreground)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius)
                            .child(text.clone()),
                    ),
            )
            .with_priority(1)
        });
        let font = gpui::font(font_family.clone());
        let editor = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                Editor::new(&self.state)
                    .bordered(false)
                    .h(relative(1.))
                    .font_family(font_family)
                    .text_size(gpui::px(font_size))
                    .text_color(cx.theme().foreground),
            )
            .child(changes::overlay(
                cx.weak_entity(),
                self.state.clone(),
                hunks,
                self.marks.clone(),
                colours,
            ))
            .when(hover, |editor| {
                editor.child(blame::hover_watch(cx.weak_entity(), self.state.clone()))
            })
            .on_drop(cx.listener(|this, link: &DraggedLink, window, cx| {
                this.drop_link(link, window, cx);
            }))
            .when_some(blame, |editor, blame| {
                // Painted after the editor, into the space kept for it on the left.
                editor.child(blame::column(
                    self.state.clone(),
                    blame,
                    font,
                    cx.theme().muted_foreground,
                    -blame::COLUMN,
                ))
            });
        gpui_component::h_flex()
            .size_full()
            .when(self.show_blame, |row| {
                row.child(
                    div()
                        .debug_selector(|| "blame-column".to_owned())
                        .flex_none()
                        .h_full()
                        .w(blame::COLUMN)
                        .bg(cx.theme().muted),
                )
            })
            .child(editor)
            .children(card)
            .children(tip)
    }
}
