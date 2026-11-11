//! Reading a repository as Git reads it: diffs and blame through the line ending conversions,
//! and history in a shallow clone. Repositories are built with `gix`; where `git` is installed
//! the tests also check that it agrees.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use adoc_git::{testing::TestRepo, HunkKind, Repository};

fn repo_in(dir: &Path) -> Repository {
    Repository::discover(dir)
        .expect("readable")
        .expect("a repository")
}

/// Runs `git` in `dir` reading no configuration but the repository's own; `None` without `git`.
fn git(dir: &Path, args: &[&str]) -> Option<Output> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .ok();
    if output.is_none() {
        eprintln!("git not found: not compared with it");
    }
    output
}

fn append_config(dir: &Path, text: &str) {
    let config = dir.join(".git/config");
    let mut existing = fs::read_to_string(&config).unwrap();
    existing.push_str(text);
    fs::write(config, existing).unwrap();
}

const LF: &str = "= Page\n\nFirst line.\n";
const CRLF: &str = "= Page\r\n\r\nFirst line.\r\n";

/// A page committed with `\n` and checked out with `\r\n`, as Git does under `setup`'s
/// configuration.
fn crlf_checkout(dir: &Path, setup: impl Fn(&TestRepo)) -> TestRepo {
    let test = TestRepo::init(dir);
    setup(&test);
    test.write("page.adoc", LF);
    test.commit_all("first");
    test.write("page.adoc", CRLF);
    test
}

fn assert_unchanged_like_git(dir: &Path) {
    let repo = repo_in(dir);
    let page = dir.join("page.adoc");
    assert_eq!(repo.line_diff(&page, None).unwrap(), Vec::new());
    assert_eq!(repo.line_diff(&page, Some(CRLF)).unwrap(), Vec::new());
    let blame = repo.blame(&page, None).unwrap();
    assert_eq!(blame.len(), 3);
    assert!(
        blame.iter().all(|line| line.commit.is_some()),
        "every line is committed: {blame:?}"
    );
    assert_eq!(
        repo.head_text_for_worktree(&page).unwrap().as_deref(),
        Some(CRLF)
    );
    if let Some(output) = git(dir, &["diff", "HEAD", "--stat"]) {
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    }
}

#[test]
fn a_crlf_checkout_under_autocrlf_has_no_changes() {
    let dir = tempfile::tempdir().unwrap();
    crlf_checkout(dir.path(), |_| {
        append_config(dir.path(), "[core]\n\tautocrlf = true\n");
    });
    assert_unchanged_like_git(dir.path());
}

#[test]
fn a_crlf_checkout_under_an_eol_attribute_has_no_changes() {
    let dir = tempfile::tempdir().unwrap();
    crlf_checkout(dir.path(), |test| {
        test.write(".gitattributes", "* text=auto eol=crlf\n");
    });
    assert_unchanged_like_git(dir.path());
}

#[test]
fn an_edited_crlf_line_is_one_hunk() {
    let dir = tempfile::tempdir().unwrap();
    crlf_checkout(dir.path(), |_| {
        append_config(dir.path(), "[core]\n\tautocrlf = true\n");
    });
    let repo = repo_in(dir.path());
    let page = dir.path().join("page.adoc");
    let edited = "= Page\r\n\r\nFirst line, edited.\r\n";
    let hunks = repo.line_diff(&page, Some(edited)).unwrap();
    assert_eq!(hunks.len(), 1, "{hunks:?}");
    assert_eq!(hunks[0].kind, HunkKind::Modified);
    assert_eq!(hunks[0].lines, 2..3);
    assert_eq!(hunks[0].head_lines, 2..3);
    let blame = repo.blame(&page, Some(edited)).unwrap();
    let committed: Vec<bool> = blame.iter().map(|line| line.commit.is_some()).collect();
    assert_eq!(committed, [true, true, false]);
}
