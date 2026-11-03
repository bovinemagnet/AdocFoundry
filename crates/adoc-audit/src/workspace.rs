//! An audit of a workspace: the existing diagnostics of every file in scope, then the audit's own
//! rules ([`crate::navigation`], [`crate::content`]).

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};

use adoc_core::{Diagnostic, SourceRange};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;

use crate::{
    content, navigation, portable_path, AuditBuilder, AuditScope, Coordinate, Finding, FindingCode,
    Gap, GapReason, Location, Revision, Severity,
};

/// What an audit reads. The audit itself never touches the filesystem: navigation files are read
/// through `read`, so that unsaved buffers can stand in for files on disk, and everything else
/// comes from the workspace's index and catalog.
pub struct AuditInput<'a> {
    pub workspace: &'a WorkspaceState,
    /// The workspace roots, normalised as the index's paths are.
    pub roots: &'a [PathBuf],
    pub read: &'a dyn Fn(&Path) -> io::Result<String>,
}

/// The findings, skips and coverage of `scope`, to be finished under a policy. Reads every file
/// in scope once: a whole audit, for a worker, not for each keystroke.
#[must_use]
pub fn audit(input: &AuditInput<'_>, scope: AuditScope) -> AuditBuilder {
    let workspace = input.workspace;
    let mut builder = AuditBuilder::new(
        scope.clone(),
        Revision {
            index: workspace.index.revision(),
            catalog: workspace.antora.revision(),
        },
        input.roots.iter().map(|root| root_name(root)).collect(),
    );
    let context = Context { input, scope };
    if let Some(gap) = context.unavailable() {
        builder.unavailable(gap);
        return builder;
    }
    for file in workspace.index.files() {
        if context.includes_file(&file.path) {
            context.diagnostics(&file.path, &mut builder);
        }
    }
    for descriptor in context.descriptor_files() {
        context.diagnostics(&descriptor, &mut builder);
    }
    navigation::check(&context, &mut builder);
    content::check(&context, &mut builder);
    builder
}

/// An audit under way: its input and scope, and where things are.
pub(crate) struct Context<'a> {
    pub(crate) input: &'a AuditInput<'a>,
    pub(crate) scope: AuditScope,
}

impl Context<'_> {
    pub(crate) fn workspace(&self) -> &WorkspaceState {
        self.input.workspace
    }

    /// Whether the scope covers `component` at `version`.
    pub(crate) fn includes(&self, component: &str, version: Option<&str>) -> bool {
        match &self.scope {
            AuditScope::Workspace => true,
            AuditScope::Component {
                name,
                version: wanted,
            } => {
                name == component
                    && wanted
                        .as_deref()
                        .is_none_or(|wanted| Some(wanted) == version)
            }
        }
    }

    /// Whether the scope covers the file at `path`: every file for the workspace, the files of
    /// a component version's modules for a component.
    pub(crate) fn includes_file(&self, path: &Path) -> bool {
        match &self.scope {
            AuditScope::Workspace => true,
            AuditScope::Component { .. } => self
                .workspace()
                .antora
                .context_for_path(path)
                .is_some_and(|context| {
                    self.includes(&context.component, context.version.as_deref())
                }),
        }
    }

    /// The gap that makes the audit unavailable: a component, or version, not in the catalog.
    fn unavailable(&self) -> Option<Gap> {
        let AuditScope::Component { name, version } = &self.scope else {
            return None;
        };
        let catalog = &self.workspace().antora;
        let known = match version {
            Some(version) => catalog.component(name, Some(version)).is_some(),
            None => !catalog.versions_of(name).is_empty(),
        };
        (!known).then(|| {
            let named = version
                .as_ref()
                .map_or_else(|| name.clone(), |version| format!("{version}@{name}"));
            Gap {
                reason: GapReason::RemoteComponentMissing,
                detail: format!("Component `{named}` is not in the local catalog"),
                affected: vec![named],
            }
        })
    }

    /// The `antora.yml` files in scope, each once.
    fn descriptor_files(&self) -> BTreeSet<PathBuf> {
        let workspace = self.workspace();
        let mut files: BTreeSet<PathBuf> = workspace
            .descriptor_problems
            .keys()
            .cloned()
            .chain(
                workspace
                    .descriptor_issues
                    .iter()
                    .map(|issue| normalize_path(&issue.path)),
            )
            .collect();
        files.retain(|path| match &self.scope {
            AuditScope::Workspace => true,
            AuditScope::Component { .. } => workspace.antora.components().any(|descriptor| {
                normalize_path(&descriptor.root.join("antora.yml")) == *path
                    && self.includes(&descriptor.name, descriptor.version.as_deref())
            }),
        });
        files
    }

    /// The existing diagnostics of the file at `path`, unchanged.
    fn diagnostics(&self, path: &Path, builder: &mut AuditBuilder) {
        for diagnostic in adoc_lang::diagnostics(self.workspace(), path) {
            builder.finding(self.finding_for(path, &diagnostic));
        }
    }

    /// A finding for an existing diagnostic: its code, severity, message and range.
    pub(crate) fn finding_for(&self, path: &Path, diagnostic: &Diagnostic) -> Finding {
        let mut finding = Finding::new(
            FindingCode::Diagnostic(diagnostic.code),
            Severity::from(diagnostic.severity),
            diagnostic.message.clone(),
            self.location(path, Some(diagnostic.range)),
        );
        finding.coordinate = self.coordinate(path);
        finding
    }

    /// Where `path` is: its root's place and the path relative to it. An indexed file is always
    /// beneath a root; anything else is named by its file name alone, never by its full path.
    pub(crate) fn location(&self, path: &Path, range: Option<SourceRange>) -> Location {
        let path = normalize_path(path);
        let found = self
            .input
            .roots
            .iter()
            .enumerate()
            .find_map(|(index, root)| {
                path.strip_prefix(normalize_path(root))
                    .ok()
                    .map(|relative| (index, portable_path(relative)))
            });
        let (root, relative) = found.unwrap_or_else(|| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (0, name)
        });
        Location::new(root, relative, range)
    }

    /// The coordinate of the Antora resource at `path`, if it is one.
    pub(crate) fn coordinate(&self, path: &Path) -> Option<Coordinate> {
        self.workspace()
            .antora
            .resource_for_path(path)
            .map(|resource| Coordinate::from(&resource.coordinate))
    }
}

/// A root by the name a front end shows: its folder name.
fn root_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || portable_path(root),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Whether a file in a `pages` folder is a page Antora publishes: an `.adoc` file with no name,
/// and no folder on its path, beginning with `_` (hidden) or `.` (ignored).
pub(crate) fn published_page(relative: &Path) -> bool {
    let adoc = relative
        .extension()
        .is_some_and(|extension| extension == "adoc");
    let hidden = relative.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| part.starts_with(['_', '.']))
    });
    adoc && !hidden
}
