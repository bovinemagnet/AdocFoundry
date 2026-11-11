//! Reading a revision without checking it out (Impact PRD §§6 and 8): a ref resolved to a
//! commit, the files of its tree, a file's text there, and the changes between two trees with
//! renames detected. Object data only: the working tree, the index and `HEAD` are never touched,
//! nothing is fetched and no `git` process runs.

use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;

use crate::{repository::relative_path, GitError, Repository};

/// A commit a revision names, and its tree.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Revision {
    /// What was asked for: `main`, `HEAD~2`, a commit id.
    pub spec: String,
    /// The commit's id, in hex.
    pub commit: String,
    /// Its tree's id, in hex: what a snapshot of the revision is cached by.
    pub tree: String,
}

/// How a file differs between two trees.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TreeChange {
    Added(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    /// Moved from `from` to `to`; `similarity` is how much of the text stayed, in per cent
    /// (100 for an unchanged move).
    Renamed {
        from: PathBuf,
        to: PathBuf,
        similarity: Option<u8>,
    },
}

impl Repository {
    /// The commit `spec` names: a branch, a tag, `HEAD` or a commit id, as Git reads it.
    pub fn resolve_revision(&self, spec: &str) -> Result<Revision, GitError> {
        let id = self
            .repo
            .rev_parse_single(spec)
            .map_err(|error| GitError::UnknownRevision {
                spec: spec.to_owned(),
                message: error.to_string(),
            })?;
        let commit = id
            .object()
            .map_err(GitError::git)?
            .peel_to_commit()
            .map_err(|error| GitError::UnknownRevision {
                spec: spec.to_owned(),
                message: error.to_string(),
            })?;
        let tree = commit.tree_id().map_err(GitError::git)?;
        Ok(Revision {
            spec: spec.to_owned(),
            commit: commit.id.to_string(),
            tree: tree.to_string(),
        })
    }

    fn tree_of(&self, revision: &Revision) -> Result<gix::Tree<'_>, GitError> {
        let id = gix::ObjectId::from_hex(revision.tree.as_bytes()).map_err(GitError::git)?;
        self.repo
            .find_object(id)
            .map_err(GitError::git)?
            .try_into_tree()
            .map_err(GitError::git)
    }

    /// Every file of `revision`'s tree, relative to the working tree, in path order.
    pub fn tree_files(&self, revision: &Revision) -> Result<Vec<PathBuf>, GitError> {
        let tree = self.tree_of(revision)?;
        let mut files: Vec<PathBuf> = tree
            .traverse()
            .breadthfirst
            .files()
            .map_err(GitError::git)?
            .into_iter()
            .filter(|entry| entry.mode.is_blob())
            .map(|entry| relative_path(entry.filepath.as_bstr()))
            .collect();
        files.sort();
        Ok(files)
    }

    /// The text of `path` (relative to the working tree, or absolute inside it) at `revision`;
    /// `None` when it is not there, or not UTF-8.
    pub fn text_at(&self, revision: &Revision, path: &Path) -> Result<Option<String>, GitError> {
        let relative = self.relative(path)?;
        let tree = self.tree_of(revision)?;
        let Some(entry) = tree
            .lookup_entry_by_path(&relative)
            .map_err(GitError::git)?
        else {
            return Ok(None);
        };
        if !entry.mode().is_blob() {
            return Ok(None);
        }
        let object = entry.object().map_err(GitError::git)?;
        Ok(object.data.to_str().ok().map(str::to_owned))
    }

    /// How `head`'s tree differs from `base`'s, file by file, with renames detected as Git
    /// detects them by default, in path order.
    pub fn tree_changes(
        &self,
        base: &Revision,
        head: &Revision,
    ) -> Result<Vec<TreeChange>, GitError> {
        use gix::object::tree::diff::ChangeDetached;

        let (old, new) = (self.tree_of(base)?, self.tree_of(head)?);
        let options =
            gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
        let changes = self
            .repo
            .diff_tree_to_tree(&old, &new, options)
            .map_err(GitError::git)?;
        let mut found = Vec::new();
        for change in changes {
            match change {
                ChangeDetached::Addition {
                    location,
                    entry_mode,
                    ..
                } if entry_mode.is_blob() => {
                    found.push(TreeChange::Added(relative_path(location.as_bstr())))
                }
                ChangeDetached::Deletion {
                    location,
                    entry_mode,
                    ..
                } if entry_mode.is_blob() => {
                    found.push(TreeChange::Removed(relative_path(location.as_bstr())))
                }
                ChangeDetached::Modification {
                    location,
                    previous_entry_mode,
                    entry_mode,
                    ..
                } => {
                    // A file that became a link or a submodule is no longer a file, and back.
                    let path = relative_path(location.as_bstr());
                    match (previous_entry_mode.is_blob(), entry_mode.is_blob()) {
                        (true, true) => found.push(TreeChange::Modified(path)),
                        (true, false) => found.push(TreeChange::Removed(path)),
                        (false, true) => found.push(TreeChange::Added(path)),
                        (false, false) => {}
                    }
                }
                ChangeDetached::Rewrite {
                    source_location,
                    location,
                    entry_mode,
                    diff,
                    copy,
                    ..
                } if entry_mode.is_blob() => {
                    let to = relative_path(location.as_bstr());
                    if copy {
                        found.push(TreeChange::Added(to));
                    } else {
                        found.push(TreeChange::Renamed {
                            from: relative_path(source_location.as_bstr()),
                            to,
                            // Git gives no measure for a move found by its object id: the
                            // text is the same.
                            similarity: Some(diff.map_or(100, |diff| {
                                (diff.similarity * 100.0).round().clamp(0.0, 100.0) as u8
                            })),
                        });
                    }
                }
                _ => {}
            }
        }
        found.sort();
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;
    use gix::object::tree::EntryKind;

    /// Commits `HEAD`'s tree with `path` replaced by an entry of `kind` holding `text`.
    fn commit_with(repo: &Repository, path: &str, kind: EntryKind, text: &str) {
        let blob = repo.repo.write_blob(text).unwrap().detach();
        let head_tree = repo.repo.head_tree_id().unwrap().detach();
        let mut editor = repo.repo.edit_tree(head_tree).unwrap();
        editor.upsert(path, kind, blob).unwrap();
        let tree = editor.write().unwrap().detach();
        let parent = repo.repo.head_id().unwrap().detach();
        repo.repo.commit("HEAD", "change", tree, [parent]).unwrap();
    }

    #[test]
    fn a_file_that_becomes_a_link_is_removed_and_back_added() {
        let dir = tempfile::tempdir().unwrap();
        let test = TestRepo::init(dir.path());
        test.write("page.adoc", "= Page\n");
        test.commit_all("first");
        let repo = Repository::discover(dir.path()).unwrap().unwrap();
        commit_with(&repo, "page.adoc", EntryKind::Link, "other.adoc");
        commit_with(&repo, "page.adoc", EntryKind::Blob, "= Page again\n");
        let [file, link, file_again] =
            ["HEAD~2", "HEAD~1", "HEAD"].map(|spec| repo.resolve_revision(spec).unwrap());
        assert_eq!(
            repo.tree_changes(&file, &link).unwrap(),
            [TreeChange::Removed("page.adoc".into())]
        );
        assert_eq!(
            repo.tree_changes(&link, &file_again).unwrap(),
            [TreeChange::Added("page.adoc".into())]
        );
    }
}
