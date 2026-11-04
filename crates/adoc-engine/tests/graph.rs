//! The link graph: built once over the workspace, then kept current file by file as pages are
//! typed, created, renamed and removed, with a full build again only after a catalog change.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{mpsc::Receiver, Arc},
    time::{Duration, Instant},
};

use adoc_core::{
    probe::{self, Probe},
    SourceRange,
};
use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, CommandOutcome, EdgeKind, Engine, EngineEvent,
    GraphFilter, GraphKey, GraphNodeKind, GraphScope, GraphSnapshot, GroupBy, Groups, Layout,
    LinkStyle, ManualClock, MindMap, NavCrumb, NewPage, Settings, TreeStyle, INDEX_SETTLE,
};

fn fixture() -> PathBuf {
    normalize_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/graph"))
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn open(root: &Path, clock: &ManualClock) -> Engine {
    let settings = Settings::load(None, None).unwrap().with_git_opt_in();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    assert!(!engine.is_graph_building());
    engine
}

/// A copy of the fixture to change, and an engine over it.
struct Copy {
    _directory: tempfile::TempDir,
    root: PathBuf,
    clock: ManualClock,
    engine: Engine,
    events: Receiver<EngineEvent>,
}

impl Copy {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        copy_tree(&fixture(), &root);
        let clock = ManualClock::new();
        let mut engine = open(&root, &clock);
        let events = engine.subscribe();
        Self {
            _directory: directory,
            root,
            clock,
            engine,
            events,
        }
    }

    fn page(&self, name: &str) -> PathBuf {
        self.root.join("guide/modules/ROOT/pages").join(name)
    }

    /// Pumps until the graph changes, or ten seconds pass.
    fn until_graph_changed(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.engine.pump();
            if self
                .events
                .try_iter()
                .any(|event| matches!(event, EngineEvent::GraphChanged { .. }))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

fn file(path: &Path) -> GraphKey {
    GraphKey::File(Arc::from(normalize_path(path)))
}

fn name(key: &GraphKey, root: &Path) -> String {
    match key {
        GraphKey::File(path) => path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/"),
        GraphKey::Unresolved(text) => format!("?{text}"),
        GraphKey::Group(name) => format!("[{name}]"),
    }
}

fn edges(snapshot: &GraphSnapshot, root: &Path) -> Vec<(String, String, EdgeKind)> {
    snapshot
        .edges
        .iter()
        .map(|edge| (name(&edge.from, root), name(&edge.to, root), edge.kind))
        .collect()
}

fn names(snapshot: &GraphSnapshot, root: &Path) -> Vec<String> {
    snapshot
        .nodes
        .iter()
        .map(|node| name(&node.key, root))
        .collect()
}

fn has_edge(snapshot: &GraphSnapshot, root: &Path, from: &str, to: &str, kind: EdgeKind) -> bool {
    edges(snapshot, root).contains(&(from.to_owned(), to.to_owned(), kind))
}

fn everything() -> GraphFilter {
    GraphFilter {
        unresolved: true,
        ..GraphFilter::default()
    }
}

#[test]
fn graph_builds_from_the_fixture() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let snapshot = engine.graph(&GraphScope::Workspace, &everything());
    assert!(snapshot.revision > 0);
    let nodes: Vec<serde_json::Value> = snapshot
        .nodes
        .iter()
        .map(|node| {
            serde_json::json!({
                "node": name(&node.key, &root),
                "title": node.title,
                "kind": format!("{:?}", node.kind),
                "coordinate": node.coordinate.as_ref().map(|coordinate| format!(
                    "{}@{}:{}:{}",
                    coordinate.version.as_deref().unwrap_or("~"),
                    coordinate.component,
                    coordinate.module,
                    coordinate.family
                )),
                "out": node.links_out,
                "in": node.links_in,
            })
        })
        .collect();
    let edges: Vec<String> = snapshot
        .edges
        .iter()
        .map(|edge| {
            format!(
                "{} -{:?}×{}-> {}",
                name(&edge.from, &root),
                edge.kind,
                edge.count,
                name(&edge.to, &root)
            )
        })
        .collect();
    insta::assert_json_snapshot!(serde_json::json!({ "nodes": nodes, "edges": edges }));
}

/// Grouped by module, the fixture's layout settles with every node nearer the centre of its
/// own module than of any other, and a module collapses into one node.
#[test]
fn grouped_layout_keeps_modules_together() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let snapshot = engine.graph(&GraphScope::Workspace, &everything());
    let groups = Groups::of(&snapshot, GroupBy::Module);
    assert_eq!(
        groups.names,
        [
            "1.0@guide:ROOT",
            "1.0@guide:admin",
            "Other",
            "reference:ROOT"
        ]
    );
    let mut layout = Layout::new(7);
    layout.sync(&snapshot);
    layout.set_grouping(Some(GroupBy::Module), &snapshot);
    layout.settle(2_000);
    assert!(!layout.is_moving());
    let at = |key: &GraphKey| layout.position(key).unwrap();
    let mut centres = vec![(0.0_f32, 0.0_f32, 0.0_f32); groups.names.len()];
    for (node, &group) in snapshot.nodes.iter().zip(&groups.of) {
        let point = at(&node.key);
        centres[group] = (
            centres[group].0 + point.x,
            centres[group].1 + point.y,
            centres[group].2 + 1.0,
        );
    }
    for (node, &own) in snapshot.nodes.iter().zip(&groups.of) {
        let point = at(&node.key);
        let distance = |(x, y, n): (f32, f32, f32)| {
            ((point.x - x / n).powi(2) + (point.y - y / n).powi(2)).sqrt()
        };
        for (group, &centre) in centres.iter().enumerate() {
            assert!(
                group == own || distance(centres[own]) < distance(centre),
                "{} is nearer {}",
                name(&node.key, &root),
                groups.names[group]
            );
        }
    }

    let collapsed = snapshot.collapse(GroupBy::Module, &["1.0@guide:ROOT".to_owned()].into());
    let names = names(&collapsed, &root);
    assert!(names.contains(&"[1.0@guide:ROOT]".to_owned()), "{names:?}");
    assert!(!names
        .iter()
        .any(|name| name.starts_with("guide/modules/ROOT")));
    let group = GraphKey::Group("1.0@guide:ROOT".to_owned());
    assert_eq!(
        collapsed.node(&group).map(|node| node.kind),
        Some(GraphNodeKind::Group)
    );
    // The nav file's link to Users, and Install's, are one edge from the group now.
    assert!(collapsed
        .edges
        .iter()
        .any(|edge| edge.from == group && name(&edge.to, &root).ends_with("users.adoc")));
}

/// Rooted at `nav.adoc`, the mind map is the navigation's nesting: Home, Install (with
/// Configure under it) and Users, the text-only Introduction handing its entries to the
/// navigation file, and every node placed once.
#[test]
fn mind_map_follows_the_nav_tree() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let snapshot = engine.graph(&GraphScope::Workspace, &everything());
    let nav = file(&root.join("guide/modules/ROOT/nav.adoc"));
    let tree = MindMap::build(&snapshot, &nav, Some(1)).unwrap();
    let titles = |keys: Vec<&GraphKey>| -> Vec<String> {
        keys.into_iter()
            .map(|key| snapshot.node(key).unwrap().title.clone())
            .collect()
    };
    assert_eq!(titles(tree.children(&nav)), ["Home", "Install", "Users"]);
    let install = file(&root.join("guide/modules/ROOT/pages/install.adoc"));
    assert_eq!(titles(tree.children(&install)), ["Configure"]);
    assert_eq!(tree.len(), 5, "the navigation's pages, each once");
    // Install's xref to Configure is the tree's link; Home's to Install is a cross-link.
    let home = file(&root.join("guide/modules/ROOT/pages/index.adoc"));
    assert!(!tree.is_tree_link(&home, &install));
    let positions = tree.positions(TreeStyle::Horizontal, &Default::default());
    assert!(positions[&nav].x < positions[&install].x);
}

#[test]
fn nav_edges_follow_the_nav_tree() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let snapshot = engine.graph(&GraphScope::Workspace, &GraphFilter::default());
    let nav: Vec<(String, String)> = edges(&snapshot, &root)
        .into_iter()
        .filter(|(.., kind)| *kind == EdgeKind::Nav)
        .map(|(from, to, _)| (from, to))
        .collect();
    let pages = "guide/modules/ROOT/pages";
    let nav_file = "guide/modules/ROOT/nav.adoc".to_owned();
    assert_eq!(
        nav,
        [
            // Top-level entries, and the children of the text-only Introduction.
            (nav_file.clone(), format!("{pages}/index.adoc")),
            (nav_file.clone(), format!("{pages}/install.adoc")),
            (
                nav_file.clone(),
                "guide/modules/admin/pages/users.adoc".to_owned()
            ),
            (
                format!("{pages}/install.adoc"),
                format!("{pages}/configure.adoc")
            ),
        ]
    );
    // Install is listed twice under the nav file: one edge, counted twice.
    let twice = snapshot
        .edges
        .iter()
        .find(|edge| name(&edge.to, &root).ends_with("install.adoc") && edge.kind == EdgeKind::Nav)
        .unwrap();
    assert_eq!(twice.count, 2);
    // The nav file's own xrefs are not drawn again.
    assert!(edges(&snapshot, &root)
        .iter()
        .all(|(from, _, kind)| *from != nav_file || *kind == EdgeKind::Nav));
    let nav_node = snapshot.node(&file(&root.join(&nav_file))).unwrap();
    assert_eq!(nav_node.kind, GraphNodeKind::Nav);
}

#[test]
fn typing_an_xref_updates_the_graph() {
    let mut copy = Copy::new();
    let orphan = copy.page("orphan.adoc");
    let id = copy.engine.open(&orphan).unwrap();
    copy.engine.pump();
    let _ = copy.events.try_iter().count();
    let end = copy.engine.document(id).unwrap().text.len();
    let typed = "See xref:index.adoc[].\n";
    copy.engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end),
                text: typed.to_owned(),
            },
        )
        .unwrap();
    copy.clock.advance(INDEX_SETTLE);
    assert!(copy.until_graph_changed());
    let snapshot = copy
        .engine
        .graph(&GraphScope::Workspace, &GraphFilter::default());
    let (from, to) = (
        "guide/modules/ROOT/pages/orphan.adoc",
        "guide/modules/ROOT/pages/index.adoc",
    );
    assert!(has_edge(&snapshot, &copy.root, from, to, EdgeKind::Xref));

    copy.engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end + typed.len()),
                text: String::new(),
            },
        )
        .unwrap();
    copy.clock.advance(INDEX_SETTLE);
    assert!(copy.until_graph_changed());
    let snapshot = copy
        .engine
        .graph(&GraphScope::Workspace, &GraphFilter::default());
    assert!(!has_edge(&snapshot, &copy.root, from, to, EdgeKind::Xref));
    assert_eq!(copy.engine.graph_builds(), 1, "no full build");
}

#[test]
fn adding_the_missing_page_resolves_a_ghost() {
    let mut copy = Copy::new();
    let ghost = GraphKey::Unresolved("missing.adoc".to_owned());
    let before = copy.engine.graph(&GraphScope::Workspace, &everything());
    assert_eq!(before.node(&ghost).unwrap().kind, GraphNodeKind::Unresolved);

    let outcome = copy
        .engine
        .invoke(
            ids::PAGE_NEW,
            CommandArgs::NewPage(NewPage {
                module: copy.root.join("guide/modules/ROOT"),
                title: "Missing".to_owned(),
                add_to_nav: false,
                file: None,
            }),
        )
        .unwrap();
    assert!(matches!(outcome, CommandOutcome::Opened(_)));
    assert!(copy.until_graph_changed());
    let after = copy.engine.graph(&GraphScope::Workspace, &everything());
    assert!(after.node(&ghost).is_none());
    let missing = after.node(&file(&copy.page("missing.adoc"))).unwrap();
    assert_eq!((missing.kind, missing.links_in), (GraphNodeKind::Page, 1));
    assert!(has_edge(
        &after,
        &copy.root,
        "guide/modules/ROOT/pages/install.adoc",
        "guide/modules/ROOT/pages/missing.adoc",
        EdgeKind::Xref
    ));
    assert_eq!(copy.engine.graph_builds(), 1, "no full build");
}

#[test]
fn a_ghost_offers_the_page_its_links_expect() {
    let mut copy = Copy::new();
    let request = copy.engine.new_page_for("missing.adoc").unwrap();
    assert_eq!(
        request,
        NewPage {
            module: copy.root.join("guide/modules/ROOT"),
            title: "Missing".to_owned(),
            add_to_nav: false,
            file: Some(PathBuf::from("missing.adoc")),
        }
    );
    // A target that resolves, or that no link names, has no page to make.
    assert_eq!(copy.engine.new_page_for("install.adoc"), None);
    assert_eq!(copy.engine.new_page_for("unheard-of.adoc"), None);

    copy.engine
        .invoke(ids::PAGE_NEW, CommandArgs::NewPage(request))
        .unwrap();
    assert!(copy.until_graph_changed());
    let after = copy.engine.graph(&GraphScope::Workspace, &everything());
    assert!(after
        .node(&GraphKey::Unresolved("missing.adoc".to_owned()))
        .is_none());
    assert!(has_edge(
        &after,
        &copy.root,
        "guide/modules/ROOT/pages/install.adoc",
        "guide/modules/ROOT/pages/missing.adoc",
        EdgeKind::Xref
    ));
    assert_eq!(copy.engine.new_page_for("missing.adoc"), None);
}

/// Opens `path` and types `text` at its end, and waits for the graph to follow.
fn type_at_end(copy: &mut Copy, path: &Path, text: &str) -> adoc_engine::DocumentId {
    let id = copy.engine.open(path).unwrap();
    let end = copy.engine.document(id).unwrap().text.len();
    copy.engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end),
                text: text.to_owned(),
            },
        )
        .unwrap();
    copy.clock.advance(INDEX_SETTLE);
    assert!(copy.until_graph_changed());
    id
}

#[test]
fn a_ghost_include_offers_the_partial_it_expects() {
    let mut copy = Copy::new();
    let install = copy.page("install.adoc");
    type_at_end(&mut copy, &install, "\ninclude::partial$tips.adoc[]\n");
    let partial = copy.root.join("guide/modules/ROOT/partials/tips.adoc");
    assert_eq!(
        copy.engine.new_partial_for("partial$tips.adoc"),
        Some(partial.clone())
    );
    // An xref's ghost is a page's, not a partial's.
    assert_eq!(copy.engine.new_partial_for("missing.adoc"), None);
    assert_eq!(copy.engine.new_page_for("partial$tips.adoc"), None);

    let outcome = copy
        .engine
        .invoke(ids::PAGE_NEW_PARTIAL, CommandArgs::Path(partial.clone()))
        .unwrap();
    let CommandOutcome::Opened(id) = outcome else {
        panic!("expected the partial opened, got {outcome:?}");
    };
    assert_eq!(copy.engine.active(), Some(id));
    assert_eq!(fs::read_to_string(&partial).unwrap(), "");
    assert!(copy.until_graph_changed());
    let after = copy.engine.graph(&GraphScope::Workspace, &everything());
    assert!(after
        .node(&GraphKey::Unresolved("partial$tips.adoc".to_owned()))
        .is_none());
    assert_eq!(
        after.node(&file(&partial)).unwrap().kind,
        GraphNodeKind::Partial
    );
    assert!(has_edge(
        &after,
        &copy.root,
        "guide/modules/ROOT/pages/install.adoc",
        "guide/modules/ROOT/partials/tips.adoc",
        EdgeKind::Include
    ));

    // One undo takes it back; a file there already, or outside the workspace, is refused.
    copy.engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert!(!partial.exists());
    for path in [
        copy.page("install.adoc"),
        PathBuf::from("/tmp/elsewhere.adoc"),
    ] {
        let refused = copy
            .engine
            .invoke(ids::PAGE_NEW_PARTIAL, CommandArgs::Path(path.clone()));
        assert!(refused.is_err(), "{}: {refused:?}", path.display());
    }
}

#[test]
fn a_ghost_include_outside_antora_is_made_beside_its_page() {
    let mut copy = Copy::new();
    let chapter = copy.root.join("loose/chapter.adoc");
    type_at_end(&mut copy, &chapter, "\ninclude::parts/_gone.adoc[]\n");
    assert_eq!(
        copy.engine.new_partial_for("parts/_gone.adoc"),
        Some(copy.root.join("loose/parts/_gone.adoc"))
    );
}

#[test]
fn a_link_from_the_active_page_is_written_as_it_resolves() {
    let mut copy = Copy::new();
    let install = copy.page("install.adoc");
    let id = copy.engine.open(&install).unwrap();
    let text = copy.engine.document(id).unwrap().text.to_string();
    let link = |copy: &Copy, offset: usize, target: &Path, style: LinkStyle| {
        copy.engine
            .link_edit(id, offset, target, style)
            .map(|edit| (edit.range.start, edit.replacement))
    };
    // An xref goes at the offset, the shortest id that names the page from here.
    let after_see = text.find("See ").unwrap() + "See ".len();
    for (target, written) in [
        (copy.page("configure.adoc"), "xref:configure.adoc[]"),
        (
            copy.root.join("guide/modules/admin/pages/users.adoc"),
            "xref:admin:users.adoc[]",
        ),
        (
            copy.root.join("reference/modules/ROOT/pages/api.adoc"),
            "xref:reference:ROOT:api.adoc[]",
        ),
    ] {
        assert_eq!(
            link(&copy, after_see, &target, LinkStyle::Xref),
            Some((after_see, written.to_owned()))
        );
    }
    // Straight after a word it is set apart by a space.
    let in_word = text.find("See").unwrap() + "See".len();
    assert_eq!(
        link(
            &copy,
            in_word,
            &copy.page("configure.adoc"),
            LinkStyle::Xref
        ),
        Some((in_word, " xref:configure.adoc[]".to_owned()))
    );
    // An include goes on a line of its own after the offset's line, or on the offset's line
    // when that is blank.
    let note = copy.root.join("guide/modules/ROOT/partials/note.adoc");
    let line_end = after_see + text[after_see..].find('\n').unwrap();
    assert_eq!(
        link(&copy, after_see, &note, LinkStyle::Include),
        Some((line_end, "\ninclude::partial$note.adoc[]".to_owned()))
    );
    let blank = text.find("\n\n").unwrap() + 1;
    assert_eq!(
        link(&copy, blank, &note, LinkStyle::Include),
        Some((blank, "include::partial$note.adoc[]".to_owned()))
    );
    // Nothing links a page to itself, or to a file it cannot name.
    assert_eq!(link(&copy, 0, &install, LinkStyle::Xref), None);
    assert_eq!(
        link(
            &copy,
            0,
            &copy.root.join("loose/chapter.adoc"),
            LinkStyle::Xref
        ),
        None
    );

    // Outside Antora, a link is a relative path.
    let chapter = copy.root.join("loose/chapter.adoc");
    let loose = copy.engine.open(&chapter).unwrap();
    let edit = copy
        .engine
        .link_edit(
            loose,
            0,
            &copy.root.join("loose/_part.adoc"),
            LinkStyle::Include,
        )
        .unwrap();
    assert_eq!(edit.replacement, "\ninclude::_part.adoc[]");
}

/// The path between two files of the fixture, as names relative to the root and the kinds
/// and direction of the links taken.
fn path_between(
    engine: &Engine,
    from: &str,
    to: &str,
    filter: &GraphFilter,
    directed: bool,
) -> Option<Vec<(String, Vec<EdgeKind>, bool)>> {
    let root = fixture();
    let key = |name: &str| file(&root.join(name));
    engine
        .graph_path(
            &GraphScope::Workspace,
            filter,
            &key(from),
            &key(to),
            directed,
        )
        .map(|steps| {
            steps
                .into_iter()
                .map(|step| (name(&step.key, &root), step.kinds, step.backwards))
                .collect()
        })
}

const HOME: &str = "guide/modules/ROOT/pages/index.adoc";
const INSTALL: &str = "guide/modules/ROOT/pages/install.adoc";
const CONFIGURE: &str = "guide/modules/ROOT/pages/configure.adoc";

#[test]
fn shortest_path_follows_links_in_order() {
    let engine = open(&fixture(), &ManualClock::new());
    let filter = GraphFilter::default();
    assert_eq!(
        path_between(&engine, HOME, CONFIGURE, &filter, true).unwrap(),
        [
            (HOME.to_owned(), vec![], false),
            (INSTALL.to_owned(), vec![EdgeKind::Xref], false),
            (
                CONFIGURE.to_owned(),
                vec![EdgeKind::Xref, EdgeKind::Nav],
                false
            ),
        ]
    );
    // Users links to Install, which leads round to Home.
    let users = "guide/modules/admin/pages/users.adoc";
    let found = path_between(&engine, users, HOME, &filter, true).unwrap();
    let names: Vec<&str> = found.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(names, [users, INSTALL, CONFIGURE, HOME]);
}

#[test]
fn shortest_path_can_ignore_direction() {
    let engine = open(&fixture(), &ManualClock::new());
    let filter = GraphFilter::default();
    // Configure links back to Home: against its direction, one step.
    assert_eq!(
        path_between(&engine, HOME, CONFIGURE, &filter, false).unwrap(),
        [
            (HOME.to_owned(), vec![], false),
            (CONFIGURE.to_owned(), vec![EdgeKind::Xref], true),
        ]
    );
    // Nothing links the orphan, either way.
    let orphan = "guide/modules/ROOT/pages/orphan.adoc";
    assert_eq!(path_between(&engine, HOME, orphan, &filter, true), None);
    assert_eq!(path_between(&engine, HOME, orphan, &filter, false), None);
}

#[test]
fn shortest_path_respects_filters() {
    let engine = open(&fixture(), &ManualClock::new());
    let note = "guide/modules/ROOT/partials/note.adoc";
    let nav = "guide/modules/ROOT/nav.adoc";
    let shown = GraphFilter::default();
    let found = path_between(&engine, CONFIGURE, note, &shown, true).unwrap();
    assert_eq!(
        found.last().unwrap(),
        &(note.to_owned(), vec![EdgeKind::Include], false)
    );
    assert!(path_between(&engine, nav, CONFIGURE, &shown, true).is_some());

    let no_partials = GraphFilter {
        partials: false,
        ..GraphFilter::default()
    };
    assert_eq!(
        path_between(&engine, CONFIGURE, note, &no_partials, true),
        None
    );
    let no_nav = GraphFilter {
        nav: false,
        ..GraphFilter::default()
    };
    assert_eq!(path_between(&engine, nav, CONFIGURE, &no_nav, true), None);
    // Without navigation, Install still reaches Configure, by its xref alone.
    assert_eq!(
        path_between(&engine, INSTALL, CONFIGURE, &no_nav, true).unwrap()[1].1,
        [EdgeKind::Xref]
    );
    // A local scope searches only what it shows.
    let root = fixture();
    let local = GraphScope::Local {
        path: root.join(INSTALL),
        depth: 1,
    };
    assert!(engine
        .graph_path(
            &local,
            &shown,
            &file(&root.join(HOME)),
            &file(&root.join(note)),
            true
        )
        .is_some());
    assert!(engine
        .graph_path(
            &local,
            &shown,
            &file(&root.join(HOME)),
            &file(&root.join("reference/modules/ROOT/pages/api.adoc")),
            true
        )
        .is_none());
}

#[test]
fn renaming_a_page_moves_its_edges() {
    let mut copy = Copy::new();
    let old = copy.page("configure.adoc");
    let new = copy.page("setup/configure-it.adoc");
    let id = copy.engine.open(&old).unwrap();
    let CommandOutcome::Preview(preview) = copy
        .engine
        .invoke(
            ids::PAGE_RENAME,
            CommandArgs::Rename {
                id,
                new_path: new.clone(),
            },
        )
        .unwrap()
    else {
        panic!("a rename previews its edits");
    };
    copy.engine
        .invoke(
            ids::WORKSPACE_APPLY_PREVIEW,
            CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
        )
        .unwrap();
    assert!(copy.until_graph_changed());
    // The edits to the pages naming it are indexed once typing would have settled.
    copy.clock.advance(INDEX_SETTLE);
    copy.engine.pump();
    copy.engine.pump();
    let snapshot = copy.engine.graph(&GraphScope::Workspace, &everything());
    let root = copy.root.clone();
    assert!(snapshot.node(&file(&old)).is_none());
    let pages = "guide/modules/ROOT/pages";
    let renamed = format!("{pages}/setup/configure-it.adoc");
    for (from, to, kind) in [
        (
            format!("{pages}/install.adoc"),
            renamed.clone(),
            EdgeKind::Xref,
        ),
        (
            format!("{pages}/install.adoc"),
            renamed.clone(),
            EdgeKind::Nav,
        ),
        (
            renamed.clone(),
            format!("{pages}/index.adoc"),
            EdgeKind::Xref,
        ),
    ] {
        assert!(
            has_edge(&snapshot, &root, &from, &to, kind),
            "{from} -> {to} {kind:?}"
        );
    }
    assert!(edges(&snapshot, &root).iter().all(
        |(from, to, _)| !from.ends_with("/configure.adoc") && !to.ends_with("/configure.adoc")
    ));
    assert_eq!(copy.engine.graph_builds(), 1, "no full build");
}

#[test]
fn catalog_change_rebuilds_the_graph() {
    let mut copy = Copy::new();
    assert_eq!(copy.engine.graph_builds(), 1);
    fs::write(
        copy.root.join("reference/antora.yml"),
        "name: reference\ntitle: Reference\nversion: '2.0'\n",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while copy.engine.graph_builds() < 2 && Instant::now() < deadline {
        copy.engine.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        copy.engine.graph_builds(),
        2,
        "a catalog change builds the graph again"
    );
    let snapshot = copy
        .engine
        .graph(&GraphScope::Workspace, &GraphFilter::default());
    let api = snapshot
        .node(&file(
            &copy.root.join("reference/modules/ROOT/pages/api.adoc"),
        ))
        .unwrap();
    assert_eq!(
        api.coordinate.as_ref().unwrap().version.as_deref(),
        Some("2.0")
    );
}

#[test]
fn local_graph_visits_only_reachable_files() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let configure = root.join("guide/modules/ROOT/pages/configure.adoc");
    let local = |depth| {
        probe::reset();
        let snapshot = engine.graph(
            &GraphScope::Local {
                path: configure.clone(),
                depth,
            },
            &GraphFilter::default(),
        );
        (snapshot, probe::count(Probe::GraphNode))
    };

    let (one, visited) = local(1);
    let pages = "guide/modules/ROOT/pages";
    assert_eq!(
        names(&one, &root),
        [
            format!("{pages}/configure.adoc"),
            format!("{pages}/index.adoc"),
            format!("{pages}/install.adoc"),
        ]
    );
    assert_eq!(visited, 3);

    let (two, visited) = local(2);
    let found: BTreeSet<String> = names(&two, &root).into_iter().collect();
    let expected: BTreeSet<String> = [
        format!("{pages}/configure.adoc"),
        format!("{pages}/index.adoc"),
        format!("{pages}/install.adoc"),
        "guide/modules/ROOT/nav.adoc".to_owned(),
        "guide/modules/ROOT/partials/note.adoc".to_owned(),
        "guide/modules/admin/pages/users.adoc".to_owned(),
        "reference/modules/ROOT/pages/api.adoc".to_owned(),
    ]
    .into();
    assert_eq!(found, expected);
    assert_eq!(visited, expected.len() as u64, "no other file was visited");

    // Neither an image nor a file the index does not hold has a local graph.
    let empty = engine.graph(
        &GraphScope::Local {
            path: root.join("nowhere.adoc"),
            depth: 2,
        },
        &GraphFilter::default(),
    );
    assert!(empty.nodes.is_empty());
}

#[test]
fn graph_filters_apply() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let snapshot = |filter: GraphFilter| engine.graph(&GraphScope::Workspace, &filter);
    let defaults = snapshot(GraphFilter::default());
    assert!(defaults
        .nodes
        .iter()
        .all(|node| node.kind != GraphNodeKind::Unresolved));
    assert!(names(&defaults, &root).contains(&"guide/modules/ROOT/pages/orphan.adoc".to_owned()));

    let no_partials = snapshot(GraphFilter {
        partials: false,
        ..GraphFilter::default()
    });
    assert!(no_partials
        .nodes
        .iter()
        .all(|node| node.kind != GraphNodeKind::Partial));
    assert!(no_partials
        .edges
        .iter()
        .all(|edge| edge.kind != EdgeKind::Include));
    assert!(!names(&no_partials, &root).contains(&"loose/_part.adoc".to_owned()));

    let no_nav = snapshot(GraphFilter {
        nav: false,
        ..GraphFilter::default()
    });
    assert!(no_nav
        .nodes
        .iter()
        .all(|node| node.kind != GraphNodeKind::Nav));
    assert!(no_nav.edges.iter().all(|edge| edge.kind != EdgeKind::Nav));

    let no_orphans = snapshot(GraphFilter {
        orphans: false,
        ..GraphFilter::default()
    });
    assert!(!names(&no_orphans, &root).contains(&"guide/modules/ROOT/pages/orphan.adoc".to_owned()));

    let reference = snapshot(GraphFilter {
        components: Some(["reference".to_owned()].into()),
        ..GraphFilter::default()
    });
    assert_eq!(
        names(&reference, &root),
        ["reference/modules/ROOT/pages/api.adoc"]
    );
}

#[test]
fn nav_places_trail_a_page_through_its_nav() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let pages = root.join("guide/modules/ROOT/pages");
    let nav = root.join("guide/modules/ROOT/nav.adoc");
    let crumb = |title: &str, page: Option<&str>| NavCrumb {
        title: title.to_owned(),
        path: page.map(|page| pages.join(page)),
    };

    let configure = engine.nav_places(&pages.join("configure.adoc"));
    assert_eq!(configure.len(), 1);
    assert_eq!(configure[0].nav, nav);
    assert_eq!(
        configure[0].ancestors,
        [
            crumb("Introduction", None),
            crumb("Install", Some("install.adoc"))
        ]
    );
    assert_eq!(
        configure[0].page,
        crumb("Configure", Some("configure.adoc"))
    );
    assert!(configure[0].children.is_empty());

    let install = engine.nav_places(&pages.join("install.adoc"));
    assert_eq!(install.len(), 2, "listed twice");
    assert_eq!(
        install[0].children,
        [crumb("Configure", Some("configure.adoc"))]
    );
    assert!(install[1].ancestors.is_empty());

    assert!(engine.nav_places(&pages.join("orphan.adoc")).is_empty());
}

#[test]
fn neighbours_merge_kinds_per_page() {
    let root = fixture();
    let engine = open(&root, &ManualClock::new());
    let pages = root.join("guide/modules/ROOT/pages");
    let configure = file(&pages.join("configure.adoc"));

    let (out, back) = engine.neighbours(&pages.join("install.adoc"), &GraphFilter::default());
    let to_configure = out
        .iter()
        .find(|neighbour| neighbour.key == configure)
        .unwrap();
    assert_eq!(to_configure.kinds, [EdgeKind::Xref, EdgeKind::Nav]);
    assert_eq!(to_configure.title, "Configure");
    assert_eq!(
        out.iter()
            .filter(|neighbour| neighbour.key == configure)
            .count(),
        1
    );
    assert!(back
        .iter()
        .any(|neighbour| neighbour.title == "Home" && neighbour.kinds == [EdgeKind::Xref]));

    let (out, _) = engine.neighbours(
        &pages.join("install.adoc"),
        &GraphFilter {
            nav: false,
            ..GraphFilter::default()
        },
    );
    let to_configure = out
        .iter()
        .find(|neighbour| neighbour.key == configure)
        .unwrap();
    assert_eq!(to_configure.kinds, [EdgeKind::Xref]);
}
