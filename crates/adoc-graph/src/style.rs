//! How nodes look, shared by every drawing of the graph: their size, and their colour by
//! component, module or family, so the app's canvas and an exported image agree.

use crate::{GraphNode, GraphSnapshot};

/// What colours the nodes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColourBy {
    #[default]
    Component,
    Module,
    Family,
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

/// Each colour group's place, in name order: components, modules or families.
#[must_use]
pub fn colour_groups(snapshot: &GraphSnapshot, by: ColourBy) -> Vec<String> {
    let mut groups: Vec<String> = snapshot
        .nodes
        .iter()
        .filter_map(|node| group_of(node, by))
        .collect();
    groups.sort();
    groups.dedup();
    groups
}

fn group_of(node: &GraphNode, by: ColourBy) -> Option<String> {
    let coordinate = node.coordinate.as_ref()?;
    Some(match by {
        ColourBy::Component => coordinate.component.clone(),
        ColourBy::Module => format!("{}:{}", coordinate.component, coordinate.module),
        ColourBy::Family => coordinate.family.clone(),
    })
}

/// A node's colour: its group's place in a palette of hues that also differ in lightness, set
/// for a light background or a dark one. `None` for nodes outside Antora and unresolved
/// targets, which are drawn grey.
#[must_use]
pub fn node_colour(node: &GraphNode, groups: &[String], by: ColourBy, dark: bool) -> Option<Hsl> {
    const HUES: [f32; 8] = [0.58, 0.08, 0.36, 0.92, 0.15, 0.75, 0.48, 0.0];
    let group = group_of(node, by)?;
    let place = groups.iter().position(|known| *known == group).unwrap_or(0);
    let lighter = (place / HUES.len()) % 2 == 1;
    let l = match (dark, lighter) {
        (false, false) => 0.42,
        (false, true) => 0.58,
        (true, false) => 0.62,
        (true, true) => 0.76,
    };
    Some(Hsl {
        h: HUES[place % HUES.len()],
        s: 0.65,
        l,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
