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
    let output = cli(&["check", &path, "--format", "json"]);
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Messages show paths as the platform writes them; JSON escapes a Windows `\` as `\\`.
    stdout(&output).replace("\\\\", "/")
}

#[test]
fn check_antora_single_component() {
    insta::assert_snapshot!(check_json("antora-single-component"));
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
        stdout(&output),
        check_json("antora-broken-references"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
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
    assert!(
        entries
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
