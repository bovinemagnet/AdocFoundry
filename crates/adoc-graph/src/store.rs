//! Each file's links, kept by the file whose text made them, with the reverse maps that find
//! who links from or to a node without visiting every file.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::Arc,
};

use crate::{EdgeKind, GraphKey};

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
/// owners in each are few, kept in a small list.
#[derive(Clone, Debug, Default)]
pub struct LinkStore {
    /// Each owner's links, sorted.
    out: BTreeMap<Arc<Path>, Vec<StoredLink>>,
    /// The owners of the links leaving each file.
    outgoing: HashMap<Arc<Path>, Vec<Arc<Path>>>,
    /// The owners of the links reaching each target.
    incoming: HashMap<GraphKey, Vec<Arc<Path>>>,
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
            for link in &links {
                add_owner(
                    self.outgoing.entry(Arc::clone(&link.from)).or_default(),
                    &owner,
                );
                add_owner(self.incoming.entry(link.to.clone()).or_default(), &owner);
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
                owners.retain(|known| **known != *owner);
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
        self.incoming.get(key).into_iter().flatten()
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

    /// The links leaving `key`, whoever owns them.
    pub fn links_from<'a>(&'a self, key: &'a GraphKey) -> impl Iterator<Item = &'a StoredLink> {
        key.path()
            .and_then(|path| self.outgoing.get(path))
            .into_iter()
            .flatten()
            .flat_map(move |owner| self.owned_by(owner))
            .filter(move |link| key.path() == Some(&*link.from))
    }

    /// The links reaching `key`, whoever owns them.
    pub fn links_to<'a>(&'a self, key: &'a GraphKey) -> impl Iterator<Item = &'a StoredLink> {
        self.linking_to(key)
            .flat_map(move |owner| self.owned_by(owner))
            .filter(move |link| link.to == *key)
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

    /// A number bumped by every change.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

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
