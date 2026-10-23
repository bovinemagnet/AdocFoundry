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
