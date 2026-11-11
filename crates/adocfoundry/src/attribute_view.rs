//! Inspect Attribute: where the value of the attribute at the caret comes from, row by row as
//! the engine words it; a click opens the place a row is written.

use adoc_engine::{AttributeInspection, InspectionRowKind};
use gpui::{
    div, prelude::FluentBuilder as _, px, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _,
};

use crate::{app::AppState, workspace_view::WorkspaceView};

pub struct AttributeView {
    workspace: gpui::WeakEntity<WorkspaceView>,
    /// None when the caret was on no attribute.
    inspection: Option<AttributeInspection>,
}

impl AttributeView {
    pub fn new(_app: Entity<AppState>, workspace: gpui::WeakEntity<WorkspaceView>) -> Self {
        Self {
            workspace,
            inspection: None,
        }
    }

    pub fn show(&mut self, inspection: Option<AttributeInspection>, cx: &mut Context<Self>) {
        self.inspection = inspection;
        cx.notify();
    }

    fn summary(&self) -> String {
        self.inspection.as_ref().map_or_else(
            || "No attribute at the caret".to_owned(),
            |inspection| inspection.summary.clone(),
        )
    }

    /// The summary, then each row as shown.
    #[cfg(test)]
    pub fn rows_for_test(&self) -> Vec<String> {
        std::iter::once(self.summary())
            .chain(self.inspection.iter().flat_map(|inspection| {
                inspection.rows.iter().map(|row| {
                    let mark = if row.wins() { "✓ " } else { "" };
                    format!(
                        "{mark}{}  {}  {}  {}",
                        row.source,
                        row.value,
                        row.status,
                        row.place.as_deref().unwrap_or("")
                    )
                    .trim_end()
                    .to_owned()
                })
            }))
            .collect()
    }

    fn open(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(location) = self
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.rows.get(index))
            .and_then(|row| row.location.clone())
        else {
            return;
        };
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_at(
                    &location.path,
                    location.range,
                    adoc_engine::OpenDisposition::Preview,
                    window,
                    cx,
                );
            });
        }
    }
}

impl Render for AttributeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary: SharedString = self.summary().into();
        let rows = self
            .inspection
            .as_ref()
            .map(|inspection| inspection.rows.clone())
            .unwrap_or_default();
        let first_includer = rows
            .iter()
            .position(|row| row.kind == InspectionRowKind::Includer);
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
                        Button::new("attribute-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(AttributeEvent::Close);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("attribute-rows")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(rows.into_iter().enumerate().flat_map(|(index, row)| {
                        let heading = (Some(index) == first_includer).then(|| {
                            div()
                                .pt_2()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("In the pages that include this file")
                                .into_any_element()
                        });
                        let wins = row.wins();
                        let line = h_flex()
                            .id(("attribute-row", index))
                            .debug_selector(move || format!("attribute-row-{index}"))
                            .gap_3()
                            .px_2()
                            .text_sm()
                            .when(row.location.is_some(), |line| {
                                line.cursor_pointer()
                                    .hover(|style| style.bg(cx.theme().accent))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open(index, window, cx);
                                    }))
                            })
                            .when(wins, |line| {
                                line.font_weight(gpui::FontWeight::SEMIBOLD)
                                    .bg(cx.theme().muted)
                            })
                            .child(div().w(px(16.)).child(if wins { "✓" } else { "" }))
                            .child(div().w(px(220.)).truncate().child(row.source))
                            .child(div().w(px(160.)).truncate().child(row.value))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(row.status),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(row.place.unwrap_or_default()),
                            )
                            .into_any_element();
                        heading.into_iter().chain(std::iter::once(line))
                    })),
            )
    }
}

/// What the panel asks of the window around it.
pub enum AttributeEvent {
    Close,
}

impl gpui::EventEmitter<AttributeEvent> for AttributeView {}
