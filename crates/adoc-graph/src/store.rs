//! Each file's links, kept by the file whose text made them, with the reverse maps that find
//! who links from or to a node without visiting every file.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::Arc,
};

use crate::{EdgeKind, GraphKey};

/// One node of a path, and how the path came to it from the node before.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathStep {
    pub key: GraphKey,
    /// The kinds of link joining it to the node before, sorted; none for the first node.
    pub kinds: Vec<EdgeKind>,
    /// Whether those links run from this node to the one before, so the path follows them
    /// backwards.
    pub backwards: bool,
}

/// One link, as the file that owns it made it. A page owns the links from itself; a navigation
/// file owns the links its tree makes, from itself and from the pages its entries name.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StoredLink {
    pub from: Arc<Path>,
    pub to: GraphKey,
    pub kind: EdgeKind,
}

/// The links of every file, keyed by the file that owns them, as the workspace index keys its
/// tables: replacing or removing a file visits only what it contributed.
///
/// The reverse maps are only ever looked up, never walked in order, so they are hashed: a
/// path compares component by component, too slowly for a full build's many insertions. The
/// owners in each are few, kept in a small list; the links reaching a target are found by their
/// places in their owners' lists, as a navigation file owns a link to each of its many pages.
#[derive(Clone, Debug, Default)]
pub struct LinkStore {
    /// Each owner's links, sorted.
    out: BTreeMap<Arc<Path>, Vec<StoredLink>>,
    /// The owners of the links leaving each file.
    outgoing: HashMap<Arc<Path>, Vec<Arc<Path>>>,
    /// The owners of the links reaching each target, by path, each with the places of those
    /// links in its list.
    incoming: HashMap<GraphKey, Vec<OwnedPlaces>>,
    revision: u64,
}

impl LinkStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the links from `file` with `links`. Returns whether anything changed.
    pub fn set_links(&mut self, file: Arc<Path>, links: Vec<(GraphKey, EdgeKind)>) -> bool {
        let links = links
            .into_iter()
            .map(|(to, kind)| StoredLink {
                from: Arc::clone(&file),
                to,
                kind,
            })
            .collect();
        self.set_owned_links(file, links)
    }

    /// Replaces every link `owner` made with `links`, which may leave other files: a navigation
    /// file's tree. Returns whether anything changed.
    pub fn set_owned_links(&mut self, owner: Arc<Path>, mut links: Vec<StoredLink>) -> bool {
        links.sort();
        if self
            .out
            .get(&owner)
            .map_or(links.is_empty(), |old| *old == links)
        {
            return false;
        }
        self.forget(&owner);
        if !links.is_empty() {
            for (at, link) in links.iter().enumerate() {
                add_owner(
                    self.outgoing.entry(Arc::clone(&link.from)).or_default(),
                    &owner,
                );
                let owners = self.incoming.entry(link.to.clone()).or_default();
                match owners.binary_search_by(|(known, _)| known.cmp(&owner)) {
                    Ok(found) => owners[found].1.push(at),
                    Err(found) => owners.insert(found, (Arc::clone(&owner), vec![at])),
                }
            }
            self.out.insert(owner, links);
        }
        self.revision += 1;
        true
    }

    /// Forgets every link `file` made. Returns whether it had any.
    pub fn remove(&mut self, file: &Path) -> bool {
        let removed = self.forget(file);
        if removed {
            self.revision += 1;
        }
        removed
    }

    fn forget(&mut self, owner: &Path) -> bool {
        let Some(old) = self.out.remove(owner) else {
            return false;
        };
        for link in old {
            if let Some(owners) = self.outgoing.get_mut(&link.from) {
                owners.retain(|known| **known != *owner);
                if owners.is_empty() {
                    self.outgoing.remove(&link.from);
                }
            }
            if let Some(owners) = self.incoming.get_mut(&link.to) {
                owners.retain(|(known, _)| **known != *owner);
                if owners.is_empty() {
                    self.incoming.remove(&link.to);
                }
            }
        }
        true
    }

    /// The files owning a link to `key`, in path order: those whose links change if `key`
    /// comes or goes.
    pub fn linking_to(&self, key: &GraphKey) -> impl Iterator<Item = &Arc<Path>> {
        self.incoming
            .get(key)
            .into_iter()
            .flatten()
            .map(|(owner, _)| owner)
    }

    /// The links `owner` made, sorted.
    #[must_use]
    pub fn owned_by(&self, owner: &Path) -> &[StoredLink] {
        self.out.get(owner).map_or(&[], Vec::as_slice)
    }

    /// The files owning links.
    pub fn owners(&self) -> impl Iterator<Item = &Arc<Path>> {
        self.out.keys()
    }

    /// Every link, owner by owner.
    pub fn links(&self) -> impl Iterator<Item = &StoredLink> {
        self.out.values().flatten()
    }

    /// The links leaving `key`, whoever owns them: in each owner's sorted list, the run of
    /// links from `key`.
    pub fn links_from<'a>(&'a self, key: &'a GraphKey) -> impl Iterator<Item = &'a StoredLink> {
        key.path()
            .and_then(|path| Some((path, self.outgoing.get(path)?)))
            .into_iter()
            .flat_map(move |(path, owners)| {
                owners.iter().flat_map(move |owner| {
                    let links = self.owned_by(owner);
                    let start = links.partition_point(|link| *link.from < *path);
                    let end = start + links[start..].partition_point(|link| *link.from == *path);
                    &links[start..end]
                })
            })
    }

    /// The links reaching `key`, whoever owns them, found by their places in their owners'
    /// lists.
    pub fn links_to<'a>(&'a self, key: &'a GraphKey) -> impl Iterator<Item = &'a StoredLink> {
        self.incoming
            .get(key)
            .into_iter()
            .flatten()
            .flat_map(move |(owner, places)| {
                let links = self.owned_by(owner);
                places.iter().map(move |&at| &links[at])
            })
    }

    /// Whether any link leaves or reaches `key`.
    #[must_use]
    pub fn is_linked(&self, key: &GraphKey) -> bool {
        self.incoming.contains_key(key)
            || key
                .path()
                .is_some_and(|path| self.outgoing.contains_key(path))
    }

    /// `key` and every node within `depth` links of it, following links both ways.
    #[must_use]
    pub fn neighbourhood(&self, key: &GraphKey, depth: u8) -> BTreeSet<GraphKey> {
        self.neighbourhood_where(key, depth, |_| true)
    }

    /// [`LinkStore::neighbourhood`] following only the links `follow` accepts. Only the nodes
    /// reached are visited.
    #[must_use]
    pub fn neighbourhood_where(
        &self,
        key: &GraphKey,
        depth: u8,
        follow: impl Fn(&StoredLink) -> bool,
    ) -> BTreeSet<GraphKey> {
        let mut seen = BTreeSet::from([key.clone()]);
        let mut frontier = vec![key.clone()];
        for _ in 0..depth {
            let mut next = Vec::new();
            for node in &frontier {
                let ends = self
                    .links_from(node)
                    .filter(|link| follow(link))
                    .map(|link| link.to.clone())
                    .chain(
                        self.links_to(node)
                            .filter(|link| follow(link))
                            .map(|link| GraphKey::File(Arc::clone(&link.from))),
                    );
                for end in ends {
                    if seen.insert(end.clone()) {
                        next.push(end);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        seen
    }

    /// The shortest path from `from` to `to` over the links `follow` allows, through the nodes
    /// `enter` allows: in the links' direction when `directed`, as a reader follows them, else
    /// either way. Of the shortest, the one through the lowest keys, a link's own direction
    /// before the reverse, so the answer is stable. `None` when `to` cannot be reached.
    pub fn shortest_path(
        &self,
        from: &GraphKey,
        to: &GraphKey,
        directed: bool,
        follow: impl Fn(&StoredLink) -> bool,
        enter: impl Fn(&GraphKey) -> bool,
    ) -> Option<Vec<PathStep>> {
        if !enter(from) || !enter(to) {
            return None;
        }
        // Each node reached, with the node it was reached from and whether backwards; only
        // looked up, so hashed, as paths compare slowly. The frontier is sorted, so the first
        // to reach a node is the lowest.
        let mut reached: HashMap<GraphKey, Option<(GraphKey, bool)>> =
            HashMap::from([(from.clone(), None)]);
        let mut frontier = vec![from.clone()];
        while !frontier.is_empty() && !reached.contains_key(to) {
            let mut next = BTreeSet::new();
            for node in &frontier {
                let mut ends: HashMap<GraphKey, bool> = HashMap::new();
                for link in self.links_from(node).filter(|link| follow(link)) {
                    ends.insert(link.to.clone(), false);
                }
                if !directed {
                    for link in self.links_to(node).filter(|link| follow(link)) {
                        ends.entry(GraphKey::File(Arc::clone(&link.from)))
                            .or_insert(true);
                    }
                }
                for (end, backwards) in ends {
                    if !reached.contains_key(&end) && enter(&end) {
                        reached.insert(end.clone(), Some((node.clone(), backwards)));
                        next.insert(end);
                    }
                }
            }
            frontier = next.into_iter().collect();
        }
        reached.get(to)?;
        let mut steps = Vec::new();
        let mut at = to.clone();
        while let Some(Some((before, backwards))) = reached.get(&at) {
            let (start, end) = if *backwards {
                (&at, before)
            } else {
                (before, &at)
            };
            let kinds: BTreeSet<EdgeKind> = self
                .links_from(start)
                .filter(|link| link.to == *end && follow(link))
                .map(|link| link.kind)
                .collect();
            steps.push(PathStep {
                key: at.clone(),
                kinds: kinds.into_iter().collect(),
                backwards: *backwards,
            });
            at = before.clone();
        }
        steps.push(PathStep {
            key: from.clone(),
            kinds: Vec::new(),
            backwards: false,
        });
        steps.reverse();
        Some(steps)
    }

    /// A number bumped by every change.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// An owner of links reaching one target, and the places of those links in its list.
type OwnedPlaces = (Arc<Path>, Vec<usize>);

/// Adds `owner` to `owners`, kept sorted and without repeats.
fn add_owner(owners: &mut Vec<Arc<Path>>, owner: &Arc<Path>) {
    if let Err(at) = owners.binary_search(owner) {
        owners.insert(at, Arc::clone(owner));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> GraphKey {
        GraphKey::File(path(name))
    }

    fn path(name: &str) -> Arc<Path> {
        Arc::from(Path::new(name))
    }

    fn owners(store: &LinkStore, key: &GraphKey) -> Vec<String> {
        store
            .linking_to(key)
            .map(|owner| owner.display().to_string())
            .collect()
    }

    #[test]
    fn shortest_path_walks_breadth_first() {
        // a → b → c → d, and a → e → d: the two-step way through e is shortest; f → a only.
        let mut store = LinkStore::new();
        store.set_links(
            path("a"),
            vec![(file("b"), EdgeKind::Xref), (file("e"), EdgeKind::Xref)],
        );
        store.set_links(path("b"), vec![(file("c"), EdgeKind::Xref)]);
        store.set_links(path("c"), vec![(file("d"), EdgeKind::Xref)]);
        store.set_links(
            path("e"),
            vec![(file("d"), EdgeKind::Include), (file("d"), EdgeKind::Xref)],
        );
        store.set_links(path("f"), vec![(file("a"), EdgeKind::Nav)]);
        let any = |_: &StoredLink| true;
        let all = |_: &GraphKey| true;
        let keys = |steps: Vec<PathStep>| {
            steps
                .into_iter()
                .map(|step| {
                    (
                        step.key.path().unwrap().display().to_string(),
                        step.kinds,
                        step.backwards,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(
                store
                    .shortest_path(&file("a"), &file("d"), true, any, all)
                    .unwrap()
            ),
            [
                ("a".to_owned(), vec![], false),
                ("e".to_owned(), vec![EdgeKind::Xref], false),
                (
                    "d".to_owned(),
                    vec![EdgeKind::Xref, EdgeKind::Include],
                    false
                ),
            ]
        );
        // Nothing links from d, or to f: only ignoring direction gets there.
        assert_eq!(
            store.shortest_path(&file("d"), &file("f"), true, any, all),
            None
        );
        assert_eq!(
            keys(
                store
                    .shortest_path(&file("d"), &file("f"), false, any, all)
                    .unwrap()
            ),
            [
                ("d".to_owned(), vec![], false),
                (
                    "e".to_owned(),
                    vec![EdgeKind::Xref, EdgeKind::Include],
                    true
                ),
                ("a".to_owned(), vec![EdgeKind::Xref], true),
                ("f".to_owned(), vec![EdgeKind::Nav], true),
            ]
        );
        // Kept out of e, the path goes the long way; kept from xrefs, it does not go at all.
        assert_eq!(
            store
                .shortest_path(&file("a"), &file("d"), true, any, |key| *key != file("e"))
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            store.shortest_path(
                &file("a"),
                &file("d"),
                true,
                |link| link.kind != EdgeKind::Xref,
                all
            ),
            None
        );
        assert_eq!(
            keys(
                store
                    .shortest_path(&file("a"), &file("a"), true, any, all)
                    .unwrap()
            ),
            [("a".to_owned(), vec![], false)]
        );
    }

    #[test]
    fn link_store_replaces_and_removes_by_file() {
        let mut store = LinkStore::new();
        assert!(store.set_links(
            path("a"),
            vec![(file("b"), EdgeKind::Xref), (file("c"), EdgeKind::Include)],
        ));
        assert_eq!(owners(&store, &file("b")), ["a"]);
        assert_eq!(owners(&store, &file("c")), ["a"]);

        let revision = store.revision();
        assert!(!store.set_links(
            path("a"),
            vec![(file("c"), EdgeKind::Include), (file("b"), EdgeKind::Xref)],
        ));
        assert_eq!(store.revision(), revision, "the same links change nothing");

        assert!(store.set_links(
            path("a"),
            vec![(GraphKey::Unresolved("d.adoc".into()), EdgeKind::Xref)],
        ));
        assert!(owners(&store, &file("b")).is_empty());
        assert!(owners(&store, &file("c")).is_empty());
        assert_eq!(
            owners(&store, &GraphKey::Unresolved("d.adoc".into())),
            ["a"]
        );

        assert!(store.remove(Path::new("a")));
        assert!(!store.remove(Path::new("a")));
        assert_eq!(store.links().count(), 0);
        assert!(store.incoming.is_empty() && store.outgoing.is_empty());
    }

    #[test]
    fn a_navigation_file_owns_links_between_pages() {
        let mut store = LinkStore::new();
        store.set_owned_links(
            path("nav"),
            vec![
                StoredLink {
                    from: path("nav"),
                    to: file("a"),
                    kind: EdgeKind::Nav,
                },
                StoredLink {
                    from: path("a"),
                    to: file("b"),
                    kind: EdgeKind::Nav,
                },
            ],
        );
        store.set_links(path("a"), vec![(file("b"), EdgeKind::Xref)]);
        let mut from_a: Vec<_> = store.links_from(&file("a")).map(|link| link.kind).collect();
        from_a.sort();
        assert_eq!(from_a, [EdgeKind::Xref, EdgeKind::Nav]);
        assert_eq!(owners(&store, &file("b")), ["a", "nav"]);

        // The page's own links are replaced without touching the navigation's.
        store.set_links(path("a"), Vec::new());
        let from_a: Vec<_> = store.links_from(&file("a")).map(|link| link.kind).collect();
        assert_eq!(from_a, [EdgeKind::Nav]);
        assert!(store.is_linked(&file("a")));
    }

    #[test]
    fn links_from_and_to_a_page_are_found_among_a_navigation_files_many() {
        let link = |from: &str, to: &str, kind| StoredLink {
            from: path(from),
            to: file(to),
            kind,
        };
        let mut store = LinkStore::new();
        store.set_owned_links(
            path("nav"),
            vec![
                link("nav", "a", EdgeKind::Nav),
                link("nav", "c", EdgeKind::Nav),
                link("a", "b", EdgeKind::Nav),
                link("a", "c", EdgeKind::Nav),
                link("c", "b", EdgeKind::Nav),
            ],
        );
        store.set_links(path("c"), vec![(file("b"), EdgeKind::Xref)]);
        let ends = |links: Vec<&StoredLink>| -> Vec<String> {
            links
                .into_iter()
                .map(|link| {
                    format!(
                        "{}>{}",
                        link.from.display(),
                        link.to.path().unwrap().display()
                    )
                })
                .collect()
        };
        assert_eq!(ends(store.links_from(&file("a")).collect()), ["a>b", "a>c"]);
        assert_eq!(ends(store.links_from(&file("c")).collect()), ["c>b", "c>b"]);
        assert_eq!(
            ends(store.links_to(&file("b")).collect()),
            ["c>b", "a>b", "c>b"]
        );
        assert_eq!(ends(store.links_to(&file("c")).collect()), ["a>c", "nav>c"]);
        assert!(store.links_from(&file("b")).next().is_none());

        // Replaced, the navigation's links are found at their new places.
        store.set_owned_links(path("nav"), vec![link("nav", "c", EdgeKind::Nav)]);
        assert_eq!(ends(store.links_to(&file("c")).collect()), ["nav>c"]);
        assert_eq!(ends(store.links_to(&file("b")).collect()), ["c>b"]);
        assert!(store.links_from(&file("a")).next().is_none());
    }

    #[test]
    fn neighbourhood_walks_both_ways() {
        // a → b → c → d, e → a, and f alone.
        let mut store = LinkStore::new();
        store.set_links(path("a"), vec![(file("b"), EdgeKind::Xref)]);
        store.set_links(path("b"), vec![(file("c"), EdgeKind::Xref)]);
        store.set_links(path("c"), vec![(file("d"), EdgeKind::Include)]);
        store.set_links(path("e"), vec![(file("a"), EdgeKind::Xref)]);
        store.set_links(path("f"), Vec::new());

        let names = |set: BTreeSet<GraphKey>| -> Vec<String> {
            set.iter()
                .map(|key| key.path().unwrap().display().to_string())
                .collect()
        };
        assert_eq!(names(store.neighbourhood(&file("b"), 1)), ["a", "b", "c"]);
        assert_eq!(
            names(store.neighbourhood(&file("b"), 2)),
            ["a", "b", "c", "d", "e"]
        );
        assert_eq!(names(store.neighbourhood(&file("f"), 3)), ["f"]);
        assert_eq!(
            names(store.neighbourhood_where(&file("b"), 2, |link| link.kind == EdgeKind::Xref)),
            ["a", "b", "c", "e"]
        );
    }
}
