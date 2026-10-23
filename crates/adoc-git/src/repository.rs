use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use gix::bstr::{BStr, ByteSlice};

use crate::{
    blame::blame_through, diff::line_hunks, BlameLine, Branch, Change, FileStatus, GitError, Head,
    Hunk,
};

/// A Git repository with a working tree.
pub struct Repository {
    repo: gix::Repository,
    workdir: PathBuf,
    git_dir: PathBuf,
}

impl Repository {
    /// Discovers the repository containing `path`, walking up its parents like
    /// `git rev-parse --show-toplevel`. `Ok(None)` when there is none, or only a bare one; an error
    /// only for a repository that exists but cannot be opened.
    ///
    /// The working tree is spelt as `path` spells it (no symbolic links resolved), so paths the
    /// caller builds from its own roots can be compared with it.
    pub fn discover(path: &Path) -> Result<Option<Self>, GitError> {
        use gix::discover::upwards::Error as NotFound;
        if let Err(error) = gix::discover::upwards(path) {
            let not_found = matches!(
                error.downcast_any_ref::<NotFound>(),
                Some(
                    NotFound::NoGitRepository { .. }
                        | NotFound::NoGitRepositoryWithinCeiling { .. }
                        | NotFound::NoGitRepositoryWithinFs { .. }
                )
            );
            return if not_found {
                Ok(None)
            } else {
                Err(GitError::git(error))
            };
        }
        let repo = gix::discover(path).map_err(GitError::git)?;
        let Some(found) = repo.workdir() else {
            return Ok(None);
        };
        let workdir = spelt_as(path, found);
        let git_dir = if repo.git_dir().starts_with(found) {
            workdir.join(
                repo.git_dir()
                    .strip_prefix(found)
                    .unwrap_or(Path::new(".git")),
            )
        } else {
            repo.git_dir().to_path_buf()
        };
        Ok(Some(Self {
            repo,
            workdir,
            git_dir,
        }))
    }

    /// As [`Repository::discover`], but reading only the repository's own configuration, not
    /// the user's or the system's.
    pub fn discover_isolated(path: &Path) -> Result<Option<Self>, GitError> {
        let Some(found) = Self::discover(path)? else {
            return Ok(None);
        };
        let repo = gix::open_opts(found.repo.git_dir(), gix::open::Options::isolated())
            .map_err(GitError::git)?;
        Ok(Some(Self { repo, ..found }))
    }

    /// The root of the working tree.
    #[must_use]
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The repository's `.git` directory, whose `HEAD`, `index` and `refs` show branch switches,
    /// staging and commits.
    #[must_use]
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn head(&self) -> Result<Head, GitError> {
        let head = self.repo.head().map_err(GitError::git)?;
        if head.is_unborn() {
            return Ok(Head::Unborn);
        }
        if let Some(name) = self.repo.head_name().map_err(GitError::git)? {
            return Ok(Head::Branch(name.shorten().to_string()));
        }
        let id = self.repo.head_id().map_err(GitError::git)?;
        Ok(Head::Detached(id.to_string()))
    }

    /// The files that differ between `HEAD`, the index and the working tree, with untracked and
    /// ignored files as `.gitignore` decides, in path order.
    pub fn status(&self) -> Result<Vec<FileStatus>, GitError> {
        use gix::status::{index_worktree::Item as Worktree, Item};
        let items = self
            .repo
            .status(gix::progress::Discard)
            .map_err(GitError::git)?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .dirwalk_options(|options| {
                options.emit_ignored(Some(gix::dir::walk::EmissionMode::Matching))
            })
            .into_iter(None)
            .map_err(GitError::git)?;
        let mut files: BTreeMap<PathBuf, FileStatus> = BTreeMap::new();
        for item in items {
            match item.map_err(GitError::git)? {
                Item::IndexWorktree(Worktree::Modification {
                    rela_path, status, ..
                }) => {
                    if let Some(change) = worktree_change(&status) {
                        entry(&mut files, rela_path.as_ref()).worktree = change;
                    }
                }
                Item::IndexWorktree(Worktree::DirectoryContents { entry: found, .. }) => {
                    let change = match found.status {
                        gix::dir::entry::Status::Untracked => Change::Untracked,
                        gix::dir::entry::Status::Ignored(_) => Change::Ignored,
                        _ => continue,
                    };
                    entry(&mut files, found.rela_path.as_ref()).worktree = change;
                }
                Item::IndexWorktree(Worktree::Rewrite {
                    source,
                    dirwalk_entry,
                    ..
                }) => {
                    let from = relative_path(source.rela_path());
                    entry(&mut files, dirwalk_entry.rela_path.as_ref()).worktree =
                        Change::Renamed(from);
                }
                Item::TreeIndex(change) => {
                    use gix::diff::index::Change as Index;
                    let (location, change) = match &change {
                        Index::Addition { location, .. } => (location, Change::Added),
                        Index::Deletion { location, .. } => (location, Change::Deleted),
                        Index::Modification { location, .. } => (location, Change::Modified),
                        Index::Rewrite {
                            source_location,
                            location,
                            ..
                        } => (location, Change::Renamed(relative_path(source_location))),
                    };
                    entry(&mut files, location.as_ref()).index = change;
                }
            }
        }
        Ok(files.into_values().collect())
    }

    /// The text of `path` in the `HEAD` commit; `None` when it is not there, there is no commit
    /// yet, or it is not UTF-8.
    pub fn head_content(&self, path: &Path) -> Result<Option<String>, GitError> {
        let relative = self.relative(path)?;
        let head = self.repo.head().map_err(GitError::git)?;
        if head.is_unborn() {
            return Ok(None);
        }
        let tree = self.repo.head_tree().map_err(GitError::git)?;
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

    /// The line changes of `path` against `HEAD`: of `current` when given, so unsaved edits are
    /// included, else of the file on disk. A file not in `HEAD` is all added lines.
    pub fn line_diff(&self, path: &Path, current: Option<&str>) -> Result<Vec<Hunk>, GitError> {
        let head = self.head_content(path)?.unwrap_or_default();
        let on_disk;
        let current = match current {
            Some(text) => text,
            None => {
                let file = self.workdir.join(self.relative(path)?);
                on_disk = match fs::read_to_string(&file) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(source) => return Err(GitError::Io { path: file, source }),
                };
                &on_disk
            }
        };
        Ok(line_hunks(&head, current))
    }

    /// The local branches, then the remote-tracking ones, each in name order.
    pub fn branches(&self) -> Result<Vec<Branch>, GitError> {
        let references = self.repo.references().map_err(GitError::git)?;
        let mut branches = Vec::new();
        for reference in references.local_branches().map_err(GitError::git)? {
            let reference = reference.map_err(GitError::git)?;
            branches.push(Branch {
                remote: None,
                name: reference.name().shorten().to_string(),
            });
        }
        for reference in references.remote_branches().map_err(GitError::git)? {
            let reference = reference.map_err(GitError::git)?;
            let short = reference.name().shorten().to_string();
            let Some((remote, name)) = short.split_once('/') else {
                continue;
            };
            if name != "HEAD" {
                branches.push(Branch {
                    remote: Some(remote.to_owned()),
                    name: name.to_owned(),
                });
            }
        }
        branches.sort();
        Ok(branches)
    }

    /// The text of `path` (relative to the working tree) at the tip of branch `branch`, a local
    /// one or `remote/name`; `None` when the file is not there or is not UTF-8. The working tree
    /// is not touched.
    pub fn file_at(&self, branch: &str, path: &Path) -> Result<Option<String>, GitError> {
        let reference = self
            .repo
            .try_find_reference(format!("refs/heads/{branch}").as_str())
            .map_err(GitError::git)?;
        let reference = match reference {
            Some(reference) => reference,
            None => self
                .repo
                .find_reference(format!("refs/remotes/{branch}").as_str())
                .map_err(GitError::git)?,
        };
        let mut reference = reference;
        let commit = reference.peel_to_commit().map_err(GitError::git)?;
        let tree = commit.tree().map_err(GitError::git)?;
        let Some(entry) = tree.lookup_entry_by_path(path).map_err(GitError::git)? else {
            return Ok(None);
        };
        if !entry.mode().is_blob() {
            return Ok(None);
        }
        let object = entry.object().map_err(GitError::git)?;
        Ok(object.data.to_str().ok().map(str::to_owned))
    }

    /// Who last changed each line of `path`: of `current` when given (lines not in `HEAD` are
    /// not committed), else of the file on disk.
    pub fn blame(&self, path: &Path, current: Option<&str>) -> Result<Vec<BlameLine>, GitError> {
        let head = self.head_blame(path)?;
        let head_text = self.head_content(path)?.unwrap_or_default();
        let on_disk;
        let current = match current {
            Some(text) => text,
            None => {
                let file = self.workdir.join(self.relative(path)?);
                on_disk = fs::read_to_string(&file).unwrap_or_default();
                &on_disk
            }
        };
        let hunks = line_hunks(&head_text, current);
        Ok(blame_through(&head, &hunks, current.lines().count()))
    }

    /// Who last changed each line of `path` as it is in `HEAD`; empty when it is not there.
    pub fn head_blame(&self, path: &Path) -> Result<Vec<BlameLine>, GitError> {
        let relative = self.relative(path)?;
        if self.head_content(&relative)?.is_none() {
            return Ok(Vec::new());
        }
        let head = self.repo.head_id().map_err(GitError::git)?.detach();
        let unix: gix::bstr::BString = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
            .into();
        let outcome = self
            .repo
            .blame_file(unix.as_ref(), head, Default::default())
            .map_err(GitError::git)?;
        let mut commits: BTreeMap<gix::ObjectId, (String, i64, String)> = BTreeMap::new();
        let mut lines = Vec::new();
        for entry in &outcome.entries {
            let id = entry.commit_id;
            if let std::collections::btree_map::Entry::Vacant(vacant) = commits.entry(id) {
                let commit = self.repo.find_commit(id).map_err(GitError::git)?;
                let author = commit.author().map_err(GitError::git)?;
                let name = author.name.to_str_lossy().into_owned();
                let time = author.time().map_or(0, |time| time.seconds);
                let summary = commit
                    .message()
                    .map(|message| message.summary().to_str_lossy().into_owned())
                    .unwrap_or_default();
                vacant.insert((name, time, summary));
            }
            let (author, time, summary) = &commits[&id];
            let short = id.to_hex_with_len(7).to_string();
            let start = entry.start_in_blamed_file as usize;
            for line in start..start + entry.len.get() as usize {
                lines.push(BlameLine {
                    line,
                    commit: Some(short.clone()),
                    author: author.clone(),
                    time: *time,
                    summary: summary.clone(),
                });
            }
        }
        lines.sort_by_key(|line| line.line);
        Ok(lines)
    }

    /// Whether branch `branch` has been pushed to `remote`: `refs/remotes/<remote>/<branch>`
    /// exists. The local branch may still be ahead.
    pub fn is_pushed(&self, remote: &str, branch: &str) -> Result<bool, GitError> {
        Ok(self
            .repo
            .try_find_reference(format!("refs/remotes/{remote}/{branch}").as_str())
            .map_err(GitError::git)?
            .is_some())
    }

    /// Stages `paths` (absolute, or relative to the working tree) as they are in the working
    /// tree, as `git add` would: new and changed files are added and missing ones removed.
    pub fn stage(&self, paths: &[&Path]) -> Result<(), GitError> {
        let mut index = self.index_file()?;
        for path in paths {
            let relative = self.relative(path)?;
            let name = unix_path(&relative);
            index.remove_entries(|_, entry_path, _| entry_path == name.as_slice());
            let file = self.workdir.join(&relative);
            let Ok(metadata) = gix::index::fs::Metadata::from_path_no_follow(&file) else {
                continue; // Gone: staged as deleted.
            };
            let bytes = fs::read(&file).map_err(|source| GitError::Io {
                path: file.clone(),
                source,
            })?;
            let id = self.repo.write_blob(bytes).map_err(GitError::git)?.detach();
            let stat = gix::index::entry::Stat::from_fs(&metadata).unwrap_or_default();
            let mode = if metadata.is_executable() {
                gix::index::entry::Mode::FILE_EXECUTABLE
            } else {
                gix::index::entry::Mode::FILE
            };
            index.dangerously_push_entry(
                stat,
                id,
                gix::index::entry::Flags::empty(),
                mode,
                name.as_ref(),
            );
        }
        index.sort_entries();
        index.write(Default::default()).map_err(GitError::git)
    }

    /// Puts `paths` back in the index as they are in `HEAD`, leaving the working tree alone, as
    /// `git restore --staged` would.
    pub fn unstage(&self, paths: &[&Path]) -> Result<(), GitError> {
        let mut index = self.index_file()?;
        let head_tree = if self.repo.head().map_err(GitError::git)?.is_unborn() {
            None
        } else {
            Some(self.repo.head_tree().map_err(GitError::git)?)
        };
        for path in paths {
            let relative = self.relative(path)?;
            let name = unix_path(&relative);
            index.remove_entries(|_, entry_path, _| entry_path == name.as_slice());
            let Some(tree) = &head_tree else {
                continue;
            };
            let Some(entry) = tree
                .lookup_entry_by_path(&relative)
                .map_err(GitError::git)?
            else {
                continue;
            };
            let mode = if entry.mode().is_executable() {
                gix::index::entry::Mode::FILE_EXECUTABLE
            } else {
                gix::index::entry::Mode::FILE
            };
            // A zeroed stat makes the next status compare the file's content.
            index.dangerously_push_entry(
                gix::index::entry::Stat::default(),
                entry.object_id(),
                gix::index::entry::Flags::empty(),
                mode,
                name.as_ref(),
            );
        }
        index.sort_entries();
        index.write(Default::default()).map_err(GitError::git)
    }

    /// The name and email commits are made with, from the usual configuration scopes.
    pub fn identity(&self) -> Result<(String, String), GitError> {
        let config = self.repo.config_snapshot();
        let name = config.string("user.name").map(|name| name.to_string());
        let email = config.string("user.email").map(|email| email.to_string());
        match (name, email) {
            (Some(name), Some(email)) if !name.is_empty() && !email.is_empty() => Ok((name, email)),
            _ => Err(GitError::NoIdentity),
        }
    }

    /// Whether the configuration asks for signed commits (`commit.gpgsign`), which this crate
    /// cannot make.
    #[must_use]
    pub fn signs_commits(&self) -> bool {
        self.repo
            .config_snapshot()
            .boolean("commit.gpgsign")
            .unwrap_or(false)
    }

    /// Commits the index on the current branch with `message`, as `git commit` would. Refuses
    /// without an identity or when nothing is staged. Returns the commit's id.
    pub fn commit(&self, message: &str) -> Result<String, GitError> {
        self.identity()?;
        let index = self.index_file()?;
        let mut editor = self
            .repo
            .edit_tree(gix::ObjectId::empty_tree(self.repo.object_hash()))
            .map_err(GitError::git)?;
        for entry in index.entries() {
            use gix::index::entry::Mode;
            use gix::object::tree::EntryKind;
            let kind = match entry.mode {
                Mode::FILE_EXECUTABLE => EntryKind::BlobExecutable,
                Mode::SYMLINK => EntryKind::Link,
                Mode::COMMIT => EntryKind::Commit,
                Mode::FILE => EntryKind::Blob,
                _ => continue,
            };
            let path = entry.path(&index).to_str_lossy().into_owned();
            editor
                .upsert(path.as_str(), kind, entry.id)
                .map_err(GitError::git)?;
        }
        let tree = editor.write().map_err(GitError::git)?.detach();
        let head = self.repo.head().map_err(GitError::git)?;
        let parents: Vec<gix::ObjectId> = if head.is_unborn() {
            Vec::new()
        } else {
            let head_tree = self.repo.head_tree_id().map_err(GitError::git)?.detach();
            if head_tree == tree {
                return Err(GitError::NothingToCommit);
            }
            vec![self.repo.head_id().map_err(GitError::git)?.detach()]
        };
        let id = self
            .repo
            .commit("HEAD", message, tree, parents)
            .map_err(GitError::git)?;
        Ok(id.to_string())
    }

    /// How many commits branch `branch` has that `remote` does not, by its remote-tracking
    /// branch; `None` when there is none, as before a first push.
    pub fn unpushed(&self, remote: &str, branch: &str) -> Result<Option<usize>, GitError> {
        let Some(mut tracking) = self
            .repo
            .try_find_reference(format!("refs/remotes/{remote}/{branch}").as_str())
            .map_err(GitError::git)?
        else {
            return Ok(None);
        };
        let pushed = tracking.peel_to_id().map_err(GitError::git)?.detach();
        let head = self.repo.head_id().map_err(GitError::git)?.detach();
        let walk = self
            .repo
            .rev_walk([head])
            .with_hidden([pushed])
            .all()
            .map_err(GitError::git)?;
        Ok(Some(walk.filter_map(Result::ok).count()))
    }

    /// The full id of the commit `HEAD` points at; `None` before the first commit.
    #[must_use]
    pub fn head_commit_id(&self) -> Option<String> {
        self.repo.head_id().ok().map(|id| id.to_string())
    }

    /// Whether branch `branch` tracks a branch on a remote (`branch.<name>.remote` is set).
    #[must_use]
    pub fn has_upstream(&self, branch: &str) -> bool {
        self.repo
            .config_snapshot()
            .string(format!("branch.{branch}.remote").as_str())
            .is_some()
    }

    /// The index as on disk, or an empty one in a repository that has none yet.
    fn index_file(&self) -> Result<gix::index::File, GitError> {
        let path = self.repo.index_path();
        if path.exists() {
            self.repo.open_index().map_err(GitError::git)
        } else {
            Ok(gix::index::File::from_state(
                gix::index::State::new(self.repo.object_hash()),
                path,
            ))
        }
    }

    /// The fetch URL of remote `name`, as configured.
    pub fn remote_url(&self, name: &str) -> Result<Option<String>, GitError> {
        let names = self.repo.remote_names();
        if !names
            .iter()
            .any(|remote| remote.as_bytes() == name.as_bytes())
        {
            return Ok(None);
        }
        let remote = self.repo.find_remote(name).map_err(GitError::git)?;
        Ok(remote
            .url(gix::remote::Direction::Fetch)
            .map(|url| url.to_bstring().to_string()))
    }

    /// `path` relative to the working tree: an absolute path inside it, or a relative one as is.
    fn relative(&self, path: &Path) -> Result<PathBuf, GitError> {
        if path.is_relative() {
            return Ok(path.to_path_buf());
        }
        path.strip_prefix(&self.workdir)
            .map(Path::to_path_buf)
            .map_err(|_| GitError::OutsideWorkdir(path.to_path_buf()))
    }
}

/// The status of `path` in `files`, added unmodified when it is not there yet.
fn entry<'a>(files: &'a mut BTreeMap<PathBuf, FileStatus>, path: &BStr) -> &'a mut FileStatus {
    let path = relative_path(path);
    files.entry(path.clone()).or_insert(FileStatus {
        path,
        index: Change::Unmodified,
        worktree: Change::Unmodified,
    })
}

fn worktree_change(
    status: &gix::status::plumbing::index_as_worktree::EntryStatus<(), gix::submodule::Status>,
) -> Option<Change> {
    use gix::status::plumbing::index_as_worktree::{Change as Worktree, EntryStatus};
    match status {
        EntryStatus::Conflict { .. } => Some(Change::Conflicted),
        EntryStatus::Change(Worktree::Removed) => Some(Change::Deleted),
        EntryStatus::Change(_) => Some(Change::Modified),
        EntryStatus::IntentToAdd => Some(Change::Added),
        EntryStatus::NeedsUpdate(_) => None,
    }
}

/// A relative platform path as Git writes it, `/`-separated.
fn unix_path(path: &Path) -> gix::bstr::BString {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
        .into()
}

/// A repository-relative path (`/`-separated) as a platform path.
fn relative_path(path: &BStr) -> PathBuf {
    path.split_str("/")
        .map(|component| component.to_str_lossy().into_owned())
        .collect()
}

/// The ancestor of `path` that is the directory `found` names, spelt as in `path`; `found` itself
/// when no ancestor is.
fn spelt_as(path: &Path, found: &Path) -> PathBuf {
    let target = fs::canonicalize(found).ok();
    let mut candidate = Some(path);
    while let Some(directory) = candidate {
        if directory == found || (target.is_some() && fs::canonicalize(directory).ok() == target) {
            return directory.to_path_buf();
        }
        candidate = directory.parent();
    }
    if found.is_relative() {
        // `gix` reports a working tree relative to the current directory when discovery
        // started from a relative path.
        return found
            .components()
            .filter(|component| *component != Component::CurDir)
            .collect();
    }
    found.to_path_buf()
}
