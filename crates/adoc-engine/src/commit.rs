//! Staging, committing and pushing a writer's changes. Everything goes through `gix` but the
//! push, which runs the `git` executable so the user's credential helper and SSH agent work
//! unchanged (and a commit, when the configuration asks for signed commits). Both run on
//! workers: signing may wait for a passphrase or a security key, so a signed commit is also
//! stopped after `[tools] commit_timeout`.

use std::{
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use adoc_git::{Change, FileStatus, Head, Repository};
use adoc_index::{is_asciidoc_path, normalize_path};

use crate::{
    commands::{ids, register, spec},
    workspace::WorkerMessage,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, EngineEvent,
};

/// The Antora family directories whose files a writer changes, whatever their type.
const FAMILY_DIRECTORIES: &[&str] = &["images", "attachments", "examples", "partials"];

impl Engine {
    /// The changed files under the workspace roots that a writer edits, in path order: AsciiDoc
    /// pages and the files of Antora's resource families, never ignored ones.
    #[must_use]
    pub fn changed_writer_files(&self) -> Vec<(PathBuf, FileStatus)> {
        self.git
            .statuses()
            .filter(|(path, status)| {
                status.worktree != Change::Ignored
                    && self
                        .roots
                        .iter()
                        .any(|root| path.strip_prefix(root).is_ok_and(is_writer_file))
            })
            .map(|(path, status)| (path.to_path_buf(), status.clone()))
            .collect()
    }

    /// The `git` executable pushes use: `[tools] git`, else the first `git` on `PATH`.
    #[must_use]
    pub fn git_executable(&self) -> Option<PathBuf> {
        let configured = &self.settings.tools.git;
        if !configured.is_empty() {
            let path = PathBuf::from(configured);
            return path.is_file().then_some(path);
        }
        let name = if cfg!(windows) { "git.exe" } else { "git" };
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|directory| directory.join(name))
                .find(|candidate| candidate.is_file())
        })
    }

    fn repository_for_paths(
        &self,
        paths: &[PathBuf],
    ) -> Result<(PathBuf, Repository), EngineError> {
        let anchor = paths
            .first()
            .cloned()
            .or_else(|| self.roots.first().cloned())
            .ok_or_else(|| EngineError::NotFound("no workspace folder is open".to_owned()))?;
        let workdir = self
            .git
            .repository_for(&anchor)
            .map(|repository| repository.workdir.clone())
            .ok_or_else(|| {
                EngineError::NotFound(format!("{} is not in a Git repository", anchor.display()))
            })?;
        let repo = open(&workdir)?;
        Ok((workdir, repo))
    }

    fn stage_paths(&mut self, paths: &[PathBuf], stage: bool) -> Result<(), EngineError> {
        if paths.is_empty() {
            return Ok(());
        }
        let (workdir, repo) = self.repository_for_paths(paths)?;
        let borrowed: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let result = if stage {
            repo.stage(&borrowed)
        } else {
            repo.unstage(&borrowed)
        };
        result.map_err(git_error)?;
        self.refresh_git(workdir);
        Ok(())
    }
}

fn open(workdir: &Path) -> Result<Repository, EngineError> {
    Repository::discover(workdir)
        .map_err(git_error)?
        .ok_or_else(|| EngineError::NotFound(format!("{} is not a repository", workdir.display())))
}

fn git_error(error: adoc_git::GitError) -> EngineError {
    EngineError::Refused(error.to_string())
}

/// Whether `path`, relative to its workspace root, is a page or in a resource family.
fn is_writer_file(path: &Path) -> bool {
    is_asciidoc_path(path)
        || path.components().any(|component| {
            FAMILY_DIRECTORIES
                .iter()
                .any(|family| component.as_os_str() == *family)
        })
}

/// The paths of a command's arguments: `Path`, or `Custom { "paths": [...] }`.
fn paths_of(command: crate::CommandId, args: &CommandArgs) -> Result<Vec<PathBuf>, EngineError> {
    match args {
        CommandArgs::Path(path) => Ok(vec![normalize_path(path)]),
        CommandArgs::Custom(value) => Ok(value
            .get("paths")
            .and_then(serde_json::Value::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(|path| normalize_path(Path::new(path)))
                    .collect()
            })
            .unwrap_or_default()),
        _ => Err(EngineError::InvalidArguments {
            command,
            expected: "a path or a list of paths",
        }),
    }
}

pub(crate) fn register_commit(registry: &mut CommandRegistry) {
    use CommandCategory::{Document, Workspace};
    use CommandContext::{ActiveDocument, Always};
    type Run = fn(&mut Engine, CommandArgs) -> Result<CommandOutcome, EngineError>;
    let commands: [(crate::CommandSpec, Run); 6] = [
        (
            spec(ids::GIT_STAGE, "Stage", Workspace, None, Always),
            |engine, args| {
                let paths = paths_of(ids::GIT_STAGE, &args)?;
                engine.stage_paths(&paths, true)?;
                Ok(CommandOutcome::Done)
            },
        ),
        (
            spec(ids::GIT_UNSTAGE, "Unstage", Workspace, None, Always),
            |engine, args| {
                let paths = paths_of(ids::GIT_UNSTAGE, &args)?;
                engine.stage_paths(&paths, false)?;
                Ok(CommandOutcome::Done)
            },
        ),
        (
            spec(
                ids::GIT_STAGE_ALL_ADOC,
                "Stage All Changes",
                Workspace,
                None,
                Always,
            ),
            |engine, _| {
                let paths: Vec<PathBuf> = engine
                    .changed_writer_files()
                    .into_iter()
                    .map(|(path, _)| path)
                    .collect();
                engine.stage_paths(&paths, true)?;
                Ok(CommandOutcome::Done)
            },
        ),
        (
            spec(ids::GIT_COMMIT, "Commit", Workspace, None, Always),
            git_commit,
        ),
        (
            spec(ids::GIT_PUSH, "Push", Workspace, None, Always),
            git_push,
        ),
        (
            spec(
                ids::GIT_SAVE_AND_COMMIT,
                "Save and Commit",
                Document,
                None,
                ActiveDocument,
            ),
            save_and_commit,
        ),
    ];
    for (spec, run) in commands {
        register(registry, spec, run);
    }
}

/// Commits the index with `Custom { "message", "paths"? }`. With paths, any of them open with
/// unsaved edits is saved first, and they are staged; the outcome lists what was saved.
fn git_commit(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let message = match &args {
        CommandArgs::Custom(value) => value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_owned),
        _ => None,
    }
    .ok_or(EngineError::InvalidArguments {
        command: ids::GIT_COMMIT,
        expected: "a message, and optionally the paths to commit",
    })?;
    if engine.git.committing {
        return Err(EngineError::Refused(
            "a signed commit is still being made".to_owned(),
        ));
    }
    let paths = paths_of(ids::GIT_COMMIT, &args)?;
    let mut saved = Vec::new();
    for path in &paths {
        let Some(id) = engine.document_for_path(path) else {
            continue;
        };
        if engine.document(id).is_some_and(|document| document.dirty) {
            engine.save(id)?;
            saved.push(path.clone());
        }
    }
    let (workdir, repo) = engine.repository_for_paths(&paths)?;
    if !paths.is_empty() {
        let borrowed: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        repo.stage(&borrowed).map_err(git_error)?;
    }
    if repo.signs_commits() {
        // `gix` cannot sign: let the user's Git do it, with their key, on a worker, as it may
        // wait for a passphrase or a touch.
        let git = engine.git_executable().ok_or_else(|| {
            EngineError::Refused(
                "commits are signed (commit.gpgsign) and need the git executable: set [tools] git"
                    .to_owned(),
            )
        })?;
        let timeout = Duration::from_secs(engine.settings.tools.commit_timeout);
        engine.git.committing = true;
        engine.workers_running += 1;
        let sender = engine.worker_sender.clone();
        let (failed, unsaved) = (workdir.clone(), saved.clone());
        let crashed = move |error| WorkerMessage::Committed {
            workdir: failed,
            saved: unsaved,
            result: Err(format!("git commit crashed: {error}")),
        };
        let started = saved.clone();
        crate::worker::spawn(sender, crashed, move |sender| {
            let result = signed_commit(&git, &workdir, &message, timeout);
            let _ = sender.send(WorkerMessage::Committed {
                workdir,
                saved,
                result,
            });
        });
        return Ok(CommandOutcome::CommitStarted { saved: started });
    }
    let commit = repo.commit(&message).map_err(git_error)?;
    engine.refresh_git(workdir);
    Ok(CommandOutcome::Committed { commit, saved })
}

/// How often a signed commit's `git` is checked on.
const POLL: Duration = Duration::from_millis(20);

/// The most of a signed commit's standard error kept for its message.
const MAX_ERROR: u64 = 64 * 1024;

/// Makes a signed commit of the index in `workdir` with `git commit`, giving the new commit's
/// id. `git` has no terminal to prompt on: its standard input is empty, Git's own prompts are
/// off, and it runs in a process group of its own, so a signing program that reads the
/// terminal is stopped rather than answered. A prompt shown in a window of its own (pinentry,
/// a security key's touch) can still be answered. Past `timeout`, `git` and everything it
/// started are stopped.
fn signed_commit(
    git: &Path,
    workdir: &Path,
    message: &str,
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new(git);
    command
        .current_dir(workdir)
        .args(["commit", "--quiet", "--message", message])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    adoc_diagrams::process::in_own_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", git.display()))?;
    // Read on a thread, so a hook that writes a lot cannot stall it; not waited for after a
    // stop, in case a process outside the group still holds the pipe.
    let (sender, errors) = mpsc::channel();
    if let Some(pipe) = child.stderr.take() {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.take(MAX_ERROR).read_to_end(&mut bytes);
            let _ = sender.send(String::from_utf8_lossy(&bytes).trim().to_owned());
        });
    }
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            Ok(None) => {
                adoc_diagrams::process::stop(&mut child);
                return Err(format!(
                    "git commit was stopped after {} s ([tools] commit_timeout): signing the \
                     commit may be waiting for a passphrase or a security key that cannot be \
                     asked for here",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                adoc_diagrams::process::stop(&mut child);
                return Err(format!("git commit could not be waited for: {error}"));
            }
        }
    };
    if status.success() {
        Ok(open(workdir)
            .ok()
            .and_then(|repo| repo.head_commit_id())
            .unwrap_or_default())
    } else {
        let error = errors
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default();
        Err(if error.is_empty() {
            format!("git commit failed ({status})")
        } else {
            error
        })
    }
}

/// Saves the document, stages it and offers a commit message for it.
fn save_and_commit(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::GIT_SAVE_AND_COMMIT,
            expected: "a document",
        });
    };
    engine.save(id)?;
    let document = engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?;
    let path = document.path.clone();
    let title = document
        .parsed()
        .title
        .as_ref()
        .map(|title| title.text.clone())
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    engine.stage_paths(std::slice::from_ref(&path), true)?;
    Ok(CommandOutcome::CommitDraft {
        message: format!("docs: update {title}"),
        paths: vec![path],
    })
}

/// Pushes the current branch to `[git] remote` with the `git` executable, on a worker thread.
/// Never forces and never sets an upstream.
fn git_push(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let git = engine.git_executable().ok_or_else(|| {
        EngineError::NotFound(
            "pushing needs the git executable: install Git or set [tools] git".to_owned(),
        )
    })?;
    let (workdir, repo) = engine.repository_for_paths(&[])?;
    let Head::Branch(branch) = repo.head().map_err(git_error)? else {
        return Err(EngineError::Refused(
            "HEAD is not on a branch: there is nothing to push".to_owned(),
        ));
    };
    let remote = engine.settings.git.remote.clone();
    if !repo.has_upstream(&branch) {
        return Err(EngineError::Refused(format!(
            "branch {branch} has no upstream: run `git push --set-upstream {remote} {branch}` once"
        )));
    }
    let sender = engine.worker_sender.clone();
    engine.workers_running += 1;
    let pushing = workdir.clone();
    let crashed = move |error| WorkerMessage::Pushed {
        workdir: pushing,
        result: Err(format!("git push crashed: {error}")),
    };
    crate::worker::spawn(sender, crashed, move |sender| {
        let result = run_git(&git, &workdir, &["push", &remote, &branch])
            .map(|()| format!("Pushed {branch} to {remote}"))
            .map_err(|error| error.to_string());
        let _ = sender.send(WorkerMessage::Pushed { workdir, result });
    });
    Ok(CommandOutcome::Done)
}

impl Engine {
    pub(crate) fn committed(
        &mut self,
        workdir: PathBuf,
        saved: Vec<PathBuf>,
        result: Result<String, String>,
    ) {
        self.git.committing = false;
        match result {
            Ok(commit) => self.emit(EngineEvent::GitCommitted { commit, saved }),
            Err(error) => self.emit(EngineEvent::CommandFailed {
                command: ids::GIT_COMMIT,
                error,
            }),
        }
        self.refresh_git(workdir);
    }

    pub(crate) fn pushed(&mut self, workdir: PathBuf, result: Result<String, String>) {
        match result {
            Ok(message) => self.emit(EngineEvent::GitPushed { message }),
            Err(error) => self.emit(EngineEvent::CommandFailed {
                command: ids::GIT_PUSH,
                error,
            }),
        }
        self.refresh_git(workdir);
    }
}

/// Runs `git` in `workdir`, its standard error the error when it fails.
fn run_git(git: &Path, workdir: &Path, arguments: &[&str]) -> Result<(), EngineError> {
    let output = Command::new(git)
        .current_dir(workdir)
        .args(arguments)
        .output()
        .map_err(|error| EngineError::io(git, error))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(EngineError::Refused(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}
