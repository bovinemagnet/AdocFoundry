//! Documentation impact analysis (Impact PRD §§6 and 8): the engine turns a change source
//! (two Git refs, the working tree against `HEAD`, or paths named) into the changes and the two
//! sides' dependency snapshots, and runs `adoc-impact`'s rules over them.
//!
//! A revision's snapshot is built from Git object data alone: the commit's tree listed under
//! the roots (honouring `[workspace] ignore`), each file's text read from its blob, catalogued
//! by the rules disk discovery follows (`discover_antora_files`) and parsed by the same parser.
//! Nothing is checked out, fetched or run, and in ref-to-ref mode the live index is never
//! consulted. Snapshots are cached by tree id.

#[cfg(feature = "git")]
use std::collections::BTreeMap;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(feature = "git")]
use adoc_impact::Gap;
use adoc_impact::{
    analyse, Change, ChangeKind, DependencySnapshot, ImpactReport, Limits, Publication,
    PublicationSelection, ReportSource,
};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;

use crate::{Engine, ExportTarget};

/// What an impact analysis compares.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChangeSource {
    /// `HEAD` against the working tree: files on disk, and the open pages' buffers.
    WorkingTree,
    /// Two revisions, by any name Git reads: branches, tags, commit ids. The working tree is not
    /// read.
    GitRefs { base: String, head: String },
    /// Files the caller names, against the working tree, with no base revision: a path that
    /// exists is modified, one that does not was removed.
    Paths(Vec<PathBuf>),
    /// The working tree against itself with these files taken out of the index and catalog:
    /// what removing, moving or renaming them would affect (Inspect Impact). Nothing is removed.
    Removal(Vec<PathBuf>),
}

/// Why no analysis could be made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImpactError {
    /// A Git comparison was asked for, and the roots are in no repository, or this build has no
    /// Git.
    NoRepository(String),
    /// A ref names no commit.
    UnknownRevision(String),
    /// Git could not be read.
    Git(String),
    /// A named path lies outside every root.
    OutsideRoots(PathBuf),
}

impl std::fmt::Display for ImpactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRepository(message) | Self::UnknownRevision(message) | Self::Git(message) => {
                formatter.write_str(message)
            }
            Self::OutsideRoots(path) => {
                write!(
                    formatter,
                    "{} is outside the workspace's roots",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ImpactError {}

/// The snapshots built from revisions, by tree id: the most recent few.
#[derive(Default)]
pub(crate) struct ImpactCache {
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    snapshots: Vec<(String, Arc<WorkspaceState>)>,
}

#[cfg(feature = "git")]
const CACHED_SNAPSHOTS: usize = 4;

/// Snapshots built from revisions while analysing, by key, for the cache.
pub(crate) type Built = Vec<(String, Arc<WorkspaceState>)>;

/// [`Inputs`] owned, for a worker.
pub(crate) struct OwnedInputs {
    pub source: ChangeSource,
    pub roots: Vec<PathBuf>,
    pub publications: Vec<Publication>,
    pub limits: Limits,
    pub live: Arc<WorkspaceState>,
    pub open: Vec<(PathBuf, bool)>,
    pub ignore: Vec<String>,
    pub cached: Vec<(String, Arc<WorkspaceState>)>,
}

/// The analysis `inputs` ask for, on whichever thread holds them.
pub(crate) fn compute_owned(inputs: &OwnedInputs) -> (Result<ImpactReport, ImpactError>, Built) {
    compute(&Inputs {
        source: &inputs.source,
        roots: &inputs.roots,
        publications: &inputs.publications,
        limits: inputs.limits,
        live: &inputs.live,
        open: &inputs.open,
        ignore: &inputs.ignore,
        cached: &inputs.cached,
    })
}

/// What an analysis reads, taken from the engine so that it runs on the caller's thread or a
/// worker alike.
struct Inputs<'a> {
    source: &'a ChangeSource,
    roots: &'a [PathBuf],
    publications: &'a [Publication],
    limits: Limits,
    /// The live workspace, the open pages' unsaved text included.
    live: &'a WorkspaceState,
    /// Each open document's path, and whether it holds unsaved edits.
    open: &'a [(PathBuf, bool)],
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    ignore: &'a [String],
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    cached: &'a [(String, Arc<WorkspaceState>)],
}

impl Engine {
    /// The impact of `source`'s changes, within `limits`, with the publication profiles of
    /// `[pdf.profiles]`, worked out on the caller's thread.
    pub fn analyse_impact(
        &mut self,
        source: &ChangeSource,
        limits: Limits,
    ) -> Result<ImpactReport, ImpactError> {
        let roots = self.roots().to_vec();
        let publications = self.impact_publications();
        let open = self.open_documents();
        let ignore = self.settings.workspace.ignore.clone();
        let (result, built) = compute(&Inputs {
            source,
            roots: &roots,
            publications: &publications,
            limits,
            live: &self.workspace,
            open: &open,
            ignore: &ignore,
            cached: &self.impact.snapshots,
        });
        self.remember_snapshots(built);
        result
    }

    /// The snapshots kept from earlier analyses, for a worker to reuse.
    pub(crate) fn impact_cached(&self) -> Vec<(String, Arc<WorkspaceState>)> {
        self.impact.snapshots.clone()
    }

    /// Each open document's path, and whether it holds unsaved edits.
    pub(crate) fn open_documents(&self) -> Vec<(PathBuf, bool)> {
        self.documents
            .values()
            .map(|document| (document.path.clone(), document.dirty))
            .collect()
    }

    /// Keeps the snapshots an analysis built, most recent first.
    #[cfg_attr(not(feature = "git"), allow(clippy::unused_self, unused_mut))]
    pub(crate) fn remember_snapshots(&mut self, built: Built) {
        #[cfg(feature = "git")]
        for (key, state) in built {
            self.impact.snapshots.retain(|(known, _)| *known != key);
            self.impact.snapshots.insert(0, (key, state));
            self.impact.snapshots.truncate(CACHED_SNAPSHOTS);
        }
        #[cfg(not(feature = "git"))]
        let _ = built;
    }

    /// The profiles of `[pdf.profiles]` as the rules read them; a profile that cannot be read is
    /// left out (the audit and `validate_publication` report it).
    pub(crate) fn impact_publications(&self) -> Vec<Publication> {
        self.settings
            .pdf
            .profiles
            .iter()
            .filter_map(|profile| {
                let args = self.profile_export_args(&profile.name).ok()?;
                let selection = match args.target {
                    ExportTarget::File(path) => PublicationSelection::Page(path),
                    ExportTarget::Page(id) => {
                        PublicationSelection::Page(self.document(id)?.path.clone())
                    }
                    ExportTarget::Component { name, version } => {
                        PublicationSelection::Component { name, version }
                    }
                    ExportTarget::Navigation(files) => PublicationSelection::Navigation(files),
                };
                Some(Publication {
                    name: profile.name.clone(),
                    selection,
                })
            })
            .collect()
    }
}

/// `path` made absolute against the first root, and refused outside every root.
fn rooted(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, ImpactError> {
    let path = normalize_path(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        roots
            .first()
            .map_or_else(|| path.to_path_buf(), |root| root.join(path))
    });
    if roots.iter().any(|root| path.starts_with(root)) {
        Ok(path)
    } else {
        Err(ImpactError::OutsideRoots(path))
    }
}

/// The analysis `inputs` ask for, and the revision snapshots it built on the way.
fn compute(inputs: &Inputs<'_>) -> (Result<ImpactReport, ImpactError>, Built) {
    let roots = inputs.roots;
    match inputs.source {
        ChangeSource::Paths(paths) => {
            let mut changes = Vec::new();
            for path in paths {
                let path = match rooted(path, roots) {
                    Ok(path) => path,
                    Err(error) => return (Err(error), Built::new()),
                };
                let open = inputs.open.iter().any(|(open, _)| *open == path);
                let kind = if path.exists() || open {
                    ChangeKind::Modified
                } else {
                    ChangeKind::Removed
                };
                changes.push(Change { path, kind });
            }
            let head = DependencySnapshot::borrowed(inputs.live, roots);
            let mut report = analyse(&changes, None, &head, inputs.publications, inputs.limits);
            report.source = Some(ReportSource::Paths);
            (Ok(report), Built::new())
        }
        ChangeSource::Removal(paths) => {
            // Base is the workspace as it is; head the same with the files taken out of the
            // index and catalog, as a staged move takes its file out.
            let mut removed = inputs.live.clone();
            let mut changes = Vec::new();
            for path in paths {
                let path = match rooted(path, roots) {
                    Ok(path) => path,
                    Err(error) => return (Err(error), Built::new()),
                };
                removed.remove_document(&path);
                removed.antora.remove_source(&path);
                changes.push(Change {
                    path,
                    kind: ChangeKind::Removed,
                });
            }
            let base = DependencySnapshot::borrowed(inputs.live, roots);
            let head = DependencySnapshot::borrowed(&removed, roots);
            let mut report = analyse(
                &changes,
                Some(&base),
                &head,
                inputs.publications,
                inputs.limits,
            );
            report.source = Some(ReportSource::Removal);
            (Ok(report), Built::new())
        }
        ChangeSource::WorkingTree | ChangeSource::GitRefs { .. } => compute_git(inputs),
    }
}

#[cfg(not(feature = "git"))]
fn compute_git(_: &Inputs<'_>) -> (Result<ImpactReport, ImpactError>, Built) {
    (
        Err(ImpactError::NoRepository(
            "this build has no Git; name the changed paths instead".to_owned(),
        )),
        Built::new(),
    )
}

#[cfg(feature = "git")]
fn compute_git(inputs: &Inputs<'_>) -> (Result<ImpactReport, ImpactError>, Built) {
    let mut built = Built::new();
    let result = analyse_git(inputs, &mut built);
    (result, built)
}

#[cfg(feature = "git")]
fn analyse_git(inputs: &Inputs<'_>, built: &mut Built) -> Result<ImpactReport, ImpactError> {
    use adoc_impact::RevisionRef;

    let roots = inputs.roots;
    let root = roots
        .first()
        .ok_or_else(|| ImpactError::NoRepository("no workspace is open".to_owned()))?;
    let repository = adoc_git::Repository::discover(root)
        .map_err(|error| ImpactError::Git(error.to_string()))?
        .ok_or_else(|| {
            ImpactError::NoRepository(format!(
                "{} is in no Git repository; name the changed paths instead",
                root.display()
            ))
        })?;
    let revision = |spec: &str| {
        repository
            .resolve_revision(spec)
            .map_err(|error| match error {
                adoc_git::GitError::UnknownRevision { .. } => {
                    ImpactError::UnknownRevision(error.to_string())
                }
                other => ImpactError::Git(other.to_string()),
            })
    };
    let workdir = repository.workdir().to_path_buf();
    let under_roots = |relative: &Path| {
        let path = normalize_path(&workdir.join(relative));
        roots
            .iter()
            .any(|root| path.starts_with(root))
            .then_some(path)
    };
    let mut snapshot = |revision: &adoc_git::Revision| {
        revision_snapshot(&repository, revision, roots, inputs, built)
    };
    match inputs.source {
        ChangeSource::GitRefs { base, head } => {
            let (base, head) = (revision(base)?, revision(head)?);
            let mut changes = Vec::new();
            for change in repository
                .tree_changes(&base, &head)
                .map_err(|error| ImpactError::Git(error.to_string()))?
            {
                changes.extend(tree_change(change, &under_roots));
            }
            let (base_state, base_gaps) = snapshot(&base)?;
            let (head_state, head_gaps) = snapshot(&head)?;
            let mut report = analyse(
                &changes,
                Some(&DependencySnapshot::borrowed(&base_state, roots)),
                &DependencySnapshot::borrowed(&head_state, roots),
                inputs.publications,
                inputs.limits,
            );
            for gap in base_gaps.into_iter().chain(head_gaps) {
                report.add_gap(gap);
            }
            report.source = Some(ReportSource::GitRefs {
                base: RevisionRef {
                    spec: base.spec,
                    commit: base.commit,
                },
                head: RevisionRef {
                    spec: head.spec,
                    commit: head.commit,
                },
            });
            Ok(report)
        }
        ChangeSource::WorkingTree => {
            let base = revision("HEAD")?;
            let mut changes = Vec::new();
            for status in repository
                .status()
                .map_err(|error| ImpactError::Git(error.to_string()))?
            {
                let Some(path) = under_roots(&status.path) else {
                    continue;
                };
                let kind = match (&status.index, &status.worktree) {
                    (adoc_git::Change::Renamed(from), _) | (_, adoc_git::Change::Renamed(from)) => {
                        match under_roots(from) {
                            Some(from) => ChangeKind::Renamed { from },
                            None => ChangeKind::Added,
                        }
                    }
                    (_, adoc_git::Change::Deleted) | (adoc_git::Change::Deleted, _) => {
                        ChangeKind::Removed
                    }
                    (adoc_git::Change::Added, _) | (_, adoc_git::Change::Untracked) => {
                        ChangeKind::Added
                    }
                    (adoc_git::Change::Ignored, _) | (_, adoc_git::Change::Ignored) => continue,
                    _ => ChangeKind::Modified,
                };
                changes.push(Change { path, kind });
            }
            // The open pages' unsaved text is the working tree too.
            let mut unsaved = false;
            for (path, _) in inputs.open.iter().filter(|(_, dirty)| *dirty) {
                unsaved = true;
                if !changes.iter().any(|change| change.path == *path)
                    && roots.iter().any(|root| path.starts_with(root))
                {
                    changes.push(Change {
                        path: path.clone(),
                        kind: ChangeKind::Modified,
                    });
                }
            }
            let (base_state, base_gaps) = snapshot(&base)?;
            let mut report = analyse(
                &changes,
                Some(&DependencySnapshot::borrowed(&base_state, roots)),
                &DependencySnapshot::borrowed(inputs.live, roots),
                inputs.publications,
                inputs.limits,
            );
            for gap in base_gaps {
                report.add_gap(gap);
            }
            report.source = Some(ReportSource::WorkingTree {
                base: RevisionRef {
                    spec: base.spec,
                    commit: base.commit,
                },
                unsaved_buffers: unsaved,
            });
            Ok(report)
        }
        ChangeSource::Paths(_) | ChangeSource::Removal(_) => {
            unreachable!("paths are analysed without Git")
        }
    }
}

/// Whether `path`, a revision's file beneath `root`, is in a folder `ignore` leaves out, by the
/// index's rule ([`adoc_core::is_ignored_beneath`]); the `antora.yml` files among `listed`, the
/// revision's files, mark its components.
#[cfg(feature = "git")]
fn ignored_in_revision(
    path: &Path,
    root: &Path,
    ignore: &[String],
    listed: &std::collections::HashSet<PathBuf>,
) -> bool {
    path.strip_prefix(root).is_ok_and(|inside| {
        adoc_core::is_ignored_beneath(inside, ignore, |folder| {
            listed.contains(&root.join(folder).join("antora.yml"))
        })
    })
}

/// The index and catalog of `revision`, built from its objects, from the cache (or what this
/// analysis built already) when its tree was built before; with the gaps its descriptors leave.
#[cfg(feature = "git")]
fn revision_snapshot(
    repository: &adoc_git::Repository,
    revision: &adoc_git::Revision,
    roots: &[PathBuf],
    inputs: &Inputs<'_>,
    built: &mut Built,
) -> Result<(Arc<WorkspaceState>, Vec<Gap>), ImpactError> {
    let key = format!("{}\u{1f}{roots:?}", revision.tree);
    if let Some((_, state)) = inputs
        .cached
        .iter()
        .chain(built.iter())
        .find(|(tree, _)| *tree == key)
    {
        return Ok((state.clone(), descriptor_gaps(state, roots, &revision.spec)));
    }
    let workdir = repository.workdir().to_path_buf();
    let ignore = inputs.ignore;
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    let listed: Vec<(PathBuf, PathBuf)> = repository
        .tree_files(revision)
        .map_err(|error| ImpactError::Git(error.to_string()))?
        .into_iter()
        .map(|relative| (normalize_path(&workdir.join(&relative)), relative))
        .collect();
    let paths: std::collections::HashSet<PathBuf> =
        listed.iter().map(|(path, _)| path.clone()).collect();
    for (path, relative) in listed {
        let Some(root) = roots.iter().find(|root| path.starts_with(root)) else {
            continue;
        };
        if ignored_in_revision(&path, root, ignore, &paths) {
            continue;
        }
        // Only text is read; anything else is listed with no text, as the catalog needs only
        // its path.
        let wanted = adoc_index::is_asciidoc_path(&path)
            || path.extension().is_some_and(|extension| extension == "yml");
        let text = if wanted {
            repository
                .text_at(revision, &relative)
                .map_err(|error| ImpactError::Git(error.to_string()))?
                .unwrap_or_default()
        } else {
            String::new()
        };
        files.insert(path, text);
    }
    let discovered = adoc_antora::discover_antora_files(roots, &files, ignore);
    let mut state = WorkspaceState {
        antora: discovered.catalog,
        descriptor_issues: discovered.issues,
        ..WorkspaceState::default()
    };
    for (path, text) in &files {
        if adoc_index::is_asciidoc_path(path) {
            let document = adoc_parser::parse(&crate::document::uri_for(path), text).document;
            state.index.replace(path.clone(), document);
        }
    }
    let state = Arc::new(state);
    built.push((key, state.clone()));
    Ok((
        state.clone(),
        descriptor_gaps(&state, roots, &revision.spec),
    ))
}

/// A Git tree change as the rules read it, for a file under the roots.
#[cfg(feature = "git")]
fn tree_change(
    change: adoc_git::TreeChange,
    under_roots: &impl Fn(&Path) -> Option<PathBuf>,
) -> Option<Change> {
    Some(match change {
        adoc_git::TreeChange::Added(path) => Change {
            path: under_roots(&path)?,
            kind: ChangeKind::Added,
        },
        adoc_git::TreeChange::Modified(path) => Change {
            path: under_roots(&path)?,
            kind: ChangeKind::Modified,
        },
        adoc_git::TreeChange::Removed(path) => Change {
            path: under_roots(&path)?,
            kind: ChangeKind::Removed,
        },
        adoc_git::TreeChange::Renamed { from, to, .. } => {
            match (under_roots(&from), under_roots(&to)) {
                (Some(from), Some(to)) => Change {
                    path: to,
                    kind: ChangeKind::Renamed { from },
                },
                (None, Some(to)) => Change {
                    path: to,
                    kind: ChangeKind::Added,
                },
                (Some(from), None) => Change {
                    path: from,
                    kind: ChangeKind::Removed,
                },
                (None, None) => return None,
            }
        }
    })
}

/// A revision's descriptors that could not be read, as gaps: what they would have held is not
/// analysed.
#[cfg(feature = "git")]
fn descriptor_gaps(state: &WorkspaceState, roots: &[PathBuf], spec: &str) -> Vec<Gap> {
    state
        .descriptor_issues
        .iter()
        .map(|issue| Gap {
            reason: "revision-descriptor-unreadable",
            detail: format!("at `{spec}`: {issue}"),
            affected: vec![roots
                .iter()
                .find_map(|root| issue.path.strip_prefix(root).ok())
                .map_or_else(
                    || issue.path.display().to_string(),
                    |relative| relative.to_string_lossy().replace('\\', "/"),
                )],
        })
        .collect()
}

#[cfg(all(test, feature = "git"))]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn a_revision_keeps_build_folders_inside_modules() {
        // Issue #531: a revision's files are left out by the index's rule.
        let root = Path::new("/work");
        let ignore = vec!["build".to_owned(), ".git".to_owned()];
        let listed: std::collections::HashSet<PathBuf> = [
            "/work/docs/antora.yml",
            "/work/docs/modules/ROOT/pages/build/index.adoc",
            "/work/build/stray.adoc",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let ignored =
            |path: &str| super::ignored_in_revision(Path::new(path), root, &ignore, &listed);
        assert!(!ignored("/work/docs/modules/ROOT/pages/build/index.adoc"));
        assert!(ignored("/work/build/stray.adoc"));
        assert!(ignored("/work/docs/build/site.adoc"));
    }
}
