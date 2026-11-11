//! Conformance with the `git` executable: `Repository::stage` leaves the index as `git add` does,
//! or refuses, and `Repository::unstage` as `git restore --staged` does. Each test builds the same
//! repository twice, stages in one with each, and compares the index entries, `git status` and
//! `git write-tree`. Skipped when `git` is absent.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use adoc_git::Repository;

/// Runs `git` in `dir`, reading no configuration but the repository's own.
fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_AUTHOR_NAME", "Ada Writer")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada Writer")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .output()
        .expect("git runs")
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let output = git(dir, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn has_git() -> bool {
    let found = Command::new("git").arg("--version").output().is_ok();
    if !found {
        eprintln!("git not found: skipping");
    }
    found
}

fn init(dir: &Path, config: &[(&str, &str)]) {
    git_ok(dir, &["init", "-q", "-b", "main"]);
    for (key, value) in config {
        git_ok(dir, &["config", key, value]);
    }
}

fn stage(dir: &Path, paths: &[&str]) -> Result<(), String> {
    let repo = Repository::discover_isolated(dir).unwrap().unwrap();
    let paths: Vec<_> = paths.iter().map(|path| dir.join(path)).collect();
    let borrowed: Vec<&Path> = paths.iter().map(|path| path.as_path()).collect();
    repo.stage(&borrowed).map_err(|error| error.to_string())
}

/// The state of the index and the working tree against it, as `git` reads them, with the tree a
/// commit would hold: `write-tree` trusts the index's cached trees, so a stale one shows here.
fn state(dir: &Path) -> (String, String) {
    (
        git_ok(dir, &["ls-files", "--stage"]),
        git_ok(dir, &["status", "--porcelain"]) + &git_ok(dir, &["write-tree"]),
    )
}

/// Builds the repository `setup` makes twice, and checks that staging `paths` in one leaves it as
/// `git add` leaves the other. Returns what they both hold.
fn assert_stages_like_git_add(paths: &[&str], setup: impl Fn(&Path)) -> (String, String) {
    let ours = tempfile::tempdir().unwrap();
    let theirs = tempfile::tempdir().unwrap();
    setup(ours.path());
    setup(theirs.path());
    stage(ours.path(), paths).unwrap();
    let mut add = vec!["add", "--"];
    add.extend_from_slice(paths);
    git_ok(theirs.path(), &add);
    let expected = state(theirs.path());
    assert_eq!(state(ours.path()), expected);
    expected
}

/// As `assert_stages_like_git_add`, for `Repository::unstage` and `git restore --staged`.
fn assert_unstages_like_git_restore(paths: &[&str], setup: impl Fn(&Path)) -> (String, String) {
    let ours = tempfile::tempdir().unwrap();
    let theirs = tempfile::tempdir().unwrap();
    setup(ours.path());
    setup(theirs.path());
    let repo = Repository::discover_isolated(ours.path()).unwrap().unwrap();
    let joined: Vec<_> = paths.iter().map(|path| ours.path().join(path)).collect();
    let borrowed: Vec<&Path> = joined.iter().map(|path| path.as_path()).collect();
    repo.unstage(&borrowed).unwrap();
    let mut restore = vec!["restore", "--staged", "--"];
    restore.extend_from_slice(paths);
    git_ok(theirs.path(), &restore);
    let expected = state(theirs.path());
    assert_eq!(state(ours.path()), expected);
    expected
}

fn commit_all(dir: &Path) {
    git_ok(dir, &["add", "-A"]);
    git_ok(dir, &["commit", "-q", "-m", "first"]);
}

/// The content staged for `path`, from the entry in `ls-files --stage`.
fn staged_blob(dir: &Path, path: &str) -> Vec<u8> {
    let object = format!(":{path}");
    let output = git(dir, &["cat-file", "blob", &object]);
    assert!(output.status.success());
    output.stdout
}

#[test]
fn crlf_with_autocrlf_true_is_stored_with_lf() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[("core.autocrlf", "true")]);
        fs::write(dir.join("page.adoc"), "one\r\ntwo\r\n").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), &[("core.autocrlf", "true")]);
    fs::write(dir.path().join("page.adoc"), "one\r\ntwo\r\n").unwrap();
    stage(dir.path(), &["page.adoc"]).unwrap();
    assert_eq!(staged_blob(dir.path(), "page.adoc"), b"one\ntwo\n");
}

#[test]
fn crlf_with_autocrlf_input_is_stored_with_lf() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[("core.autocrlf", "input")]);
        fs::write(dir.join("page.adoc"), "one\r\ntwo\r\n").unwrap();
    });
}

#[test]
fn a_file_committed_with_crlf_keeps_it_under_autocrlf() {
    if !has_git() {
        return;
    }
    // Git leaves a file alone when the index already holds it with carriage returns.
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[("core.autocrlf", "false")]);
        fs::write(dir.join("page.adoc"), "one\r\n").unwrap();
        commit_all(dir);
        git_ok(dir, &["config", "core.autocrlf", "true"]);
        fs::write(dir.join("page.adoc"), "one\r\ntwo\r\n").unwrap();
    });
}

#[test]
fn text_eol_lf_attribute_is_stored_with_lf() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[]);
        fs::write(dir.join(".gitattributes"), "*.adoc text eol=lf\n").unwrap();
        fs::write(dir.join("page.adoc"), "one\r\ntwo\r\n").unwrap();
    });
}

#[test]
fn text_eol_crlf_attribute_is_stored_with_lf() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[]);
        fs::write(dir.join(".gitattributes"), "*.adoc text eol=crlf\n").unwrap();
        fs::write(dir.join("page.adoc"), "one\r\ntwo\r\n").unwrap();
    });
}

#[test]
fn unset_text_attribute_keeps_crlf() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[("core.autocrlf", "true")]);
        fs::write(dir.join(".gitattributes"), "*.adoc -text\n").unwrap();
        fs::write(dir.join("page.adoc"), "one\r\ntwo\r\n").unwrap();
    });
}

#[test]
fn a_binary_file_under_text_auto_is_left_alone() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["image.bin"], |dir| {
        init(dir, &[]);
        fs::write(dir.join(".gitattributes"), "* text=auto\n").unwrap();
        fs::write(dir.join("image.bin"), b"\0\x01\r\nbinary\r\n").unwrap();
    });
}

#[test]
fn ident_attribute_collapses_the_keyword() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[]);
        fs::write(dir.join(".gitattributes"), "*.adoc ident\n").unwrap();
        fs::write(dir.join("page.adoc"), "$Id: 0123456789 $\n").unwrap();
    });
}

#[test]
fn working_tree_encoding_is_stored_as_utf8() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[]);
        fs::write(
            dir.join(".gitattributes"),
            "*.adoc working-tree-encoding=UTF-16LE\n",
        )
        .unwrap();
        let text: Vec<u8> = "one\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
        fs::write(dir.join("page.adoc"), text).unwrap();
    });
}

#[test]
fn a_safecrlf_failure_refuses_as_git_add_does() {
    if !has_git() {
        return;
    }
    let setup = |dir: &Path| {
        init(
            dir,
            &[("core.autocrlf", "input"), ("core.safecrlf", "true")],
        );
        fs::write(dir.join("page.adoc"), "one\r\n").unwrap();
    };
    let ours = tempfile::tempdir().unwrap();
    let theirs = tempfile::tempdir().unwrap();
    setup(ours.path());
    setup(theirs.path());
    assert!(!git(theirs.path(), &["add", "page.adoc"]).status.success());
    assert!(stage(ours.path(), &["page.adoc"]).is_err());
    assert_eq!(state(ours.path()), state(theirs.path()));
}

#[test]
fn a_file_under_a_filter_is_refused_and_its_driver_not_run() {
    if !has_git() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("driver-ran");
    let clean = if cfg!(windows) {
        format!("cmd /c echo ran> \"{}\"", marker.display())
    } else {
        format!("touch '{}'; cat", marker.display())
    };
    init(dir.path(), &[("filter.lfs.clean", clean.as_str())]);
    fs::write(dir.path().join(".gitattributes"), "*.png filter=lfs\n").unwrap();
    fs::write(dir.path().join("image.png"), b"picture").unwrap();
    fs::write(dir.path().join("page.adoc"), "one\n").unwrap();

    let before = state(dir.path());
    let error = stage(dir.path(), &["page.adoc", "image.png"]).unwrap_err();
    assert!(error.contains("image.png"), "{error}");
    assert!(error.contains("`lfs`"), "{error}");
    assert!(!marker.exists(), "the filter's driver was run");
    assert_eq!(state(dir.path()), before, "nothing is staged");
}

#[test]
fn a_named_filter_without_a_driver_is_refused() {
    if !has_git() {
        return;
    }
    // Git would store the raw file, the mistake LFS users most want to avoid when it is not set
    // up on this machine, so it is left for them to do by hand.
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), &[]);
    fs::write(dir.path().join(".gitattributes"), "*.png filter=lfs\n").unwrap();
    fs::write(dir.path().join("image.png"), b"picture").unwrap();
    let error = stage(dir.path(), &["image.png"]).unwrap_err();
    assert!(error.contains("`lfs`"), "{error}");
}

#[test]
fn an_unset_filter_is_staged_as_git_add_does() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["image.png"], |dir| {
        init(dir, &[]);
        fs::write(dir.join(".gitattributes"), "* filter=lfs\n*.png -filter\n").unwrap();
        fs::write(dir.join("image.png"), b"picture").unwrap();
    });
}

#[test]
fn a_file_outside_the_filter_s_pattern_is_staged() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[]);
        fs::write(dir.join(".gitattributes"), "*.png filter=lfs\n").unwrap();
        fs::write(dir.join("page.adoc"), "one\n").unwrap();
    });
}

#[test]
fn a_removed_file_is_staged_as_deleted() {
    if !has_git() {
        return;
    }
    assert_stages_like_git_add(&["page.adoc"], |dir| {
        init(dir, &[]);
        fs::write(dir.join("page.adoc"), "one\n").unwrap();
        commit_all(dir);
        fs::remove_file(dir.join("page.adoc")).unwrap();
    });
}

#[test]
fn changes_in_a_folder_are_staged_so_git_sees_them() {
    if !has_git() {
        return;
    }
    // A commit leaves the index with cached trees, which staging must not leave stale.
    let (entries, status) = assert_stages_like_git_add(
        &["docs/page.adoc", "docs/new.adoc", "docs/gone.adoc"],
        |dir| {
            init(dir, &[]);
            fs::create_dir(dir.join("docs")).unwrap();
            fs::write(dir.join("docs/page.adoc"), "one\n").unwrap();
            fs::write(dir.join("docs/gone.adoc"), "gone\n").unwrap();
            commit_all(dir);
            fs::write(dir.join("docs/page.adoc"), "two\n").unwrap();
            fs::write(dir.join("docs/new.adoc"), "new\n").unwrap();
            fs::remove_file(dir.join("docs/gone.adoc")).unwrap();
        },
    );
    assert!(entries.contains("docs/new.adoc"), "{entries}");
    assert!(status.contains("M  docs/page.adoc"), "{status}");
}

#[test]
fn unstaging_in_a_folder_leaves_no_stale_tree() {
    if !has_git() {
        return;
    }
    let (_, status) = assert_unstages_like_git_restore(&["docs/page.adoc"], |dir| {
        init(dir, &[]);
        fs::create_dir(dir.join("docs")).unwrap();
        fs::write(dir.join("docs/page.adoc"), "one\n").unwrap();
        commit_all(dir);
        fs::write(dir.join("docs/page.adoc"), "two\n").unwrap();
        git_ok(dir, &["add", "docs/page.adoc"]);
        // Caches the staged tree, which unstaging must not leave behind.
        git_ok(dir, &["write-tree"]);
    });
    assert!(status.starts_with(" M docs/page.adoc"), "{status}");
}

#[test]
fn unstaging_a_new_file_leaves_it_untracked() {
    if !has_git() {
        return;
    }
    let (entries, _) = assert_unstages_like_git_restore(&["new.adoc"], |dir| {
        init(dir, &[]);
        fs::write(dir.join("page.adoc"), "one\n").unwrap();
        commit_all(dir);
        fs::write(dir.join("new.adoc"), "new\n").unwrap();
        git_ok(dir, &["add", "new.adoc"]);
    });
    assert!(!entries.contains("new.adoc"), "{entries}");
}

#[test]
fn unstaging_a_submodule_keeps_it_a_submodule() {
    if !has_git() {
        return;
    }
    let (entries, _) = assert_unstages_like_git_restore(&["sub"], |dir| {
        init(dir, &[]);
        fs::write(dir.join("page.adoc"), "one\n").unwrap();
        commit_all(dir);
        let first = git_ok(dir, &["rev-parse", "HEAD"]);
        let gitlink = format!("160000,{},sub", first.trim());
        git_ok(dir, &["update-index", "--add", "--cacheinfo", &gitlink]);
        git_ok(dir, &["commit", "-q", "-m", "second"]);
        let second = git_ok(dir, &["rev-parse", "HEAD"]);
        let gitlink = format!("160000,{},sub", second.trim());
        git_ok(dir, &["update-index", "--cacheinfo", &gitlink]);
    });
    assert!(entries.starts_with("100644 "), "{entries}");
    assert!(entries.contains("160000 "), "{entries}");
}

#[test]
fn unstaging_a_folder_is_refused() {
    if !has_git() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), &[]);
    fs::create_dir(dir.path().join("docs")).unwrap();
    fs::write(dir.path().join("docs/page.adoc"), "one\n").unwrap();
    commit_all(dir.path());
    fs::write(dir.path().join("docs/page.adoc"), "two\n").unwrap();
    git_ok(dir.path(), &["add", "docs/page.adoc"]);
    let before = state(dir.path());
    let repo = Repository::discover_isolated(dir.path()).unwrap().unwrap();
    let error = repo
        .unstage(&[dir.path().join("docs").as_path()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("folder"), "{error}");
    assert_eq!(state(dir.path()), before, "nothing is unstaged");
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    use super::*;

    #[test]
    fn a_symlink_to_a_file_is_staged_as_a_link() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_stages_like_git_add(&["link.adoc"], |dir| {
            init(dir, &[]);
            fs::write(dir.join("page.adoc"), "the target's text\n").unwrap();
            symlink("page.adoc", dir.join("link.adoc")).unwrap();
        });
        assert!(entries.starts_with("120000 "), "{entries}");
    }

    #[test]
    fn a_symlink_to_a_folder_is_staged_as_a_link() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_stages_like_git_add(&["link"], |dir| {
            init(dir, &[]);
            fs::create_dir(dir.join("folder")).unwrap();
            symlink("folder", dir.join("link")).unwrap();
        });
        assert!(entries.starts_with("120000 "), "{entries}");
    }

    #[test]
    fn a_dangling_symlink_is_staged_as_a_link() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_stages_like_git_add(&["link.adoc"], |dir| {
            init(dir, &[]);
            symlink("nowhere.adoc", dir.join("link.adoc")).unwrap();
        });
        assert!(entries.starts_with("120000 "), "{entries}");
    }

    #[test]
    fn a_link_is_not_filtered_as_text() {
        if !has_git() {
            return;
        }
        assert_stages_like_git_add(&["link.adoc"], |dir| {
            init(dir, &[("core.autocrlf", "true")]);
            fs::write(dir.join(".gitattributes"), "*.adoc filter=lfs\n").unwrap();
            symlink("nowhere", dir.join("link.adoc")).unwrap();
        });
    }

    #[test]
    fn a_link_checked_out_as_a_file_stays_a_link_without_symlinks() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_stages_like_git_add(&["link.adoc"], |dir| {
            init(dir, &[]);
            symlink("page.adoc", dir.join("link.adoc")).unwrap();
            commit_all(dir);
            git_ok(dir, &["config", "core.symlinks", "false"]);
            // What a checkout writes on a file system without links.
            fs::remove_file(dir.join("link.adoc")).unwrap();
            fs::write(dir.join("link.adoc"), "other.adoc").unwrap();
        });
        assert!(entries.starts_with("120000 "), "{entries}");
    }

    #[test]
    fn unstaging_a_link_keeps_it_a_link() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_unstages_like_git_restore(&["link.adoc"], |dir| {
            init(dir, &[]);
            fs::write(dir.join("target.adoc"), "target\n").unwrap();
            symlink("target.adoc", dir.join("link.adoc")).unwrap();
            commit_all(dir);
            fs::remove_file(dir.join("link.adoc")).unwrap();
            symlink("other.adoc", dir.join("link.adoc")).unwrap();
            stage(dir, &["link.adoc"]).unwrap();
        });
        assert!(entries.starts_with("120000 "), "{entries}");
    }

    #[test]
    fn unstaging_an_executable_file_keeps_it_executable() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_unstages_like_git_restore(&["run.sh"], |dir| {
            init(dir, &[]);
            fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
            fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
            commit_all(dir);
            fs::write(dir.join("run.sh"), "#!/bin/sh\n# changed\n").unwrap();
            git_ok(dir, &["add", "run.sh"]);
        });
        assert!(entries.starts_with("100755 "), "{entries}");
    }

    #[test]
    fn an_executable_file_is_staged_as_executable() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_stages_like_git_add(&["run.sh"], |dir| {
            init(dir, &[]);
            fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
            fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        });
        assert!(entries.starts_with("100755 "), "{entries}");
    }

    #[test]
    fn the_index_keeps_its_mode_without_file_modes() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_stages_like_git_add(&["run.sh"], |dir| {
            init(dir, &[]);
            fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
            fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
            commit_all(dir);
            git_ok(dir, &["config", "core.filemode", "false"]);
            fs::write(dir.join("run.sh"), "#!/bin/sh\n# changed\n").unwrap();
            fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
        });
        assert!(entries.starts_with("100755 "), "{entries}");
    }

    #[test]
    fn a_new_file_is_not_executable_without_file_modes() {
        if !has_git() {
            return;
        }
        let (entries, _) = assert_stages_like_git_add(&["run.sh"], |dir| {
            init(dir, &[("core.filemode", "false")]);
            fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
            fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        });
        assert!(entries.starts_with("100644 "), "{entries}");
    }
}
