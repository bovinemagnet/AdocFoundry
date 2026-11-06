//! Admonition icons, as asciidoctor-pdf draws them with `icons=font`: a Font Awesome glyph for
//! each kind, named, sized and coloured by the theme's `admonition_icon_<kind>` keys over
//! asciidoctor-pdf's defaults. The glyphs come from the bundled subsets of Font Awesome Free
//! (`fonts/fontawesome-*-subset.ttf`, made by `fonts/fontawesome-subset.py`, which lists the
//! same names); a name outside them falls back to the kind's default glyph.

use crate::theme::{Color, LoadedTheme, ThemeValue};

/// The family the solid subset is registered under.
pub(crate) const SOLID_FAMILY: &str = "Font Awesome 5 Free Solid";
/// The family the regular subset is registered under.
pub(crate) const REGULAR_FAMILY: &str = "Font Awesome 5 Free Regular";

/// The glyphs of the solid subset (`fas-<name>`), by name.
const SOLID: &[(&str, char)] = &[
    ("ban", '\u{f05e}'),
    ("bell", '\u{f0f3}'),
    ("bolt", '\u{f0e7}'),
    ("bug", '\u{f188}'),
    ("check", '\u{f00c}'),
    ("check-circle", '\u{f058}'),
    ("comment", '\u{f075}'),
    ("comment-alt", '\u{f27a}'),
    ("exclamation", '\u{f12a}'),
    ("exclamation-circle", '\u{f06a}'),
    ("exclamation-triangle", '\u{f071}'),
    ("fire", '\u{f06d}'),
    ("fire-alt", '\u{f7e4}'),
    ("flag", '\u{f024}'),
    ("hand-paper", '\u{f256}'),
    ("heart", '\u{f004}'),
    ("info", '\u{f129}'),
    ("info-circle", '\u{f05a}'),
    ("lightbulb", '\u{f0eb}'),
    ("question", '\u{f128}'),
    ("question-circle", '\u{f059}'),
    ("star", '\u{f005}'),
    ("sticky-note", '\u{f249}'),
    ("stop-circle", '\u{f28d}'),
    ("thumbtack", '\u{f08d}'),
    ("times-circle", '\u{f057}'),
];

/// The glyphs of the regular subset (`far-<name>`), by name.
const REGULAR: &[(&str, char)] = &[
    ("bell", '\u{f0f3}'),
    ("check-circle", '\u{f058}'),
    ("comment", '\u{f075}'),
    ("comment-alt", '\u{f27a}'),
    ("flag", '\u{f024}'),
    ("hand-paper", '\u{f256}'),
    ("heart", '\u{f004}'),
    ("lightbulb", '\u{f0eb}'),
    ("question-circle", '\u{f059}'),
    ("star", '\u{f005}'),
    ("sticky-note", '\u{f249}'),
    ("stop-circle", '\u{f28d}'),
    ("times-circle", '\u{f057}'),
];

/// Font Awesome 4 names whose Font Awesome 5 glyph is not `fas-` and the same name, from
/// prawn-icon's `fa4/shims.yml`, for the glyphs bundled.
const SHIMS: &[(&str, &str)] = &[
    ("fa-bell-o", "far-bell"),
    ("fa-check-circle-o", "far-check-circle"),
    ("fa-comment-o", "far-comment"),
    ("fa-flag-o", "far-flag"),
    ("fa-hand-paper-o", "far-hand-paper"),
    ("fa-heart-o", "far-heart"),
    ("fa-lightbulb-o", "far-lightbulb"),
    ("fa-question-circle-o", "far-question-circle"),
    ("fa-star-o", "far-star"),
    ("fa-sticky-note-o", "far-sticky-note"),
    ("fa-stop-circle-o", "far-stop-circle"),
    ("fa-thumb-tack", "fas-thumbtack"),
    ("fa-times-circle-o", "far-times-circle"),
    ("fa-warning", "fas-exclamation-triangle"),
];

/// asciidoctor-pdf's `AdmonitionIcons`: each kind's icon, colour and size.
fn default_icon(kind: &str) -> (&'static str, [u8; 3]) {
    match kind {
        "tip" => ("far-lightbulb", [0x11, 0x11, 0x11]),
        "warning" => ("fas-exclamation-triangle", [0xBF, 0x69, 0x00]),
        "caution" => ("fas-fire", [0xBF, 0x34, 0x00]),
        "important" => ("fas-exclamation-circle", [0xBF, 0x00, 0x00]),
        _ => ("fas-info-circle", [0x19, 0x40, 0x7C]),
    }
}

const DEFAULT_SIZE: f64 = 24.0;

/// The kinds an admonition can be, as the template names them.
pub(crate) const KINDS: [&str; 5] = ["note", "tip", "important", "caution", "warning"];

/// The bundled glyph a theme's icon name stands for: `fas-`, `far-`, or a Font Awesome 4 name
/// (`fa-name` or a bare `name`), which asciidoctor-pdf reads through prawn-icon's shims.
pub(crate) fn glyph(name: &str) -> Option<(&'static str, char)> {
    let name = name.trim();
    if let Some(name) = name.strip_prefix("fas-") {
        return find(SOLID, SOLID_FAMILY, name);
    }
    if let Some(name) = name.strip_prefix("far-") {
        return find(REGULAR, REGULAR_FAMILY, name);
    }
    let legacy = name.strip_prefix("fa-").unwrap_or(name);
    match SHIMS
        .iter()
        .find(|(old, _)| old.strip_prefix("fa-") == Some(legacy))
    {
        Some((_, new)) => glyph(new),
        None => find(SOLID, SOLID_FAMILY, legacy),
    }
}

fn find(set: &[(&str, char)], family: &'static str, name: &str) -> Option<(&'static str, char)> {
    set.iter()
        .find(|(glyph, _)| *glyph == name)
        .map(|(_, code)| (family, *code))
}

/// One admonition kind's icon.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AdmonitionIcon {
    pub family: &'static str,
    pub glyph: char,
    /// In points.
    pub size: f64,
    /// `None` draws it in the surrounding text's colour.
    pub color: Option<Color>,
}

/// The icon `theme` gives admonitions of `kind`: its `admonition_icon_<kind>` keys merged over
/// the defaults, as asciidoctor-pdf's `admonition_icon_data` merges them.
pub(crate) fn admonition_icon(theme: &LoadedTheme, kind: &str) -> AdmonitionIcon {
    let (default_name, default_color) = default_icon(kind);
    let fallback = glyph(default_name).expect("every default glyph is bundled");
    let mut icon = AdmonitionIcon {
        family: fallback.0,
        glyph: fallback.1,
        size: DEFAULT_SIZE,
        color: Some(Color::Rgb(default_color)),
    };
    let Some(ThemeValue::Map(entries)) = theme.keys.get(&format!("admonition_icon_{kind}")) else {
        return icon;
    };
    for (key, value) in entries {
        match key.as_str() {
            "name" => {
                let named = value.as_text().and_then(glyph);
                (icon.family, icon.glyph) = named.unwrap_or(fallback);
            }
            "size" => {
                if let Some(size) = value.as_number().filter(|size| *size > 0.0) {
                    icon.size = size;
                }
            }
            "stroke_color" => {
                icon.color = match value {
                    ThemeValue::Color(Color::Transparent) | ThemeValue::Null => None,
                    ThemeValue::Color(color) => Some(color.clone()),
                    _ => icon.color.take(),
                };
            }
            _ => {}
        }
    }
    icon
}

#[cfg(test)]
mod tests {
    use super::{glyph, REGULAR, REGULAR_FAMILY, SOLID, SOLID_FAMILY};

    #[test]
    fn names_resolve_as_prawn_icon_resolves_them() {
        assert_eq!(glyph("fas-info-circle"), Some((SOLID_FAMILY, '\u{f05a}')));
        assert_eq!(glyph("far-lightbulb"), Some((REGULAR_FAMILY, '\u{f0eb}')));
        assert_eq!(glyph("fas-lightbulb"), Some((SOLID_FAMILY, '\u{f0eb}')));
        // Font Awesome 4 names, with and without their prefix.
        assert_eq!(glyph("fa-lightbulb-o"), Some((REGULAR_FAMILY, '\u{f0eb}')));
        assert_eq!(glyph("lightbulb-o"), Some((REGULAR_FAMILY, '\u{f0eb}')));
        assert_eq!(glyph("fa-info-circle"), Some((SOLID_FAMILY, '\u{f05a}')));
        assert_eq!(glyph("info-circle"), Some((SOLID_FAMILY, '\u{f05a}')));
        assert_eq!(glyph("fa-warning"), Some((SOLID_FAMILY, '\u{f071}')));
        // Not bundled.
        assert_eq!(glyph("fas-rocket"), None);
        assert_eq!(glyph("fab-github"), None);
        assert_eq!(glyph("far-info-circle"), None);
        assert_eq!(glyph(""), None);
    }

    #[test]
    fn every_glyph_is_in_its_subset() {
        for (data, set) in [
            (
                &include_bytes!("../fonts/fontawesome-solid-subset.ttf")[..],
                SOLID,
            ),
            (
                &include_bytes!("../fonts/fontawesome-regular-subset.ttf")[..],
                REGULAR,
            ),
        ] {
            let font = typst::text::Font::new(typst::foundations::Bytes::new(data), 0).unwrap();
            for (name, code) in set {
                assert!(
                    font.info().coverage.contains(u32::from(*code)),
                    "{name} is not in the subset"
                );
            }
        }
    }
}
