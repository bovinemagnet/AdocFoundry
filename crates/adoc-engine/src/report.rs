//! Reports for headless front ends: the workspace's diagnostics, and the Git status of a root,
//! in the shapes `adocfoundry-cli check` and `git-status` print as JSON, so the CLI and the MCP
//! server give the same answers.

use std::path::Path;

use adoc_core::{DiagnosticSeverity, LineIndex};
use serde::Serialize;

use crate::Engine;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CheckRange {
    pub start: usize,
    pub end: usize,
}

/// One diagnostic of [`Engine::check_report`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CheckEntry {
    /// Relative to the root, with `/` separators.
    pub path: String,
    pub range: CheckRange,
    /// One-based.
    pub line: usize,
    /// One-based, in bytes.
    pub column: usize,
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
    #[serde(skip)]
    pub level: DiagnosticSeverity,
}

impl CheckEntry {
    /// Whether this diagnostic is at least as severe as `threshold`.
    #[must_use]
    pub fn meets(&self, threshold: DiagnosticSeverity) -> bool {
        let rank = |severity| match severity {
            DiagnosticSeverity::Information => 0,
            DiagnosticSeverity::Warning => 1,
            DiagnosticSeverity::Error => 2,
        };
        rank(self.level) >= rank(threshold)
    }
}

/// `path` relative to `root` when it lies beneath it, with `/` separators.
#[must_use]
pub fn display_path(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `info`, `warning` or `error`.
#[must_use]
pub fn severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Information => "info",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Error => "error",
    }
}

impl Engine {
    /// Every diagnostic in the workspace, of files beneath `scope` when given, with paths
    /// relative to `root`.
    #[must_use]
    pub fn check_report(&self, root: &Path, scope: Option<&Path>) -> Vec<CheckEntry> {
        let mut entries = Vec::new();
        for (path, diagnostics) in self.workspace_diagnostics() {
            if scope.is_some_and(|scope| !path.starts_with(scope)) {
                continue;
            }
            let text = self
                .workspace
                .index
                .file(&path)
                .and_then(|file| file.text().map(std::borrow::Cow::into_owned))
                .or_else(|| std::fs::read_to_string(&path).ok())
                .unwrap_or_default();
            let lines = LineIndex::new(&text);
            for diagnostic in diagnostics {
                let (line, column) = lines.line_column(diagnostic.range.start);
                entries.push(CheckEntry {
                    path: display_path(&path, root),
                    range: CheckRange {
                        start: diagnostic.range.start,
                        end: diagnostic.range.end,
                    },
                    line: line + 1,
                    column: column + 1,
                    severity: severity_name(diagnostic.severity),
                    code: diagnostic.code.as_str(),
                    message: diagnostic.message,
                    level: diagnostic.severity,
                });
            }
        }
        entries
    }
}

#[cfg(feature = "git")]
pub use git_status::{ChangedFile, GitStatusReport};

#[cfg(feature = "git")]
mod git_status {
    use std::{
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    use serde::Serialize;

    use super::display_path;
    use crate::{Engine, EngineError, GitChange};

    /// The branch of the repository holding a root and its changed AsciiDoc files.
    #[derive(Clone, Debug, Eq, PartialEq, Serialize)]
    pub struct GitStatusReport {
        /// `main`, `detached @ <id>` or `no commits yet`.
        pub head: String,
        pub files: Vec<ChangedFile>,
    }

    #[derive(Clone, Debug, Eq, PartialEq, Serialize)]
    pub struct ChangedFile {
        pub path: String,
        pub index: &'static str,
        pub worktree: &'static str,
        /// Changed line runs against `HEAD`; absent for a file that is no longer there.
        pub hunks: Option<usize>,
    }

    fn change_name(change: &GitChange) -> &'static str {
        match change {
            GitChange::Unmodified => "unmodified",
            GitChange::Added => "added",
            GitChange::Modified => "modified",
            GitChange::Deleted => "deleted",
            GitChange::Renamed(_) => "renamed",
            GitChange::Untracked => "untracked",
            GitChange::Ignored => "ignored",
            GitChange::Conflicted => "conflicted",
        }
    }

    impl Engine {
        /// The branch of the repository holding `root` and its changed AsciiDoc files beneath
        /// it, with the number of changed line runs in each. Files it opens to count them are
        /// closed again; it waits at most `timeout` for the counts.
        pub fn git_status_report(
            &mut self,
            root: &Path,
            timeout: Duration,
        ) -> Result<GitStatusReport, EngineError> {
            let Some(repository) = self.git().repository_for(root).cloned() else {
                return Err(EngineError::NotFound(format!(
                    "{} is not in a Git repository",
                    root.display()
                )));
            };
            let changed: Vec<(PathBuf, GitChange, GitChange)> = self
                .git()
                .statuses()
                .filter(|(path, status)| {
                    path.starts_with(root)
                        && adoc_index::is_asciidoc_path(path)
                        && status.worktree != GitChange::Ignored
                })
                .map(|(path, status)| {
                    (
                        path.to_path_buf(),
                        status.index.clone(),
                        status.worktree.clone(),
                    )
                })
                .collect();
            let mut opened = Vec::new();
            let mut to_close = Vec::new();
            for (path, _, _) in &changed {
                if !path.is_file() {
                    opened.push(None);
                    continue;
                }
                if self.document_for_path(path).is_none() {
                    let id = self.open(path)?;
                    to_close.push(id);
                }
                opened.push(self.document_for_path(path));
            }
            let deadline = Instant::now() + timeout;
            while self.has_pending() && Instant::now() < deadline {
                self.pump();
                std::thread::sleep(Duration::from_millis(10));
            }

            let files = changed
                .iter()
                .zip(opened)
                .map(|((path, index, worktree), id)| ChangedFile {
                    path: display_path(path, root),
                    index: change_name(index),
                    worktree: change_name(worktree),
                    hunks: id.map(|id| self.git().hunks_for(id).len()),
                })
                .collect();
            for id in to_close {
                let _ = self.close(id);
            }
            Ok(GitStatusReport {
                head: repository.head.to_string(),
                files,
            })
        }
    }
}
