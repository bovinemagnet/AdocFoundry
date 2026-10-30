//! Find References: the references to an anchor or a page, grouped by file; a click opens one.

use adoc_engine::SearchMatch;
use gpui::{
    div, prelude::FluentBuilder as _, px, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _,
};

use crate::{app::AppState, sidebar::display_path, workspace_view::WorkspaceView};

/// What the panel lists, row by row.
enum Row {
    File(String),
    /// A reference, by its index among the results, with its line.
    Reference(usize, String),
}

pub struct ReferencesView {
    app: Entity<AppState>,
    workspace: gpui::WeakEntity<WorkspaceView>,
    results: Vec<SearchMatch>,
}

impl ReferencesView {
    pub fn new(app: Entity<AppState>, workspace: gpui::WeakEntity<WorkspaceView>) -> Self {
        Self {
            app,
            workspace,
            results: Vec::new(),
        }
    }

    /// Lists `results`, which are in file order.
    pub fn show(&mut self, results: Vec<SearchMatch>, cx: &mut Context<Self>) {
        self.results = results;
        cx.notify();
    }

    fn summary(&self) -> String {
        let mut files: Vec<_> = self.results.iter().map(|found| &found.path).collect();
        files.dedup();
        match (self.results.len(), files.len()) {
            (0, _) => "No references".to_owned(),
            (1, _) => "1 reference".to_owned(),
            (references, 1) => format!("{references} references in 1 file"),
            (references, files) => format!("{references} references in {files} files"),
        }
    }

    fn rows(&self, cx: &gpui::App) -> Vec<Row> {
        let roots = self.app.read(cx).engine.roots().to_vec();
        let mut rows = Vec::new();
        let mut file = None;
        for (index, found) in self.results.iter().enumerate() {
            if file != Some(&found.path) {
                file = Some(&found.path);
                rows.push(Row::File(display_path(&found.path, &roots)));
            }
            rows.push(Row::Reference(
                index,
                format!("{}  {}", found.line + 1, found.line_text.trim()),
            ));
        }
        rows
    }

    /// The summary, then each file and its references, as shown.
    #[cfg(test)]
    pub fn rows_for_test(&self, cx: &gpui::App) -> Vec<String> {
        std::iter::once(self.summary())
            .chain(self.rows(cx).into_iter().map(|row| match row {
                Row::File(label) | Row::Reference(_, label) => label,
            }))
            .collect()
    }

    fn open(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
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

impl Render for ReferencesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary: SharedString = self.summary().into();
        let rows = self.rows(cx);
        v_flex()
            .size_full()
            .gap_1()
            .p_2()
            .bg(cx.theme().background)
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1().child(summary))
                    .child(
                        Button::new("references-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(ReferencesEvent::Close);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("references")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(rows.into_iter().map(|row| {
                        match row {
                            Row::File(label) => div()
                                .pt_1()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(label)
                                .into_any_element(),
                            Row::Reference(index, label) => h_flex()
                                .id(("reference", index))
                                .debug_selector(move || format!("reference-{index}"))
                                .pl_4()
                                .text_sm()
                                .cursor_pointer()
                                .hover(|style| style.bg(cx.theme().accent))
                                .when(index % 2 == 1, |row| row.bg(cx.theme().muted))
                                .child(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open(index, window, cx);
                                }))
                                .into_any_element(),
                        }
                    })),
            )
    }
}

/// What the panel asks of the window around it.
pub enum ReferencesEvent {
    Close,
}

impl gpui::EventEmitter<ReferencesEvent> for ReferencesView {}
