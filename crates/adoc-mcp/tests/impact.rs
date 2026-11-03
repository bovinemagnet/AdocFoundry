//! The `analyse_impact` tool (MCP PRD §§7.4 and 11.2, tests 10–13), driven through the binary
//! over stdio on a temporary Git repository holding `tests/fixtures/impact`.

mod common;

use std::{fs, path::PathBuf};

use adoc_git::testing::TestRepo;
use common::{copy_tree, fixture, Client};
use serde_json::{json, Value};

const NESTED: &str = "v1/modules/ROOT/partials/nested.adoc";
const INTRO: &str = "v1/modules/ROOT/partials/intro.adoc";
const INDEX: &str = "v1/modules/ROOT/pages/index.adoc";
const INSTALL: &str = "v1/modules/ROOT/pages/install.adoc";
const SETUP: &str = "v1/modules/ROOT/pages/setup.adoc";
const GUIDE: &str = "v1/modules/ROOT/pages/guide.adoc";

struct Repo {
    _directory: tempfile::TempDir,
    repo: TestRepo,
    root: PathBuf,
    base: String,
}

impl Repo {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_engine::normalize_path(&directory.path().join("docs"));
        fs::create_dir_all(&root).unwrap();
        let repo = TestRepo::init(&root);
        copy_tree(&fixture("impact"), &root);
        let base = repo.commit_all("base");
        Self {
            _directory: directory,
            repo,
            root,
            base,
        }
    }

    fn commit(&self) -> String {
        self.repo.commit_all("head")
    }
}

fn impacts<'a>(report: &'a Value, kind: &str, origin: &str) -> Vec<&'a Value> {
    report["impacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|impact| impact["kind"] == kind && impact["origin"] == origin)
        .collect()
}

fn paths(impacts: &[&Value]) -> Vec<String> {
    impacts
        .iter()
        .map(|impact| impact["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn mcp_impact_partial_propagates_with_evidence() {
    let repo = Repo::new();
    repo.repo.write(NESTED, "A nested note, changed.\n");
    let head = repo.commit();
    let mut client = Client::start(&repo.root, false);
    let report = client
        .call(
            "analyse_impact",
            json!({ "source": { "type": "git_refs", "base": repo.base, "head": head } }),
        )
        .unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["completeness"]["status"], "complete");
    let render = impacts(&report, "render", NESTED);
    assert_eq!(paths(&render), [INDEX, INSTALL]);
    for impact in render {
        assert_eq!(impact["evidence"], "both");
        assert_eq!(impact["via"], json!([NESTED, INTRO, impact["path"]]));
        assert_eq!(impact["version"], "1.0");
        assert!(impact["source"]["range"]["start"].is_u64());
    }
    let compact = client
        .call(
            "analyse_impact",
            json!({
                "source": { "type": "git_refs", "base": repo.base, "head": head },
                "include_provenance": false,
            }),
        )
        .unwrap();
    assert!(compact["impacts"][0].get("via").is_none());
    assert!(compact["impacts"][0].get("source").is_none());
    client.stop();
}

#[test]
fn mcp_impact_prose_does_not_propagate_via_xref() {
    let repo = Repo::new();
    repo.repo.write(
        INSTALL,
        "= Installing\n\ninclude::partial$intro.adoc[]\n\n[#setup]\n== Setting up\n\nOther steps.\n",
    );
    let head = repo.commit();
    let mut client = Client::start(&repo.root, false);
    let report = client
        .call(
            "analyse_impact",
            json!({ "source": { "type": "git_refs", "base": repo.base, "head": head } }),
        )
        .unwrap();
    assert_eq!(paths(&impacts(&report, "render", INSTALL)), [INSTALL]);
    assert!(impacts(&report, "reference_integrity", INSTALL).is_empty());
    assert!(!report.to_string().contains("guide.adoc"), "{report}");
    client.stop();
}

#[test]
fn mcp_impact_ref_comparison_uses_two_snapshots() {
    let repo = Repo::new();
    let text = fs::read_to_string(repo.root.join(INSTALL)).unwrap();
    repo.repo.remove(INSTALL);
    repo.repo.write(SETUP, &text);
    repo.repo.remove(NESTED);
    repo.repo.write(INTRO, "An introduction.\n");
    let head = repo.commit();
    // Uncommitted work in the working tree is not either side of a ref comparison.
    repo.repo
        .write(GUIDE, "= Guide\n\nRewritten, uncommitted.\n");
    let mut client = Client::start(&repo.root, false);
    let report = client
        .call(
            "analyse_impact",
            json!({ "source": { "type": "git_refs", "base": repo.base, "head": head } }),
        )
        .unwrap();
    // The removed partial's consumers come from base, where the page was still `install`.
    let removed = impacts(&report, "render", NESTED);
    assert_eq!(paths(&removed), [INDEX, INSTALL]);
    assert!(removed.iter().all(|impact| impact["evidence"] == "base"));
    // The moved page's referrers come from base and still name the old place in head.
    let moved: Vec<(String, String)> = impacts(&report, "reference_integrity", INSTALL)
        .iter()
        .map(|impact| {
            (
                impact["path"].as_str().unwrap().to_owned(),
                impact["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        moved,
        [
            (GUIDE.to_owned(), "broken".to_owned()),
            (INDEX.to_owned(), "broken".to_owned()),
        ]
    );
    assert_eq!(report["source"]["kind"], "git-refs");
    assert_eq!(
        fs::read_to_string(repo.root.join(GUIDE)).unwrap(),
        "= Guide\n\nRewritten, uncommitted.\n"
    );

    // Errors for a ref that names nothing and an ambiguous scope.
    let unknown = client.call_result(
        "analyse_impact",
        json!({ "source": { "type": "git_refs", "base": "no-such-branch", "head": head } }),
    );
    assert_eq!(unknown["structuredContent"]["error"]["code"], "not_found");
    let scope = client.call_result(
        "analyse_impact",
        json!({ "source": { "type": "working_tree" }, "version": "1.0" }),
    );
    assert_eq!(
        scope["structuredContent"]["error"]["code"],
        "invalid_argument"
    );
    let outside = client.call_result(
        "analyse_impact",
        json!({ "source": { "type": "paths", "paths": ["../elsewhere.adoc"] } }),
    );
    assert_eq!(
        outside["structuredContent"]["error"]["code"],
        "outside_workspace"
    );
    client.stop();
}

#[test]
fn mcp_analysis_pagination_does_not_hide_incomplete() {
    let repo = Repo::new();
    let mut client = Client::start(&repo.root, false);
    // A removed path with no base revision leaves the analysis partial.
    fs::remove_file(repo.root.join(INSTALL)).unwrap();
    let mut cursor: Option<String> = None;
    let mut seen = 0;
    loop {
        let mut arguments = json!({
            "source": { "type": "paths", "paths": [INSTALL, NESTED] },
            "limit": 1,
        });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = client.call("analyse_impact", arguments).unwrap();
        assert_eq!(
            page["completeness"]["status"], "partial",
            "every page says so"
        );
        assert_eq!(
            page["completeness"]["gaps"][0]["reason"],
            "no-base-revision"
        );
        assert_eq!(page["counts"]["render"], 2, "the whole report's counts");
        seen += page["impacts"].as_array().unwrap().len();
        match page["next_cursor"].as_str() {
            Some(next) => {
                assert_eq!(page["truncated"]["shown"], 1);
                cursor = Some(next.to_owned());
            }
            None => break,
        }
    }
    assert_eq!(seen, 2);
    client.stop();
}
