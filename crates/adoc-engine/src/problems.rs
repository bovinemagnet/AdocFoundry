//! The workspace's problems, for a Problems panel: every file's diagnostics with the line and
//! column they start at, so a front end shows them without diagnosing or reading anything.
//!
//! A change to the whole workspace (indexing, a component descriptor, files changed on disk)
//! diagnoses every file again, on a worker, over a copy of the workspace; a newer change drops
//! the result of an older one. An edit re-diagnoses only the files it can change
//! ([`adoc_lang::diagnosed_with`]), when the engine is pumped. Either way
//! [`EngineEvent::ProblemsChanged`] follows. Only a front end that calls
//! [`Engine::enable_problems`] (the app) pays for any of it.
//!
//! While the quality audit is kept too, its `quality.*` findings are problems as well, from the
//! kept audit under the `[quality]` policy, so an excluded finding or a rule turned off is not
//! listed (`src/quality.rs` gives them). The existing diagnostics the audit aggregates are
//! already problems and are not listed again. A [`ProblemFilter`] picks problems by severity,
//! code or rule, and component version and module.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use adoc_audit::FindingCode;
use adoc_core::{Diagnostic, DiagnosticSeverity, FileAccess, LineIndex, SourceRange};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;

use crate::{workspace::WorkerMessage, Engine, EngineEvent};

/// One problem of the workspace, where it is: a diagnostic, or a finding of the quality audit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Problem {
    /// Where it is; the file's start for a problem about the whole file.
    pub range: SourceRange,
    /// The line the problem starts on, counted from 1.
    pub line: usize,
    /// The character on that line it starts at, counted from 1.
    pub column: usize,
    pub severity: DiagnosticSeverity,
    /// What reported it: an existing diagnostic, or one of the audit's `quality.*` rules.
    pub code: FindingCode,
    pub message: String,
    /// It is about the whole file (an unlisted page, a missing title) rather than a place in it,
    /// so its line and column say nothing.
    pub whole_file: bool,
}

/// The component version and module a file belongs to, for filtering problems.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProblemPlace {
    pub component: String,
    /// `None` for a versionless component.
    pub version: Option<String>,
    /// `None` for a file in no module, such as an `antora.yml`.
    pub module: Option<String>,
}

/// Which problems to show; each field left `None` lets any through.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProblemFilter {
    pub severity: Option<DiagnosticSeverity>,
    pub code: Option<FindingCode>,
    pub component: Option<String>,
    /// The component's version: `Some(None)` for a versionless component.
    pub version: Option<Option<String>>,
    pub module: Option<String>,
}

impl ProblemFilter {
    /// Whether it lets every problem through.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether it lets through `problem` of a file at `place`. A filter by component, version or
    /// module stops every problem of a file in no component.
    #[must_use]
    pub fn matches(&self, place: Option<&ProblemPlace>, problem: &Problem) -> bool {
        if self
            .severity
            .is_some_and(|severity| severity != problem.severity)
            || self.code.is_some_and(|code| code != problem.code)
        {
            return false;
        }
        if self.component.is_none() && self.version.is_none() && self.module.is_none() {
            return true;
        }
        let Some(place) = place else {
            return false;
        };
        self.component
            .as_ref()
            .is_none_or(|component| *component == place.component)
            && self
                .version
                .as_ref()
                .is_none_or(|version| *version == place.version)
            && self
                .module
                .as_ref()
                .is_none_or(|module| place.module.as_ref() == Some(module))
    }
}

#[derive(Debug, Default)]
pub(crate) struct ProblemsState {
    enabled: bool,
    /// Each file with problems, and its problems in order: its diagnostics and its quality
    /// findings together.
    by_path: BTreeMap<PathBuf, Vec<Problem>>,
    /// Each file's diagnostics.
    diagnostics: BTreeMap<PathBuf, Vec<Problem>>,
    /// Each file's `quality.*` findings, from the kept audit.
    quality: BTreeMap<PathBuf, Vec<Problem>>,
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
        self.refresh_quality_problems(None);
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

    /// The problems `filter` lets through, by file, each file's in order; files with none left
    /// out.
    #[must_use]
    pub fn filtered_problems(&self, filter: &ProblemFilter) -> Vec<(&Path, Vec<&Problem>)> {
        let by_place =
            filter.component.is_some() || filter.version.is_some() || filter.module.is_some();
        self.problems
            .by_path
            .iter()
            .filter_map(|(path, problems)| {
                let place = if by_place {
                    self.problem_place(path)
                } else {
                    None
                };
                let shown: Vec<&Problem> = problems
                    .iter()
                    .filter(|problem| filter.matches(place.as_ref(), problem))
                    .collect();
                (!shown.is_empty()).then_some((path.as_path(), shown))
            })
            .collect()
    }

    /// The component version and module the file at `path` belongs to: a resource's or a module
    /// file's, or for a component descriptor its component version's, in no module.
    #[must_use]
    pub fn problem_place(&self, path: &Path) -> Option<ProblemPlace> {
        let catalog = &self.workspace.antora;
        if let Some(context) = catalog.context_for_path(path) {
            return Some(ProblemPlace {
                component: context.component,
                version: context.version,
                module: Some(context.module),
            });
        }
        let path = normalize_path(path);
        catalog
            .components()
            .find(|descriptor| normalize_path(&descriptor.root.join("antora.yml")) == path)
            .map(|descriptor| ProblemPlace {
                component: descriptor.name.clone(),
                version: descriptor.version.clone(),
                module: None,
            })
    }

    /// Whether the problems are kept.
    pub(crate) fn problems_kept(&self) -> bool {
        self.problems.enabled
    }

    /// Replaces the quality findings listed of the file at `path` with `problems`, and reports
    /// whether they changed.
    pub(crate) fn set_quality_problems_of(&mut self, path: &Path, problems: Vec<Problem>) -> bool {
        let state = &mut self.problems;
        if state
            .quality
            .get(path)
            .map_or(problems.is_empty(), |old| *old == problems)
        {
            return false;
        }
        if problems.is_empty() {
            state.quality.remove(path);
        } else {
            state.quality.insert(path.to_path_buf(), problems);
        }
        state.merge(path);
        true
    }

    /// Replaces the quality findings listed with `quality`, by file, and reports whether the
    /// problems changed.
    pub(crate) fn set_quality_problems(
        &mut self,
        mut quality: BTreeMap<PathBuf, Vec<Problem>>,
    ) -> bool {
        let state = &mut self.problems;
        let paths: BTreeSet<PathBuf> = state
            .quality
            .keys()
            .chain(quality.keys())
            .cloned()
            .collect();
        let mut changed = false;
        for path in paths {
            let fresh = quality.remove(&path).filter(|fresh| !fresh.is_empty());
            if state.quality.get(&path) == fresh.as_ref() {
                continue;
            }
            match fresh {
                Some(fresh) => state.quality.insert(path.clone(), fresh),
                None => state.quality.remove(&path),
            };
            state.merge(&path);
            changed = true;
        }
        changed
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
            let crashed = move |error| WorkerMessage::Crashed {
                job: crate::worker::Crashed::Problems { generation },
                error,
            };
            crate::worker::spawn(sender, crashed, move |sender| {
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
                self.problems.diagnostics.remove(path);
            } else {
                self.problems.diagnostics.insert(path.clone(), found);
            }
            self.problems.merge(path);
        }
        self.emit(EngineEvent::ProblemsChanged);
        1
    }

    /// Pass `generation` crashed: the problems found before stay, and the next change
    /// starts another.
    pub(crate) fn problems_crashed(&mut self, generation: u64) {
        if self.problems.running == Some(generation) {
            self.problems.running = None;
        }
    }

    pub(crate) fn problems_computed(
        &mut self,
        generation: u64,
        problems: BTreeMap<PathBuf, Vec<Problem>>,
    ) {
        if self.problems.running != Some(generation) {
            return;
        }
        let state = &mut self.problems;
        state.running = None;
        state.diagnostics = problems;
        let paths: BTreeSet<PathBuf> = state
            .diagnostics
            .keys()
            .chain(state.quality.keys())
            .chain(state.by_path.keys())
            .cloned()
            .collect();
        for path in &paths {
            state.merge(path);
        }
        self.emit(EngineEvent::ProblemsChanged);
    }
}

impl ProblemsState {
    /// Lists the diagnostics and quality findings of `path` together: those about the whole
    /// file first, then in the order of their places, a diagnostic before a finding at the same
    /// place.
    fn merge(&mut self, path: &Path) {
        let mut problems: Vec<Problem> = self
            .diagnostics
            .get(path)
            .into_iter()
            .chain(self.quality.get(path))
            .flatten()
            .cloned()
            .collect();
        problems
            .sort_by_key(|problem| (!problem.whole_file, problem.range.start, problem.range.end));
        if problems.is_empty() {
            self.by_path.remove(path);
        } else {
            self.by_path.insert(path.to_path_buf(), problems);
        }
    }
}

/// The 1-based line and character column of `offset` in `text`, whose lines are `lines`.
pub(crate) fn line_and_column(lines: &LineIndex, text: &str, offset: usize) -> (usize, usize) {
    let (line, byte) = lines.line_column(offset);
    let start = offset.min(text.len()) - byte;
    let column = text
        .get(start..start + byte)
        .map_or(byte, |before| before.chars().count());
    (line + 1, column + 1)
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
            let (line, column) = line_and_column(&lines, &text, diagnostic.range.start);
            Problem {
                range: diagnostic.range,
                line,
                column,
                severity: diagnostic.severity,
                code: FindingCode::Diagnostic(diagnostic.code),
                message: diagnostic.message,
                whole_file: false,
            }
        })
        .collect()
}
