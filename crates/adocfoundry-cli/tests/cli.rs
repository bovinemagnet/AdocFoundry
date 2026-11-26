//! Drives the `adocfoundry-cli` binary.
//!
//! Commands run from the repository root with relative fixture paths, so that paths in the
//! output, and therefore the snapshots, are the same on every machine.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cli(arguments: &[&str]) -> Output {
    cli_in("", arguments)
}

/// Runs the binary from `directory`, relative to the repository root.
fn cli_in(directory: &str, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(repository_root().join(directory))
        .args(arguments)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn prints_version() {
    let output = cli(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output).trim(),
        format!("adocfoundry-cli {}", env!("CARGO_PKG_VERSION"))
    );
}

fn check_json(fixture: &str) -> String {
    let path = format!("tests/fixtures/{fixture}");
    check_json_of(&cli(&["check", &path, "--format", "json"]), fixture)
}

/// `check --format json`'s output, with what differs between machines and releases, the root's
/// absolute path and the CLI's version, put as `[root]` and `[version]` once checked.
fn check_json_of(output: &Output, fixture: &str) -> String {
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let root = Path::new(json["root"].as_str().unwrap());
    assert!(root.is_absolute(), "{root:?}");
    assert!(
        root.ends_with(Path::new("tests/fixtures").join(fixture)),
        "{root:?}"
    );
    assert_eq!(json["tool"]["version"], env!("CARGO_PKG_VERSION"));
    json["root"] = "[root]".into();
    json["tool"]["version"] = "[version]".into();
    // Messages show paths as the platform writes them; JSON escapes a Windows `\` as `\\`.
    serde_json::to_string_pretty(&json)
        .unwrap()
        .replace("\\\\", "/")
}

#[test]
fn check_antora_single_component() {
    insta::assert_snapshot!(check_json("antora-single-component"));
}

#[test]
fn a_page_in_a_build_folder_of_a_module_is_checked_and_indexed() {
    // Issue #531: catalogued and indexed alike, so an xref to its anchor resolves.
    let output = cli(&["check", "tests/fixtures/ignored-folders"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(
        !stdout(&output).contains("unresolved"),
        "{}",
        stdout(&output)
    );
    let output = cli(&["symbols", "tests/fixtures/ignored-folders", "Tools"]);
    assert!(stdout(&output).contains("build"), "{}", stdout(&output));
    assert!(!stdout(&output).contains("stray"), "{}", stdout(&output));
}

#[test]
fn check_antora_nested_pages() {
    insta::assert_snapshot!(check_json("antora-nested-pages"));
}

#[test]
fn check_antora_nested_includes() {
    insta::assert_snapshot!(check_json("antora-nested-includes"));
}

#[test]
fn check_antora_invalid_descriptor() {
    insta::assert_snapshot!(check_json("antora-invalid-descriptor"));
}

#[test]
fn check_antora_multi_component() {
    insta::assert_snapshot!(check_json("antora-multi-component"));
}

#[test]
fn check_antora_versions() {
    insta::assert_snapshot!(check_json("antora-versions"));
}

#[test]
fn check_antora_broken_references() {
    insta::assert_snapshot!(check_json("antora-broken-references"));
}

#[test]
fn check_broken_references() {
    insta::assert_snapshot!(check_json("broken-references"));
}

/// Issue #593: a code span that does not close where it seems to, and none for the spans
/// written well or in a listing.
#[test]
fn check_broken_code_spans() {
    insta::assert_snapshot!(check_json("broken-code-spans"));
}

#[test]
fn graph_prints_json() {
    let output = cli(&["graph", "tests/fixtures/graph", "--unresolved"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    insta::assert_snapshot!(stdout(&output));

    let local = cli(&[
        "graph",
        "tests/fixtures/graph",
        "--page",
        "tests/fixtures/graph/guide/modules/ROOT/pages/configure.adoc",
        "--depth",
        "1",
        "--no-nav",
    ]);
    assert!(local.status.success());
    let report: serde_json::Value = serde_json::from_slice(&local.stdout).unwrap();
    let ids: Vec<&str> = report["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "guide/modules/ROOT/pages/configure.adoc",
            "guide/modules/ROOT/pages/index.adoc",
            "guide/modules/ROOT/pages/install.adoc"
        ]
    );
    assert!(report["edges"]
        .as_array()
        .unwrap()
        .iter()
        .all(|edge| edge["kind"] != "nav"));
}

/// `--ages` adds when each page last changed, from its Git history, or `uncommitted`.
#[test]
fn graph_prints_ages_from_git() {
    let directory = tempfile::tempdir().unwrap();
    let repo = adoc_git::testing::TestRepo::init(directory.path());
    repo.write("old.adoc", "= Old\n\nSee xref:new.adoc[].\n");
    repo.write("new.adoc", "= New\n");
    repo.commit_all_at("first", 86_400);
    repo.write("new.adoc", "= New\n\nEdited.\n");
    repo.commit_all_at("second", 2 * 86_400);
    repo.write("draft.adoc", "= Draft\n");
    let root = directory.path().to_str().unwrap();

    let output = cli(&["graph", root, "--ages"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let age = |id: &str| {
        report["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .map(|node| node["last_changed"].clone())
            .unwrap()
    };
    assert_eq!(age("old.adoc"), 86_400);
    assert_eq!(age("new.adoc"), 2 * 86_400);
    assert_eq!(age("draft.adoc"), "uncommitted");

    // Without it, Git is not read and no node has an age.
    let plain = cli(&["graph", root]);
    let report: serde_json::Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert!(report["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|node| node.get("last_changed").is_none()));
}

#[test]
fn graph_prints_dot() {
    let output = cli(&[
        "graph",
        "tests/fixtures/graph",
        "--format",
        "dot",
        "--no-partials",
    ]);
    assert!(output.status.success());
    let dot = stdout(&output);
    assert!(dot.starts_with("digraph links {"));
    assert!(dot.contains(
        "\"guide/modules/ROOT/nav.adoc\" -> \"guide/modules/ROOT/pages/install.adoc\" \
         [kind=nav style=dotted label=\"2\"];"
    ));
    assert!(dot.contains("\"guide/modules/ROOT/nav.adoc\" [label=\"nav.adoc\" shape=box];"));
    assert!(!dot.contains("partials/note.adoc"));
    assert!(dot.trim_end().ends_with('}'));

    let refused = cli(&["graph", "tests/fixtures/graph", "--depth", "2"]);
    assert_eq!(refused.status.code(), Some(2));
}

#[test]
fn graph_prints_svg() {
    let output = cli(&[
        "graph",
        "tests/fixtures/graph",
        "--page",
        "tests/fixtures/graph/guide/modules/ROOT/pages/configure.adoc",
        "--format",
        "svg",
    ]);
    assert!(output.status.success());
    insta::assert_snapshot!(stdout(&output));
}

/// The same documentation in two places, here two temporary folders at different depths, is
/// laid out and drawn alike: the layout's start does not depend on the folders above it.
#[test]
fn graph_svg_is_the_same_wherever_the_files_are() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = repository_root().join("tests/fixtures/graph");
    let (near, far) = (
        directory.path().join("docs"),
        directory.path().join("elsewhere/checkout/docs"),
    );
    copy_tree(&fixture, &near);
    copy_tree(&fixture, &far);
    let svg = |root: &Path| {
        let output = cli(&["graph", root.to_str().unwrap(), "--format", "svg"]);
        assert!(output.status.success());
        stdout(&output)
    };
    assert_eq!(svg(&near), svg(&far));
}

#[test]
fn graph_prints_the_path_between_two_pages() {
    let pages = "tests/fixtures/graph/guide/modules/ROOT/pages";
    let (home, configure, orphan) = (
        format!("{pages}/index.adoc"),
        format!("{pages}/configure.adoc"),
        format!("{pages}/orphan.adoc"),
    );
    let output = cli(&["graph", "tests/fixtures/graph", "--path", &home, &configure]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let steps: Vec<(&str, &serde_json::Value)> = report["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| (step["title"].as_str().unwrap(), &step["links"]))
        .collect();
    assert_eq!(
        steps,
        [
            ("Home", &serde_json::json!([])),
            ("Install", &serde_json::json!(["xref"])),
            ("Configure", &serde_json::json!(["xref", "nav"])),
        ]
    );
    assert_eq!(report["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(report["edges"].as_array().unwrap().len(), 3);

    // Either way, Configure's xref back to Home is the one step.
    let either = cli(&[
        "graph",
        "tests/fixtures/graph",
        "--path",
        &home,
        &configure,
        "--undirected",
        "--format",
        "dot",
    ]);
    let dot = stdout(&either);
    assert_eq!(dot.matches(" -> ").count(), 1, "{dot}");

    let none = cli(&["graph", "tests/fixtures/graph", "--path", &home, &orphan]);
    assert_eq!(none.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&none.stderr).contains("no path from"));
    let half = cli(&["graph", "tests/fixtures/graph", "--path", &home]);
    assert_eq!(half.status.code(), Some(2));
}

/// `check --format json` is an object with a schema version, the tool and its version, the
/// root, the counts by severity and the diagnostics (#807), so a build tool can rely on it.
#[test]
fn check_json_is_versioned() {
    let output = cli(&[
        "check",
        "tests/fixtures/broken-references",
        "--format",
        "json",
    ]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1, "{json}");
    assert_eq!(json["tool"]["name"], "adocfoundry-cli", "{json}");
    assert_eq!(json["tool"]["version"], env!("CARGO_PKG_VERSION"), "{json}");
    let diagnostics = json["diagnostics"].as_array().unwrap();
    assert!(!diagnostics.is_empty(), "{json}");
    for severity in ["error", "warning", "info"] {
        let count = diagnostics
            .iter()
            .filter(|entry| entry["severity"] == severity)
            .count();
        assert_eq!(json["summary"][severity], count, "{severity}: {json}");
    }
    let keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["schema_version", "tool", "root", "summary", "diagnostics"],
        "{json}"
    );
}

/// `check` exits with 0 when nothing meets `--fail-on`, 1 when something does, and 2 for a
/// usage or runtime error, whatever the format (#807).
#[test]
fn check_exit_codes() {
    let root = "tests/fixtures/antora-broken-references";
    let cases: [(&[&str], i32); 8] = [
        (&["check", root], 0),
        (&["check", root, "--format", "json"], 0),
        (&["check", root, "--fail-on", "warning"], 1),
        (
            &["check", root, "--format", "json", "--fail-on", "warning"],
            1,
        ),
        (&["check"], 2),
        (&["check", root, "--format", "yaml"], 2),
        (&["check", root, "--fail-on", "never"], 2),
        (&["check", root, "--bogus", "x"], 2),
    ];
    for (arguments, expected) in cases {
        let output = cli(arguments);
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn check_text_format_and_fail_on() {
    let output = cli(&["check", "tests/fixtures/antora-invalid-descriptor"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "warnings do not fail by default"
    );
    let text = stdout(&output);
    assert!(text.starts_with("antora.yml:1:1: warning adoc.antora.missing-key "));

    let output = cli(&[
        "check",
        "tests/fixtures/antora-invalid-descriptor",
        "--fail-on",
        "warning",
    ]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn columns_count_characters_as_editors_do() {
    // Issue #533: `héllo wörld çà ` is 15 characters and 19 bytes, so the xref after it is at
    // column 16, where an editor and the Problems panel put it, not 20.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join("page.adoc"),
        "= Page\n\nhéllo wörld çà xref:missing.adoc[] xref:other.adoc#ici[]\n",
    )
    .unwrap();
    std::fs::write(root.join("other.adoc"), "= Other\n\nçà et là [[ici]]here\n").unwrap();
    let root = root.to_str().unwrap();

    let text = stdout(&cli(&["check", root]));
    assert!(text.starts_with("page.adoc:3:16: warning "), "{text}");
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&cli(&["check", root, "--format", "json"]))).unwrap();
    assert_eq!(json["diagnostics"][0]["column"], 16, "{json}");

    let page = format!("{root}/page.adoc");
    let offset = std::fs::read_to_string(&page)
        .unwrap()
        .find("xref:other")
        .unwrap()
        + 6;
    let output = cli(&["follow", &page, &offset.to_string()]);
    assert_eq!(stdout(&output).trim(), "other.adoc:3:10");
}

#[test]
fn follow_resolves_an_xref() {
    let output = cli(&["follow", "tests/fixtures/xrefs/index.adoc", "25"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "other.adoc:3:1");
}

#[test]
fn follow_resolves_across_components() {
    let page = "tests/fixtures/antora-multi-component/alpha/modules/ROOT/pages/index.adoc";
    let text = std::fs::read_to_string(repository_root().join(page)).unwrap();
    let offset = text.find("xref:beta::index.adoc").unwrap() + 6;
    let output = cli(&["follow", page, &offset.to_string()]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output).trim(),
        "beta/modules/ROOT/pages/index.adoc:1:1"
    );
}

#[test]
fn follow_reports_nothing_to_follow() {
    let output = cli(&["follow", "tests/fixtures/xrefs/index.adoc", "0"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn symbols_finds_headings_by_word() {
    let output = cli(&["symbols", "tests/fixtures/antora-multi-component", "refer"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("API Reference beta/modules/api/pages/reference.adoc:1:3"));
}

#[test]
fn rejects_unknown_commands() {
    assert_eq!(cli(&["frobnicate"]).status.code(), Some(2));
    assert_eq!(cli(&[]).status.code(), Some(2));
}

#[test]
fn render_writes_html() {
    let directory = tempfile::tempdir().unwrap();
    let output_path = directory.path().join("blocks.html");
    let output = cli(&[
        "render",
        "tests/fixtures/render/blocks.adoc",
        "--output",
        output_path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = std::fs::read_to_string(&output_path).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    assert!(html.contains("<title>Every Block Kind</title>"));
    assert!(html.contains("<div class=\"sect1\">"));
    assert!(
        html.contains("A paragraph from a partial."),
        "includes are followed"
    );

    // Without --output, the page goes to standard output.
    let output = cli(&["render", "tests/fixtures/simple/guide.adoc"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("<div id=\"content\">"));

    let output = cli(&[
        "render",
        "tests/fixtures/simple/guide.adoc",
        "--parser",
        "nope",
    ]);
    assert_eq!(output.status.code(), Some(2));
}

/// Issue #102: `--authoring` reads the page with an authoring profile's attributes, and an
/// unknown profile is an error that names it.
#[test]
fn render_reads_the_page_with_an_authoring_profile() {
    let render = |extra: &[&str]| {
        let mut arguments = vec![
            "render",
            "tests/fixtures/authoring/install.adoc",
            "--root",
            "tests/fixtures/authoring",
        ];
        arguments.extend_from_slice(extra);
        cli(&arguments)
    };

    let plain = stdout(&render(&[]));
    assert!(plain.contains("Run the installer."), "{plain}");
    assert!(!plain.contains("cloud console"), "{plain}");

    let output = render(&["--authoring", "cloud"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cloud = stdout(&output);
    assert!(cloud.contains("Sign in to the cloud console."), "{cloud}");
    assert!(cloud.contains("The Cloud edition."), "{cloud}");
    assert!(!cloud.contains("Run the installer."), "{cloud}");

    let unknown = render(&["--authoring", "staging"]);
    assert!(!unknown.status.success());
    let error = String::from_utf8_lossy(&unknown.stderr);
    assert!(error.contains("--authoring staging"), "{error}");
    assert!(error.contains("no authoring profile is named"), "{error}");
}

#[test]
fn render_to_a_file_embeds_local_images_unless_asked_to_link_them() {
    let directory = tempfile::tempdir().unwrap();
    let output_path = directory.path().join("guide.html");
    let render = |extra: &[&str]| {
        let mut arguments = vec![
            "render",
            "tests/fixtures/images/guide.adoc",
            "--output",
            output_path.to_str().unwrap(),
        ];
        arguments.extend_from_slice(extra);
        let output = cli(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::read_to_string(&output_path).unwrap()
    };

    // A file is meant to be moved or published, so it carries its images.
    let embedded = render(&[]);
    assert!(
        embedded.contains("src=\"data:image/svg+xml;base64,"),
        "{embedded}"
    );
    assert!(
        !embedded.contains("present.svg\""),
        "the image is not linked: {embedded}"
    );

    let linked = render(&["--link-images"]);
    assert!(linked.contains("src=\"file://"), "{linked}");
    assert!(linked.contains("present.svg\""), "{linked}");

    // On standard output, images stay linked, as before.
    let output = cli(&["render", "tests/fixtures/images/guide.adoc"]);
    assert!(stdout(&output).contains("src=\"file://"));
}

#[test]
fn check_accepts_the_current_directory_as_root() {
    let fixture = "tests/fixtures/antora-broken-references";
    let output = cli_in(fixture, &["check", ".", "--format", "json"]);
    assert_eq!(
        check_json_of(&output, "antora-broken-references"),
        check_json("antora-broken-references"),
    );
}

#[test]
fn render_resolves_antora_resources_from_the_current_directory() {
    let fixture = "tests/fixtures/antora-single-component";
    for root in [&["--root", "."][..], &[]] {
        let page = |path: &str| {
            let output = cli_in(fixture, &[&["render", path][..], root].concat());
            assert!(
                output.status.success(),
                "{root:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            stdout(&output)
        };
        let index = page("modules/ROOT/pages/index.adoc");
        assert!(
            // To where the page's own export sits beside its source, never a resource id,
            // which a browser would read as a URL scheme (#426).
            index.contains(
                "<a href=\"../../security/pages/authentication.html\">Authentication</a>"
            ),
            "{root:?}: {index}"
        );
        assert!(!index.contains("class=\"unresolved\""), "{root:?}: {index}");
        let authentication = page("modules/security/pages/authentication.adoc");
        assert!(
            authentication.contains("Authentication tokens must be protected."),
            "{root:?}: {authentication}"
        );
    }
}

#[test]
fn follow_accepts_the_current_directory_as_root() {
    let fixture = "tests/fixtures/antora-single-component";
    let page = "modules/ROOT/pages/index.adoc";
    let text = std::fs::read_to_string(repository_root().join(fixture).join(page)).unwrap();
    let offset = text.find("xref:").unwrap() + 6;
    let output = cli_in(
        fixture,
        &["follow", page, &offset.to_string(), "--root", "."],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout(&output).trim(),
        "modules/security/pages/authentication.adoc:1:1"
    );
}

#[test]
fn git_status_lists_changed_pages_with_hunk_counts() {
    let dir = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(dir.path());
    test.write("docs/page.adoc", "= Page\n\nOne.\n\nTwo.\n\nThree.\n");
    test.write("docs/same.adoc", "= Same\n");
    test.write("README.md", "readme\n");
    test.commit_all("first");
    test.write(
        "docs/page.adoc",
        "= Page\n\nOne, changed.\n\nTwo.\n\nThree.\n\nFour.\n",
    );
    test.write("docs/new.adoc", "= New\n");
    test.write("README.md", "readme, changed\n");

    let root = dir.path().to_str().unwrap();
    let output = cli(&["git-status", root, "--format", "json"]);
    assert!(output.status.success(), "{output:?}");
    let status: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(status["head"], "main");
    let files = status["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "only changed AsciiDoc files: {files:?}");
    assert_eq!(files[0]["path"], "docs/new.adoc");
    assert_eq!(files[0]["worktree"], "untracked");
    assert_eq!(files[1]["path"], "docs/page.adoc");
    assert_eq!(files[1]["worktree"], "modified");
    assert_eq!(files[1]["hunks"], 2);

    let text = stdout(&cli(&["git-status", root]));
    assert!(text.starts_with("branch main\n"), "{text}");
    assert!(text.contains(" M docs/page.adoc (2 changed)"), "{text}");
    assert!(text.contains("?? docs/new.adoc"), "{text}");

    let outside = tempfile::tempdir().unwrap();
    if !outside
        .path()
        .ancestors()
        .any(|ancestor| ancestor.join(".git").exists())
    {
        let output = cli(&["git-status", outside.path().to_str().unwrap()]);
        assert!(!output.status.success());
    }
}

#[test]
fn pdf_exports_a_page() {
    let directory = tempfile::tempdir().unwrap();
    let pdf = directory.path().join("article.pdf");
    let typst = directory.path().join("article.typ");
    let output = cli(&[
        "pdf",
        "tests/fixtures/pdf/reference/article.adoc",
        "--output",
        pdf.to_str().unwrap(),
        "--typst",
        typst.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
    assert!(stdout(&output).contains("(4 pages)"), "{}", stdout(&output));
    assert!(std::fs::read_to_string(&typst)
        .unwrap()
        .contains("#adoc-doctitle[#\"Exporting Documentation to PDF\"]"));
}

#[test]
fn pdf_fail_on_warning_refuses_a_diagram_left_as_source() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    // In a list item, where the export once missed it (#433).
    std::fs::write(
        &page,
        "= Page\n\n* An item\n+\n[graphviz]\n....\ndigraph { a -> b }\n....\n",
    )
    .unwrap();
    // An empty PATH, so `dot` is never found.
    let empty = directory.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(directory.path())
            .env("PATH", &empty)
            .args(["pdf", "page.adoc", "--output", "page.pdf"])
            .args(extra)
            .output()
            .unwrap()
    };

    let output = run(&[]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("page.adoc:5: warning: graphviz diagram not rendered"),
        "{stderr}"
    );

    let output = run(&["--fail-on", "warning"]);
    assert_eq!(output.status.code(), Some(1));
    // The PDF is written all the same, for whoever wants to look at it.
    assert!(std::fs::read(directory.path().join("page.pdf"))
        .unwrap()
        .starts_with(b"%PDF-"));

    let output = run(&["--fail-on", "error"]);
    assert!(output.status.success());
    let output = run(&["--fail-on", "sometimes"]);
    assert!(!output.status.success());
}

#[test]
fn pdf_exports_a_component() {
    // Copied, so the diagram cache is written outside the repository.
    let directory = tempfile::tempdir().unwrap();
    copy_tree(
        &repository_root().join("tests/fixtures/pdf/book"),
        &directory.path().join("book"),
    );
    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(directory.path())
            .args(arguments)
            .output()
            .unwrap()
    };
    // A component directory exports its book, named after the component; given by a relative
    // name, its siblings are in the workspace too, so references to them resolve.
    let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(directory.path().join("book"))
        .args(["pdf", "guide", "--typst", "guide.typ"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let typst = std::fs::read_to_string(directory.path().join("book/guide.typ")).unwrap();
    assert!(
        !typst.contains("adoc-link(label(\"p2\"), [#\"other component\"])"),
        "the reference to the other component leads back to its own page"
    );
    let output = run(&["pdf", "book/guide"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(directory.path().join("guide.pdf").is_file());
    // So does `--component`, with an Antora version.
    let output = run(&[
        "pdf",
        "book",
        "--component",
        "1.0@guide",
        "--output",
        "named.pdf",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout(&output).starts_with("named.pdf ("),
        "{}",
        stdout(&output)
    );
}

#[test]
fn pdf_reports_theme_errors() {
    let directory = tempfile::tempdir().unwrap();
    let theme = directory.path().join("broken-theme.yml");
    std::fs::write(&theme, "base: [unclosed\n").unwrap();
    let output = cli(&[
        "pdf",
        "tests/fixtures/pdf/reference/article.adoc",
        "--theme",
        theme.to_str().unwrap(),
        "--output",
        directory.path().join("x.pdf").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("broken-theme.yml"), "{stderr}");
    assert!(!directory.path().join("x.pdf").exists());
    // An unknown standard is a usage error.
    let output = cli(&[
        "pdf",
        "tests/fixtures/pdf/reference/article.adoc",
        "--standard",
        "pdf-x",
    ]);
    assert_eq!(output.status.code(), Some(2));
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn doctor_reports_the_program_tools_and_settings() {
    let output = cli(&["doctor", "tests/fixtures/simple", "--format", "json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let titles: Vec<&str> = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| section["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Program", "System", "PDF", "Tools", "Settings", "Spelling"]
    );
    assert_eq!(
        report["sections"][0]["lines"][0]["value"],
        format!("adocfoundry-cli {}", env!("CARGO_PKG_VERSION"))
    );
    let tools: Vec<&str> = report["sections"][3]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["label"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["asciidoctor", "dot", "plantuml", "d2", "git"]);

    let text = cli(&["doctor"]);
    assert!(text.status.success());
    assert!(stdout(&text).starts_with("AdocFoundry diagnostics\n"));
}

#[test]
fn pdf_profile_writes_what_the_app_writes() {
    // Issue #103: a profile exported from the command line and from the app gives the same PDF.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&repository_root().join("tests/fixtures/pdf/book"), &root);
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf.profiles.manual]\nnav = [\"guide/modules/admin/nav.adoc\", \"guide/modules/ROOT/nav.adoc\"]\noutput = \"build/manual.pdf\"\nattributes = { pdf-title = \"The Manual\" }\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(directory.path())
        .args(["pdf", "--profile", "manual", "book"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = root.join("build/manual.pdf");
    assert!(
        stdout(&output).contains("manual.pdf ("),
        "{}",
        stdout(&output)
    );
    let from_cli = std::fs::read(&written).unwrap();
    std::fs::remove_file(&written).unwrap();

    // The app's command, on an engine opened as the app opens one.
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    let wait = std::time::Duration::from_secs(120);
    assert!(engine.wait_for_workers(wait));
    engine
        .invoke(
            adoc_engine::commands::ids::EXPORT_PDF_PROFILE,
            adoc_engine::CommandArgs::Profile("manual".to_owned()),
        )
        .unwrap();
    while engine.is_exporting() {
        assert!(engine.wait_for_workers(wait));
    }
    assert!(
        std::fs::read(&written).unwrap() == from_cli,
        "the app's PDF differs from the command line's"
    );

    // An unknown profile is an error that names it.
    let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(directory.path())
        .args(["pdf", "--profile", "missing", "book"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no PDF profile `missing`"));
}

#[test]
fn pdf_profile_from_inside_the_workspace_writes_only_inside_it() {
    // Issue #528: from a relative root, `../../out` was read as `out` and permitted.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("a/b/book");
    copy_tree(&repository_root().join("tests/fixtures/pdf/book"), &root);
    std::fs::create_dir_all(directory.path().join("a/out")).unwrap();
    std::fs::create_dir_all(root.join("out")).unwrap();
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf.profiles.manual]\nnav = [\"guide/modules/ROOT/nav.adoc\"]\noutput = \"../../out/manual.pdf\"\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(&root)
        .args(["pdf", "--profile", "manual"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside the workspace"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!directory.path().join("a/out/manual.pdf").exists());
    assert!(!root.join("out/manual.pdf").exists());
}

#[test]
fn new_module_makes_a_module_through_the_engine_command() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    copy_tree(
        &repository_root().join("tests/fixtures/antora-single-component"),
        &root,
    );
    let root_text = root.to_string_lossy().into_owned();
    let output = cli(&[
        "new-module",
        &root_text,
        "admin",
        "--families",
        "pages,partials",
        "--index-title",
        "Administration",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let descriptor = std::fs::read_to_string(root.join("antora.yml")).unwrap();
    insta::assert_snapshot!(
        "new_module",
        format!("{}\n--- antora.yml\n{descriptor}", stdout(&output))
    );
    assert!(
        std::fs::read_to_string(root.join("modules/admin/pages/index.adoc"))
            .unwrap()
            .starts_with("= Administration\n")
    );

    let again = cli(&["new-module", &root_text, "admin"]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("exists already"));
}

#[test]
fn new_project_from_a_template_checks_clean() {
    let directory = tempfile::tempdir().unwrap();
    for (template, name) in [("multi-component", Some("shop")), ("single-page", None)] {
        let root = directory.path().join(template);
        std::fs::create_dir(&root).unwrap();
        let root_text = root.to_string_lossy().into_owned();
        let mut arguments = vec![
            "new-project",
            &root_text,
            "--template",
            template,
            "--no-git",
        ];
        if let Some(name) = name {
            arguments.extend(["--name", name]);
        }
        let output = cli(&arguments);
        assert!(
            output.status.success(),
            "{template}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let check = cli(&["check", &root_text]);
        assert_eq!(stdout(&check), "", "{template}: check reports nothing");
        match template {
            "multi-component" => {
                assert!(stdout(&output).contains("made components/reference/antora.yml"));
                assert!(root.join("components/guide/antora.yml").is_file());
            }
            _ => assert!(root.join("index.adoc").is_file()),
        }
    }
    // A template that names components needs `--name`; an unknown one is refused.
    let root = directory.path().join("unnamed");
    std::fs::create_dir(&root).unwrap();
    let root_text = root.to_string_lossy().into_owned();
    let output = cli(&["new-project", &root_text, "--template", "product-docs"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("--name"));
    let output = cli(&[
        "new-project",
        &root_text,
        "--name",
        "x",
        "--template",
        "nope",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no template `nope`"));
}

#[test]
fn new_project_scaffolds_a_component_that_checks_clean() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("site");
    std::fs::create_dir(&root).unwrap();
    let root_text = root.to_string_lossy().into_owned();
    let output = cli(&["new-project", &root_text, "--name", "demo", "--no-git"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let descriptor = std::fs::read_to_string(root.join("antora.yml")).unwrap();
    insta::assert_snapshot!(
        "new_project",
        format!("{}\n--- antora.yml\n{descriptor}", stdout(&output))
    );
    assert!(!root.join(".git").exists());

    let check = cli(&["check", &root_text]);
    assert!(check.status.success(), "{}", stdout(&check));
    assert_eq!(stdout(&check), "", "check reports nothing");

    // A folder that is not empty is refused, unless the project goes in a subfolder.
    let again = cli(&["new-project", &root_text, "--name", "other"]);
    assert_eq!(again.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&again.stderr).contains("New Module"));
    let code = directory.path().join("code");
    std::fs::create_dir(&code).unwrap();
    std::fs::write(code.join("main.rs"), "fn main() {}\n").unwrap();
    let code_text = code.to_string_lossy().into_owned();
    let output = cli(&[
        "new-project",
        &code_text,
        "--name",
        "tool",
        "--subfolder",
        "docs",
        "--title",
        "The Tool",
        "--version",
        "1.0",
        "--no-readme",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout(&output).contains("made docs/antora.yml"),
        "{}",
        stdout(&output)
    );
    assert!(!code.join("docs/README.adoc").exists());
    assert!(code.join(".git").is_dir(), "Git initialised by default");
    let descriptor = std::fs::read_to_string(code.join("docs/antora.yml")).unwrap();
    assert!(
        descriptor.contains("title: \"The Tool\"\nversion: \"1.0\"\n"),
        "{descriptor}"
    );
}

/// Output to a pipe whose reader has gone (`check docs | head`) ends the output, not the
/// program: the exit code is the one the command earned, never a panic's 101 (#513).
#[test]
fn a_closed_standard_output_ends_the_output_quietly() {
    let run = |arguments: &[&str]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(repository_root())
            .args(arguments)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // The reader goes before the command writes anything.
        drop(child.stdout.take());
        child.wait_with_output().unwrap()
    };
    for (arguments, earned) in [
        (&["check", "tests/fixtures/antora-broken-references"][..], 0),
        (
            &[
                "check",
                "tests/fixtures/antora-broken-references",
                "--fail-on",
                "warning",
            ][..],
            1,
        ),
        (
            &["symbols", "tests/fixtures/antora-multi-component", "e"][..],
            0,
        ),
        (&["graph", "tests/fixtures/graph"][..], 0),
    ] {
        let output = run(arguments);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(earned),
            "{arguments:?}: {stderr}"
        );
        assert!(!stderr.contains("panicked"), "{arguments:?}: {stderr}");
    }
}

/// An image target opens in an image tab in the app; here it is printed as an attachment's
/// is, rather than failing as an unexpected outcome (#506).
#[test]
fn follow_prints_an_image_target() {
    let page = "tests/fixtures/images/guide.adoc";
    let text = std::fs::read_to_string(repository_root().join(page)).unwrap();
    let offset = text.find("present.svg").unwrap() + 2;
    let output = cli(&["follow", page, &offset.to_string()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout(&output).trim(), "external media/present.svg");
}

/// What could not be read was not checked, so `check` says so rather than passing it in
/// silence: a folder the index left out is an error in the report, and an index that failed
/// is a runtime error (#505).
#[cfg(unix)]
#[test]
fn check_reports_what_it_could_not_read() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    copy_tree(
        &repository_root().join("tests/fixtures/antora-broken-references"),
        &root,
    );
    let notes = root.join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("note.adoc"), "= Note\n\nxref:nope.adoc[]\n").unwrap();
    let lock = |folder: &Path, mode| {
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let check = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(directory.path())
            .args(arguments)
            .output()
            .unwrap()
    };

    lock(&notes, 0o000);
    let readable = std::fs::read_dir(&notes).is_ok();
    let text = check(&["check", "docs"]);
    let json = check(&["check", "docs", "--format", "json"]);
    lock(&notes, 0o755);
    if readable {
        // Running with the rights to read anything (as root), so nothing is unreadable.
        return;
    }
    assert_eq!(text.status.code(), Some(1), "an error by default");
    assert!(
        stdout(&text)
            .lines()
            .any(|line| line.starts_with("notes:1:1: error adoc.unreadable ")),
        "{}",
        stdout(&text)
    );
    let entries: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(entries["summary"]["error"], 1, "{entries}");
    assert!(
        entries["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "notes" && entry["code"] == "adoc.unreadable"),
        "{entries}"
    );

    // A module's family folder that cannot be read fails Antora discovery, and the index.
    let pages = root.join("modules/ROOT/pages");
    lock(&pages, 0o000);
    let failed = check(&["check", "docs"]);
    lock(&pages, 0o755);
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert_eq!(failed.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("indexing failed"), "{stderr}");
}

/// Every path argument is read against the current directory, so the workspace is the same
/// wherever the command runs from and however its paths are written (#510).
#[test]
fn paths_are_read_against_the_current_directory() {
    let succeeded = |output: &Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        stdout(output)
    };

    // From inside a pages folder, the component above is still found.
    let pages = "tests/fixtures/antora-single-component/modules/ROOT/pages";
    let html = succeeded(&cli_in(pages, &["render", "index.adoc"]));
    assert!(
        html.contains("<a href=\"../../security/pages/authentication.html\">Authentication</a>"),
        "{html}"
    );
    assert!(!html.contains("class=\"unresolved\""), "{html}");

    // From a component's own folder, its siblings are in the workspace.
    let alpha = "tests/fixtures/antora-multi-component/alpha";
    let page = "modules/ROOT/pages/index.adoc";
    let text = std::fs::read_to_string(repository_root().join(alpha).join(page)).unwrap();
    let offset = (text.find("xref:beta::index.adoc").unwrap() + 6).to_string();
    let followed = succeeded(&cli_in(alpha, &["follow", page, &offset]));
    assert_eq!(followed.trim(), "beta/modules/ROOT/pages/index.adoc:1:1");

    // An absolute root with a relative file.
    let fixture = "tests/fixtures/antora-single-component";
    let absolute = std::path::absolute(repository_root().join(fixture)).unwrap();
    let absolute = absolute.to_str().unwrap();
    let page = format!("{fixture}/modules/ROOT/pages/index.adoc");
    let text = std::fs::read_to_string(repository_root().join(&page)).unwrap();
    let offset = (text.find("xref:").unwrap() + 6).to_string();
    let followed = succeeded(&cli(&["follow", &page, &offset, "--root", absolute]));
    assert_eq!(
        followed.trim(),
        "modules/security/pages/authentication.adoc:1:1"
    );
    let html = succeeded(&cli(&["render", &page, "--root", absolute]));
    assert!(!html.contains("class=\"unresolved\""), "{html}");

    // An absolute root with a relative page, and the reverse.
    let graph = std::path::absolute(repository_root().join("tests/fixtures/graph")).unwrap();
    let configure = "tests/fixtures/graph/guide/modules/ROOT/pages/configure.adoc";
    succeeded(&cli(&[
        "graph",
        graph.to_str().unwrap(),
        "--page",
        configure,
    ]));
    succeeded(&cli(&[
        "graph",
        "tests/fixtures/graph",
        "--page",
        graph
            .join("guide/modules/ROOT/pages/configure.adoc")
            .to_str()
            .unwrap(),
    ]));

    // A relative component folder with an absolute root. Copied, so the diagram cache is
    // written outside the repository.
    let directory = tempfile::tempdir().unwrap();
    copy_tree(
        &repository_root().join("tests/fixtures/pdf/book"),
        &directory.path().join("book"),
    );
    // As the command reads its current directory: on Unix with any link in the temporary
    // folder's path followed (macOS's `/var` is a link), on Windows as it was set, since
    // `canonicalize` there gives the `\\?\` form with long names where the temporary folder
    // may be written with short ones (`RUNNER~1`).
    let base = if cfg!(windows) {
        directory.path().to_path_buf()
    } else {
        directory.path().canonicalize().unwrap()
    };
    let book = base.join("book");
    let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(directory.path())
        .args(["pdf", "book/guide", "--root", book.to_str().unwrap()])
        .output()
        .unwrap();
    succeeded(&output);
    assert!(directory.path().join("guide.pdf").is_file());
}

/// A component folder is exported as the component version it holds, and to a file in the
/// current directory whatever the component's name says (#511).
#[test]
fn pdf_exports_the_component_folder_named_to_the_current_directory() {
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let docs = work.join("docs");
    copy_tree(
        &repository_root().join("tests/fixtures/antora-versions"),
        &docs,
    );
    // Unversioned and a prerelease, so not the latest version: 2.0 is.
    std::fs::write(
        docs.join("main/antora.yml"),
        "name: product\nversion: ~\nprerelease: true\n",
    )
    .unwrap();
    let pdf = |arguments: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(&work)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        stdout(&output)
    };

    pdf(&["pdf", "docs/main", "--typst", "main.typ"]);
    let typst = std::fs::read_to_string(work.join("main.typ")).unwrap();
    assert!(typst.contains("[#\"Product\"]"), "{typst}");
    assert!(!typst.contains("Product 2.0"), "{typst}");
    // Without a version, `--component` is the latest version, as Antora reads it.
    pdf(&[
        "pdf",
        "docs",
        "--component",
        "product",
        "--typst",
        "latest.typ",
        "--output",
        "latest.pdf",
    ]);
    let typst = std::fs::read_to_string(work.join("latest.typ")).unwrap();
    assert!(typst.contains("Product 2.0"), "{typst}");

    // A name that is a path writes only its last part, here.
    for name in ["api/v2", "../../escaped"] {
        std::fs::write(
            docs.join("v1/antora.yml"),
            format!("name: '{name}'\nversion: '1.0'\n"),
        )
        .unwrap();
        let last = name.rsplit('/').next().unwrap();
        let printed = pdf(&["pdf", "docs/v1"]);
        assert!(printed.starts_with(&format!("{last}.pdf (")), "{printed}");
        assert!(work.join(format!("{last}.pdf")).is_file(), "{name}");
    }
    assert!(!directory.path().join("escaped.pdf").exists());
    assert!(!work.join("api").exists());
}

/// The Kindle export PRD, §12 test 16: an article, a book and a profile in both formats, the
/// format from `--format` or the output's extension, with exit codes that follow `--fail-on`.
#[test]
fn kindle_cli_exports_and_gates() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&repository_root().join("tests/fixtures/pdf/book"), &root);
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[kindle.profiles.handbook]\ncomponent = \"1.0@guide\"\nformat = \"kfx\"\noutput = \"build/handbook.kfx\"\n",
    )
    .unwrap();
    let run = |folder: &Path, arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(folder)
            .args(arguments)
            .output()
            .unwrap()
    };
    let succeeded = |output: &Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let is_azw3 = |path: &Path| std::fs::read(path).unwrap()[60..68] == *b"BOOKMOBI";
    let is_kfx = |path: &Path| std::fs::read(path).unwrap().starts_with(b"CONT");

    // A page: AZW3 by default, beside it.
    let output = run(&root, &["kindle", "guide/modules/ROOT/pages/install.adoc"]);
    succeeded(&output);
    assert!(is_azw3(&root.join("guide/modules/ROOT/pages/install.azw3")));
    assert!(
        stdout(&output).contains("install.azw3 (1 AsciiDoc page)"),
        "{}",
        stdout(&output)
    );
    // KFX by `--format`, and by the output's extension.
    let output = run(
        &root,
        &[
            "kindle",
            "guide/modules/ROOT/pages/install.adoc",
            "--format",
            "kfx",
        ],
    );
    succeeded(&output);
    assert!(is_kfx(&root.join("guide/modules/ROOT/pages/install.kfx")));
    succeeded(&run(
        &root,
        &[
            "kindle",
            "guide/modules/ROOT/pages/install.adoc",
            "--output",
            "named.kfx",
        ],
    ));
    assert!(is_kfx(&root.join("named.kfx")));
    // A format and an output that disagree, or a format that is not Kindle's, are refused.
    let output = run(
        &root,
        &[
            "kindle",
            "guide/modules/ROOT/pages/install.adoc",
            "--format",
            "kfx",
            "--output",
            "x.azw3",
        ],
    );
    assert!(!output.status.success());
    assert!(!root.join("x.azw3").exists());
    assert!(!run(
        &root,
        &[
            "kindle",
            "guide/modules/ROOT/pages/install.adoc",
            "--output",
            "x.epub"
        ]
    )
    .status
    .success());
    assert!(!run(
        &root,
        &[
            "kindle",
            "guide/modules/ROOT/pages/install.adoc",
            "--format",
            "mobi"
        ]
    )
    .status
    .success());

    // A component folder: its book, in both formats.
    let output = run(directory.path(), &["kindle", "book/guide"]);
    succeeded(&output);
    assert!(is_azw3(&directory.path().join("guide.azw3")));
    assert!(
        stdout(&output).contains("(8 AsciiDoc pages)"),
        "{}",
        stdout(&output)
    );
    succeeded(&run(
        directory.path(),
        &[
            "kindle",
            "book",
            "--component",
            "1.0@guide",
            "--format",
            "kfx",
        ],
    ));
    assert!(is_kfx(&directory.path().join("guide.kfx")));

    // A profile writes what the app's command writes.
    let output = run(
        directory.path(),
        &["kindle", "--profile", "handbook", "book"],
    );
    succeeded(&output);
    let written = root.join("build/handbook.kfx");
    let from_cli = std::fs::read(&written).unwrap();
    std::fs::remove_file(&written).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    let wait = std::time::Duration::from_secs(120);
    assert!(engine.wait_for_workers(wait));
    engine
        .invoke(
            adoc_engine::commands::ids::EXPORT_KINDLE_PROFILE,
            adoc_engine::CommandArgs::Profile("handbook".to_owned()),
        )
        .unwrap();
    while engine.is_exporting() {
        assert!(engine.wait_for_workers(wait));
    }
    assert!(
        std::fs::read(&written).unwrap() == from_cli,
        "the app's book differs from the command line's"
    );
    // The profile in the other format, by `--format`.
    succeeded(&run(
        directory.path(),
        &[
            "kindle",
            "--profile",
            "handbook",
            "book",
            "--format",
            "azw3",
        ],
    ));
    assert!(is_azw3(&root.join("build/handbook.azw3")));
    let output = run(
        directory.path(),
        &["kindle", "--profile", "missing", "book"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no Kindle profile `missing`"));

    // `--fail-on`: a reference that leads nowhere is a warning; the book is written all the
    // same.
    std::fs::write(
        root.join("guide/modules/ROOT/pages/broken.adoc"),
        "= Broken\n\nSee xref:missing.adoc[nowhere].\n",
    )
    .unwrap();
    let page = [
        "kindle",
        "guide/modules/ROOT/pages/broken.adoc",
        "--output",
        "broken.kfx",
    ];
    let output = run(&root, &page);
    succeeded(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("broken.adoc:3: warning: the cross reference missing.adoc leads to nothing"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run(&root, &[&page[..], &["--fail-on", "warning"]].concat());
    assert_eq!(output.status.code(), Some(1));
    assert!(is_kfx(&root.join("broken.kfx")));
    succeeded(&run(&root, &[&page[..], &["--fail-on", "error"]].concat()));
    succeeded(&run(&root, &[&page[..], &["--fail-on", "none"]].concat()));
}

/// The EPUB export PRD, §11 test 19: an article, a book and a profile, each a valid EPUB, with
/// exit codes that follow `--fail-on`, and a note that never fails an export.
#[test]
fn epub_cli_exports_and_gates() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&repository_root().join("tests/fixtures/pdf/book"), &root);
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[epub.profiles.handbook]\ncomponent = \"1.0@guide\"\noutput = \"build/handbook.epub\"\n\
         modified = \"2026-11-19\"\n",
    )
    .unwrap();
    // Dated, so a book does not carry the time it was exported, unless a test says otherwise.
    let command = |folder: &Path, arguments: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"));
        command
            .current_dir(folder)
            .args(arguments)
            .env("SOURCE_DATE_EPOCH", "1795046400");
        command
    };
    let run = |folder: &Path, arguments: &[&str]| command(folder, arguments).output().unwrap();
    let succeeded = |output: &Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let is_epub = |path: &Path| {
        let (_, package) = adoc_epub::testing::assert_valid_ocf(&std::fs::read(path).unwrap());
        package
    };

    // A page: beside it.
    let output = run(&root, &["epub", "guide/modules/ROOT/pages/install.adoc"]);
    succeeded(&output);
    let package = is_epub(&root.join("guide/modules/ROOT/pages/install.epub"));
    assert_eq!(package.modified, "2026-11-19T00:00:00Z");
    assert!(
        stdout(&output).contains("install.epub (1 AsciiDoc page)"),
        "{}",
        stdout(&output)
    );
    succeeded(&run(
        &root,
        &[
            "epub",
            "guide/modules/ROOT/pages/install.adoc",
            "--output",
            "named.epub",
        ],
    ));
    is_epub(&root.join("named.epub"));
    // An output that is not an `.epub` file is refused, and nothing is written.
    for wrong in ["x.kfx", "x.docx", "x"] {
        let output = run(
            &root,
            &[
                "epub",
                "guide/modules/ROOT/pages/install.adoc",
                "--output",
                wrong,
            ],
        );
        assert_eq!(output.status.code(), Some(2), "{wrong}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("is not an .epub file"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!root.join(wrong).exists());
    }

    // A component folder: its book, in the current folder; and by `--component`.
    let output = run(directory.path(), &["epub", "book/guide"]);
    succeeded(&output);
    is_epub(&directory.path().join("guide.epub"));
    assert!(
        stdout(&output).contains("guide.epub (8 AsciiDoc pages)"),
        "{}",
        stdout(&output)
    );
    succeeded(&run(
        directory.path(),
        &[
            "epub",
            "book",
            "--component",
            "1.0@guide",
            "--output",
            "by-name.epub",
        ],
    ));
    is_epub(&directory.path().join("by-name.epub"));

    // A profile writes what the app's command writes, dated by the profile.
    let output = run(directory.path(), &["epub", "--profile", "handbook", "book"]);
    succeeded(&output);
    let written = root.join("build/handbook.epub");
    let from_cli = std::fs::read(&written).unwrap();
    is_epub(&written);
    std::fs::remove_file(&written).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    let wait = std::time::Duration::from_secs(120);
    assert!(engine.wait_for_workers(wait));
    engine
        .invoke(
            adoc_engine::commands::ids::EXPORT_EPUB_PROFILE,
            adoc_engine::CommandArgs::Profile("handbook".to_owned()),
        )
        .unwrap();
    while engine.is_exporting() {
        assert!(engine.wait_for_workers(wait));
    }
    assert!(
        std::fs::read(&written).unwrap() == from_cli,
        "the app's book differs from the command line's"
    );
    // The profile written elsewhere by `--output`.
    succeeded(&run(
        directory.path(),
        &[
            "epub",
            "--profile",
            "handbook",
            "book",
            "--output",
            "book/build/elsewhere.epub",
        ],
    ));
    is_epub(&root.join("build/elsewhere.epub"));
    let output = run(directory.path(), &["epub", "--profile", "missing", "book"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no EPUB profile `missing`"));

    // `--fail-on`: a reference that leads nowhere is a warning; the book is written all the
    // same.
    std::fs::write(
        root.join("guide/modules/ROOT/pages/broken.adoc"),
        "= Broken\n:lang: en-GB\n\nSee xref:missing.adoc[nowhere].\n",
    )
    .unwrap();
    let page = [
        "epub",
        "guide/modules/ROOT/pages/broken.adoc",
        "--output",
        "broken.epub",
    ];
    let output = run(&root, &page);
    succeeded(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("broken.adoc:4: warning: the cross reference missing.adoc leads to nothing"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(root.join("broken.epub")).unwrap();
    let output = run(&root, &[&page[..], &["--fail-on", "warning"]].concat());
    assert_eq!(output.status.code(), Some(1));
    is_epub(&root.join("broken.epub"));
    succeeded(&run(&root, &[&page[..], &["--fail-on", "error"]].concat()));
    succeeded(&run(&root, &[&page[..], &["--fail-on", "none"]].concat()));
    assert!(
        !run(&root, &[&page[..], &["--fail-on", "sometimes"]].concat())
            .status
            .success()
    );

    // An undated book is noted, and the note never fails it, even under `--fail-on warning`.
    std::fs::write(
        root.join("guide/modules/ROOT/pages/clean.adoc"),
        "= Clean\n:lang: en-GB\n\nNothing here leads anywhere.\n",
    )
    .unwrap();
    let output = command(
        &root,
        &[
            "epub",
            "guide/modules/ROOT/pages/clean.adoc",
            "--fail-on",
            "warning",
        ],
    )
    .env_remove("SOURCE_DATE_EPOCH")
    .output()
    .unwrap();
    succeeded(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("info: the book is dated with the time of the export"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    is_epub(&root.join("guide/modules/ROOT/pages/clean.epub"));
}

/// The DOCX export PRD, §10 test 15: an article, a book and a profile, with exit codes that
/// follow `--fail-on`, and nothing of PDF's.
#[test]
fn docx_cli_exports_and_gates() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&repository_root().join("tests/fixtures/pdf/book"), &root);
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[docx.profiles.manual]\nnav = [\"guide/modules/admin/nav.adoc\", \"guide/modules/ROOT/nav.adoc\"]\noutput = \"build/manual.docx\"\n",
    )
    .unwrap();
    let run = |folder: &Path, arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(folder)
            .args(arguments)
            .output()
            .unwrap()
    };
    let succeeded = |output: &Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let is_docx = |path: &Path| std::fs::read(path).unwrap().starts_with(b"PK\x03\x04");

    // A page: beside it, named as it is.
    let output = run(&root, &["docx", "guide/modules/ROOT/pages/install.adoc"]);
    succeeded(&output);
    assert!(is_docx(&root.join("guide/modules/ROOT/pages/install.docx")));
    assert!(
        stdout(&output).contains("install.docx (1 AsciiDoc page)"),
        "{}",
        stdout(&output)
    );

    // A component folder: its book, named after the component, in the current folder.
    let output = run(directory.path(), &["docx", "book/guide"]);
    succeeded(&output);
    assert!(is_docx(&directory.path().join("guide.docx")));
    assert!(
        stdout(&output).contains("(8 AsciiDoc pages)"),
        "{}",
        stdout(&output)
    );
    let output = run(
        directory.path(),
        &[
            "docx",
            "book",
            "--component",
            "1.0@guide",
            "--output",
            "named.docx",
        ],
    );
    succeeded(&output);
    assert!(
        stdout(&output).starts_with("named.docx ("),
        "{}",
        stdout(&output)
    );

    // A profile writes what the app's command writes.
    let output = run(directory.path(), &["docx", "--profile", "manual", "book"]);
    succeeded(&output);
    let written = root.join("build/manual.docx");
    let from_cli = std::fs::read(&written).unwrap();
    std::fs::remove_file(&written).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    let wait = std::time::Duration::from_secs(120);
    assert!(engine.wait_for_workers(wait));
    engine
        .invoke(
            adoc_engine::commands::ids::EXPORT_DOCX_PROFILE,
            adoc_engine::CommandArgs::Profile("manual".to_owned()),
        )
        .unwrap();
    while engine.is_exporting() {
        assert!(engine.wait_for_workers(wait));
    }
    assert!(
        std::fs::read(&written).unwrap() == from_cli,
        "the app's document differs from the command line's"
    );
    let output = run(directory.path(), &["docx", "--profile", "missing", "book"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no DOCX profile `missing`"));

    // `--fail-on`: a reference that leads nowhere is a warning; the document is written all
    // the same.
    std::fs::write(
        root.join("guide/modules/ROOT/pages/broken.adoc"),
        "= Broken\n\nSee xref:missing.adoc[nowhere].\n",
    )
    .unwrap();
    let page = [
        "docx",
        "guide/modules/ROOT/pages/broken.adoc",
        "--output",
        "broken.docx",
    ];
    let output = run(&root, &page);
    succeeded(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("broken.adoc:3: warning: the cross reference missing.adoc leads to nothing"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run(&root, &[&page[..], &["--fail-on", "warning"]].concat());
    assert_eq!(output.status.code(), Some(1));
    assert!(is_docx(&root.join("broken.docx")));
    succeeded(&run(&root, &[&page[..], &["--fail-on", "error"]].concat()));
    assert!(
        !run(&root, &[&page[..], &["--fail-on", "sometimes"]].concat())
            .status
            .success()
    );

    // Nothing of PDF's but `--theme`, which chooses the Word look: its other flags are refused,
    // and so is an output that is not a `.docx` file.
    for extra in [["--standard", "pdf-ua-1"], ["--typst", "x.typ"]] {
        assert!(
            !run(&root, &[&page[..], &extra[..]].concat())
                .status
                .success(),
            "{extra:?}"
        );
    }
    let output = run(
        &root,
        &[
            "docx",
            "guide/modules/ROOT/pages/install.adoc",
            "--output",
            "install.pdf",
        ],
    );
    assert!(!output.status.success());
    assert!(!root.join("install.pdf").exists());
}

/// The Word looks PRD's §9 test 10, for the command line: `--theme`, `--template` and
/// `--no-theme`, and a profile's look, write the same bytes as the engine exporting with the
/// same `LookChoice`, the reference every front end is held to (the app's and the MCP
/// server's tests compare theirs with it too).
#[test]
fn docx_look_choice_everywhere() {
    use adoc_engine::{DocxExportArgs, Engine, ExportTarget, LookChoice, Settings};

    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("book"));
    copy_tree(&repository_root().join("tests/fixtures/pdf/book"), &root);
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(
        root.join("themes/sans-theme.yml"),
        "extends: default-sans\nbase:\n  font-color: #112233\n",
    )
    .unwrap();
    std::fs::write(
        root.join("themes/corporate.dotx"),
        adoc_docx::testing::template(|_| {}),
    )
    .unwrap();
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf]\nthemes_dir = \"themes\"\n\n\
         [docx.profiles.themed]\npage = \"guide/modules/ROOT/pages/install.adoc\"\ntheme = \"sans\"\n\n\
         [docx.profiles.templated]\npage = \"guide/modules/ROOT/pages/install.adoc\"\ntemplate = \"corporate\"\n\n\
         [docx.profiles.plain]\npage = \"guide/modules/ROOT/pages/install.adoc\"\nlook = \"none\"\n",
    )
    .unwrap();
    let page = "guide/modules/ROOT/pages/install.adoc";
    let run = |arguments: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(&root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    // The reference: the engine itself, with the settings the command line reads.
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(60)));
    let id = engine.open(&root.join(page)).unwrap();
    let mut reference = |look: LookChoice, name: &str| {
        let output = directory.path().join(name);
        engine
            .export_docx_blocking(
                DocxExportArgs {
                    target: ExportTarget::Page(id),
                    output: output.clone(),
                    attributes: Vec::new(),
                    paper: None,
                    landscape: None,
                    look: Some(look),
                },
                std::time::Duration::from_secs(120),
            )
            .unwrap();
        std::fs::read(output).unwrap()
    };
    let themed = reference(LookChoice::PdfTheme("sans".to_owned()), "themed.docx");
    let templated = reference(
        LookChoice::Template("corporate".to_owned()),
        "templated.docx",
    );
    let plain = reference(LookChoice::None, "plain.docx");
    assert!(themed != plain && templated != plain && themed != templated);

    for (arguments, expected, look) in [
        (vec!["--theme", "sans"], &themed, "look: pdf-theme sans"),
        (
            vec!["--template", "corporate"],
            &templated,
            "look: template corporate",
        ),
        (vec!["--no-theme"], &plain, "look: none"),
        (vec!["--profile", "themed"], &themed, "look: pdf-theme sans"),
        (
            vec!["--profile", "templated"],
            &templated,
            "look: template corporate",
        ),
        (vec!["--profile", "plain"], &plain, "look: none"),
        // An option given with a profile wins over the profile's look.
        (
            vec!["--profile", "themed", "--no-theme"],
            &plain,
            "look: none",
        ),
    ] {
        let written = directory.path().join("cli.docx");
        let written_name = written.to_string_lossy().into_owned();
        let mut all = vec!["docx"];
        if !arguments.contains(&"--profile") {
            all.push(page);
        }
        all.extend(arguments.iter().copied());
        all.extend(["--output", written_name.as_str()]);
        let output = run(&all);
        assert!(
            stdout(&output).contains(look),
            "{all:?}: {}",
            stdout(&output)
        );
        assert!(
            std::fs::read(&written).unwrap() == *expected,
            "{all:?} wrote other bytes than the engine"
        );
    }

    // At most one look; PDF export knows only `--theme`.
    let refused = |arguments: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(&root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{arguments:?} was accepted");
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    assert!(refused(&["docx", page, "--theme", "sans", "--no-theme"]).contains("at most one"));
    assert!(
        refused(&["docx", page, "--theme", "sans", "--template", "corporate"])
            .contains("at most one")
    );
    assert!(refused(&["pdf", page, "--template", "corporate"]).contains("unknown option"));
    assert!(refused(&["pdf", page, "--no-theme"]).contains("unknown option"));
}

/// Read-only exports, READONLY 5/5 (`docs/prd/PRD-AdocFoundry_READONLY.md` §10, test 22): the
/// command line's exit status, message and preserved output for each case of §5.2.
#[test]
fn protection_cli_exit_codes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("ws");
    let pages = root.join("docs/modules/ROOT/pages");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        root.join("docs/antora.yml"),
        "name: docs\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    std::fs::write(
        root.join("docs/modules/ROOT/nav.adoc"),
        "* xref:a.adoc[]\n* xref:b.adoc[]\n",
    )
    .unwrap();
    std::fs::write(
        pages.join("a.adoc"),
        "= A\n:export-read-only: true\n\nText.\n",
    )
    .unwrap();
    std::fs::write(pages.join("b.adoc"), "= B\n\nText.\n").unwrap();
    std::fs::write(
        pages.join("bad.adoc"),
        "= Bad\n:export-read-only: maybe\n\nText.\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[docx.profiles.manual]\ncomponent = \"1.0@docs\"\noutput = \"manual.docx\"\n",
    )
    .unwrap();
    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(&root)
            .args(arguments)
            .output()
            .unwrap()
    };
    let a = "docs/modules/ROOT/pages/a.adoc";
    let b = "docs/modules/ROOT/pages/b.adoc";

    // Nothing asked: as before.
    let output = run(&["docx", b, "--output", "b.docx"]);
    assert!(output.status.success());
    assert!(!stdout(&output).contains("protected"));
    // A protected page to Word: read-only, and said.
    let output = run(&["docx", a, "--output", "a.docx"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout(&output).contains("protected: read-only"),
        "{}",
        stdout(&output)
    );

    // Every refusal fails, with the page and line, and leaves what was there.
    let refused = |arguments: &[&str], output: &str, says: &str| {
        let file = root.join(output);
        std::fs::write(&file, "kept").unwrap();
        let mut all = arguments.to_vec();
        all.extend(["--output", output]);
        let result = run(&all);
        assert_eq!(result.status.code(), Some(1), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        assert!(stderr.contains(says), "{arguments:?}: {stderr}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "kept",
            "{arguments:?}"
        );
    };
    refused(&["pdf", a], "a.pdf", "a.adoc:2");
    refused(
        &["pdf", a, "--standard", "pdf-a-2a"],
        "archive.pdf",
        "PDF/A-2a",
    );
    refused(&["docx", "docs"], "docs.docx", "export them separately");
    refused(&["pdf", "docs"], "docs.pdf", "export them separately");
    refused(
        &["docx", "docs/modules/ROOT/pages/bad.adoc"],
        "bad.docx",
        "bad.adoc:2",
    );
    refused(
        &["docx", "--profile", "manual"],
        "manual.docx",
        "export them separately",
    );
    // `--fail-on` keeps its meaning: a refusal fails whatever it says.
    refused(
        &["docx", "docs", "--fail-on", "none"],
        "docs.docx",
        "export them separately",
    );
}

/// The DocBook export PRD, §11 test 19: an article, a component book and a profile, each with
/// its pictures beside it, with exit codes that follow `--fail-on`; `--assets reference` copies
/// nothing.
#[test]
fn docbook_cli_exports_and_gates() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("reference");
    copy_tree(
        &repository_root().join("tests/fixtures/docx/reference"),
        &root,
    );
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[docbook.profiles.handbook]\ncomponent = \"2.0@manual\"\noutput = \"build/handbook.xml\"\n",
    )
    .unwrap();
    let run = |folder: &Path, arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .current_dir(folder)
            .args(arguments)
            .output()
            .unwrap()
    };
    let succeeded = |output: &Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let is_docbook = |path: &Path| {
        let xml = std::fs::read_to_string(path).unwrap();
        adoc_docbook::testing::assert_valid(&xml);
        xml
    };
    let pages = root.join("manual/modules/ROOT/pages");

    // A page: beside it, with its pictures in the folder named after it.
    let output = run(&root, &["docbook", "manual/modules/ROOT/pages/media.adoc"]);
    succeeded(&output);
    let xml = is_docbook(&pages.join("media.xml"));
    assert!(xml.contains("fileref=\"media_assets/i1.png\""), "{xml}");
    assert!(pages.join("media_assets/i1.png").is_file());
    let shown = stdout(&output);
    assert!(shown.contains("media.xml (1 AsciiDoc page)"), "{shown}");
    assert!(shown.contains("assets: "), "{shown}");
    // An output that is not an `.xml` file, or an unknown asset policy, is refused, and
    // nothing is written.
    for (flag, value, problem) in [
        ("--output", "x.dbk", "is not an .xml file"),
        ("--output", "x.epub", "is not an .xml file"),
        ("--assets", "move", "expected copy or reference"),
    ] {
        let output = run(
            &root,
            &[
                "docbook",
                "manual/modules/ROOT/pages/media.adoc",
                flag,
                value,
            ],
        );
        assert_eq!(output.status.code(), Some(2), "{flag} {value}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(problem),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!root.join("x.dbk").exists());

    // Referred to where they are: nothing copied.
    succeeded(&run(
        &root,
        &[
            "docbook",
            "manual/modules/ROOT/pages/media.adoc",
            "--output",
            "referenced.xml",
            "--assets",
            "reference",
        ],
    ));
    let xml = std::fs::read_to_string(root.join("referenced.xml")).unwrap();
    adoc_docbook::testing::assert_valid_referenced(&xml);
    assert!(
        xml.contains("fileref=\"manual/modules/ROOT/images/logo.png\""),
        "{xml}"
    );
    assert!(!root.join("referenced_assets").exists());

    // A component folder: its book, in the current folder; and by `--component`.
    let output = run(
        directory.path(),
        &[
            "docbook",
            "reference",
            "--component",
            "2.0@manual",
            "--output",
            "manual.xml",
        ],
    );
    succeeded(&output);
    let xml = is_docbook(&directory.path().join("manual.xml"));
    assert!(xml.contains("<book "), "{xml}");
    assert!(directory.path().join("manual_assets").is_dir());

    // A profile writes what the app's command writes.
    succeeded(&run(
        directory.path(),
        &["docbook", "--profile", "handbook", "reference"],
    ));
    let written = root.join("build/handbook.xml");
    let from_cli = std::fs::read(&written).unwrap();
    is_docbook(&written);
    std::fs::remove_file(&written).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    let wait = std::time::Duration::from_secs(120);
    assert!(engine.wait_for_workers(wait));
    engine
        .invoke(
            adoc_engine::commands::ids::EXPORT_DOCBOOK_PROFILE,
            adoc_engine::CommandArgs::Profile("handbook".to_owned()),
        )
        .unwrap();
    while engine.is_exporting() {
        assert!(engine.wait_for_workers(wait));
    }
    assert!(
        std::fs::read(&written).unwrap() == from_cli,
        "the app's document differs from the command line's"
    );
    let output = run(
        directory.path(),
        &["docbook", "--profile", "missing", "reference"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no DocBook profile `missing`"));

    // `--fail-on`: a reference that leads nowhere is a warning; the document is written all
    // the same.
    std::fs::write(
        pages.join("broken.adoc"),
        "= Broken\n\nSee xref:missing.adoc[nowhere].\n",
    )
    .unwrap();
    let page = [
        "docbook",
        "manual/modules/ROOT/pages/broken.adoc",
        "--output",
        "broken.xml",
    ];
    let output = run(&root, &page);
    succeeded(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("broken.adoc:3: warning: the cross reference missing.adoc leads to nothing"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(root.join("broken.xml")).unwrap();
    let output = run(&root, &[&page[..], &["--fail-on", "warning"]].concat());
    assert_eq!(output.status.code(), Some(1));
    is_docbook(&root.join("broken.xml"));
    succeeded(&run(&root, &[&page[..], &["--fail-on", "error"]].concat()));
    succeeded(&run(&root, &[&page[..], &["--fail-on", "none"]].concat()));
}

/// Runs the binary from `directory` with `environment` added.
fn cli_env(directory: &Path, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(directory)
        .args(arguments)
        .env_remove("ADOCFOUNDRY_TIMEOUT")
        .env_remove("ADOCFOUNDRY_CACHE_DIR")
        .envs(environment.iter().copied())
        .output()
        .unwrap()
}

/// Every file and folder beneath `root`, relative to it, sorted.
fn tree(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, folder: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
            if path.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// `--timeout` bounds the whole command, and `ADOCFOUNDRY_TIMEOUT` stands in for it (#809): a
/// command that runs out of time says which step did and exits with 2.
#[test]
fn a_timeout_names_the_step_and_exits_with_2() {
    let root = repository_root();
    let fixture = "tests/fixtures/antora-broken-references";
    for (arguments, environment) in [
        (&["check", fixture, "--timeout", "0.000001"][..], &[][..]),
        (&["--timeout", "0.000001", "check", fixture][..], &[][..]),
        (
            &["check", fixture][..],
            &[("ADOCFOUNDRY_TIMEOUT", "0.000001")][..],
        ),
    ] {
        let output = cli_env(&root, arguments, environment);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {stderr}");
        assert!(
            stderr.contains("indexing did not finish within the timeout"),
            "{arguments:?}: {stderr}"
        );
    }

    // The option wins over the environment, and an ample one passes.
    let output = cli_env(
        &root,
        &["check", fixture, "--timeout", "120"],
        &[("ADOCFOUNDRY_TIMEOUT", "0.000001")],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    for (arguments, environment, complaint) in [
        (
            &["check", fixture, "--timeout", "0"][..],
            &[][..],
            "--timeout",
        ),
        (
            &["check", fixture, "--timeout", "soon"][..],
            &[][..],
            "--timeout",
        ),
        (
            &["check", fixture, "--timeout", "-5"][..],
            &[][..],
            "--timeout",
        ),
        (&["check", fixture, "--timeout"][..], &[][..], "--timeout"),
        (
            &["check", fixture][..],
            &[("ADOCFOUNDRY_TIMEOUT", "soon")][..],
            "ADOCFOUNDRY_TIMEOUT",
        ),
    ] {
        let output = cli_env(&root, arguments, environment);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {stderr}");
        assert!(stderr.contains(complaint), "{arguments:?}: {stderr}");
    }
}

/// An export that runs out of time exits with 2, says so, and leaves the file it would have
/// replaced as it was, with nothing of its own beside it (#809).
#[test]
fn a_timed_out_export_leaves_the_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("long");
    let pages = root.join("modules/ROOT/pages");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        root.join("antora.yml"),
        "name: long\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    let mut nav = String::new();
    for page in 0..60 {
        let sections: String = (0..20)
            .map(|section| {
                format!("== Section {section}\n\nText of {page}.{section}.footnote:[A note.]\n\n|===\n|a |b\n|===\n\n")
            })
            .collect();
        std::fs::write(
            pages.join(format!("p{page}.adoc")),
            format!("= Page {page}\n\n{sections}"),
        )
        .unwrap();
        nav.push_str(&format!("* xref:p{page}.adoc[]\n"));
    }
    std::fs::write(root.join("modules/ROOT/nav.adoc"), nav).unwrap();
    let out = directory.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let output = out.join("long.docx");

    let mut timeout = 0.002_f64;
    let mut steps = Vec::new();
    loop {
        std::fs::write(&output, "the earlier document").unwrap();
        let seconds = timeout.to_string();
        let run = cli_env(
            directory.path(),
            &[
                "docx",
                "long",
                "--output",
                output.to_str().unwrap(),
                "--timeout",
                &seconds,
            ],
            &[],
        );
        let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
        if run.status.code() == Some(0) {
            break;
        }
        assert_eq!(run.status.code(), Some(2), "at {seconds} s: {stderr}");
        let step = ["indexing", "the export"]
            .into_iter()
            .find(|step| stderr.contains(&format!("{step} did not finish within the timeout")))
            .unwrap_or_else(|| panic!("at {seconds} s, no step named: {stderr}"));
        steps.push(step);
        assert_eq!(
            std::fs::read_to_string(&output).unwrap_or_default(),
            "the earlier document",
            "replaced after timing out at {seconds} s"
        );
        assert_eq!(tree(&out), [PathBuf::from("long.docx")], "at {seconds} s");
        assert!(timeout < 120.0, "never finished");
        timeout *= 1.25;
    }
    assert!(steps.contains(&"the export"), "{steps:?}");
    assert!(std::fs::read(&output).unwrap().starts_with(b"PK\x03\x04"));
}

/// With `--no-cache` nothing is kept: diagrams are drawn into a temporary folder that goes when
/// the command ends, beneath neither the workspace nor anywhere else (#810).
#[test]
fn no_cache_keeps_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("page.adoc"),
        "= Page\n\n[mermaid]\n----\ngraph LR\n  X --> Y\n----\n",
    )
    .unwrap();
    let before = tree(&root);
    let build = directory.path().join("build");
    let temporary = directory.path().join("tmp");
    std::fs::create_dir(&build).unwrap();
    std::fs::create_dir(&temporary).unwrap();
    let temporary_text = temporary.to_str().unwrap();
    let pdf = build.join("page.pdf");
    let environment = [
        ("TMPDIR", temporary_text),
        ("TMP", temporary_text),
        ("TEMP", temporary_text),
        // The option wins over the variable.
        ("ADOCFOUNDRY_CACHE_DIR", "build/cache"),
    ];

    let output = cli_env(
        directory.path(),
        &[
            "pdf",
            "docs/page.adoc",
            "--output",
            pdf.to_str().unwrap(),
            "--no-cache",
        ],
        &environment,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(pdf.is_file());
    assert_eq!(tree(&root), before, "written into the workspace");
    assert_eq!(tree(&build), [PathBuf::from("page.pdf")]);
    assert_eq!(tree(&temporary), Vec::<PathBuf>::new(), "left behind");

    let output = cli_env(
        directory.path(),
        &["check", "docs", "--no-cache", "--cache-dir", "build/cache"],
        &[],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--no-cache"), "{stderr}");
}

/// With `--cache-dir`, or `ADOCFOUNDRY_CACHE_DIR`, diagrams are kept in `<dir>/diagrams` and
/// nothing is written beneath the workspace, so a build tool's inputs stay as they were (#810).
#[test]
fn a_cache_dir_keeps_diagrams_out_of_the_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("page.adoc"),
        "= Page\n\n[mermaid]\n----\ngraph LR\n  X --> Y\n----\n",
    )
    .unwrap();
    let before = tree(&root);
    let build = directory.path().join("build");
    let pdf = build.join("page.pdf");
    std::fs::create_dir(&build).unwrap();

    let output = cli_env(
        directory.path(),
        &[
            "pdf",
            "docs/page.adoc",
            "--output",
            pdf.to_str().unwrap(),
            "--cache-dir",
            "build/cache",
        ],
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(tree(&root), before, "written into the workspace");
    let cached: Vec<PathBuf> = std::fs::read_dir(build.join("cache/diagrams"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(cached.len(), 1, "{cached:?}");

    // A second export with the same cache reads the image kept there rather than drawing it.
    let marked = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>";
    std::fs::write(&cached[0], marked).unwrap();
    let output = cli_env(
        directory.path(),
        &["pdf", "docs/page.adoc", "--output", pdf.to_str().unwrap()],
        &[("ADOCFOUNDRY_CACHE_DIR", "build/cache")],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&cached[0]).unwrap(), marked);
    assert_eq!(tree(&root), before, "written into the workspace");

    // `doctor` reports the cache where it is kept.
    let doctor = cli_env(
        directory.path(),
        &["doctor", "docs", "--cache-dir", "build/cache"],
        &[],
    );
    let report = stdout(&doctor);
    let kept = Path::new("build").join("cache").join("diagrams");
    assert!(
        report.lines().any(|line| line.contains("Diagram cache:")
            && line.contains(&format!("{} (", kept.display()))),
        "{report}"
    );

    // Without either, the cache is in the workspace, as before.
    let output = cli_env(
        directory.path(),
        &["pdf", "docs/page.adoc", "--output", pdf.to_str().unwrap()],
        &[],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(root.join(".cache/adocfoundry/diagrams").is_dir());
}

/// A component in `docs` of a temporary folder, its page holding `header`, and the files in
/// `extra` written beside it, relative to `docs`.
fn themed_workspace(header: &str, antora: &str, extra: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let docs = directory.path().join("docs");
    let pages = docs.join("modules/ROOT/pages");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        docs.join("antora.yml"),
        format!("name: guide\nversion: ~\n{antora}"),
    )
    .unwrap();
    std::fs::write(
        pages.join("index.adoc"),
        format!("= Guide\n{header}\nSome text.\n"),
    )
    .unwrap();
    for (path, text) in extra {
        let path = docs.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    directory
}

/// Exports the page of a [`themed_workspace`] to `out.pdf` beside `docs`, with `arguments`
/// and `environment` added and no user settings file unless `environment` names one.
fn export_themed(directory: &Path, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"));
    command
        .current_dir(directory)
        .args([
            "pdf",
            "docs/modules/ROOT/pages/index.adoc",
            "--root",
            "docs",
            "--output",
            "out.pdf",
        ])
        .args(arguments)
        .env(
            "ADOCFOUNDRY_USER_SETTINGS",
            directory.join("no-user-settings.toml"),
        )
        .envs(environment.iter().copied());
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// `pdf` names the theme it used and where the choice came from, and the fonts folders the
/// same way (#812).
#[test]
fn pdf_names_its_theme_and_where_it_came_from() {
    let theme = "extends: default\nbase:\n  font_color: 333333\n";
    let line = |output: &Output, start: &str| {
        stdout(output)
            .lines()
            .find(|line| line.starts_with(start))
            .unwrap_or_else(|| panic!("no `{start}` line: {}", stdout(output)))
            .to_owned()
    };

    let plain = themed_workspace("", "", &[]);
    let output = export_themed(plain.path(), &[], &[]);
    assert_eq!(line(&output, "theme:"), "theme: default (bundled)");
    assert_eq!(line(&output, "fonts:"), "fonts: bundled");

    let output = export_themed(plain.path(), &["--theme", "default-sans"], &[]);
    assert_eq!(
        line(&output, "theme:"),
        "theme: default-sans (bundled, from --theme)"
    );

    let page = themed_workspace(":pdf-theme: default-for-print\n", "", &[]);
    let output = export_themed(page.path(), &[], &[]);
    assert_eq!(
        line(&output, "theme:"),
        "theme: default-for-print (bundled, from pdf-theme in modules/ROOT/pages/index.adoc)"
    );

    let component = themed_workspace("", "asciidoc:\n  attributes:\n    pdf-theme: base\n", &[]);
    let output = export_themed(component.path(), &[], &[]);
    assert_eq!(
        line(&output, "theme:"),
        "theme: base (bundled, from pdf-theme in antora.yml)"
    );

    let settings = themed_workspace(
        "",
        "",
        &[
            (
                ".adocfoundry.toml",
                "[pdf]\ntheme = \"acme\"\nthemes_dir = \"themes\"\nfonts_dir = \"fonts\"\n",
            ),
            ("themes/acme-theme.yml", theme),
            ("fonts/.keep", ""),
        ],
    );
    let output = export_themed(settings.path(), &[], &[]);
    assert_eq!(
        line(&output, "theme:"),
        "theme: acme (themes/acme-theme.yml, from [pdf] theme in .adocfoundry.toml)"
    );
    assert_eq!(
        line(&output, "fonts:"),
        "fonts: fonts (from [pdf] fonts_dir in .adocfoundry.toml)"
    );

    // The export's own wins over the settings'.
    let output = export_themed(settings.path(), &["--fonts-dir", "docs/fonts"], &[]);
    assert_eq!(
        line(&output, "fonts:"),
        "fonts: docs/fonts (from --fonts-dir)"
    );
}

/// A `[pdf]` key or profile the user settings set and the workspace's do not is used by the
/// app, never by the command line, which says so in an information note that fails nothing;
/// the PDF is the one written without the file (#812).
#[test]
fn pdf_notes_user_settings_the_app_would_use() {
    let workspace = themed_workspace(
        "",
        "",
        &[
            (".adocfoundry.toml", "[pdf]\nfonts_dir = \"fonts\"\n"),
            ("fonts/.keep", ""),
        ],
    );
    let directory = workspace.path();
    let without = export_themed(directory, &["--fail-on", "warning"], &[]);
    assert!(
        !String::from_utf8_lossy(&without.stderr).contains("info:"),
        "{}",
        String::from_utf8_lossy(&without.stderr)
    );
    let written = std::fs::read(directory.join("out.pdf")).unwrap();

    let user = directory.join("user").join("adocfoundry.toml");
    std::fs::create_dir_all(user.parent().unwrap()).unwrap();
    std::fs::write(
        &user,
        "[pdf]\ntheme = \"default-sans\"\nfonts_dir = \"elsewhere\"\nstandards = [\"pdf-a-2a\"]\n\n[pdf.profiles.handbook]\ncomponent = \"guide\"\n",
    )
    .unwrap();
    let with = export_themed(
        directory,
        &["--fail-on", "warning"],
        &[("ADOCFOUNDRY_USER_SETTINGS", user.to_str().unwrap())],
    );
    let stderr = String::from_utf8_lossy(&with.stderr);
    let notes: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("info: "))
        .collect();
    assert_eq!(notes.len(), 3, "{stderr}");
    assert!(
        notes[0].contains("[pdf] theme = \"default-sans\"")
            && notes[0].contains("used by the app but not by the command line")
            && notes[0].contains(".adocfoundry.toml"),
        "{stderr}"
    );
    assert!(notes[1].contains("[pdf] standards"), "{stderr}");
    assert!(notes[2].contains("[pdf.profiles.handbook]"), "{stderr}");
    assert!(
        !stderr.contains("fonts_dir"),
        "the workspace sets it: {stderr}"
    );
    assert_eq!(line_of(&with, "theme:"), "theme: default (bundled)");
    assert_eq!(std::fs::read(directory.join("out.pdf")).unwrap(), written);
}

fn line_of(output: &Output, start: &str) -> String {
    stdout(output)
        .lines()
        .find(|line| line.starts_with(start))
        .unwrap_or_default()
        .to_owned()
}

/// An export's `--format json` object (#808), with what differs between machines and releases
/// put in brackets once checked: the CLI's version, and every path beneath `places`, each
/// `(path, name)`, as `[name]`, its separators `/`. Standard output holds the object alone.
fn export_json_of(output: &Output, places: &[(&Path, &str)]) -> String {
    fn redact(value: &mut serde_json::Value, places: &[(String, &str)]) {
        match value {
            serde_json::Value::String(text) => {
                // Anywhere in the text, as a message names files too.
                if let Some((place, name)) = places
                    .iter()
                    .find(|(place, _)| text.contains(place.as_str()))
                {
                    *text = text
                        .replace(place.as_str(), &format!("[{name}]"))
                        .replace('\\', "/");
                }
            }
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(|item| redact(item, places));
            }
            serde_json::Value::Object(fields) => {
                fields.values_mut().for_each(|item| redact(item, places));
            }
            _ => {}
        }
    }
    let mut json: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                stdout(output),
                String::from_utf8_lossy(&output.stderr)
            )
        });
    assert_eq!(json["tool"]["name"], "adocfoundry-cli");
    assert_eq!(json["tool"]["version"], env!("CARGO_PKG_VERSION"));
    json["tool"]["version"] = "[version]".into();
    assert!(Path::new(json["root"].as_str().unwrap()).is_absolute());
    for written in json["outputs"].as_array().unwrap() {
        let path = Path::new(written["path"].as_str().unwrap());
        assert!(path.is_absolute(), "{path:?}");
        match written["kind"].as_str().unwrap() {
            "file" => assert!(path.is_file(), "{path:?}"),
            "folder" => assert!(path.is_dir(), "{path:?}"),
            other => panic!("kind {other}"),
        }
    }
    // Each place as written and as the system resolves it (macOS's temporary folder is a link
    // to `/private/var`), the longest first, so a place inside another is found first.
    let mut places: Vec<(String, &str)> = places
        .iter()
        .flat_map(|(path, name)| {
            [
                std::path::absolute(path).unwrap(),
                std::fs::canonicalize(path).unwrap(),
            ]
            .map(|path| (path.to_str().unwrap().to_owned(), *name))
        })
        .collect();
    places.sort_by_key(|(place, _)| std::cmp::Reverse(place.len()));
    redact(&mut json, &places);
    serde_json::to_string_pretty(&json).unwrap()
}

/// `pdf --format json`: one object naming the file written, its pages and sources, the look
/// and what chose it, and the outcome; the text lines are not printed (#808).
#[test]
fn export_json_pdf_article() {
    let directory = tempfile::tempdir().unwrap();
    let pdf = directory.path().join("article.pdf");
    let output = cli(&[
        "pdf",
        "tests/fixtures/pdf/reference/article.adoc",
        "--output",
        pdf.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = repository_root().join("tests/fixtures/pdf/reference");
    insta::assert_snapshot!(export_json_of(
        &output,
        &[(&root, "root"), (directory.path(), "tmp")]
    ));
}

/// A component version's book: its sources in the navigation's order, and a diagnostic with
/// its page and line.
#[test]
fn export_json_pdf_component_book() {
    let directory = tempfile::tempdir().unwrap();
    let book = directory.path().join("book");
    copy_tree(&repository_root().join("tests/fixtures/pdf/book"), &book);
    let output = cli_env(
        directory.path(),
        &["pdf", "book/guide", "--format", "json"],
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    insta::assert_snapshot!(export_json_of(
        &output,
        &[(&book, "root"), (directory.path(), "tmp")]
    ));
}

/// A read-only page to PDF is refused: the object says so and why, names nothing written,
/// and the command exits with 1 as it does in text.
#[test]
fn export_json_refusal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("ws");
    let pages = root.join("docs/modules/ROOT/pages");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(root.join("docs/antora.yml"), "name: docs\nversion: '1.0'\n").unwrap();
    std::fs::write(
        pages.join("a.adoc"),
        "= A\n:export-read-only: true\n\nText.\n",
    )
    .unwrap();
    let output = cli_env(
        &root,
        &[
            "pdf",
            "docs/modules/ROOT/pages/a.adoc",
            "--output",
            "a.pdf",
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(!root.join("a.pdf").exists());
    insta::assert_snapshot!(export_json_of(
        &output,
        &[(&root, "root"), (directory.path(), "tmp")]
    ));
}

/// DocBook names the document and the folder of pictures beside it as outputs.
#[test]
fn export_json_docbook_with_assets() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("reference");
    copy_tree(
        &repository_root().join("tests/fixtures/docx/reference"),
        &root,
    );
    let output = cli_env(
        &root,
        &[
            "docbook",
            "manual/modules/ROOT/pages/media.adoc",
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    insta::assert_snapshot!(export_json_of(
        &output,
        &[(&root, "root"), (directory.path(), "tmp")]
    ));
}

/// Word, EPUB and Kindle print the object too; Kindle's `--format` takes the book's format and
/// the report's side by side, in either order, and `--fail-on` fails a written export.
#[test]
fn export_json_every_format() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("reference");
    copy_tree(
        &repository_root().join("tests/fixtures/docx/reference"),
        &root,
    );
    let page = "manual/modules/ROOT/pages/media.adoc";
    let json = |arguments: &[&str]| {
        let output = cli_env(&root, arguments, &[]);
        let json: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{arguments:?}: {error}: {}\n{}",
                    stdout(&output),
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        (output.status.code(), json)
    };
    for (arguments, format, extension) in [
        (vec!["docx", page, "--format", "json"], "docx", "docx"),
        (vec!["epub", page, "--format", "json"], "epub", "epub"),
        (
            vec!["kindle", page, "--format", "json", "--format", "kfx"],
            "kfx",
            "kfx",
        ),
        (
            vec!["kindle", page, "--format", "azw3", "--format", "json"],
            "azw3",
            "azw3",
        ),
    ] {
        let (code, json) = json(&arguments);
        assert_eq!(code, Some(0), "{arguments:?}: {json}");
        assert_eq!(json["format"], format, "{json}");
        assert_eq!(json["outcome"], "written", "{json}");
        let written = json["outputs"][0]["path"].as_str().unwrap();
        assert!(
            written.ends_with(&format!("media.{extension}")),
            "{written}"
        );
        assert_eq!(json["sources"][0], page, "{json}");
    }
    // Word names its look.
    let (_, word) = json(&["docx", page, "--format", "json", "--no-theme"]);
    assert_eq!(word["look"]["kind"], "none", "{word}");
    // Two report formats, or two book formats, are refused.
    for arguments in [
        vec!["kindle", page, "--format", "json", "--format", "text"],
        vec!["kindle", page, "--format", "kfx", "--format", "azw3"],
        vec!["pdf", page, "--format", "yaml"],
    ] {
        let output = cli_env(&root, &arguments, &[]);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
    }
    // `--fail-on`: written, and failed.
    std::fs::write(
        root.join("manual/modules/ROOT/pages/broken.adoc"),
        "= Broken\n\nSee xref:missing.adoc[nowhere].\n",
    )
    .unwrap();
    let (code, broken) = json(&[
        "epub",
        "manual/modules/ROOT/pages/broken.adoc",
        "--format",
        "json",
        "--fail-on",
        "warning",
    ]);
    assert_eq!(code, Some(1), "{broken}");
    assert_eq!(broken["outcome"], "written", "{broken}");
    let warning = &broken["diagnostics"][0];
    assert_eq!(warning["severity"], "warning", "{broken}");
    assert_eq!(
        warning["path"], "manual/modules/ROOT/pages/broken.adoc",
        "{broken}"
    );
    assert_eq!(warning["line"], 3, "{broken}");
    // `--format text` is the default.
    let text = cli_env(&root, &["epub", page, "--format", "text"], &[]);
    let default = cli_env(&root, &["epub", page], &[]);
    assert_eq!(text.status.code(), Some(0));
    assert_eq!(stdout(&text), stdout(&default));
}
