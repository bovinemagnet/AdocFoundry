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
            index.contains("<a href=\"security:authentication.html\">Authentication</a>"),
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
