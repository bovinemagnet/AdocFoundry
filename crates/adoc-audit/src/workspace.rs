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
    Gap, GapReason, Location, NavCoverage, Revision, Severity, Skip,
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

/// One file's part of an audit: its existing diagnostics, and the content rules' findings and
/// skips. It depends on the file's own parse, the catalog, and the files its diagnostics depend
/// on (`adoc_lang::diagnosed_with`), never on the scope, so it can be kept per file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileAudit {
    pub findings: Vec<Finding>,
    pub skips: Vec<Skip>,
}

impl FileAudit {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty() && self.skips.is_empty()
    }
}

/// One component version's part of an audit: its navigation's diagnostics, its unlisted pages
/// and its coverage. It depends on the version's navigation files and the catalog only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionAudit {
    pub findings: Vec<Finding>,
    pub skips: Vec<Skip>,
    /// Names the version, too.
    pub coverage: NavCoverage,
}

/// The findings, skips and coverage of `scope`, to be finished under a policy: the parts in
/// scope ([`audit_file`], [`audit_version`]) put together ([`assemble`]). Reads every file in
/// scope once: a whole audit, for a worker or a command line, not for each keystroke.
#[must_use]
pub fn audit(input: &AuditInput<'_>, scope: AuditScope) -> AuditBuilder {
    let context = Context {
        input,
        scope: scope.clone(),
    };
    let files: Vec<(PathBuf, FileAudit)> = audited_files(input.workspace)
        .into_iter()
        .filter(|path| context.includes_path(path))
        .map(|path| {
            let part = audit_file(input, &path);
            (path, part)
        })
        .collect();
    let versions: Vec<VersionAudit> = input
        .workspace
        .antora
        .components()
        .filter(|descriptor| context.includes(&descriptor.name, descriptor.version.as_deref()))
        .filter_map(|descriptor| {
            audit_version(input, &descriptor.name, descriptor.version.as_deref())
        })
        .collect();
    assemble(
        input,
        scope,
        files.iter().map(|(path, part)| (path.as_path(), part)),
        &versions,
    )
}

/// Every file an audit has a part for: each indexed file and each `antora.yml`, in path order.
#[must_use]
pub fn audited_files(workspace: &WorkspaceState) -> Vec<PathBuf> {
    let mut files: BTreeSet<PathBuf> = workspace
        .index
        .files()
        .map(|file| file.path.clone())
        .collect();
    files.extend(workspace.descriptor_problems.keys().cloned());
    files.extend(
        workspace
            .descriptor_issues
            .iter()
            .map(|issue| normalize_path(&issue.path)),
    );
    files.into_iter().collect()
}

/// The part of the file at `path`: empty for a file neither indexed nor a descriptor.
#[must_use]
pub fn audit_file(input: &AuditInput<'_>, path: &Path) -> FileAudit {
    adoc_core::probe::hit(adoc_core::probe::Probe::AuditFile);
    let context = Context {
        input,
        scope: AuditScope::Workspace,
    };
    let mut part = FileAudit::default();
    for diagnostic in adoc_lang::diagnostics(input.workspace, path) {
        part.findings.push(context.finding_for(path, &diagnostic));
    }
    if let Some(file) = input.workspace.index.file(path) {
        content::file(&context, &file.path, &file.document, &mut part);
    }
    part
}

/// The part of `component` at `version`, or `None` when the catalog has no such version.
#[must_use]
pub fn audit_version(
    input: &AuditInput<'_>,
    component: &str,
    version: Option<&str>,
) -> Option<VersionAudit> {
    adoc_core::probe::hit(adoc_core::probe::Probe::AuditVersion);
    let descriptor = input.workspace.antora.component(component, version)?;
    let context = Context {
        input,
        scope: AuditScope::Workspace,
    };
    Some(navigation::version(&context, descriptor))
}

/// Puts the parts in `scope` together: what [`audit`] does with the parts it computes, and what
/// a front end keeping parts current does with the ones it keeps. A scope whose component is not
/// in the catalog is unavailable.
pub fn assemble<'p>(
    input: &AuditInput<'_>,
    scope: AuditScope,
    files: impl IntoIterator<Item = (&'p Path, &'p FileAudit)>,
    versions: impl IntoIterator<Item = &'p VersionAudit>,
) -> AuditBuilder {
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
    for (path, part) in files {
        if context.includes_path(path) {
            for finding in &part.findings {
                builder.finding(finding.clone());
            }
            for skip in &part.skips {
                builder.skip(skip.clone());
            }
        }
    }
    for part in versions {
        let coverage = &part.coverage;
        if context.includes(&coverage.component, coverage.version.as_deref()) {
            for finding in &part.findings {
                builder.finding(finding.clone());
            }
            for skip in &part.skips {
                builder.skip(skip.clone());
            }
            builder.coverage(coverage.clone());
        }
    }
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
        self.scope.covers(component, version)
    }

    /// Whether the scope covers the file at `path`: every file for the workspace; for a
    /// component, the files of its versions' modules and their `antora.yml` files.
    fn includes_path(&self, path: &Path) -> bool {
        if matches!(self.scope, AuditScope::Workspace) {
            return true;
        }
        let catalog = &self.workspace().antora;
        if let Some(context) = catalog.context_for_path(path) {
            return self.includes(&context.component, context.version.as_deref());
        }
        catalog.all_descriptors().any(|descriptor| {
            normalize_path(&descriptor.root.join("antora.yml")) == path
                && self.includes(&descriptor.name, descriptor.version.as_deref())
        })
    }

    /// The gap that makes the audit unavailable: a component, or version, not in the catalog.
    fn unavailable(&self) -> Option<Gap> {
        let AuditScope::Component { name, version } = &self.scope else {
            return None;
        };
        let catalog = &self.workspace().antora;
        let known = match version.as_deref() {
            Some("~") => catalog.component(name, None).is_some(),
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

    /// A finding for an existing diagnostic: its code, severity, message and range. The message
    /// is the diagnostic's own, except that a path beneath a root is written relative to it
    /// ([`Context::relative_paths`]).
    pub(crate) fn finding_for(&self, path: &Path, diagnostic: &Diagnostic) -> Finding {
        let mut finding = Finding::new(
            FindingCode::Diagnostic(diagnostic.code),
            Severity::from(diagnostic.severity),
            self.relative_paths(&diagnostic.message),
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

    /// `message` with every path beneath a root written relative to it with `/` separators, as
    /// the report writes paths, so that no absolute path leaves the machine in a report. A root
    /// named on its own becomes its folder name.
    pub(crate) fn relative_paths(&self, message: &str) -> String {
        let mut message = message.to_owned();
        for root in self.input.roots {
            let root = normalize_path(root);
            let absolute = root.to_string_lossy().into_owned();
            if absolute.is_empty() || !message.contains(&absolute) {
                continue;
            }
            let mut rewritten = String::with_capacity(message.len());
            let mut rest = message.as_str();
            while let Some(found) = rest.find(&absolute) {
                rewritten.push_str(&rest[..found]);
                rest = &rest[found + absolute.len()..];
                match rest.strip_prefix(['/', '\\']) {
                    Some(after) => {
                        let end = after
                            .find(|c: char| c == '`' || c == '\'' || c == '"' || c.is_whitespace())
                            .unwrap_or(after.len());
                        rewritten.push_str(&after[..end].replace('\\', "/"));
                        rest = &after[end..];
                    }
                    // The root on its own, not the start of a longer name such as another
                    // root's (`/work/docs` in `/work/docs-internal`).
                    None if ends_a_path(rest) => rewritten.push_str(&root_name(&root)),
                    None => rewritten.push_str(&absolute),
                }
            }
            rewritten.push_str(rest);
            message = rewritten;
        }
        message
    }

    /// The coordinate of the Antora resource at `path`, if it is one.
    pub(crate) fn coordinate(&self, path: &Path) -> Option<Coordinate> {
        self.workspace()
            .antora
            .resource_for_path(path)
            .map(|resource| Coordinate::from(&resource.coordinate))
    }
}

/// Whether a path named in a message ends where `rest` begins: at the end of the message,
/// whitespace, a quote or punctuation, or a full stop ending a sentence.
fn ends_a_path(rest: &str) -> bool {
    let mut characters = rest.chars();
    match characters.next() {
        None => true,
        Some('.') => characters.next().is_none_or(char::is_whitespace),
        Some(next) => next.is_whitespace() || "`'\",;:)]".contains(next),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_paths_beneath_a_root_relative_to_it() {
        let workspace = WorkspaceState::new();
        let roots = [PathBuf::from("/work/docs")];
        let read = |_: &Path| Ok(String::new());
        let input = AuditInput {
            workspace: &workspace,
            roots: &roots,
            read: &read,
        };
        let context = Context {
            input: &input,
            scope: AuditScope::Workspace,
        };
        // Messages name the root as the platform shows it: `\work\docs` on Windows.
        let root = normalize_path(&roots[0]).display().to_string();
        assert_eq!(
            context.relative_paths(&format!(
                "`{root}/a/antora.yml` is invalid; see {root}/b/c.adoc"
            )),
            "`a/antora.yml` is invalid; see b/c.adoc"
        );
        assert_eq!(
            context.relative_paths(&format!(r"`{root}\a\antora.yml` is invalid")),
            "`a/antora.yml` is invalid",
            "separators after the root become `/`"
        );
        assert_eq!(
            context.relative_paths(&format!("Nothing under {root} is indexed")),
            "Nothing under docs is indexed"
        );
        assert_eq!(
            context.relative_paths("/elsewhere/x.adoc"),
            "/elsewhere/x.adoc"
        );
    }

    #[test]
    fn a_root_is_a_prefix_of_a_path_not_of_text() {
        let workspace = WorkspaceState::new();
        let roots = [
            normalize_path(Path::new("/work/docs")),
            normalize_path(Path::new("/work/docs-internal")),
        ];
        let read = |_: &Path| Ok(String::new());
        let input = AuditInput {
            workspace: &workspace,
            roots: &roots,
            read: &read,
        };
        let context = Context {
            input: &input,
            scope: AuditScope::Workspace,
        };
        let internal = roots[1].display().to_string();
        assert_eq!(
            context.relative_paths(&format!("see {internal}/a.adoc")),
            "see a.adoc"
        );
        assert_eq!(
            context.relative_paths(&format!("Nothing under {internal}.")),
            "Nothing under docs-internal."
        );
    }
}
