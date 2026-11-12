//! Reports for headless front ends: the workspace's diagnostics, the Git status of a root and
//! the link graph, in the shapes `adocfoundry-cli check`, `git-status` and `graph` print as
//! JSON, so the CLI and the MCP server give the same answers.

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
    /// One-based, in characters.
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

/// A node of [`Engine::graph_report`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphReportNode {
    /// A file's path relative to the root, with `/` separators, or an unresolved target as
    /// written.
    pub id: String,
    pub title: String,
    /// `page`, `partial`, `nav` or `unresolved`.
    pub kind: &'static str,
    /// `version@component:module:family`, inside an Antora component.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinate: Option<String>,
    pub links_out: usize,
    pub links_in: usize,
    /// When the file last changed, where Git says: seconds since the Unix epoch, or
    /// `"uncommitted"` for a file with changes not committed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_changed: Option<ReportedChange>,
}

/// When a node's file last changed, as [`GraphReportNode::last_changed`] writes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ReportedChange {
    At(i64),
    /// Always `"uncommitted"`.
    Uncommitted(&'static str),
}

/// An edge of [`Engine::graph_report`]: every link of one kind between two nodes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphReportEdge {
    pub from: String,
    pub to: String,
    /// `xref`, `include` or `nav`.
    pub kind: &'static str,
    pub count: usize,
}

/// A node of a path through the graph, in order, and how the path came to it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphReportStep {
    pub id: String,
    pub title: String,
    /// The kinds of link from the node before (`xref`, `include`, `nav`); none for the first.
    pub links: Vec<&'static str>,
    /// Whether those links run from this node to the one before, followed backwards.
    pub backwards: bool,
}

/// The link graph, or a page's part of it, with paths relative to a root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphReport {
    pub revision: u64,
    pub nodes: Vec<GraphReportNode>,
    pub edges: Vec<GraphReportEdge>,
    /// For a path between two pages, its nodes in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<GraphReportStep>>,
}

impl GraphReport {
    /// The graph as Graphviz DOT: navigation files are boxes, partials dashed, unresolved
    /// targets dotted; include edges are dashed and navigation edges dotted.
    #[must_use]
    pub fn to_dot(&self) -> String {
        use std::fmt::Write as _;
        let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
        let mut dot = String::from("digraph links {\n  node [shape=ellipse];\n");
        for node in &self.nodes {
            let style = match node.kind {
                "nav" => " shape=box",
                "partial" => " style=dashed",
                "unresolved" => " style=dotted",
                _ => "",
            };
            let _ = writeln!(
                dot,
                "  {} [label={}{style}];",
                quote(&node.id),
                quote(&node.title)
            );
        }
        for edge in &self.edges {
            let style = match edge.kind {
                "include" => " style=dashed",
                "nav" => " style=dotted",
                _ => "",
            };
            let weight = if edge.count > 1 {
                format!(" label=\"{}\"", edge.count)
            } else {
                String::new()
            };
            let _ = writeln!(
                dot,
                "  {} -> {} [kind={}{style}{weight}];",
                quote(&edge.from),
                quote(&edge.to),
                edge.kind
            );
        }
        dot.push_str("}\n");
        dot
    }
}

impl Engine {
    /// The link graph `scope` covers, as `filter` allows, laid out until it settles and drawn
    /// as SVG.
    #[must_use]
    pub fn graph_svg(
        &self,
        scope: &crate::GraphScope,
        filter: &crate::GraphFilter,
        style: &crate::SvgStyle,
    ) -> String {
        crate::to_svg_laid_out(&self.graph(scope, filter), style)
    }

    /// The workspace's graph along the shortest path from the file at `from` to the one at
    /// `to`, as `filter` allows, in the links' direction when `directed`, else either way; with
    /// the path's steps. `None` when there is no such path.
    #[must_use]
    pub fn graph_path_view(
        &self,
        filter: &crate::GraphFilter,
        from: &Path,
        to: &Path,
        directed: bool,
    ) -> Option<(crate::GraphSnapshot, Vec<crate::PathStep>)> {
        let key =
            |path: &Path| crate::GraphKey::File(std::sync::Arc::from(crate::normalize_path(path)));
        let scope = crate::GraphScope::Workspace;
        let steps = self.graph_path(&scope, filter, &key(from), &key(to), directed)?;
        Some((self.graph(&scope, filter).along(&steps), steps))
    }

    /// The shortest path from the file at `from` to the one at `to`, as [`Self::graph_report`]
    /// gives a graph, with `path` listing its steps in order.
    #[must_use]
    pub fn graph_path_report(
        &self,
        root: &Path,
        filter: &crate::GraphFilter,
        from: &Path,
        to: &Path,
        directed: bool,
    ) -> Option<GraphReport> {
        let (snapshot, steps) = self.graph_path_view(filter, from, to, directed)?;
        let mut report = report_of(&snapshot, root);
        report.path = Some(
            steps
                .iter()
                .map(|step| GraphReportStep {
                    id: report_id(&step.key, root),
                    title: snapshot
                        .node(&step.key)
                        .map(|node| node.title.clone())
                        .unwrap_or_default(),
                    links: step.kinds.iter().map(|kind| kind.name()).collect(),
                    backwards: step.backwards,
                })
                .collect(),
        );
        Some(report)
    }

    /// The link graph `scope` covers, as `filter` allows, with paths relative to `root`.
    #[must_use]
    pub fn graph_report(
        &self,
        root: &Path,
        scope: &crate::GraphScope,
        filter: &crate::GraphFilter,
    ) -> GraphReport {
        report_of(&self.graph(scope, filter), root)
    }

    /// Every diagnostic in the workspace, of files beneath `scope` when given, with paths
    /// relative to `root`, in order of path. A folder or file the last whole index could not
    /// read is an error, `adoc.unreadable`, as nothing in it was checked.
    #[must_use]
    pub fn check_report(&self, root: &Path, scope: Option<&Path>) -> Vec<CheckEntry> {
        let mut entries = Vec::new();
        for path in &self.unreadable {
            if scope.is_some_and(|scope| !path.starts_with(scope) && !scope.starts_with(path)) {
                continue;
            }
            entries.push((
                path.clone(),
                CheckEntry {
                    path: display_path(path, root),
                    range: CheckRange { start: 0, end: 0 },
                    line: 1,
                    column: 1,
                    severity: severity_name(DiagnosticSeverity::Error),
                    code: "adoc.unreadable",
                    message: "Could not be read, so nothing in it was checked".to_owned(),
                    level: DiagnosticSeverity::Error,
                },
            ));
        }
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
                let (line, column) = lines.line_character(&text, diagnostic.range.start);
                entries.push((
                    path.clone(),
                    CheckEntry {
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
                    },
                ));
            }
        }
        // Stable, so each file's diagnostics stay in the order of their places.
        entries.sort_by(|(one, _), (other, _)| one.cmp(other));
        entries.into_iter().map(|(_, entry)| entry).collect()
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

/// A node's key as a report names it: a file's path relative to `root`, or an unresolved
/// target as written.
fn report_id(key: &crate::GraphKey, root: &Path) -> String {
    match key {
        crate::GraphKey::File(path) => display_path(path, root),
        crate::GraphKey::Unresolved(text) | crate::GraphKey::Group(text) => text.clone(),
    }
}

/// `snapshot` as a report, with paths relative to `root`.
fn report_of(snapshot: &crate::GraphSnapshot, root: &Path) -> GraphReport {
    use crate::GraphNodeKind;
    let id = |key: &crate::GraphKey| report_id(key, root);
    GraphReport {
        revision: snapshot.revision,
        nodes: snapshot
            .nodes
            .iter()
            .map(|node| GraphReportNode {
                id: id(&node.key),
                title: node.title.clone(),
                kind: match node.kind {
                    GraphNodeKind::Page => "page",
                    GraphNodeKind::Partial => "partial",
                    GraphNodeKind::Nav => "nav",
                    GraphNodeKind::Unresolved => "unresolved",
                    GraphNodeKind::Group => "group",
                },
                coordinate: node.coordinate.as_ref().map(|coordinate| {
                    format!(
                        "{}@{}:{}:{}",
                        coordinate.version.as_deref().unwrap_or("~"),
                        coordinate.component,
                        coordinate.module,
                        coordinate.family
                    )
                }),
                links_out: node.links_out,
                links_in: node.links_in,
                last_changed: node.last_changed.map(|changed| match changed {
                    crate::LastChanged::At(time) => ReportedChange::At(time),
                    crate::LastChanged::Uncommitted => ReportedChange::Uncommitted("uncommitted"),
                }),
            })
            .collect(),
        edges: snapshot
            .edges
            .iter()
            .map(|edge| GraphReportEdge {
                from: id(&edge.from),
                to: id(&edge.to),
                kind: edge.kind.name(),
                count: edge.count,
            })
            .collect(),
        path: None,
    }
}
