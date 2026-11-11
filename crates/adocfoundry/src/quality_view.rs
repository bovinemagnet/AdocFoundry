//! The Quality overview in the sidebar: the kept audit's status and counts, never its findings,
//! which the Problems panel lists. Every count that stands for problems opens Problems filtered
//! to them; skips and exclusions are counted here only, since Problems lists neither.
//!
//! The overview is built from the engine's report when the audit changes
//! ([`Overview::of`]), so drawing it reads nothing.

use std::collections::BTreeMap;

use adoc_core::DiagnosticSeverity;
use adoc_engine::{
    audit::{AuditReport, Completeness, FindingCode, ProviderState, RuleId},
    Engine, ProblemFilter,
};
use gpui::{
    div, prelude::FluentBuilder as _, AnyElement, App, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Sizable as _,
};

use crate::{
    sidebar::{severity_label, version_label},
    workspace_view::WorkspaceView,
};

/// A line of the overview, opening Problems with `filter` when it has one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub label: String,
    pub filter: Option<ProblemFilter>,
}

impl Row {
    fn text(label: String) -> Self {
        Self {
            label,
            filter: None,
        }
    }
}

/// A titled group of rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

/// What the overview shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Overview {
    /// Whether the audit is complete, partial or unavailable, or why there is none yet.
    pub status: String,
    /// Why coverage is missing, for a partial or unavailable audit.
    pub reasons: Vec<String>,
    /// The workspace has moved on since this result: a newer one is on its way.
    pub stale: bool,
    pub sections: Vec<Section>,
}

impl Overview {
    /// The overview of the engine's kept audit of the workspace.
    pub fn of(engine: &Engine) -> Self {
        let report = engine.audit_report(&adoc_engine::audit::AuditScope::Workspace);
        match report {
            Some(report) => Self::of_report(&report, engine.audit_stale()),
            None => Self {
                status: if engine.audit_enabled() {
                    "Auditing…".to_owned()
                } else {
                    "No audit has run in this workspace yet.".to_owned()
                },
                reasons: Vec::new(),
                stale: false,
                sections: Vec::new(),
            },
        }
    }

    /// The overview of `report`.
    pub fn of_report(report: &AuditReport, stale: bool) -> Self {
        let (status, gaps) = match report.completeness() {
            Completeness::Complete => ("Complete: everything in the workspace was audited", None),
            Completeness::Partial(gaps) => {
                ("Partial: part of the workspace was not audited", Some(gaps))
            }
            Completeness::Unavailable(gaps) => {
                ("Unavailable: nothing could be audited", Some(gaps))
            }
        };
        let reasons = gaps
            .into_iter()
            .flatten()
            .map(|gap| gap.detail.clone())
            .collect();

        let counts = report.counts();
        let severities = [
            (DiagnosticSeverity::Error, counts.error),
            (DiagnosticSeverity::Warning, counts.warning),
            (DiagnosticSeverity::Information, counts.info),
        ]
        .into_iter()
        .map(|(severity, count)| Row {
            label: format!("{}: {count}", severity_label(severity)),
            filter: (count > 0).then(|| ProblemFilter {
                severity: Some(severity),
                ..ProblemFilter::default()
            }),
        })
        .collect();

        let mut by_code: BTreeMap<&str, (FindingCode, usize)> = BTreeMap::new();
        for finding in report.findings() {
            by_code
                .entry(finding.code.as_str())
                .or_insert((finding.code, 0))
                .1 += 1;
        }
        let rules = by_code
            .into_values()
            .map(|(code, count)| Row {
                label: format!("{}: {count}", code.as_str()),
                filter: Some(ProblemFilter {
                    code: Some(code),
                    ..ProblemFilter::default()
                }),
            })
            .collect();

        let coverage = report
            .coverage()
            .iter()
            .map(|coverage| {
                let unlisted = coverage
                    .pages
                    .saturating_sub(coverage.listed + coverage.undetermined);
                let mut label = format!(
                    "{}: {} of {} pages listed",
                    version_label(&coverage.component, coverage.version.as_deref()),
                    coverage.listed,
                    coverage.pages
                );
                if coverage.undetermined > 0 {
                    label.push_str(&format!(", {} undetermined", coverage.undetermined));
                }
                if coverage.navigation_files == 0 {
                    label.push_str(", no navigation");
                }
                Row {
                    label,
                    filter: (unlisted > 0).then(|| ProblemFilter {
                        code: Some(FindingCode::Rule(RuleId::UNLISTED_PAGE)),
                        component: Some(coverage.component.clone()),
                        version: Some(coverage.version.clone()),
                        ..ProblemFilter::default()
                    }),
                }
            })
            .collect();

        let mut skipped: BTreeMap<&str, usize> = BTreeMap::new();
        for skip in report.skips() {
            *skipped.entry(skip.code.as_str()).or_default() += 1;
        }
        let skips = skipped
            .into_iter()
            .map(|(code, count)| Row::text(format!("{code}: {count}")))
            .collect();

        let excluded = report
            .excluded()
            .iter()
            .map(|excluded| {
                Row::text(format!(
                    "{} {}: {}",
                    excluded.finding.location.path,
                    excluded.finding.code.as_str(),
                    excluded.reason
                ))
            })
            .collect();

        // Only the providers the settings enable; the reason one could not run is a gap above.
        let providers = report
            .providers()
            .iter()
            .filter_map(|status| {
                let state = match status.state {
                    ProviderState::Disabled => return None,
                    ProviderState::Ran => "ran",
                    ProviderState::Unavailable { .. } => "unavailable",
                };
                Some(Row::text(format!(
                    "{}: {state} ({})",
                    status.provider,
                    status.provider.spec().summary
                )))
            })
            .collect();

        let sections = [
            ("Findings by severity", severities),
            ("Findings by rule", rules),
            ("Navigation coverage", coverage),
            ("Skipped, not known without a build", skips),
            ("Excluded by the settings", excluded),
            ("Optional providers", providers),
        ]
        .into_iter()
        .filter(|(_, rows): &(_, Vec<Row>)| !rows.is_empty())
        .map(|(title, rows)| Section { title, rows })
        .collect();
        Self {
            status: status.to_owned(),
            reasons,
            stale,
            sections,
        }
    }

    /// Every row's label, in order, for tests.
    #[cfg(test)]
    pub fn labels(&self) -> Vec<String> {
        self.sections
            .iter()
            .flat_map(|section| section.rows.iter().map(|row| row.label.clone()))
            .collect()
    }
}

/// The overview drawn: the status and Run Audit, then each section. Row `n` (counted across
/// sections) has the debug selector `quality-row-n`.
pub fn render(overview: &Overview, workspace: WeakEntity<WorkspaceView>, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let run = workspace.clone();
    let mut list = v_flex()
        .id("quality")
        .size_full()
        .overflow_y_scroll()
        .gap_1()
        .p_2()
        .text_sm()
        .child(
            h_flex()
                .gap_2()
                .child(div().flex_1().min_w_0().child(overview.status.clone()))
                .child(
                    Button::new("quality-run")
                        .label("Run Audit")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = run.upgrade() {
                                workspace.update(cx, |workspace, cx| {
                                    workspace.run_audit(window, cx);
                                });
                            }
                        }),
                ),
        )
        .when(overview.stale, |list| {
            list.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child("Out of date: the workspace has changed, and a new audit is running."),
            )
        })
        .children(
            overview
                .reasons
                .iter()
                .map(|reason| div().text_xs().text_color(muted).child(reason.clone())),
        );
    let mut index: usize = 0;
    for section in &overview.sections {
        list = list.child(
            div()
                .pt_2()
                .text_xs()
                .text_color(muted)
                .child(section.title),
        );
        for row in &section.rows {
            let selector = format!("quality-row-{index}");
            let label = SharedString::from(row.label.clone());
            let element = div()
                .id(("quality-row", index))
                .debug_selector(move || selector)
                .px_1()
                .rounded(cx.theme().radius)
                .truncate()
                .child(label);
            list = list.child(match row.filter.clone() {
                Some(filter) => {
                    let workspace = workspace.clone();
                    element
                        .cursor_pointer()
                        .text_color(cx.theme().link)
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .on_click(move |_, _, cx| {
                            if let Some(workspace) = workspace.upgrade() {
                                workspace.update(cx, |workspace, cx| {
                                    workspace.filter_problems(filter.clone(), cx);
                                });
                            }
                        })
                        .into_any_element()
                }
                None => element.into_any_element(),
            });
            index += 1;
        }
    }
    list.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_overview_counts_and_links_to_problems() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/audit");
        let mut engine = Engine::new(adoc_engine::Settings::default().with_git_opt_in());
        engine
            .open_workspace(vec![adoc_index::normalize_path(&root)])
            .unwrap();
        assert!(engine.wait_for_workers(std::time::Duration::from_secs(30)));
        let report = engine.audit_now(adoc_engine::audit::AuditScope::Workspace);
        let overview = Overview::of_report(&report, false);
        assert!(overview.status.starts_with("Complete"));
        let labels = overview.labels();
        for expected in [
            "Errors: 0",
            "Warnings: 7",
            "Information: 1",
            "quality.image.missing-alt: 3",
            "guide: 2 of 3 pages listed, 1 undetermined",
            "product 3.0: 7 of 8 pages listed",
            "quality.navigation.unlisted-page: 1",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "{expected} in {labels:#?}"
            );
        }
        let row = |label: &str| {
            overview
                .sections
                .iter()
                .flat_map(|section| &section.rows)
                .find(|row| row.label == label)
                .unwrap()
                .clone()
        };
        assert_eq!(row("Errors: 0").filter, None, "nothing to show");
        assert_eq!(
            row("product 3.0: 7 of 8 pages listed").filter,
            Some(ProblemFilter {
                code: Some(FindingCode::Rule(RuleId::UNLISTED_PAGE)),
                component: Some("product".to_owned()),
                version: Some(Some("3.0".to_owned())),
                ..ProblemFilter::default()
            })
        );
        assert_eq!(
            row("guide: 2 of 3 pages listed, 1 undetermined").filter,
            None,
            "the undetermined page is a skip, not a problem"
        );
        assert!(
            overview
                .sections
                .iter()
                .all(|section| section.title != "Optional providers"),
            "none is enabled"
        );

        let mut policy = adoc_engine::audit::Policy::default();
        policy.enable_provider("vale").unwrap();
        let report = engine.audit_now_with(adoc_engine::audit::AuditScope::Workspace, &policy);
        let overview = Overview::of_report(&report, false);
        assert!(overview.status.starts_with("Partial"));
        assert!(overview.reasons[0].contains("`vale` provider could not run"));
        assert!(overview
            .labels()
            .contains(&"vale: unavailable (Prose style alerts from Vale)".to_owned()));
    }
}
