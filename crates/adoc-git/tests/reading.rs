//! Reading a repository as Git reads it: diffs and blame through the line ending conversions,
//! and history in a shallow clone. Repositories are built with `gix`; where `git` is installed
//! the tests also check that it agrees.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use adoc_git::{testing::TestRepo, Change, HunkKind, Repository};

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

#[test]
fn last_changed_stops_at_a_shallow_clone_s_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("old.adoc", "old\n");
    let first = test.commit_all_at("first", 1_000);
    test.write("new.adoc", "new\n");
    let boundary = test.commit_all_at("second", 2_000);
    test.write("new.adoc", "newer\n");
    let newest = test.commit_all_at("third", 3_000);
    // A clone two commits deep: the boundary is recorded and its parent is not there.
    fs::write(dir.path().join(".git/shallow"), format!("{boundary}\n")).unwrap();
    fs::remove_file(
        dir.path()
            .join(".git/objects")
            .join(&first[..2])
            .join(&first[2..]),
    )
    .unwrap();
    if let Some(output) = git(dir.path(), &["log", "--format=%H"]) {
        assert!(output.status.success(), "git reads the shallow history");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("{newest}\n{boundary}\n")
        );
    }

    let repo = repo_in(dir.path());
    let paths = [Path::new("old.adoc").to_path_buf(), "new.adoc".into()];
    let found = repo.last_changed(&paths, adoc_git::HISTORY_LIMIT).unwrap();
    assert_eq!(found.changes.len(), 1, "{found:?}");
    assert_eq!(found.changes[Path::new("new.adoc")].commit, newest);
    assert_eq!(found.cut_short, Some(2_000));
}

#[test]
fn unpushed_counts_the_branch_asked_for_not_head() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("page.adoc", "one\n");
    test.commit_all("first");
    test.add_remote("origin", "git@github.com:org/docs.git");
    test.track_remote("origin");
    test.switch_branch("other");
    test.write("page.adoc", "two\n");
    test.commit_all("second");
    assert_eq!(
        repo_in(dir.path()).unpushed("origin", "main").unwrap(),
        Some(0)
    );
}

#[test]
fn status_reports_a_copy_as_added() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    append_config(dir.path(), "[status]\n\trenames = copies\n");
    let text: String = (1..=20).map(|line| format!("line {line}\n")).collect();
    test.write("a.adoc", &text);
    test.commit_all("first");
    // Git finds copies of files changed in the same change.
    test.write("a.adoc", &format!("{text}line 21\n"));
    test.write("b.adoc", &text);
    let repo = repo_in(dir.path());
    repo.stage(&[&dir.path().join("a.adoc"), &dir.path().join("b.adoc")])
        .unwrap();
    let status = repo.status().unwrap();
    let b = status
        .iter()
        .find(|status| status.path == Path::new("b.adoc"))
        .unwrap();
    // Git says `C  a.adoc -> b.adoc`: a copy, which leaves `a.adoc` where it was.
    assert_eq!(b.index, Change::Added, "{status:?}");
}

#[test]
fn identity_reads_author_and_committer_settings() {
    let dir = tempfile::tempdir().unwrap();
    TestRepo::init(dir.path());
    append_config(
        dir.path(),
        "[author]\n\tname = Ada Author\n\temail = ada@example.com\n\
         [committer]\n\tname = Cy Committer\n\temail = cy@example.com\n",
    );
    let repo = Repository::discover_isolated(dir.path()).unwrap().unwrap();
    assert_eq!(
        repo.identity().unwrap(),
        ("Cy Committer".to_owned(), "cy@example.com".to_owned())
    );
}

#[test]
fn file_at_takes_an_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("docs/page.adoc", "= Page\n");
    test.commit_all("first");
    let repo = repo_in(dir.path());
    assert_eq!(
        repo.file_at("main", &dir.path().join("docs/page.adoc"))
            .unwrap()
            .as_deref(),
        Some("= Page\n")
    );
}
