//! Mind maps: a spanning tree of a snapshot from one root, and its deterministic tree layouts,
//! left to right or radial. A navigation file's tree follows its navigation links only, in
//! their direction, so it is the navigation's own nesting; a page's follows every link both
//! ways to a depth. Each node is placed once, at its first and shallowest place; the snapshot's
//! other links are cross-links, which front ends draw faintly.
//!
//! As in [`crate::Layout`], only `f32` arithmetic in a fixed order is used: the radial layout's
//! sines and cosines are polynomials of its own, never the platform's.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{EdgeKind, GraphKey, GraphSnapshot, NodeKind, Point};

/// How a mind map is drawn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TreeStyle {
    /// The root on the left, each level a column to its right.
    Horizontal,
    /// The root in the middle, each level a ring around it.
    Radial,
}

/// The distance between levels, and between neighbouring leaves, in layout units.
pub const LEVEL_GAP: f32 = 200.0;
pub const SIBLING_GAP: f32 = 36.0;

/// A spanning tree of a snapshot, from its root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MindMap {
    /// The tree's nodes, breadth first, the root first.
    keys: Vec<GraphKey>,
    /// Hashed: a path compares component by component, too slowly for thousands of nodes.
    index: HashMap<GraphKey, usize>,
    parent: Vec<Option<usize>>,
    /// Each node's children, in title order.
    children: Vec<Vec<usize>>,
    depth: Vec<u8>,
}

impl MindMap {
    /// The tree of `snapshot` from `root`, or `None` when the snapshot does not hold it. From a
    /// navigation file it follows navigation links in their direction; from anything else every
    /// link both ways, at most `depth` links out. A node reached by several links is placed
    /// under the parent of the first: navigation links before xrefs before includes, then the
    /// earlier parent, breadth first; siblings are in title order.
    #[must_use]
    pub fn build(snapshot: &GraphSnapshot, root: &GraphKey, depth: Option<u8>) -> Option<Self> {
        let root_node = snapshot.node(root)?;
        let nav = root_node.kind == NodeKind::Nav;
        let depth = if nav { None } else { depth };
        let place: HashMap<&GraphKey, usize> = snapshot
            .nodes
            .iter()
            .enumerate()
            .map(|(at, node)| (&node.key, at))
            .collect();
        // Each node's neighbours in the tree's sense, with the rank of the link's kind.
        let mut adjacent: Vec<Vec<(usize, u8)>> = vec![Vec::new(); snapshot.nodes.len()];
        for edge in &snapshot.edges {
            let (from, to) = (place[&edge.from], place[&edge.to]);
            let rank = match edge.kind {
                EdgeKind::Nav => 0,
                EdgeKind::Xref => 1,
                EdgeKind::Include => 2,
            };
            if nav {
                if edge.kind == EdgeKind::Nav {
                    adjacent[from].push((to, rank));
                }
            } else {
                adjacent[from].push((to, rank));
                adjacent[to].push((from, rank));
            }
        }
        let title_order = |left: &usize, right: &usize| {
            let (left, right) = (&snapshot.nodes[*left], &snapshot.nodes[*right]);
            left.title
                .cmp(&right.title)
                .then_with(|| left.key.cmp(&right.key))
        };

        let start = place[root];
        let mut tree = Self {
            keys: vec![root.clone()],
            index: HashMap::from([(root.clone(), 0)]),
            parent: vec![None],
            children: vec![Vec::new()],
            depth: vec![0],
        };
        // Places in the snapshot of the nodes placed, and of the level being widened.
        let mut placed = vec![false; snapshot.nodes.len()];
        placed[start] = true;
        let mut level: Vec<usize> = vec![0];
        let mut levels = 0u8;
        while !level.is_empty() && depth.is_none_or(|depth| levels < depth) {
            // The best parent in this level for each node not yet placed: by rank, then by the
            // parent's place in the level.
            let mut best: BTreeMap<usize, (u8, usize)> = BTreeMap::new();
            for (order, &node) in level.iter().enumerate() {
                let at = place[&tree.keys[node]];
                for &(next, rank) in &adjacent[at] {
                    if placed[next] {
                        continue;
                    }
                    let candidate = (rank, order);
                    best.entry(next)
                        .and_modify(|known| *known = (*known).min(candidate))
                        .or_insert(candidate);
                }
            }
            let mut under: Vec<Vec<usize>> = vec![Vec::new(); level.len()];
            for (next, (_, order)) in best {
                under[order].push(next);
            }
            levels += 1;
            let mut next_level = Vec::new();
            for (order, mut found) in under.into_iter().enumerate() {
                found.sort_by(title_order);
                let parent = level[order];
                for at in found {
                    placed[at] = true;
                    let key = snapshot.nodes[at].key.clone();
                    let node = tree.keys.len();
                    tree.index.insert(key.clone(), node);
                    tree.keys.push(key);
                    tree.parent.push(Some(parent));
                    tree.children.push(Vec::new());
                    tree.depth.push(levels);
                    tree.children[parent].push(node);
                    next_level.push(node);
                }
            }
            level = next_level;
        }
        Some(tree)
    }

    #[must_use]
    pub fn root(&self) -> &GraphKey {
        &self.keys[0]
    }

    /// The tree's nodes, breadth first, the root first.
    pub fn keys(&self) -> impl Iterator<Item = &GraphKey> {
        self.keys.iter()
    }

    #[must_use]
    pub fn contains(&self, key: &GraphKey) -> bool {
        self.index.contains_key(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[must_use]
    pub fn parent(&self, key: &GraphKey) -> Option<&GraphKey> {
        let node = *self.index.get(key)?;
        self.parent[node].map(|parent| &self.keys[parent])
    }

    /// The node's children, in title order.
    #[must_use]
    pub fn children(&self, key: &GraphKey) -> Vec<&GraphKey> {
        self.index.get(key).map_or_else(Vec::new, |&node| {
            self.children[node]
                .iter()
                .map(|&child| &self.keys[child])
                .collect()
        })
    }

    /// How many links the node is from the root.
    #[must_use]
    pub fn depth(&self, key: &GraphKey) -> Option<u8> {
        self.index.get(key).map(|&node| self.depth[node])
    }

    /// Whether a link between `from` and `to` is one of the tree's, either way; any other link
    /// between two of its nodes is a cross-link.
    #[must_use]
    pub fn is_tree_link(&self, from: &GraphKey, to: &GraphKey) -> bool {
        let (Some(&from), Some(&to)) = (self.index.get(from), self.index.get(to)) else {
            return false;
        };
        self.parent[to] == Some(from) || self.parent[from] == Some(to)
    }

    /// The nodes shown with the branches under `collapsed` closed: every node none of whose
    /// ancestors is collapsed. A collapsed node itself is shown.
    #[must_use]
    pub fn visible(&self, collapsed: &BTreeSet<GraphKey>) -> BTreeSet<GraphKey> {
        let mut shown = vec![false; self.keys.len()];
        shown[0] = true;
        // Breadth first, so a parent is decided before its children.
        for node in 1..self.keys.len() {
            let parent = self.parent[node].unwrap_or(0);
            shown[node] = shown[parent] && !collapsed.contains(&self.keys[parent]);
        }
        self.keys
            .iter()
            .zip(shown)
            .filter(|(_, shown)| *shown)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Where each shown node goes, the branches under `collapsed` closed, the root at the
    /// origin. Leaves are spaced evenly and a parent is centred on its children, so siblings
    /// never overlap.
    #[must_use]
    pub fn positions(
        &self,
        style: TreeStyle,
        collapsed: &BTreeSet<GraphKey>,
    ) -> HashMap<GraphKey, Point> {
        // Each shown node's slot along the leaves: a leaf's own, a parent's the middle of its
        // children's. Children come after their parent breadth first, so walk backwards.
        let shown_children = |node: usize| -> &[usize] {
            if collapsed.contains(&self.keys[node]) {
                &[]
            } else {
                &self.children[node]
            }
        };
        let mut slot = vec![0.0_f32; self.keys.len()];
        let mut leaves = 0usize;
        // Depth first, children in order, for the leaves' order.
        let mut stack = vec![0usize];
        let mut order = Vec::new();
        while let Some(node) = stack.pop() {
            order.push(node);
            let children = shown_children(node);
            if children.is_empty() {
                slot[node] = leaves as f32;
                leaves += 1;
            }
            stack.extend(children.iter().rev());
        }
        for &node in order.iter().rev() {
            let children = shown_children(node);
            if let (Some(&first), Some(&last)) = (children.first(), children.last()) {
                slot[node] = (slot[first] + slot[last]) / 2.0;
            }
        }
        let leaves = leaves.max(1) as f32;
        // Radial: each ring far enough out for neighbouring leaves on the first to stay apart,
        // with a little to spare for the chord being shorter than the arc.
        let ring = LEVEL_GAP.max(1.05 * SIBLING_GAP * leaves / std::f32::consts::TAU);
        let root_slot = slot[0];
        order.sort_unstable();
        order
            .into_iter()
            .map(|node| {
                let depth = f32::from(self.depth[node]);
                let at = match style {
                    TreeStyle::Horizontal => {
                        Point::new(depth * LEVEL_GAP, (slot[node] - root_slot) * SIBLING_GAP)
                    }
                    TreeStyle::Radial => {
                        let (cos, sin) = turn((slot[node] + 0.5) / leaves);
                        Point::new(depth * ring * cos, depth * ring * sin)
                    }
                };
                (self.keys[node].clone(), at)
            })
            .collect()
    }
}

/// The cosine and sine of `fraction` of a full turn, from polynomials, so every platform gives
/// the same bits.
fn turn(fraction: f32) -> (f32, f32) {
    use std::f32::consts::FRAC_PI_4;
    let eighths = fraction.rem_euclid(1.0) * 8.0;
    let octant = (eighths as u32).min(7);
    // The angle within its octant, from 0 to π/4.
    let within = (eighths - octant as f32) * FRAC_PI_4;
    let (c, s) = (cos_poly(within), sin_poly(within));
    let (c_far, s_far) = (cos_poly(FRAC_PI_4 - within), sin_poly(FRAC_PI_4 - within));
    match octant {
        0 => (c, s),
        1 => (s_far, c_far),
        2 => (-s, c),
        3 => (-c_far, s_far),
        4 => (-c, -s),
        5 => (-s_far, -c_far),
        6 => (s, -c),
        _ => (c_far, -s_far),
    }
}

/// Taylor series to the ninth power, within 4e-9 of the sine on [0, π/4].
fn sin_poly(x: f32) -> f32 {
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))))
}

fn cos_poly(x: f32) -> f32 {
    let x2 = x * x;
    1.0 - x2 / 2.0 * (1.0 - x2 / 12.0 * (1.0 - x2 / 30.0 * (1.0 - x2 / 56.0)))
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use super::*;
    use crate::{GraphEdge, GraphNode};

    fn key(name: &str) -> GraphKey {
        GraphKey::File(Arc::from(Path::new(name)))
    }

    fn snapshot(nodes: &[(&str, NodeKind)], edges: &[(&str, &str, EdgeKind)]) -> GraphSnapshot {
        let mut nodes: Vec<GraphNode> = nodes
            .iter()
            .map(|(name, kind)| GraphNode {
                key: key(name),
                title: name.to_uppercase(),
                kind: *kind,
                coordinate: None,
                links_out: 0,
                links_in: 0,
            })
            .collect();
        nodes.sort_by(|left, right| left.key.cmp(&right.key));
        let mut edges: Vec<GraphEdge> = edges
            .iter()
            .map(|(from, to, kind)| GraphEdge {
                from: key(from),
                to: key(to),
                kind: *kind,
                count: 1,
            })
            .collect();
        edges.sort();
        GraphSnapshot {
            revision: 1,
            nodes,
            edges,
        }
    }

    fn names(keys: Vec<&GraphKey>) -> Vec<String> {
        keys.into_iter()
            .map(|key| key.path().unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn mind_map_places_each_page_once() {
        use EdgeKind::{Include, Xref};
        let page = NodeKind::Page;
        let graph = snapshot(
            &[
                ("a", page),
                ("b", page),
                ("c", page),
                ("d", page),
                ("e", page),
            ],
            &[
                ("a", "b", Xref),
                ("a", "c", Xref),
                ("b", "c", Xref),
                ("c", "d", Xref),
                ("d", "b", Include),
                // Linking to the root: reached both ways.
                ("e", "a", Xref),
            ],
        );
        let tree = MindMap::build(&graph, &key("a"), Some(3)).unwrap();
        assert_eq!(names(tree.children(&key("a"))), ["b", "c", "e"]);
        assert_eq!(names(tree.children(&key("c"))), ["d"]);
        assert!(tree.children(&key("b")).is_empty());
        assert_eq!(tree.depth(&key("d")), Some(2));
        // b → c and d → b are cross-links.
        assert!(tree.is_tree_link(&key("a"), &key("c")));
        assert!(tree.is_tree_link(&key("d"), &key("c")), "either way");
        assert!(!tree.is_tree_link(&key("b"), &key("c")));
        assert!(!tree.is_tree_link(&key("d"), &key("b")));

        // The snapshot cut down to the tree's nodes keeps the links among them.
        let shown = tree.visible(&BTreeSet::from([key("c")]));
        let cut = graph.restrict(|key| shown.contains(key));
        assert_eq!(cut.nodes.len(), 4);
        assert!(cut.edges.iter().all(|edge| edge.to != key("d")));
        let c = cut.node(&key("c")).unwrap();
        assert_eq!((c.links_out, c.links_in), (0, 2));

        // To a depth of one, only the root's own links.
        let near = MindMap::build(&graph, &key("a"), Some(1)).unwrap();
        assert_eq!(near.len(), 4);
        assert!(!near.contains(&key("d")));
        assert!(MindMap::build(&graph, &key("zz"), None).is_none());
    }

    #[test]
    fn a_navigation_tree_follows_navigation_links_only() {
        use EdgeKind::{Nav, Xref};
        let graph = snapshot(
            &[
                ("nav", NodeKind::Nav),
                ("home", NodeKind::Page),
                ("install", NodeKind::Page),
                ("configure", NodeKind::Page),
                ("stray", NodeKind::Page),
            ],
            &[
                ("nav", "home", Nav),
                ("nav", "install", Nav),
                ("install", "configure", Nav),
                // An xref straight to Configure does not lift it out of its branch.
                ("home", "configure", Xref),
                ("home", "stray", Xref),
            ],
        );
        let tree = MindMap::build(&graph, &key("nav"), Some(1)).unwrap();
        assert_eq!(names(tree.children(&key("nav"))), ["home", "install"]);
        assert_eq!(names(tree.children(&key("install"))), ["configure"]);
        assert!(!tree.contains(&key("stray")));
        assert!(!tree.is_tree_link(&key("home"), &key("configure")));
    }

    #[test]
    fn mind_map_layout_is_deterministic() {
        let mut nodes = vec![("root".to_owned(), NodeKind::Page)];
        let mut edges = Vec::new();
        for branch in 0..6 {
            let name = format!("b{branch}");
            edges.push(("root".to_owned(), name.clone()));
            for leaf in 0..branch {
                let child = format!("b{branch}l{leaf}");
                edges.push((name.clone(), child.clone()));
                nodes.push((child, NodeKind::Page));
            }
            nodes.push((name, NodeKind::Page));
        }
        let node_refs: Vec<(&str, NodeKind)> = nodes
            .iter()
            .map(|(name, kind)| (name.as_str(), *kind))
            .collect();
        let edge_refs: Vec<(&str, &str, EdgeKind)> = edges
            .iter()
            .map(|(from, to)| (from.as_str(), to.as_str(), EdgeKind::Xref))
            .collect();
        let graph = snapshot(&node_refs, &edge_refs);
        let tree = MindMap::build(&graph, &key("root"), Some(3)).unwrap();
        for style in [TreeStyle::Horizontal, TreeStyle::Radial] {
            let none = BTreeSet::new();
            let positions = tree.positions(style, &none);
            assert_eq!(positions, tree.positions(style, &none), "{style:?}");
            assert_eq!(positions.len(), tree.len());
            assert_eq!(positions[&key("root")], Point::new(0.0, 0.0), "{style:?}");
            let all: Vec<Point> = positions.values().copied().collect();
            for (at, a) in all.iter().enumerate() {
                for b in &all[at + 1..] {
                    let apart = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                    assert!(apart >= SIBLING_GAP * 0.99, "{style:?}: {a:?} and {b:?}");
                }
            }
        }
        // Horizontal: a parent is centred on its children, a column per level.
        let positions = tree.positions(TreeStyle::Horizontal, &BTreeSet::new());
        let (b3, first, last) = (
            positions[&key("b3")],
            positions[&key("b3l0")],
            positions[&key("b3l2")],
        );
        assert_eq!(b3.y, (first.y + last.y) / 2.0);
        assert_eq!((b3.x, first.x), (LEVEL_GAP, 2.0 * LEVEL_GAP));

        // A collapsed branch hides what is under it, and frees its room.
        let collapsed = BTreeSet::from([key("b5")]);
        let visible = tree.visible(&collapsed);
        assert!(visible.contains(&key("b5")) && !visible.contains(&key("b5l0")));
        let positions = tree.positions(TreeStyle::Horizontal, &collapsed);
        assert_eq!(positions.len(), visible.len());
        assert!(positions.keys().all(|key| visible.contains(key)));
    }

    #[test]
    fn turns_are_within_a_hair_of_the_platforms_trigonometry() {
        for step in 0..=256 {
            let fraction = step as f32 / 256.0;
            let (cos, sin) = turn(fraction);
            let angle = f64::from(fraction) * std::f64::consts::TAU;
            assert!(
                (f64::from(cos) - angle.cos()).abs() < 1e-5,
                "cos at {fraction}"
            );
            assert!(
                (f64::from(sin) - angle.sin()).abs() < 1e-5,
                "sin at {fraction}"
            );
        }
    }
}
