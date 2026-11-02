//! A seeded, deterministic force-directed layout: every pair of nodes repels, every edge pulls
//! its ends towards a rest length, every node is drawn gently to the centre, and the motion is
//! damped and cools, so it settles. Above [`BARNES_HUT_ABOVE`] nodes, repulsion is
//! approximated with a Barnes–Hut quadtree.
//!
//! Only `f32` arithmetic in a fixed order is used, never trigonometry or a hash map's order, so
//! the same seed and the same calls give the same positions on every platform.

use std::collections::BTreeMap;

use crate::{GraphKey, GraphSnapshot};

/// A position in layout units. A front end maps it to pixels with its pan and zoom.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

/// The length an edge pulls its ends towards.
pub const REST_LENGTH: f32 = 60.0;

/// `step` reports the layout settled once no node moves further than this in a step.
pub const SETTLED: f32 = 0.05;

/// Above this many nodes, repulsion is approximated.
pub const BARNES_HUT_ABOVE: usize = 500;

const REPULSION: f32 = 0.1 * REST_LENGTH * REST_LENGTH;
const SPRING: f32 = 0.05;
const GRAVITY: f32 = 0.01;
const DAMPING: f32 = 0.6;
const MAX_SPEED: f32 = REST_LENGTH;
/// The cooling per step, and the heat below which the layout counts as settled whatever moves.
const COOLING: f32 = 0.977;
const COLD: f32 = 0.002;
/// The heat a drag or a change wakes the layout with.
const WARM: f32 = 0.3;
/// Barnes–Hut's accuracy: a cell is one body when its width over its distance is below this.
const THETA: f32 = 0.9;
const MAX_TREE_DEPTH: u8 = 24;

/// The positions of a graph's nodes, by key.
#[derive(Clone, Debug)]
pub struct Layout {
    seed: u64,
    /// The nodes, sorted, and each one's place in the tables below.
    keys: Vec<GraphKey>,
    index: BTreeMap<GraphKey, usize>,
    positions: Vec<Point>,
    velocities: Vec<Point>,
    pinned: Vec<bool>,
    /// Each pair of nodes an edge joins, once, in index order.
    edges: Vec<(usize, usize)>,
    heat: f32,
}

impl Layout {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            keys: Vec::new(),
            index: BTreeMap::new(),
            positions: Vec::new(),
            velocities: Vec::new(),
            pinned: Vec::new(),
            edges: Vec::new(),
            heat: 0.0,
        }
    }

    /// Adds the snapshot's new nodes and drops those it no longer has, keeping the rest where
    /// they are. A new node starts next to a neighbour already placed, or at a seeded place on
    /// a ring. Wakes the layout if anything changed.
    pub fn sync(&mut self, snapshot: &GraphSnapshot) {
        let old: BTreeMap<GraphKey, (Point, Point, bool)> = self
            .keys
            .drain(..)
            .zip(self.positions.drain(..))
            .zip(self.velocities.drain(..).zip(self.pinned.drain(..)))
            .map(|((key, position), (velocity, pinned))| (key, (position, velocity, pinned)))
            .collect();
        let keys: Vec<GraphKey> = snapshot.nodes.iter().map(|node| node.key.clone()).collect();
        let index: BTreeMap<GraphKey, usize> = keys
            .iter()
            .enumerate()
            .map(|(at, key)| (key.clone(), at))
            .collect();
        let mut edges: Vec<(usize, usize)> = snapshot
            .edges
            .iter()
            .filter_map(|edge| {
                let (from, to) = (*index.get(&edge.from)?, *index.get(&edge.to)?);
                (from != to).then_some((from.min(to), from.max(to)))
            })
            .collect();
        edges.sort_unstable();
        edges.dedup();

        let changed = old.len() != keys.len()
            || keys.iter().any(|key| !old.contains_key(key))
            || edges != self.edges;
        let mut placed: Vec<Option<Point>> = keys
            .iter()
            .map(|key| old.get(key).map(|(position, ..)| *position))
            .collect();
        let neighbours = adjacency(keys.len(), &edges);
        let ring = ring_radius(keys.len());
        for at in 0..keys.len() {
            if placed[at].is_some() {
                continue;
            }
            let direction = unit(mix(self.seed, &keys[at], 0));
            let start = neighbours[at]
                .iter()
                .find_map(|&other| placed[other])
                .map_or_else(
                    || {
                        let spread = 0.5 + 0.5 * fraction(mix(self.seed, &keys[at], 1));
                        Point::new(direction.x * ring * spread, direction.y * ring * spread)
                    },
                    |next_to| {
                        Point::new(
                            next_to.x + direction.x * REST_LENGTH * 0.5,
                            next_to.y + direction.y * REST_LENGTH * 0.5,
                        )
                    },
                );
            placed[at] = Some(start);
        }
        self.positions = placed.into_iter().map(Option::unwrap_or_default).collect();
        self.velocities = keys
            .iter()
            .map(|key| {
                old.get(key)
                    .map_or_else(Point::default, |(_, velocity, _)| *velocity)
            })
            .collect();
        self.pinned = keys
            .iter()
            .map(|key| old.get(key).is_some_and(|(_, _, pinned)| *pinned))
            .collect();
        self.keys = keys;
        self.index = index;
        self.edges = edges;
        if changed {
            self.heat = if old.is_empty() {
                1.0
            } else {
                self.heat.max(WARM)
            };
        }
    }

    /// Moves every node one step of `dt` (1.0 a frame). Returns `false` once the layout has
    /// settled, so a front end can stop asking for frames.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.heat < COLD || self.keys.is_empty() {
            self.heat = 0.0;
            return false;
        }
        let forces = self.forces();
        let mut largest = 0.0_f32;
        for (at, force) in forces.into_iter().enumerate() {
            if self.pinned[at] {
                self.velocities[at] = Point::default();
                continue;
            }
            let velocity = &mut self.velocities[at];
            velocity.x = (velocity.x + force.x * self.heat * dt) * DAMPING;
            velocity.y = (velocity.y + force.y * self.heat * dt) * DAMPING;
            let speed = velocity.length();
            if speed > MAX_SPEED {
                velocity.x *= MAX_SPEED / speed;
                velocity.y *= MAX_SPEED / speed;
            }
            let position = &mut self.positions[at];
            position.x += velocity.x * dt;
            position.y += velocity.y * dt;
            largest = largest.max(velocity.length() * dt);
        }
        self.heat *= COOLING;
        if largest < SETTLED {
            self.heat = 0.0;
        }
        self.heat >= COLD
    }

    /// Whether the layout is still moving.
    #[must_use]
    pub fn is_moving(&self) -> bool {
        self.heat >= COLD
    }

    /// Wakes the layout, as a drag does.
    pub fn wake(&mut self) {
        self.heat = self.heat.max(WARM);
    }

    /// Holds the node at `at` until it is unpinned.
    pub fn pin(&mut self, key: &GraphKey, at: Point) {
        if let Some(&index) = self.index.get(key) {
            self.positions[index] = at;
            self.velocities[index] = Point::default();
            self.pinned[index] = true;
            self.wake();
        }
    }

    pub fn unpin(&mut self, key: &GraphKey) {
        if let Some(&index) = self.index.get(key) {
            self.pinned[index] = false;
            self.wake();
        }
    }

    /// Whether the node is pinned.
    #[must_use]
    pub fn is_pinned(&self, key: &GraphKey) -> bool {
        self.index.get(key).is_some_and(|&index| self.pinned[index])
    }

    /// Unpins every node and lays the graph out again from its seeded start.
    pub fn reset(&mut self) {
        let snapshot_keys = std::mem::take(&mut self.keys);
        let edges = std::mem::take(&mut self.edges);
        self.positions.clear();
        self.velocities.clear();
        self.pinned.clear();
        self.index.clear();
        let snapshot = GraphSnapshot {
            revision: 0,
            nodes: snapshot_keys
                .into_iter()
                .map(|key| crate::GraphNode {
                    key,
                    title: String::new(),
                    kind: crate::NodeKind::Page,
                    coordinate: None,
                    links_out: 0,
                    links_in: 0,
                })
                .collect(),
            edges: Vec::new(),
        };
        let keys: Vec<GraphKey> = snapshot.nodes.iter().map(|node| node.key.clone()).collect();
        let snapshot = GraphSnapshot {
            edges: edges
                .iter()
                .map(|&(from, to)| crate::GraphEdge {
                    from: keys[from].clone(),
                    to: keys[to].clone(),
                    kind: crate::EdgeKind::Xref,
                    count: 1,
                })
                .collect(),
            ..snapshot
        };
        self.sync(&snapshot);
        self.heat = 1.0;
    }

    #[must_use]
    pub fn position(&self, key: &GraphKey) -> Option<Point> {
        self.index.get(key).map(|&index| self.positions[index])
    }

    /// Every node and its position, in key order.
    pub fn positions(&self) -> impl Iterator<Item = (&GraphKey, Point)> {
        self.keys.iter().zip(self.positions.iter().copied())
    }

    /// The node nearest `at` within `radius`.
    #[must_use]
    pub fn node_at(&self, at: Point, radius: f32) -> Option<&GraphKey> {
        let mut best = None;
        let mut nearest = radius * radius;
        for (key, position) in self.positions() {
            let (dx, dy) = (position.x - at.x, position.y - at.y);
            let distance = dx * dx + dy * dy;
            if distance <= nearest {
                nearest = distance;
                best = Some(key);
            }
        }
        best
    }

    /// The corners of the box holding every node, if there are any.
    #[must_use]
    pub fn bounds(&self) -> Option<(Point, Point)> {
        let mut positions = self.positions.iter();
        let first = *positions.next()?;
        Some(positions.fold((first, first), |(low, high), point| {
            (
                Point::new(low.x.min(point.x), low.y.min(point.y)),
                Point::new(high.x.max(point.x), high.y.max(point.y)),
            )
        }))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn forces(&self) -> Vec<Point> {
        let mut forces: Vec<Point> = self
            .positions
            .iter()
            .map(|position| Point::new(-position.x * GRAVITY, -position.y * GRAVITY))
            .collect();
        if self.positions.len() > BARNES_HUT_ABOVE {
            let tree = QuadTree::build(&self.positions);
            for (at, force) in forces.iter_mut().enumerate() {
                let push = tree.repulsion(&self.positions, at);
                force.x += push.x;
                force.y += push.y;
            }
        } else {
            for i in 0..self.positions.len() {
                for j in i + 1..self.positions.len() {
                    let push = repel(self.positions[i], self.positions[j], 1.0, i, j);
                    forces[i].x += push.x;
                    forces[i].y += push.y;
                    forces[j].x -= push.x;
                    forces[j].y -= push.y;
                }
            }
        }
        for &(i, j) in &self.edges {
            let (a, b) = (self.positions[i], self.positions[j]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let distance = (dx * dx + dy * dy).sqrt().max(0.01);
            let pull = (distance - REST_LENGTH) * SPRING / distance;
            forces[i].x += dx * pull;
            forces[i].y += dy * pull;
            forces[j].x -= dx * pull;
            forces[j].y -= dy * pull;
        }
        forces
    }
}

/// The push on a body at `a` from `mass` at `b`. Bodies in the same place are parted by their
/// indices, so the result never depends on anything but the inputs.
fn repel(a: Point, b: Point, mass: f32, i: usize, j: usize) -> Point {
    let (mut dx, mut dy) = (a.x - b.x, a.y - b.y);
    let mut squared = dx * dx + dy * dy;
    if squared < 1e-4 {
        dx = if i < j { 0.01 } else { -0.01 };
        dy = 0.0;
        squared = 1e-4;
    }
    // REPULSION / distance along the unit vector: REPULSION / distance².
    let strength = REPULSION * mass / squared;
    Point::new(dx * strength, dy * strength)
}

fn adjacency(nodes: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut neighbours = vec![Vec::new(); nodes];
    for &(from, to) in edges {
        neighbours[from].push(to);
        neighbours[to].push(from);
    }
    neighbours
}

fn ring_radius(nodes: usize) -> f32 {
    REST_LENGTH * (nodes.max(1) as f32).sqrt()
}

/// A number from the seed and the key, the same on every platform.
fn mix(seed: u64, key: &GraphKey, salt: u64) -> u64 {
    // FNV-1a over the key's bytes, then SplitMix64.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
    let bytes: &[u8] = match key {
        GraphKey::File(path) => path.as_os_str().as_encoded_bytes(),
        GraphKey::Unresolved(text) => text.as_bytes(),
    };
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let mut z = hash ^ seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A number from 0 to 1.
fn fraction(bits: u64) -> f32 {
    (bits >> 40) as f32 / (1u64 << 24) as f32
}

/// A direction from `bits`, without trigonometry.
fn unit(bits: u64) -> Point {
    let x = fraction(bits) * 2.0 - 1.0;
    let y = fraction(bits << 24) * 2.0 - 1.0;
    let length = (x * x + y * y).sqrt();
    if length < 1e-3 {
        Point::new(1.0, 0.0)
    } else {
        Point::new(x / length, y / length)
    }
}

/// Cells of a quadtree over the bodies, for Barnes–Hut.
struct QuadTree {
    cells: Vec<Cell>,
}

#[derive(Clone, Copy)]
struct Cell {
    centre: Point,
    half: f32,
    mass: f32,
    /// The sum of the bodies' positions, for their centre of mass.
    sum: Point,
    /// The first of four children, or 0 for a leaf.
    children: u32,
    /// The body in a leaf with one, else `u32::MAX`.
    body: u32,
}

impl Cell {
    fn new(centre: Point, half: f32) -> Self {
        Self {
            centre,
            half,
            mass: 0.0,
            sum: Point::default(),
            children: 0,
            body: u32::MAX,
        }
    }
}

impl QuadTree {
    fn build(positions: &[Point]) -> Self {
        let mut low = Point::new(f32::MAX, f32::MAX);
        let mut high = Point::new(f32::MIN, f32::MIN);
        for position in positions {
            low = Point::new(low.x.min(position.x), low.y.min(position.y));
            high = Point::new(high.x.max(position.x), high.y.max(position.y));
        }
        let half = ((high.x - low.x).max(high.y - low.y) / 2.0).max(1.0) + 1.0;
        let centre = Point::new((low.x + high.x) / 2.0, (low.y + high.y) / 2.0);
        let mut tree = Self {
            cells: Vec::with_capacity(positions.len() * 2),
        };
        tree.cells.push(Cell::new(centre, half));
        for (body, &position) in positions.iter().enumerate() {
            tree.insert(positions, body as u32, position);
        }
        tree
    }

    fn insert(&mut self, positions: &[Point], body: u32, position: Point) {
        let mut cell = 0usize;
        let mut depth = 0u8;
        loop {
            {
                let current = &mut self.cells[cell];
                current.mass += 1.0;
                current.sum.x += position.x;
                current.sum.y += position.y;
            }
            let current = self.cells[cell];
            if current.children == 0 {
                if current.mass == 1.0 {
                    self.cells[cell].body = body;
                    return;
                }
                if depth >= MAX_TREE_DEPTH || current.body == u32::MAX {
                    // Bodies too close to part stay together in one leaf.
                    self.cells[cell].body = u32::MAX;
                    return;
                }
                // Split the leaf, moving its one body down.
                let first = self.cells.len() as u32;
                let quarter = current.half / 2.0;
                for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    self.cells.push(Cell::new(
                        Point::new(
                            current.centre.x + sx * quarter,
                            current.centre.y + sy * quarter,
                        ),
                        quarter,
                    ));
                }
                let resident = current.body;
                self.cells[cell].children = first;
                self.cells[cell].body = u32::MAX;
                let at = positions[resident as usize];
                let child = first as usize + quadrant(current.centre, at);
                let leaf = &mut self.cells[child];
                leaf.mass = 1.0;
                leaf.sum = at;
                leaf.body = resident;
            }
            let current = self.cells[cell];
            cell = current.children as usize + quadrant(current.centre, position);
            depth += 1;
        }
    }

    fn repulsion(&self, positions: &[Point], body: usize) -> Point {
        let at = positions[body];
        let mut force = Point::default();
        let mut stack = vec![0usize];
        while let Some(cell) = stack.pop() {
            let current = &self.cells[cell];
            if current.mass == 0.0 {
                continue;
            }
            if current.children == 0 && current.body == body as u32 {
                continue;
            }
            let centre = Point::new(current.sum.x / current.mass, current.sum.y / current.mass);
            let (dx, dy) = (at.x - centre.x, at.y - centre.y);
            let squared = dx * dx + dy * dy;
            let width = current.half * 2.0;
            if current.children == 0 || width * width < THETA * THETA * squared {
                let other = if current.body == u32::MAX {
                    usize::MAX
                } else {
                    current.body as usize
                };
                let mass = if current.children == 0 && current.body == u32::MAX {
                    // A leaf of bodies in one place, perhaps this one among them.
                    let inside = (at.x - centre.x).abs() < 1e-3 && (at.y - centre.y).abs() < 1e-3;
                    current.mass - if inside { 1.0 } else { 0.0 }
                } else {
                    current.mass
                };
                if mass > 0.0 {
                    let push = repel(at, centre, mass, body, other);
                    force.x += push.x;
                    force.y += push.y;
                }
            } else {
                for child in 0..4 {
                    stack.push(current.children as usize + child);
                }
            }
        }
        force
    }
}

fn quadrant(centre: Point, at: Point) -> usize {
    usize::from(at.x >= centre.x) + 2 * usize::from(at.y >= centre.y)
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use super::*;
    use crate::{EdgeKind, GraphEdge, GraphNode, NodeKind};

    fn key(name: &str) -> GraphKey {
        GraphKey::File(Arc::from(Path::new(name)))
    }

    /// A graph of `nodes` pages, each linking to the next few, sorted as snapshots are.
    pub(crate) fn graph(nodes: usize, edges_each: usize) -> GraphSnapshot {
        let mut keys: Vec<GraphKey> = (0..nodes).map(|n| key(&format!("p{n:05}"))).collect();
        keys.sort();
        let mut edges = Vec::new();
        for n in 0..nodes {
            for step in 1..=edges_each {
                let to = (n * 7 + step * 13) % nodes;
                if to != n {
                    edges.push(GraphEdge {
                        from: keys[n].clone(),
                        to: keys[to].clone(),
                        kind: EdgeKind::Xref,
                        count: 1,
                    });
                }
            }
        }
        edges.sort();
        edges.dedup_by(|a, b| a.from == b.from && a.to == b.to);
        GraphSnapshot {
            revision: 1,
            nodes: keys
                .into_iter()
                .map(|key| GraphNode {
                    key,
                    title: String::new(),
                    kind: NodeKind::Page,
                    coordinate: None,
                    links_out: 0,
                    links_in: 0,
                })
                .collect(),
            edges,
        }
    }

    fn settle(layout: &mut Layout, steps: usize) -> usize {
        (1..=steps).find(|_| !layout.step(1.0)).unwrap_or(steps + 1)
    }

    #[test]
    fn layout_is_deterministic() {
        let snapshot = graph(40, 2);
        let run = || {
            let mut layout = Layout::new(7);
            layout.sync(&snapshot);
            for _ in 0..200 {
                layout.step(1.0);
            }
            layout.positions().map(|(_, at)| at).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
        let mut other = Layout::new(8);
        other.sync(&snapshot);
        assert_ne!(
            run()[0],
            other.position(&snapshot.nodes[0].key).unwrap(),
            "the seed decides the start"
        );
    }

    #[test]
    fn barnes_hut_is_deterministic_and_close_to_exact() {
        let snapshot = graph(BARNES_HUT_ABOVE + 100, 3);
        let mut layout = Layout::new(1);
        layout.sync(&snapshot);
        for _ in 0..20 {
            layout.step(1.0);
        }
        let mut again = Layout::new(1);
        again.sync(&snapshot);
        for _ in 0..20 {
            again.step(1.0);
        }
        assert!(layout.positions().eq(again.positions()));

        // The approximation stays near the exact sum.
        let tree = QuadTree::build(&layout.positions);
        let approximate = tree.repulsion(&layout.positions, 3);
        let mut exact = Point::default();
        for other in 0..layout.positions.len() {
            if other != 3 {
                let push = repel(layout.positions[3], layout.positions[other], 1.0, 3, other);
                exact.x += push.x;
                exact.y += push.y;
            }
        }
        let error = Point::new(approximate.x - exact.x, approximate.y - exact.y).length();
        assert!(
            error <= exact.length() * 0.2 + 0.5,
            "{approximate:?} vs {exact:?}"
        );
    }

    #[test]
    fn pinned_nodes_stay_put() {
        let snapshot = graph(20, 2);
        let mut layout = Layout::new(3);
        layout.sync(&snapshot);
        let pinned = snapshot.nodes[4].key.clone();
        layout.pin(&pinned, Point::new(500.0, -200.0));
        for _ in 0..100 {
            layout.step(1.0);
        }
        assert_eq!(layout.position(&pinned), Some(Point::new(500.0, -200.0)));
        assert!(layout.is_pinned(&pinned));
        layout.unpin(&pinned);
        layout.step(1.0);
        assert_ne!(layout.position(&pinned), Some(Point::new(500.0, -200.0)));
    }

    #[test]
    fn sync_keeps_existing_positions() {
        let mut snapshot = graph(30, 2);
        let mut layout = Layout::new(5);
        layout.sync(&snapshot);
        assert!(settle(&mut layout, 500) <= 500);
        let before: Vec<(GraphKey, Point)> = layout
            .positions()
            .map(|(key, at)| (key.clone(), at))
            .collect();

        let added = key("zz-new");
        snapshot.nodes.push(GraphNode {
            key: added.clone(),
            title: String::new(),
            kind: NodeKind::Page,
            coordinate: None,
            links_out: 1,
            links_in: 0,
        });
        snapshot.edges.push(GraphEdge {
            from: added.clone(),
            to: before[0].0.clone(),
            kind: EdgeKind::Xref,
            count: 1,
        });
        layout.sync(&snapshot);
        let start = layout.position(&added).unwrap();
        let neighbour = before[0].1;
        assert!(Point::new(start.x - neighbour.x, start.y - neighbour.y).length() <= REST_LENGTH);
        for (key, at) in &before {
            assert_eq!(layout.position(key), Some(*at), "sync moves nothing");
        }
        settle(&mut layout, 500);
        for (key, at) in &before {
            let now = layout.position(key).unwrap();
            let moved = Point::new(now.x - at.x, now.y - at.y).length();
            assert!(moved <= REST_LENGTH, "{key:?} moved {moved}");
        }
    }

    #[test]
    fn layout_settles() {
        let mut layout = Layout::new(11);
        layout.sync(&graph(12, 2));
        let steps = settle(&mut layout, 500);
        assert!(steps <= 500, "still moving after 500 steps");
        assert!(!layout.is_moving());
        assert!(!layout.step(1.0));
    }

    #[test]
    fn reset_lays_out_again_from_the_start() {
        let snapshot = graph(15, 2);
        let mut layout = Layout::new(2);
        layout.sync(&snapshot);
        let start: Vec<Point> = layout.positions().map(|(_, at)| at).collect();
        layout.pin(&snapshot.nodes[0].key, Point::new(900.0, 900.0));
        settle(&mut layout, 500);
        layout.reset();
        assert!(!layout.is_pinned(&snapshot.nodes[0].key));
        assert!(layout.is_moving());
        assert_eq!(
            layout.positions().map(|(_, at)| at).collect::<Vec<_>>(),
            start
        );
    }

    #[test]
    fn node_at_finds_the_nearest_within_the_radius() {
        let mut layout = Layout::new(1);
        layout.sync(&graph(3, 1));
        let first = layout.positions().next().map(|(key, at)| (key.clone(), at));
        let (key, at) = first.unwrap();
        assert_eq!(
            layout.node_at(Point::new(at.x + 1.0, at.y), 4.0),
            Some(&key)
        );
        assert_eq!(layout.node_at(Point::new(at.x + 1e5, at.y), 4.0), None);
    }
}
