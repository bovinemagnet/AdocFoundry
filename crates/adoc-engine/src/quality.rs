//! The documentation quality audit, kept current for a front end that asks for it.
//!
//! Nothing is kept until a front end calls [`Engine::enable_audit`] or the settings turn
//! `[quality] enabled` on, so a front end that never asks (the language server, by default)
//! does no audit work. Then the first build runs on a worker over a copy of the workspace,
//! keeping each file's part and each component version's part (`adoc_audit::audit_file`,
//! `audit_version`). After that the audit follows the problems: the files an edit can change
//! (`adoc_lang::diagnosed_with`) are audited again at the next pump, with the version of a
//! navigation file among them; a change to the whole workspace builds it all again on a worker;
//! and a new catalog revision audits every version's navigation again, which reads only the
//! navigation files. [`Engine::audit_report`] puts the parts in a scope together under the
//! current `[quality]` policy, the same way [`Engine::audit_now`] does for parts it computes
//! there and then, so the kept report and a fresh one always agree.
//!
//! While the problems are kept too, the kept audit's `quality.*` findings are problems, each
//! placed by the line and column its part was audited at, which are worked out from the same
//! text when the part is (on the worker for a whole build), so listing them reads no file.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
};

use adoc_audit::{AuditInput, AuditReport, AuditScope, FileAudit, FindingCode, VersionAudit};
use adoc_core::{DiagnosticSeverity, FileAccess, LineIndex, SourceRange};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;

use crate::{problems::line_and_column, workspace::WorkerMessage, Engine, EngineEvent, Problem};

/// A component version, as the audit keys its part.
type VersionKey = (String, Option<String>);

/// The 1-based line and column of each range a file's `quality.*` findings name.
type Positions = BTreeMap<SourceRange, (usize, usize)>;

/// The parts a whole build found.
pub(crate) struct AuditParts {
    files: BTreeMap<PathBuf, FileAudit>,
    positions: BTreeMap<PathBuf, Positions>,
    versions: BTreeMap<VersionKey, VersionAudit>,
    /// The catalog revision the versions were audited at.
    catalog: u64,
}

#[derive(Debug, Default)]
pub(crate) struct QualityState {
    /// A front end asked for the audit.
    enabled: bool,
    /// Each file's part, for the files with something to report.
    files: BTreeMap<PathBuf, FileAudit>,
    /// Where in each file its part's `quality.*` findings are, for the problems.
    positions: BTreeMap<PathBuf, Positions>,
    versions: BTreeMap<VersionKey, VersionAudit>,
    /// The catalog revision `versions` belong to; another means every version is audited again.
    catalog: u64,
    /// A whole build has arrived.
    built: bool,
    /// How many whole builds have arrived.
    builds: u64,
    /// A whole build has been started, so the first one is not started again after a crash.
    started: bool,
    /// The whole workspace is to be audited again.
    full_due: bool,
    /// The whole build running, by generation; a result of any other is dropped.
    running: Option<u64>,
    generation: u64,
    /// Files whose parts an edit can have changed, audited again at the next pump.
    dirty: BTreeSet<PathBuf>,
    /// Navigation files whose text changed, whose versions are audited again at the next pump.
    navigation: BTreeSet<PathBuf>,
}

impl Engine {
    /// Keeps the quality audit ([`Engine::audit_report`]) for a front end that shows it.
    pub fn enable_audit(&mut self) {
        self.quality.enabled = true;
    }

    /// Run Audit: keeps the audit, as [`Engine::enable_audit`] does, and audits the whole
    /// workspace again if it was kept already.
    pub fn run_audit(&mut self) {
        self.quality.enabled = true;
        if self.quality.started {
            self.quality.full_due = true;
        }
    }

    /// Whether the audit is kept: asked for, or turned on by `[quality] enabled`.
    #[must_use]
    pub fn audit_enabled(&self) -> bool {
        self.audit_kept()
    }

    /// Whether the kept audit is older than the workspace: a build or a change waits to be
    /// audited. `false` before the first build, which has nothing to be older than.
    #[must_use]
    pub fn audit_stale(&self) -> bool {
        self.quality.built && self.has_due_audit()
    }

    /// The kept audit of `scope`, under the current `[quality]` policy; `None` until the first
    /// build arrives ([`EngineEvent::AuditChanged`]).
    #[must_use]
    pub fn audit_report(&self, scope: &AuditScope) -> Option<AuditReport> {
        if !self.quality.built {
            return None;
        }
        let read = self.audit_reader();
        let input = AuditInput {
            workspace: &self.workspace,
            roots: &self.roots,
            read: &read,
        };
        let files = self
            .quality
            .files
            .iter()
            .map(|(path, part)| (path.as_path(), part));
        let mut builder =
            adoc_audit::assemble(&input, scope.clone(), files, self.quality.versions.values());
        self.add_audit_gaps(scope, &mut builder);
        Some(builder.finish(&self.settings.quality.policy))
    }

    /// An audit of `scope` made here and now from the engine's state, open buffers included,
    /// under the current `[quality]` policy. It blocks for as long as the scope takes, so it is
    /// for front ends with no event loop, such as the command line.
    #[must_use]
    pub fn audit_now(&self, scope: AuditScope) -> AuditReport {
        self.audit_now_with(scope, &self.settings.quality.policy)
    }

    /// [`Engine::audit_now`] under `policy` rather than the settings', for a front end that
    /// overrides part of it, as the command line's `--fail-on` does.
    #[must_use]
    pub fn audit_now_with(&self, scope: AuditScope, policy: &adoc_audit::Policy) -> AuditReport {
        let read = self.audit_reader();
        let input = AuditInput {
            workspace: &self.workspace,
            roots: &self.roots,
            read: &read,
        };
        let mut builder = adoc_audit::audit(&input, scope.clone());
        self.add_audit_gaps(&scope, &mut builder);
        builder.finish(policy)
    }

    /// The coverage the index is missing in `scope`: each folder it could not read
    /// (`scope-not-indexed`), named relative to its root. A component's scope misses only the
    /// folders inside its versions.
    fn add_audit_gaps(&self, scope: &AuditScope, builder: &mut adoc_audit::AuditBuilder) {
        for folder in &self.unreadable {
            let in_scope = match scope {
                AuditScope::Workspace => true,
                AuditScope::Component { .. } => {
                    self.workspace.antora.all_descriptors().any(|descriptor| {
                        scope.covers(&descriptor.name, descriptor.version.as_deref())
                            && folder.starts_with(normalize_path(&descriptor.root))
                    })
                }
            };
            let Some(relative) = self.roots.iter().find_map(|root| {
                folder
                    .strip_prefix(root)
                    .ok()
                    .map(adoc_audit::portable_path)
            }) else {
                continue;
            };
            if in_scope {
                builder.gap(adoc_audit::Gap {
                    reason: adoc_audit::GapReason::ScopeNotIndexed,
                    detail: format!("`{relative}` could not be read, so it is not audited"),
                    affected: vec![relative],
                });
            }
        }
    }

    /// How many whole builds of the audit have arrived.
    #[must_use]
    pub fn audit_builds(&self) -> u64 {
        self.quality.builds
    }

    /// Whether the audit is kept: asked for, or turned on by `[quality] enabled`.
    fn audit_kept(&self) -> bool {
        self.quality.enabled || self.settings.quality.enabled
    }

    pub(crate) fn has_due_audit(&self) -> bool {
        let quality = &self.quality;
        self.audit_kept()
            && (!quality.started
                || quality.full_due
                || quality.running.is_some()
                || !quality.dirty.is_empty()
                || !quality.navigation.is_empty()
                || (quality.built && quality.catalog != self.workspace.antora.revision()))
    }

    /// Now, when the audit has changes to take in and is not running, whose result comes first.
    pub(crate) fn due_audit(&self) -> Option<std::time::Instant> {
        (self.has_due_audit() && self.quality.running.is_none()).then(|| self.clock.now())
    }

    /// The whole workspace is to be audited again.
    pub(crate) fn audit_changed_everywhere(&mut self) {
        if self.audit_kept() && self.quality.started {
            self.quality.full_due = true;
        }
    }

    /// The text of `changed` changed, and with it the parts of `paths` (`changed` among them)
    /// may have. A version's part depends on its navigation files' text and the catalog alone,
    /// so only a change to a navigation file's own text audits its version again.
    pub(crate) fn audit_changed_in(&mut self, changed: &Path, paths: &BTreeSet<PathBuf>) {
        if self.audit_kept() && self.quality.started {
            self.quality.dirty.extend(paths.iter().cloned());
            if self.workspace.antora.is_nav_file(changed) {
                self.quality.navigation.insert(normalize_path(changed));
            }
        }
    }

    /// Starts a whole build that is due, or audits again the files and versions changes reached.
    /// Changes made while a build runs wait for its result, which is older than they are.
    pub(crate) fn update_due_audit(&mut self) -> usize {
        if !self.audit_kept() || self.quality.running.is_some() {
            return 0;
        }
        if self.quality.full_due || !self.quality.started {
            self.start_audit_build();
            return 1;
        }
        let dirty = std::mem::take(&mut self.quality.dirty);
        let navigation = std::mem::take(&mut self.quality.navigation);
        let catalog = self.workspace.antora.revision();
        let versions: BTreeSet<VersionKey> = if self.quality.catalog == catalog {
            self.versions_navigated_by(&navigation)
        } else {
            self.workspace
                .antora
                .components()
                .map(|descriptor| (descriptor.name.clone(), descriptor.version.clone()))
                .chain(self.quality.versions.keys().cloned())
                .collect()
        };
        if dirty.is_empty() && versions.is_empty() {
            self.quality.catalog = catalog; // No version to audit: nothing is left due.
            return 0;
        }
        let read = self.audit_reader();
        let input = AuditInput {
            workspace: &self.workspace,
            roots: &self.roots,
            read: &read,
        };
        let mut files = Vec::new();
        for path in &dirty {
            let part = adoc_audit::audit_file(&input, path);
            let positions = positions_of(&part, || self.audit_text(path));
            files.push((path.clone(), part, positions));
        }
        let mut audited = Vec::new();
        for key in versions {
            let part = adoc_audit::audit_version(&input, &key.0, key.1.as_deref());
            audited.push((key, part));
        }
        drop(read);
        let mut changed = false;
        // The files whose quality problems these parts can change: the files audited, and those
        // the old and new findings of the versions audited are about.
        let mut touched: BTreeSet<PathBuf> = dirty;
        for (key, part) in &audited {
            let findings = self
                .quality
                .versions
                .get(key)
                .into_iter()
                .chain(part)
                .flat_map(|part| &part.findings);
            touched.extend(findings.filter_map(|finding| self.audit_path(&finding.location)));
        }
        for (path, part, positions) in files {
            let moved = if positions.is_empty() {
                self.quality.positions.remove(&path).is_some()
            } else {
                self.quality
                    .positions
                    .insert(path.clone(), positions.clone())
                    != Some(positions)
            };
            changed |= if part.is_empty() {
                self.quality.files.remove(&path).is_some()
            } else {
                self.quality.files.insert(path, part.clone()) != Some(part)
            } || moved;
        }
        for (key, part) in audited {
            changed |= match part {
                Some(part) => self.quality.versions.insert(key, part.clone()) != Some(part),
                None => self.quality.versions.remove(&key).is_some(),
            };
        }
        self.quality.catalog = catalog;
        if changed {
            self.audit_changed(Some(&touched));
        }
        1
    }

    /// The kept audit changed, in the parts of the files `touched` names or, for `None`,
    /// anywhere: their findings among the problems follow, then [`EngineEvent::AuditChanged`].
    fn audit_changed(&mut self, touched: Option<&BTreeSet<PathBuf>>) {
        self.refresh_quality_problems(touched);
        self.emit(EngineEvent::AuditChanged);
    }

    /// The settings were read again, and the audit was kept before if `was_kept`. The policy may
    /// have changed, so the kept audit is put together again; an audit kept again after being
    /// let go is built again, its parts being older than the changes made meanwhile.
    pub(crate) fn audit_settings_changed(&mut self, was_kept: bool) {
        if self.audit_kept() && !was_kept && self.quality.started {
            self.quality.full_due = true;
        }
        if self.quality.built {
            self.audit_changed(None);
        }
    }

    /// Lists the kept audit's `quality.*` findings among the problems, those of the files
    /// `touched` names or, for `None`, every file's; none while the audit is not kept. Only for
    /// a front end that keeps the problems. Each finding is as the kept report has it, under the
    /// same policy ([`adoc_audit::Policy::apply`]), without putting the report together.
    pub(crate) fn refresh_quality_problems(&mut self, touched: Option<&BTreeSet<PathBuf>>) {
        if !self.problems_kept() {
            return;
        }
        let kept = self.audit_kept() && self.quality.built;
        let changed = match touched {
            Some(paths) if kept => {
                let mut found = self.quality_problems(Some(paths));
                let mut changed = false;
                for path in paths {
                    let problems = found.remove(path).unwrap_or_default();
                    changed |= self.set_quality_problems_of(path, problems);
                }
                changed
            }
            Some(_) => false,
            None => {
                let found = if kept {
                    self.quality_problems(None)
                } else {
                    BTreeMap::new()
                };
                self.set_quality_problems(found)
            }
        };
        if changed {
            self.emit(EngineEvent::ProblemsChanged);
        }
    }

    /// The kept `quality.*` findings as problems, by file: of the files `only` names, or of
    /// every file. A finding about a place is placed where its part found it; one about the
    /// whole file is at its start. An excluded finding, or one of a rule turned off, is none.
    fn quality_problems(
        &self,
        only: Option<&BTreeSet<PathBuf>>,
    ) -> BTreeMap<PathBuf, Vec<Problem>> {
        let rule = |finding: &&adoc_audit::Finding| matches!(finding.code, FindingCode::Rule(_));
        let versions = self
            .quality
            .versions
            .values()
            .flat_map(|part| &part.findings)
            .filter(rule);
        let mut found: BTreeMap<PathBuf, Vec<&adoc_audit::Finding>> = BTreeMap::new();
        match only {
            None => {
                for (path, part) in &self.quality.files {
                    found
                        .entry(path.clone())
                        .or_default()
                        .extend(part.findings.iter().filter(rule));
                }
                for finding in versions {
                    if let Some(path) = self.audit_path(&finding.location) {
                        found.entry(path).or_default().push(finding);
                    }
                }
            }
            Some(paths) => {
                // Each file by its root and its path from there, as the audit names it.
                let mut wanted: BTreeMap<usize, BTreeMap<String, &PathBuf>> = BTreeMap::new();
                for path in paths {
                    if let Some((root, relative)) = self.audit_location(path) {
                        wanted.entry(root).or_default().insert(relative, path);
                    }
                    let part = self.quality.files.get(path);
                    found.entry(path.clone()).or_default().extend(
                        part.into_iter()
                            .flat_map(|part| &part.findings)
                            .filter(rule),
                    );
                }
                for finding in versions {
                    let path = wanted
                        .get(&finding.location.root)
                        .and_then(|paths| paths.get(finding.location.path.as_str()));
                    if let Some(path) = path {
                        found.entry((*path).clone()).or_default().push(finding);
                    }
                }
            }
        }
        let policy = &self.settings.quality.policy;
        found
            .into_iter()
            .map(|(path, findings)| {
                let positions = self.quality.positions.get(&path);
                let mut problems: Vec<Problem> = findings
                    .into_iter()
                    .filter_map(|finding| match policy.apply(finding.clone()) {
                        adoc_audit::Applied::Kept(finding) => Some(finding),
                        _ => None,
                    })
                    .map(|finding| {
                        let (range, (line, column)) = match finding.location.range {
                            Some(range) => (
                                range,
                                positions
                                    .and_then(|positions| positions.get(&range))
                                    .copied()
                                    .unwrap_or((1, 1)),
                            ),
                            None => (SourceRange::new(0, 0), (1, 1)),
                        };
                        Problem {
                            range,
                            line,
                            column,
                            severity: match finding.severity {
                                adoc_audit::Severity::Info => DiagnosticSeverity::Information,
                                adoc_audit::Severity::Warning => DiagnosticSeverity::Warning,
                                adoc_audit::Severity::Error => DiagnosticSeverity::Error,
                            },
                            code: finding.code,
                            whole_file: finding.location.range.is_none(),
                            message: finding.message,
                        }
                    })
                    .collect();
                problems.sort_by(|left, right| {
                    (left.range, left.code.as_str(), &left.message).cmp(&(
                        right.range,
                        right.code.as_str(),
                        &right.message,
                    ))
                });
                problems.dedup();
                (path, problems)
            })
            .collect()
    }

    /// The file a finding's location names: its path from its root, joined to the root.
    fn audit_path(&self, location: &adoc_audit::Location) -> Option<PathBuf> {
        let root = self.roots.get(location.root)?;
        Some(
            location
                .path
                .split('/')
                .fold(root.clone(), |path, step| path.join(step)),
        )
    }

    /// The root and the path from it with `/` separators that the audit names the file at
    /// `path` by: the first root holding it, as `adoc_audit` places a finding.
    fn audit_location(&self, path: &Path) -> Option<(usize, String)> {
        self.roots.iter().enumerate().find_map(|(index, root)| {
            path.strip_prefix(root)
                .ok()
                .map(|relative| (index, adoc_audit::portable_path(relative)))
        })
    }

    /// The text of the file at `path` as the audit read it: an open document's buffer, the
    /// index's text, or the file.
    fn audit_text(&self, path: &Path) -> Option<String> {
        if let Some(document) = self
            .document_for_path(path)
            .and_then(|id| self.document(id))
        {
            return Some(document.text.to_string());
        }
        file_text(&self.workspace, &self.file_access, path)
    }

    /// The component versions whose `antora.yml` lists one of `paths` as a navigation file.
    fn versions_navigated_by(&self, paths: &BTreeSet<PathBuf>) -> BTreeSet<VersionKey> {
        let catalog = &self.workspace.antora;
        paths
            .iter()
            .flat_map(|path| {
                catalog.all_descriptors().filter(move |descriptor| {
                    descriptor
                        .nav
                        .iter()
                        .any(|entry| normalize_path(&descriptor.root.join(entry)) == **path)
                })
            })
            .map(|descriptor| (descriptor.name.clone(), descriptor.version.clone()))
            .collect()
    }

    fn start_audit_build(&mut self) {
        let quality = &mut self.quality;
        quality.full_due = false;
        quality.started = true;
        quality.dirty.clear();
        quality.navigation.clear();
        quality.generation += 1;
        let generation = quality.generation;
        quality.running = Some(generation);
        let state = self.workspace.clone();
        let roots = self.roots.clone();
        let access = self.file_access.clone();
        let open: BTreeMap<PathBuf, String> = self
            .documents
            .values()
            .map(|document| (document.path.clone(), document.text.to_string()))
            .collect();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::Crashed {
            job: crate::worker::Crashed::Audit { generation },
            error,
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let parts = whole_audit(&state, &roots, &access, &open);
            let _ = sender.send(WorkerMessage::AuditComputed {
                generation,
                parts: Box::new(parts),
            });
        });
    }

    /// Build `generation` crashed: the parts found before stay, and the next change starts
    /// another.
    pub(crate) fn audit_crashed(&mut self, generation: u64) {
        if self.quality.running == Some(generation) {
            self.quality.running = None;
        }
    }

    pub(crate) fn audit_computed(&mut self, generation: u64, parts: AuditParts) {
        if self.quality.running != Some(generation) {
            return;
        }
        let quality = &mut self.quality;
        quality.running = None;
        quality.files = parts.files;
        quality.positions = parts.positions;
        quality.versions = parts.versions;
        quality.catalog = parts.catalog;
        quality.built = true;
        quality.builds += 1;
        self.audit_changed(None);
    }

    /// Reads a navigation file for the audit: an open document from its buffer, any other file
    /// through the engine's file access.
    fn audit_reader(&self) -> impl Fn(&Path) -> io::Result<String> + '_ {
        move |path: &Path| match self.document_for_path(path) {
            Some(id) => Ok(self
                .document(id)
                .map(|document| document.text.to_string())
                .unwrap_or_default()),
            None => self.file_access.read_to_string(path),
        }
    }
}

/// Every file's and every component version's part, on a worker.
fn whole_audit(
    state: &WorkspaceState,
    roots: &[PathBuf],
    access: &FileAccess,
    open: &BTreeMap<PathBuf, String>,
) -> AuditParts {
    let read = |path: &Path| match open.get(&normalize_path(path)) {
        Some(text) => Ok(text.clone()),
        None => access.read_to_string(path),
    };
    let input = AuditInput {
        workspace: state,
        roots,
        read: &read,
    };
    let mut positions = BTreeMap::new();
    let files = adoc_audit::audited_files(state)
        .into_iter()
        .filter_map(|path| {
            let part = adoc_audit::audit_file(&input, &path);
            let found = positions_of(&part, || match open.get(&path) {
                Some(text) => Some(text.clone()),
                None => file_text(state, access, &path),
            });
            if !found.is_empty() {
                positions.insert(path.clone(), found);
            }
            (!part.is_empty()).then_some((path, part))
        })
        .collect();
    let versions = state
        .antora
        .components()
        .filter_map(|descriptor| {
            let part =
                adoc_audit::audit_version(&input, &descriptor.name, descriptor.version.as_deref())?;
            Some(((descriptor.name.clone(), descriptor.version.clone()), part))
        })
        .collect();
    AuditParts {
        files,
        positions,
        versions,
        catalog: state.antora.revision(),
    }
}

/// The line and column of each range `part`'s `quality.*` findings name, in the text `text`
/// gives, which is read only when there is such a range.
fn positions_of(part: &FileAudit, text: impl FnOnce() -> Option<String>) -> Positions {
    let ranges: BTreeSet<SourceRange> = part
        .findings
        .iter()
        .filter(|finding| matches!(finding.code, FindingCode::Rule(_)))
        .filter_map(|finding| finding.location.range)
        .collect();
    if ranges.is_empty() {
        return Positions::new();
    }
    let text = text().unwrap_or_default();
    let lines = LineIndex::new(&text);
    ranges
        .into_iter()
        .map(|range| (range, line_and_column(&lines, &text, range.start)))
        .collect()
}

/// The index's text of the file at `path`, or else the file's.
fn file_text(state: &WorkspaceState, access: &FileAccess, path: &Path) -> Option<String> {
    state
        .index
        .file(path)
        .and_then(adoc_index::FileEntry::text)
        .map(std::borrow::Cow::into_owned)
        .or_else(|| access.read_to_string(path).ok())
}
