//! A seeded, deterministic force-directed layout: every pair of nodes repels, every edge pulls
//! its ends towards a rest length, every node is drawn gently to the centre, and the motion is
//! damped and cools, so it settles. Above [`BARNES_HUT_ABOVE`] nodes, repulsion is
//! approximated with a Barnes–Hut quadtree. Grouped ([`Layout::set_grouping`]), each node is
//! also drawn towards its group's centre, and the groups push each other apart. Given targets
//! ([`Layout::set_targets`], a mind map's places), nodes ease towards them instead of moving
//! by force.
//!
//! Only `f32` arithmetic in a fixed order is used, never trigonometry or a hash map's order, so
//! the same seed and the same calls give the same positions on every platform.

use std::collections::{BTreeMap, HashMap};

use crate::{GraphKey, GraphSnapshot, GroupBy, Groups};

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
/// With targets: the part of the way to its target a node goes in a step.
const EASE: f32 = 0.25;
/// Barnes–Hut's accuracy: a cell is one body when its width over its distance is below this.
const THETA: f32 = 0.9;
const MAX_TREE_DEPTH: u8 = 24;
/// Grouped: how hard a node is drawn towards its group's centre, and how hard each pair of
/// groups pushes apart, as if every node of one pushed every node of the other six times more.
const GROUP_PULL: f32 = 0.1;
const GROUP_REPULSION: f32 = 6.0 * REPULSION;

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
    grouping: Option<GroupBy>,
    /// Each node's group, by place, and how many groups there are; empty while not grouped.
    groups: Vec<usize>,
    group_count: usize,
    /// Where each node is drawn to instead of by force, by key and then by place; `None` while
    /// laid out by force.
    targets: Option<HashMap<GraphKey, Point>>,
    placed_targets: Vec<Option<Point>>,
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
            grouping: None,
            groups: Vec::new(),
            group_count: 0,
            targets: None,
            placed_targets: Vec::new(),
        }
    }

    /// Eases each node towards its place in `targets` rather than moving it by force, a node
    /// with no place staying where it is; `None` goes back to the forces. Either way the layout
    /// wakes. The targets are kept for every snapshot `sync` takes.
    pub fn set_targets(&mut self, targets: Option<HashMap<GraphKey, Point>>) {
        self.targets = targets;
        self.place_targets();
        self.heat = 1.0;
    }

    /// Whether the nodes are eased towards targets.
    #[must_use]
    pub fn has_targets(&self) -> bool {
        self.targets.is_some()
    }

    fn place_targets(&mut self) {
        self.placed_targets = match &self.targets {
            Some(targets) => self
                .keys
                .iter()
                .map(|key| targets.get(key).copied())
                .collect(),
            None => Vec::new(),
        };
    }

    /// Groups the nodes by `by`, read from `snapshot` now and from every snapshot `sync` takes,
    /// or with `None` stops. Wakes the layout if that changes anything.
    pub fn set_grouping(&mut self, by: Option<GroupBy>, snapshot: &GraphSnapshot) {
        let before = (self.grouping, std::mem::take(&mut self.groups));
        self.grouping = by;
        self.assign_groups(snapshot);
        if (self.grouping, &self.groups) != (before.0, &before.1) {
            self.wake();
        }
    }

    /// What groups the nodes, if anything.
    #[must_use]
    pub fn grouping(&self) -> Option<GroupBy> {
        self.grouping
    }

    fn assign_groups(&mut self, snapshot: &GraphSnapshot) {
        match self.grouping {
            // The layout holds its nodes in the snapshot's order.
            Some(by) if snapshot.nodes.len() == self.keys.len() => {
                let groups = Groups::of(snapshot, by);
                self.group_count = groups.names.len();
                self.groups = groups.of;
            }
            _ => {
                self.group_count = 0;
                self.groups.clear();
            }
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
        self.assign_groups(snapshot);
        self.place_targets();
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
        if self.targets.is_some() {
            return self.ease(dt);
        }
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

    /// Moves each node with a target part of the way there.
    fn ease(&mut self, dt: f32) -> bool {
        if self.heat < COLD {
            return false;
        }
        let share = (EASE * dt).min(1.0);
        let mut largest = 0.0_f32;
        for (at, target) in self.placed_targets.iter().enumerate() {
            let Some(target) = target else {
                continue;
            };
            if self.pinned[at] {
                continue;
            }
            let position = &mut self.positions[at];
            let (dx, dy) = (target.x - position.x, target.y - position.y);
            let distance = (dx * dx + dy * dy).sqrt();
            if distance < SETTLED {
                *position = *target;
            } else {
                position.x += dx * share;
                position.y += dy * share;
                largest = largest.max(distance * share);
            }
            self.velocities[at] = Point::default();
        }
        if largest < SETTLED {
            // Close enough: put every node exactly in its place.
            for (at, target) in self.placed_targets.iter().enumerate() {
                if let (Some(target), false) = (target, self.pinned[at]) {
                    self.positions[at] = *target;
                }
            }
            self.heat = 0.0;
        } else {
            self.heat = 1.0;
        }
        self.heat >= COLD
    }

    /// Steps until the layout settles, or `limit` steps have been taken, for a drawing with no
    /// frames to wait for.
    pub fn settle(&mut self, limit: usize) {
        for _ in 0..limit {
            if !self.step(1.0) {
                break;
            }
        }
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
        let (groups, group_count) = (std::mem::take(&mut self.groups), self.group_count);
        let grouping = self.grouping.take();
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
        // The same nodes, so the same groups.
        (self.grouping, self.groups, self.group_count) = (grouping, groups, group_count);
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
        if !self.groups.is_empty() {
            self.group_forces(&mut forces);
        }
        forces
    }

    /// Draws each node towards its group's centre, and pushes each group away from the others
    /// in proportion to both groups' sizes.
    fn group_forces(&self, forces: &mut [Point]) {
        let count = self.group_count;
        let mut centres = vec![Point::default(); count];
        let mut sizes = vec![0.0_f32; count];
        for (&group, position) in self.groups.iter().zip(&self.positions) {
            centres[group].x += position.x;
            centres[group].y += position.y;
            sizes[group] += 1.0;
        }
        for (centre, &size) in centres.iter_mut().zip(&sizes) {
            if size > 0.0 {
                centre.x /= size;
                centre.y /= size;
            }
        }
        let mut pushes = vec![Point::default(); count];
        for g in 0..count {
            for h in g + 1..count {
                if sizes[g] == 0.0 || sizes[h] == 0.0 {
                    continue;
                }
                let push = repel(centres[g], centres[h], GROUP_REPULSION / REPULSION, g, h);
                pushes[g].x += push.x * sizes[h];
                pushes[g].y += push.y * sizes[h];
                pushes[h].x -= push.x * sizes[g];
                pushes[h].y -= push.y * sizes[g];
            }
        }
        for ((force, &group), position) in forces.iter_mut().zip(&self.groups).zip(&self.positions)
        {
            let (centre, push) = (centres[group], pushes[group]);
            force.x += (centre.x - position.x) * GROUP_PULL + push.x;
            force.y += (centre.y - position.y) * GROUP_PULL + push.y;
        }
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

/// A number from the seed and the key, the same on every platform: a path's separators are
/// taken as `/`, so `docs\page.adoc` on Windows starts where `docs/page.adoc` does elsewhere.
fn mix(seed: u64, key: &GraphKey, salt: u64) -> u64 {
    // FNV-1a over the key's bytes, then SplitMix64.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
    let (bytes, path): (&[u8], bool) = match key {
        GraphKey::File(path) => (path.as_os_str().as_encoded_bytes(), true),
        GraphKey::Unresolved(text) | GraphKey::Group(text) => (text.as_bytes(), false),
    };
    for &byte in bytes {
        // `\` is ASCII, never part of a longer character in the encoding.
        let byte = if path && byte == b'\\' { b'/' } else { byte };
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
    use crate::{EdgeKind, GraphEdge, GraphNode, NodeCoordinate, NodeKind};

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

    /// Three components of three modules of 20 pages each, the pages of a module linking to
    /// each other and every fifth page to one in another module, some in another component.
    pub(crate) fn grouped_graph() -> GraphSnapshot {
        let mut nodes = Vec::new();
        for component in ["alpha", "beta", "gamma"] {
            for module in ["ROOT", "admin", "api"] {
                for page in 0..20 {
                    nodes.push(GraphNode {
                        key: key(&format!("{component}/{module}/p{page:02}")),
                        title: String::new(),
                        kind: NodeKind::Page,
                        coordinate: Some(NodeCoordinate {
                            component: component.to_owned(),
                            version: None,
                            module: module.to_owned(),
                            family: "page".to_owned(),
                        }),
                        links_out: 0,
                        links_in: 0,
                    });
                }
            }
        }
        nodes.sort_by(|left, right| left.key.cmp(&right.key));
        let mut edges = Vec::new();
        for (at, node) in nodes.iter().enumerate() {
            let (module, page) = (at / 20, at % 20);
            let mut targets = vec![module * 20 + (page + 1) % 20, module * 20 + (page + 3) % 20];
            if page % 5 == 0 {
                targets.push(((module + 1 + page / 5) % 9) * 20 + page);
            }
            for to in targets {
                edges.push(GraphEdge {
                    from: node.key.clone(),
                    to: nodes[to].key.clone(),
                    kind: EdgeKind::Xref,
                    count: 1,
                });
            }
        }
        edges.sort();
        edges.dedup();
        GraphSnapshot {
            revision: 1,
            nodes,
            edges,
        }
    }

    /// Whether every node is nearer the centre of its own group than of any other.
    pub(crate) fn groups_kept_together(layout: &Layout, snapshot: &GraphSnapshot) -> bool {
        let groups = Groups::of(snapshot, GroupBy::Module);
        let mut centres = vec![(0.0_f32, 0.0_f32, 0.0_f32); groups.names.len()];
        for (node, &group) in snapshot.nodes.iter().zip(&groups.of) {
            let at = layout.position(&node.key).unwrap();
            centres[group].0 += at.x;
            centres[group].1 += at.y;
            centres[group].2 += 1.0;
        }
        let centres: Vec<Point> = centres
            .into_iter()
            .map(|(x, y, n)| Point::new(x / n, y / n))
            .collect();
        snapshot.nodes.iter().zip(&groups.of).all(|(node, &own)| {
            let at = layout.position(&node.key).unwrap();
            let distance = |centre: Point| Point::new(at.x - centre.x, at.y - centre.y).length();
            let mine = distance(centres[own]);
            centres
                .iter()
                .enumerate()
                .all(|(group, &centre)| group == own || mine < distance(centre))
        })
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
    fn grouped_layout_keeps_modules_together() {
        let snapshot = grouped_graph();
        let mut ungrouped = Layout::new(4);
        ungrouped.sync(&snapshot);
        ungrouped.settle(2_000);
        assert!(
            !groups_kept_together(&ungrouped, &snapshot),
            "the links alone do not keep the modules apart"
        );

        let mut layout = Layout::new(4);
        layout.sync(&snapshot);
        layout.set_grouping(Some(GroupBy::Module), &snapshot);
        assert_eq!(layout.grouping(), Some(GroupBy::Module));
        layout.settle(2_000);
        assert!(!layout.is_moving(), "a grouped layout settles");
        assert!(groups_kept_together(&layout, &snapshot));

        // A reset keeps the grouping, and so does a sync of a changed snapshot.
        layout.reset();
        layout.settle(2_000);
        assert!(groups_kept_together(&layout, &snapshot));
        let mut fewer = snapshot.clone();
        fewer.nodes.pop();
        let gone = fewer.nodes.len();
        fewer.edges.retain(|edge| {
            edge.from != snapshot.nodes[gone].key && edge.to != snapshot.nodes[gone].key
        });
        layout.sync(&fewer);
        layout.settle(2_000);
        assert!(groups_kept_together(&layout, &fewer));

        layout.set_grouping(None, &fewer);
        assert!(layout.is_moving(), "ungrouping wakes the layout");
    }

    #[test]
    fn grouped_layout_is_deterministic() {
        let snapshot = grouped_graph();
        let run = || {
            let mut layout = Layout::new(9);
            layout.sync(&snapshot);
            layout.set_grouping(Some(GroupBy::Component), &snapshot);
            layout.settle(300);
            layout.positions().map(|(_, at)| at).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn targets_draw_nodes_to_their_places() {
        let snapshot = graph(10, 2);
        let mut layout = Layout::new(6);
        layout.sync(&snapshot);
        let places: HashMap<GraphKey, Point> = snapshot
            .nodes
            .iter()
            .enumerate()
            .skip(1)
            .map(|(at, node)| (node.key.clone(), Point::new(at as f32 * 50.0, -20.0)))
            .collect();
        let first = snapshot.nodes[0].key.clone();
        let stays = layout.position(&first).unwrap();
        let pinned = snapshot.nodes[1].key.clone();
        layout.pin(&pinned, Point::new(3.0, 4.0));
        layout.set_targets(Some(places.clone()));
        assert!(layout.has_targets() && layout.is_moving());
        layout.settle(200);
        assert!(!layout.is_moving(), "easing settles");
        for (key, place) in &places {
            if *key != pinned {
                assert_eq!(layout.position(key), Some(*place));
            }
        }
        assert_eq!(
            layout.position(&first),
            Some(stays),
            "a node with no place stays"
        );
        assert_eq!(layout.position(&pinned), Some(Point::new(3.0, 4.0)));

        // A sync keeps the targets; none puts the forces back.
        layout.sync(&snapshot);
        assert!(layout.has_targets());
        layout.set_targets(None);
        assert!(!layout.has_targets() && layout.is_moving());
        layout.step(1.0);
        assert_ne!(layout.position(&first), Some(stays));
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
