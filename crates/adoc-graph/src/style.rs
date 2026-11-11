//! How nodes look, shared by every drawing of the graph: their size, and their colour by
//! component, module, family or age, so the app's canvas and an exported image agree.

use crate::{GraphNode, GraphSnapshot, LastChanged, NodeCoordinate};

/// What colours the nodes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColourBy {
    #[default]
    Component,
    Module,
    Family,
    /// How long ago the node's file last changed, from recent to old across the snapshot.
    Age,
}

/// A colour as hue, saturation and lightness, each from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

impl Hsl {
    /// The colour as `#rrggbb`.
    #[must_use]
    pub fn to_hex(self) -> String {
        let chroma = (1.0 - (2.0 * self.l - 1.0).abs()) * self.s;
        let sector = (self.h.rem_euclid(1.0)) * 6.0;
        let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
        let (r, g, b) = match sector as u8 {
            0 => (chroma, second, 0.0),
            1 => (second, chroma, 0.0),
            2 => (0.0, chroma, second),
            3 => (0.0, second, chroma),
            4 => (second, 0.0, chroma),
            _ => (chroma, 0.0, second),
        };
        let lift = self.l - chroma / 2.0;
        let byte = |channel: f32| ((channel + lift).clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
    }
}

/// The radius of a node with `links` edges, at zoom 1.
#[must_use]
pub fn node_radius(links: usize) -> f32 {
    (4.0 + (links as f32).sqrt() * 1.6).min(14.0)
}

/// The colours of one snapshot's nodes: each colour group's place, in name order
/// (components, modules or families), among the snapshot's groups and any others it is given,
/// or the span of its nodes' ages.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Palette {
    by: ColourBy,
    groups: Vec<String>,
    /// The newest and oldest change among the nodes, uncommitted changes counting as newer than
    /// any commit.
    ages: Option<(i64, i64)>,
}

/// An uncommitted change's place on the age scale: newer than any commit.
fn age_key(changed: LastChanged) -> i64 {
    match changed {
        LastChanged::Uncommitted => i64::MAX,
        LastChanged::At(time) => time,
    }
}

impl Palette {
    #[must_use]
    pub fn of(snapshot: &GraphSnapshot, by: ColourBy) -> Self {
        let mut groups: Vec<String> = snapshot
            .nodes
            .iter()
            .filter_map(|node| group_of(node, by))
            .collect();
        groups.sort();
        groups.dedup();
        let ages = snapshot
            .nodes
            .iter()
            .filter_map(|node| node.last_changed)
            .filter_map(|changed| match changed {
                LastChanged::At(time) => Some(time),
                LastChanged::Uncommitted => None,
            })
            .fold(None, |span: Option<(i64, i64)>, time| {
                Some(span.map_or((time, time), |(newest, oldest)| {
                    (newest.max(time), oldest.min(time))
                }))
            });
        Self { by, groups, ages }
    }

    /// The palette with `groups` ranked among the snapshot's own: a part of the graph given
    /// the whole's groups ([`colour_groups`]) colours each group as the whole does.
    #[must_use]
    pub fn with_groups(mut self, groups: impl IntoIterator<Item = String>) -> Self {
        self.groups.extend(groups);
        self.groups.sort();
        self.groups.dedup();
        self
    }

    #[must_use]
    pub fn by(&self) -> ColourBy {
        self.by
    }

    /// A node's colour: its group's place in a palette of hues that also differ in lightness,
    /// muted past the palette's sixteen colours with hues of their own, or by age a scale from green (recent) through amber to red (old), set for a light
    /// background or a dark one. `None` for nodes outside Antora, unresolved targets and, by
    /// age, nodes with no history, which are drawn grey.
    #[must_use]
    pub fn colour(&self, node: &GraphNode, dark: bool) -> Option<Hsl> {
        if self.by == ColourBy::Age {
            let changed = node.last_changed?;
            // 0 for the newest, 1 for the oldest.
            let old = match self.ages {
                Some((newest, oldest)) if newest > oldest => {
                    let time = age_key(changed).min(newest);
                    (newest - time) as f32 / (newest - oldest) as f32
                }
                _ => 0.0,
            };
            return Some(Hsl {
                h: 0.33 * (1.0 - old),
                s: 0.7,
                l: if dark { 0.6 } else { 0.45 },
            });
        }
        node_colour(node, &self.groups, self.by, dark)
    }
}

/// The groups that colour `snapshot`'s nodes, in name order: give them to the palette of a part
/// of it ([`Palette::with_groups`]) and each group keeps its colour there.
#[must_use]
pub fn colour_groups(snapshot: &GraphSnapshot, by: ColourBy) -> Vec<String> {
    Palette::of(snapshot, by).groups
}

fn group_of(node: &GraphNode, by: ColourBy) -> Option<String> {
    colour_group(node.coordinate.as_ref()?, by)
}

/// The colour group of a node at `coordinate`: its component, `component:module` or family;
/// `None` by age.
#[must_use]
pub fn colour_group(coordinate: &NodeCoordinate, by: ColourBy) -> Option<String> {
    match by {
        ColourBy::Component => Some(coordinate.component.clone()),
        ColourBy::Module => Some(format!("{}:{}", coordinate.component, coordinate.module)),
        ColourBy::Family => Some(coordinate.family.clone()),
        ColourBy::Age => None,
    }
}

fn node_colour(node: &GraphNode, groups: &[String], by: ColourBy, dark: bool) -> Option<Hsl> {
    const HUES: [f32; 8] = [0.58, 0.08, 0.36, 0.92, 0.15, 0.75, 0.48, 0.0];
    /// The golden ratio's fractional part, which spreads hues evenly however many there are.
    const GOLDEN: f32 = 0.618_034;
    let group = group_of(node, by)?;
    let place = groups.binary_search(&group).unwrap_or(0);
    let palette = 2 * HUES.len();
    // The first sixteen: eight hues, darker then lighter. Past them, muted, so none repeats one
    // of the sixteen, each with a hue of its own, alternately darker and lighter.
    let (h, s, lighter) = if place < palette {
        (HUES[place % HUES.len()], 0.65, place >= HUES.len())
    } else {
        (
            (HUES[0] + place as f32 * GOLDEN).fract(),
            0.35,
            place % 2 == 1,
        )
    };
    let l = match (dark, lighter) {
        (false, false) => 0.42,
        (false, true) => 0.58,
        (true, false) => 0.62,
        (true, true) => 0.76,
    };
    Some(Hsl { h, s, l })
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use super::*;
    use crate::{GraphKey, NodeKind};

    #[test]
    fn age_runs_from_green_for_the_newest_to_red_for_the_oldest() {
        let node = |name: &str, last_changed| GraphNode {
            key: GraphKey::File(Arc::from(Path::new(name))),
            title: name.to_owned(),
            kind: NodeKind::Page,
            coordinate: None,
            links_out: 0,
            links_in: 0,
            last_changed,
        };
        let snapshot = GraphSnapshot {
            revision: 1,
            nodes: vec![
                node("new", Some(LastChanged::At(300))),
                node("middle", Some(LastChanged::At(200))),
                node("old", Some(LastChanged::At(100))),
                node("editing", Some(LastChanged::Uncommitted)),
                node("unknown", None),
            ],
            edges: Vec::new(),
        };
        let palette = Palette::of(&snapshot, ColourBy::Age);
        let hue = |at: usize| palette.colour(&snapshot.nodes[at], false).map(|hsl| hsl.h);
        assert_eq!(hue(0), Some(0.33));
        assert!((hue(1).unwrap() - 0.165).abs() < 1e-6);
        assert_eq!(hue(2), Some(0.0));
        assert_eq!(hue(3), Some(0.33), "uncommitted is as new as the newest");
        assert_eq!(hue(4), None);
        // Grouping colours leave age out.
        assert_eq!(
            Palette::of(&snapshot, ColourBy::Module).colour(&snapshot.nodes[0], false),
            None
        );
    }

    /// A snapshot of one page in each of `modules` modules.
    fn modules(modules: usize) -> GraphSnapshot {
        let mut nodes: Vec<GraphNode> = (0..modules)
            .map(|n| GraphNode {
                key: GraphKey::File(Arc::from(Path::new(&format!("m{n}/index.adoc")))),
                title: format!("m{n}"),
                kind: NodeKind::Page,
                coordinate: Some(crate::NodeCoordinate {
                    component: "docs".to_owned(),
                    version: None,
                    module: format!("m{n}"),
                    family: "page".to_owned(),
                }),
                links_out: 0,
                links_in: 0,
                last_changed: None,
            })
            .collect();
        nodes.sort_by(|left, right| left.key.cmp(&right.key));
        GraphSnapshot {
            revision: 1,
            nodes,
            edges: Vec::new(),
        }
    }

    #[test]
    fn a_group_keeps_its_colour_in_part_of_the_graph() {
        let whole = modules(21);
        let groups = colour_groups(&whole, ColourBy::Module);
        let m1 = GraphKey::File(Arc::from(Path::new("m1/index.adoc")));
        let colour = |snapshot: &GraphSnapshot, palette: &Palette| {
            palette
                .colour(snapshot.node(&m1).unwrap(), false)
                .unwrap()
                .to_hex()
        };
        let everywhere = colour(&whole, &Palette::of(&whole, ColourBy::Module));
        let part = whole.restrict(|key| {
            key.path()
                .is_some_and(|path| path.starts_with("m1") || path.starts_with("m2"))
        });
        let palette = Palette::of(&part, ColourBy::Module).with_groups(groups);
        assert_eq!(colour(&part, &palette), everywhere);
    }

    #[test]
    fn groups_past_the_palettes_size_have_colours_of_their_own() {
        for count in [17, 40] {
            let snapshot = modules(count);
            for dark in [false, true] {
                let palette = Palette::of(&snapshot, ColourBy::Module);
                let colours: std::collections::BTreeSet<String> = snapshot
                    .nodes
                    .iter()
                    .map(|node| palette.colour(node, dark).unwrap().to_hex())
                    .collect();
                assert_eq!(colours.len(), count, "{count} groups, dark {dark}");
            }
        }
    }

    #[test]
    fn hsl_converts_to_hex() {
        let hex = |h, s, l| Hsl { h, s, l }.to_hex();
        assert_eq!(hex(0.0, 1.0, 0.5), "#ff0000");
        assert_eq!(hex(1.0 / 3.0, 1.0, 0.5), "#00ff00");
        assert_eq!(hex(2.0 / 3.0, 1.0, 0.5), "#0000ff");
        assert_eq!(hex(0.0, 0.0, 1.0), "#ffffff");
        assert_eq!(hex(0.5, 0.0, 0.0), "#000000");
    }
}
