//! When each file last changed: one walk of `HEAD`'s first-parent history, newest first, each
//! commit's tree compared with its parent's, stopping once every file is found or after a
//! limit. A file moved without a change is followed to its earlier name, so a move does not
//! make a page new. Object data only, as in `revision.rs`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use gix::bstr::ByteSlice;

use crate::{repository::relative_path, GitError, Repository};

/// How many commits [`Repository::last_changed`] reads by default.
pub const HISTORY_LIMIT: usize = 10_000;

/// The commit that last changed a file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LastChange {
    /// The commit's id, in hex.
    pub commit: String,
    /// When it was committed, in seconds since the Unix epoch.
    pub time: i64,
}

/// What [`Repository::last_changed`] found.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LastChanges {
    /// Each file asked for that is in `HEAD` and whose last change was found, by the path as
    /// it was asked for.
    pub changes: HashMap<PathBuf, LastChange>,
    /// When the walk stopped at its limit, or at a shallow clone's boundary, with files of
    /// `HEAD` still not found: the time of the oldest commit read, which those files are at
    /// least as old as.
    pub cut_short: Option<i64>,
}

impl Repository {
    /// When each of `paths` (relative to the working tree, or absolute inside it) last changed
    /// in `HEAD`'s history, reading at most `limit` commits. A path not in `HEAD` is left out;
    /// with no commit yet, nothing is found.
    pub fn last_changed(&self, paths: &[PathBuf], limit: usize) -> Result<LastChanges, GitError> {
        let mut found = LastChanges::default();
        let Ok(head) = self.repo.head_id() else {
            return Ok(found);
        };
        let head_tree = self.repo.head_tree().map_err(GitError::git)?;
        // Each file still looked for, by its name at the commit being read, and the path it
        // was asked for by.
        let mut wanted: HashMap<PathBuf, &Path> = HashMap::new();
        for path in paths {
            let relative = self.relative(path)?;
            let in_head = head_tree
                .lookup_entry_by_path(&relative)
                .map_err(GitError::git)?
                .is_some_and(|entry| entry.mode().is_blob());
            if in_head {
                wanted.insert(relative, path.as_path());
            }
        }
        let walk = self
            .repo
            .rev_walk([head.detach()])
            .first_parent_only()
            .all()
            .map_err(GitError::git)?;
        let options =
            gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
        // A shallow clone's boundary commits have parents that are not there to compare with.
        let shallow = self.repo.shallow_commits().map_err(GitError::git)?;
        let mut oldest = None;
        for (read, info) in walk.enumerate() {
            if wanted.is_empty() {
                break;
            }
            if read == limit {
                found.cut_short = oldest;
                break;
            }
            let info = info.map_err(GitError::git)?;
            let commit = info.object().map_err(GitError::git)?;
            let time = commit.time().map_err(GitError::git)?.seconds;
            oldest = Some(time);
            let parent_missing = info.parent_ids().next().is_some()
                && shallow
                    .as_ref()
                    .is_some_and(|boundary| boundary.contains(&commit.id));
            if parent_missing {
                // As at the limit: what is still looked for is at least as old as this commit.
                found.cut_short = oldest;
                break;
            }
            let tree = commit.tree().map_err(GitError::git)?;
            let parent = match info.parent_ids().next() {
                Some(parent) => parent
                    .object()
                    .map_err(GitError::git)?
                    .peel_to_commit()
                    .map_err(GitError::git)?
                    .tree()
                    .map_err(GitError::git)?,
                None => self.repo.empty_tree(),
            };
            let changes = self
                .repo
                .diff_tree_to_tree(&parent, &tree, options)
                .map_err(GitError::git)?;
            let change = LastChange {
                commit: commit.id.to_string(),
                time,
            };
            for change_made in changes {
                use gix::object::tree::diff::ChangeDetached;
                let (location, moved_from) = match &change_made {
                    ChangeDetached::Addition { location, .. }
                    | ChangeDetached::Modification { location, .. } => (location, None),
                    ChangeDetached::Rewrite {
                        source_location,
                        location,
                        diff,
                        copy,
                        ..
                    } => {
                        // A move found by its object id has no diff: the text is the same.
                        let unchanged = !copy && diff.is_none();
                        (location, unchanged.then_some(source_location))
                    }
                    ChangeDetached::Deletion { .. } => continue,
                };
                let at = relative_path(location.as_bstr());
                let Some(asked) = wanted.remove(&at) else {
                    continue;
                };
                match moved_from {
                    Some(from) => {
                        wanted.insert(relative_path(from.as_bstr()), asked);
                    }
                    None => {
                        found.changes.insert(asked.to_path_buf(), change.clone());
                    }
                }
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    const DAY: i64 = 86_400;

    #[test]
    fn last_changed_finds_each_paths_newest_commit() {
        let directory = tempfile::tempdir().unwrap();
        let repo = TestRepo::init(directory.path());
        let long = "A page long enough that a small edit leaves it mostly the same.\n".repeat(8);
        repo.write("docs/edited.adoc", "= Edited\n");
        repo.write("docs/moved.adoc", &long);
        repo.write("docs/renamed.adoc", &long);
        repo.write("docs/untouched.adoc", "= Untouched\n");
        repo.write("docs/gone.adoc", "= Gone\n");
        let first = repo.commit_all_at("First", 10 * DAY);
        repo.write("docs/edited.adoc", "= Edited\n\nMore.\n");
        repo.remove("docs/moved.adoc");
        repo.write("docs/now-here.adoc", &long);
        let second = repo.commit_all_at("Edit one, move one", 20 * DAY);
        repo.remove("docs/renamed.adoc");
        repo.write(
            "docs/renamed-and-edited.adoc",
            &format!("{long}One more line.\n"),
        );
        repo.remove("docs/gone.adoc");
        let third = repo.commit_all_at("Rename with an edit, delete one", 30 * DAY);

        let paths: Vec<PathBuf> = [
            "docs/edited.adoc",
            "docs/now-here.adoc",
            "docs/renamed-and-edited.adoc",
            "docs/untouched.adoc",
            "docs/gone.adoc",
            "docs/never.adoc",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let found = Repository::discover(directory.path())
            .unwrap()
            .unwrap()
            .last_changed(&paths, HISTORY_LIMIT)
            .unwrap();
        let at = |path: &str| {
            found
                .changes
                .get(Path::new(path))
                .map(|change| (change.commit.clone(), change.time))
        };
        assert_eq!(at("docs/edited.adoc"), Some((second, 20 * DAY)));
        assert_eq!(
            at("docs/now-here.adoc"),
            Some((first.clone(), 10 * DAY)),
            "an unchanged move keeps the page's age"
        );
        assert_eq!(
            at("docs/renamed-and-edited.adoc"),
            Some((third, 30 * DAY)),
            "a move with an edit is a change"
        );
        assert_eq!(at("docs/untouched.adoc"), Some((first, 10 * DAY)));
        assert_eq!(at("docs/gone.adoc"), None, "not in HEAD");
        assert_eq!(at("docs/never.adoc"), None);
        assert_eq!(found.cut_short, None);

        // An absolute path is answered by the same path.
        let absolute = directory.path().join("docs/untouched.adoc");
        let found = Repository::discover(directory.path())
            .unwrap()
            .unwrap()
            .last_changed(std::slice::from_ref(&absolute), HISTORY_LIMIT)
            .unwrap();
        assert!(found.changes.contains_key(&absolute));
    }

    #[test]
    fn last_changed_stops_at_the_limit() {
        let directory = tempfile::tempdir().unwrap();
        let repo = TestRepo::init(directory.path());
        repo.write("old.adoc", "= Old\n");
        repo.write("new.adoc", "= New\n");
        repo.commit_all_at("First", DAY);
        for day in 2..=4 {
            repo.write("new.adoc", &format!("= New\n\nDay {day}.\n"));
            repo.commit_all_at("Again", day * DAY);
        }
        let repository = Repository::discover(directory.path()).unwrap().unwrap();
        let paths = [PathBuf::from("old.adoc"), PathBuf::from("new.adoc")];
        let found = repository.last_changed(&paths, 2).unwrap();
        assert_eq!(found.changes[Path::new("new.adoc")].time, 4 * DAY);
        assert!(!found.changes.contains_key(Path::new("old.adoc")));
        assert_eq!(
            found.cut_short,
            Some(3 * DAY),
            "old.adoc is older than day 3"
        );
        let all = repository.last_changed(&paths, HISTORY_LIMIT).unwrap();
        assert_eq!(all.changes[Path::new("old.adoc")].time, DAY);

        // Before the first commit, nothing.
        let empty = tempfile::tempdir().unwrap();
        let _repo = TestRepo::init(empty.path());
        let found = Repository::discover(empty.path())
            .unwrap()
            .unwrap()
            .last_changed(&paths, HISTORY_LIMIT)
            .unwrap();
        assert_eq!(found, LastChanges::default());
    }
}
