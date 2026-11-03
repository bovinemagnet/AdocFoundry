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

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
};

use adoc_audit::{AuditInput, AuditReport, AuditScope, FileAudit, VersionAudit};
use adoc_core::FileAccess;
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;

use crate::{workspace::WorkerMessage, Engine, EngineEvent};

/// A component version, as the audit keys its part.
type VersionKey = (String, Option<String>);

/// The parts a whole build found.
pub(crate) struct AuditParts {
    files: BTreeMap<PathBuf, FileAudit>,
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
        Some(
            adoc_audit::assemble(&input, scope.clone(), files, self.quality.versions.values())
                .finish(&self.settings.quality.policy),
        )
    }

    /// An audit of `scope` made here and now from the engine's state, open buffers included,
    /// under the current `[quality]` policy. It blocks for as long as the scope takes, so it is
    /// for front ends with no event loop, such as the command line.
    #[must_use]
    pub fn audit_now(&self, scope: AuditScope) -> AuditReport {
        let read = self.audit_reader();
        let input = AuditInput {
            workspace: &self.workspace,
            roots: &self.roots,
            read: &read,
        };
        adoc_audit::audit(&input, scope).finish(&self.settings.quality.policy)
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
            files.push((path.clone(), adoc_audit::audit_file(&input, path)));
        }
        let mut audited = Vec::new();
        for key in versions {
            let part = adoc_audit::audit_version(&input, &key.0, key.1.as_deref());
            audited.push((key, part));
        }
        drop(read);
        let mut changed = false;
        for (path, part) in files {
            changed |= if part.is_empty() {
                self.quality.files.remove(&path).is_some()
            } else {
                self.quality.files.insert(path, part.clone()) != Some(part)
            };
        }
        for (key, part) in audited {
            changed |= match part {
                Some(part) => self.quality.versions.insert(key, part.clone()) != Some(part),
                None => self.quality.versions.remove(&key).is_some(),
            };
        }
        self.quality.catalog = catalog;
        if changed {
            self.emit(EngineEvent::AuditChanged);
        }
        1
    }

    /// The component versions whose `antora.yml` lists one of `paths` as a navigation file.
    fn versions_navigated_by(&self, paths: &BTreeSet<PathBuf>) -> BTreeSet<VersionKey> {
        let catalog = &self.workspace.antora;
        paths
            .iter()
            .flat_map(|path| {
                catalog.components().filter(move |descriptor| {
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
        quality.versions = parts.versions;
        quality.catalog = parts.catalog;
        quality.built = true;
        quality.builds += 1;
        self.emit(EngineEvent::AuditChanged);
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
    let files = adoc_audit::audited_files(state)
        .into_iter()
        .filter_map(|path| {
            let part = adoc_audit::audit_file(&input, &path);
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
        versions,
        catalog: state.antora.revision(),
    }
}
