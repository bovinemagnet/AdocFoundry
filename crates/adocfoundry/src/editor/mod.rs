//! The source editor: gpui-component's code editor mirroring one engine document.
//!
//! The widget owns a display buffer for performance. Every widget edit is sent to the engine as
//! `edit.insert_text` or `edit.replace_range`; every engine change from elsewhere (undo, replace,
//! rename, a reload from disk) is applied back to the widget. The engine rope is the truth.

pub mod bindings;
pub mod language;
mod providers;
pub mod sync;

use std::rc::Rc;

use adoc_core::{DiagnosticSeverity, SourceRange};
use adoc_engine::{commands::ids, CommandArgs, DocumentId, EngineEvent};
use gpui::{
    div, relative, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window,
};
use gpui_component::{
    highlighter::{Diagnostic, DiagnosticSeverity as WidgetSeverity},
    input::{Editor, EditorState, InputEvent, RopeExt as _, TabSize},
    ActiveTheme as _,
};

use crate::app::AppState;
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

pub struct SourceEditorView {
    app: Entity<AppState>,
    document: DocumentId,
    state: Entity<EditorState>,
    _diagnostics: Option<gpui::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Diagnostics are redrawn once typing pauses for this long, not on every keystroke.
const DIAGNOSTICS_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

impl SourceEditorView {
    pub fn new(
        app: Entity<AppState>,
        document: DocumentId,
        readonly: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (text, soft_wrap, tab_width) = {
            let engine = &app.read(cx).engine;
            let text = engine
                .document(document)
                .map(|open| open.text.to_string())
                .unwrap_or_default();
            let editor = &engine.settings().editor;
            (text, editor.soft_wrap, editor.tab_width as usize)
        };
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language::ASCIIDOC)
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
                EngineEvent::DiagnosticsUpdated { path } if this.is_path(path, cx) => {
                    this._diagnostics = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(DIAGNOSTICS_DELAY).await;
                        let _ = this.update(cx, |this, cx| this.show_diagnostics(cx));
                    }));
                }
                _ => {}
            },
        ));

        let mut view = Self {
            app,
            document,
            state,
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
    #[cfg(test)]
    pub fn widget_text(&self, cx: &gpui::App) -> String {
        self.state.read(cx).text().to_string()
    }

    /// The caret's byte offset.
    pub fn cursor(&self, cx: &gpui::App) -> usize {
        self.state.read(cx).cursor()
    }

    /// Places the caret at `range` and focuses the editor.
    pub fn reveal(&self, range: SourceRange, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let len = state.text().len();
            state.set_selected_range(range.start.min(len)..range.end.min(len), cx);
            state.focus(window, cx);
        });
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
        div().size_full().child(
            Editor::new(&self.state)
                .bordered(false)
                .h(relative(1.))
                .font_family(font_family)
                .text_size(gpui::px(font_size))
                .text_color(cx.theme().foreground),
        )
    }
}
