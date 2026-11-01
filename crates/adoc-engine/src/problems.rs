//! The workspace's problems, for a Problems panel: every file's diagnostics with the line and
//! column they start at, so a front end shows them without diagnosing or reading anything.
//!
//! A change to the whole workspace (indexing, a component descriptor, files changed on disk)
//! diagnoses every file again, on a worker, over a copy of the workspace; a newer change drops
//! the result of an older one. An edit re-diagnoses only the files it can change
//! ([`adoc_lang::diagnosed_with`]), when the engine is pumped. Either way
//! [`EngineEvent::ProblemsChanged`] follows. Only a front end that calls
//! [`Engine::enable_problems`] (the app) pays for any of it.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    thread,
};

use adoc_core::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, FileAccess, LineIndex, SourceRange,
};
use adoc_lang::WorkspaceState;

use crate::{workspace::WorkerMessage, Engine, EngineEvent};

/// One diagnostic of the workspace, where it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Problem {
    pub range: SourceRange,
    /// The line the problem starts on, counted from 1.
    pub line: usize,
    /// The character on that line it starts at, counted from 1.
    pub column: usize,
    pub severity: DiagnosticSeverity,
    pub code: DiagnosticCode,
    pub message: String,
}

#[derive(Debug, Default)]
pub(crate) struct ProblemsState {
    enabled: bool,
    /// Each file with problems, and its problems in order.
    by_path: BTreeMap<PathBuf, Vec<Problem>>,
    /// The whole workspace is to be diagnosed again.
    full_due: bool,
    /// The whole-workspace pass running, by generation; a result of any other is dropped.
    running: Option<u64>,
    generation: u64,
    /// Files an edit can have changed the problems of, re-diagnosed at the next pump.
    dirty: BTreeSet<PathBuf>,
}

impl Engine {
    /// Keeps the workspace's problems ([`Engine::problems`]) for a front end that shows them.
    pub fn enable_problems(&mut self) {
        self.problems.enabled = true;
        self.problems.full_due = true;
    }

    /// Each file with problems, by path, and its problems in order. Empty until the first
    /// [`EngineEvent::ProblemsChanged`].
    #[must_use]
    pub fn problems(&self) -> &BTreeMap<PathBuf, Vec<Problem>> {
        &self.problems.by_path
    }

    /// How many problems the workspace has.
    #[must_use]
    pub fn problem_count(&self) -> usize {
        self.problems.by_path.values().map(Vec::len).sum()
    }

    pub(crate) fn has_due_problems(&self) -> bool {
        self.problems.enabled
            && (self.problems.full_due
                || self.problems.running.is_some()
                || !self.problems.dirty.is_empty())
    }

    /// The whole workspace is to be diagnosed again.
    pub(crate) fn problems_changed_everywhere(&mut self) {
        if self.problems.enabled {
            self.problems.full_due = true;
        }
    }

    /// The problems of `paths` may have changed.
    pub(crate) fn problems_changed_in(&mut self, paths: &BTreeSet<PathBuf>) {
        if self.problems.enabled {
            self.problems.dirty.extend(paths.iter().cloned());
        }
    }

    /// Starts a whole-workspace pass that is due, or re-diagnoses the files edits changed.
    /// Edits made while a pass runs wait for its result, which is older than they are.
    pub(crate) fn update_due_problems(&mut self) -> usize {
        if !self.problems.enabled || self.problems.running.is_some() {
            return 0;
        }
        if self.problems.full_due {
            self.problems.full_due = false;
            self.problems.dirty.clear();
            self.problems.generation += 1;
            let generation = self.problems.generation;
            self.problems.running = Some(generation);
            let state = self.workspace.clone();
            let access = self.file_access.clone();
            let sender = self.worker_sender.clone();
            self.workers_running += 1;
            thread::spawn(move || {
                let problems = workspace_problems(&state, &access);
                let _ = sender.send(WorkerMessage::ProblemsComputed {
                    generation,
                    problems,
                });
            });
            return 1;
        }
        if self.problems.dirty.is_empty() {
            return 0;
        }
        let dirty = std::mem::take(&mut self.problems.dirty);
        for path in &dirty {
            let found = problems_of(&self.workspace, &self.file_access, path);
            if found.is_empty() {
                self.problems.by_path.remove(path);
            } else {
                self.problems.by_path.insert(path.clone(), found);
            }
        }
        self.emit(EngineEvent::ProblemsChanged);
        1
    }

    pub(crate) fn problems_computed(
        &mut self,
        generation: u64,
        problems: BTreeMap<PathBuf, Vec<Problem>>,
    ) {
        if self.problems.running != Some(generation) {
            return;
        }
        self.problems.running = None;
        self.problems.by_path = problems;
        self.emit(EngineEvent::ProblemsChanged);
    }
}

/// Every file's problems.
fn workspace_problems(ws: &WorkspaceState, access: &FileAccess) -> BTreeMap<PathBuf, Vec<Problem>> {
    adoc_lang::workspace_diagnostics(ws)
        .into_iter()
        .map(|(path, diagnostics)| {
            let problems = located(ws, access, &path, diagnostics);
            (path, problems)
        })
        .collect()
}

/// The problems of the file at `path`.
fn problems_of(ws: &WorkspaceState, access: &FileAccess, path: &Path) -> Vec<Problem> {
    let mut diagnostics = adoc_lang::diagnostics(ws, path);
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    located(ws, access, path, diagnostics)
}

/// `diagnostics` of `path` with their lines and columns, from the index's text of the file or,
/// for a component descriptor, the file.
fn located(
    ws: &WorkspaceState,
    access: &FileAccess,
    path: &Path,
    diagnostics: Vec<Diagnostic>,
) -> Vec<Problem> {
    if diagnostics.is_empty() {
        return Vec::new();
    }
    let text = match ws.index.file(path).and_then(adoc_index::FileEntry::text) {
        Some(text) => text.into_owned(),
        None => access.read_to_string(path).unwrap_or_default(),
    };
    let lines = LineIndex::new(&text);
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            let (line, byte) = lines.line_column(diagnostic.range.start);
            let start = diagnostic.range.start.min(text.len()) - byte;
            let column = text
                .get(start..start + byte)
                .map_or(byte, |before| before.chars().count());
            Problem {
                range: diagnostic.range,
                line: line + 1,
                column: column + 1,
                severity: diagnostic.severity,
                code: diagnostic.code,
                message: diagnostic.message,
            }
        })
        .collect()
}
