//! `adoc-git` against temporary repositories built with `gix`.

use std::path::Path;

use adoc_core::SourceRange;
use adoc_git::{testing::TestRepo, Change, FileStatus, Head, HunkKind, Repository};

fn repo_in(dir: &Path) -> Repository {
    Repository::discover(dir)
        .expect("readable")
        .expect("a repository")
}

fn status_of(statuses: &[FileStatus], path: &str) -> (Change, Change) {
    let status = statuses
        .iter()
        .find(|status| status.path == Path::new(path))
        .unwrap_or_else(|| panic!("{path} has no status in {statuses:?}"));
    (status.index.clone(), status.worktree.clone())
}

#[test]
fn discover_finds_repo_from_nested_path() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("docs/modules/ROOT/pages/index.adoc", "= Home\n");
    test.commit_all("first");

    let nested = dir.path().join("docs/modules/ROOT/pages");
    let repo = repo_in(&nested);
    assert_eq!(repo.workdir(), dir.path());
    assert_eq!(repo.git_dir(), dir.path().join(".git"));
}

#[test]
fn discover_returns_none_outside_repo() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("plain/folder")).unwrap();
    // A temporary directory may itself sit inside a repository on some machines; only claim
    // "none" when there is no `.git` above it.
    let inside_another = dir
        .path()
        .ancestors()
        .any(|ancestor| ancestor.join(".git").exists());
    let found = Repository::discover(&dir.path().join("plain/folder")).unwrap();
    assert_eq!(found.is_some(), inside_another);
}

#[test]
fn head_reports_branch_detached_and_unborn() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    assert_eq!(repo_in(dir.path()).head().unwrap(), Head::Unborn);

    test.write("a.adoc", "a\n");
    let id = test.commit_all("first");
    assert_eq!(
        repo_in(dir.path()).head().unwrap(),
        Head::Branch("main".to_owned())
    );

    test.switch_branch("release/1.0");
    assert_eq!(
        repo_in(dir.path()).head().unwrap(),
        Head::Branch("release/1.0".to_owned())
    );

    assert_eq!(test.detach(), id);
    let detached = repo_in(dir.path()).head().unwrap();
    assert_eq!(detached.to_string(), format!("detached @ {}", &id[..7]));
    assert_eq!(detached, Head::Detached(id));
    assert_eq!(Head::Unborn.to_string(), "no commits yet");
}

#[test]
fn status_reports_added_modified_deleted_untracked_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write(".gitignore", "build/\n");
    test.write("changed.adoc", "one\n");
    test.write("deleted.adoc", "gone\n");
    test.write("same.adoc", "same\n");
    test.commit_all("first");

    test.write("added.adoc", "new and staged\n");
    test.stage("added.adoc");
    test.write("changed.adoc", "one\ntwo\n");
    test.remove("deleted.adoc");
    test.write("untracked.adoc", "loose\n");
    test.write("build/site.html", "<html>\n");

    let statuses = repo_in(dir.path()).status().unwrap();
    assert_eq!(
        status_of(&statuses, "added.adoc"),
        (Change::Added, Change::Unmodified)
    );
    assert_eq!(
        status_of(&statuses, "changed.adoc"),
        (Change::Unmodified, Change::Modified)
    );
    assert_eq!(
        status_of(&statuses, "deleted.adoc"),
        (Change::Unmodified, Change::Deleted)
    );
    assert_eq!(
        status_of(&statuses, "untracked.adoc"),
        (Change::Unmodified, Change::Untracked)
    );
    assert_eq!(
        status_of(&statuses, "build"),
        (Change::Unmodified, Change::Ignored)
    );
    assert!(
        !statuses
            .iter()
            .any(|status| status.path == Path::new("same.adoc")),
        "an unchanged file has no status"
    );
}

/// Twenty lines, `line 1` to `line 20`.
fn twenty_lines() -> String {
    (1..=20).map(|number| format!("line {number}\n")).collect()
}

#[test]
fn line_diff_marks_added_modified_deleted_hunks() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("page.adoc", &twenty_lines());
    test.commit_all("first");

    // An insertion after line 3, a change to line 10 and the deletion of line 15.
    let mut lines: Vec<String> = (1..=20).map(|number| format!("line {number}")).collect();
    lines.insert(3, "inserted".to_owned());
    lines[10] = "line ten, changed".to_owned();
    lines.remove(15);
    let current = lines.join("\n") + "\n";
    test.write("page.adoc", &current);

    let hunks = repo_in(dir.path())
        .line_diff(&dir.path().join("page.adoc"), None)
        .unwrap();
    assert_eq!(hunks.len(), 3, "{hunks:?}");

    let offset = |needle: &str| current.find(needle).unwrap();
    assert_eq!(hunks[0].kind, HunkKind::Added);
    assert_eq!(hunks[0].lines, 3..4);
    assert_eq!(hunks[0].head_lines, 3..3);
    assert_eq!(
        hunks[0].range,
        SourceRange::new(offset("inserted"), offset("line 4\n"))
    );

    assert_eq!(hunks[1].kind, HunkKind::Modified);
    assert_eq!(hunks[1].lines, 10..11);
    assert_eq!(hunks[1].head_lines, 9..10);
    assert_eq!(
        hunks[1].range,
        SourceRange::new(offset("line ten"), offset("line 11\n"))
    );

    assert_eq!(hunks[2].kind, HunkKind::Deleted);
    assert_eq!(hunks[2].lines, 15..15);
    assert_eq!(hunks[2].head_lines, 14..15);
    let after = offset("line 16\n");
    assert_eq!(hunks[2].range, SourceRange::new(after, after));
}

#[test]
fn line_diff_uses_buffer_text_when_given() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("page.adoc", "one\ntwo\n");
    test.commit_all("first");

    let repo = repo_in(dir.path());
    let page = dir.path().join("page.adoc");
    assert!(repo.line_diff(&page, None).unwrap().is_empty());
    let hunks = repo.line_diff(&page, Some("one\ntwo\nthree\n")).unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].kind, HunkKind::Added);
    assert_eq!(hunks[0].lines, 2..3);
    // The file on disk is untouched.
    assert!(repo.line_diff(&page, None).unwrap().is_empty());
}

#[test]
fn head_content_and_remote_url() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("page.adoc", "committed\n");
    test.commit_all("first");
    test.write("page.adoc", "edited\n");

    let repo = repo_in(dir.path());
    assert_eq!(
        repo.head_content(Path::new("page.adoc"))
            .unwrap()
            .as_deref(),
        Some("committed\n")
    );
    assert_eq!(repo.head_content(Path::new("absent.adoc")).unwrap(), None);
    assert!(repo
        .head_content(Path::new("/elsewhere/page.adoc"))
        .is_err());
    assert_eq!(repo.remote_url("origin").unwrap(), None);
}

#[test]
fn branches_and_files_at_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("docs/antora.yml", "name: product\nversion: ~\n");
    test.commit_all("main");
    test.switch_branch("v1.0");
    test.write("docs/antora.yml", "name: product\nversion: '1.0'\n");
    test.commit_all("version 1.0");
    test.checkout("main");

    let repo = repo_in(dir.path());
    let names: Vec<String> = repo
        .branches()
        .unwrap()
        .into_iter()
        .map(|branch| branch.name)
        .collect();
    assert_eq!(names, ["main", "v1.0"]);
    assert_eq!(
        repo.file_at("v1.0", Path::new("docs/antora.yml"))
            .unwrap()
            .as_deref(),
        Some("name: product\nversion: '1.0'\n")
    );
    assert_eq!(
        repo.file_at("main", Path::new("docs/antora.yml"))
            .unwrap()
            .as_deref(),
        Some("name: product\nversion: ~\n")
    );
    assert_eq!(repo.file_at("v1.0", Path::new("absent.yml")).unwrap(), None);
    assert!(repo
        .file_at("no-such-branch", Path::new("docs/antora.yml"))
        .is_err());
    // The checkout put main's files back in the working tree.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("docs/antora.yml")).unwrap(),
        "name: product\nversion: ~\n"
    );
}
