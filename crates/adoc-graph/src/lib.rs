//! A graph of how files link to each other, with no AsciiDoc knowledge: its keys, nodes and
//! edges, the [`LinkStore`] of each file's links kept by file, walks outwards from a node,
//! snapshots filtered for display, groups of nodes by module or component version
//! ([`Groups`], [`GraphSnapshot::collapse`]), a force-directed [`Layout`], and the graph drawn as
//! SVG ([`to_svg`]) in the colours the app's canvas uses, and mind maps ([`MindMap`]): a tree
//! of the snapshot from one root, laid out left to right or radially.
//!
//! The engine decides what the links are and when they change; this crate only keeps them.
//! Nothing here knows about a UI framework, so the layout is tested and benchmarked headlessly.

mod group;
mod layout;
mod snapshot;
mod store;
mod style;
mod svg;
mod tree;

use std::{collections::BTreeSet, path::Path, sync::Arc};

pub use group::{group_name, GroupBy, Groups, OTHER_GROUP};
pub use layout::{Layout, Point, SETTLED};
pub use snapshot::NodeInput;
pub use store::{LinkStore, PathStep, StoredLink};
pub use style::{colour_group, colour_groups, node_radius, ColourBy, Hsl, Palette};
pub use svg::{to_svg, to_svg_laid_out, SvgStyle, EXPORT_SEED};
pub use tree::{MindMap, TreeStyle, LEVEL_GAP, SIBLING_GAP};

/// A node of the graph: a file, or a link target that does not resolve.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GraphKey {
    /// A normalised path.
    File(Arc<Path>),
    /// The target's text as written.
    Unresolved(String),
    /// A group of nodes collapsed into one, by its name ([`GraphSnapshot::collapse`]).
    Group(String),
}

impl GraphKey {
    /// The file this key names, if it names one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Unresolved(_) | Self::Group(_) => None,
        }
    }
}

/// What a link is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EdgeKind {
    Xref,
    Include,
    Nav,
}

impl EdgeKind {
    /// Its name in lower case, as front ends show it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Xref => "xref",
            Self::Include => "include",
            Self::Nav => "nav",
        }
    }
}

/// What a node is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NodeKind {
    Page,
    /// A file reached by `include::`.
    Partial,
    /// A navigation file.
    Nav,
    /// A target that does not resolve.
    Unresolved,
    /// A group collapsed into one node.
    Group,
}

/// Where a file sits in an Antora workspace.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeCoordinate {
    pub component: String,
    pub version: Option<String>,
    pub module: String,
    pub family: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphNode {
    pub key: GraphKey,
    /// The document title, else the file name.
    pub title: String,
    pub kind: NodeKind,
    /// Filled by the engine from the Antora catalog.
    pub coordinate: Option<NodeCoordinate>,
    /// The edges leaving the node in the snapshot.
    pub links_out: usize,
    /// The edges reaching the node in the snapshot.
    pub links_in: usize,
    /// When its file last changed, where Git says; `None` without Git, outside a repository,
    /// or for a target that does not resolve.
    pub last_changed: Option<LastChanged>,
}

/// When a file last changed, from its Git history.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LastChanged {
    /// It has changes not committed yet, so it changed now.
    Uncommitted,
    /// Committed at this time, in seconds since the Unix epoch. For a file older than all the
    /// history read, the oldest commit read: it is at least that old.
    At(i64),
}

/// Every link of one kind between two nodes, as one edge.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GraphEdge {
    pub from: GraphKey,
    pub to: GraphKey,
    pub kind: EdgeKind,
    /// How many links the edge stands for.
    pub count: usize,
}

/// The nodes and edges a query found, sorted by key and by ends.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GraphSnapshot {
    /// The graph's revision when the snapshot was taken; 0 before the graph is first built.
    pub revision: u64,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl GraphSnapshot {
    /// The node with `key`.
    #[must_use]
    pub fn node(&self, key: &GraphKey) -> Option<&GraphNode> {
        self.nodes
            .binary_search_by(|node| node.key.cmp(key))
            .ok()
            .map(|at| &self.nodes[at])
    }

    /// The snapshot with only the nodes `keep` accepts and the edges between them, each node's
    /// links in and out counted again.
    #[must_use]
    pub fn restrict(&self, keep: impl Fn(&GraphKey) -> bool) -> Self {
        let nodes: Vec<GraphNode> = self
            .nodes
            .iter()
            .filter(|node| keep(&node.key))
            .cloned()
            .collect();
        let kept: std::collections::HashSet<&GraphKey> =
            nodes.iter().map(|node| &node.key).collect();
        let edges: Vec<GraphEdge> = self
            .edges
            .iter()
            .filter(|edge| kept.contains(&edge.from) && kept.contains(&edge.to))
            .cloned()
            .collect();
        let mut nodes = nodes;
        recount(&mut nodes, &edges);
        Self {
            revision: self.revision,
            nodes,
            edges,
        }
    }

    /// The part of the snapshot a path covers: its nodes, and the edges of the kinds it took
    /// between each and the next, each node's links in and out counted again.
    #[must_use]
    pub fn along(&self, steps: &[PathStep]) -> Self {
        let mut taken = BTreeSet::new();
        for pair in steps.windows(2) {
            let (before, step) = (&pair[0].key, &pair[1]);
            let (from, to) = if step.backwards {
                (&step.key, before)
            } else {
                (before, &step.key)
            };
            for kind in &step.kinds {
                taken.insert((from.clone(), to.clone(), *kind));
            }
        }
        let on_path: BTreeSet<&GraphKey> = steps.iter().map(|step| &step.key).collect();
        let mut nodes: Vec<GraphNode> = self
            .nodes
            .iter()
            .filter(|node| on_path.contains(&node.key))
            .cloned()
            .collect();
        let edges: Vec<GraphEdge> = self
            .edges
            .iter()
            .filter(|edge| taken.contains(&(edge.from.clone(), edge.to.clone(), edge.kind)))
            .cloned()
            .collect();
        recount(&mut nodes, &edges);
        Self {
            revision: self.revision,
            nodes,
            edges,
        }
    }
}

/// Sets each node's links in and out to the number of `edges` leaving and reaching it.
pub(crate) fn recount(nodes: &mut [GraphNode], edges: &[GraphEdge]) {
    let mut counts: std::collections::HashMap<&GraphKey, (usize, usize)> =
        std::collections::HashMap::new();
    for edge in edges {
        counts.entry(&edge.from).or_default().0 += 1;
        counts.entry(&edge.to).or_default().1 += 1;
    }
    for node in nodes {
        let (out, into) = counts.get(&node.key).copied().unwrap_or_default();
        node.links_out = out;
        node.links_in = into;
    }
}

/// What a snapshot shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphFilter {
    pub partials: bool,
    pub nav: bool,
    /// Pages with no links in or out, navigation included.
    pub orphans: bool,
    pub unresolved: bool,
    /// `None`: every component.
    pub components: Option<BTreeSet<String>>,
}

impl Default for GraphFilter {
    fn default() -> Self {
        Self {
            partials: true,
            nav: true,
            orphans: true,
            unresolved: false,
            components: None,
        }
    }
}

impl GraphFilter {
    /// Whether a link of `kind` may be shown or walked.
    #[must_use]
    pub fn allows_edge(&self, kind: EdgeKind) -> bool {
        match kind {
            EdgeKind::Nav => self.nav,
            EdgeKind::Include => self.partials,
            EdgeKind::Xref => true,
        }
    }

    /// Whether a node of `kind` may be shown, orphans and components aside.
    #[must_use]
    pub fn allows_kind(&self, kind: NodeKind) -> bool {
        match kind {
            NodeKind::Page => true,
            NodeKind::Partial => self.partials,
            NodeKind::Nav => self.nav,
            NodeKind::Unresolved => self.unresolved,
            NodeKind::Group => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> GraphKey {
        GraphKey::File(Arc::from(Path::new(name)))
    }

    fn node(name: &str, links_out: usize, links_in: usize) -> GraphNode {
        GraphNode {
            key: file(name),
            title: name.to_owned(),
            kind: NodeKind::Page,
            coordinate: None,
            links_out,
            links_in,
            last_changed: None,
        }
    }

    fn edge(from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            from: file(from),
            to: file(to),
            kind: EdgeKind::Xref,
            count: 1,
        }
    }

    fn step(name: &str, kinds: Vec<EdgeKind>) -> PathStep {
        PathStep {
            key: file(name),
            kinds,
            backwards: false,
        }
    }

    #[test]
    fn a_path_counts_only_its_own_links() {
        let snapshot = GraphSnapshot {
            revision: 1,
            nodes: vec![
                node("a", 3, 0),
                node("b", 1, 1),
                node("c", 0, 1),
                node("d", 0, 1),
                node("e", 0, 1),
            ],
            edges: vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("a", "d"),
                edge("b", "e"),
            ],
        };
        let path = snapshot.along(&[
            step("a", Vec::new()),
            step("b", vec![EdgeKind::Xref]),
            step("e", vec![EdgeKind::Xref]),
        ]);
        assert_eq!(path.edges.len(), 2);
        let counts = |name: &str| {
            let node = path.node(&file(name)).unwrap();
            (node.links_out, node.links_in)
        };
        assert_eq!(counts("a"), (1, 0));
        assert_eq!(counts("b"), (1, 1));
        assert_eq!(counts("e"), (0, 1));
    }
}
