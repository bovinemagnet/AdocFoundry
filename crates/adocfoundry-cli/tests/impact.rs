//! `adocfoundry-cli impact` (Impact PRD §§7, 9 and 11; #310): reproducible JSON against fixture
//! refs, a text summary that tells rendered content from references and navigation, exit codes,
//! and the explicit-path mode without Git.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use adoc_git::testing::TestRepo;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cli(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .args(arguments)
        .output()
        .expect("run adocfoundry-cli")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

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

const NESTED: &str = "v1/modules/ROOT/partials/nested.adoc";
const INSTALL: &str = "v1/modules/ROOT/pages/install.adoc";

/// A repository holding `tests/fixtures/impact`: `base` its first commit; the second changes the
/// nested partial and removes `install`'s anchor.
fn repository() -> (tempfile::TempDir, PathBuf, String, String) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    fs::create_dir_all(&root).unwrap();
    let repo = TestRepo::init(&root);
    copy_tree(&repository_root().join("tests/fixtures/impact"), &root);
    let base = repo.commit_all("base");
    repo.write(NESTED, "A nested note, changed.\n");
    repo.write(
        INSTALL,
        "= Installing\n\ninclude::partial$intro.adoc[]\n\n== Getting ready\n\nSteps.\n",
    );
    let head = repo.commit_all("head");
    (directory, root, base, head)
}

#[test]
fn impact_json_is_reproducible_against_fixture_refs() {
    let (_directory, root, base, head) = repository();
    let root_text = root.to_string_lossy().into_owned();
    let run = || {
        cli(&[
            "impact", &root_text, "--base", &base, "--head", &head, "--format", "json",
        ])
    };
    let (first, second) = (run(), run());
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(stdout(&first), stdout(&second), "the same every time");
    let text = stdout(&first);
    assert!(!text.contains(&root_text), "no absolute path");
    // Commit ids change with every run of the test; the snapshot holds placeholders.
    let report = text.replace(&base, "[base]").replace(&head, "[head]");
    insta::assert_snapshot!("impact_json", report);
}

#[test]
fn impact_text_tells_rendered_content_from_references() {
    let (_directory, root, base, head) = repository();
    let root_text = root.to_string_lossy().into_owned();
    let output = cli(&["impact", &root_text, "--base", &base, "--head", &head]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.starts_with("Impact of"), "{text}");
    assert!(
        text.lines().next().unwrap().ends_with(": complete"),
        "{text}"
    );
    let rendered = text.find("Rendered content that may change (").unwrap();
    let references = text.find("References to review (").unwrap();
    assert!(rendered < references);
    assert!(
        text.contains("1.0@product:ROOT:page$index.adoc [broken] #setup"),
        "{text}"
    );
    assert!(
        text.contains(
            "via v1/modules/ROOT/partials/nested.adoc → v1/modules/ROOT/partials/intro.adoc"
        ),
        "{text}"
    );
    // A page's prose change reaches no page that merely links to it.
    assert!(!text.contains("guide.adoc"), "{text}");
}

#[test]
fn impact_cli_handles_no_git() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    copy_tree(&repository_root().join("tests/fixtures/impact"), &root);
    let root_text = root.to_string_lossy().into_owned();
    // Absolute, as the current directory is elsewhere.
    let nested = root.join(NESTED).to_string_lossy().into_owned();
    let install = root.join(INSTALL).to_string_lossy().into_owned();

    // Explicit paths need no repository.
    let output = cli(&[
        "impact",
        &root_text,
        "--changed-file",
        &nested,
        "--format",
        "json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["source"]["kind"], "paths");
    assert_eq!(report["counts"]["render"], 2);

    // A removed path with no base is partial: exit 0, or 2 under --require-complete.
    fs::remove_file(root.join(INSTALL)).unwrap();
    let partial = cli(&["impact", &root_text, "--changed-file", &install]);
    assert_eq!(partial.status.code(), Some(0));
    assert!(stdout(&partial)
        .lines()
        .next()
        .unwrap()
        .ends_with(": partial"));
    let strict = cli(&[
        "impact",
        &root_text,
        "--changed-file",
        &install,
        "--require-complete",
    ]);
    assert_eq!(strict.status.code(), Some(2));

    // A ref comparison outside any repository is refused, with only the message on stderr.
    let refused = cli(&["impact", &root_text, "--base", "main", "--head", "HEAD"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(stdout(&refused).is_empty());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("no Git repository"));
    let ambiguous = cli(&[
        "impact",
        &root_text,
        "--base",
        "main",
        "--changed-file",
        &nested,
    ]);
    assert_eq!(ambiguous.status.code(), Some(2));
}

/// `--changed-file` is read against the current directory, as every other path argument is and
/// as `git diff --name-only` prints paths from the top of the repository (#510).
#[test]
fn impact_changed_files_are_relative_to_the_current_directory() {
    let directory = tempfile::tempdir().unwrap();
    copy_tree(
        &repository_root().join("tests/fixtures/impact"),
        &directory.path().join("docs"),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .current_dir(directory.path())
        .args([
            "impact",
            "docs",
            "--changed-file",
            &format!("docs/{NESTED}"),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["changes"][0]["path"], NESTED, "{report}");
    assert_eq!(report["changes"][0]["kind"], "modified", "{report}");
    assert_eq!(report["counts"]["render"], 2);
}
