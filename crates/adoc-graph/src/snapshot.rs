//! Snapshots: the nodes a query covers and the links among them, filtered, with the links of
//! one kind between two nodes merged into one edge.

use std::collections::HashMap;

use crate::{
    EdgeKind, GraphEdge, GraphFilter, GraphKey, GraphNode, GraphSnapshot, NodeCoordinate, NodeKind,
    StoredLink,
};

/// A node as the caller knows it, before filtering and counting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeInput {
    pub key: GraphKey,
    pub title: String,
    pub kind: NodeKind,
    pub coordinate: Option<NodeCoordinate>,
    /// No link leaves or reaches it, of any kind.
    pub orphan: bool,
}

impl GraphSnapshot {
    /// The snapshot of `nodes` and the `links` among them, as `filter` allows. A link to an
    /// unresolved target makes its node; a link to a file not in `nodes` is left out. `centre`,
    /// a local graph's page, is kept whatever the filter says.
    #[must_use]
    pub fn build<'a>(
        revision: u64,
        nodes: impl IntoIterator<Item = NodeInput>,
        links: impl IntoIterator<Item = &'a StoredLink>,
        filter: &GraphFilter,
        centre: Option<&GraphKey>,
    ) -> Self {
        let in_components = |coordinate: Option<&NodeCoordinate>| {
            filter.components.as_ref().is_none_or(|components| {
                coordinate.is_some_and(|coordinate| components.contains(&coordinate.component))
            })
        };
        // Keys are hashed while the snapshot is put together: a path compares component by
        // component, too slowly for thousands of nodes. Only the result is sorted.
        let mut kept: HashMap<GraphKey, NodeInput> = nodes
            .into_iter()
            .filter(|node| {
                centre == Some(&node.key)
                    || (filter.allows_kind(node.kind)
                        && (filter.orphans || !node.orphan)
                        && in_components(node.coordinate.as_ref()))
            })
            .map(|node| (node.key.clone(), node))
            .collect();

        let mut merged: HashMap<(GraphKey, GraphKey, EdgeKind), usize> = HashMap::new();
        for link in links {
            let from = GraphKey::File(link.from.clone());
            if !filter.allows_edge(link.kind) || from == link.to || !kept.contains_key(&from) {
                continue;
            }
            match &link.to {
                GraphKey::Unresolved(text) if filter.unresolved => {
                    kept.entry(link.to.clone()).or_insert_with(|| NodeInput {
                        key: link.to.clone(),
                        title: text.clone(),
                        kind: NodeKind::Unresolved,
                        coordinate: None,
                        orphan: false,
                    });
                }
                to if kept.contains_key(to) => {}
                _ => continue,
            }
            *merged
                .entry((from, link.to.clone(), link.kind))
                .or_default() += 1;
        }

        let mut nodes: Vec<GraphNode> = kept
            .into_values()
            .map(|node| GraphNode {
                key: node.key,
                title: node.title,
                kind: node.kind,
                coordinate: node.coordinate,
                links_out: 0,
                links_in: 0,
            })
            .collect();
        nodes.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        let place: HashMap<&GraphKey, usize> = nodes
            .iter()
            .enumerate()
            .map(|(at, node)| (&node.key, at))
            .collect();
        // Edges sorted by their ends' places, which is their keys' order.
        let mut placed: Vec<(usize, usize, EdgeKind, usize)> = merged
            .iter()
            .map(|((from, to, kind), count)| (place[from], place[to], *kind, *count))
            .collect();
        placed.sort_unstable();
        let mut counts = vec![(0, 0); nodes.len()];
        let edges: Vec<GraphEdge> = placed
            .into_iter()
            .map(|(from, to, kind, count)| {
                counts[from].0 += 1;
                counts[to].1 += 1;
                GraphEdge {
                    from: nodes[from].key.clone(),
                    to: nodes[to].key.clone(),
                    kind,
                    count,
                }
            })
            .collect();
        for (node, (out, into)) in nodes.iter_mut().zip(counts) {
            node.links_out = out;
            node.links_in = into;
        }
        Self {
            revision,
            nodes,
            edges,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use super::*;
    use crate::LinkStore;

    fn file(name: &str) -> GraphKey {
        GraphKey::File(Arc::from(Path::new(name)))
    }

    fn node(name: &str, kind: NodeKind, component: &str, store: &LinkStore) -> NodeInput {
        NodeInput {
            key: file(name),
            title: name.to_uppercase(),
            kind,
            coordinate: Some(NodeCoordinate {
                component: component.to_owned(),
                version: None,
                module: "ROOT".to_owned(),
                family: "page".to_owned(),
            }),
            orphan: !store.is_linked(&file(name)),
        }
    }

    fn store() -> LinkStore {
        let mut store = LinkStore::new();
        let path = |name: &str| Arc::from(Path::new(name));
        store.set_links(
            path("a"),
            vec![
                (file("b"), EdgeKind::Xref),
                (file("b"), EdgeKind::Xref),
                (file("p"), EdgeKind::Include),
                (GraphKey::Unresolved("gone.adoc".into()), EdgeKind::Xref),
            ],
        );
        store.set_links(path("b"), vec![(file("x"), EdgeKind::Xref)]);
        store
    }

    fn snapshot(filter: &GraphFilter) -> GraphSnapshot {
        let store = store();
        let nodes = vec![
            node("a", NodeKind::Page, "one", &store),
            node("b", NodeKind::Page, "one", &store),
            node("p", NodeKind::Partial, "one", &store),
            node("o", NodeKind::Page, "one", &store),
            node("x", NodeKind::Page, "two", &store),
        ];
        GraphSnapshot::build(3, nodes, store.links(), filter, None)
    }

    fn keys(snapshot: &GraphSnapshot) -> Vec<String> {
        snapshot
            .nodes
            .iter()
            .map(|node| match &node.key {
                GraphKey::File(path) => path.display().to_string(),
                GraphKey::Unresolved(text) => format!("?{text}"),
                GraphKey::Group(name) => format!("[{name}]"),
            })
            .collect()
    }

    #[test]
    fn links_of_a_kind_merge_and_count() {
        let all = snapshot(&GraphFilter {
            unresolved: true,
            ..GraphFilter::default()
        });
        assert_eq!(keys(&all), ["a", "b", "o", "p", "x", "?gone.adoc"]);
        let a_to_b = &all.edges[0];
        assert_eq!((a_to_b.kind, a_to_b.count), (EdgeKind::Xref, 2));
        let a = all.node(&file("a")).unwrap();
        assert_eq!((a.links_out, a.links_in), (3, 0));
    }

    #[test]
    fn filters_leave_out_nodes_and_their_edges() {
        let plain = snapshot(&GraphFilter {
            partials: false,
            orphans: false,
            ..GraphFilter::default()
        });
        assert_eq!(keys(&plain), ["a", "b", "x"]);
        assert!(plain.edges.iter().all(|edge| edge.kind == EdgeKind::Xref));

        let one = snapshot(&GraphFilter {
            components: Some(["one".to_owned()].into()),
            ..GraphFilter::default()
        });
        assert_eq!(keys(&one), ["a", "b", "o", "p"]);
        assert_eq!(one.edges.len(), 2);
    }
}
