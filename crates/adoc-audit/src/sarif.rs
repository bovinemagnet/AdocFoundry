//! A report as SARIF 2.1.0, for code-scanning tools.
//!
//! One run, by the tool `AdocFoundry`, with every rule a finding names in `tool.driver.rules`.
//! Findings are results in the report's order; an excluded finding is a result with an
//! `external` suppression giving the exclusion's reason. Locations are relative to their
//! workspace root (`uriBaseId` `ROOT<n>`), with `/` separators, and never absolute; a region is
//! 1-based lines and columns counted in characters (`columnKind` `unicodeCodePoints`), given by
//! the caller, who can read the files. Completeness is a property of the run, and each gap a
//! tool execution notification, so a partial audit is never mistaken for a clean one. Each
//! optional provider's state is a property of the run too.

use std::collections::BTreeMap;

use adoc_core::SourceRange;
use serde_json::{json, Value};

use crate::{AuditReport, Completeness, Finding, FindingCode, Location, Severity};

/// A region of a file in 1-based lines and character columns, the end exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl Region {
    /// The region of `range` in `text`, counting columns in characters. Offsets past the end, or
    /// inside a character, are moved back to a character boundary.
    #[must_use]
    pub fn of(text: &str, range: SourceRange) -> Self {
        let place = |offset: usize| {
            let mut offset = offset.min(text.len());
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            let before = &text[..offset];
            let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
            (
                before.matches('\n').count() + 1,
                text[line_start..offset].chars().count() + 1,
            )
        };
        let (start_line, start_column) = place(range.start);
        let (end_line, end_column) = place(range.end.max(range.start));
        Self {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }
}

/// `report` as a SARIF log. `regions` gives the region of a location with a range, if the file
/// can be read; a location without one names only the file.
pub fn to_sarif(
    report: &AuditReport,
    tool_version: &str,
    regions: &dyn Fn(&Location) -> Option<Region>,
) -> Value {
    let mut rules: BTreeMap<&str, Value> = BTreeMap::new();
    let mut results = Vec::new();
    let mut add = |finding: &Finding, suppression: Option<&str>| {
        rules
            .entry(finding.code.as_str())
            .or_insert_with(|| rule(finding.code));
        let mut result = json!({
            "ruleId": finding.code.as_str(),
            "level": level(finding.severity),
            "message": { "text": finding.message },
            "locations": [location(&finding.location, regions)],
        });
        if let Some(coordinate) = &finding.coordinate {
            result["properties"] = json!({ "coordinate": coordinate });
        }
        if let Some(reason) = suppression {
            result["suppressions"] = json!([{ "kind": "external", "justification": reason }]);
        }
        results.push(result);
    };
    for finding in report.findings() {
        add(finding, None);
    }
    for excluded in report.excluded() {
        add(&excluded.finding, Some(&excluded.reason));
    }

    let (status, gaps) = match report.completeness() {
        Completeness::Complete => ("complete", &[][..]),
        Completeness::Partial(gaps) => ("partial", gaps.as_slice()),
        Completeness::Unavailable(gaps) => ("unavailable", gaps.as_slice()),
    };
    let notifications: Vec<Value> = gaps
        .iter()
        .map(|gap| {
            let reason = serde_json::to_value(gap.reason).unwrap_or(Value::Null);
            json!({
                "level": "warning",
                "message": { "text": gap.detail },
                "descriptor": { "id": reason },
                "properties": { "affected": gap.affected },
            })
        })
        .collect();
    let base_ids: serde_json::Map<String, Value> = report
        .roots()
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                format!("ROOT{index}"),
                json!({ "description": { "text": format!("The workspace root `{name}`") } }),
            )
        })
        .collect();

    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "AdocFoundry",
                    "version": tool_version,
                    "informationUri": "https://github.com/bovinemagnet/AdocFoundry",
                    "rules": rules.into_values().collect::<Vec<_>>(),
                }
            },
            "originalUriBaseIds": base_ids,
            "columnKind": "unicodeCodePoints",
            "invocations": [{
                "executionSuccessful": true,
                "toolExecutionNotifications": notifications,
            }],
            "results": results,
            "properties": {
                "schemaVersion": report.schema_version(),
                "completeness": status,
                "providers": report.providers(),
                "counts": report.counts(),
                "coverage": report.coverage(),
            },
        }]
    })
}

fn rule(code: FindingCode) -> Value {
    match code {
        FindingCode::Rule(rule) => {
            let spec = rule.spec();
            json!({
                "id": rule.as_str(),
                "shortDescription": { "text": spec.summary },
                "defaultConfiguration": { "level": level(spec.default_severity) },
            })
        }
        FindingCode::Diagnostic(code) => json!({
            "id": code.as_str(),
            "shortDescription": { "text": "An AdocFoundry diagnostic, reported as the editor reports it" },
        }),
    }
}

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

fn location(location: &Location, regions: &dyn Fn(&Location) -> Option<Region>) -> Value {
    let mut physical = json!({
        "artifactLocation": {
            "uri": uri(&location.path),
            "uriBaseId": format!("ROOT{}", location.root),
        }
    });
    if location.range.is_some() {
        if let Some(region) = regions(location) {
            physical["region"] = json!({
                "startLine": region.start_line,
                "startColumn": region.start_column,
                "endLine": region.end_line,
                "endColumn": region.end_column,
            });
        }
    }
    json!({ "physicalLocation": physical })
}

/// A relative path as a URI reference: each segment percent-encoded where a URI needs it.
fn uri(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_count_lines_and_characters_from_one() {
        let text = "= Café\n\nimage::a.png[]\n";
        let start = text.find("image").unwrap();
        assert_eq!(
            Region::of(text, SourceRange::new(start, start + 5)),
            Region {
                start_line: 3,
                start_column: 1,
                end_line: 3,
                end_column: 6,
            }
        );
        let after = text.find('é').unwrap() + 'é'.len_utf8();
        let region = Region::of(text, SourceRange::new(after, after));
        assert_eq!(
            (region.start_line, region.start_column),
            (1, 7),
            "é is one column"
        );
        let region = Region::of(text, SourceRange::new(after - 1, 999));
        assert_eq!(
            region.start_column, 6,
            "inside a character moves back to its start"
        );
        assert_eq!(region.end_line, 4, "past the end is the end");
    }

    #[test]
    fn uris_are_percent_encoded_relative_paths() {
        assert_eq!(
            uri("modules/ROOT/pages/a b.adoc"),
            "modules/ROOT/pages/a%20b.adoc"
        );
        assert_eq!(uri("pages/é.adoc"), "pages/%C3%A9.adoc");
    }
}
