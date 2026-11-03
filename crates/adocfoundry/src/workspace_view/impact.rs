//! The impact inspector (#311): the Impact panel, a sidebar tab shown once an analysis has been
//! asked for. It draws the engine's kept `ImpactInspection` and works nothing out itself: the
//! analysis runs on the engine's worker, and the panel only filters what the report holds.
//! Results are grouped as the report's kinds are (rendered content, references to review,
//! navigation, publications); each names its dependency path, a render dependency (what
//! includes what) apart from a reader path (what links where); a click opens where it is
//! written, and Graph shows that path in the graph window under its own title, never as a
//! shortest path.

use adoc_engine::{
    impact::{
        Completeness, Confidence, Impact, ImpactKind, ImpactReport, ImpactStatus, ReportSource,
    },
    ChangeSource, InspectionState,
};

use gpui::AnyElement;
use gpui_component::Selectable as _;

use super::*;
use crate::editor::bindings::{AnalyseImpact, CompareRevisions, InspectImpact};

/// The kinds, in the order the panel groups them, with their headings.
const GROUPS: [(ImpactKind, &str); 4] = [
    (ImpactKind::Render, "Rendered content"),
    (ImpactKind::ReferenceIntegrity, "References to review"),
    (ImpactKind::Navigation, "Navigation"),
    (ImpactKind::Publication, "Publications"),
];

/// A component version as the filter names it: the component and its version, `None` for the
/// versionless one.
pub(super) type ImpactFacet = (String, Option<String>);

fn facet_label((component, version): &ImpactFacet) -> String {
    match version {
        Some(version) => format!("{component} {version}"),
        None => component.clone(),
    }
}

fn status_text(status: ImpactStatus) -> &'static str {
    match status {
        ImpactStatus::Broken => "broken",
        ImpactStatus::ResolvedByChange => "resolved by the change",
        ImpactStatus::ResolvedByAlias => "resolved by page-aliases",
        ImpactStatus::NowResolves => "now resolves",
        ImpactStatus::Listed => "now listed",
        ImpactStatus::Unlisted => "no longer listed",
    }
}

/// A root-relative path's file name.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// An impact's path in words: what includes what for rendered content (from the changed file
/// to the page), or what a reader follows (from the page that links to the changed file).
fn path_text(impact: &Impact) -> Option<String> {
    if impact.via.len() < 2 {
        return None;
    }
    let names: Vec<&str> = impact.via.iter().map(|path| file_name(path)).collect();
    Some(match impact.kind {
        ImpactKind::Render | ImpactKind::Publication => {
            format!(
                "Render dependency: {} is in {}",
                names[0],
                names[1..].join(", in ")
            )
        }
        ImpactKind::ReferenceIntegrity | ImpactKind::Navigation => {
            let mut reader: Vec<&str> = names.clone();
            reader.reverse();
            format!("Reader path: {}", reader.join(" → "))
        }
    })
}

/// The title the graph window gives an impact's path.
fn trail_title(impact: &Impact) -> &'static str {
    match impact.kind {
        ImpactKind::Render | ImpactKind::Publication => "Render dependency",
        ImpactKind::ReferenceIntegrity | ImpactKind::Navigation => "Reader path",
    }
}

/// What the inspection compared, in words.
fn source_text(
    source: &ChangeSource,
    report: Option<&ImpactReport>,
    engine: &adoc_engine::Engine,
) -> String {
    match source {
        ChangeSource::WorkingTree => {
            let unsaved = matches!(
                report.and_then(|report| report.source.as_ref()),
                Some(ReportSource::WorkingTree {
                    unsaved_buffers: true,
                    ..
                })
            );
            if unsaved {
                "Changes since HEAD, unsaved edits included".to_owned()
            } else {
                "Changes since HEAD".to_owned()
            }
        }
        ChangeSource::GitRefs { base, head } => format!("Between {base} and {head}"),
        ChangeSource::Removal(paths) => {
            let names: Vec<String> = paths
                .iter()
                .map(|path| {
                    engine
                        .resource_id(path)
                        .unwrap_or_else(|| path.display().to_string())
                })
                .collect();
            format!("If {} were moved or removed", names.join(", "))
        }
        ChangeSource::Paths(_) => "The files named".to_owned(),
    }
}

impl WorkspaceView {
    pub(super) fn on_analyse_impact(
        &mut self,
        _: &AnalyseImpact,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .invoke(ids::IMPACT_ANALYSE_CHANGES, CommandArgs::None, window, cx)
            .is_some()
        {
            self.show_impact(cx);
        }
    }

    pub(super) fn on_inspect_impact(
        &mut self,
        _: &InspectImpact,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .invoke(ids::IMPACT_INSPECT, CommandArgs::None, window, cx)
            .is_some()
        {
            self.show_impact(cx);
        }
    }

    /// Inspect Impact for the file at `path`, from a tab's or the Files panel's menu.
    pub(crate) fn inspect_impact_of(&mut self, path: &Path, cx: &mut Context<Self>) {
        let asked = self.app.update(cx, |app, cx| {
            app.invoke(
                ids::IMPACT_INSPECT,
                CommandArgs::Path(path.to_path_buf()),
                cx,
            )
        });
        if asked.is_ok() {
            self.show_impact(cx);
        }
    }

    /// Asks for the base and head revisions to compare.
    pub(super) fn on_compare_revisions(
        &mut self,
        _: &CompareRevisions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = |value: &str, placeholder: &'static str, window: &mut Window, cx: &mut App| {
            let value = value.to_owned();
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(value, window, cx);
                state
            })
        };
        let (base, head) = match self
            .app
            .read(cx)
            .engine
            .impact_inspection()
            .map(|inspection| &inspection.source)
        {
            Some(ChangeSource::GitRefs { base, head }) => (base.clone(), head.clone()),
            _ => ("main".to_owned(), "HEAD".to_owned()),
        };
        let base = input(&base, "Base: a branch, tag or commit", window, cx);
        let head = input(&head, "Head: a branch, tag or commit", window, cx);
        let first = base.clone();
        let workspace = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (base, head, workspace) = (base.clone(), head.clone(), workspace.clone());
            dialog
                .title("Compare Revisions")
                .child(
                    v_flex()
                        .gap_2()
                        .w(px(360.))
                        .child(div().text_xs().child("Base"))
                        .child(
                            div()
                                .debug_selector(|| "impact-base".to_owned())
                                .child(Input::new(&base)),
                        )
                        .child(div().text_xs().child("Head"))
                        .child(
                            div()
                                .debug_selector(|| "impact-head".to_owned())
                                .child(Input::new(&head)),
                        ),
                )
                .confirm()
                .ok_text("Compare")
                .on_ok(move |_, window, cx| {
                    let base = base.read(cx).value().trim().to_owned();
                    let head = head.read(cx).value().trim().to_owned();
                    if base.is_empty() || head.is_empty() {
                        return false;
                    }
                    let _ = workspace.update(cx, |this, cx| {
                        if this
                            .invoke(
                                ids::IMPACT_COMPARE_REVISIONS,
                                CommandArgs::Revisions { base, head },
                                window,
                                cx,
                            )
                            .is_some()
                        {
                            this.show_impact(cx);
                        }
                    });
                    true
                })
        });
        first.update(cx, |input, cx| input.focus(window, cx));
    }

    fn show_impact(&mut self, cx: &mut Context<Self>) {
        self.sidebar_tab = SidebarTab::Impact;
        cx.notify();
    }

    /// Asks again what the kept analysis asked.
    fn impact_again(&mut self, cx: &mut Context<Self>) {
        let source = self
            .app
            .read(cx)
            .engine
            .impact_inspection()
            .map(|inspection| inspection.source.clone());
        if let Some(source) = source {
            self.app
                .update(cx, |app, _| app.engine.inspect_impact(source));
            cx.notify();
        }
    }

    /// Opens where `impact` is written: the reference or entry, else the affected file.
    fn open_impact(&mut self, impact: &Impact, cx: &mut Context<Self>) {
        let Some(place) = self.app.read(cx).engine.impact_place(impact) else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let _ = app.invoke(ids::NAVIGATE_GO_TO, CommandArgs::Position(place), cx);
        });
    }

    /// Shows `impact`'s dependency path in the graph window.
    fn show_impact_path(&mut self, impact: &Impact, cx: &mut Context<Self>) {
        let steps = self.app.read(cx).engine.impact_trail(impact);
        if let Some(view) = self.open_graph_window(cx) {
            let title = trail_title(impact);
            view.update(cx, |view, cx| view.show_trail(title, steps, cx));
        }
    }

    /// The Impact panel: what was compared, how complete the analysis is and whether it is out
    /// of date, a component version filter, and the impacts grouped by kind.
    pub(super) fn render_impact(&self, cx: &mut Context<Self>) -> AnyElement {
        let engine = &self.app.read(cx).engine;
        let Some(inspection) = engine.impact_inspection() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let report = match &inspection.state {
            InspectionState::Ready(report) => Some(report.as_ref()),
            _ => None,
        };
        let header = v_flex()
            .p_2()
            .gap_1()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .debug_selector(|| "impact-source".to_owned())
                            .child(source_text(&inspection.source, report, engine)),
                    )
                    .child(
                        Button::new("impact-again")
                            .label("Again")
                            .xsmall()
                            .ghost()
                            .debug_selector(|| "impact-again".to_owned())
                            .on_click(cx.listener(|this, _, _, cx| this.impact_again(cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("impact-changes")
                            .label("Changes")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.on_analyse_impact(&AnalyseImpact, window, cx);
                            })),
                    )
                    .child(
                        Button::new("impact-revisions")
                            .label("Revisions…")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.on_compare_revisions(&CompareRevisions, window, cx);
                            })),
                    ),
            );
        let status = |text: String, selector: &'static str| {
            div()
                .px_2()
                .text_xs()
                .text_color(muted)
                .debug_selector(move || selector.to_owned())
                .child(text)
        };
        let report = match &inspection.state {
            InspectionState::Running => {
                return v_flex()
                    .size_full()
                    .child(header)
                    .child(status("Analysing…".to_owned(), "impact-status"))
                    .into_any_element();
            }
            InspectionState::Failed(error) => {
                return v_flex()
                    .size_full()
                    .child(header)
                    .child(status(
                        format!("Could not analyse: {error}"),
                        "impact-status",
                    ))
                    .into_any_element();
            }
            InspectionState::Ready(report) => report.as_ref(),
        };
        let mut notes = v_flex().gap_0p5();
        notes = notes.child(status(
            match &report.completeness {
                Completeness::Complete => "Complete.".to_owned(),
                Completeness::Partial { .. } => {
                    "Partial: not everything could be analysed.".to_owned()
                }
                Completeness::Unavailable { .. } => "Unavailable.".to_owned(),
            },
            "impact-status",
        ));
        if let Completeness::Partial { gaps } | Completeness::Unavailable { gaps } =
            &report.completeness
        {
            for gap in gaps {
                notes = notes.child(status(format!("· {}", gap.detail), "impact-gap"));
            }
        }
        if let Some(truncated) = report.truncated {
            notes = notes.child(status(
                format!("Showing {} of {}.", truncated.shown, truncated.total),
                "impact-truncated",
            ));
        }
        if engine.impact_stale() {
            notes = notes.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .debug_selector(|| "impact-stale".to_owned())
                    .child("Out of date: the workspace changed since. Choose Again."),
            );
        }

        // The component versions the impacts are in, to filter by.
        let mut facets: Vec<ImpactFacet> = report
            .impacts
            .iter()
            .filter_map(|impact| Some((impact.component.clone()?, impact.version.clone())))
            .collect();
        facets.sort();
        facets.dedup();
        let filter = self
            .impact_filter
            .clone()
            .filter(|chosen| facets.contains(chosen));
        let filters = (facets.len() > 1).then(|| {
            let chip =
                |id: SharedString, label: String, chosen: bool, value: Option<ImpactFacet>| {
                    Button::new(id.clone())
                        .label(label)
                        .xsmall()
                        .ghost()
                        .selected(chosen)
                        .debug_selector(move || id.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.impact_filter = value.clone();
                            cx.notify();
                        }))
                };
            h_flex()
                .flex_wrap()
                .gap_1()
                .px_2()
                .child(chip(
                    "impact-filter-all".into(),
                    "All".to_owned(),
                    filter.is_none(),
                    None,
                ))
                .children(facets.iter().enumerate().map(|(number, facet)| {
                    chip(
                        format!("impact-filter-{number}").into(),
                        facet_label(facet),
                        filter.as_ref() == Some(facet),
                        Some(facet.clone()),
                    )
                }))
        });

        let shown = |impact: &&Impact| {
            filter.as_ref().is_none_or(|(component, version)| {
                impact.component.as_ref() == Some(component) && impact.version == *version
            })
        };
        let mut list = v_flex().gap_1().pb_2();
        let mut number = 0usize;
        let mut any = false;
        for (kind, heading) in GROUPS {
            let impacts: Vec<&Impact> = report
                .impacts
                .iter()
                .filter(|impact| impact.kind == kind)
                .filter(shown)
                .collect();
            if impacts.is_empty() {
                continue;
            }
            any = true;
            list = list.child(
                div()
                    .px_2()
                    .pt_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(format!("{heading} ({})", impacts.len())),
            );
            for impact in impacts {
                let row = number;
                number += 1;
                let name = impact
                    .resource_id
                    .clone()
                    .unwrap_or_else(|| impact.path.clone());
                let mut notes: Vec<String> = Vec::new();
                if let Some(anchor) = &impact.anchor {
                    notes.push(format!("#{anchor}"));
                }
                if let Some(status) = impact.status {
                    notes.push(status_text(status).to_owned());
                }
                if impact.confidence == Confidence::Potential {
                    notes.push("potential".to_owned());
                }
                if let Some(publication) = &impact.publication {
                    notes.push(format!("profile {publication}"));
                }
                let opened = impact.clone();
                let graphed = impact.clone();
                list = list.child(
                    h_flex()
                        .id(("impact-row", row))
                        .debug_selector(move || format!("impact-row-{row}"))
                        .px_2()
                        .py_0p5()
                        .gap_1()
                        .items_start()
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_impact(&opened, cx)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().truncate().child(name))
                                .when(!notes.is_empty(), |row| {
                                    row.child(
                                        div().text_xs().text_color(muted).child(notes.join(" · ")),
                                    )
                                })
                                .children(
                                    path_text(impact)
                                        .map(|text| div().text_xs().text_color(muted).child(text)),
                                ),
                        )
                        .when(impact.via.len() >= 2, |row_element| {
                            row_element.child(
                                Button::new(("impact-graph", row))
                                    .label("Graph")
                                    .xsmall()
                                    .ghost()
                                    .debug_selector(move || format!("impact-graph-{row}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.show_impact_path(&graphed, cx);
                                    })),
                            )
                        }),
                );
            }
        }
        if !any {
            list = list.child(status("Nothing is affected.".to_owned(), "impact-none"));
        }
        v_flex()
            .size_full()
            .child(header)
            .child(notes)
            .children(filters)
            .child(
                div()
                    .id("impact-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element()
    }
}
