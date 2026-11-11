//! The graph as an SVG image, drawn as the app's canvas draws it: pages filled, partials
//! hollow, unresolved targets dashed, navigation files square; xrefs solid, includes dashed,
//! navigation dotted and bowed, links to unresolved targets dash-dot, and a link of one kind
//! repeated between two files heavier. Every node is labelled with its title.

use std::{fmt::Write as _, path::Path, sync::Arc};

use crate::{
    style::{node_radius, Palette},
    ColourBy, EdgeKind, GraphKey, GraphSnapshot, NodeKind, Point,
};

/// How an exported image looks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SvgStyle {
    pub colour_by: ColourBy,
    /// Light text and lines on a dark background.
    pub dark: bool,
    /// Colour groups to rank the snapshot's among ([`crate::Palette::with_groups`]): the whole
    /// graph's, so that a part of it is coloured as the whole is.
    pub groups: Vec<String>,
}

/// The colours of one background.
struct Theme {
    background: &'static str,
    text: &'static str,
    /// Lines, and nodes with no colour group.
    muted: &'static str,
    /// Navigation edges.
    accent: &'static str,
}

const LIGHT: Theme = Theme {
    background: "#ffffff",
    text: "#0a0a0a",
    muted: "#737373",
    accent: "#2563eb",
};

const DARK: Theme = Theme {
    background: "#0a0a0a",
    text: "#fafafa",
    muted: "#a3a3a3",
    accent: "#60a5fa",
};

const FONT_SIZE: f32 = 11.0;
/// How far off the line between its ends a navigation edge's control point lies, as a part of
/// the line's length.
const BOW: f32 = 0.15;
const MARGIN: f32 = 20.0;

/// The widest `text` is likely to be in the label font, for the image's bounds. Text cannot be
/// measured here, so the estimate errs on the wide side: an em for East Asian wide and
/// full-width characters and emoji, more for capitals than for other letters.
fn label_width(text: &str) -> f32 {
    text.chars()
        .map(|character| {
            if is_wide(character) {
                FONT_SIZE
            } else if character.is_uppercase() || matches!(character, 'm' | 'w' | '@' | '%') {
                9.0
            } else {
                7.0
            }
        })
        .sum()
}

/// Whether `character` is drawn an em wide: the East Asian wide and full-width blocks, and
/// emoji.
fn is_wide(character: char) -> bool {
    matches!(
        u32::from(character),
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD
    )
}

/// The control point of the curve a navigation edge from `from` to `to` is drawn as, bowed to
/// one side as the canvas draws it.
fn bow(from: Point, to: Point) -> Point {
    let middle = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    Point::new(middle.x - dy * BOW, middle.y + dx * BOW)
}

/// `snapshot` drawn at `positions`, one for each node in the snapshot's order (as
/// [`crate::Layout::positions`] gives them), as a standalone SVG document. The same snapshot
/// and positions always give the same bytes.
#[must_use]
pub fn to_svg(snapshot: &GraphSnapshot, positions: &[Point], style: &SvgStyle) -> String {
    let theme = if style.dark { &DARK } else { &LIGHT };
    let palette = Palette::of(snapshot, style.colour_by).with_groups(style.groups.iter().cloned());
    let at = |place: usize| positions.get(place).copied().unwrap_or_default();
    let radii: Vec<f32> = snapshot
        .nodes
        .iter()
        .map(|node| node_radius(node.links_in + node.links_out))
        .collect();

    let place_of = |key: &GraphKey| {
        snapshot
            .nodes
            .binary_search_by(|node| node.key.cmp(key))
            .ok()
    };
    let (mut left, mut top, mut right, mut bottom) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for (place, node) in snapshot.nodes.iter().enumerate() {
        let (point, radius) = (at(place), radii[place]);
        let label = label_width(&node.title);
        if place == 0 {
            (left, top, right, bottom) = (point.x, point.y, point.x, point.y);
        }
        left = left.min(point.x - radius);
        top = top.min(point.y - radius);
        right = right.max(point.x + radius + 3.0 + label);
        bottom = bottom.max(point.y + radius);
    }
    // A navigation edge's curve reaches furthest from its line half-way along, half-way
    // between the line's middle and the control point.
    for edge in snapshot
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Nav)
    {
        let (Some(from), Some(to)) = (place_of(&edge.from), place_of(&edge.to)) else {
            continue;
        };
        let (from, to) = (at(from), at(to));
        let control = bow(from, to);
        let apex = Point::new(
            ((from.x + to.x) / 2.0 + control.x) / 2.0,
            ((from.y + to.y) / 2.0 + control.y) / 2.0,
        );
        left = left.min(apex.x);
        top = top.min(apex.y);
        right = right.max(apex.x);
        bottom = bottom.max(apex.y);
    }
    let (x, y) = (left - MARGIN, top - MARGIN);
    let (width, height) = (right - left + 2.0 * MARGIN, bottom - top + 2.0 * MARGIN);

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" \
         viewBox=\"{x:.1} {y:.1} {width:.1} {height:.1}\" \
         font-family=\"system-ui, -apple-system, 'Segoe UI', sans-serif\" \
         font-size=\"{FONT_SIZE:.0}\">"
    );
    let _ = writeln!(
        svg,
        "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{width:.1}\" height=\"{height:.1}\" fill=\"{}\"/>",
        theme.background
    );

    svg.push_str("<g fill=\"none\">\n");
    for edge in &snapshot.edges {
        let (Some(from), Some(to)) = (place_of(&edge.from), place_of(&edge.to)) else {
            continue;
        };
        let (from, to) = (at(from), at(to));
        let unresolved = matches!(edge.to, GraphKey::Unresolved(_));
        let dashes = match (edge.kind, unresolved) {
            (_, true) => Some("6 3 1.5 3"),
            (EdgeKind::Xref, _) => None,
            (EdgeKind::Include, _) => Some("6 4"),
            (EdgeKind::Nav, _) => Some("1.5 3"),
        };
        let (colour, opacity) = match edge.kind {
            EdgeKind::Nav => (theme.accent, 0.6),
            _ => (theme.muted, 0.55),
        };
        let line = if edge.kind == EdgeKind::Nav {
            let control = bow(from, to);
            format!(
                "M{:.1} {:.1} Q{:.1} {:.1} {:.1} {:.1}",
                from.x, from.y, control.x, control.y, to.x, to.y
            )
        } else {
            format!("M{:.1} {:.1} L{:.1} {:.1}", from.x, from.y, to.x, to.y)
        };
        let width = if edge.count > 1 { 2 } else { 1 };
        let _ = write!(
            svg,
            "<path class=\"edge {}\" d=\"{line}\" stroke=\"{colour}\" stroke-opacity=\"{opacity}\" \
             stroke-width=\"{width}\"",
            edge.kind.name()
        );
        if let Some(dashes) = dashes {
            let _ = write!(svg, " stroke-dasharray=\"{dashes}\"");
        }
        svg.push_str("/>\n");
    }
    svg.push_str("</g>\n<g stroke-width=\"2\">\n");

    for (place, node) in snapshot.nodes.iter().enumerate() {
        let (point, radius) = (at(place), radii[place]);
        let colour = palette
            .colour(node, style.dark)
            .map_or_else(|| theme.muted.to_owned(), |colour| colour.to_hex());
        let (kind, fill, dashes) = match node.kind {
            NodeKind::Page => ("page", colour.as_str(), ""),
            NodeKind::Nav => ("nav", colour.as_str(), ""),
            NodeKind::Partial => ("partial", "none", ""),
            NodeKind::Unresolved => ("unresolved", "none", " stroke-dasharray=\"3 2\""),
            NodeKind::Group => ("group", colour.as_str(), ""),
        };
        let title = escape(&node.title);
        if matches!(node.kind, NodeKind::Nav | NodeKind::Group) {
            let corner = if node.kind == NodeKind::Nav { 2 } else { 6 };
            let _ = writeln!(
                svg,
                "<rect class=\"node {kind}\" x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" \
                 height=\"{:.1}\" rx=\"{corner}\" fill=\"{fill}\" stroke=\"{colour}\"{dashes}>\
                 <title>{title}</title></rect>",
                point.x - radius,
                point.y - radius,
                radius * 2.0,
                radius * 2.0
            );
        } else {
            let _ = writeln!(
                svg,
                "<circle class=\"node {kind}\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"{radius:.1}\" \
                 fill=\"{fill}\" stroke=\"{colour}\"{dashes}><title>{title}</title></circle>",
                point.x, point.y
            );
        }
    }
    let _ = writeln!(svg, "</g>\n<g fill=\"{}\">", theme.text);
    for (place, node) in snapshot.nodes.iter().enumerate() {
        let (point, radius) = (at(place), radii[place]);
        let _ = writeln!(
            svg,
            "<text class=\"label\" x=\"{:.1}\" y=\"{:.1}\">{}</text>",
            point.x + radius + 3.0,
            point.y + FONT_SIZE * 0.35,
            escape(&node.title)
        );
    }
    svg.push_str("</g>\n</svg>\n");
    svg
}

/// The seed of a drawing laid out for export: the app's canvas's own, so both start alike.
pub const EXPORT_SEED: u64 = 7;

/// How many steps a drawing laid out for export may take to settle.
pub(crate) const EXPORT_STEPS: usize = 3000;

/// `snapshot` laid out afresh until it settles, then drawn as [`to_svg`] draws it, for a
/// drawing with no canvas to take positions from. The layout is seeded from the files' paths
/// relative to the folder holding them all, so the same files give the same bytes wherever
/// they are.
#[must_use]
pub fn to_svg_laid_out(snapshot: &GraphSnapshot, style: &SvgStyle) -> String {
    let mut layout = crate::Layout::new(EXPORT_SEED);
    layout.set_roots(common_folder(snapshot));
    layout.sync(snapshot);
    layout.settle(EXPORT_STEPS);
    let positions: Vec<Point> = layout.positions().map(|(_, at)| at).collect();
    to_svg(snapshot, &positions, style)
}

/// The deepest folder holding every file of `snapshot`, if it has a file.
fn common_folder(snapshot: &GraphSnapshot) -> Option<Arc<Path>> {
    let mut files = snapshot.nodes.iter().filter_map(|node| node.key.path());
    let mut folder = files.next()?.parent()?;
    for file in files {
        while !file.starts_with(folder) {
            folder = folder.parent()?;
        }
    }
    Some(Arc::from(folder))
}

/// `text` safe inside an element or a quoted attribute.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            // Characters XML 1.0 cannot hold at all.
            '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' => {}
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use super::*;
    use crate::{GraphEdge, GraphNode, NodeCoordinate};

    fn file(name: &str) -> GraphKey {
        GraphKey::File(Arc::from(Path::new(name)))
    }

    fn node(key: GraphKey, title: &str, kind: NodeKind, links: usize) -> GraphNode {
        GraphNode {
            key,
            title: title.to_owned(),
            kind,
            coordinate: Some(NodeCoordinate {
                component: "guide".to_owned(),
                version: None,
                module: "ROOT".to_owned(),
                family: "page".to_owned(),
            }),
            links_out: links,
            links_in: 0,
            last_changed: None,
        }
    }

    fn snapshot() -> GraphSnapshot {
        let mut nodes = vec![
            node(file("/a.adoc"), "Home & <away>", NodeKind::Page, 3),
            node(file("/b.adoc"), "Install", NodeKind::Page, 1),
            node(file("/c.adoc"), "Note", NodeKind::Partial, 0),
            node(file("/nav.adoc"), "nav.adoc", NodeKind::Nav, 1),
            node(
                GraphKey::Unresolved("gone.adoc".to_owned()),
                "\"gone\" isn't",
                NodeKind::Unresolved,
                0,
            ),
        ];
        nodes.sort_by(|left, right| left.key.cmp(&right.key));
        let edge = |from: GraphKey, to: GraphKey, kind, count| GraphEdge {
            from,
            to,
            kind,
            count,
        };
        GraphSnapshot {
            revision: 1,
            nodes,
            edges: vec![
                edge(file("/a.adoc"), file("/b.adoc"), EdgeKind::Xref, 2),
                edge(file("/a.adoc"), file("/c.adoc"), EdgeKind::Include, 1),
                edge(file("/nav.adoc"), file("/a.adoc"), EdgeKind::Nav, 1),
                edge(
                    file("/a.adoc"),
                    GraphKey::Unresolved("gone.adoc".to_owned()),
                    EdgeKind::Xref,
                    1,
                ),
            ],
        }
    }

    fn positions(snapshot: &GraphSnapshot) -> Vec<Point> {
        (0..snapshot.nodes.len())
            .map(|place| Point::new(place as f32 * 50.0, (place % 2) as f32 * 40.0))
            .collect()
    }

    #[test]
    fn graph_svg_draws_every_node_and_edge() {
        let snapshot = snapshot();
        let svg = to_svg(&snapshot, &positions(&snapshot), &SvgStyle::default());
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.trim_end().ends_with("</svg>"));
        for (class, count) in [
            ("node page", 2),
            ("node partial", 1),
            ("node nav", 1),
            ("node unresolved", 1),
            ("edge xref", 2),
            ("edge include", 1),
            ("edge nav", 1),
            ("label", 5),
        ] {
            assert_eq!(
                svg.matches(&format!("class=\"{class}\"")).count(),
                count,
                "{class}"
            );
        }
        // Each kind keeps its look: heavier for two links, dashes by kind.
        let line_of = |class: &str| {
            svg.lines()
                .filter(|line| line.contains(&format!("class=\"{class}\"")))
                .collect::<Vec<_>>()
        };
        let xrefs = line_of("edge xref");
        assert!(xrefs
            .iter()
            .any(|line| line.contains("stroke-width=\"2\"") && !line.contains("dasharray")));
        assert!(xrefs.iter().any(|line| line.contains("6 3 1.5 3")));
        assert!(line_of("edge include")[0].contains("stroke-dasharray=\"6 4\""));
        assert!(line_of("edge nav")[0].contains(" Q"));
        assert!(line_of("node partial")[0].contains("fill=\"none\""));
        assert!(line_of("node unresolved")[0].contains("stroke-dasharray=\"3 2\""));
        assert!(line_of("node nav")[0].starts_with("<rect"));
    }

    /// The `viewBox` of `svg`: left, top, right and bottom.
    fn view_box(svg: &str) -> (f32, f32, f32, f32) {
        let start = svg.find("viewBox=\"").unwrap() + "viewBox=\"".len();
        let end = start + svg[start..].find('"').unwrap();
        let numbers: Vec<f32> = svg[start..end]
            .split(' ')
            .map(|number| number.parse().unwrap())
            .collect();
        let [x, y, width, height] = numbers[..] else {
            panic!("{numbers:?}");
        };
        (x, y, x + width, y + height)
    }

    #[test]
    fn a_label_in_wide_glyphs_fits_the_image() {
        let title = "インストールと設定の手引き";
        let snapshot = GraphSnapshot {
            revision: 1,
            nodes: vec![node(file("/a.adoc"), title, NodeKind::Page, 0)],
            edges: Vec::new(),
        };
        let svg = to_svg(&snapshot, &[Point::new(0.0, 0.0)], &SvgStyle::default());
        let (_, _, right, _) = view_box(&svg);
        // The label starts beside the node, and each character is an em wide.
        let start = node_radius(0) + 3.0;
        assert!(
            right >= start + title.chars().count() as f32 * FONT_SIZE,
            "{right}"
        );
    }

    #[test]
    fn a_long_navigation_edge_lies_inside_the_image() {
        let snapshot = GraphSnapshot {
            revision: 1,
            nodes: vec![
                node(file("/a.adoc"), "A", NodeKind::Page, 1),
                node(file("/nav.adoc"), "N", NodeKind::Nav, 1),
            ],
            edges: vec![GraphEdge {
                from: file("/nav.adoc"),
                to: file("/a.adoc"),
                kind: EdgeKind::Nav,
                count: 1,
            }],
        };
        // Both on the image's edge, 600 apart: the curve bows 45 off the line between them, out
        // of the image the nodes alone would make.
        let positions = [Point::new(600.0, 0.0), Point::new(0.0, 0.0)];
        let svg = to_svg(&snapshot, &positions, &SvgStyle::default());
        let (left, top, right, bottom) = view_box(&svg);
        let apex = Point::new(300.0, 45.0);
        assert!(
            left <= apex.x && apex.x <= right && top <= apex.y && apex.y <= bottom,
            "({left}, {top}, {right}, {bottom})"
        );
    }

    #[test]
    fn a_layout_for_export_is_the_same_wherever_the_files_are() {
        let moved = |prefix: &str| {
            let mut snapshot = snapshot();
            let key = |key: &GraphKey| match key {
                GraphKey::File(path) => GraphKey::File(Arc::from(
                    Path::new(prefix).join(path.strip_prefix("/").unwrap()),
                )),
                other => other.clone(),
            };
            for node in &mut snapshot.nodes {
                node.key = key(&node.key);
            }
            for edge in &mut snapshot.edges {
                (edge.from, edge.to) = (key(&edge.from), key(&edge.to));
            }
            snapshot
        };
        let style = SvgStyle::default();
        assert_eq!(
            to_svg_laid_out(&moved("/home/a/docs"), &style),
            to_svg_laid_out(&moved("/builds/ci/project/docs"), &style)
        );
    }

    #[test]
    fn graph_svg_escapes_titles() {
        let snapshot = snapshot();
        let svg = to_svg(&snapshot, &positions(&snapshot), &SvgStyle::default());
        assert!(svg.contains(">Home &amp; &lt;away&gt;</text>"));
        assert!(svg.contains(">&quot;gone&quot; isn&apos;t</text>"));
        assert!(!svg.contains("<away>"));
        // No absolute paths: only titles are written.
        assert!(!svg.contains("/a.adoc"));
    }

    #[test]
    fn graph_svg_is_deterministic() {
        let snapshot = snapshot();
        let positions = positions(&snapshot);
        let style = SvgStyle {
            colour_by: ColourBy::Module,
            dark: true,
            ..SvgStyle::default()
        };
        assert_eq!(
            to_svg(&snapshot, &positions, &style),
            to_svg(&snapshot, &positions, &style)
        );
        assert_ne!(
            to_svg(&snapshot, &positions, &style),
            to_svg(&snapshot, &positions, &SvgStyle::default())
        );
    }
}
