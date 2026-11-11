//! `adocfoundry-cli audit`: the quality audit as text, JSON or SARIF, and the exit codes of the
//! CI policy (Quality PRD §5.4). Commands run from the repository root with relative fixture
//! paths, so the output, and therefore the snapshots, are the same on every machine.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const FIXTURE: &str = "tests/fixtures/audit";

fn repository_root() -> PathBuf {
    adoc_index::normalize_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

fn cli(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(repository_root())
        .args(arguments)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exited")
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!(
            "stdout is not one JSON document ({error}):\n{}\nstderr:\n{}",
            stdout(output),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A copy of the audit fixture to change.
fn copy() -> (tempfile::TempDir, PathBuf) {
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    copy_tree(&repository_root().join(FIXTURE), &root);
    (directory, root)
}

#[test]
fn audit_cli_exit_codes() {
    // Warnings only: below the default threshold, at `--fail-on warning`.
    assert_eq!(code(&cli(&["audit", FIXTURE])), 0);
    assert_eq!(code(&cli(&["audit", FIXTURE, "--fail-on", "warning"])), 1);
    assert_eq!(code(&cli(&["audit", FIXTURE, "--fail-on", "none"])), 0);
    // A bad option is a failed run.
    assert_eq!(code(&cli(&["audit", FIXTURE, "--fail-on", "info"])), 2);
    assert_eq!(code(&cli(&["audit", FIXTURE, "--format", "xml"])), 2);

    // An invalid policy is invalid configuration: the report is still written, then exit 2.
    let (_directory, root) = copy();
    fs::write(
        root.join(".adocfoundry.toml"),
        "[[quality.exclusions]]\nrule = \"quality.navigation.unlisted-page\"\nresource = \"a.adoc\"\nreason = \"Unqualified\"\n",
    )
    .unwrap();
    let output = cli(&["audit", root.to_str().unwrap(), "--format", "json"]);
    assert_eq!(code(&output), 2);
    assert!(json(&output)["findings"].is_array(), "written in full");
    assert!(String::from_utf8_lossy(&output.stderr).contains("quality.exclusions[1]"));
}

#[test]
fn audit_unavailable_never_passes() {
    for fail_on in ["none", "error"] {
        let output = cli(&[
            "audit",
            FIXTURE,
            "--component",
            "platform",
            "--fail-on",
            fail_on,
            "--format",
            "json",
        ]);
        assert_eq!(code(&output), 2, "fail-on {fail_on}");
        assert_eq!(json(&output)["completeness"]["status"], "unavailable");
    }
}

#[test]
fn audit_skips_do_not_make_partial() {
    let output = cli(&["audit", FIXTURE, "--format", "json", "--require-complete"]);
    assert_eq!(code(&output), 0, "complete, with skips");
    let report = json(&output);
    assert_eq!(report["completeness"]["status"], "complete");
    assert!(report["counts"]["skipped"].as_u64().unwrap() > 0);
}

/// An unreadable folder leaves the audit partial: passing by default, untrustworthy in strict
/// mode, and written in full either way.
#[cfg(unix)]
#[test]
fn audit_cli_strict_mode_fails_incomplete() {
    use std::os::unix::fs::PermissionsExt;

    let (_directory, root) = copy();
    // Outside the modules: one inside a family folder fails Antora discovery, and the index.
    let locked = root.join("product/locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("secret.adoc"), "= Secret\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&locked).is_ok();
    let root_text = root.to_str().unwrap().to_owned();

    let default = cli(&["audit", &root_text, "--format", "json"]);
    let strict = cli(&[
        "audit",
        &root_text,
        "--format",
        "json",
        "--require-complete",
    ]);
    let text = cli(&["audit", &root_text]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    if readable {
        // Running with the rights to read anything (as root), so nothing is unreadable.
        return;
    }
    assert_eq!(
        code(&default),
        0,
        "passing, with partial coverage: {}",
        String::from_utf8_lossy(&default.stderr)
    );
    let report = json(&default);
    assert_eq!(report["completeness"]["status"], "partial");
    assert_eq!(
        report["completeness"]["gaps"][0]["reason"],
        "scope-not-indexed"
    );
    assert_eq!(
        report["completeness"]["gaps"][0]["affected"][0],
        "product/locked"
    );
    assert_eq!(code(&strict), 2);
    assert!(json(&strict)["findings"].is_array(), "written in full");
    let text = stdout(&text);
    assert!(
        text.lines().next().unwrap().contains("partial"),
        "the first line says so: {text}"
    );
    assert!(text.contains("with partial coverage"), "{text}");

    // An index that failed audited nothing, which is no pass (#505).
    let pages = root.join("product/modules/ROOT/pages");
    fs::set_permissions(&pages, fs::Permissions::from_mode(0o000)).unwrap();
    let failed = cli(&["audit", &root_text]);
    fs::set_permissions(&pages, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code(&failed), 2);
    assert!(stdout(&failed).is_empty());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("indexing failed"));
}

#[test]
fn audit_of_the_current_directory_reports_as_audit_of_its_path() {
    // Issue #534: with the root `.`, messages lost their full stops and files their folders.
    let here = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(repository_root().join(FIXTURE))
        .args(["audit", ".", "--format", "json"])
        .output()
        .unwrap();
    let there = cli(&["audit", FIXTURE, "--format", "json"]);
    let findings = json(&there)["findings"].clone();
    assert!(findings.as_array().is_some_and(|all| !all.is_empty()));
    assert_eq!(json(&here)["findings"], findings);
}

#[test]
fn audit_json_and_sarif_are_deterministic() {
    for format in ["json", "sarif"] {
        let first = cli(&["audit", FIXTURE, "--format", format]);
        let second = cli(&["audit", FIXTURE, "--format", format]);
        assert_eq!(
            code(&first),
            0,
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert_eq!(
            stdout(&first),
            stdout(&second),
            "{format} is the same every time"
        );
        json(&first);
    }
    // The index's and catalog's revisions count their updates, which say nothing about the
    // audit, so the snapshot holds a placeholder for them.
    let mut report = json(&cli(&["audit", FIXTURE, "--format", "json"]));
    report["revision"] = serde_json::Value::from("[revision]");
    insta::assert_snapshot!("audit_json", serde_json::to_string_pretty(&report).unwrap());
    // The tool's version is the release's, so the snapshot holds a placeholder for it.
    insta::assert_snapshot!(
        "audit_sarif",
        stdout(&cli(&["audit", FIXTURE, "--format", "sarif"])).replace(
            &format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION")),
            "\"version\": \"[version]\""
        )
    );
}

#[test]
fn sarif_results_have_valid_relative_locations() {
    let sarif = json(&cli(&["audit", FIXTURE, "--format", "sarif"]));
    assert_eq!(sarif["version"], "2.1.0");
    let run = &sarif["runs"][0];
    assert_eq!(run["tool"]["driver"]["name"], "AdocFoundry");
    assert_eq!(run["columnKind"], "unicodeCodePoints");
    let rules: Vec<&str> = run["tool"]["driver"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["id"].as_str().unwrap())
        .collect();
    let results = run["results"].as_array().unwrap();
    assert!(!results.is_empty());
    for result in results {
        assert!(
            rules.contains(&result["ruleId"].as_str().unwrap()),
            "{result}"
        );
        assert!(["error", "warning", "note"].contains(&result["level"].as_str().unwrap()));
        let location = &result["locations"][0]["physicalLocation"];
        let uri = location["artifactLocation"]["uri"].as_str().unwrap();
        assert!(
            !uri.starts_with('/') && !uri.contains(':') && !uri.contains('\\'),
            "relative, with `/`: {uri}"
        );
        if let Some(region) = location.get("region") {
            for key in ["startLine", "startColumn", "endLine", "endColumn"] {
                assert!(
                    region[key].as_u64().unwrap() >= 1,
                    "1-based {key}: {region}"
                );
            }
        }
    }
    assert_eq!(run["invocations"][0]["executionSuccessful"], true);
    assert_eq!(run["properties"]["completeness"], "complete");
}

#[test]
fn excluded_findings_are_suppressed_results_in_sarif() {
    let (_directory, root) = copy();
    fs::write(
        root.join(".adocfoundry.toml"),
        "[[quality.exclusions]]\nrule = \"quality.navigation.unlisted-page\"\nresource = \"3.0@product:ROOT:orphan.adoc\"\nreason = \"Reached from the release notes\"\n",
    )
    .unwrap();
    let sarif = json(&cli(&[
        "audit",
        root.to_str().unwrap(),
        "--format",
        "sarif",
    ]));
    let suppressed: Vec<&serde_json::Value> = sarif["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|result| result.get("suppressions").is_some())
        .collect();
    assert_eq!(suppressed.len(), 1);
    assert_eq!(
        suppressed[0]["suppressions"][0]["justification"],
        "Reached from the release notes"
    );
}

#[test]
fn reports_hold_no_absolute_path() {
    let absolute = repository_root().to_string_lossy().into_owned();
    for format in ["text", "json", "sarif"] {
        let output = stdout(&cli(&["audit", FIXTURE, "--format", format]));
        assert!(!output.contains(&absolute), "{format}:\n{output}");
    }
}

#[test]
fn text_states_completeness_first_and_the_result_last() {
    let output = cli(&["audit", FIXTURE]);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Audit of the workspace: complete");
    assert_eq!(*lines.last().unwrap(), "Result: pass");
    assert!(
        text.contains(
            "product/modules/ROOT/pages/orphan.adoc: warning quality.navigation.unlisted-page"
        ),
        "a finding about the whole file has no position: {text}"
    );
    assert!(
        text.contains(
            "product/modules/ROOT/pages/images.adoc:3:1: warning quality.image.missing-alt"
        ),
        "{text}"
    );
}

#[test]
fn a_component_scope_and_an_output_file() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("report.json");
    let output = cli(&[
        "audit",
        FIXTURE,
        "--component",
        "2.0@product",
        "--format",
        "json",
        "--output",
        file.to_str().unwrap(),
    ]);
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).is_empty(), "the report goes to the file");
    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(report["scope"]["name"], "product");
    assert_eq!(report["scope"]["version"], "2.0");
    assert_eq!(report["coverage"].as_array().unwrap().len(), 1);
}

/// An enabled provider this build cannot run is reported as unavailable, leaving the audit
/// partial, while the core audit's findings stand: passing by default, untrustworthy in strict
/// mode.
#[test]
fn audit_unavailable_providers_leave_the_core_audit() {
    let (_directory, root) = copy();
    let root_text = root.to_str().unwrap().to_owned();
    let plain = json(&cli(&["audit", &root_text, "--format", "json"]));
    fs::write(
        root.join(".adocfoundry.toml"),
        "[quality]\nproviders = [\"vale\"]\n",
    )
    .unwrap();

    let output = cli(&["audit", &root_text, "--format", "json"]);
    assert_eq!(code(&output), 0, "passing, with partial coverage");
    let report = json(&output);
    assert_eq!(
        report["findings"], plain["findings"],
        "the core audit stands"
    );
    assert_eq!(report["completeness"]["status"], "partial");
    assert_eq!(
        report["completeness"]["gaps"][0]["reason"],
        "provider-unavailable"
    );
    let vale = report["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|status| status["provider"] == "vale")
        .unwrap();
    assert_eq!(vale["state"], "unavailable");
    assert!(vale["reason"].as_str().unwrap().contains("#85"));
    assert!(report["providers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|status| status["provider"] != "vale")
        .all(|status| status["state"] == "disabled"));

    assert_eq!(
        code(&cli(&["audit", &root_text, "--require-complete"])),
        2,
        "strict mode does not trust it"
    );
    let text = stdout(&cli(&["audit", &root_text]));
    assert!(text.contains("Provider vale: unavailable"), "{text}");
    assert!(text.contains("Gap (provider-unavailable)"), "{text}");
    let sarif = json(&cli(&["audit", &root_text, "--format", "sarif"]));
    let run = &sarif["runs"][0];
    assert_eq!(
        run["invocations"][0]["toolExecutionNotifications"][0]["descriptor"]["id"],
        "provider-unavailable"
    );
    assert_eq!(run["properties"]["providers"], report["providers"]);
}

/// End to end: the command line's audit and the audit the app keeps (the engine with the
/// problems and the audit kept, as the app has them) agree under a workspace's policy, in the
/// report the Quality overview shows and, finding by finding, in what Problems lists.
#[test]
fn cli_and_app_report_the_same_audit() {
    use adoc_engine::{audit, Engine, Settings};
    use std::time::{Duration, Instant};

    let (_directory, root) = copy();
    fs::write(
        root.join(".adocfoundry.toml"),
        "[quality]\nproviders = [\"page-age\"]\n\n[quality.rules]\n\"quality.page.missing-title\" = \"error\"\n\n[[quality.exclusions]]\nrule = \"quality.navigation.unlisted-page\"\nresource = \"3.0@product:ROOT:orphan.adoc\"\nreason = \"Reached from the release notes\"\n",
    )
    .unwrap();
    let root = adoc_index::normalize_path(&root);
    let root_text = root.to_str().unwrap().to_owned();
    let mut from_cli = json(&cli(&["audit", &root_text, "--format", "json"]));

    let mut engine = Engine::new(Settings::load(None, Some(&root)).unwrap().with_git_opt_in());
    engine.enable_problems();
    engine.enable_audit();
    engine.open_workspace(vec![root.clone()]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while engine.audit_builds() == 0 || engine.has_pending() {
        assert!(Instant::now() < deadline, "the app's audit arrives");
        engine.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    let kept = engine
        .audit_report(&audit::AuditScope::Workspace)
        .expect("kept");
    let mut from_app = serde_json::to_value(&kept).unwrap();
    // The revisions count the index's and catalog's updates, which differ between the two runs.
    from_cli["revision"] = serde_json::Value::Null;
    from_app["revision"] = serde_json::Value::Null;
    assert_eq!(from_app, from_cli, "the same report, counts and detail");
    assert_eq!(kept.counts().excluded, 1);
    assert_eq!(
        from_cli["completeness"]["status"], "partial",
        "page-age is unavailable"
    );

    // Every `quality.*` finding the command line prints is the problem the app lists, at the
    // same place, and there are no others.
    let text = stdout(&cli(&["audit", &root_text]));
    let mut printed: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(" quality."))
        .collect();
    printed.sort_unstable();
    let mut listed: Vec<String> = engine
        .problems()
        .iter()
        .flat_map(|(path, problems)| {
            let relative = adoc_audit_path(path.strip_prefix(&root).unwrap());
            problems
                .iter()
                .filter(|problem| matches!(problem.code, audit::FindingCode::Rule(_)))
                .map(move |problem| {
                    let place = if problem.whole_file {
                        relative.clone()
                    } else {
                        format!("{relative}:{}:{}", problem.line, problem.column)
                    };
                    let severity = match problem.severity {
                        adoc_core::DiagnosticSeverity::Error => "error",
                        adoc_core::DiagnosticSeverity::Warning => "warning",
                        adoc_core::DiagnosticSeverity::Information => "info",
                    };
                    format!(
                        "{place}: {severity} {} {}",
                        problem.code.as_str(),
                        problem.message
                    )
                })
        })
        .collect();
    listed.sort_unstable();
    assert!(!listed.is_empty());
    assert_eq!(listed, printed);
}

/// `path` with `/` separators, as reports name files.
fn adoc_audit_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}
