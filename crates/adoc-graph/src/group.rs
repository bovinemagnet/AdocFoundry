//! Groups of nodes: each node's Antora module or component version, which the layout draws
//! together and a front end may collapse into one node, its links to other groups merged.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{EdgeKind, GraphEdge, GraphKey, GraphNode, GraphSnapshot, NodeCoordinate, NodeKind};

/// What groups the nodes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GroupBy {
    /// Each module of each component version: `version@component:module`.
    Module,
    /// Each component version: `version@component`.
    Component,
}

/// The group of nodes outside Antora and of unresolved targets.
pub const OTHER_GROUP: &str = "Other";

/// The name of `node`'s group: its module or component version as a resource ID writes it
/// (the version left out when there is none), [`OTHER_GROUP`] outside Antora, and a collapsed
/// group's own name.
#[must_use]
pub fn group_name(node: &GraphNode, by: GroupBy) -> String {
    if let GraphKey::Group(name) = &node.key {
        return name.clone();
    }
    match (&node.coordinate, node.kind) {
        (Some(coordinate), kind) if kind != NodeKind::Unresolved => name_of(coordinate, by),
        _ => OTHER_GROUP.to_owned(),
    }
}

fn name_of(coordinate: &NodeCoordinate, by: GroupBy) -> String {
    let version = coordinate
        .version
        .as_deref()
        .filter(|version| !version.is_empty())
        .map(|version| format!("{version}@"))
        .unwrap_or_default();
    match by {
        GroupBy::Module => format!("{version}{}:{}", coordinate.component, coordinate.module),
        GroupBy::Component => format!("{version}{}", coordinate.component),
    }
}

/// Every node's group in a snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Groups {
    /// The groups' names, sorted.
    pub names: Vec<String>,
    /// Each node's group, as a place in `names`, in the snapshot's node order.
    pub of: Vec<usize>,
}

impl Groups {
    #[must_use]
    pub fn of(snapshot: &GraphSnapshot, by: GroupBy) -> Self {
        let named: Vec<String> = snapshot
            .nodes
            .iter()
            .map(|node| group_name(node, by))
            .collect();
        let mut names = named.clone();
        names.sort();
        names.dedup();
        let place: HashMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(at, name)| (name.as_str(), at))
            .collect();
        let of = named.iter().map(|name| place[name.as_str()]).collect();
        Self { names, of }
    }

    /// How many nodes each group holds, by place.
    #[must_use]
    pub fn sizes(&self) -> Vec<usize> {
        let mut sizes = vec![0; self.names.len()];
        for &group in &self.of {
            sizes[group] += 1;
        }
        sizes
    }
}

impl GraphSnapshot {
    /// The snapshot with each group named in `collapsed` drawn as one node, keyed
    /// [`GraphKey::Group`] and titled with its name, its links to other nodes merged by kind
    /// with their counts summed, and links within it left out.
    #[must_use]
    pub fn collapse(&self, by: GroupBy, collapsed: &BTreeSet<String>) -> Self {
        if collapsed.is_empty() {
            return self.clone();
        }
        // Each node's key in the result: its own, or its group's.
        let mut members: BTreeMap<String, Vec<&GraphNode>> = BTreeMap::new();
        let shown: Vec<GraphKey> = self
            .nodes
            .iter()
            .map(|node| {
                let name = group_name(node, by);
                if collapsed.contains(&name) {
                    members.entry(name.clone()).or_default().push(node);
                    GraphKey::Group(name)
                } else {
                    node.key.clone()
                }
            })
            .collect();
        let place: HashMap<&GraphKey, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(at, node)| (&node.key, at))
            .collect();
        let mut merged: BTreeMap<(GraphKey, GraphKey, EdgeKind), usize> = BTreeMap::new();
        for edge in &self.edges {
            let from = &shown[place[&edge.from]];
            let to = &shown[place[&edge.to]];
            if from != to {
                *merged
                    .entry((from.clone(), to.clone(), edge.kind))
                    .or_default() += edge.count;
            }
        }
        let mut nodes: Vec<GraphNode> = self
            .nodes
            .iter()
            .filter(|node| !collapsed.contains(&group_name(node, by)))
            .cloned()
            .chain(members.into_iter().map(|(name, members)| {
                GraphNode {
                    key: GraphKey::Group(name.clone()),
                    title: name,
                    kind: NodeKind::Group,
                    // The members share a module or component version, which colours the node.
                    coordinate: members[0].coordinate.clone(),
                    links_out: 0,
                    links_in: 0,
                    // The group changed when its newest member did.
                    last_changed: members
                        .iter()
                        .filter_map(|member| member.last_changed)
                        .max_by_key(|changed| match changed {
                            crate::LastChanged::Uncommitted => i64::MAX,
                            crate::LastChanged::At(time) => *time,
                        }),
                }
            }))
            .collect();
        nodes.sort_by(|left, right| left.key.cmp(&right.key));
        let edges: Vec<GraphEdge> = merged
            .into_iter()
            .map(|((from, to, kind), count)| GraphEdge {
                from,
                to,
                kind,
                count,
            })
            .collect();
        crate::recount(&mut nodes, &edges);
        Self {
            revision: self.revision,
            nodes,
            edges,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use super::*;

    fn file(name: &str) -> GraphKey {
        GraphKey::File(Arc::from(Path::new(name)))
    }

    fn node(name: &str, component: &str, module: &str) -> GraphNode {
        GraphNode {
            key: file(name),
            title: name.to_owned(),
            kind: NodeKind::Page,
            coordinate: Some(NodeCoordinate {
                component: component.to_owned(),
                version: None,
                module: module.to_owned(),
                family: "page".to_owned(),
            }),
            links_out: 0,
            links_in: 0,
            last_changed: None,
        }
    }

    fn edge(from: &str, to: &str, kind: EdgeKind, count: usize) -> GraphEdge {
        GraphEdge {
            from: file(from),
            to: file(to),
            kind,
            count,
        }
    }

    /// `a1` and `a2` in one:a, `b1` in one:b, `c1` in two:ROOT and `loose` outside Antora.
    fn snapshot() -> GraphSnapshot {
        let mut loose = node("loose", "", "");
        loose.coordinate = None;
        let mut edges = vec![
            edge("a1", "a2", EdgeKind::Xref, 1),
            edge("a1", "b1", EdgeKind::Xref, 2),
            edge("a2", "b1", EdgeKind::Xref, 1),
            edge("a2", "b1", EdgeKind::Include, 1),
            edge("b1", "a1", EdgeKind::Xref, 1),
            edge("c1", "a2", EdgeKind::Nav, 1),
            edge("loose", "c1", EdgeKind::Xref, 1),
        ];
        edges.sort();
        GraphSnapshot {
            revision: 4,
            nodes: vec![
                node("a1", "one", "a"),
                node("a2", "one", "a"),
                node("b1", "one", "b"),
                node("c1", "two", "ROOT"),
                loose,
            ],
            edges,
        }
    }

    #[test]
    fn groups_name_modules_and_component_versions() {
        let snapshot = snapshot();
        let modules = Groups::of(&snapshot, GroupBy::Module);
        assert_eq!(modules.names, ["Other", "one:a", "one:b", "two:ROOT"]);
        assert_eq!(modules.of, [1, 1, 2, 3, 0]);
        assert_eq!(modules.sizes(), [1, 2, 1, 1]);
        let components = Groups::of(&snapshot, GroupBy::Component);
        assert_eq!(components.names, ["Other", "one", "two"]);

        let mut versioned = node("v", "one", "a");
        versioned.coordinate.as_mut().unwrap().version = Some("2.0".to_owned());
        assert_eq!(group_name(&versioned, GroupBy::Module), "2.0@one:a");
        assert_eq!(group_name(&versioned, GroupBy::Component), "2.0@one");
    }

    #[test]
    fn collapsing_a_group_merges_its_edges() {
        let snapshot = snapshot();
        let collapsed = snapshot.collapse(GroupBy::Module, &["one:a".to_owned()].into());
        let group = GraphKey::Group("one:a".to_owned());
        let keys: Vec<&GraphKey> = collapsed.nodes.iter().map(|node| &node.key).collect();
        assert_eq!(keys, [&file("b1"), &file("c1"), &file("loose"), &group]);
        let node = collapsed.node(&group).unwrap();
        assert_eq!((node.kind, node.title.as_str()), (NodeKind::Group, "one:a"));
        let edges: Vec<(&GraphKey, &GraphKey, EdgeKind, usize)> = collapsed
            .edges
            .iter()
            .map(|edge| (&edge.from, &edge.to, edge.kind, edge.count))
            .collect();
        // a1 → a2 is inside the group; a1 → b1 (2) and a2 → b1 (1) merge to 3.
        assert_eq!(
            edges,
            [
                (&file("b1"), &group, EdgeKind::Xref, 1),
                (&file("c1"), &group, EdgeKind::Nav, 1),
                (&file("loose"), &file("c1"), EdgeKind::Xref, 1),
                (&group, &file("b1"), EdgeKind::Xref, 3),
                (&group, &file("b1"), EdgeKind::Include, 1),
            ]
        );
        assert_eq!((node.links_out, node.links_in), (2, 2));

        // Two groups collapsed: the links between them merge too.
        let both = snapshot.collapse(
            GroupBy::Module,
            &["one:a".to_owned(), "one:b".to_owned()].into(),
        );
        let between: Vec<usize> = both
            .edges
            .iter()
            .filter(|edge| matches!(edge.from, GraphKey::Group(_)))
            .filter(|edge| matches!(edge.to, GraphKey::Group(_)))
            .map(|edge| edge.count)
            .collect();
        assert_eq!(between, [3, 1, 1]);
        assert!(snapshot.collapse(GroupBy::Module, &BTreeSet::new()) == snapshot);
    }
}
