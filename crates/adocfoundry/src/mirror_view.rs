//! A read-only view of the active document in a second window, updating as it is edited.
//!
//! It exists so the second-window path is proven in Phase 1: Phase 2's detachable preview is
//! opened the same way.

use adoc_engine::{DocumentId, EngineEvent};
use gpui::{
    div, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window,
};
use gpui_component::{v_flex, ActiveTheme as _, TitleBar};

use crate::{app::AppState, editor::SourceEditorView};

pub struct MirrorView {
    app: Entity<AppState>,
    editor: Option<Entity<SourceEditorView>>,
    _subscription: Subscription,
}

impl MirrorView {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe_in(&app, window, |this, _, event, window, cx| {
            if matches!(
                event,
                EngineEvent::NavigationChanged | EngineEvent::DocumentClosed(_)
            ) {
                this.follow_active(window, cx);
            }
        });
        let mut view = Self {
            app,
            editor: None,
            _subscription: subscription,
        };
        view.follow_active(window, cx);
        view
    }

    fn follow_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active: Option<DocumentId> = self.app.read(cx).engine.active();
        let current = self
            .editor
            .as_ref()
            .map(|editor| editor.read(cx).document());
        if active == current {
            return;
        }
        self.editor = active.map(|document| {
            let app = self.app.clone();
            cx.new(|cx| SourceEditorView::new(app, document, true, window, cx))
        });
        cx.notify();
    }
}

impl Render for MirrorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .editor
            .as_ref()
            .and_then(|editor| {
                let document = editor.read(cx).document();
                self.app.read(cx).engine.document(document).map(|open| {
                    open.path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
            })
            .unwrap_or_else(|| "No document".to_owned());
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(TitleBar::new().child(format!("{title} — read-only")))
            .child(div().flex_1().min_h_0().children(self.editor.clone()))
    }
}
