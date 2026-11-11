//! The `quality_audit` tool (MCP PRD §§7.3 and 11.2, tests 8, 9 and 13), driven through the
//! binary over stdio on copies of `tests/fixtures/audit`, against what `adocfoundry-cli audit`
//! reports for the same fixture.

mod common;

use std::path::Path;

use adoc_engine::{audit::AuditScope, Engine, Settings};
use common::{scratch, Client};
use serde_json::{json, Value};

/// The fields the tool adds to the report it projects.
const ADDED: [&str; 8] = [
    "findings_total",
    "next_cursor",
    "truncated",
    "verdict",
    "exit_code",
    "outcome",
    "policy_problems",
    "workspace_revision",
];

/// Every finding of the audit `arguments` asks for, listed `limit` at a time, reassembled into
/// the report the pages project: the tool's own fields and the findings' regions taken away,
/// and the revision as the command line's snapshot holds it.
fn whole_report(client: &mut Client, arguments: &Value, limit: usize) -> Value {
    let mut findings = Vec::new();
    let mut first: Option<Value> = None;
    let mut cursor: Option<String> = None;
    loop {
        let mut arguments = arguments.clone();
        arguments["limit"] = json!(limit);
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = client.call("quality_audit", arguments).unwrap();
        assert!(page["findings"].as_array().unwrap().len() <= limit);
        findings.extend(page["findings"].as_array().unwrap().iter().cloned());
        let next = page["next_cursor"].as_str().map(str::to_owned);
        first.get_or_insert(page);
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let mut report = first.unwrap();
    assert_eq!(report["findings_total"], findings.len());
    for finding in &mut findings {
        finding.as_object_mut().unwrap().remove("region");
    }
    report["findings"] = Value::Array(findings);
    for field in ADDED {
        report.as_object_mut().unwrap().remove(field);
    }
    report["revision"] = json!("[revision]");
    report
}

/// The JSON report `adocfoundry-cli audit tests/fixtures/audit --format json` gives, as its
/// tests hold it in their snapshot, the revision a placeholder.
fn cli_report() -> Value {
    let snapshot = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../adocfoundry-cli/tests/snapshots/audit__audit_json.snap"),
    )
    .unwrap();
    let body = snapshot
        .splitn(3, "---\n")
        .nth(2)
        .expect("an insta snapshot: a header between `---` lines, then the body");
    serde_json::from_str(body).unwrap()
}

#[test]
fn mcp_quality_matches_cli() {
    let (_directory, root) = scratch("audit");
    let mut client = Client::start(&root, false);

    // The whole workspace, a few findings at a time, is the command line's report.
    let report = whole_report(&mut client, &json!({ "include_excluded": true }), 2);
    assert_eq!(report, cli_report());
    let page = client.call("quality_audit", json!({})).unwrap();
    assert_eq!(page["exit_code"], 0, "as the command line exits");
    assert_eq!(page["verdict"], "pass");
    assert_eq!(page["outcome"], "pass");

    // A component version's audit is the engine's, under the same workspace-only policy the
    // command line reads.
    let mut engine = Engine::new(Settings::load(None, Some(&root)).unwrap().with_git_opt_in());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(30)));
    let policy = engine.settings().quality.policy.clone();
    let scope = AuditScope::Component {
        name: "product".into(),
        version: Some("3.0".into()),
    };
    let mut expected = serde_json::to_value(engine.audit_now_with(scope, &policy)).unwrap();
    expected["revision"] = json!("[revision]");
    let report = whole_report(
        &mut client,
        &json!({ "component": "product", "version": "3.0", "include_excluded": true }),
        500,
    );
    assert_eq!(report, expected);
    client.stop();
}

#[test]
fn mcp_quality_reports_skips_separately_from_coverage() {
    let (_directory, root) = scratch("audit");
    let mut client = Client::start(&root, false);
    let page = client.call("quality_audit", json!({})).unwrap();
    // Dynamic constructs that cannot be judged without a build are skips: counted and listed,
    // while the audit still covered everything.
    assert_eq!(page["completeness"]["status"], "complete");
    assert_eq!(page["counts"]["skipped"], 2);
    assert_eq!(page["skips"].as_array().unwrap().len(), 2);
    let guide = page["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|coverage| coverage["component"] == "guide")
        .unwrap();
    assert_eq!(guide["undetermined"], 1);
    assert_eq!(page["outcome"], "pass");

    // Findings carry their places: a byte range and the lines and columns it spans.
    let titled = client
        .call(
            "quality_audit",
            json!({ "rules": ["quality.image.missing-alt"] }),
        )
        .unwrap();
    assert_eq!(titled["findings_total"], 3);
    assert_eq!(
        titled["counts"]["warning"], 7,
        "counts are the whole report's"
    );
    let region = &titled["findings"][0]["region"];
    assert!(region["start_line"].as_u64().unwrap() >= 1, "{titled}");
    let warnings = client
        .call("quality_audit", json!({ "severity": "warning" }))
        .unwrap();
    assert_eq!(
        warnings["findings_total"], 7,
        "the information finding left out"
    );
    assert!(
        warnings.get("excluded").is_none(),
        "exclusions listed only when asked"
    );
    client.stop();
}

#[test]
fn mcp_analysis_pagination_does_not_hide_incomplete() {
    let (_directory, root) = scratch("audit");
    // An enabled provider that this build cannot run leaves the audit partial.
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[quality]\nproviders = [\"vale\"]\n",
    )
    .unwrap();
    let mut client = Client::start(&root, false);
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut arguments = json!({ "limit": 1 });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = client.call("quality_audit", arguments).unwrap();
        pages += 1;
        // Every page carries the whole report's coverage and counts, not only the first.
        assert_eq!(page["completeness"]["status"], "partial", "page {pages}");
        assert_eq!(
            page["completeness"]["gaps"][0]["reason"],
            "provider-unavailable"
        );
        assert_eq!(page["counts"]["warning"], 7);
        assert_eq!(page["outcome"], "pass_partial", "never a plain pass");
        assert_eq!(page["exit_code"], 0);
        match page["next_cursor"].as_str() {
            Some(next) => {
                assert_eq!(page["truncated"]["shown"], 1);
                cursor = Some(next.to_owned());
            }
            None => break,
        }
    }
    assert_eq!(pages, 8, "one finding a page");

    let strict = client
        .call("quality_audit", json!({ "require_complete": true }))
        .unwrap();
    assert_eq!(strict["outcome"], "untrustworthy");
    assert_eq!(strict["exit_code"], 2, "as `--require-complete` exits");

    // A cursor holds to its query: other filters or policy start again.
    let first = client.call("quality_audit", json!({ "limit": 1 })).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    for arguments in [
        json!({ "limit": 1, "cursor": cursor, "severity": "error" }),
        json!({ "limit": 1, "cursor": cursor, "require_complete": true }),
    ] {
        let refused = client.call_result("quality_audit", arguments);
        assert_eq!(
            refused["structuredContent"]["error"]["code"],
            "invalid_argument"
        );
    }
    for (arguments, code) in [
        (json!({ "version": "3.0" }), "invalid_argument"),
        (
            json!({ "component": "product", "version": "~" }),
            "invalid_argument",
        ),
        (json!({ "component": "nope" }), "not_found"),
        (json!({ "rules": ["quality.made-up"] }), "invalid_argument"),
        (json!({ "severity": "fatal" }), "invalid_argument"),
        (json!({ "fail_on": "always" }), "invalid_argument"),
    ] {
        let refused = client.call_result("quality_audit", arguments.clone());
        assert_eq!(
            refused["structuredContent"]["error"]["code"], code,
            "{arguments}"
        );
    }
    client.stop();
}
