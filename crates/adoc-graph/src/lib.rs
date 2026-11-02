//! A graph of how files link to each other, with no AsciiDoc knowledge: its keys, nodes and
//! edges, the [`LinkStore`] of each file's links kept by file, walks outwards from a node,
//! snapshots filtered for display, a force-directed [`Layout`], and the graph drawn as SVG
//! ([`to_svg`]) in the colours the app's canvas uses.
//!
//! The engine decides what the links are and when they change; this crate only keeps them.
//! Nothing here knows about a UI framework, so the layout is tested and benchmarked headlessly.

mod layout;
mod snapshot;
mod store;
mod style;
mod svg;

use std::{collections::BTreeSet, path::Path, sync::Arc};

pub use layout::{Layout, Point, SETTLED};
pub use snapshot::NodeInput;
pub use store::{LinkStore, PathStep, StoredLink};
pub use style::{colour_groups, node_colour, node_radius, ColourBy, Hsl};
pub use svg::{to_svg, to_svg_laid_out, SvgStyle, EXPORT_SEED};

/// A node of the graph: a file, or a link target that does not resolve.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GraphKey {
    /// A normalised path.
    File(Arc<Path>),
    /// The target's text as written.
    Unresolved(String),
}

impl GraphKey {
    /// The file this key names, if it names one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Unresolved(_) => None,
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

    /// The part of the snapshot a path covers: its nodes, and the edges of the kinds it took
    /// between each and the next.
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
        Self {
            revision: self.revision,
            nodes: self
                .nodes
                .iter()
                .filter(|node| on_path.contains(&node.key))
                .cloned()
                .collect(),
            edges: self
                .edges
                .iter()
                .filter(|edge| taken.contains(&(edge.from.clone(), edge.to.clone(), edge.kind)))
                .cloned()
                .collect(),
        }
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
        }
    }
}
