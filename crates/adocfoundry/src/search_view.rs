//! Workspace find and replace: a query, options, a results list, and replace through a preview.

use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, EngineEvent, SearchId, SearchMatch, SearchRequest,
};
use gpui::{
    div, prelude::FluentBuilder as _, px, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex, ActiveTheme as _, Sizable as _,
};

use crate::{app::AppState, sidebar::display_path, workspace_view::WorkspaceView};

/// The most results listed; the count still reports them all.
const SHOWN_RESULTS: usize = 500;

pub struct SearchView {
    app: Entity<AppState>,
    workspace: gpui::WeakEntity<WorkspaceView>,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    pending: Option<SearchId>,
    results: Vec<SearchMatch>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl SearchView {
    pub fn new(
        app: Entity<AppState>,
        workspace: gpui::WeakEntity<WorkspaceView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find in workspace"));
        let replacement = cx.new(|cx| InputState::new(window, cx).placeholder("Replace with"));
        let subscriptions = vec![
            cx.subscribe_in(&query, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.search(window, cx);
                }
            }),
            cx.subscribe(&app, |this, _, event, cx| {
                if let EngineEvent::SearchResults {
                    request, results, ..
                } = event
                {
                    if this.pending == Some(*request) {
                        this.results = results.clone();
                        this.pending = None;
                        cx.notify();
                    }
                }
            }),
        ];
        Self {
            app,
            workspace,
            query,
            replacement,
            regex: false,
            case_sensitive: false,
            whole_word: false,
            pending: None,
            results: Vec::new(),
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |query, cx| query.focus(window, cx));
    }

    fn request(&self, cx: &Context<Self>) -> SearchRequest {
        SearchRequest {
            query: self.query.read(cx).value().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            scope: None,
        }
    }

    fn search(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let request = self.request(cx);
        if request.query.is_empty() {
            return;
        }
        let result = self.app.update(cx, |app, _| app.engine.search(request));
        match result {
            Ok(id) => {
                self.pending = Some(id);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string().into()),
        }
        cx.notify();
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.request(cx);
        if request.query.is_empty() {
            return;
        }
        let replacement = self.replacement.read(cx).value().to_string();
        let outcome = self.app.update(cx, |app, cx| {
            app.invoke(
                ids::WORKSPACE_REPLACE,
                CommandArgs::Custom(serde_json::json!({
                    "request": request,
                    "replacement": replacement,
                })),
                cx,
            )
        });
        match outcome {
            Ok(CommandOutcome::Preview(preview)) => {
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.show_preview(preview, window, cx);
                    });
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    fn open_result(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(found) = self.results.get(index).cloned() else {
            return;
        };
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_at(&found.path, found.range, window, cx);
            });
        }
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let roots = self.app.read(cx).engine.roots().to_vec();
        let status: SharedString = if let Some(error) = &self.error {
            error.clone()
        } else if self.pending.is_some() {
            "Searching…".into()
        } else {
            let files = {
                let mut paths: Vec<_> = self.results.iter().map(|found| &found.path).collect();
                paths.dedup();
                paths.len()
            };
            format!("{} result(s) in {files} file(s)", self.results.len()).into()
        };
        v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .bg(cx.theme().background)
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.query).small()))
                    .child(
                        Checkbox::new("search-regex")
                            .label(".*")
                            .checked(self.regex)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.regex = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("search-case")
                            .label("Aa")
                            .checked(self.case_sensitive)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.case_sensitive = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("search-word")
                            .label("Word")
                            .checked(self.whole_word)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.whole_word = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("search-run")
                            .primary()
                            .small()
                            .label("Find")
                            .on_click(cx.listener(|this, _, window, cx| this.search(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.replacement).small()))
                    .child(
                        Button::new("search-replace")
                            .small()
                            .label("Replace All…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.replace_all(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
            .child(
                v_flex()
                    .id("search-results")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(self.results.iter().take(SHOWN_RESULTS).enumerate().map(
                        |(index, found)| {
                            let label = format!(
                                "{}:{}  {}",
                                display_path(&found.path, &roots),
                                found.line + 1,
                                found.line_text.trim()
                            );
                            div()
                                .id(("search-result", index))
                                .px_1()
                                .text_sm()
                                .cursor_pointer()
                                .hover(|style| style.bg(cx.theme().accent))
                                .when(index % 2 == 1, |row| row.bg(cx.theme().muted))
                                .child(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_result(index, window, cx);
                                }))
                        },
                    )),
            )
    }
}
