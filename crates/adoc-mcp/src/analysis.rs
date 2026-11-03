//! The analysis tools: `quality_audit` (MCP PRD §7.3), the documentation quality audit as
//! `adocfoundry-cli audit` makes it, for one scope, under the workspace's own policy. The rules
//! stay in `adoc-audit` and the orchestration in the engine; this adapter only chooses the
//! scope and policy as the command line does, filters and pages the findings, and says plainly
//! when a passing report covers less than everything.

use adoc_engine::{
    audit::{AuditScope, Completeness, FailOn, FindingCode, Region, Severity, Verdict},
    Settings,
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    responses::{page, Failure},
    schemas,
    server::{Answer, Server},
};

#[derive(Deserialize, JsonSchema)]
struct QualityAuditArgs {
    /// Audit this component alone; the whole workspace without it.
    component: Option<String>,
    /// One version of `component`, such as `3.0`; every version of it without one. It needs a
    /// `component`.
    version: Option<String>,
    /// List only findings with these codes: rule ids such as `quality.page.missing-title`, or
    /// diagnostic codes such as `adoc.unresolved-xref-file`. Counts are the whole report's.
    rules: Option<Vec<String>>,
    /// List only findings at least this severe: `info`, `warning` or `error`.
    severity: Option<String>,
    /// Also list the findings the policy excludes, with their reasons; false by default.
    include_excluded: Option<bool>,
    /// Treat a partial audit as untrustworthy, as `--require-complete` does; the workspace's
    /// policy decides without it.
    require_complete: Option<bool>,
    /// The severity that fails the audit, as `--fail-on`: `none`, `warning` or `error`; the
    /// workspace's policy decides without it.
    fail_on: Option<String>,
    /// The `next_cursor` of the page before, to list the next findings; the first page without
    /// it.
    cursor: Option<String>,
    /// At most this many findings in the page, up to 500 (the default).
    limit: Option<usize>,
}

/// What a report means for a client deciding what to do: a pass is `pass` only when the audit
/// covered everything, so a partial report is never taken for an unqualified pass.
fn outcome(verdict: Verdict, completeness: &Completeness, policy_problems: bool) -> &'static str {
    if policy_problems {
        return "untrustworthy";
    }
    match (verdict, completeness) {
        (Verdict::Pass, Completeness::Complete) => "pass",
        (Verdict::Pass, _) => "pass_partial",
        (Verdict::Fail, _) => "fail",
        (Verdict::Untrustworthy, _) => "untrustworthy",
    }
}

#[tool_router(router = analysis_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Audit the documentation's quality, as `adocfoundry-cli audit --format json` does, under the workspace's own `.adocfoundry.toml` policy: the report's counts, rules, findings (a page at a time, with byte ranges and lines), skipped dynamic constructs, exclusions, navigation coverage and completeness, and its outcome. `pass_partial` means nothing failed in what was audited, but not everything was. Runs no optional provider (Vale, external links) and makes no network request.",
        output_schema = schemas::quality_audit(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn quality_audit(&self, Parameters(args): Parameters<QualityAuditArgs>) -> Answer {
        self.run(move |state| {
            state.indexed()?;
            let scope = match (&args.component, &args.version) {
                (None, None) => AuditScope::Workspace,
                (None, Some(_)) => {
                    return Err(Failure::invalid(
                        "a `version` needs its `component`, so no other is taken",
                    ))
                }
                (Some(_), Some(version)) if version == "~" => {
                    return Err(Failure::invalid(
                        "an audit's scope names a numbered version, or every version of the \
                         component, the versionless one included: leave `version` out",
                    ))
                }
                (Some(name), version) => {
                    let catalog = &state.engine.workspace().antora;
                    let known = match version {
                        Some(version) => catalog.component(name, Some(version)).is_some(),
                        None => !catalog.versions_of(name).is_empty(),
                    };
                    if !known {
                        return Err(Failure::not_found(format!(
                            "no component `{name}`{} in the workspace",
                            version
                                .as_deref()
                                .map(|version| format!(" at version `{version}`"))
                                .unwrap_or_default()
                        )));
                    }
                    AuditScope::Component {
                        name: name.clone(),
                        version: version.clone(),
                    }
                }
            };
            let codes = args
                .rules
                .iter()
                .flatten()
                .map(|name| {
                    FindingCode::from_name(name).ok_or_else(|| {
                        Failure::invalid(format!("unknown rule or diagnostic code `{name}`"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let floor = args
                .severity
                .as_deref()
                .map(|name| {
                    Severity::from_name(name).ok_or_else(|| {
                        Failure::invalid(format!(
                            "unknown severity `{name}`; use info, warning or error"
                        ))
                    })
                })
                .transpose()?;
            let fail_on = args
                .fail_on
                .as_deref()
                .map(|name| {
                    FailOn::from_name(name).ok_or_else(|| {
                        Failure::invalid(format!(
                            "unknown fail_on level `{name}`; use none, warning or error"
                        ))
                    })
                })
                .transpose()?;

            // The policy is the workspace's own, as the command line reads it: never the user's
            // settings file, so every client and CI see the same result.
            let quality = Settings::load(None, Some(state.root()))
                .map_err(|error| Failure::invalid(format!("the workspace's settings: {error}")))?
                .quality;
            let mut policy = quality.policy.clone();
            if let Some(fail_on) = fail_on {
                policy.fail_on = fail_on;
            }
            if args.require_complete == Some(true) {
                policy.require_complete = true;
            }
            let revision = state.engine.workspace_revision();
            let report = state.engine.audit_now_with(scope.clone(), &policy);
            let verdict = report.verdict();
            let outcome = outcome(verdict, report.completeness(), !quality.problems.is_empty());
            let mut value = serde_json::to_value(&report)
                .map_err(|error| Failure::internal(error.to_string()))?;

            let mut names: Vec<&str> = codes.iter().map(|code| code.as_str()).collect();
            names.sort_unstable();
            let include_excluded = args.include_excluded.unwrap_or(false);
            let query = format!(
                "quality_audit\u{1f}{scope:?}\u{1f}{}\u{1f}{floor:?}\u{1f}{include_excluded}\u{1f}{:?}\u{1f}{}",
                names.join(","),
                policy.fail_on,
                policy.require_complete,
            );
            let findings: Vec<Value> = report
                .findings()
                .iter()
                .zip(value["findings"].as_array().cloned().unwrap_or_default())
                .filter(|(finding, _)| codes.is_empty() || codes.contains(&finding.code))
                .filter(|(finding, _)| floor.is_none_or(|floor| finding.severity >= floor))
                .map(|(finding, mut json)| {
                    let location = &finding.location;
                    if let (Some(range), Some(root)) =
                        (location.range, state.roots.get(location.root))
                    {
                        if let Ok(text) = state.text(&root.join(&location.path)) {
                            let region = Region::of(&text, range);
                            json["region"] = json!({
                                "start_line": region.start_line,
                                "start_column": region.start_column,
                                "end_line": region.end_line,
                                "end_column": region.end_column,
                            });
                        }
                    }
                    json
                })
                .collect();
            let page = page(findings, &query, revision, args.cursor.as_deref(), args.limit)?;
            value["findings"] = json!(page.items);
            value["findings_total"] = json!(page.total);
            value["next_cursor"] = json!(page.next_cursor);
            if page.next_cursor.is_some() {
                value["truncated"] = json!({ "shown": page.items.len(), "total": page.total });
            }
            if !include_excluded {
                if let Some(object) = value.as_object_mut() {
                    object.remove("excluded");
                }
            }
            value["verdict"] = json!(match verdict {
                Verdict::Pass => "pass",
                Verdict::Fail => "fail",
                Verdict::Untrustworthy => "untrustworthy",
            });
            value["exit_code"] = json!(if quality.problems.is_empty() {
                verdict.exit_code()
            } else {
                2
            });
            value["outcome"] = json!(outcome);
            value["policy_problems"] = json!(quality.problems);
            value["workspace_revision"] = json!(revision);
            Ok(value)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_pass_is_never_a_plain_pass() {
        let partial = Completeness::Partial(Vec::new());
        assert_eq!(
            outcome(Verdict::Pass, &Completeness::Complete, false),
            "pass"
        );
        assert_eq!(outcome(Verdict::Pass, &partial, false), "pass_partial");
        assert_eq!(outcome(Verdict::Fail, &partial, false), "fail");
        assert_eq!(
            outcome(Verdict::Untrustworthy, &partial, false),
            "untrustworthy"
        );
        assert_eq!(
            outcome(Verdict::Pass, &Completeness::Complete, true),
            "untrustworthy",
            "an invalid policy is no result"
        );
    }
}
