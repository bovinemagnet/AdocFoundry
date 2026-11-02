//! The link graph, drawn: the graph window's view of the whole workspace, and the Graph sidebar
//! panel's view of the active page and its neighbours. Both draw the engine's
//! [`GraphSnapshot`] on one canvas, laid out by `adoc-graph`'s [`Layout`], which is stepped once
//! a frame until it settles.
//!
//! Dragging the background pans, the wheel or a pinch zooms around the pointer, and dragging a
//! node pins it (released with Alt held, it is let go). Hovering a node highlights it and its
//! neighbours. A click selects a node and opens its page as a preview, a double click keeps it
//! open, and both go through `navigate.go_to` as every other way of opening a page does.

use std::{cell::Cell, cell::RefCell, path::PathBuf, rc::Rc};

use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, EdgeKind, EngineEvent, GraphColour, GraphFilter,
    GraphKey, GraphNodeKind, GraphScope, GraphSnapshot, Layout, NavPlace, NavigationPosition,
    Neighbour, Point as LayoutPoint, TabItem,
};
use gpui::{
    actions, canvas, div, point, prelude::FluentBuilder as _, px, quad, size, App, AppContext as _,
    BorderStyle, Bounds, Context, Corners, Entity, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, PathBuilder, PinchEvent, Pixels, Render, ScrollWheelEvent,
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

actions!(adocfoundry, [NextNode, PreviousNode, OpenNode]);

/// The key context of the graph's canvas, where Tab, Shift-Tab and Enter move between nodes
/// and open one rather than moving focus.
pub const GRAPH_CONTEXT: &str = "Graph";

/// Binds the canvas's keys.
pub fn bind(cx: &mut App) {
    let context = Some(GRAPH_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("tab", NextNode, context),
        KeyBinding::new("shift-tab", PreviousNode, context),
        KeyBinding::new("enter", OpenNode, context),
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

/// Everything the canvas paints, worked out in `render`.
struct Drawing {
    nodes: Vec<DrawnNode>,
    edges: Vec<DrawnEdge>,
    zoom: f32,
    pan: (f32, f32),
    edge: Hsla,
    nav_edge: Hsla,
    label: Hsla,
    ring: Hsla,
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
            drag: None,
            menu_target: Rc::default(),
            _subscriptions: subscriptions,
        };
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
        true
    }

    /// Asks the engine for the graph again and fits the layout to it.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let scope = if self.local {
            match &self.centre {
                Some(path) => GraphScope::Local {
                    path: path.clone(),
                    depth: self.depth,
                },
                None => {
                    self.set_snapshot(GraphSnapshot::default());
                    self.layout.sync(&self.snapshot);
                    cx.notify();
                    return;
                }
            }
        } else {
            GraphScope::Workspace
        };
        let snapshot = self.app.read(cx).engine.graph(&scope, &self.filter);
        let recentre = self.local && snapshot.nodes.len() != self.snapshot.nodes.len();
        self.set_snapshot(snapshot);
        self.layout.sync(&self.snapshot);
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
        cx.notify();
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
        let reach = (node_radius(16) * self.zoom.clamp(0.6, 1.6) + HIT_SLOP) / self.zoom;
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

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        if let Some(key) = self.selected.clone() {
            self.open(&key, false, cx);
        }
    }

    // --- drawing -----------------------------------------------------------------------------

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
        let groups = colour_groups(&self.snapshot, self.colour_by);
        let all_labels = self.zoom >= LABEL_ZOOM || nodes_shown.len() <= 12;
        // The layout holds its nodes in the snapshot's order.
        let positions: Vec<LayoutPoint> = if self.layout.len() == nodes_shown.len() {
            self.layout.positions().map(|(_, at)| at).collect()
        } else {
            nodes_shown
                .iter()
                .map(|node| self.layout.position(&node.key).unwrap_or_default())
                .collect()
        };
        let nodes: Vec<DrawnNode> = nodes_shown
            .iter()
            .enumerate()
            .map(|(place, node)| {
                let emphasised = focus.is_none() || near[place];
                let found = matches(node);
                let dim = !emphasised || !found;
                let label = all_labels
                    || (focus.is_some() && emphasised)
                    || (!self.query.is_empty() && found)
                    || selected == Some(place);
                DrawnNode {
                    at: positions[place],
                    radius: node_radius(node.links_in + node.links_out),
                    kind: node.kind,
                    colour: node_colour(
                        node,
                        &groups,
                        self.colour_by,
                        dark,
                        theme.muted_foreground,
                    ),
                    label: label.then(|| node.title.clone().into()),
                    dim,
                    selected: selected == Some(place),
                }
            })
            .collect();
        let edges = self
            .snapshot
            .edges
            .iter()
            .zip(&self.edge_places)
            .map(|(edge, &(from, to))| {
                let emphasised = focus.is_none_or(|focus| from == focus || to == focus);
                DrawnEdge {
                    from,
                    to,
                    kind: edge.kind,
                    count: edge.count,
                    unresolved: matches!(edge.to, GraphKey::Unresolved(_)),
                    dim: !emphasised || nodes[from].dim && nodes[to].dim,
                }
            })
            .collect();
        Drawing {
            nodes,
            edges,
            zoom: self.zoom,
            pan: self.pan,
            edge: theme.muted_foreground.opacity(0.55),
            nav_edge: theme.primary.opacity(0.6),
            label: theme.foreground,
            ring: theme.ring,
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
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_pinch(cx.listener(Self::on_pinch))
            .on_action(cx.listener(Self::on_next_node))
            .on_action(cx.listener(Self::on_previous_node))
            .on_action(cx.listener(Self::on_open_node))
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
            .context_menu(move |menu, _, _| {
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
                let file = key.path().is_some();
                if !file {
                    return menu.item(PopupMenuItem::new("Unresolved target").disabled(true));
                }
                menu.item(item("Open", |view, key, cx| view.open(key, true, cx)))
                    .item(item("Open to the Side", |view, key, cx| {
                        view.open_to_the_side(key, cx);
                    }))
                    .separator()
                    .item(item("Find References", |view, key, cx| {
                        view.find_references(key, cx);
                    }))
                    .item(item("Reveal in Files", |view, key, cx| {
                        view.reveal_in_files(key, cx);
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.colour_by = match this.colour_by {
                            GraphColour::Component => GraphColour::Module,
                            GraphColour::Module => GraphColour::Family,
                            GraphColour::Family => GraphColour::Component,
                        };
                        cx.notify();
                    })),
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
            .when(self.local, |bar| bar.child(self.depth_buttons(cx)))
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
                    let title = self.snapshot.node(key)?.title.clone();
                    Some((path, title))
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
                    .map(|side| match selected {
                        Some((path, title)) => side
                            .child(
                                div()
                                    .px_2()
                                    .pt_2()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
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

/// The radius of a node with `links` edges, in pixels at zoom 1.
fn node_radius(links: usize) -> f32 {
    (4.0 + (links as f32).sqrt() * 1.6).min(14.0)
}

/// Each colour group's place, in name order: components, modules or families.
fn colour_groups(snapshot: &GraphSnapshot, by: GraphColour) -> Vec<String> {
    let mut groups: Vec<String> = snapshot
        .nodes
        .iter()
        .filter_map(|node| group_of(node, by))
        .collect();
    groups.sort();
    groups.dedup();
    groups
}

fn group_of(node: &adoc_engine::GraphNode, by: GraphColour) -> Option<String> {
    let coordinate = node.coordinate.as_ref()?;
    Some(match by {
        GraphColour::Component => coordinate.component.clone(),
        GraphColour::Module => format!("{}:{}", coordinate.component, coordinate.module),
        GraphColour::Family => coordinate.family.clone(),
    })
}

/// A node's colour: its group's place in a palette of hues that also differ in lightness, set
/// for the light theme or the dark. Nodes outside Antora, and unresolved targets, are grey.
fn node_colour(
    node: &adoc_engine::GraphNode,
    groups: &[String],
    by: GraphColour,
    dark: bool,
    grey: Hsla,
) -> Hsla {
    const HUES: [f32; 8] = [0.58, 0.08, 0.36, 0.92, 0.15, 0.75, 0.48, 0.0];
    let Some(group) = group_of(node, by) else {
        return grey;
    };
    let place = groups.iter().position(|known| *known == group).unwrap_or(0);
    let hue = HUES[place % HUES.len()];
    let lighter = (place / HUES.len()) % 2 == 1;
    let lightness = match (dark, lighter) {
        (false, false) => 0.42,
        (false, true) => 0.58,
        (true, false) => 0.62,
        (true, true) => 0.76,
    };
    gpui::hsla(hue, 0.65, lightness, 1.0)
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
        paint_edges(drawing, &at, window);
        let font = window.text_style().font();
        let font_size = px(11.);
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
