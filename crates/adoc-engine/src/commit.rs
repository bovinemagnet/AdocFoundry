//! Staging, committing and pushing a writer's changes. Everything goes through `gix` but the
//! push, which runs the `git` executable so the user's credential helper and SSH agent work
//! unchanged (and a commit, when the configuration asks for signed commits).

use std::{
    path::{Path, PathBuf},
    process::Command,
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
    let commit = if repo.signs_commits() {
        // `gix` cannot sign: let the user's Git do it, with their key.
        let git = engine.git_executable().ok_or_else(|| {
            EngineError::Refused(
                "commits are signed (commit.gpgsign) and need the git executable: set [tools] git"
                    .to_owned(),
            )
        })?;
        run_git(
            &git,
            &workdir,
            &["commit", "--quiet", "--message", &message],
        )?;
        open(&workdir)?.head_commit_id().unwrap_or_default()
    } else {
        repo.commit(&message).map_err(git_error)?
    };
    engine.refresh_git(workdir);
    Ok(CommandOutcome::Committed { commit, saved })
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
