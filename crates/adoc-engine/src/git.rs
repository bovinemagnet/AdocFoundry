//! Git: the repository holding each workspace root, its branch, the status of its files and the
//! line changes of open documents against `HEAD`.
//!
//! Discovery, status and diffs run on worker threads and are applied by [`Engine::pump`]. A
//! workspace outside any repository, or with `[git] enabled = false`, has no Git state at all.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use adoc_git::{Change, FileStatus, Head, Hunk, Repository};
use adoc_index::normalize_path;

use crate::{
    commands::{ids, register, spec},
    workspace::WorkerMessage,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, EngineEvent,
};

/// A document's line changes are recomputed once typing pauses this long.
pub const GIT_SETTLE: Duration = Duration::from_millis(150);

/// A repository holding at least one workspace root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitRepository {
    /// The root of the working tree.
    pub workdir: PathBuf,
    /// Its `.git` directory.
    pub git_dir: PathBuf,
    pub head: Head,
    /// The workspace roots inside it.
    pub roots: Vec<PathBuf>,
}

/// What the engine knows about the workspace's repositories.
#[derive(Default)]
pub struct GitState {
    repositories: Vec<GitRepository>,
    /// By absolute path.
    status: BTreeMap<PathBuf, FileStatus>,
    hunks: BTreeMap<DocumentId, Vec<Hunk>>,
    /// Documents whose hunks are due, with the time after which they are computed.
    pending_hunks: BTreeMap<DocumentId, std::time::Instant>,
    /// Discovery runs; a result from an older one is dropped.
    generation: u64,
    /// The latest refresh started for each working tree; older results are dropped.
    refreshes: BTreeMap<PathBuf, u64>,
    next_refresh: u64,
}

impl GitState {
    /// The repositories holding the workspace's roots, one each however many roots it holds.
    #[must_use]
    pub fn repositories(&self) -> &[GitRepository] {
        &self.repositories
    }

    /// The repository whose working tree holds `path`.
    #[must_use]
    pub fn repository_for(&self, path: &Path) -> Option<&GitRepository> {
        self.repositories
            .iter()
            .filter(|repository| path.starts_with(&repository.workdir))
            .max_by_key(|repository| repository.workdir.components().count())
    }

    /// What `HEAD` is in the repository holding `root` (or any path).
    #[must_use]
    pub fn head_for(&self, root: &Path) -> Option<&Head> {
        self.repository_for(root).map(|repository| &repository.head)
    }

    /// The status of the file at `path`, when it differs from `HEAD` or is untracked or ignored.
    #[must_use]
    pub fn status_for(&self, path: &Path) -> Option<&FileStatus> {
        self.status.get(&normalize_path(path))
    }

    /// Every file with a status, by absolute path.
    pub fn statuses(&self) -> impl Iterator<Item = (&Path, &FileStatus)> {
        self.status
            .iter()
            .map(|(path, status)| (path.as_path(), status))
    }

    /// The line changes of open document `id` against `HEAD`, as of the last computation.
    #[must_use]
    pub fn hunks_for(&self, id: DocumentId) -> &[Hunk] {
        self.hunks.get(&id).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending_hunks.is_empty()
    }

    pub(crate) fn is_metadata_path(&self, path: &Path) -> bool {
        self.is_metadata(path).is_some()
    }

    /// The repository whose `.git` directory holds `path`, when `path` is one of the files that
    /// change on a commit, a branch switch or staging.
    fn is_metadata(&self, path: &Path) -> Option<&GitRepository> {
        self.repositories.iter().find(|repository| {
            let Ok(inside) = path.strip_prefix(&repository.git_dir) else {
                return false;
            };
            inside.starts_with("refs")
                || ["HEAD", "index", "packed-refs"]
                    .iter()
                    .any(|name| inside == Path::new(name))
        })
    }
}

impl Engine {
    /// The workspace's repositories, file status and the line changes of open documents.
    #[must_use]
    pub fn git(&self) -> &GitState {
        &self.git
    }

    /// Finds the repositories of the workspace's roots and their status on a worker thread,
    /// replacing what was known. Does nothing but forget when `[git] enabled` is false.
    pub(crate) fn discover_git(&mut self) {
        self.git.generation += 1;
        let had_repositories = !self.git.repositories.is_empty();
        let forgotten: Vec<PathBuf> = self
            .git
            .repositories
            .iter()
            .flat_map(|repository| repository.roots.clone())
            .collect();
        self.git.repositories.clear();
        self.git.status.clear();
        self.git.refreshes.clear();
        let had_hunks: Vec<DocumentId> = self.git.hunks.keys().copied().collect();
        self.git.hunks.clear();
        self.git.pending_hunks.clear();
        if had_repositories {
            for root in forgotten {
                self.emit(EngineEvent::GitStatusChanged { root });
            }
            for id in had_hunks {
                self.emit(EngineEvent::GitHunksChanged(id));
            }
        }
        if !self.settings.git.enabled || self.roots.is_empty() {
            return;
        }
        let roots = self.roots.clone();
        let generation = self.git.generation;
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let mut found: Vec<(GitRepository, Vec<FileStatus>)> = Vec::new();
            let mut errors = Vec::new();
            for root in roots {
                if let Some((repository, _)) = found
                    .iter_mut()
                    .find(|(repository, _)| root.starts_with(&repository.workdir))
                {
                    repository.roots.push(root);
                    continue;
                }
                match Repository::discover(&root) {
                    Ok(None) => {}
                    Ok(Some(repo)) => match snapshot(&repo) {
                        Ok((head, status)) => found.push((
                            GitRepository {
                                workdir: normalize_path(repo.workdir()),
                                git_dir: normalize_path(repo.git_dir()),
                                head,
                                roots: vec![root],
                            },
                            status,
                        )),
                        Err(error) => errors.push(error),
                    },
                    Err(error) => errors.push(error.to_string()),
                }
            }
            let _ = sender.send(WorkerMessage::GitDiscovered {
                generation,
                found,
                errors,
            });
        });
    }

    pub(crate) fn git_discovered(
        &mut self,
        generation: u64,
        found: Vec<(GitRepository, Vec<FileStatus>)>,
        errors: Vec<String>,
    ) {
        if generation != self.git.generation {
            return;
        }
        for error in errors {
            self.emit(EngineEvent::CommandFailed {
                command: ids::GIT_REFRESH,
                error,
            });
        }
        let mut watch = Vec::new();
        for (repository, status) in found {
            for root in &repository.roots {
                self.emit(EngineEvent::GitHeadChanged {
                    root: root.clone(),
                    head: repository.head.clone(),
                });
            }
            self.replace_status(&repository.workdir, status);
            watch.push(repository.git_dir.clone());
            let roots = repository.roots.clone();
            self.git.repositories.push(repository);
            for root in roots {
                self.emit(EngineEvent::GitStatusChanged { root });
            }
        }
        if let Some(watcher) = self.watcher.as_mut() {
            for git_dir in watch {
                watcher.watch_git_dir(&git_dir);
            }
        }
        self.schedule_all_hunks();
    }

    /// Re-reads the head and status of the repository at `workdir` on a worker thread.
    fn refresh_git(&mut self, workdir: PathBuf) {
        self.git.next_refresh += 1;
        let refresh = self.git.next_refresh;
        self.git.refreshes.insert(workdir.clone(), refresh);
        let generation = self.git.generation;
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let result = match Repository::discover(&workdir) {
                Ok(Some(repo)) => snapshot(&repo),
                Ok(None) => Err(format!("{} is no longer a repository", workdir.display())),
                Err(error) => Err(error.to_string()),
            };
            let _ = sender.send(WorkerMessage::GitRefreshed {
                generation,
                workdir,
                refresh,
                result,
            });
        });
    }

    pub(crate) fn git_refreshed(
        &mut self,
        generation: u64,
        workdir: &Path,
        refresh: u64,
        result: Result<(Head, Vec<FileStatus>), String>,
    ) {
        if generation != self.git.generation || self.git.refreshes.get(workdir) != Some(&refresh) {
            return;
        }
        let (head, status) = match result {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.emit(EngineEvent::CommandFailed {
                    command: ids::GIT_REFRESH,
                    error,
                });
                return;
            }
        };
        let Some(index) = self
            .git
            .repositories
            .iter()
            .position(|repository| repository.workdir == workdir)
        else {
            return;
        };
        let roots = self.git.repositories[index].roots.clone();
        if self.git.repositories[index].head != head {
            self.git.repositories[index].head = head.clone();
            for root in &roots {
                self.emit(EngineEvent::GitHeadChanged {
                    root: root.clone(),
                    head: head.clone(),
                });
            }
        }
        self.replace_status(workdir, status);
        for root in roots {
            self.emit(EngineEvent::GitStatusChanged { root });
        }
        // A commit or a branch switch moves `HEAD` under the open documents.
        self.schedule_all_hunks();
    }

    fn replace_status(&mut self, workdir: &Path, status: Vec<FileStatus>) {
        self.git.status.retain(|path, _| !path.starts_with(workdir));
        for file in status {
            let path = normalize_path(&workdir.join(&file.path));
            self.git.status.insert(path, file);
        }
    }

    /// Refreshes the repositories that changes to `paths` concern: their `.git` metadata or files
    /// in their working trees. Returns the paths that are only `.git` metadata, which nothing
    /// else should see.
    pub(crate) fn git_paths_changed(&mut self, paths: &[PathBuf]) -> BTreeSet<PathBuf> {
        let mut metadata = BTreeSet::new();
        let mut workdirs = BTreeSet::new();
        for path in paths {
            if let Some(repository) = self.git.is_metadata(path) {
                workdirs.insert(repository.workdir.clone());
                metadata.insert(path.clone());
            } else if let Some(repository) = self.git.repository_for(path) {
                if !path.starts_with(&repository.git_dir) {
                    workdirs.insert(repository.workdir.clone());
                }
            }
        }
        for workdir in workdirs {
            self.refresh_git(workdir);
        }
        metadata
    }

    /// Recomputes the hunks of open document `id` now, or once typing pauses when `typed`.
    pub(crate) fn schedule_hunks(&mut self, id: DocumentId, typed: bool) {
        if self.git.repositories.is_empty() {
            return;
        }
        let due = if typed {
            self.clock.now() + GIT_SETTLE
        } else {
            self.clock.now()
        };
        self.git.pending_hunks.insert(id, due);
    }

    fn schedule_all_hunks(&mut self) {
        let open: Vec<DocumentId> = self
            .documents
            .values()
            .filter(|document| !document.untitled)
            .map(|document| document.id)
            .collect();
        for id in open {
            self.schedule_hunks(id, false);
        }
    }

    pub(crate) fn forget_hunks(&mut self, id: DocumentId) {
        self.git.hunks.remove(&id);
        self.git.pending_hunks.remove(&id);
    }

    /// Starts the hunk computations that are due. Returns the number started.
    pub(crate) fn start_due_hunks(&mut self) -> usize {
        let now = self.clock.now();
        let due: Vec<DocumentId> = self
            .git
            .pending_hunks
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            self.git.pending_hunks.remove(id);
            let Some(document) = self.documents.get(id) else {
                continue;
            };
            let Some(repository) = self.git.repository_for(&document.path) else {
                self.set_hunks(*id, Vec::new());
                continue;
            };
            // An untracked or ignored file has nothing in `HEAD` to differ from.
            let unversioned = self.git.status_for(&document.path).is_some_and(|status| {
                matches!(status.worktree, Change::Untracked | Change::Ignored)
                    && status.index == Change::Unmodified
            });
            if unversioned {
                self.set_hunks(*id, Vec::new());
                continue;
            }
            let (path, text, version) = (
                document.path.clone(),
                document.text.to_string(),
                document.version,
            );
            let workdir = repository.workdir.clone();
            let sender = self.worker_sender.clone();
            let id = *id;
            self.workers_running += 1;
            thread::spawn(move || {
                let hunks = match Repository::discover(&workdir) {
                    Ok(Some(repo)) => repo
                        .line_diff(&path, Some(&text))
                        .map_err(|error| error.to_string()),
                    Ok(None) => Ok(Vec::new()),
                    Err(error) => Err(error.to_string()),
                };
                let _ = sender.send(WorkerMessage::GitHunks { id, version, hunks });
            });
        }
        due.len()
    }

    pub(crate) fn git_hunks(
        &mut self,
        id: DocumentId,
        version: u64,
        hunks: Result<Vec<Hunk>, String>,
    ) {
        if self.documents.get(&id).map(|document| document.version) != Some(version) {
            return; // A newer edit has scheduled its own computation.
        }
        match hunks {
            Ok(hunks) => self.set_hunks(id, hunks),
            Err(error) => self.emit(EngineEvent::CommandFailed {
                command: ids::GIT_REFRESH,
                error,
            }),
        }
    }

    fn set_hunks(&mut self, id: DocumentId, hunks: Vec<Hunk>) {
        if self.git.hunks_for(id) == hunks.as_slice() {
            return;
        }
        if hunks.is_empty() {
            self.git.hunks.remove(&id);
        } else {
            self.git.hunks.insert(id, hunks);
        }
        self.emit(EngineEvent::GitHunksChanged(id));
    }
}

/// The head and status of `repo`, with errors as text for the engine thread.
fn snapshot(repo: &Repository) -> Result<(Head, Vec<FileStatus>), String> {
    let head = repo.head().map_err(|error| error.to_string())?;
    let status = repo.status().map_err(|error| error.to_string())?;
    Ok((head, status))
}

impl Engine {
    /// The line changes of open document `id` against `HEAD` now, computed on this thread from
    /// its current text, for commands that must not act on hunks a few keystrokes old.
    pub fn hunks_now(&self, id: DocumentId) -> Result<Vec<Hunk>, EngineError> {
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        let repo = self.repository_of(&document.path)?;
        repo.line_diff(&document.path, Some(&document.text.to_string()))
            .map_err(|error| EngineError::InvalidRequest(error.to_string()))
    }

    /// The text in `HEAD` of the lines `hunk` replaces in open document `id`.
    pub fn head_text(&self, id: DocumentId, hunk: &Hunk) -> Result<String, EngineError> {
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        let repo = self.repository_of(&document.path)?;
        let head = repo
            .head_content(&document.path)
            .map_err(|error| EngineError::InvalidRequest(error.to_string()))?
            .unwrap_or_default();
        let lines = adoc_core::LineIndex::new(&head);
        let start = |line: usize| lines.offset(line, 0).unwrap_or(head.len());
        Ok(head[start(hunk.head_lines.start)..start(hunk.head_lines.end)].to_owned())
    }

    fn repository_of(&self, path: &Path) -> Result<Repository, EngineError> {
        let not_in_git =
            || EngineError::NotFound(format!("{} is not in a Git repository", path.display()));
        let workdir = &self
            .git
            .repository_for(path)
            .ok_or_else(not_in_git)?
            .workdir;
        Repository::discover(workdir)
            .map_err(|error| EngineError::InvalidRequest(error.to_string()))?
            .ok_or_else(not_in_git)
    }
}

/// The hunk at `offset`: the one whose lines hold it, or the deletion just before it.
fn hunk_at(hunks: &[Hunk], offset: usize) -> Option<&Hunk> {
    hunks
        .iter()
        .find(|hunk| hunk.range.start <= offset && offset < hunk.range.end)
        .or_else(|| hunks.iter().find(|hunk| hunk.range.start == offset))
}

pub(crate) fn register_git(registry: &mut CommandRegistry) {
    use CommandCategory::{Edit, Navigate, View, Workspace};
    use CommandContext::{ActiveDocument, Always};
    let commands: [(crate::CommandSpec, Run); 5] = [
        (
            spec(
                ids::GIT_REFRESH,
                "Refresh Git Status",
                Workspace,
                None,
                Always,
            ),
            git_refresh,
        ),
        (
            spec(
                ids::GIT_REVERT_HUNK,
                "Revert Change",
                Edit,
                None,
                ActiveDocument,
            ),
            git_revert_hunk,
        ),
        (
            spec(
                ids::GIT_NEXT_CHANGE,
                "Go to Next Change",
                Navigate,
                Some("alt-down"),
                ActiveDocument,
            ),
            |engine, args| go_to_change(engine, args, true),
        ),
        (
            spec(
                ids::GIT_PREVIOUS_CHANGE,
                "Go to Previous Change",
                Navigate,
                Some("alt-up"),
                ActiveDocument,
            ),
            |engine, args| go_to_change(engine, args, false),
        ),
        (
            spec(ids::GIT_SHOW_CHANGES, "Show Changes", View, None, Always),
            // The front end shows its Changes panel; the engine has nothing to do.
            |_, _| Ok(CommandOutcome::Done),
        ),
    ];
    for (spec, run) in commands {
        register(registry, spec, run);
    }
}

type Run = fn(&mut Engine, CommandArgs) -> Result<CommandOutcome, EngineError>;

/// The document and caret offset of a command's arguments; `Document` is the start.
fn document_offset(
    command: crate::CommandId,
    args: CommandArgs,
) -> Result<(DocumentId, Option<usize>), EngineError> {
    match args {
        CommandArgs::Offset { id, offset } => Ok((id, Some(offset))),
        CommandArgs::Document(id) => Ok((id, None)),
        _ => Err(EngineError::InvalidArguments {
            command,
            expected: "a document and an offset",
        }),
    }
}

/// Replaces the change at the offset with its text in `HEAD`, as one undo step.
fn git_revert_hunk(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let (id, offset) = document_offset(ids::GIT_REVERT_HUNK, args)?;
    let hunks = engine.hunks_now(id)?;
    let hunk = hunk_at(&hunks, offset.unwrap_or(0))
        .ok_or_else(|| EngineError::NotFound("there is no change here".to_owned()))?
        .clone();
    let head = engine.head_text(id, &hunk)?;
    let group = engine.new_group();
    engine.apply_edits(
        id,
        vec![crate::TextEdit::new(hunk.range, head)],
        crate::engine::UndoMode::Group(group),
    )?;
    Ok(CommandOutcome::Done)
}

/// The next change after the offset (or the previous one before it), wrapping round.
fn go_to_change(
    engine: &mut Engine,
    args: CommandArgs,
    forward: bool,
) -> Result<CommandOutcome, EngineError> {
    let command = if forward {
        ids::GIT_NEXT_CHANGE
    } else {
        ids::GIT_PREVIOUS_CHANGE
    };
    let (id, offset) = document_offset(command, args)?;
    let hunks = engine.hunks_now(id)?;
    let target = if forward {
        let after = offset.map_or(0, |offset| offset + 1);
        hunks
            .iter()
            .find(|hunk| hunk.range.start >= after)
            .or(hunks.first())
    } else {
        let offset = offset.unwrap_or(usize::MAX);
        hunks
            .iter()
            .rev()
            .find(|hunk| hunk.range.start < offset && !(offset < hunk.range.end))
            .or(hunks.last())
    }
    .ok_or_else(|| EngineError::NotFound("there are no changes".to_owned()))?;
    let path = engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?
        .path
        .clone();
    Ok(CommandOutcome::Location(crate::Location {
        path,
        range: adoc_core::SourceRange::new(target.range.start, target.range.start),
    }))
}

fn git_refresh(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    engine.discover_git();
    Ok(CommandOutcome::Done)
}
