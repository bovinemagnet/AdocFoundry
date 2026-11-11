//! Workspace find and replace: a query, options, a results list, and replace through a preview.

use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, EngineEvent, SearchId, SearchMatch, SearchRequest,
};
use gpui::{
    div, prelude::FluentBuilder as _, px, uniform_list, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex, ActiveTheme as _, Sizable as _,
};

use crate::{app::AppState, sidebar::display_path, workspace_view::WorkspaceView};

pub struct SearchView {
    app: Entity<AppState>,
    workspace: gpui::WeakEntity<WorkspaceView>,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    /// Searches every text file in the roots, not only the indexed AsciiDoc files.
    all_text_files: bool,
    /// A search has been run, so changing an option runs it again.
    searched: bool,
    pending: Option<SearchId>,
    results: Vec<SearchMatch>,
    /// The search stopped at the engine's limit, with more to find.
    limited: bool,
    error: Option<SharedString>,
    /// The results list, which draws only the rows in view.
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

/// An option the panel's checkboxes set.
#[derive(Clone, Copy)]
enum SearchOption {
    Regex,
    CaseSensitive,
    WholeWord,
    AllTextFiles,
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
            cx.subscribe_in(&query, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.search(cx);
                }
            }),
            cx.subscribe(&app, |this, _, event, cx| {
                // Matches arrive in batches, the last one `done`.
                if let EngineEvent::SearchResults {
                    request,
                    results,
                    done,
                    limited,
                } = event
                {
                    if this.pending == Some(*request) {
                        this.results.extend(results.iter().cloned());
                        if *done {
                            this.pending = None;
                            this.limited = *limited;
                        }
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
            all_text_files: false,
            searched: false,
            pending: None,
            results: Vec::new(),
            limited: false,
            error: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Sets the query and runs the search, as pressing Enter in the query box does.
    #[cfg(test)]
    pub fn run_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |input, cx| {
            input.set_value(query.to_owned(), window, cx)
        });
        self.search(cx);
    }

    #[cfg(test)]
    pub fn result_count(&self) -> Option<usize> {
        self.pending.is_none().then_some(self.results.len())
    }

    #[cfg(test)]
    pub fn set_replacement_for_test(
        &mut self,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replacement.update(cx, |input, cx| {
            input.set_value(replacement.to_owned(), window, cx)
        });
    }

    /// Previews replacing the result at `index`, as its Replace button does.
    #[cfg(test)]
    pub fn replace_result(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.replace(Some(index), window, cx);
    }

    /// Scrolls the results to the one at `index`.
    #[cfg(test)]
    pub fn reveal_result(&self, index: usize, cx: &mut Context<Self>) {
        self.scroll.scroll_to_item(index, ScrollStrategy::Top);
        cx.notify();
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
            all_text_files: self.all_text_files,
        }
    }

    /// Runs the search, its results replacing those listed; the engine stops any search still
    /// running.
    fn search(&mut self, cx: &mut Context<Self>) {
        let request = self.request(cx);
        if request.query.is_empty() {
            return;
        }
        self.searched = true;
        let result = self.app.update(cx, |app, _| app.engine.search(request));
        match result {
            Ok(id) => {
                self.pending = Some(id);
                self.results.clear();
                self.limited = false;
                self.error = None;
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            Err(error) => self.error = Some(error.to_string().into()),
        }
        cx.notify();
    }

    /// Sets an option and, once a search has been run, runs it again, so the results listed
    /// are always those the options find, which a replace acts on.
    fn set_option(&mut self, option: SearchOption, checked: bool, cx: &mut Context<Self>) {
        let flag = match option {
            SearchOption::Regex => &mut self.regex,
            SearchOption::CaseSensitive => &mut self.case_sensitive,
            SearchOption::WholeWord => &mut self.whole_word,
            SearchOption::AllTextFiles => &mut self.all_text_files,
        };
        *flag = checked;
        if self.searched {
            self.search(cx);
        } else {
            cx.notify();
        }
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace(None, window, cx);
    }

    /// Previews replacing every match, or only the result at `only`. Once the preview is
    /// applied the search runs again, since the results' ranges in the files it changed are
    /// out of date.
    fn replace(&mut self, only: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.request(cx);
        if request.query.is_empty() {
            return;
        }
        let replacement = self.replacement.read(cx).value().to_string();
        let mut args = serde_json::json!({
            "request": request,
            "replacement": replacement,
        });
        if let Some(found) = only.and_then(|index| self.results.get(index)) {
            args["matches"] = serde_json::json!([{
                "path": found.path,
                "start": found.range.start,
                "end": found.range.end,
            }]);
        }
        let outcome = self.app.update(cx, |app, cx| {
            app.invoke(ids::WORKSPACE_REPLACE, CommandArgs::Custom(args), cx)
        });
        match outcome {
            Ok(CommandOutcome::Preview(preview)) => {
                let this = cx.weak_entity();
                let search_again = move |cx: &mut gpui::App| {
                    let _ = this.update(cx, |this, cx| this.search(cx));
                };
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.show_preview_then(preview, search_again, window, cx);
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
                workspace.open_at(
                    &found.path,
                    found.range,
                    adoc_engine::OpenDisposition::Preview,
                    window,
                    cx,
                );
            });
        }
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let roots = self.app.read(cx).engine.roots().to_vec();
        let status: SharedString = if let Some(error) = &self.error {
            error.clone()
        } else if self.pending.is_some() && self.results.is_empty() {
            "Searching…".into()
        } else {
            let files = {
                let mut paths: Vec<_> = self.results.iter().map(|found| &found.path).collect();
                paths.dedup();
                paths.len()
            };
            let found = format!("{} result(s) in {files} file(s)", self.results.len());
            if self.pending.is_some() {
                format!("Searching… {found} so far").into()
            } else if self.limited {
                format!("The first {found}; narrow the search to find the rest").into()
            } else {
                found.into()
            }
        };
        let option = |id: &'static str, label: &'static str, checked: bool, option| {
            Checkbox::new(id)
                .debug_selector(move || id.into())
                .label(label)
                .checked(checked)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    this.set_option(option, *checked, cx);
                }))
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
                    .child(option(
                        "search-regex",
                        ".*",
                        self.regex,
                        SearchOption::Regex,
                    ))
                    .child(option(
                        "search-case",
                        "Aa",
                        self.case_sensitive,
                        SearchOption::CaseSensitive,
                    ))
                    .child(option(
                        "search-word",
                        "Word",
                        self.whole_word,
                        SearchOption::WholeWord,
                    ))
                    .child(
                        option(
                            "search-all-files",
                            "All files",
                            self.all_text_files,
                            SearchOption::AllTextFiles,
                        )
                        .tooltip("Search every text file, not only the AsciiDoc pages"),
                    )
                    .child(
                        Button::new("search-run")
                            .primary()
                            .small()
                            .label("Find")
                            .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
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
                uniform_list(
                    "search-results",
                    self.results.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|index| this.results.get(index).map(|found| (index, found)))
                            .map(|(index, found)| {
                                let label = format!(
                                    "{}:{}  {}",
                                    display_path(&found.path, &roots),
                                    found.line + 1,
                                    found.line_text.trim()
                                );
                                h_flex()
                                    .id(("search-result", index))
                                    .debug_selector(move || format!("search-result-{index}"))
                                    .px_1()
                                    .gap_2()
                                    .text_sm()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(cx.theme().accent))
                                    .when(index % 2 == 1, |row| row.bg(cx.theme().muted))
                                    // One line each, as the list needs rows of one height.
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(label),
                                    )
                                    .child(
                                        Button::new(("replace-one", index))
                                            .xsmall()
                                            .ghost()
                                            .label("Replace")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.replace(Some(index), window, cx);
                                            })),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_result(index, window, cx);
                                    }))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h(px(0.)),
            )
    }
}
