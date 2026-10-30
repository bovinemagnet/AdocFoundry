//! Sidebar trees built from engine state: the Antora navigation, the module and family browser,
//! the workspace diagnostics, the files changed since the last commit, and the outline of the
//! active document.
//!
//! Every leaf's id encodes where clicking it leads, so one handler serves all of them.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use adoc_core::{LineIndex, SourceRange};
use adoc_engine::{
    Branch, DocumentId, DocumentSymbol, Engine, GitChange, NavItem, SidebarPanel, WorkspaceState,
};
use gpui_component::tree::TreeItem;

/// Where a tree item leads when clicked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub range: SourceRange,
}

/// Encodes a target as a tree item id, unique within one tree.
pub fn target_id(path: &Path, range: SourceRange, salt: usize) -> String {
    format!("{}|{}|{}|{}", range.start, range.end, salt, path.display())
}

/// Ends the branch key at the start of a Navigation or Modules item's id; see [`Node::id`].
const KEY_END: char = '\u{1f}';
/// Joins the steps of a branch key, which titles and paths cannot contain.
const KEY_STEP: char = '\u{1e}';

/// Decodes the target of an id made by [`target_id`], or of a Navigation or Modules item that
/// leads somewhere; folder ids do not decode.
pub fn parse_target(id: &str) -> Option<Target> {
    let id = id.split_once(KEY_END).map_or(id, |(_, target)| target);
    let mut parts = id.splitn(4, '|');
    let start = parts.next()?.parse().ok()?;
    let end = parts.next()?.parse().ok()?;
    let _salt = parts.next()?;
    let path = PathBuf::from(parts.next()?);
    Some(Target {
        path,
        range: SourceRange::new(start, end),
    })
}

/// The branch key of a Navigation or Modules item's id.
pub fn branch_key(id: &str) -> &str {
    id.split(KEY_END).next().unwrap_or(id)
}

fn folder(id: impl Into<String>, label: impl Into<String>, children: Vec<TreeItem>) -> TreeItem {
    TreeItem::new(format!("folder:{}", id.into()), label.into())
        .children(children)
        .expanded(true)
}

/// A row of the Navigation or Modules tree, before it is shown. Its key names the same logical
/// row however the tree is rebuilt: the component version, then module, family or navigation
/// entry by entry, never a row number or a position among unrelated rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub key: String,
    pub label: String,
    pub target: Option<Target>,
    /// Whether the row is a navigation entry linking to a fragment of its page.
    pub fragment: bool,
    pub children: Vec<Node>,
}

impl Node {
    fn new(key: String, label: impl Into<String>, target: Option<Target>) -> Self {
        Self {
            key,
            label: label.into(),
            target,
            fragment: false,
            children: Vec::new(),
        }
    }

    /// The tree item's id: the key, then the target, so the click handler and the expansion
    /// state each read their part.
    pub fn id(&self) -> String {
        match &self.target {
            Some(target) => format!(
                "{}{KEY_END}{}",
                self.key,
                target_id(&target.path, target.range, 0)
            ),
            None => self.key.clone(),
        }
    }
}

fn step(parent: &str, name: &str) -> String {
    format!("{parent}{KEY_STEP}{name}")
}

/// The branches of `nodes`, by key, for the engine's expansion state.
pub fn branches(nodes: &[Node]) -> Vec<Branch> {
    nodes
        .iter()
        .filter(|node| !node.children.is_empty())
        .map(|node| Branch {
            key: node.key.clone(),
            children: branches(&node.children),
        })
        .collect()
}

/// `nodes` as tree items, each branch open or closed as the engine has it for `panel`.
pub fn tree_items(nodes: &[Node], engine: &Engine, panel: SidebarPanel) -> Vec<TreeItem> {
    nodes
        .iter()
        .map(|node| {
            let item = TreeItem::new(node.id(), node.label.clone());
            if node.children.is_empty() {
                item
            } else {
                item.children(tree_items(&node.children, engine, panel))
                    .expanded(engine.branch_expanded(panel, &node.key))
            }
        })
        .collect()
}

/// The ids of the rows from the root down to the row with id `id`, if `nodes` has one.
pub fn path_to(nodes: &[Node], id: &str) -> Option<Vec<String>> {
    for node in nodes {
        let own = node.id();
        if own == id {
            return Some(vec![own]);
        }
        if let Some(mut path) = path_to(&node.children, id) {
            path.insert(0, own);
            return Some(path);
        }
    }
    None
}

/// The ids of the rows from the root down to the first row, depth first, that opens `file`: a
/// resource's row, or a navigation entry linking to it, never an entry opening a nav file's line.
/// An entry linking to a fragment of the page is found only when no entry links to the page itself.
pub fn path_to_file(nodes: &[Node], file: &Path) -> Option<Vec<String>> {
    path_to_entry(nodes, file, false).or_else(|| path_to_entry(nodes, file, true))
}

fn path_to_entry(nodes: &[Node], file: &Path, fragments: bool) -> Option<Vec<String>> {
    for node in nodes {
        if (fragments || !node.fragment)
            && node
                .target
                .as_ref()
                .is_some_and(|target| opens(target, file))
        {
            return Some(vec![node.id()]);
        }
        if let Some(mut path) = path_to_entry(&node.children, file, fragments) {
            path.insert(0, node.id());
            return Some(path);
        }
    }
    None
}

/// Whether the row with id `id` opens `file`, as [`path_to_file`] finds rows.
pub fn opens_file(id: &str, file: &Path) -> bool {
    parse_target(id).is_some_and(|target| opens(&target, file))
}

/// Resources and linked entries open their file at its start; other entries open a line.
fn opens(target: &Target, file: &Path) -> bool {
    target.path == file && target.range == SourceRange::new(0, 0)
}

fn component_label(name: &str, title: Option<&str>, version: Option<&str>) -> String {
    let title = title.unwrap_or(name);
    match version {
        Some(version) => format!("{title} {version}"),
        None => title.to_owned(),
    }
}

/// A component version's key in `panel`.
fn component_key(panel: &str, name: &str, version: Option<&str>) -> String {
    format!("{panel}:{name}@{}", version.unwrap_or("~"))
}

/// The page a navigation entry links to, as a file to open.
fn nav_target(workspace: &WorkspaceState, item: &NavItem) -> Option<PathBuf> {
    let id = item.target.as_ref()?;
    let coordinate = adoc_antora_coordinate(id)?;
    workspace
        .antora
        .resolve(&coordinate)
        .map(|resource| resource.source_path.clone())
}

fn adoc_antora_coordinate(
    id: &adoc_engine::AntoraResourceId,
) -> Option<adoc_engine::AntoraCoordinate> {
    Some(adoc_engine::AntoraCoordinate {
        component: id.component.clone()?,
        version: id.version.clone(),
        module: id.module.clone()?,
        family: id.family?,
        relative_path: PathBuf::from(&id.path),
    })
}

/// The entries of a navigation list under the row keyed `parent`. An entry is keyed by its
/// title and what it links to; entries alike among their siblings are told apart by count.
fn nav_items(workspace: &WorkspaceState, items: &[NavItem], parent: &str) -> Vec<Node> {
    let mut alike: HashMap<String, usize> = HashMap::new();
    items
        .iter()
        .map(|item| {
            let link = item
                .target
                .as_ref()
                .map(adoc_engine::resource_id_string)
                .unwrap_or_default();
            let name = format!("{}\u{1d}{link}", item.title);
            let seen = alike.entry(name.clone()).or_default();
            let key = match *seen {
                0 => step(parent, &name),
                n => step(parent, &format!("{name}#{n}")),
            };
            *seen += 1;
            // A linked entry opens its page; anything else opens its line in the nav file.
            let (path, range) = match nav_target(workspace, item) {
                Some(path) => (path, SourceRange::new(0, 0)),
                None => (item.source.path.clone(), item.source.range),
            };
            let mut node = Node::new(key, item.title.clone(), Some(Target { path, range }));
            node.fragment = item.fragment.is_some();
            node.children = nav_items(workspace, &item.children, &node.key);
            node
        })
        .collect()
}

/// The navigation of every component version, falling back to its pages when it declares none.
pub fn navigation(engine: &Engine) -> Vec<Node> {
    let workspace = engine.workspace();
    let mut components: Vec<_> = workspace.antora.components().collect();
    components.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| adoc_engine::compare_component_versions(right, left))
    });
    components
        .into_iter()
        .map(|component| {
            let version = component.version.as_deref();
            let label = component_label(&component.name, component.title.as_deref(), version);
            let mut node = Node::new(component_key("nav", &component.name, version), label, None);
            let trees = engine
                .nav_trees(&component.name, version)
                .unwrap_or_default();
            node.children = if trees.is_empty() {
                pages(workspace, &component.name, version, &node.key)
            } else {
                // Each nav file's entries are keyed under it.
                trees
                    .iter()
                    .flat_map(|tree| {
                        let file = step(&node.key, &tree.path.display().to_string());
                        nav_items(workspace, &tree.items, &file)
                    })
                    .collect()
            };
            node
        })
        .collect()
}

fn pages(
    workspace: &WorkspaceState,
    component: &str,
    version: Option<&str>,
    parent: &str,
) -> Vec<Node> {
    workspace
        .antora
        .modules_of(component, version)
        .map(|module| {
            let mut node = Node::new(
                step(parent, &format!("module:{}", module.name)),
                module.name.clone(),
                None,
            );
            node.children = workspace
                .antora
                .resources_in(
                    component,
                    version,
                    &module.name,
                    adoc_engine::ResourceFamily::Page,
                )
                .map(|resource| resource_node(&node.key, resource))
                .collect();
            node
        })
        .collect()
}

fn resource_node(parent: &str, resource: &adoc_engine::AntoraResource) -> Node {
    let relative = resource.coordinate.relative_path.display().to_string();
    Node::new(
        step(parent, &relative),
        relative,
        Some(Target {
            path: resource.source_path.clone(),
            range: SourceRange::new(0, 0),
        }),
    )
}

/// Every module of every component version, with its families and their resources.
pub fn families(engine: &Engine) -> Vec<Node> {
    let workspace = engine.workspace();
    let mut components: Vec<_> = workspace.antora.components().collect();
    components.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| adoc_engine::compare_component_versions(right, left))
    });
    components
        .into_iter()
        .map(|component| {
            let version = component.version.as_deref();
            let mut node = Node::new(
                component_key("mod", &component.name, version),
                component_label(&component.name, component.title.as_deref(), version),
                None,
            );
            node.children = workspace
                .antora
                .modules_of(&component.name, version)
                .map(|module| {
                    let mut module_node =
                        Node::new(step(&node.key, &module.name), module.name.clone(), None);
                    module_node.children = adoc_engine::ResourceFamily::ALL
                        .into_iter()
                        .filter_map(|family| {
                            let key = step(&module_node.key, &family.to_string());
                            let resources: Vec<Node> = workspace
                                .antora
                                .resources_in(&component.name, version, &module.name, family)
                                .map(|resource| resource_node(&key, resource))
                                .collect();
                            (!resources.is_empty()).then(|| {
                                let mut family_node =
                                    Node::new(key, format!("{family} ({})", resources.len()), None);
                                family_node.children = resources;
                                family_node
                            })
                        })
                        .collect();
                    module_node
                })
                .collect();
            node
        })
        .collect()
}

/// Workspace diagnostics grouped by file, each opening its location.
pub fn problems(engine: &Engine, roots: &[PathBuf]) -> (Vec<TreeItem>, usize) {
    let mut count = 0;
    let mut salt = 0;
    let items = engine
        .workspace_diagnostics()
        .into_iter()
        .map(|(path, diagnostics)| {
            let text = engine
                .document_for_path(&path)
                .and_then(|id| engine.document(id))
                .map(|document| document.text.to_string())
                .or_else(|| std::fs::read_to_string(&path).ok())
                .unwrap_or_default();
            let lines = LineIndex::new(&text);
            let children = diagnostics
                .iter()
                .map(|diagnostic| {
                    count += 1;
                    salt += 1;
                    let (line, column) = lines.line_column(diagnostic.range.start);
                    TreeItem::new(
                        target_id(&path, diagnostic.range, salt),
                        format!("{}:{} {}", line + 1, column + 1, diagnostic.message),
                    )
                })
                .collect();
            folder(
                path.display().to_string(),
                display_path(&path, roots),
                children,
            )
        })
        .collect();
    (items, count)
}

/// One row of the Changes panel: a changed file a writer edits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeRow {
    pub path: PathBuf,
    /// `git status --short` letters, the path from its root and, when open, the hunk count.
    pub label: String,
    /// Whether the file has changes staged for the next commit.
    pub staged: bool,
    /// A deleted file, which there is nothing to open of.
    pub deleted: bool,
}

/// The Changes panel's rows, grouped by repository (named by its folder) when there are
/// several.
pub fn change_rows(engine: &Engine) -> Vec<(Option<String>, Vec<ChangeRow>)> {
    let git = engine.git();
    let roots = engine.roots();
    let changed = engine.changed_writer_files();
    let row = |(path, status): &(PathBuf, adoc_engine::FileStatus)| {
        let hunks = engine
            .document_for_path(path)
            .map_or(0, |id| git.hunks_for(id).len());
        let mut label = format!("{} {}", short_status(status), display_path(path, roots));
        if hunks > 0 {
            label.push_str(&format!(" ({hunks})"));
        }
        ChangeRow {
            path: path.clone(),
            label,
            staged: status.index != GitChange::Unmodified,
            deleted: matches!(status.worktree, GitChange::Deleted)
                || matches!(status.index, GitChange::Deleted),
        }
    };
    let repositories = git.repositories();
    if repositories.len() <= 1 {
        return vec![(None, changed.iter().map(row).collect())];
    }
    repositories
        .iter()
        .map(|repository| {
            let name = repository.workdir.file_name().map_or_else(
                || repository.workdir.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let rows = changed
                .iter()
                .filter(|(path, _)| path.starts_with(&repository.workdir))
                .map(row)
                .collect();
            (Some(name), rows)
        })
        .collect()
}

/// The two letters `git status --short` shows for a file.
fn short_status(status: &adoc_engine::FileStatus) -> String {
    let letter = |change: &GitChange| match change {
        GitChange::Unmodified => ' ',
        GitChange::Added => 'A',
        GitChange::Modified => 'M',
        GitChange::Deleted => 'D',
        GitChange::Renamed(_) => 'R',
        GitChange::Untracked => '?',
        GitChange::Ignored => '!',
        GitChange::Conflicted => 'U',
    };
    match status.worktree {
        GitChange::Untracked => "??".to_owned(),
        _ => format!("{}{}", letter(&status.index), letter(&status.worktree)),
    }
}

/// The outline of document `id`.
pub fn outline(engine: &Engine, id: DocumentId) -> Vec<TreeItem> {
    let Some(path) = engine.document(id).map(|document| document.path.clone()) else {
        return Vec::new();
    };
    let mut salt = 0;
    symbols(&engine.outline(id), &path, &mut salt)
}

fn symbols(symbols: &[DocumentSymbol], path: &Path, salt: &mut usize) -> Vec<TreeItem> {
    symbols
        .iter()
        .map(|symbol| {
            *salt += 1;
            let item = TreeItem::new(
                target_id(path, symbol.selection_range, *salt),
                symbol.name.clone(),
            );
            let children = self::symbols(&symbol.children, path, salt);
            if children.is_empty() {
                item
            } else {
                item.children(children).expanded(true)
            }
        })
        .collect()
}

/// `path` relative to the workspace root that contains it.
pub fn display_path(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_ids_round_trip() {
        let path = Path::new("/docs/a|b/page.adoc");
        let id = target_id(path, SourceRange::new(3, 9), 7);
        assert_eq!(
            parse_target(&id),
            Some(Target {
                path: path.to_path_buf(),
                range: SourceRange::new(3, 9),
            })
        );
        assert_eq!(parse_target("folder:demo@1.0"), None);
    }

    fn fixture_engine(root: &Path) -> Engine {
        let mut engine = Engine::new(adoc_engine::Settings::default());
        engine.open_workspace(vec![root.to_path_buf()]).unwrap();
        assert!(engine.wait_for_workers(std::time::Duration::from_secs(30)));
        engine
    }

    fn keys(nodes: &[Node], out: &mut Vec<String>) {
        for node in nodes {
            out.push(node.key.clone());
            keys(&node.children, out);
        }
    }

    fn copy_fixture(to: &Path) {
        fn copy(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                if entry.path().is_dir() {
                    copy(&entry.path(), &to.join(entry.file_name()));
                } else {
                    std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
                }
            }
        }
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sidebar-expansion"),
            to,
        );
    }

    #[test]
    fn branch_keys_are_unique_and_stable() {
        let directory = tempfile::tempdir().unwrap();
        copy_fixture(directory.path());
        let engine = fixture_engine(directory.path());
        for nodes in [navigation(&engine), families(&engine)] {
            let mut all = Vec::new();
            keys(&nodes, &mut all);
            let unique: std::collections::BTreeSet<_> = all.iter().collect();
            assert_eq!(unique.len(), all.len(), "{all:#?}");
            // Two versions of one component, each its own row.
            assert_eq!(nodes.len(), 2);
            assert_ne!(nodes[0].key, nodes[1].key);
        }

        // The repeated Install entry is two branches, and the tree's branches nest as the nav.
        let nav = navigation(&engine);
        let home = &nav[0].children[0];
        assert_eq!(home.label, "Home");
        let installs: Vec<&Node> = home
            .children
            .iter()
            .filter(|node| node.label == "Install")
            .collect();
        assert_eq!(installs.len(), 2);
        assert_ne!(installs[0].key, installs[1].key);
        assert_eq!(
            branches(&nav[..1])[0].children[0].children.len(),
            1,
            "one Install has a child"
        );

        // A Modules family's key names the version.
        let modules = families(&engine);
        let family = &modules[0].children[0].children[0];
        assert!(family.key.contains("guide@2.0"), "{}", family.key);

        // An entry inserted before the others leaves their keys as they were.
        let before = {
            let mut all = Vec::new();
            keys(&nav, &mut all);
            all
        };
        let file = directory.path().join("guide-2.0/modules/ROOT/nav.adoc");
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, format!("* xref:faq.adoc[New first]\n{text}")).unwrap();
        drop(engine);
        let engine = fixture_engine(directory.path());
        let mut after = Vec::new();
        keys(&navigation(&engine), &mut after);
        assert_eq!(after.len(), before.len() + 1);
        assert!(before.iter().all(|key| after.contains(key)));
    }

    #[test]
    fn an_entry_without_a_fragment_is_found_before_one_with() {
        let page = Path::new("/docs/pages/install.adoc");
        let entry = |key: &str, fragment: bool| {
            let target = Target {
                path: page.to_path_buf(),
                range: SourceRange::new(0, 0),
            };
            let mut node = Node::new(key.to_owned(), key, Some(target));
            node.fragment = fragment;
            node
        };
        let mut home = Node::new("home".to_owned(), "Home", None);
        home.children = vec![entry("home/steps", true), entry("home/install", false)];
        let nodes = vec![entry("first", true), home];
        assert_eq!(
            path_to_file(&nodes, page),
            Some(vec![nodes[1].id(), nodes[1].children[1].id()])
        );
        // Only entries with a fragment: the first of them.
        let nodes = vec![entry("steps", true), entry("again", true)];
        assert_eq!(path_to_file(&nodes, page), Some(vec![nodes[0].id()]));
    }

    #[test]
    fn the_first_row_opening_a_file_is_found() {
        let directory = tempfile::tempdir().unwrap();
        copy_fixture(directory.path());
        let root = adoc_index::normalize_path(directory.path());
        let engine = fixture_engine(&root);
        let labels = |nodes: &[Node], ids: Vec<String>| -> Vec<String> {
            let mut level = nodes;
            ids.iter()
                .map(|id| {
                    let node = level.iter().find(|node| &node.id() == id).unwrap();
                    level = &node.children;
                    node.label.clone()
                })
                .collect()
        };
        let pages = root.join("guide-2.0/modules/ROOT/pages");

        // Install is listed twice: the first entry is the one found.
        let nav = navigation(&engine);
        let install = path_to_file(&nav, &pages.join("install.adoc")).unwrap();
        assert_eq!(
            labels(&nav, install.clone()),
            ["Guide 2.0", "Home", "Install"]
        );
        assert_eq!(
            install.last(),
            Some(&nav[0].children[0].children[0].id()),
            "the first Install"
        );
        assert!(opens_file(
            install.last().unwrap(),
            &pages.join("install.adoc")
        ));
        assert!(!opens_file(
            install.last().unwrap(),
            &pages.join("faq.adoc")
        ));
        assert!(!opens_file(&nav[0].id(), &pages.join("install.adoc")));
        assert_eq!(
            labels(&nav, path_to_file(&nav, &pages.join("linux.adoc")).unwrap()),
            ["Guide 2.0", "Home", "Install", "Linux"]
        );
        // A page in no navigation file, and the navigation file itself, have no row.
        let more = root.join("guide-2.0/modules/extra/pages/more.adoc");
        assert_eq!(path_to_file(&nav, &more), None);
        let nav_file = root.join("guide-2.0/modules/ROOT/nav.adoc");
        assert_eq!(path_to_file(&nav, &nav_file), None);

        // Modules lists pages and images alike.
        let modules = families(&engine);
        assert_eq!(
            labels(&modules, path_to_file(&modules, &more).unwrap()),
            ["Guide 2.0", "extra", "page (1)", "more.adoc"]
        );
        let image = root.join("guide-1.0/modules/ROOT/images/diagram.svg");
        assert_eq!(
            labels(&modules, path_to_file(&modules, &image).unwrap()),
            ["Guide 1.0", "ROOT", "image (1)", "diagram.svg"]
        );
    }
}
