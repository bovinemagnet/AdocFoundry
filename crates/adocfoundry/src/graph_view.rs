//! The link graph, drawn: the graph window's view of the whole workspace, and the Graph sidebar
//! panel's view of the active page and its neighbours. Both draw the engine's
//! [`GraphSnapshot`] on one canvas, laid out by `adoc-graph`'s [`Layout`], which is stepped once
//! a frame until it settles.
//!
//! Dragging the background pans, the wheel or a pinch zooms around the pointer, and dragging a
//! node pins it (released with Alt held, it is let go). Hovering a node highlights it and its
//! neighbours. A click selects a node and opens its page as a preview, a double click keeps it
//! open, and both go through `navigate.go_to` as every other way of opening a page does.
//!
//! The window may group the nodes by module or component version (`[graph] group`): the layout
//! draws each group together, a soft hull is drawn behind it, a click on the hull's label
//! collapses the group into one node, and a click on that node expands it again.
//!
//! Both may lay the graph out as a mind map instead (`[graph] layout`): a tree from one root,
//! left to right or radial, its nodes eased to `adoc-graph`'s [`MindMap`] places. A navigation
//! file's tree is its navigation; a page's reaches the chosen depth. Links outside the tree
//! are drawn faintly, a node's disclosure closes and opens its branch, and a double click, or
//! Root Mind Map Here, roots the map at another node.
//!
//! Where Git has read the pages' history, nodes may be coloured by how long ago they changed,
//! pages unchanged for `[graph] stale_after_days` are ringed, the Stale filter shows only them
//! and their neighbours, and the side panel says when the selected page last changed.

use std::{
    cell::Cell,
    cell::RefCell,
    collections::BTreeSet,
    path::{Path, PathBuf},
    rc::Rc,
};

use adoc_engine::{
    commands::ids, group_name, CommandArgs, CommandOutcome, DocumentId, EdgeKind, Engine,
    EngineEvent, GraphColour, GraphFilter, GraphKey, GraphLayout, GraphNodeKind, GraphScope,
    GraphSnapshot, GroupBy, Groups, LastChanged, Layout, LinkStyle, MindMap, NavPlace,
    NavigationPosition, Neighbour, Palette, PathStep, Point as LayoutPoint, TabItem, TextEdit,
    TreeStyle,
};
use gpui::{
    actions, canvas, div, point, prelude::FluentBuilder as _, px, quad, size, App, AppContext as _,
    BorderStyle, Bounds, Context, Corners, Entity, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, PathBuilder, PinchEvent, Pixels, Render, ScrollEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, TextAlign, TextRun,
    WeakEntity, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    v_flex, ActiveTheme as _, IconName, Selectable as _, Sizable as _,
};

use crate::{
    app::AppState,
    workspace_view::{item_args, WorkspaceView},
};

actions!(adocfoundry, [NextNode, PreviousNode, OpenNode, ClearPath]);

/// The active page and the edit linking it to the file at `path` at its caret, or at its end
/// before any caret has been reported.
fn link_from_active(
    engine: &Engine,
    path: &Path,
    style: LinkStyle,
) -> Option<(DocumentId, TextEdit)> {
    let id = engine.active()?;
    let offset = engine
        .caret(id)
        .or_else(|| engine.document(id).map(|document| document.text.len()))?;
    engine
        .link_edit(id, offset, path, style)
        .map(|edit| (id, edit))
}

/// The key context of the graph's canvas, where Tab, Shift-Tab and Enter move between nodes
/// and open one rather than moving focus, and Esc clears a path.
pub const GRAPH_CONTEXT: &str = "Graph";

/// Binds the canvas's keys.
pub fn bind(cx: &mut App) {
    let context = Some(GRAPH_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("tab", NextNode, context),
        KeyBinding::new("shift-tab", PreviousNode, context),
        KeyBinding::new("enter", OpenNode, context),
        KeyBinding::new("escape", ClearPath, context),
    ]);
}

/// The seed every layout starts from, so a graph opens the same way each time.
const SEED: u64 = 7;
/// The smallest and largest zoom, in pixels per layout unit.
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 6.0;
/// Above this zoom every node is labelled; below it only the hovered, the selected, their
/// neighbours and search matches are.
const LABEL_ZOOM: f32 = 1.1;
/// How far, in pixels, the pointer may be from a node's edge and still be on it.
const HIT_SLOP: f32 = 4.0;
/// How far, in pixels, a press moves before it is a drag rather than a click.
const DRAG_START: f32 = 3.0;
/// The space kept around the graph when it is fitted to the canvas, in pixels.
const FIT_MARGIN: f32 = 40.0;
/// The space between a group's hull and its outermost nodes, in pixels.
const HULL_PAD: f32 = 22.0;
/// A collapsed group's radius, at zoom 1.
const GROUP_RADIUS: f32 = 14.0;
/// A mind map node's disclosure: its radius, and its gap from the node's edge, in pixels.
const FOLD_RADIUS: f32 = 5.0;
const FOLD_GAP: f32 = 4.0;

/// Where a graph view is shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphMode {
    /// The graph window: the workspace, or the active page's neighbourhood, with the whole
    /// toolbar and a side panel of the selected node's links.
    Window,
    /// The Graph sidebar panel: the active page's neighbourhood, a reduced toolbar, and the
    /// page's place in the navigation and its links below.
    Panel,
}

/// What a press on the canvas is doing.
#[derive(Clone, Debug)]
enum Drag {
    /// Panning, the pointer last at this window position.
    Pan(gpui::Point<Pixels>),
    /// On a node: a click until it moves, then a drag that pins it.
    Node {
        key: GraphKey,
        start: gpui::Point<Pixels>,
        moved: bool,
        clicks: usize,
    },
}

/// One node as the canvas draws it.
#[derive(Clone)]
struct DrawnNode {
    at: LayoutPoint,
    radius: f32,
    kind: GraphNodeKind,
    colour: Hsla,
    label: Option<SharedString>,
    dim: bool,
    selected: bool,
    /// In a mind map, a node with children has a disclosure: whether its branch is closed.
    folded: Option<bool>,
    /// Unchanged for longer than `[graph] stale_after_days`, and ringed.
    stale: bool,
}

/// One edge as the canvas draws it, by its ends' places among the nodes.
#[derive(Clone, Copy)]
struct DrawnEdge {
    from: usize,
    to: usize,
    kind: EdgeKind,
    count: usize,
    unresolved: bool,
    dim: bool,
}

/// A group's hull as the canvas draws it: the box around its nodes, in layout units.
#[derive(Clone)]
struct DrawnHull {
    low: LayoutPoint,
    high: LayoutPoint,
    label: SharedString,
    colour: Hsla,
}

/// One group's nodes: its name, the box around them in layout units, and the place of its
/// first node, whose colour the hull takes.
struct Hull {
    name: String,
    low: LayoutPoint,
    high: LayoutPoint,
    first: usize,
}

/// A hull's box on the canvas, and the band at its top where its label is, in window pixels.
fn hull_bounds(
    low: LayoutPoint,
    high: LayoutPoint,
    zoom: f32,
    pan: (f32, f32),
    centre: gpui::Point<Pixels>,
    band: f32,
) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let x = |at: f32| centre.x + px(at * zoom + pan.0);
    let y = |at: f32| centre.y + px(at * zoom + pan.1);
    let top_left = point(x(low.x) - px(HULL_PAD), y(low.y) - px(HULL_PAD + band));
    let bottom_right = point(x(high.x) + px(HULL_PAD), y(high.y) + px(HULL_PAD));
    let body = Bounds::from_corners(top_left, bottom_right);
    let label = Bounds::new(top_left, size(body.size.width, px(band)));
    (body, label)
}

/// Everything the canvas paints, worked out in `render`.
struct Drawing {
    nodes: Vec<DrawnNode>,
    edges: Vec<DrawnEdge>,
    hulls: Vec<DrawnHull>,
    /// The height of a hull's label band, in pixels.
    band: f32,
    zoom: f32,
    pan: (f32, f32),
    edge: Hsla,
    nav_edge: Hsla,
    label: Hsla,
    ring: Hsla,
    stale_ring: Hsla,
}

pub struct GraphView {
    app: Entity<AppState>,
    workspace: WeakEntity<WorkspaceView>,
    mode: GraphMode,
    focus_handle: FocusHandle,
    /// The window shows the workspace unless switched to the active page's neighbourhood.
    local: bool,
    depth: u8,
    /// The page a local graph is around: the active page.
    centre: Option<PathBuf>,
    filter: GraphFilter,
    colour_by: GraphColour,
    /// What the window groups the nodes by, and the groups drawn as one node each.
    group_by: Option<GroupBy>,
    collapsed: BTreeSet<String>,
    /// The height of a hull's label band, in pixels, from the rem size when last drawn.
    label_band: f32,
    /// Only stale pages and their neighbours are shown.
    stale_only: bool,
    /// How the graph is laid out; as a mind map, the root chosen, the tree shown and the nodes
    /// whose branches are closed.
    arrangement: GraphLayout,
    mind_root: Option<GraphKey>,
    tree: Option<MindMap>,
    folded: BTreeSet<GraphKey>,
    search: Option<Entity<InputState>>,
    query: String,
    snapshot: GraphSnapshot,
    /// Each edge's ends, by their places among the snapshot's nodes.
    edge_places: Vec<(usize, usize)>,
    layout: Layout,
    zoom: f32,
    /// Where the layout's origin is drawn, from the canvas's centre, in pixels.
    pan: (f32, f32),
    /// The view is fitted to the canvas as the layout settles, until the user pans or zooms.
    fitting: bool,
    /// The canvas's bounds in the window when it was last painted.
    viewport: Rc<Cell<Bounds<Pixels>>>,
    hovered: Option<GraphKey>,
    selected: Option<GraphKey>,
    /// The nodes a path is asked between, and the path found, if any.
    path_ends: Option<(GraphKey, GraphKey)>,
    path: Option<Vec<PathStep>>,
    /// The title of a path given rather than searched for: an impact's dependency path (#311).
    trail: Option<SharedString>,
    /// Whether a path follows links only in their direction, as a reader does.
    directed: bool,
    drag: Option<Drag>,
    /// The node a context menu was opened on.
    menu_target: Rc<RefCell<Option<GraphKey>>>,
    _subscriptions: Vec<Subscription>,
}

impl GraphView {
    pub fn new(
        app: Entity<AppState>,
        workspace: WeakEntity<WorkspaceView>,
        mode: GraphMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = app.read(cx).engine.settings().graph.clone();
        let mut subscriptions = vec![cx.subscribe(&app, Self::on_engine_event)];
        if mode == GraphMode::Window {
            subscriptions.push(cx.observe_window_bounds(window, |this, window, cx| {
                crate::app::record_graph_window(&this.app, window, cx);
            }));
        }
        let search = (mode == GraphMode::Window).then(|| {
            let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search the graph"));
            subscriptions.push(cx.subscribe(&search, |this, search, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = search.read(cx).value().trim().to_lowercase();
                    cx.notify();
                }
            }));
            search
        });
        let mut view = Self {
            app,
            workspace,
            mode,
            focus_handle: cx.focus_handle(),
            local: mode == GraphMode::Panel,
            depth: settings.depth.clamp(1, 3),
            centre: None,
            filter: settings.filter(),
            colour_by: settings.colour_by,
            group_by: settings.group.filter(|_| mode == GraphMode::Window),
            collapsed: BTreeSet::new(),
            label_band: 20.0,
            stale_only: false,
            arrangement: settings.layout,
            mind_root: None,
            tree: None,
            folded: BTreeSet::new(),
            search,
            query: String::new(),
            snapshot: GraphSnapshot::default(),
            edge_places: Vec::new(),
            layout: Layout::new(SEED),
            zoom: 1.0,
            pan: (0.0, 0.0),
            fitting: true,
            viewport: Rc::default(),
            hovered: None,
            selected: None,
            path_ends: None,
            path: None,
            trail: None,
            directed: true,
            drag: None,
            menu_target: Rc::default(),
            _subscriptions: subscriptions,
        };
        view.layout.set_grouping(view.group_by, &view.snapshot);
        view.follow_active(cx);
        view.refresh(cx);
        view
    }

    fn on_engine_event(
        &mut self,
        _: Entity<AppState>,
        event: &EngineEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            EngineEvent::GraphChanged { .. } => self.refresh(cx),
            EngineEvent::NavigationChanged | EngineEvent::DocumentMoved { .. }
                if self.follow_active(cx) && self.local =>
            {
                self.refresh(cx);
            }
            _ => {}
        }
    }

    /// Takes the active page as the centre of a local graph. Returns whether it changed.
    fn follow_active(&mut self, cx: &mut Context<Self>) -> bool {
        let engine = &self.app.read(cx).engine;
        let centre = engine
            .active()
            .and_then(|id| engine.document(id))
            .filter(|document| document.asciidoc && !document.untitled)
            .map(|document| document.path.clone());
        if centre == self.centre {
            return false;
        }
        self.centre = centre;
        if self.mode == GraphMode::Panel {
            // The panel's mind map is rooted at the active page.
            self.mind_root = None;
            self.folded.clear();
        }
        true
    }

    /// What the view shows: the workspace, or the neighbourhood of the page a local graph is
    /// around; `None` for a local graph with no page.
    fn scope(&self) -> Option<GraphScope> {
        if !self.local {
            return Some(GraphScope::Workspace);
        }
        self.centre.as_ref().map(|path| GraphScope::Local {
            path: path.clone(),
            depth: self.depth,
        })
    }

    /// Asks the engine for the graph again and fits the layout to it.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.scope() else {
            self.set_snapshot(GraphSnapshot::default());
            self.layout.sync(&self.snapshot);
            self.update_path(cx);
            cx.notify();
            return;
        };
        let snapshot = self.app.read(cx).engine.graph(&scope, &self.filter);
        let snapshot = match (self.arrangement, self.group_by) {
            (GraphLayout::MindMap(_), _) => self.mind_map_of(snapshot),
            (GraphLayout::Force, Some(by)) => snapshot.collapse(by, &self.collapsed),
            (GraphLayout::Force, None) => snapshot,
        };
        let snapshot = if self.stale_only {
            self.only_stale(snapshot, cx)
        } else {
            snapshot
        };
        let recentre = self.local && snapshot.nodes.len() != self.snapshot.nodes.len();
        self.set_snapshot(snapshot);
        self.layout.sync(&self.snapshot);
        match (self.arrangement, &self.tree) {
            (GraphLayout::MindMap(style), Some(tree)) => {
                self.layout
                    .set_targets(Some(tree.positions(style, &self.folded)));
            }
            _ if self.layout.has_targets() => self.layout.set_targets(None),
            _ => {}
        }
        if recentre {
            self.fitting = true;
        }
        for key in [&mut self.hovered, &mut self.selected] {
            if key
                .as_ref()
                .is_some_and(|key| self.snapshot.node(key).is_none())
            {
                *key = None;
            }
        }
        self.update_path(cx);
        cx.notify();
    }

    // --- paths -------------------------------------------------------------------------------

    /// Shows the shortest path from `from` to `to`: its nodes numbered and the rest dimmed, and
    /// its steps listed in the window's side panel.
    fn show_path(&mut self, from: GraphKey, to: GraphKey, cx: &mut Context<Self>) {
        self.trail = None;
        self.path_ends = Some((from, to));
        self.update_path(cx);
        cx.notify();
    }

    /// Shows `steps`, a path the caller worked out (an impact's dependency path, which is not a
    /// shortest path), as a path is shown, under `title`, and brings its last node into view.
    pub fn show_trail(
        &mut self,
        title: impl Into<SharedString>,
        steps: Vec<PathStep>,
        cx: &mut Context<Self>,
    ) {
        self.path_ends = None;
        self.trail = Some(title.into());
        let last = steps.last().map(|step| step.key.clone());
        self.path = Some(steps);
        if let Some(last) = last {
            self.reveal(&last, cx);
        }
        cx.notify();
    }

    /// The title and steps of the path given to [`GraphView::show_trail`], for tests.
    #[cfg(test)]
    pub(crate) fn trail_for_test(&self) -> Option<(SharedString, Vec<PathStep>)> {
        Some((self.trail.clone()?, self.path.clone()?))
    }

    /// Finds the path between the ends asked for again, over what the view shows; a path given
    /// is kept as it was given.
    fn update_path(&mut self, cx: &App) {
        if self.trail.is_some() {
            return;
        }
        self.path = match (&self.path_ends, self.scope()) {
            (Some((from, to)), Some(scope)) => {
                self.app
                    .read(cx)
                    .engine
                    .graph_path(&scope, &self.filter, from, to, self.directed)
            }
            _ => None,
        };
    }

    fn clear_path(&mut self, cx: &mut Context<Self>) {
        self.trail = None;
        self.path_ends = None;
        self.path = None;
        cx.notify();
    }

    fn on_clear_path(&mut self, _: &ClearPath, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_path(cx);
    }

    /// The workspace window whose panels the context menu reaches.
    pub fn set_workspace(&mut self, workspace: WeakEntity<WorkspaceView>) {
        self.workspace = workspace;
    }

    /// Lays the graph out again from its start, every node unpinned.
    pub fn reset_layout(&mut self, cx: &mut Context<Self>) {
        self.layout.reset();
        self.fitting = true;
        cx.notify();
    }

    /// Selects `key` and brings it into the middle of the canvas.
    pub fn reveal(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        if self.snapshot.node(key).is_none() && self.local {
            // A page outside the neighbourhood: show the whole workspace.
            self.local = false;
            self.refresh(cx);
        }
        if let Some(at) = self.layout.position(key) {
            self.selected = Some(key.clone());
            self.fitting = false;
            self.pan = (-at.x * self.zoom, -at.y * self.zoom);
            cx.notify();
        }
    }

    fn set_filter(&mut self, change: impl FnOnce(&mut GraphFilter), cx: &mut Context<Self>) {
        change(&mut self.filter);
        self.refresh(cx);
    }

    // --- ages ----------------------------------------------------------------------------------

    /// The time before which a page counts as stale, in seconds since the Unix epoch.
    fn stale_before(&self, cx: &App) -> i64 {
        let days = i64::from(self.app.read(cx).engine.settings().graph.stale_after_days);
        now_seconds() - days * DAY
    }

    /// `snapshot` cut down to its stale pages and their neighbours.
    fn only_stale(&self, snapshot: GraphSnapshot, cx: &App) -> GraphSnapshot {
        let before = self.stale_before(cx);
        let stale: std::collections::HashSet<&GraphKey> = snapshot
            .nodes
            .iter()
            .filter(|node| is_stale(node, before))
            .map(|node| &node.key)
            .collect();
        let mut keep: std::collections::HashSet<GraphKey> =
            stale.iter().map(|key| (*key).clone()).collect();
        for edge in &snapshot.edges {
            if stale.contains(&edge.from) || stale.contains(&edge.to) {
                keep.insert(edge.from.clone());
                keep.insert(edge.to.clone());
            }
        }
        snapshot.restrict(|key| keep.contains(key))
    }

    fn toggle_stale(&mut self, cx: &mut Context<Self>) {
        self.stale_only = !self.stale_only;
        self.fitting = true;
        self.refresh(cx);
    }

    /// The colours the Colour button moves through: by age only where pages have ages.
    fn next_colour(&self, cx: &App) -> GraphColour {
        let ages = self.app.read(cx).engine.has_page_ages();
        match self.colour_by {
            GraphColour::Component => GraphColour::Module,
            GraphColour::Module => GraphColour::Family,
            GraphColour::Family if ages => GraphColour::Age,
            GraphColour::Family | GraphColour::Age => GraphColour::Component,
        }
    }

    // --- mind maps ---------------------------------------------------------------------------

    /// The tree of `snapshot` from the mind map's root, kept in `tree`, and the snapshot cut
    /// down to the nodes it shows. The root is the one chosen, else the active page, else the
    /// first navigation file, else the first node.
    fn mind_map_of(&mut self, snapshot: GraphSnapshot) -> GraphSnapshot {
        let centre = self
            .centre
            .as_ref()
            .map(|path| GraphKey::File(std::sync::Arc::from(adoc_engine::normalize_path(path))));
        let root = [self.mind_root.clone(), centre]
            .into_iter()
            .flatten()
            .find(|key| snapshot.node(key).is_some())
            .or_else(|| {
                snapshot
                    .nodes
                    .iter()
                    .find(|node| node.kind == GraphNodeKind::Nav)
                    .or(snapshot.nodes.first())
                    .map(|node| node.key.clone())
            });
        self.tree = root.and_then(|root| MindMap::build(&snapshot, &root, Some(self.depth)));
        match &self.tree {
            Some(tree) => {
                let shown = tree.visible(&self.folded);
                snapshot.restrict(|key| shown.contains(key))
            }
            None => snapshot,
        }
    }

    /// Lays out by force, as a mind map, then radially. Entering a mind map roots it at the
    /// selected node, if any.
    fn cycle_arrangement(&mut self, cx: &mut Context<Self>) {
        self.arrangement = match self.arrangement {
            GraphLayout::Force => GraphLayout::MindMap(TreeStyle::Horizontal),
            GraphLayout::MindMap(TreeStyle::Horizontal) => GraphLayout::MindMap(TreeStyle::Radial),
            GraphLayout::MindMap(TreeStyle::Radial) => GraphLayout::Force,
        };
        if self.arrangement == GraphLayout::MindMap(TreeStyle::Horizontal) {
            self.mind_root = self.selected.clone();
            self.folded.clear();
        }
        if self.arrangement == GraphLayout::Force {
            self.tree = None;
        }
        self.fitting = true;
        self.refresh(cx);
    }

    /// Roots the mind map at `key`, laying out as one if not already, every branch open.
    fn root_mind_map(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        if self.arrangement == GraphLayout::Force {
            self.arrangement = GraphLayout::MindMap(TreeStyle::Horizontal);
        }
        self.mind_root = Some(key.clone());
        self.selected = Some(key.clone());
        self.folded.clear();
        self.fitting = true;
        self.refresh(cx);
    }

    /// Closes the branch under `key`, or opens it again.
    fn toggle_fold(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        if !self.folded.remove(key) {
            self.folded.insert(key.clone());
        }
        self.refresh(cx);
    }

    /// The middle of `key`'s disclosure, in window coordinates, if it has one: to the left of
    /// the node, as its label is to the right.
    fn fold_centre(&self, key: &GraphKey) -> Option<gpui::Point<Pixels>> {
        let tree = self.tree.as_ref()?;
        if tree.children(key).is_empty() {
            return None;
        }
        let node = self.snapshot.node(key)?;
        let at = self.layout.position(key)?;
        let radius =
            adoc_engine::node_radius(node.links_in + node.links_out) * self.zoom.clamp(0.6, 1.6);
        let centre = self.viewport.get().center();
        Some(point(
            centre.x + px(at.x * self.zoom + self.pan.0 - radius - FOLD_GAP - FOLD_RADIUS),
            centre.y + px(at.y * self.zoom + self.pan.1),
        ))
    }

    /// The node whose disclosure is under the window position `at`.
    fn fold_at(&self, at: gpui::Point<Pixels>) -> Option<GraphKey> {
        let tree = self.tree.as_ref()?;
        tree.keys()
            .filter(|key| self.snapshot.node(key).is_some())
            .find(|key| {
                self.fold_centre(key).is_some_and(|centre| {
                    let (dx, dy) = (f32::from(at.x - centre.x), f32::from(at.y - centre.y));
                    dx * dx + dy * dy <= (FOLD_RADIUS + HIT_SLOP).powi(2)
                })
            })
            .cloned()
    }

    // --- groups ------------------------------------------------------------------------------

    /// Groups by nothing, by module, then by component version, every group expanded.
    fn cycle_grouping(&mut self, cx: &mut Context<Self>) {
        self.group_by = match self.group_by {
            None => Some(GroupBy::Module),
            Some(GroupBy::Module) => Some(GroupBy::Component),
            Some(GroupBy::Component) => None,
        };
        self.collapsed.clear();
        self.refresh(cx);
        self.layout.set_grouping(self.group_by, &self.snapshot);
        cx.notify();
    }

    /// Draws the group `name` as one node, where its nodes were.
    fn collapse_group(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(by) = self.group_by else {
            return;
        };
        let members: Vec<LayoutPoint> = self
            .snapshot
            .nodes
            .iter()
            .filter(|node| group_name(node, by) == name)
            .filter_map(|node| self.layout.position(&node.key))
            .collect();
        self.collapsed.insert(name.to_owned());
        self.refresh(cx);
        if !members.is_empty() {
            let count = members.len() as f32;
            let (x, y) = members
                .iter()
                .fold((0.0, 0.0), |(x, y), at| (x + at.x, y + at.y));
            self.move_node(
                &GraphKey::Group(name.to_owned()),
                LayoutPoint::new(x / count, y / count),
            );
        }
    }

    /// Draws the nodes of the collapsed group `name` again, around where it was.
    fn expand_group(&mut self, name: &str, cx: &mut Context<Self>) {
        let (Some(by), true) = (self.group_by, self.collapsed.remove(name)) else {
            return;
        };
        let at = self.layout.position(&GraphKey::Group(name.to_owned()));
        self.refresh(cx);
        let Some(at) = at else {
            return;
        };
        let members: Vec<GraphKey> = self
            .snapshot
            .nodes
            .iter()
            .filter(|node| node.kind != GraphNodeKind::Group && group_name(node, by) == name)
            .map(|node| node.key.clone())
            .collect();
        // Spread on a small grid, so the repulsion parts them every way.
        let side = (members.len() as f32).sqrt().ceil().max(1.0) as usize;
        for (n, key) in members.iter().enumerate() {
            let offset = |k: usize| (k as f32 - (side as f32 - 1.0) / 2.0) * 8.0;
            self.move_node(
                key,
                LayoutPoint::new(at.x + offset(n % side), at.y + offset(n / side)),
            );
        }
    }

    /// Moves the node `key` to `at`, free to move on from there.
    fn move_node(&mut self, key: &GraphKey, at: LayoutPoint) {
        self.layout.pin(key, at);
        self.layout.unpin(key);
    }

    /// Every group's nodes and the box around them, at `positions`; none while not grouped. A
    /// collapsed group is a node, not a hull.
    fn hulls(&self, positions: &[LayoutPoint]) -> Vec<Hull> {
        let (Some(by), GraphLayout::Force) = (self.group_by, self.arrangement) else {
            return Vec::new();
        };
        let groups = Groups::of(&self.snapshot, by);
        let mut hulls: Vec<Option<Hull>> = groups.names.iter().map(|_| None).collect();
        for (place, (node, &group)) in self.snapshot.nodes.iter().zip(&groups.of).enumerate() {
            if node.kind == GraphNodeKind::Group {
                continue;
            }
            let at = positions[place];
            match &mut hulls[group] {
                Some(hull) => {
                    hull.low = LayoutPoint::new(hull.low.x.min(at.x), hull.low.y.min(at.y));
                    hull.high = LayoutPoint::new(hull.high.x.max(at.x), hull.high.y.max(at.y));
                }
                slot => {
                    *slot = Some(Hull {
                        name: groups.names[group].clone(),
                        low: at,
                        high: at,
                        first: place,
                    });
                }
            }
        }
        hulls.into_iter().flatten().collect()
    }

    /// The group whose hull's label is under the window position `at`.
    fn hull_label_at(&self, at: gpui::Point<Pixels>) -> Option<String> {
        let centre = self.viewport.get().center();
        self.hulls(&self.positions())
            .into_iter()
            .rev()
            .find(|hull| {
                let (_, label) = hull_bounds(
                    hull.low,
                    hull.high,
                    self.zoom,
                    self.pan,
                    centre,
                    self.label_band,
                );
                label.contains(&at)
            })
            .map(|hull| hull.name)
    }

    /// Where each node is, in the snapshot's order.
    fn positions(&self) -> Vec<LayoutPoint> {
        // The layout holds its nodes in the snapshot's order.
        if self.layout.len() == self.snapshot.nodes.len() {
            self.layout.positions().map(|(_, at)| at).collect()
        } else {
            self.snapshot
                .nodes
                .iter()
                .map(|node| self.layout.position(&node.key).unwrap_or_default())
                .collect()
        }
    }

    // --- opening -----------------------------------------------------------------------------

    /// Opens `key`'s page through `navigate.go_to`: as a preview, or kept open.
    fn open(&mut self, key: &GraphKey, keep: bool, cx: &mut Context<Self>) {
        let Some(path) = key.path().map(std::path::Path::to_path_buf) else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let opened = app.invoke(
                ids::NAVIGATE_GO_TO,
                CommandArgs::Position(NavigationPosition { path, offset: 0 }),
                cx,
            );
            let item = match opened {
                Ok(CommandOutcome::Opened(id)) => TabItem::Document(id),
                Ok(CommandOutcome::OpenedImage(id)) => TabItem::Image(id),
                _ => return,
            };
            if keep {
                let _ = app.invoke(ids::DOCUMENT_KEEP_OPEN, item_args(item), cx);
            }
        });
    }

    /// Opens `key`'s page in a tab of its own and pins the preview to it, so it shows beside
    /// the page being edited.
    fn open_to_the_side(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        let Some(path) = key.path().map(std::path::Path::to_path_buf) else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let Ok(CommandOutcome::Opened(id)) =
                app.invoke(ids::DOCUMENT_OPEN, CommandArgs::Path(path), cx)
            else {
                return;
            };
            // Pinning toggles: a preview pinned to another page is unpinned first.
            let pinned = app.engine.preview_layout().pinned;
            if pinned.is_some_and(|pinned| pinned != id) {
                let _ = app.invoke(ids::PREVIEW_PIN, CommandArgs::None, cx);
            }
            if app.engine.preview_layout().pinned != Some(id) {
                let _ = app.invoke(ids::PREVIEW_PIN, CommandArgs::Document(id), cx);
            }
        });
    }

    /// Makes the page the unresolved target `key` names, where its links expect it, and opens
    /// it.
    fn create_page(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        let GraphKey::Unresolved(target) = key else {
            return;
        };
        self.app.update(cx, |app, cx| {
            if let Some(request) = app.engine.new_page_for(target) {
                let _ = app.invoke(ids::PAGE_NEW, CommandArgs::NewPage(request), cx);
            }
        });
    }

    /// Makes the file the unresolved include target `key` names, where its includes expect it,
    /// and opens it.
    fn create_partial(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        let GraphKey::Unresolved(target) = key else {
            return;
        };
        self.app.update(cx, |app, cx| {
            if let Some(path) = app.engine.new_partial_for(target) {
                let _ = app.invoke(ids::PAGE_NEW_PARTIAL, CommandArgs::Path(path), cx);
            }
        });
    }

    /// Writes a link to `key`'s file into the active page at its caret, as an xref or an
    /// include.
    fn link_from_active(&mut self, key: &GraphKey, style: LinkStyle, cx: &mut Context<Self>) {
        let Some(path) = key.path().map(Path::to_path_buf) else {
            return;
        };
        self.app.update(cx, |app, cx| {
            if let Some((id, edit)) = link_from_active(&app.engine, &path, style) {
                let _ = app.invoke(
                    ids::EDIT_INSERT_TEXT,
                    CommandArgs::Text {
                        id,
                        range: edit.range,
                        text: edit.replacement,
                    },
                    cx,
                );
            }
        });
    }

    fn find_references(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        if let (Some(path), Some(workspace)) = (key.path(), self.workspace.upgrade()) {
            let path = path.to_path_buf();
            workspace.update(cx, |workspace, cx| workspace.find_references_to(&path, cx));
        }
    }

    fn reveal_in_files(&mut self, key: &GraphKey, cx: &mut Context<Self>) {
        if let (Some(path), Some(workspace)) = (key.path(), self.workspace.upgrade()) {
            let path = path.to_path_buf();
            workspace.update(cx, |workspace, cx| {
                workspace.reveal_path_in_files(&path, cx)
            });
        }
    }

    // --- the canvas's coordinates ------------------------------------------------------------

    /// The layout point under the window position `at`.
    fn to_layout(&self, at: gpui::Point<Pixels>) -> LayoutPoint {
        let bounds = self.viewport.get();
        let centre = bounds.center();
        LayoutPoint::new(
            (f32::from(at.x - centre.x) - self.pan.0) / self.zoom,
            (f32::from(at.y - centre.y) - self.pan.1) / self.zoom,
        )
    }

    /// The window position of the layout point `at`.
    #[cfg(test)]
    fn to_window(&self, at: LayoutPoint) -> gpui::Point<Pixels> {
        let centre = self.viewport.get().center();
        point(
            centre.x + px(at.x * self.zoom + self.pan.0),
            centre.y + px(at.y * self.zoom + self.pan.1),
        )
    }

    /// The node under the window position `at`.
    fn node_under(&self, at: gpui::Point<Pixels>) -> Option<GraphKey> {
        let reach =
            (adoc_engine::node_radius(16) * self.zoom.clamp(0.6, 1.6) + HIT_SLOP) / self.zoom;
        self.layout.node_at(self.to_layout(at), reach).cloned()
    }

    /// Fits the whole layout into the canvas.
    fn fit(&mut self) {
        let viewport = self.viewport.get();
        let Some((low, high)) = self.layout.bounds() else {
            return;
        };
        let (width, height) = (
            f32::from(viewport.size.width),
            f32::from(viewport.size.height),
        );
        if width < 1.0 || height < 1.0 {
            return;
        }
        let spread = ((high.x - low.x).max(1.0), (high.y - low.y).max(1.0));
        let zoom = ((width - 2.0 * FIT_MARGIN) / spread.0)
            .min((height - 2.0 * FIT_MARGIN) / spread.1)
            .clamp(MIN_ZOOM, 2.0);
        self.zoom = zoom;
        self.pan = (
            -(low.x + high.x) / 2.0 * zoom,
            -(low.y + high.y) / 2.0 * zoom,
        );
    }

    fn zoom_around(&mut self, factor: f32, at: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let under = self.to_layout(at);
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let centre = self.viewport.get().center();
        self.pan = (
            f32::from(at.x - centre.x) - under.x * zoom,
            f32::from(at.y - centre.y) - under.y * zoom,
        );
        self.zoom = zoom;
        self.fitting = false;
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let centre = self.viewport.get().center();
        self.zoom_around(factor, centre, cx);
    }

    // --- pointer and keys --------------------------------------------------------------------

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if let Some(key) = self.fold_at(event.position) {
            self.drag = None;
            self.toggle_fold(&key, cx);
            return;
        }
        if self.node_under(event.position).is_none() {
            if let Some(group) = self.hull_label_at(event.position) {
                self.drag = None;
                self.collapse_group(&group, cx);
                return;
            }
        }
        self.drag = Some(match self.node_under(event.position) {
            Some(key) => Drag::Node {
                key,
                start: event.position,
                moved: false,
                clicks: event.click_count,
            },
            None => Drag::Pan(event.position),
        });
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pressed = event.pressed_button == Some(MouseButton::Left);
        match self.drag.as_mut() {
            Some(Drag::Pan(last)) if pressed => {
                let delta = event.position - *last;
                *last = event.position;
                self.pan.0 += f32::from(delta.x);
                self.pan.1 += f32::from(delta.y);
                self.fitting = false;
                cx.notify();
            }
            Some(Drag::Node {
                key, start, moved, ..
            }) if pressed => {
                let travelled = event.position - *start;
                if !*moved && f32::from(travelled.x.abs().max(travelled.y.abs())) < DRAG_START {
                    return;
                }
                *moved = true;
                let key = key.clone();
                let at = self.to_layout(event.position);
                self.layout.pin(&key, at);
                self.fitting = false;
                cx.notify();
            }
            _ => {
                let hovered = self.node_under(event.position);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    cx.notify();
                }
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Node {
                key,
                moved: false,
                clicks,
                ..
            }) => {
                if let GraphKey::Group(name) = &key {
                    self.expand_group(name, cx);
                    return;
                }
                // Shift-click: a path from the selection, which stays where it is.
                let from = self.selected.clone().filter(|selected| *selected != key);
                if let (true, Some(from)) = (event.modifiers.shift, from) {
                    self.show_path(from, key, cx);
                    return;
                }
                if clicks >= 2 && self.arrangement != GraphLayout::Force {
                    self.root_mind_map(&key, cx);
                    return;
                }
                self.selected = Some(key.clone());
                self.open(&key, clicks >= 2, cx);
                cx.notify();
            }
            Some(Drag::Node {
                key, moved: true, ..
            }) if event.modifiers.alt => {
                self.layout.unpin(&key);
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_scroll_wheel(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = f32::from(event.delta.pixel_delta(px(16.)).y);
        if delta != 0.0 {
            self.zoom_around((1.0 + delta * 0.0025).clamp(0.5, 2.0), event.position, cx);
        }
    }

    fn on_pinch(&mut self, event: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_around((1.0 + event.delta).clamp(0.5, 2.0), event.position, cx);
    }

    /// Moves the selection to the next node in title order, or the previous with `back`.
    fn select_next(&mut self, back: bool, cx: &mut Context<Self>) {
        let mut order: Vec<&adoc_engine::GraphNode> = self.snapshot.nodes.iter().collect();
        order.sort_by(|left, right| left.title.cmp(&right.title).then(left.key.cmp(&right.key)));
        if order.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| order.iter().position(|node| node.key == *selected));
        let next = match (at, back) {
            (None, false) => 0,
            (None, true) => order.len() - 1,
            (Some(at), false) => (at + 1) % order.len(),
            (Some(at), true) => (at + order.len() - 1) % order.len(),
        };
        let key = order[next].key.clone();
        self.reveal(&key, cx);
    }

    fn on_next_node(&mut self, _: &NextNode, _: &mut Window, cx: &mut Context<Self>) {
        self.select_next(false, cx);
    }

    fn on_previous_node(&mut self, _: &PreviousNode, _: &mut Window, cx: &mut Context<Self>) {
        self.select_next(true, cx);
    }

    fn on_open_node(&mut self, _: &OpenNode, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(GraphKey::Group(name)) => self.expand_group(&name, cx),
            Some(key) => self.open(&key, false, cx),
            None => {}
        }
    }

    // --- drawing -----------------------------------------------------------------------------

    /// The graph as it is shown, its scope, filters, colours and every node where the layout
    /// has it, pinned or not, drawn as SVG on a light or dark background.
    pub(crate) fn to_svg(&self, dark: bool) -> String {
        let positions: Vec<LayoutPoint> = self
            .snapshot
            .nodes
            .iter()
            .map(|node| self.layout.position(&node.key).unwrap_or_default())
            .collect();
        adoc_engine::to_svg(
            &self.snapshot,
            &positions,
            &adoc_engine::SvgStyle {
                colour_by: self.colour_by,
                dark,
            },
        )
    }

    /// The nodes and edges to paint, emphasised and labelled as the hover, selection, search
    /// and zoom say.
    fn drawing(&self, cx: &App) -> Drawing {
        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        let nodes_shown = &self.snapshot.nodes;
        // Everything below works by place among the nodes: a path compares slowly, and a
        // drawing of thousands of nodes is made every frame while the layout moves.
        let focus = self
            .hovered
            .as_ref()
            .or(self.selected.as_ref())
            .and_then(|key| self.place(key));
        let selected = self.selected.as_ref().and_then(|key| self.place(key));
        let mut near = vec![false; nodes_shown.len()];
        if let Some(focus) = focus {
            near[focus] = true;
            for &(from, to) in &self.edge_places {
                if from == focus || to == focus {
                    near[from] = true;
                    near[to] = true;
                }
            }
        }
        let matches = |node: &adoc_engine::GraphNode| {
            self.query.is_empty()
                || node.title.to_lowercase().contains(&self.query)
                || node
                    .key
                    .path()
                    .is_some_and(|path| path.to_string_lossy().to_lowercase().contains(&self.query))
        };
        let palette = Palette::of(&self.snapshot, self.colour_by);
        let stale_before = self.stale_before(cx);
        // Stale pages are ringed while the colours or the filter are about age.
        let ring_stale = self.colour_by == GraphColour::Age || self.stale_only;
        let all_labels = self.zoom >= LABEL_ZOOM || nodes_shown.len() <= 12;
        let positions = self.positions();
        // A path shown takes the place of the hover and the search: its nodes numbered, its
        // links drawn, and the rest dimmed.
        let path = self.path.as_ref().map(|steps| {
            let mut order = std::collections::HashMap::new();
            let mut links = std::collections::HashSet::new();
            for (at, step) in steps.iter().enumerate() {
                let Some(place) = self.place(&step.key) else {
                    continue;
                };
                order.insert(place, at + 1);
                let before = at
                    .checked_sub(1)
                    .and_then(|before| self.place(&steps[before].key));
                if let Some(before) = before {
                    let (from, to) = if step.backwards {
                        (place, before)
                    } else {
                        (before, place)
                    };
                    links.extend(step.kinds.iter().map(|kind| (from, to, *kind)));
                }
            }
            (order, links)
        });
        let nodes: Vec<DrawnNode> = nodes_shown
            .iter()
            .enumerate()
            .map(|(place, node)| {
                let emphasised = focus.is_none() || near[place];
                let found = matches(node);
                let on_path = path.as_ref().map(|(order, _)| order.get(&place).copied());
                let dim = match on_path {
                    Some(order) => order.is_none(),
                    None => !emphasised || !found,
                };
                let label = match on_path {
                    Some(Some(order)) => Some(format!("{order}. {}", node.title).into()),
                    _ if node.kind == GraphNodeKind::Group => Some(node.title.clone().into()),
                    Some(None) => all_labels.then(|| node.title.clone().into()),
                    None => (all_labels
                        || (focus.is_some() && emphasised)
                        || (!self.query.is_empty() && found)
                        || selected == Some(place))
                    .then(|| node.title.clone().into()),
                };
                DrawnNode {
                    at: positions[place],
                    radius: if node.kind == GraphNodeKind::Group {
                        GROUP_RADIUS
                    } else {
                        adoc_engine::node_radius(node.links_in + node.links_out)
                    },
                    kind: node.kind,
                    colour: node_colour(node, &palette, dark, theme.muted_foreground),
                    label,
                    dim,
                    selected: selected == Some(place),
                    folded: self
                        .tree
                        .as_ref()
                        .filter(|tree| !tree.children(&node.key).is_empty())
                        .map(|_| self.folded.contains(&node.key)),
                    stale: ring_stale && is_stale(node, stale_before),
                }
            })
            .collect();
        let edges = self
            .snapshot
            .edges
            .iter()
            .zip(&self.edge_places)
            .map(|(edge, &(from, to))| {
                let emphasised = match &path {
                    Some((_, links)) => links.contains(&(from, to, edge.kind)),
                    None => focus.is_none_or(|focus| from == focus || to == focus),
                };
                // A mind map's cross-links are drawn faintly.
                let cross = self
                    .tree
                    .as_ref()
                    .is_some_and(|tree| !tree.is_tree_link(&edge.from, &edge.to));
                DrawnEdge {
                    from,
                    to,
                    kind: edge.kind,
                    count: edge.count,
                    unresolved: matches!(edge.to, GraphKey::Unresolved(_)),
                    dim: cross || !emphasised || nodes[from].dim && nodes[to].dim,
                }
            })
            .collect();
        let hulls = self
            .hulls(&positions)
            .into_iter()
            .map(|hull| DrawnHull {
                low: hull.low,
                high: hull.high,
                label: hull.name.into(),
                colour: nodes[hull.first].colour,
            })
            .collect();
        Drawing {
            nodes,
            edges,
            hulls,
            band: self.label_band,
            zoom: self.zoom,
            pan: self.pan,
            edge: theme.muted_foreground.opacity(0.55),
            nav_edge: theme.primary.opacity(0.6),
            label: theme.foreground,
            ring: theme.ring,
            stale_ring: theme.warning,
        }
    }

    fn place(&self, key: &GraphKey) -> Option<usize> {
        self.snapshot
            .nodes
            .binary_search_by(|node| node.key.cmp(key))
            .ok()
    }

    /// Takes `snapshot`, noting where each edge's ends are among its nodes.
    fn set_snapshot(&mut self, snapshot: GraphSnapshot) {
        let places: std::collections::HashMap<&GraphKey, usize> = snapshot
            .nodes
            .iter()
            .enumerate()
            .map(|(place, node)| (&node.key, place))
            .collect();
        // A snapshot's edges always join two of its nodes.
        self.edge_places = snapshot
            .edges
            .iter()
            .map(|edge| (places[&edge.from], places[&edge.to]))
            .collect();
        self.snapshot = snapshot;
    }

    fn render_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.layout.is_moving() {
            self.layout.step(1.0);
            window.request_animation_frame();
        }
        if self.fitting {
            self.fit();
        }
        // A label and its margins, so the band follows `[appearance] font_scale`.
        self.label_band = f32::from(window.rem_size()) * 1.25;
        let drawing = self.drawing(cx);
        let viewport = self.viewport.clone();
        let menu_target = self.menu_target.clone();
        let view = cx.entity().downgrade();
        let building = self.app.read(cx).engine.is_graph_building() && self.snapshot.revision == 0;
        let empty = self.snapshot.nodes.is_empty();
        let message = if building {
            Some("Building graph…")
        } else if empty && self.local && self.centre.is_none() {
            Some("No page")
        } else {
            None
        };
        div()
            .id("graph-canvas")
            .debug_selector(|| "graph-canvas".to_owned())
            .key_context(GRAPH_CONTEXT)
            .track_focus(&self.focus_handle)
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    *this.menu_target.borrow_mut() = this.node_under(event.position);
                }),
            )
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_scroll(cx.listener(Self::on_scroll_wheel))
            .on_pinch(cx.listener(Self::on_pinch))
            .on_action(cx.listener(Self::on_next_node))
            .on_action(cx.listener(Self::on_previous_node))
            .on_action(cx.listener(Self::on_open_node))
            .on_action(cx.listener(Self::on_clear_path))
            .child(
                canvas(
                    move |bounds, window, _| {
                        if bounds != viewport.get() {
                            viewport.set(bounds);
                            window.refresh();
                        }
                    },
                    move |bounds, (), window, cx| paint(&drawing, bounds, window, cx),
                )
                .absolute()
                .size_full(),
            )
            .when_some(message, |canvas, message| {
                canvas.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .debug_selector(move || format!("graph-message-{message}"))
                        .child(message),
                )
            })
            .context_menu(move |menu, _, cx| {
                let Some(key) = menu_target.borrow().clone() else {
                    return menu;
                };
                let item = |label: &'static str, run: fn(&mut GraphView, &GraphKey, &mut Context<GraphView>)| {
                    let view = view.clone();
                    let key = key.clone();
                    PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |view, cx| run(view, &key, cx));
                        }
                    })
                };
                let Some(this) = view.upgrade() else {
                    return menu;
                };
                let from_selection = this
                    .read(cx)
                    .selected
                    .as_ref()
                    .is_some_and(|selected| *selected != key);
                if let GraphKey::Group(name) = &key {
                    return menu
                        .item(PopupMenuItem::new(name.clone()).disabled(true))
                        .separator()
                        .item(item("Expand Group", |view, key, cx| {
                            if let GraphKey::Group(name) = key {
                                view.expand_group(name, cx);
                            }
                        }));
                }
                let (page, partial, xref, include) = {
                    let graph = this.read(cx);
                    let engine = &graph.app.read(cx).engine;
                    match &key {
                        GraphKey::Group(_) => (false, false, None, None),
                        GraphKey::Unresolved(target) => (
                            engine.new_page_for(target).is_some(),
                            engine.new_partial_for(target).is_some(),
                            None,
                            None,
                        ),
                        GraphKey::File(path) => {
                            let kind = graph.snapshot.node(&key).map(|node| node.kind);
                            let linkable =
                                |style| link_from_active(engine, path, style).is_some();
                            (
                                false,
                                false,
                                (kind == Some(GraphNodeKind::Page))
                                    .then(|| linkable(LinkStyle::Xref)),
                                (kind == Some(GraphNodeKind::Partial))
                                    .then(|| linkable(LinkStyle::Include)),
                            )
                        }
                    }
                };
                if key.path().is_none() {
                    let mut menu = menu
                        .item(PopupMenuItem::new("Unresolved target").disabled(true))
                        .separator();
                    if page || !partial {
                        menu = menu.item(
                            item("Create Page", |view, key, cx| view.create_page(key, cx))
                                .disabled(!page),
                        );
                    }
                    if partial {
                        menu = menu.item(item("Create Partial", |view, key, cx| {
                            view.create_partial(key, cx);
                        }));
                    }
                    return menu;
                }
                let mut menu = menu
                    .item(item("Open", |view, key, cx| view.open(key, true, cx)))
                    .item(item("Open to the Side", |view, key, cx| {
                        view.open_to_the_side(key, cx);
                    }))
                    .separator();
                if let Some(enabled) = xref {
                    menu = menu.item(
                        item("Insert Xref at Caret", |view, key, cx| {
                            view.link_from_active(key, LinkStyle::Xref, cx);
                        })
                        .disabled(!enabled),
                    );
                }
                if let Some(enabled) = include {
                    menu = menu.item(
                        item("Include at Caret", |view, key, cx| {
                            view.link_from_active(key, LinkStyle::Include, cx);
                        })
                        .disabled(!enabled),
                    );
                }
                if xref.is_some() || include.is_some() {
                    menu = menu.separator();
                }
                let menu = menu
                    .item(item("Root Mind Map Here", |view, key, cx| {
                        view.root_mind_map(key, cx);
                    }))
                    .separator();
                let menu = menu
                    .item(item("Find References", |view, key, cx| {
                        view.find_references(key, cx);
                    }))
                    .item(item("Reveal in Files", |view, key, cx| {
                        view.reveal_in_files(key, cx);
                    }));
                if !from_selection {
                    return menu;
                }
                menu.separator()
                    .item(item("Show Path from Selection", |view, key, cx| {
                        if let Some(from) = view.selected.clone() {
                            view.show_path(from, key.clone(), cx);
                        }
                    }))
            })
            .into_any_element()
    }

    // --- toolbars and lists ------------------------------------------------------------------

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        change: fn(&mut GraphFilter),
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .label(label)
            .xsmall()
            .ghost()
            .selected(on)
            .debug_selector(move || id.to_owned())
            .on_click(cx.listener(move |this, _, _, cx| this.set_filter(change, cx)))
    }

    fn depth_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_0p5().children((1..=3u8).map(|depth| {
            Button::new(("graph-depth", depth as usize))
                .label(depth.to_string())
                .xsmall()
                .ghost()
                .selected(self.depth == depth)
                .debug_selector(move || format!("graph-depth-{depth}"))
                .tooltip("Depth")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.depth = depth;
                    this.refresh(cx);
                }))
        }))
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter.clone();
        let components: Vec<String> = {
            let engine = &self.app.read(cx).engine;
            let mut names: Vec<String> = engine
                .workspace()
                .antora
                .components()
                .map(|component| component.name.clone())
                .collect();
            names.sort();
            names.dedup();
            names
        };
        let view = cx.entity().downgrade();
        let chosen = filter.components.clone();
        let colour = match self.colour_by {
            GraphColour::Component => "Colour: Component",
            GraphColour::Module => "Colour: Module",
            GraphColour::Family => "Colour: Family",
            GraphColour::Age => "Colour: Age",
        };
        let ages = self.app.read(cx).engine.has_page_ages();
        let group = match self.group_by {
            None => "Group: Off",
            Some(GroupBy::Module) => "Group: Module",
            Some(GroupBy::Component) => "Group: Component",
        };
        h_flex()
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .children(self.search.as_ref().map(|search| {
                div()
                    .w(px(180.))
                    .debug_selector(|| "graph-search".to_owned())
                    .child(Input::new(search).small().cleanable(true))
            }))
            .child(self.toggle(
                "graph-partials",
                "Partials",
                filter.partials,
                |f| f.partials = !f.partials,
                cx,
            ))
            .child(self.toggle(
                "graph-nav",
                "Navigation",
                filter.nav,
                |f| f.nav = !f.nav,
                cx,
            ))
            .child(self.toggle(
                "graph-orphans",
                "Orphans",
                filter.orphans,
                |f| f.orphans = !f.orphans,
                cx,
            ))
            .child(self.toggle(
                "graph-unresolved",
                "Unresolved",
                filter.unresolved,
                |f| f.unresolved = !f.unresolved,
                cx,
            ))
            .when(!components.is_empty(), |bar| {
                bar.child(
                    Button::new("graph-components")
                        .label(match &chosen {
                            None => "All components".to_owned(),
                            Some(chosen) => {
                                format!("{} of {} components", chosen.len(), components.len())
                            }
                        })
                        .xsmall()
                        .ghost()
                        .dropdown_menu(move |mut menu, _, _| {
                            for name in &components {
                                let shown =
                                    chosen.as_ref().is_none_or(|chosen| chosen.contains(name));
                                let view = view.clone();
                                let name = name.clone();
                                let all = components.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(name.clone()).checked(shown).on_click(
                                        move |_, _, cx| {
                                            if let Some(view) = view.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.toggle_component(&name, &all, cx);
                                                });
                                            }
                                        },
                                    ),
                                );
                            }
                            menu
                        }),
                )
            })
            .child(
                Button::new("graph-colour")
                    .label(colour)
                    .xsmall()
                    .ghost()
                    .debug_selector(|| "graph-colour".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.colour_by = this.next_colour(cx);
                        cx.notify();
                    })),
            )
            .when(ages, |bar| {
                bar.child(
                    Button::new("graph-stale")
                        .label("Stale")
                        .xsmall()
                        .ghost()
                        .selected(self.stale_only)
                        .debug_selector(|| "graph-stale".to_owned())
                        .tooltip("Show only pages unchanged for a long time, and their neighbours")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_stale(cx))),
                )
            })
            .child(
                Button::new("graph-group")
                    .label(group)
                    .xsmall()
                    .ghost()
                    .debug_selector(|| "graph-group".to_owned())
                    .tooltip("Group the nodes by module or component version")
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_grouping(cx))),
            )
            .child(
                Button::new("graph-scope")
                    .label(if self.local { "Local" } else { "Workspace" })
                    .xsmall()
                    .ghost()
                    .debug_selector(|| "graph-scope".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.local = !this.local;
                        this.follow_active(cx);
                        this.fitting = true;
                        this.refresh(cx);
                    })),
            )
            .child(self.arrangement_button(cx))
            .when(
                self.local || self.arrangement != GraphLayout::Force,
                |bar| bar.child(self.depth_buttons(cx)),
            )
            .child(div().flex_1())
            .child(self.zoom_buttons(cx))
            .child(
                Button::new("graph-reset-layout")
                    .label("Reset Layout")
                    .xsmall()
                    .ghost()
                    .debug_selector(|| "graph-reset-layout".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| this.reset_layout(cx))),
            )
    }

    fn zoom_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_0p5()
            .child(
                Button::new("graph-zoom-out")
                    .icon(IconName::Minus)
                    .xsmall()
                    .ghost()
                    .tooltip("Zoom Out")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(0.8, cx))),
            )
            .child(
                Button::new("graph-zoom-in")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost()
                    .tooltip("Zoom In")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.25, cx))),
            )
            .child(
                Button::new("graph-fit")
                    .icon(IconName::Maximize)
                    .xsmall()
                    .ghost()
                    .tooltip("Fit")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.fitting = true;
                        cx.notify();
                    })),
            )
    }

    fn toggle_component(&mut self, name: &str, all: &[String], cx: &mut Context<Self>) {
        let mut shown: std::collections::BTreeSet<String> = self
            .filter
            .components
            .clone()
            .unwrap_or_else(|| all.iter().cloned().collect());
        if !shown.remove(name) {
            shown.insert(name.to_owned());
        }
        self.filter.components = (shown.len() != all.len()).then_some(shown);
        self.refresh(cx);
    }

    /// The path asked for, step by step, each naming the links it took; or that there is none.
    /// A step's row selects its node.
    fn render_path(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let title = |key: &GraphKey| {
            self.snapshot
                .node(key)
                .map_or_else(|| key_name(key), |node| node.title.clone())
        };
        let header = h_flex()
            .px_2()
            .pt_2()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .debug_selector(|| "graph-path-title".to_owned())
                    .child(self.trail.clone().unwrap_or_else(|| "Path".into())),
            )
            .when(self.trail.is_none(), |header| {
                header.child(
                    Button::new("graph-path-directed")
                        .label(if self.directed {
                            "Directed"
                        } else {
                            "Either way"
                        })
                        .xsmall()
                        .ghost()
                        .selected(self.directed)
                        .debug_selector(|| "graph-path-directed".to_owned())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.directed = !this.directed;
                            this.update_path(cx);
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("graph-path-clear")
                    .label("Clear")
                    .xsmall()
                    .ghost()
                    .debug_selector(|| "graph-path-clear".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| this.clear_path(cx))),
            );
        let body = match (&self.path, &self.path_ends) {
            (Some(steps), _) => v_flex()
                .py_1()
                .children(steps.iter().enumerate().map(|(at, step)| {
                    let key = step.key.clone();
                    let selector = format!("graph-path-step-{at}");
                    let how = (at > 0).then(|| {
                        let kinds: Vec<&str> = step.kinds.iter().map(|kind| kind.name()).collect();
                        let back = if step.backwards {
                            ", followed back"
                        } else {
                            ""
                        };
                        format!("by {}{back}", kinds.join(" · "))
                    });
                    v_flex()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .px_2()
                        .py_0p5()
                        .cursor_pointer()
                        .hover(|row| row.bg(cx.theme().list_hover))
                        .on_click(cx.listener(move |this, _, _, cx| this.reveal(&key, cx)))
                        .child(format!("{}. {}", at + 1, title(&step.key)))
                        .children(how.map(|how| div().text_xs().text_color(muted).child(how)))
                }))
                .into_any_element(),
            (None, Some((from, to))) => div()
                .px_2()
                .py_1()
                .text_sm()
                .text_color(muted)
                .debug_selector(|| "graph-path-none".to_owned())
                .child(format!("No path from {} to {}.", title(from), title(to)))
                .into_any_element(),
            (None, None) => div().into_any_element(),
        };
        v_flex()
            .pb_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(header)
            .child(body)
    }

    fn arrangement_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("graph-layout")
            .label(match self.arrangement {
                GraphLayout::Force => "Layout: Force",
                GraphLayout::MindMap(TreeStyle::Horizontal) => "Layout: Mind Map",
                GraphLayout::MindMap(TreeStyle::Radial) => "Layout: Radial",
            })
            .xsmall()
            .ghost()
            .debug_selector(|| "graph-layout".to_owned())
            .tooltip("Lay the graph out by force, or as a mind map from one node")
            .on_click(cx.listener(|this, _, _, cx| this.cycle_arrangement(cx)))
    }

    fn render_panel_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter.clone();
        h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .child(self.depth_buttons(cx))
            .child(self.toggle(
                "graph-partials",
                "Partials",
                filter.partials,
                |f| f.partials = !f.partials,
                cx,
            ))
            .child(self.toggle(
                "graph-nav",
                "Navigation",
                filter.nav,
                |f| f.nav = !f.nav,
                cx,
            ))
            .child(self.arrangement_button(cx))
            .child(div().flex_1())
            .child(
                Button::new("graph-open-window")
                    .icon(IconName::ExternalLink)
                    .xsmall()
                    .ghost()
                    .tooltip("Open the Graph Window")
                    .debug_selector(|| "graph-open-window".to_owned())
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::editor::bindings::ShowGraph), cx);
                    }),
            )
    }

    /// One row of a list of links: the neighbour, and every kind of link to it.
    fn neighbour_row(
        &self,
        list: &'static str,
        neighbour: &Neighbour,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let kinds = neighbour
            .kinds
            .iter()
            .map(|kind| kind.name())
            .collect::<Vec<_>>()
            .join(" · ");
        let key = neighbour.key.clone();
        let title = neighbour.title.clone();
        let selector = format!("graph-{list}-{title}");
        h_flex()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .px_2()
            .gap_2()
            .text_sm()
            .rounded_sm()
            .cursor_pointer()
            .hover(|row| row.bg(cx.theme().list_hover))
            .child(div().flex_1().truncate().child(title))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(kinds),
            )
            .on_click(cx.listener(move |this, _, _, cx| match this.mode {
                // In the window a row selects its node; in the panel it opens its page.
                GraphMode::Window => this.reveal(&key, cx),
                GraphMode::Panel => this.open(&key, false, cx),
            }))
    }

    fn render_lists(&self, path: &std::path::Path, cx: &mut Context<Self>) -> impl IntoElement {
        let (out, back) = self.app.read(cx).engine.neighbours(path, &self.filter);
        let heading = |text: &'static str, count: usize, cx: &mut Context<Self>| {
            div()
                .px_2()
                .pt_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{text} ({count})"))
        };
        v_flex()
            .child(heading("Links out", out.len(), cx))
            .children(
                out.iter()
                    .map(|neighbour| self.neighbour_row("out", neighbour, cx)),
            )
            .child(heading("Backlinks", back.len(), cx))
            .children(
                back.iter()
                    .map(|neighbour| self.neighbour_row("back", neighbour, cx)),
            )
    }

    /// Where the page sits in the navigation: one trail for each place it is listed.
    fn render_trails(&self, path: &std::path::Path, cx: &mut Context<Self>) -> impl IntoElement {
        let places = self.app.read(cx).engine.nav_places(path);
        let muted = cx.theme().muted_foreground;
        let body = v_flex()
            .px_2()
            .pt_2()
            .gap_1()
            .text_sm()
            .debug_selector(|| "graph-trails".to_owned());
        if places.is_empty() {
            return body
                .child(
                    div()
                        .text_color(muted)
                        .debug_selector(|| "graph-trail-none".to_owned())
                        .child("Not in any navigation"),
                )
                .into_any_element();
        }
        body.children(
            places
                .iter()
                .enumerate()
                .map(|(index, place)| self.render_trail(index, place, cx)),
        )
        .into_any_element()
    }

    fn render_trail(
        &self,
        index: usize,
        place: &NavPlace,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let separator = || div().text_color(muted).child("›");
        let crumb = |id: String,
                     title: String,
                     path: Option<PathBuf>,
                     bold: bool,
                     cx: &mut Context<Self>| {
            let link = path.clone();
            div()
                .id(SharedString::from(id))
                .when(bold, |crumb| crumb.font_weight(gpui::FontWeight::BOLD))
                .when(path.is_none(), |crumb| crumb.text_color(muted))
                .when(path.is_some() && !bold, |crumb| {
                    crumb.cursor_pointer().hover(|crumb| crumb.underline())
                })
                .child(title)
                .when_some(link.filter(|_| !bold), |crumb, path| {
                    crumb.on_click(cx.listener(move |this, _, _, cx| {
                        this.open(
                            &GraphKey::File(std::sync::Arc::from(path.as_path())),
                            false,
                            cx,
                        );
                    }))
                })
        };
        let nav_name = place
            .nav
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let mut row = h_flex()
            .flex_wrap()
            .gap_1()
            .debug_selector(move || format!("graph-trail-{index}"))
            .child(crumb(
                format!("trail-{index}-nav"),
                nav_name,
                Some(place.nav.clone()),
                false,
                cx,
            ));
        for (step, ancestor) in place.ancestors.iter().enumerate() {
            row = row.child(separator()).child(crumb(
                format!("trail-{index}-{step}"),
                ancestor.title.clone(),
                ancestor.path.clone(),
                false,
                cx,
            ));
        }
        row = row.child(separator()).child(crumb(
            format!("trail-{index}-page"),
            place.page.title.clone(),
            place.page.path.clone(),
            true,
            cx,
        ));
        if !place.children.is_empty() {
            let children: Vec<String> = place
                .children
                .iter()
                .map(|child| child.title.clone())
                .collect();
            row = row
                .child(separator())
                .child(div().text_color(muted).child(children.join(", ")));
        }
        row
    }

    // --- tests -------------------------------------------------------------------------------

    #[cfg(test)]
    pub fn snapshot_for_test(&self) -> &GraphSnapshot {
        &self.snapshot
    }

    /// Where `key` is drawn, in window coordinates.
    #[cfg(test)]
    pub fn node_position_for_test(&self, key: &GraphKey) -> Option<gpui::Point<Pixels>> {
        self.layout.position(key).map(|at| self.to_window(at))
    }

    #[cfg(test)]
    pub fn selected_for_test(&self) -> Option<&GraphKey> {
        self.selected.as_ref()
    }

    #[cfg(test)]
    pub fn hovered_for_test(&self) -> Option<&GraphKey> {
        self.hovered.as_ref()
    }

    #[cfg(test)]
    pub fn is_pinned_for_test(&self, key: &GraphKey) -> bool {
        self.layout.is_pinned(key)
    }

    #[cfg(test)]
    pub fn zoom_for_test(&self) -> f32 {
        self.zoom
    }

    /// The nodes drawn with the stale ring.
    #[cfg(test)]
    pub fn stale_for_test(&self, cx: &App) -> Vec<GraphKey> {
        let drawing = self.drawing(cx);
        self.snapshot
            .nodes
            .iter()
            .zip(drawing.nodes)
            .filter(|(_, drawn)| drawn.stale)
            .map(|(node, _)| node.key.clone())
            .collect()
    }

    #[cfg(test)]
    pub fn colour_for_test(&self) -> GraphColour {
        self.colour_by
    }

    #[cfg(test)]
    pub fn arrangement_for_test(&self) -> (GraphLayout, Option<GraphKey>) {
        (
            self.arrangement,
            self.tree.as_ref().map(|tree| tree.root().clone()),
        )
    }

    /// Where `key`'s disclosure is drawn, in window coordinates.
    #[cfg(test)]
    pub fn fold_position_for_test(&self, key: &GraphKey) -> Option<gpui::Point<Pixels>> {
        self.fold_centre(key)
    }

    #[cfg(test)]
    pub fn grouping_for_test(&self) -> (Option<GroupBy>, Option<GroupBy>) {
        (self.group_by, self.layout.grouping())
    }

    /// The middle of the label of the hull of group `name`, in window coordinates.
    #[cfg(test)]
    pub fn hull_label_for_test(&self, name: &str) -> Option<gpui::Point<Pixels>> {
        let centre = self.viewport.get().center();
        let hull = self
            .hulls(&self.positions())
            .into_iter()
            .find(|hull| hull.name == name)?;
        let (_, label) = hull_bounds(
            hull.low,
            hull.high,
            self.zoom,
            self.pan,
            centre,
            self.label_band,
        );
        Some(label.center())
    }

    /// The nodes drawn dimmed, as the hover, selection and search leave them.
    #[cfg(test)]
    pub fn dimmed_for_test(&self, cx: &App) -> Vec<GraphKey> {
        let drawing = self.drawing(cx);
        self.snapshot
            .nodes
            .iter()
            .zip(drawing.nodes)
            .filter(|(_, drawn)| drawn.dim)
            .map(|(node, _)| node.key.clone())
            .collect()
    }

    /// Stops the layout, so a node stays where a test clicks it.
    #[cfg(test)]
    pub fn settle_for_test(&mut self) {
        while self.layout.step(1.0) {}
        self.fit();
    }
}

impl Focusable for GraphView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let canvas = self.render_canvas(window, cx);
        match self.mode {
            GraphMode::Window => {
                // Always there, so selecting a node never moves the canvas under the pointer.
                let selected = self.selected.as_ref().and_then(|key| {
                    let path = key.path()?.to_path_buf();
                    let node = self.snapshot.node(key)?;
                    Some((path, node.title.clone(), node.last_changed))
                });
                let muted = cx.theme().muted_foreground;
                let side = v_flex()
                    .id("graph-side")
                    .debug_selector(|| "graph-side".to_owned())
                    .flex_none()
                    .w(px(260.))
                    .h_full()
                    .overflow_y_scroll()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .when(self.path_ends.is_some() || self.trail.is_some(), |side| {
                        side.child(self.render_path(cx))
                    })
                    .map(|side| match selected {
                        Some((path, title, changed)) => side
                            .child(
                                div()
                                    .px_2()
                                    .pt_2()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .children(changed.map(|changed| {
                                div()
                                    .px_2()
                                    .text_xs()
                                    .text_color(muted)
                                    .debug_selector(|| "graph-last-changed".to_owned())
                                    .child(last_changed_text(changed, now_seconds()))
                            }))
                            .child(self.render_lists(&path, cx)),
                        None => side.child(
                            div()
                                .p_2()
                                .text_sm()
                                .text_color(muted)
                                .child("Select a node to see its links."),
                        ),
                    });
                v_flex()
                    .size_full()
                    .bg(cx.theme().background)
                    .text_color(cx.theme().foreground)
                    .child(self.render_toolbar(cx))
                    .child(
                        h_flex()
                            .items_stretch()
                            .flex_1()
                            .min_h_0()
                            .child(canvas)
                            .child(side),
                    )
                    .into_any_element()
            }
            GraphMode::Panel => {
                let below = self.centre.clone().map(|path| {
                    v_flex()
                        .when(self.filter.nav, |below| {
                            below.child(self.render_trails(&path, cx))
                        })
                        .child(self.render_lists(&path, cx))
                });
                v_flex()
                    .id("graph-panel")
                    .size_full()
                    .overflow_y_scroll()
                    .child(self.render_panel_toolbar(cx))
                    .child(div().h(px(240.)).flex_none().flex().child(canvas))
                    .children(below)
                    .into_any_element()
            }
        }
    }
}

/// A node's name when the snapshot does not hold it: its file's name, or an unresolved target
/// as written.
fn key_name(key: &GraphKey) -> String {
    match key {
        GraphKey::File(path) => path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        GraphKey::Unresolved(text) | GraphKey::Group(text) => text.clone(),
    }
}

/// A node's colour on the canvas, from the palette exported images share, or `grey` where it
/// has none.
fn node_colour(node: &adoc_engine::GraphNode, palette: &Palette, dark: bool, grey: Hsla) -> Hsla {
    palette
        .colour(node, dark)
        .map_or(grey, |colour| gpui::hsla(colour.h, colour.s, colour.l, 1.0))
}

const DAY: i64 = 86_400;

/// Now, in seconds since the Unix epoch.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Whether the node's file was last committed before `before`.
fn is_stale(node: &adoc_engine::GraphNode, before: i64) -> bool {
    matches!(node.last_changed, Some(LastChanged::At(time)) if time < before)
}

/// When a file last changed, as the side panel says it: "Last changed 14 months ago".
fn last_changed_text(changed: LastChanged, now: i64) -> String {
    let LastChanged::At(time) = changed else {
        return "Changed, not committed yet".to_owned();
    };
    let days = (now - time).max(0) / DAY;
    let plural = |count: i64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    let ago = match days {
        0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        2..=44 => plural(days, "day"),
        45..=547 => plural((days + 15) / 30, "month"),
        _ => plural((days + 182) / 365, "year"),
    };
    format!("Last changed {ago}")
}

/// Writes the graph image `svg` to `output`: drawn as a PNG at twice its size when the name
/// ends in `.png`, else as the SVG itself.
pub(crate) fn write_graph_image(output: &Path, svg: &str) -> Result<(), String> {
    let png = output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    let bytes = if png {
        crate::image_decode::svg_to_png(svg, 2.0).map_err(|error| error.to_string())?
    } else {
        svg.as_bytes().to_vec()
    };
    std::fs::write(output, bytes).map_err(|error| format!("{}: {error}", output.display()))
}

fn paint(drawing: &Drawing, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let centre = bounds.center();
    let scale = drawing.zoom.clamp(0.6, 1.6);
    let at = |point: LayoutPoint| {
        gpui::point(
            centre.x + px(point.x * drawing.zoom + drawing.pan.0),
            centre.y + px(point.y * drawing.zoom + drawing.pan.1),
        )
    };
    window.paint_layer(bounds, |window| {
        let font = window.text_style().font();
        // 11 px at the default rem size, so labels follow `[appearance] font_scale`.
        let font_size = window.rem_size() * (11. / 16.);
        for hull in &drawing.hulls {
            let (body, label) = hull_bounds(
                hull.low,
                hull.high,
                drawing.zoom,
                drawing.pan,
                centre,
                drawing.band,
            );
            if !bounds.intersects(&body) {
                continue;
            }
            window.paint_quad(quad(
                body,
                Corners::all(px(12.)),
                hull.colour.opacity(0.07),
                px(1.),
                hull.colour.opacity(0.35),
                BorderStyle::Solid,
            ));
            let run = TextRun {
                len: hull.label.len(),
                font: font.clone(),
                color: drawing.label.opacity(0.75),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped =
                window
                    .text_system()
                    .shape_line(hull.label.clone(), font_size, &[run], None);
            let _ = shaped.paint(
                point(
                    label.origin.x + px(8.),
                    label.origin.y + (label.size.height - font_size * 1.3) / 2.,
                ),
                font_size * 1.3,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        paint_edges(drawing, &at, window);
        for node in &drawing.nodes {
            let middle = at(node.at);
            let radius = px(node.radius * scale);
            let alpha = if node.dim { 0.25 } else { 1.0 };
            let colour = node.colour.opacity(alpha);
            let square = Bounds::new(
                point(middle.x - radius, middle.y - radius),
                size(radius * 2., radius * 2.),
            );
            if !bounds.intersects(&square) {
                continue;
            }
            let corners = match node.kind {
                GraphNodeKind::Nav => Corners::all(px(2.)),
                GraphNodeKind::Group => Corners::all(radius * 0.4),
                _ => Corners::all(radius),
            };
            let (background, border, style) = match node.kind {
                GraphNodeKind::Partial => (gpui::transparent_black(), colour, BorderStyle::Solid),
                GraphNodeKind::Unresolved => {
                    (gpui::transparent_black(), colour, BorderStyle::Dashed)
                }
                _ => (colour, colour, BorderStyle::Solid),
            };
            window.paint_quad(quad(square, corners, background, px(2.), border, style));
            if let Some(folded) = node.folded {
                paint_fold(
                    point(middle.x - radius - px(FOLD_GAP + FOLD_RADIUS), middle.y),
                    folded,
                    drawing.label.opacity(alpha),
                    window,
                );
            }
            if node.stale {
                window.paint_quad(quad(
                    square.dilate(px(2.)),
                    match node.kind {
                        GraphNodeKind::Nav => Corners::all(px(4.)),
                        _ => Corners::all(radius + px(2.)),
                    },
                    gpui::transparent_black(),
                    px(1.5),
                    drawing.stale_ring.opacity(alpha),
                    BorderStyle::Dashed,
                ));
            }
            if node.selected {
                let ring = square.dilate(px(3.));
                let ring_corners = match node.kind {
                    GraphNodeKind::Nav => Corners::all(px(5.)),
                    _ => Corners::all(radius + px(3.)),
                };
                window.paint_quad(quad(
                    ring,
                    ring_corners,
                    gpui::transparent_black(),
                    px(1.5),
                    drawing.ring,
                    BorderStyle::Solid,
                ));
            }
            if let Some(label) = &node.label {
                let run = TextRun {
                    len: label.len(),
                    font: font.clone(),
                    color: drawing.label.opacity(alpha),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped =
                    window
                        .text_system()
                        .shape_line(label.clone(), font_size, &[run], None);
                let _ = shaped.paint(
                    point(middle.x + radius + px(3.), middle.y - font_size * 0.65),
                    font_size * 1.3,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
    });
}

/// A mind map node's disclosure at `centre`: a ring holding − while its branch is open and +
/// while it is closed.
fn paint_fold(centre: gpui::Point<Pixels>, folded: bool, colour: Hsla, window: &mut Window) {
    let ring = Bounds::new(
        point(centre.x - px(FOLD_RADIUS), centre.y - px(FOLD_RADIUS)),
        size(px(FOLD_RADIUS * 2.), px(FOLD_RADIUS * 2.)),
    );
    window.paint_quad(quad(
        ring,
        Corners::all(px(FOLD_RADIUS)),
        gpui::transparent_black(),
        px(1.),
        colour,
        BorderStyle::Solid,
    ));
    let bar = |width: f32, height: f32| {
        Bounds::new(
            point(centre.x - px(width / 2.), centre.y - px(height / 2.)),
            size(px(width), px(height)),
        )
    };
    window.paint_quad(gpui::fill(bar(5., 1.2), colour));
    if folded {
        window.paint_quad(gpui::fill(bar(1.2, 5.), colour));
    }
}

/// Every edge, gathered into one path for each style, so thousands draw in a few calls: xrefs
/// solid, includes dashed, navigation dotted and bowed, links to unresolved targets dash-dot.
fn paint_edges(
    drawing: &Drawing,
    at: &dyn Fn(LayoutPoint) -> gpui::Point<Pixels>,
    window: &mut Window,
) {
    #[derive(Clone, Copy, Eq, Hash, PartialEq)]
    enum Style {
        Solid,
        Dashed,
        Dotted,
        DashDot,
    }
    let mut paths: std::collections::HashMap<(Style, bool, bool), PathBuilder> =
        std::collections::HashMap::new();
    for edge in &drawing.edges {
        let (from, to) = (
            at(drawing.nodes[edge.from].at),
            at(drawing.nodes[edge.to].at),
        );
        let style = match (edge.kind, edge.unresolved) {
            (_, true) => Style::DashDot,
            (EdgeKind::Xref, _) => Style::Solid,
            (EdgeKind::Include, _) => Style::Dashed,
            (EdgeKind::Nav, _) => Style::Dotted,
        };
        let heavy = edge.count > 1;
        let width = if heavy { px(2.) } else { px(1.) };
        let builder = paths.entry((style, edge.dim, heavy)).or_insert_with(|| {
            let builder = PathBuilder::stroke(width);
            match style {
                Style::Solid => builder,
                Style::Dashed => builder.dash_array(&[px(6.), px(4.)]),
                Style::Dotted => builder.dash_array(&[px(1.5), px(3.)]),
                Style::DashDot => builder.dash_array(&[px(6.), px(3.), px(1.5), px(3.)]),
            }
        });
        builder.move_to(from);
        if edge.kind == EdgeKind::Nav {
            // Bowed to one side, so a navigation edge beside an xref between the same pages
            // stays visible.
            let middle = point((from.x + to.x) / 2., (from.y + to.y) / 2.);
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            builder.curve_to(to, point(middle.x - dy * 0.15, middle.y + dx * 0.15));
        } else {
            builder.line_to(to);
        }
    }
    let mut keys: Vec<(Style, bool, bool)> = paths.keys().copied().collect();
    // Dim edges first, so the emphasised lie on top.
    keys.sort_by_key(|(_, dim, heavy)| (!dim, *heavy));
    for key in keys {
        let (style, dim, _) = key;
        let Some(builder) = paths.remove(&key) else {
            continue;
        };
        let colour = if style == Style::Dotted {
            drawing.nav_edge
        } else {
            drawing.edge
        };
        let colour = if dim { colour.opacity(0.2) } else { colour };
        if let Ok(path) = builder.build() {
            window.paint_path(path, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_changed_reads_as_a_rough_age() {
        let now = 1_000 * DAY;
        let text = |days_ago: i64| last_changed_text(LastChanged::At(now - days_ago * DAY), now);
        assert_eq!(text(0), "Last changed today");
        assert_eq!(text(1), "Last changed yesterday");
        assert_eq!(text(9), "Last changed 9 days ago");
        assert_eq!(text(45), "Last changed 2 months ago");
        assert_eq!(text(425), "Last changed 14 months ago");
        assert_eq!(text(548), "Last changed 2 years ago");
        assert_eq!(text(365 * 3), "Last changed 3 years ago");
        assert_eq!(
            last_changed_text(LastChanged::Uncommitted, now),
            "Changed, not committed yet"
        );
    }
}
