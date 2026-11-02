# PRD: AdocFoundry — Phase 7 Implementation Specification

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Date | 2026-11-02 |
| Status | Implemented |
| Scope | Phase 7: an interactive graph view of how a workspace's pages link to each other |
| Depends on | `docs/design/DESIGN-AdocFoundry_Architecture.md`, Phases 1 to 6 as released in 0.5.0, issue #272 |

## 1. Document Purpose

AdocFoundry already resolves every xref, include and navigation entry in a workspace, but it only ever shows one link at a time: Find References, the Problems panel and link following. Phase 7 draws all of them at once. It adds a node-based map of how pages connect, for the whole workspace or around one page, in the spirit of Obsidian's graph view.

Phase 7 must provide:

- **The link graph (#272):** the engine keeps a graph of the workspace's pages, partials and navigation files. Its edges are xrefs, includes and navigation entries. It stays current while the user types and when files change on disk, without rescanning the workspace.
- **A layout:** a deterministic force-directed layout, in a new `adoc-graph` crate free of any UI framework, that stays interactive for a few thousand nodes.
- **The graph window:** the whole workspace's graph in its own window, with pan, zoom, drag, hover highlighting, search, filters and colouring, and with clicks that open pages.
- **The local graph:** a sidebar panel showing the active page and its neighbours to a chosen depth, following the active tab.
- **Headless access:** `adocfoundry-cli graph` and a read-only `adoc-mcp` tool, so the graph can be checked in CI and read by other tools.
- **The specification itself:** this document, with the phase added to the design's phase table and the README.

## 2. Decisions

| Question | Decision | Consequence |
|---|---|---|
| Phase | the graph view is Phase 7, specified here, not an extra package in Phase 6 | Phase 6's scope stays as released. |
| Nodes | pages, partials (reached by `include::`) and navigation files listed under `nav` in an `antora.yml`. Unresolved targets are optional "ghost" nodes. Images are not nodes | The graph is about how the documentation reads, not about its assets. Broken links become visible without a separate view. |
| Edges | `Xref` (page to page, from the index's references), `Include` (a file to the file it includes) and `Nav` (from the navigation tree, §4). Several links between the same two files make one edge with a count | Each kind can be filtered and has its own line style. |
| Navigation edges | taken from `adoc_antora::NavTree` through `Engine::nav_trees_of`: the nav file links to each top-level entry's page, and each entry's page links to its children's pages. An entry with no target hands its children to its nearest ancestor with one (or to the nav file). A nav file's own xrefs are not also drawn as `Xref` edges | The navigation's hierarchy shows as structure, rather than as one hub with an edge to every page. |
| The full build | one full build on a worker after the workspace is first indexed, and again only when the Antora catalog changes. Every other change is incremental (§4) | This is the one exception to "never add a feature that scans every document", agreed for this phase. The initial index already visits every file, so the build adds one more pass over parses already in memory, and reads no file. |
| Where the graph lives | a new L1 crate, `adoc-graph`, with no internal dependencies and no AsciiDoc knowledge: the graph's types, the `LinkStore` of edges keyed by file, local walks, snapshots and the layout. Resolution of one file's links in `adoc-lang` (`links_from`). The engine joins them: it decides when to re-resolve, builds navigation edges, runs the full build, publishes events and owns settings and commands | The layout and the walks are general graph code, tested and benchmarked on their own without an engine. `adoc-engine`, already the largest crate, gains only the wiring. Graph semantics stay below the app, and the UI framework stays a one-crate swap. `adoc-ls`, the CLI and `adoc-mcp` can read the same graph. |
| A new crate | `adoc-graph` at layer 1, added to `LAYERS` in `xtask/src/layers.rs` and to the crate ladder. `adoc-lang` does not depend on it: `links_from` returns `adoc-lang`'s own `Link`, and the engine maps it | The project prefers extending a crate, but none of the existing ones fits code that is neither AsciiDoc nor engine state. No same-layer pair is needed. |
| Global graph placement | its own window, `WindowKind::Graph`, opened like the detached preview (`open_secondary_window`). One per workspace window; opening it again focuses it | The engine's `TabItem` stays a document or an image. The graph can sit on a second screen beside the editor. |
| Local graph placement | a new sidebar panel, `SidebarTab::Graph`, after Changes | It follows the active tab without taking space from the editor. |
| Layout | a seeded force-directed simulation (repulsion, spring edges, centring, damping), with Barnes–Hut approximation above 500 nodes, in `adoc-graph/src/layout.rs`. Positions are kept by path, so a refresh moves only what changed | The layout can be tested headlessly and gives the same result for the same seed. The app only calls `step` on animation frames and draws. |
| Drawing | `gpui::canvas` with `PathBuilder` for edges and nodes, as `image_view.rs` already uses `canvas`. Labels appear only above a zoom threshold, on hover, and for the selection and its neighbours | One element draws thousands of nodes, and the labels stay readable. |
| Opening a node | through `navigate.go_to`: a click opens with `OpenDisposition::Preview`, a double click with `Persistent` | No new navigation path, and preview tabs behave as everywhere else. |
| Dependencies | no new external dependency | The layout and hit testing are a few hundred lines of plain Rust. Nothing needs `cargo deny` changes. |
| Saved state | the graph window's size and position are remembered: the engine keeps them and writes them with the session (§6). Its filters and colouring are kept in memory per workspace. Pinned node positions are not saved | The window comes back where the user put it, often on a second screen. Saving a layout across sessions can follow if it is asked for (§9). |

## 3. Architecture

```
L4  adocfoundry       (WindowKind::Graph, GraphView, the Graph sidebar panel, menus)
    adocfoundry-cli   (graph <root>)
    adoc-mcp          (link_graph tool, read-only)
L3  adoc-engine       (graph.rs: keeps the LinkStore current, nav edges, the full build, GraphChanged, [graph] settings, commands)
L1  adoc-graph        (new: GraphKey, GraphNode, GraphEdge, GraphSnapshot, LinkStore, local walks, Layout, Point)
    adoc-lang         (graph.rs: links_from, one file's resolved links)
```

- One crate is added, `adoc-graph`, at layer 1 with no internal dependencies. It gets an entry in `LAYERS` in `xtask/src/layers.rs`, and a line in the crate ladder in `CLAUDE.md` and the developer documentation, in WP 7.1.
- No dependency direction changes, and no same-layer pair is added.
- No new external dependency.

## 4. The link graph (WP 7.1, #272)

```rust
// adoc-lang
pub enum LinkKind { Xref, Include, Nav }
pub enum LinkTarget { File(PathBuf), Unresolved(String) }
pub struct Link { pub kind: LinkKind, pub target: LinkTarget, pub range: SourceRange }
pub fn links_from(ws: &WorkspaceState, path: &Path) -> Vec<Link>;   // xrefs and includes only

// adoc-graph
pub enum GraphKey { File(Arc<Path>), Unresolved(String) }   // a normalised path, or the target's text
pub enum EdgeKind { Xref, Include, Nav }
pub enum NodeKind { Page, Partial, Nav, Unresolved }
pub struct NodeCoordinate { pub component: String, pub version: Option<String>, pub module: String, pub family: String }
pub struct GraphNode {
    pub key: GraphKey,
    pub title: String,                 // the document title, else the file name
    pub kind: NodeKind,
    pub coordinate: Option<NodeCoordinate>,   // filled by the engine from the Antora catalog
    pub links_out: usize,
    pub links_in: usize,
}
pub struct GraphEdge { pub from: GraphKey, pub to: GraphKey, pub kind: EdgeKind, pub count: usize }
pub struct GraphSnapshot { pub revision: u64, pub nodes: Vec<GraphNode>, pub edges: Vec<GraphEdge> }
pub struct GraphFilter {
    pub partials: bool, pub nav: bool, pub orphans: bool, pub unresolved: bool,
    pub components: Option<BTreeSet<String>>,   // None: every component
}
pub struct LinkStore { /* out, incoming, revision: see Tables */ }
impl LinkStore {
    pub fn set_links(&mut self, file: Arc<Path>, links: Vec<(GraphKey, EdgeKind)>);   // replaces the file's links
    pub fn remove(&mut self, file: &Path);
    pub fn linking_to(&self, key: &GraphKey) -> impl Iterator<Item = &Arc<Path>>;
    pub fn neighbourhood(&self, key: &GraphKey, depth: u8) -> BTreeSet<GraphKey>;   // both ways
    pub fn revision(&self) -> u64;
}

// adoc-engine
pub enum GraphScope { Workspace, Local { path: PathBuf, depth: u8 } }   // depth 1 to 3
impl Engine {
    pub fn graph(&self, scope: &GraphScope, filter: &GraphFilter) -> GraphSnapshot;
    pub fn graph_revision(&self) -> u64;
}
EngineEvent::GraphChanged { revision: u64 }
```

- **Resolving one file:** `links_from` resolves each of a file's references through `reference_target_path`, and each include through `adoc_index::resolve_include_target`, falling back to `reference_target_path` for resource ids, exactly as `includes_of` does. A target that cannot be resolved becomes `LinkTarget::Unresolved` with the target's text. A dynamic target (`include::{attr}/x.adoc[]`) makes no link at all. Conditional references follow the rules #178 set for diagnostics. Half-typed input never panics.
- **Navigation:** for each nav file the catalog lists, `Engine::nav_trees_of` gives its tree, and the edges are made as §2 says. A nav file is a `NodeKind::Nav` node.
- **Tables:** `adoc-graph`'s `LinkStore` follows the index's shape. Every table is keyed by file or holds the file's one shared `Arc<Path>`:
  - `out: BTreeMap<Arc<Path>, Vec<Link>>`: each file's resolved links, nav edges included;
  - `incoming: BTreeMap<GraphKey, BTreeSet<Arc<Path>>>`: the files linking to each target, so replacing or removing a file visits only what it contributed;
  - `revision`, bumped on every change.
- **The engine's part:** `adoc-engine/src/graph.rs` holds the `LinkStore`, maps `adoc-lang`'s `Link`s and the navigation edges into it, adds titles and coordinates when it builds a snapshot, and applies the filter.
- **Incremental updates:** wherever the engine replaces, adds or removes an index entry (typing after `INDEX_SETTLE`, a save, the watcher, a rename or its undo), it re-resolves that one file's links.
  - A new or renamed file may resolve links that were dangling. The files that might name it are found with the index's reverse maps (`xrefs_naming` and `includes_naming` for its file name and its `page-aliases`), and only those are re-resolved.
  - A changed nav file rebuilds only its own tree's edges.
  - A catalog change (an `antora.yml` added, changed or removed) rebuilds the graph as a whole, on a worker.
- **The full build:** runs on a worker started with `worker::spawn` once the workspace is indexed, over a snapshot of the parses. Its result is dropped if the index's revision moved meanwhile, and the build runs again. Until it lands, `graph` returns an empty snapshot whose revision is 0, and the views say "Building graph…".
- **Queries:** `GraphScope::Workspace` returns every node and edge the filter allows. `GraphScope::Local` walks outwards from the page through `out` and `incoming`, both ways, to `depth`, and never visits the whole graph. A local query on a file that is not indexed (an image, a non-AsciiDoc file) returns an empty snapshot.
- **Orphans:** a page with no links in or out, nav edges included. A page listed in a nav file is therefore never an orphan.
- **Event:** `GraphChanged` is published at most once per `pump`, after the graph changed.
- **Settings:** a new `[graph]` table, read through `Settings` and documented with the others:

  ```toml
  [graph]
  depth = 1              # the local graph's starting depth, 1 to 3
  show_partials = true
  show_nav = true
  show_orphans = true
  show_unresolved = false
  colour_by = "component"   # "component", "module" or "family"
  ```

Acceptance tests:

- `links_from_resolves_xrefs_and_includes` (adoc-lang): a relative xref, a cross-module and a cross-component xref, a `page-aliases` target and an include of a partial each resolve to the right file.
- `links_from_keeps_unresolved_and_skips_dynamic` (adoc-lang): a broken xref is `Unresolved`, a dynamic include makes no link, and `xref:` with no target does not panic.
- `link_store_replaces_and_removes_by_file` (adoc-graph): replacing a file's links updates `linking_to` for the old and new targets, and removing the file leaves nothing behind.
- `neighbourhood_walks_both_ways` (adoc-graph): depth 1 and depth 2 over a small hand-built store.
- `graph_builds_from_the_fixture` (engine, `crates/adoc-engine/tests/graph.rs`): the full build over `tests/fixtures/graph/` matches its `insta` snapshot.
- `nav_edges_follow_the_nav_tree` (engine): the nav file links to top-level entries, parents link to children, a text-only entry hands its children up, and the nav file's xrefs are not drawn twice.
- `typing_an_xref_updates_the_graph` (engine): typing an xref adds an edge and publishes `GraphChanged`, and deleting it removes the edge.
- `adding_the_missing_page_resolves_a_ghost` (engine): creating the page an unresolved xref names turns the ghost node into a page node without a full build.
- `renaming_a_page_moves_its_edges` (engine).
- `catalog_change_rebuilds_the_graph` (engine).
- `local_graph_visits_only_reachable_files` (engine): depth 1 and depth 2 give the expected nodes, and a counter shows no other file was visited.
- `graph_filters_apply` (engine).

## 5. The layout (WP 7.2, #272)

```rust
// adoc-graph
pub struct Layout { /* positions by GraphKey, velocities, pins, seed */ }
impl Layout {
    pub fn new(seed: u64) -> Self;
    pub fn sync(&mut self, snapshot: &GraphSnapshot);   // add and remove nodes, keeping the rest in place
    pub fn step(&mut self, dt: f32) -> bool;            // false once settled
    pub fn pin(&mut self, key: &GraphKey, at: Point);
    pub fn unpin(&mut self, key: &GraphKey);
    pub fn reset(&mut self);
    pub fn position(&self, key: &GraphKey) -> Option<Point>;
    pub fn node_at(&self, at: Point, radius: f32) -> Option<&GraphKey>;
}
```

- **Model:** every pair of nodes repels, every edge pulls its ends towards a rest length, every node is drawn gently to the centre, and the motion is damped. Above 500 nodes, repulsion uses a Barnes–Hut quadtree.
- **Placing new nodes:** a node added by `sync` starts next to a neighbour already placed, or at a seeded position on a ring. Existing nodes keep their positions.
- **Settling:** `step` returns `false` once the largest move falls below a threshold, so the app stops requesting animation frames. Dragging or a `sync` that changed something wakes it up.
- **Determinism:** the same seed and the same sequence of calls give the same positions on every platform. Only `f32` arithmetic in a fixed order, and no hashing order, is used.
- **`Point`** is `adoc-graph`'s own framework-free type (`x`, `y` in layout units). The app maps it to pixels with its pan and zoom.

Acceptance tests:

- `layout_is_deterministic`: two layouts with one seed give identical positions after 200 steps.
- `pinned_nodes_stay_put`
- `sync_keeps_existing_positions`: adding one node moves no existing node by more than one rest length.
- `layout_settles`: the fixture's graph settles within 500 steps.
- Benchmarks, held to their budgets like the others: one `step` for 2,000 nodes and 6,000 edges in under 8 ms (`cargo bench -p adoc-graph`), and a full build for 5,000 pages in under 500 ms (`cargo bench -p adoc-engine`), both in release builds.

## 6. The graph window (WP 7.3, #272)

- **Window:** `WindowKind::Graph`, opened by `view.graph` from the workspace window through `open_secondary_window`. It shows the workspace's graph and keeps a `Layout` of its own.
- **Size and position:** the window opens where it was last, at the size it last had.

  ```rust
  // adoc-engine
  pub struct WindowPlacement { pub x: f32, pub y: f32, pub width: f32, pub height: f32, pub maximised: bool }   // logical pixels
  impl Engine {
      pub fn graph_window(&self) -> Option<WindowPlacement>;
      pub fn set_graph_window(&mut self, placement: WindowPlacement);
  }
  ```

  - **Recording:** the app records the window's bounds whenever it is moved, resized, maximised or closed. A record changes only the engine's memory and writes nothing.
  - **Saving:** `write_session` writes the placement into `session.json` as an optional `graph_window` field, read with `#[serde(default)]`. `FORMAT_VERSION` stays 1, so older session files still load, and `restore_session` brings the placement back. With sessions disabled, the placement lasts only until the app quits.
  - **Opening:** the app passes the placement to `open_secondary_window` as the window's bounds, maximised if it was maximised. The first time, with nothing saved, the window opens centred at 1100 × 760.
  - **A missing display:** if less than half of the saved bounds lies on a connected display (a monitor unplugged, a smaller screen), the window opens centred on the main display at the saved size, shrunk to fit it.
  - **Scope:** only the graph window is remembered. The detached preview and the mirror keep opening centred, as now.
- **Toolbar:**
  - a search box that highlights matching nodes by title or path and dims the rest;
  - toggles for partials, navigation, orphans and unresolved targets;
  - a component filter;
  - colour by component, module or family;
  - a Workspace or Local switch, with a depth control from 1 to 3 when Local is chosen;
  - Reset Layout (`graph.reset_layout`).
- **Canvas:** pan by dragging the background, zoom with the wheel or a pinch around the pointer, and buttons for zoom in, zoom out and fit. Nodes are sized by their links, partials are hollow, unresolved targets have a dashed outline, and navigation files are squares.
- **Edge styles:**
  - `Xref` edges are solid lines and `Include` edges dashed.
  - `Nav` edges are dotted, in their own colour, and bowed into a gentle curve. Where a nav edge and an xref join the same two pages, both stay visible.
  - An edge to an unresolved target is dash-dot.
- **Interaction:**
  - hovering a node highlights it, its neighbours and their edges, and dims the rest;
  - a click selects the node and opens its page in the preview tab, and a double click opens it persistently;
  - dragging a node pins it, and releasing it with Alt held unpins it;
  - the context menu offers Open, Open to the Side, Find References and Reveal in Files;
  - a side panel lists the selected node's links out and backlinks, and clicking one selects it. Each neighbour has one row, naming every kind of link between the two (`xref · nav`). For a navigation file it lists the top-level entries.
- **Colours:** from the active gpui-component theme. The component and module palette is chosen to work in both light and dark themes, and the colours also differ in lightness.
- **Keyboard:** Tab moves between nodes in title order and Enter opens the focused one. Each node's accessible label is its title and resource id.
- **Updates:** on `GraphChanged` the window queries the engine again and calls `Layout::sync`. While the full build is running it shows "Building graph…".
- **Menus:** `view.graph` is added to `MENUS` in `src/menus.rs` with a keymap binding, and Show in Graph is added to the page tab's and the Files panel's context menus.

Acceptance tests (`ui_tests.rs`, on GPUI's test platform):

- `graph_window_opens_and_shows_the_workspace`
- `clicking_a_node_opens_its_page_as_a_preview`
- `double_clicking_a_node_keeps_its_page_open`
- `graph_window_refreshes_on_graph_changed`
- `opening_the_graph_again_focuses_its_window`
- `graph_window_reopens_where_it_was` (app): a window moved and resized, then closed and opened again, has the bounds it had.
- `graph_window_placement_survives_a_session` (engine): a placement set, written with `write_session` and brought back by `restore_session`, is the same. A session file without `graph_window` still restores, with no placement.
- `graph_window_off_screen_is_recentred` (app): a placement that lies off every display opens centred on the main display, at a size that fits it.

## 7. The local graph panel (WP 7.4, #272)

- **Panel:** `SidebarTab::Graph`, after Changes, titled Graph. It shows the active page's local graph to the depth set in its header, with partials and navigation toggles and a button that opens the graph window.
- **Following:** it follows `EngineEvent::NavigationChanged`. With an image or a non-AsciiDoc file active, it says "No page" and draws nothing.
- **Place in navigation:** below the canvas, a trail shows where the page sits in the navigation. It starts at the nav file, runs through the entries above the page to the page itself (shown in bold), and ends with the page's child entries (`nav.adoc › Introduction › Configuration › Settings reference`).
  - A page listed in several places, or in several nav files, has one trail for each.
  - A page in no navigation says "Not in any navigation".
  - Clicking an entry that has a page opens it as a preview, and clicking the nav file opens that file.
  - The trails hide with the navigation toggle.
- **Lists:** below the trail, Links out and Backlinks list the page's neighbours, one row for each neighbour, naming every kind of link between the two (`xref · nav`). A click opens one as a preview.
- **Sharing code:** the panel draws through the same `GraphView` as the window, with the toolbar reduced.
- `view.local_graph` shows the panel, and is in `MENUS`.
- **The engine's part:** the trails and the merged rows come from the engine, so the panel only draws them:

  ```rust
  // adoc-engine
  pub struct NavCrumb { pub title: String, pub path: Option<PathBuf> }   // None: a text-only entry
  pub struct NavPlace { pub nav: PathBuf, pub ancestors: Vec<NavCrumb>, pub page: NavCrumb, pub children: Vec<NavCrumb> }
  pub struct Neighbour { pub key: GraphKey, pub title: String, pub kinds: Vec<EdgeKind> }   // kinds in Xref, Include, Nav order
  impl Engine {
      pub fn nav_places(&self, path: &Path) -> Vec<NavPlace>;   // in catalog order, then source order
      pub fn neighbours(&self, path: &Path, filter: &GraphFilter) -> (Vec<Neighbour>, Vec<Neighbour>);   // links out, backlinks
  }
  ```

  `nav_places` reads the nav trees the graph already holds and visits no other file. The graph window's side panel uses `neighbours` too.

Acceptance tests:

- `local_graph_follows_the_active_tab`
- `local_graph_is_empty_for_an_image`
- `local_graph_lists_links_and_backlinks`
- `nav_places_trail_a_page_through_its_nav` (engine): over the fixture, a nested page's trail has its nav file, its ancestors and its children. A text-only ancestor is a crumb with no path, a page listed twice has two places, and a page in no navigation has none.
- `neighbours_merge_kinds_per_page` (engine): a page that is both an xref target and a nav child of the active page is one neighbour with the kinds `[Xref, Nav]`. With navigation filtered out, it has the kind `[Xref]` only.
- `local_graph_shows_the_place_in_navigation` (app): the panel draws the trail for a nested page, and "Not in any navigation" for an orphan.

## 8. Headless access (WP 7.5, #272)

```text
adocfoundry-cli graph <root> [--page <file> [--depth <n>]] [--format json|dot] [--no-partials] [--no-nav] [--unresolved]
```

- **CLI:** prints the workspace graph, or a page's local graph, as JSON (the `GraphSnapshot`, with paths relative to the root) or as Graphviz DOT. Git stays off, as for `check`.
- **MCP:** a read-only tool `link_graph`, taking a scope (`workspace`, or `page` with a `depth`) and the filter. It is always listed. A page's path goes through `paths::confine`.

Acceptance tests:

- `graph_prints_json` (CLI, an `insta` snapshot over the fixture).
- `graph_prints_dot` (CLI).
- `link_graph_tool_returns_a_local_graph` (adoc-mcp).

## 9. Out of Scope in Phase 7, Open Questions and Implementation Notes

Out of scope:

- Images as nodes.
- Saving pinned positions or a layout between sessions.
- A graph tab in the editor's tab strip.
- Editing links from the graph (drag to link, delete an edge).
- Graph views in `adoc-ls` clients.
- Sections and anchors as nodes, rather than whole pages.

Open questions, to settle before the package that needs them:

- [x] **WP 7.3:** whether the graph window should remember its size and position: it does, written with the session (§6). The detached preview does not remember its own, and stays as it is.

Implementation notes, as each package lands:

| Area | Specification | Implementation | Why |
|---|---|---|---|
| The full build (WP 7.1) | a worker started with `worker::spawn` once the workspace is indexed | The first build runs on the index worker itself, with the parses it has just made, and comes back with the index; so does the build after any whole-workspace reindex (a folder or playbook change, a burst of files), which replaces the index. A catalog change builds on a worker of its own, over a copy of the workspace. Resolving is spread over up to eight threads | No second copy of a large index is made, and the graph is never older than the index it is shown with. 5,000 pages build in about 90 ms locally |
| What counts as a catalog change (WP 7.1) | an `antora.yml` added, changed or removed | Only a change to the catalog's components, versions, roots, navigation lists or modules builds the whole graph again. New Page, a rename and their undo re-read a descriptor too, but the pages that came or went with it are followed file by file | `adding_the_missing_page_resolves_a_ghost` and `renaming_a_page_moves_its_edges` must not need a full build |
| Changes during a full build (WP 7.1) | the result is dropped if the index moved meanwhile, and the build runs again | The result is adopted, then the files marked since it began are resolved again on top of it | The same graph, without a second whole pass |
| The full build reads no file (WP 7.1) | it reads no file | It reads the navigation files the catalog lists (from their open buffers when open), since the index keeps their parse but not their text | A tree needs the nav file's text; there are few of them |
| `LinkStore` (WP 7.1) | `set_links(file, links)`; `linking_to` the files linking to a target | Every link is owned by the file whose text made it: a page owns its own, a navigation file owns its tree's, which leave the pages its entries name (`set_owned_links`). `linking_to` gives the owners, the files to resolve again when a target comes or goes; `links_from` and `links_to` give the links themselves. The reverse maps are hashed | A page's xrefs and the navigation's edges from that page are replaced independently. A path compares component by component, too slowly in ordered maps for a full build's insertions |
| Skipping unchanged files (WP 7.1) | a replaced index entry re-resolves that file | A fingerprint of what a file's links come from (its xref and include targets, which includes are conditional, and its attributes when a target names one) is kept, and a file whose fingerprint is unchanged is not resolved again | Typing prose in a long page would otherwise resolve every link of it at each pause (`settle_git` went from 26 µs to 3 ms) |
| Which links are made (WP 7.1) | xrefs and includes, unresolved targets kept | An xref into a component or version missing from the workspace makes no link, as it is published elsewhere and the diagnostics say nothing of it. An include of a file that is not AsciiDoc (an example's source code) makes no link. An unresolved navigation entry makes no ghost node, as the nav tree keeps no text for it | Ghosts are for broken links, and images and other assets are out of scope |
| Partials outside Antora (WP 7.1) | partials are files reached by `include::` | In a module, the `partials` family decides; outside Antora, a file another includes is a partial | Plain workspaces have no families |
| Snapshot benchmark (WP 7.2) | the full build and a layout step are budgeted | A whole-workspace snapshot of 5,000 pages (`graph_snapshot`, about 50 ms locally) is recorded, not budgeted | The window asks for one only when the graph changes, and CI runners are a few times slower |
| Opening a node (WP 7.3) | a click opens with `Preview`, a double click with `Persistent`, through `navigate.go_to` | A click invokes `navigate.go_to`, which opens as a preview; a double click does the same and then `document.keep_open` | `navigate.go_to` takes a position only |
| Open to the Side (WP 7.3) | in the context menu | Opens the page in a tab of its own and pins the docked preview to it | The app has no editor split; the pinned preview is what shows a page beside the one being edited |
| The side panel (WP 7.3) | lists the selected node's links | Always shown in the window, saying "Select a node to see its links." when nothing is selected | Showing it on selection narrowed the canvas under the pointer, so a double click missed its node |
| Accessible labels (WP 7.3) | each node's accessible label is its title and resource id | Not done: GPUI has no accessibility tree to label | Left for when GPUI has one |
| Show in Graph (WP 7.3) | in the page tab's and the Files panel's context menus | At the end of the page tab's menu, after the close commands, and for AsciiDoc files in the Files panel's | The tab menu is walked with the arrow keys; new items go last |
| Keyboard (WP 7.3) | Tab moves between nodes in title order and Enter opens the focused one | Bound as actions (`NextNode`, `PreviousNode`, `OpenNode`) in the canvas's `Graph` key context | gpui-component's `Root` binds Tab to move focus, and a bound key never reaches a key-down handler |
| Visual checks (WP 7.3, 7.4) | — | Drawing, colours in both themes, pinch and the window's placement on a second display are checked by hand, `docs/qa/phase7.md` | GPUI's test platform draws no pixels and has one display |

## 10. Implementation Sequence

| # | Package | Depends on |
|---|---|---|
| 7.1 | The link graph: the `adoc-graph` crate and its `LinkStore`, `links_from`, navigation edges, incremental updates, queries, `GraphChanged`, `[graph]` settings | Phase 6 |
| 7.2 | The layout and its benchmark | 7.1 |
| 7.3 | The graph window | 7.2 |
| 7.4 | The local graph sidebar panel | 7.3 |
| 7.5 | `adocfoundry-cli graph` and the `link_graph` MCP tool | 7.1 |

The fixture `tests/fixtures/graph/` is added in WP 7.1. It holds an Antora component with a nav file that has a text-only entry, an orphan page, a partial included twice, a cross-component xref, a `page-aliases` target and a broken xref.

## 11. Definition of Done for Phase 7

- The graph window shows the workspace's pages, partials and navigation files, with xref, include and navigation edges. Partials, navigation, orphans and unresolved targets can each be filtered.
- Pan, zoom, drag, hover highlighting, search, a click to open as a preview and a double click to open persistently all work. The local graph panel follows the active tab.
- The graph window reopens at its last size and position, across restarts, and recentres when that place is no longer on a display.
- The graph stays current while typing and when files change on disk, with no full rebuild except after a catalog change.
- A workspace of a few thousand pages stays interactive, and the benchmark holds its budget.
- Every acceptance test above exists and passes. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check` and `cargo xtask layers` pass.
- README, the documentation site, `CHANGELOG.md` and `docs/qa/phase7.md` describe what shipped.
