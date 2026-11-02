//! The link graph: every page, partial and navigation file, and the xrefs, includes and
//! navigation entries between them.
//!
//! `adoc-graph` keeps the links ([`LinkStore`]) and `adoc-lang` resolves one file's
//! ([`adoc_lang::graph::links_from`]); the engine decides when. The whole graph is built once,
//! on the index worker with the parses it made, and again on a worker of its own only when the
//! Antora catalog's components, versions, modules or navigation files change. Every other
//! change is incremental: a file whose index entry was replaced, added or removed is marked,
//! and at the next pump only it is resolved again, with the files that name it when it came or
//! went, and only a navigation file that changed has its tree read again. Then
//! [`EngineEvent::GraphChanged`] follows, at most once a pump.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use adoc_antora::{AntoraCoordinate, NavItem, NavTree, ResourceFamily};
use adoc_graph::{
    EdgeKind, GraphFilter, GraphKey, GraphSnapshot, LinkStore, NodeCoordinate, NodeInput, NodeKind,
    PathStep, StoredLink,
};
use adoc_index::{is_asciidoc_path, normalize_path};
use adoc_lang::{
    graph::{LinkKind, LinkTarget},
    WorkspaceState,
};

use crate::{
    command::FnCommand, commands::ids, workspace::WorkerMessage, CommandCategory, CommandContext,
    CommandOutcome, CommandRegistry, CommandSpec, Engine, EngineEvent, NewPage,
};

/// What a graph query covers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphScope {
    /// Every node.
    Workspace,
    /// The page at `path` and the nodes within `depth` links of it (1 to 3), both ways.
    Local { path: PathBuf, depth: u8 },
}

/// One entry on the way to a page in a navigation file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavCrumb {
    pub title: String,
    /// The page the entry links to; `None` for a text-only entry.
    pub path: Option<PathBuf>,
}

/// Where a page sits in one navigation file: the entries above it, its own, and those beneath.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavPlace {
    pub nav: PathBuf,
    pub ancestors: Vec<NavCrumb>,
    pub page: NavCrumb,
    pub children: Vec<NavCrumb>,
}

/// A node linked to a page, with every kind of link between the two.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Neighbour {
    pub key: GraphKey,
    pub title: String,
    /// In `Xref`, `Include`, `Nav` order.
    pub kinds: Vec<EdgeKind>,
}

/// What the graph knows of a file: its title, as nodes show it.
type Titles = BTreeMap<Arc<Path>, String>;

/// A whole graph, built on a worker.
#[derive(Debug, Default)]
pub(crate) struct Built {
    store: LinkStore,
    titles: Titles,
    /// Each file's [`link_inputs`] when its links were resolved.
    inputs: BTreeMap<Arc<Path>, u64>,
    /// Each navigation file's tree, in catalog order.
    nav: Vec<NavTree>,
    shape: CatalogShape,
}

/// What of the catalog a full build depends on: its components, their versions, roots and
/// navigation files, and their modules. Pages coming and going are followed file by file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CatalogShape {
    components: Vec<(String, Option<String>, PathBuf, Vec<String>)>,
    modules: Vec<(String, Option<String>, String)>,
}

impl CatalogShape {
    fn of(ws: &WorkspaceState) -> Self {
        Self {
            components: ws
                .antora
                .components()
                .map(|component| {
                    (
                        component.name.clone(),
                        component.version.clone(),
                        normalize_path(&component.root),
                        component.nav.clone(),
                    )
                })
                .collect(),
            modules: ws
                .antora
                .modules()
                .map(|module| {
                    (
                        module.component.clone(),
                        module.version.clone(),
                        module.name.clone(),
                    )
                })
                .collect(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct GraphState {
    store: LinkStore,
    titles: Titles,
    inputs: BTreeMap<Arc<Path>, u64>,
    nav: Vec<NavTree>,
    /// The paths of `nav`'s files, to tell a navigation file at once.
    nav_paths: std::collections::HashSet<PathBuf>,
    shape: CatalogShape,
    /// Whether a full build has landed.
    built: bool,
    /// A catalog change wants a full build.
    full_due: bool,
    /// The full build running, by generation; any other's result is dropped.
    building: Option<u64>,
    generation: u64,
    /// Files whose index entries changed since the graph last caught up.
    dirty: BTreeSet<PathBuf>,
    /// Counts the graph's changes; 0 until the first build lands.
    revision: u64,
    /// A change not yet announced with `GraphChanged`.
    unannounced: bool,
    /// Full builds that landed, for tests.
    builds: u64,
}

/// A whole graph from `ws`, the texts of open navigation files standing in for the disk's.
pub(crate) fn build(ws: &WorkspaceState, open: &BTreeMap<PathBuf, String>) -> Built {
    let mut built = Built {
        shape: CatalogShape::of(ws),
        ..Built::default()
    };
    let nav_files: BTreeSet<PathBuf> = nav_files(ws).into_iter().collect();
    let files: Vec<Arc<Path>> = ws
        .index
        .files()
        .filter(|file| is_asciidoc_path(&file.path))
        .map(|file| Arc::from(normalize_path(&file.path)))
        .collect();
    // Each file resolves on its own, so the files are shared among threads.
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(BUILD_THREADS);
    let chunk = files.len().div_ceil(threads).max(1);
    let resolved: Vec<Resolved> = std::thread::scope(|scope| {
        let workers: Vec<_> = files
            .chunks(chunk)
            .map(|chunk| {
                let nav_files = &nav_files;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|path| Resolved {
                            title: title_of(ws, path),
                            inputs: ws.index.file(path).map(|file| link_inputs(&file.document)),
                            links: (!nav_files.contains(&**path)).then(|| links_of(ws, path)),
                            path: Arc::clone(path),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("resolving links never panics"))
            .collect()
    });
    for file in resolved {
        built.titles.insert(Arc::clone(&file.path), file.title);
        if let Some(inputs) = file.inputs {
            built.inputs.insert(Arc::clone(&file.path), inputs);
        }
        if let Some(links) = file.links {
            built.store.set_links(file.path, links);
        }
    }
    let nav_files: Vec<PathBuf> = self::nav_files(ws);
    for nav in &nav_files {
        let text = open
            .get(nav)
            .cloned()
            .or_else(|| fs::read_to_string(nav).ok());
        if let Some(tree) = text.and_then(|text| nav_tree(ws, nav, &text)) {
            built
                .store
                .set_owned_links(Arc::from(nav.as_path()), nav_links(ws, &tree));
            built.nav.push(tree);
        }
    }
    built
}

/// At most this many threads resolve a full build's links.
const BUILD_THREADS: usize = 8;

/// One file's part of a full build.
struct Resolved {
    path: Arc<Path>,
    title: String,
    inputs: Option<u64>,
    /// `None` for a navigation file, whose links come from its tree.
    links: Option<Vec<(GraphKey, EdgeKind)>>,
}

/// A fingerprint of what a file's links are resolved from: its xref and include targets,
/// whether each include is conditional, and the attributes it declares when a target names
/// one. Typing that leaves it alone leaves the links alone.
fn link_inputs(document: &adoc_core::Document) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut attributes = false;
    for reference in &document.references {
        if reference.kind == adoc_core::ReferenceKind::Xref {
            reference.target.hash(&mut hasher);
            attributes |= reference.target.contains('{');
        }
    }
    for include in &document.includes {
        (&include.target, include.conditional).hash(&mut hasher);
        attributes |= include.target.contains('{');
    }
    if attributes {
        for attribute in &document.attributes {
            (&attribute.name, &attribute.value).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// The navigation files the catalog lists, in its order.
fn nav_files(ws: &WorkspaceState) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for component in ws.antora.components() {
        for entry in &component.nav {
            let path = normalize_path(&component.root.join(entry));
            if !files.contains(&path) {
                files.push(path);
            }
        }
    }
    files
}

fn nav_tree(ws: &WorkspaceState, path: &Path, text: &str) -> Option<NavTree> {
    let read = |_: &Path| Ok(text.to_owned());
    adoc_antora::nav_trees_of(&ws.antora, &[path.to_path_buf()], &read)
        .ok()?
        .pop()
}

/// The file's title, else its file name.
fn title_of(ws: &WorkspaceState, path: &Path) -> String {
    ws.index
        .file(path)
        .and_then(|file| file.document.title.as_ref())
        .map(|title| title.text.trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| file_name(path))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// One file's xrefs and includes as graph links. A target that resolves to a file no longer
/// there is unresolved.
fn links_of(ws: &WorkspaceState, path: &Path) -> Vec<(GraphKey, EdgeKind)> {
    adoc_lang::graph::links_from(ws, path)
        .into_iter()
        .map(|link| {
            let kind = match link.kind {
                LinkKind::Xref => EdgeKind::Xref,
                LinkKind::Include => EdgeKind::Include,
                LinkKind::Nav => EdgeKind::Nav,
            };
            let key = match link.target {
                LinkTarget::File(target) => GraphKey::File(Arc::from(target)),
                LinkTarget::Unresolved(text) => GraphKey::Unresolved(text),
            };
            (key, kind)
        })
        .collect()
}

/// The links a navigation tree makes: from the nav file to each top-level entry's page, and
/// from each entry's page to its children's. An entry with no page hands its children to its
/// nearest ancestor with one.
fn nav_links(ws: &WorkspaceState, tree: &NavTree) -> Vec<StoredLink> {
    fn walk(
        ws: &WorkspaceState,
        items: &[NavItem],
        parent: &Arc<Path>,
        links: &mut Vec<StoredLink>,
    ) {
        for item in items {
            match nav_target(ws, item) {
                Some(page) => {
                    let page: Arc<Path> = Arc::from(page);
                    if page != *parent {
                        links.push(StoredLink {
                            from: Arc::clone(parent),
                            to: GraphKey::File(Arc::clone(&page)),
                            kind: EdgeKind::Nav,
                        });
                    }
                    walk(ws, &item.children, &page, links);
                }
                None => walk(ws, &item.children, parent, links),
            }
        }
    }
    let mut links = Vec::new();
    walk(ws, &tree.items, &Arc::from(tree.path.as_path()), &mut links);
    links
}

/// The page a navigation entry links to.
fn nav_target(ws: &WorkspaceState, item: &NavItem) -> Option<PathBuf> {
    let id = item.target.as_ref()?;
    let coordinate = AntoraCoordinate {
        component: id.component.clone()?,
        version: id.version.clone(),
        module: id.module.clone()?,
        family: id.family?,
        relative_path: PathBuf::from(&id.path),
    };
    ws.antora
        .resolve(&coordinate)
        .map(|resource| normalize_path(&resource.source_path))
}

fn family_name(family: ResourceFamily) -> &'static str {
    match family {
        ResourceFamily::Page => "page",
        ResourceFamily::Partial => "partial",
        ResourceFamily::Example => "example",
        ResourceFamily::Image => "image",
        ResourceFamily::Attachment => "attachment",
    }
}

impl GraphState {
    fn adopt(&mut self, built: Built) {
        self.store = built.store;
        self.titles = built.titles;
        self.inputs = built.inputs;
        self.nav = built.nav;
        self.nav_changed();
        self.shape = built.shape;
        self.built = true;
        self.builds += 1;
        self.revision += 1;
        self.unannounced = true;
    }

    fn is_nav_file(&self, path: &Path) -> bool {
        self.nav_paths.contains(path)
    }

    /// Notes the navigation files `nav` holds.
    fn nav_changed(&mut self) {
        self.nav_paths = self.nav.iter().map(|tree| tree.path.clone()).collect();
    }

    fn changed(&mut self) {
        self.revision += 1;
        self.unannounced = true;
    }
}

impl Engine {
    /// The graph `scope` covers, as `filter` allows. Empty, with revision 0, until the first
    /// build lands; empty for a local graph of a file that is not an indexed page.
    #[must_use]
    pub fn graph(&self, scope: &GraphScope, filter: &GraphFilter) -> GraphSnapshot {
        let graph = &self.graph;
        if !graph.built {
            return GraphSnapshot::default();
        }
        match scope {
            GraphScope::Workspace => GraphSnapshot::build(
                graph.revision,
                graph
                    .titles
                    .keys()
                    .map(|path| self.node_input(&GraphKey::File(Arc::clone(path)))),
                graph.store.links(),
                filter,
                None,
            ),
            GraphScope::Local { path, depth } => {
                let path = normalize_path(path);
                let Some((centre, _)) = graph.titles.get_key_value(path.as_path()) else {
                    return GraphSnapshot {
                        revision: graph.revision,
                        ..GraphSnapshot::default()
                    };
                };
                let centre = GraphKey::File(Arc::clone(centre));
                let reached =
                    graph
                        .store
                        .neighbourhood_where(&centre, (*depth).clamp(1, 3), |link| {
                            filter.allows_edge(link.kind)
                                && self.shown(&GraphKey::File(Arc::clone(&link.from)), filter)
                                && self.shown(&link.to, filter)
                        });
                let links: Vec<&StoredLink> = reached
                    .iter()
                    .flat_map(|key| graph.store.links_from(key))
                    .filter(|link| reached.contains(&link.to))
                    .collect();
                let nodes: Vec<NodeInput> = reached
                    .iter()
                    .filter(|key| {
                        key.path()
                            .is_some_and(|path| graph.titles.contains_key(path))
                    })
                    .map(|key| self.node_input(key))
                    .collect();
                GraphSnapshot::build(graph.revision, nodes, links, filter, Some(&centre))
            }
        }
    }

    /// Counts the graph's changes; 0 until it is first built.
    #[must_use]
    pub fn graph_revision(&self) -> u64 {
        self.graph.revision
    }

    /// Whether a full build of the graph is due or running.
    #[must_use]
    pub fn is_graph_building(&self) -> bool {
        !self.graph.built || self.graph.building.is_some() || self.graph.full_due
    }

    /// How many full builds of the graph have landed.
    #[doc(hidden)]
    #[must_use]
    pub fn graph_builds(&self) -> u64 {
        self.graph.builds
    }

    /// Builds the whole graph on this thread, as a catalog change would on a worker.
    #[doc(hidden)]
    pub fn build_graph_blocking(&mut self) {
        let built = build(&self.workspace, &self.open_nav_texts());
        self.graph.adopt(built);
        self.graph.dirty.clear();
    }

    /// Where the page at `path` sits in each navigation file that lists it, in catalog order
    /// and then source order. Reads only the trees the graph holds.
    #[must_use]
    pub fn nav_places(&self, path: &Path) -> Vec<NavPlace> {
        fn walk(
            ws: &WorkspaceState,
            nav: &Path,
            items: &[NavItem],
            ancestors: &mut Vec<NavCrumb>,
            page: &Path,
            places: &mut Vec<NavPlace>,
        ) {
            for item in items {
                let entry = crumb(ws, item);
                if entry.path.as_deref() == Some(page) {
                    places.push(NavPlace {
                        nav: nav.to_path_buf(),
                        ancestors: ancestors.clone(),
                        page: entry.clone(),
                        children: item.children.iter().map(|child| crumb(ws, child)).collect(),
                    });
                }
                ancestors.push(entry);
                walk(ws, nav, &item.children, ancestors, page, places);
                ancestors.pop();
            }
        }
        fn crumb(ws: &WorkspaceState, item: &NavItem) -> NavCrumb {
            NavCrumb {
                title: item.title.clone(),
                path: nav_target(ws, item),
            }
        }
        let page = normalize_path(path);
        let mut places = Vec::new();
        for tree in &self.graph.nav {
            walk(
                &self.workspace,
                &tree.path,
                &tree.items,
                &mut Vec::new(),
                &page,
                &mut places,
            );
        }
        places
    }

    /// The nodes the page at `path` links to and those linking to it, one for each, naming
    /// every kind of link between the two that `filter` allows, sorted by title.
    #[must_use]
    pub fn neighbours(
        &self,
        path: &Path,
        filter: &GraphFilter,
    ) -> (Vec<Neighbour>, Vec<Neighbour>) {
        let key = GraphKey::File(Arc::from(normalize_path(path)));
        let store = &self.graph.store;
        let gather = |ends: Vec<(GraphKey, EdgeKind)>| {
            let mut by_key: BTreeMap<GraphKey, BTreeSet<EdgeKind>> = BTreeMap::new();
            for (end, kind) in ends {
                if end != key && filter.allows_edge(kind) && self.shown(&end, filter) {
                    by_key.entry(end).or_default().insert(kind);
                }
            }
            let mut found: Vec<Neighbour> = by_key
                .into_iter()
                .map(|(end, kinds)| Neighbour {
                    title: self.title_of_key(&end),
                    key: end,
                    kinds: kinds.into_iter().collect(),
                })
                .collect();
            found
                .sort_by(|left, right| left.title.cmp(&right.title).then(left.key.cmp(&right.key)));
            found
        };
        let out = gather(
            store
                .links_from(&key)
                .map(|link| (link.to.clone(), link.kind))
                .collect(),
        );
        let back = gather(
            store
                .links_to(&key)
                .map(|link| (GraphKey::File(Arc::clone(&link.from)), link.kind))
                .collect(),
        );
        (out, back)
    }

    /// The shortest path from `from` to `to` among the nodes `scope` shows as `filter` allows,
    /// over the links it shows: in their direction when `directed`, as a reader follows them,
    /// else either way. `None` when there is none, or either end is not shown.
    #[must_use]
    pub fn graph_path(
        &self,
        scope: &GraphScope,
        filter: &GraphFilter,
        from: &GraphKey,
        to: &GraphKey,
        directed: bool,
    ) -> Option<Vec<PathStep>> {
        let shown = self.graph(scope, filter);
        self.graph.store.shortest_path(
            from,
            to,
            directed,
            |link| filter.allows_edge(link.kind),
            |key| shown.node(key).is_some(),
        )
    }

    /// The page to make for the unresolved target `target`, where the xrefs and navigation
    /// entries naming it expect it: in their module's `pages` folder, titled from its file
    /// name and left out of the navigation. `None` when no such link names a page in a module
    /// the workspace has, or two links expect different files.
    #[must_use]
    pub fn new_page_for(&self, target: &str) -> Option<NewPage> {
        let key = GraphKey::Unresolved(target.to_owned());
        let mut expected = self
            .graph
            .store
            .links_to(&key)
            .filter(|link| link.kind != EdgeKind::Include)
            .map(|link| adoc_lang::graph::missing_page(&self.workspace, &link.from, target));
        let first = expected.next()??;
        if expected.any(|other| other.as_ref() != Some(&first)) {
            return None;
        }
        let stem = first
            .file
            .file_stem()?
            .to_string_lossy()
            .replace(['-', '_'], " ");
        let mut letters = stem.trim().chars();
        let title = letters.next()?.to_uppercase().chain(letters).collect();
        Some(NewPage {
            module: first.module,
            title,
            add_to_nav: false,
            file: Some(first.file),
        })
    }

    /// The file to make for the unresolved include target `target`, where the includes naming
    /// it expect it: a partial (or other family) in its module, or a file beside the including
    /// page. `None` when no include names a file inside the workspace's roots, or two expect
    /// different files.
    #[must_use]
    pub fn new_partial_for(&self, target: &str) -> Option<PathBuf> {
        let key = GraphKey::Unresolved(target.to_owned());
        let mut expected = self
            .graph
            .store
            .links_to(&key)
            .filter(|link| link.kind == EdgeKind::Include)
            .map(|link| adoc_lang::graph::missing_include(&self.workspace, &link.from, target));
        let first = expected.next()??;
        if expected.any(|other| other.as_ref() != Some(&first)) {
            return None;
        }
        self.roots()
            .iter()
            .any(|root| first.starts_with(root))
            .then_some(first)
    }

    /// Whether `filter` shows the node `key`.
    fn shown(&self, key: &GraphKey, filter: &GraphFilter) -> bool {
        filter.allows_kind(self.node_kind(key))
    }

    fn title_of_key(&self, key: &GraphKey) -> String {
        match key {
            GraphKey::File(path) => self
                .graph
                .titles
                .get(&**path)
                .cloned()
                .unwrap_or_else(|| file_name(path)),
            GraphKey::Unresolved(text) => text.clone(),
        }
    }

    fn node_kind(&self, key: &GraphKey) -> NodeKind {
        let GraphKey::File(path) = key else {
            return NodeKind::Unresolved;
        };
        self.file_kind(key, path, self.workspace.antora.resource_for_path(path))
    }

    /// The kind of the file `key` names, `resource` being its place in the catalog.
    fn file_kind(
        &self,
        key: &GraphKey,
        path: &Path,
        resource: Option<&adoc_antora::AntoraResource>,
    ) -> NodeKind {
        if self.graph.is_nav_file(path) {
            return NodeKind::Nav;
        }
        match resource {
            Some(resource) if resource.coordinate.family == ResourceFamily::Partial => {
                NodeKind::Partial
            }
            Some(_) => NodeKind::Page,
            // Outside Antora, a file that is included is a partial.
            None if self
                .graph
                .store
                .links_to(key)
                .any(|link| link.kind == EdgeKind::Include) =>
            {
                NodeKind::Partial
            }
            None => NodeKind::Page,
        }
    }

    fn node_input(&self, key: &GraphKey) -> NodeInput {
        adoc_core::probe::hit(adoc_core::probe::Probe::GraphNode);
        let (kind, coordinate) = match key {
            GraphKey::File(path) => {
                let antora = &self.workspace.antora;
                let resource = antora.resource_for_path(path);
                let kind = self.file_kind(key, path, resource);
                let coordinate = match resource {
                    Some(resource) if kind != NodeKind::Nav => {
                        let place = &resource.coordinate;
                        Some(NodeCoordinate {
                            component: place.component.clone(),
                            version: place.version.clone(),
                            module: place.module.clone(),
                            family: family_name(place.family).to_owned(),
                        })
                    }
                    _ => antora.context_for_path(path).map(|context| NodeCoordinate {
                        family: if kind == NodeKind::Nav {
                            "nav"
                        } else {
                            family_name(context.family)
                        }
                        .to_owned(),
                        component: context.component,
                        version: context.version,
                        module: context.module,
                    }),
                };
                (kind, coordinate)
            }
            GraphKey::Unresolved(_) => (NodeKind::Unresolved, None),
        };
        NodeInput {
            key: key.clone(),
            title: self.title_of_key(key),
            kind,
            coordinate,
            orphan: !self.graph.store.is_linked(key),
        }
    }

    /// The texts of open navigation files, newer than the disk's.
    fn open_nav_texts(&self) -> BTreeMap<PathBuf, String> {
        self.documents
            .values()
            .filter(|document| self.workspace.antora.is_nav_file(&document.path))
            .map(|document| (normalize_path(&document.path), document.text.to_string()))
            .collect()
    }

    // --- keeping the graph current -----------------------------------------------------------

    /// The index entry of `path` was replaced, added or removed: its links are resolved again
    /// at the next pump.
    pub(crate) fn graph_file_changed(&mut self, path: &Path) {
        self.graph.dirty.insert(normalize_path(path));
    }

    /// The whole index was replaced by a new one, whose graph `built` came with it.
    pub(crate) fn graph_indexed(&mut self, built: Built) {
        // A catalog build in flight read the index replaced.
        self.graph.building = None;
        self.graph.full_due = false;
        self.graph.dirty.clear();
        self.graph.adopt(built);
    }

    /// A component descriptor was read again. Only a change to the catalog's components,
    /// versions, modules or navigation files builds the whole graph again; pages that came or
    /// went with it are followed file by file.
    pub(crate) fn graph_catalog_changed(&mut self) {
        if CatalogShape::of(&self.workspace) != self.graph.shape {
            self.graph.full_due = true;
        }
    }

    pub(crate) fn has_due_graph(&self) -> bool {
        self.graph.built
            && (self.graph.full_due || !self.graph.dirty.is_empty() || self.graph.unannounced)
    }

    /// Starts a full build that is due, or resolves the files that changed; then announces a
    /// change. Changes made while a full build runs wait for it.
    pub(crate) fn update_due_graph(&mut self) -> usize {
        if !self.graph.built || self.graph.building.is_some() {
            return 0;
        }
        let mut applied = 0;
        if self.graph.full_due {
            self.graph.full_due = false;
            self.graph.dirty.clear();
            self.graph.generation += 1;
            let generation = self.graph.generation;
            self.graph.building = Some(generation);
            let state = self.workspace.clone();
            let open = self.open_nav_texts();
            let sender = self.worker_sender.clone();
            self.workers_running += 1;
            let crashed = move |error| WorkerMessage::Crashed {
                job: crate::worker::Crashed::Graph { generation },
                error,
            };
            crate::worker::spawn(sender, crashed, move |sender| {
                let built = build(&state, &open);
                let _ = sender.send(WorkerMessage::GraphBuilt {
                    generation,
                    built: Box::new(built),
                });
            });
            return 1;
        }
        if !self.graph.dirty.is_empty() {
            let dirty = std::mem::take(&mut self.graph.dirty);
            if self.catch_up(&dirty) {
                self.graph.changed();
            }
            applied += 1;
        }
        if self.graph.unannounced {
            self.graph.unannounced = false;
            self.emit(EngineEvent::GraphChanged {
                revision: self.graph.revision,
            });
            applied += 1;
        }
        applied
    }

    /// Resolves again the files in `dirty`, and those naming a file that came or went. Returns
    /// whether the graph changed.
    fn catch_up(&mut self, dirty: &BTreeSet<PathBuf>) -> bool {
        let mut changed = false;
        // Files to resolve again; those that only changed themselves are skipped when what
        // their links come from is as it was.
        let mut resolve: BTreeSet<PathBuf> = BTreeSet::new();
        let mut own: BTreeSet<PathBuf> = BTreeSet::new();
        for path in dirty {
            let indexed = is_asciidoc_path(path) && self.workspace.index.file(path).is_some();
            let known = self.graph.titles.contains_key(path.as_path());
            if indexed {
                let title = title_of(&self.workspace, path);
                if self.graph.titles.get(path.as_path()) != Some(&title) {
                    self.graph.titles.insert(Arc::from(path.as_path()), title);
                    changed = true;
                }
                if known {
                    own.insert(path.clone());
                } else {
                    resolve.insert(path.clone());
                    resolve.extend(self.naming(path));
                }
            } else if known {
                self.graph.titles.remove(path.as_path());
                self.graph.inputs.remove(path.as_path());
                self.graph.store.remove(path);
                let key = GraphKey::File(Arc::from(path.as_path()));
                resolve.extend(
                    self.graph
                        .store
                        .linking_to(&key)
                        .map(|owner| owner.to_path_buf()),
                );
                changed = true;
            }
        }
        for path in own {
            let inputs = self
                .workspace
                .index
                .file(&path)
                .map(|file| link_inputs(&file.document));
            let unchanged = inputs.is_some()
                && self.graph.inputs.get(path.as_path()) == inputs.as_ref()
                && !self.graph.is_nav_file(&path);
            if !unchanged {
                resolve.insert(path);
            }
        }
        for path in resolve {
            changed |= self.resolve_file(&path);
        }
        changed
    }

    /// The files whose links may name the file at `path`, by its file name or one of its
    /// `page-aliases`.
    fn naming(&self, path: &Path) -> BTreeSet<PathBuf> {
        let mut names = BTreeSet::new();
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            names.insert(name.to_owned());
        }
        if let Some(resource) = self.workspace.antora.resource_for_path(path) {
            names.extend(resource.aliases.iter().filter_map(|alias| {
                Path::new(&alias.path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            }));
        }
        let index = &self.workspace.index;
        names
            .iter()
            .flat_map(|name| {
                index
                    .xrefs_naming(name)
                    .map(|location| location.path.to_path_buf())
                    .chain(
                        index
                            .includes_naming(name)
                            .map(|location| location.path.to_path_buf()),
                    )
            })
            .collect()
    }

    /// Resolves one file's links again: a navigation file's tree, or a page's xrefs and
    /// includes. Returns whether they changed.
    fn resolve_file(&mut self, path: &Path) -> bool {
        let owner: Arc<Path> = Arc::from(path);
        if self.workspace.antora.is_nav_file(path) || self.graph.is_nav_file(path) {
            let text = match self.document_for_path(path) {
                Some(id) => self
                    .documents
                    .get(&id)
                    .map(|document| document.text.to_string()),
                None => self
                    .workspace
                    .index
                    .file(path)
                    .and_then(adoc_index::FileEntry::text)
                    .map(std::borrow::Cow::into_owned),
            };
            let tree = text.and_then(|text| nav_tree(&self.workspace, path, &text));
            let old = self.graph.nav.iter().position(|known| known.path == path);
            let Some(tree) = tree else {
                if let Some(at) = old {
                    self.graph.nav.remove(at);
                    self.graph.nav_changed();
                }
                return self.graph.store.remove(path) || old.is_some();
            };
            let links = nav_links(&self.workspace, &tree);
            let tree_changed = match old {
                Some(at) if self.graph.nav[at] == tree => false,
                Some(at) => {
                    self.graph.nav[at] = tree;
                    true
                }
                None => {
                    self.graph.nav.push(tree);
                    self.graph.nav_changed();
                    true
                }
            };
            return self.graph.store.set_owned_links(owner, links) || tree_changed;
        }
        let Some(file) = self.workspace.index.file(path) else {
            self.graph.inputs.remove(path);
            return self.graph.store.remove(path);
        };
        self.graph
            .inputs
            .insert(Arc::clone(&owner), link_inputs(&file.document));
        let links = links_of(&self.workspace, path);
        self.graph.store.set_links(owner, links)
    }

    pub(crate) fn graph_built(&mut self, generation: u64, built: Built) {
        if self.graph.building != Some(generation) {
            return;
        }
        self.graph.building = None;
        self.graph.adopt(built);
    }

    pub(crate) fn graph_crashed(&mut self, generation: u64) {
        if self.graph.building == Some(generation) {
            self.graph.building = None;
        }
    }
}

pub(crate) fn register_graph(registry: &mut CommandRegistry) {
    use CommandCategory::View;
    use CommandContext::Always;
    // The front end shows the graph window or panel, and lays the graph out; the engine
    // keeps the graph they draw.
    for (id, title, binding) in [
        (ids::VIEW_GRAPH, "Show Graph", Some("cmd-k g")),
        (
            ids::VIEW_LOCAL_GRAPH,
            "Show Local Graph",
            Some("cmd-k shift-g"),
        ),
        (ids::GRAPH_RESET_LAYOUT, "Reset Graph Layout", None),
        (ids::GRAPH_EXPORT, "Export Graph…", None),
    ] {
        registry
            .register(Box::new(FnCommand {
                spec: CommandSpec {
                    id,
                    title,
                    category: View,
                    default_binding: binding,
                    when: Always,
                },
                run: |_, _| Ok(CommandOutcome::Done),
            }))
            .expect("built-in command ids are unique");
    }
}
