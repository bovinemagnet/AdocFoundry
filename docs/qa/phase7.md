# Phase 7 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Phase 7 acceptance (spec §11) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ and Ubuntu 22.04+ from a build of the `phase-7` branch or a release, unless the item names a platform. Record the build, the platform and the result in the table at the end.

Automated coverage for reference:

- **The link graph:** `crates/adoc-engine/tests/graph.rs` (the full build's snapshot, navigation edges, typing an xref, a ghost resolving, a rename, a catalog change, local walks visiting only what they reach, filters, navigation trails and merged neighbours), `crates/adoc-lang/src/graph.rs` (`links_from`) and `crates/adoc-graph` (the store, walks, snapshots and the layout's determinism, pinning and settling).
- **Budgets:** `cargo bench -p adoc-graph` (one layout step, 2,000 nodes and 6,000 edges, under 8 ms) and `graph_build` in `cargo bench -p adoc-engine` (5,000 pages under 500 ms).
- **The window and the panel:** the `graph_window_*`, `clicking_a_node_*`, `double_clicking_a_node_*`, `opening_the_graph_again_*` and `local_graph_*` tests, and the interaction tests driven by mouse, wheel, pinch and key events (`graph_canvas_pans_and_zooms_around_the_pointer`, `dragging_a_node_pins_it_and_alt_lets_it_go`, `hovering_a_node_highlights_it_and_its_neighbours`, `graph_toolbar_filters_and_searches`, `tab_moves_through_the_nodes_and_enter_opens_one`) in `crates/adocfoundry/src/ui_tests.rs`, and `graph_window_placement_survives_a_session` in `crates/adoc-engine/tests/session.rs`. GPUI's test platform draws no pixels and has one display, so what follows is checked by eye.
- **Headless access:** `graph_prints_json`, `graph_prints_dot` (CLI) and `link_graph_tool_returns_a_local_graph` (MCP).

## Setup

1. A build of the app, and `git clone --recurse-submodules https://github.com/bovinemagnet/AdocFoundry.git` for `tests/fixtures/graph`.
2. A large workspace of a few thousand pages, such as a real documentation repository.
3. For the placement checks, a second display that can be unplugged.

## The graph window

- [ ] Open `tests/fixtures/graph` and choose View › Show Graph (Cmd/Ctrl-K G). The window opens centred, about 1100 × 760, says "Building graph…" at most briefly, then settles within a second or two.
- [ ] In the light theme and the dark: nodes are coloured by component and readable on the background; partials are hollow, `nav.adoc` is a square, and with Unresolved on `missing.adoc` has a dashed outline.
- [ ] Edges: xrefs solid, includes dashed, navigation dotted, bowed and in their own colour; `index.adoc` → `install.adoc` (two xrefs) is heavier; where navigation and an xref join Install and Configure, both are visible.
- [ ] Hovering a node highlights it, its neighbours and their edges and dims the rest; labels appear on hover and once zoomed in.
- [ ] Drag the background to pan; the wheel zooms around the pointer; on a trackpad, pinch zooms (macOS).
- [ ] Drag a node: it stays where it is dropped while the rest settle around it. Drag it again and release with Alt held: it is let go.
- [ ] Click a node: its page opens in the preview tab of the main window, and the side panel lists its links out and backlinks with their kinds (`xref · nav` for Configure from Install). Double click: the page is kept open.
- [ ] Right-click a node: Open, Open to the Side (the page opens and the docked preview is pinned to it), Find References (the panel below the editor lists them) and Reveal in Files.
- [ ] With Unresolved on, right-click `missing.adoc`: Create Page makes `guide/modules/ROOT/pages/missing.adoc` titled "Missing" and opens it, and the ghost becomes a page linked from Install. Undo takes it back.
- [ ] Search for `inst`: Install is highlighted and labelled, the rest dimmed.
- [ ] The toggles hide and show partials, navigation, orphans (`orphan.adoc`) and unresolved targets; the component menu hides `reference`; Colour cycles component, module and family; Local shows the active page's neighbourhood at depths 1 to 3; Reset Layout lays it out again and unpins.
- [ ] Tab and Shift-Tab move the selection through the nodes in title order, and Enter opens the selected one.
- [ ] Choose View › Show Graph again: the same window comes to the front.
- [ ] Type an xref into a page and pause: the edge appears. Delete it: the edge goes. Add a page on disk that a broken xref names: the ghost becomes a page.

## The window's place

- [ ] Move and resize the window, quit, relaunch with nothing to open, and open the graph: it is where it was, at the size it had. The same when maximised.
- [ ] Move it to a second display, quit, unplug the display, relaunch and open the graph: it opens centred on the main display, no larger than it.

## The Graph panel

- [ ] Choose View › Show Local Graph (Cmd/Ctrl-K Shift-G): the Graph tab of the sidebar shows the active page and its neighbours, and follows as other tabs are chosen. With an image active it says "No page".
- [ ] For `configure.adoc` the trail reads `nav.adoc › Introduction › Install › Configure`, with Configure in bold; Install has two trails; `orphan.adoc` says "Not in any navigation". Clicking Install opens it; clicking `nav.adoc` opens the nav file. The trails hide with Navigation off.
- [ ] Links out and Backlinks list each neighbour once, naming every kind of link; a click opens it as a preview.
- [ ] Show in Graph, from a page tab's menu and from a page in the Files panel, opens the window with that page selected and in view.

## A large workspace

- [ ] On a workspace of a few thousand pages, the graph window pans, zooms and drags without stutter once the layout has settled, and typing in a page stays as quick as with the window closed.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
| 2026-11-02 | `phase-7` debug | macOS (Metal), off-screen windows captured from their textures | Pass | `tests/fixtures/graph` drawn in the window (light and dark) and the panel: pages coloured by component, partials hollow, `nav.adoc` square, xrefs solid, includes dashed, navigation dotted and bowed, Home → Install heavier; selecting Install rings it, keeps its neighbours lit and dims the rest, and the side panel lists Configure as `xref · nav`; the panel shows both of Install's navigation trails and its links. Icons were not drawn, the capture having no assets. Pinch and the second display remain. |
| 2026-11-02 | `phase-7` release | macOS (Metal), off-screen window 1100 × 760 | Pass | Frame times of the graph window, measured through the real renderer. A synthetic workspace of 3,060 pages and 24,000 edges indexed and built its graph in 187 ms. A frame takes 5.5 ms on the CPU (layout step, scene and paint), and about 22 ms with the GPU render and a full-size readback, while the layout moves and on hover; a pan step takes 28 ms with the readback. The real 807-page corpus takes 20 to 23 ms a frame with the readback. Measuring first found 46 ms a frame spent in path lookups while building the drawing, now done by place. |
| 2026-11-02 | `phase-7` release | macOS, at the machine | Pass | The graph window and panel looked right to the user on `tests/fixtures/graph`; they found it "maybe a little slow on the long page". The typing benchmark (`ADOCFOUNDRY_TYPING_BENCH=10`) on a 5,000-line page of that workspace gives a median frame of 16.7 ms, p99 19 ms and one frame over 25 ms in ten seconds, the same with the Navigation panel or the Graph panel showing. |
| 2026-11-02 | `phase-7` release | macOS, at the machine | Pass, two items open | The user's verdict on a second look: "looks good". They zoomed with a mouse's scroll wheel, which passes the wheel half of the zoom item; a trackpad pinch is a separate event (`PinchEvent`) and was not tried. Neither the pinch nor the unplugged display has been checked by hand: the pinch is covered only by `graph_canvas_pans_and_zooms_around_the_pointer` (a synthetic `PinchEvent`), and the recentring only by `a_window_mostly_off_every_display_is_brought_back` and `graph_window_off_screen_is_recentred`. The slow "long page" was not named; typing on the 5,000-line page measured above is within budget. |
