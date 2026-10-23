//! Running content and the title page: the theme's `header_*` and `footer_*` content written as
//! Typst functions of the page they are drawn on, and the title page's parts.

use std::{collections::BTreeMap, fmt::Write as _, path::Path, sync::OnceLock};

use regex::Regex;

use crate::{
    theme::{LoadedTheme, ThemeValue},
    typst::{self, string, text},
};

/// Attributes running content can name that change from page to page.
const DYNAMIC: &[&str] = &[
    "page-number",
    "page-count",
    "part-title",
    "chapter-title",
    "section-title",
    "section-or-chapter-title",
];

const POSITIONS: [&str; 3] = ["left", "center", "right"];

/// The `running` dictionary: for `header` and `footer`, for `recto` and `verso`, each column's
/// place (`x` and `width` as fractions of the content width, `align`) and a function from the
/// page's values (`ctx`) to its content.
pub(crate) fn running_dictionary(
    theme: &LoadedTheme,
    attributes: &BTreeMap<String, String>,
    files: &mut dyn FnMut(&Path) -> String,
) -> String {
    let keys = &theme.keys;
    let mut out = String::from("#let running = (\n");
    for periphery in ["header", "footer"] {
        if keys
            .get(&format!("{periphery}_height"))
            .is_none_or(ThemeValue::is_null)
        {
            continue;
        }
        let _ = writeln!(out, "  {periphery}: (");
        for side in ["recto", "verso"] {
            let columns = keys
                .get(&format!("{periphery}_{side}_columns"))
                .or_else(|| keys.get(&format!("{periphery}_columns")))
                .filter(|value| !value.is_null())
                .map(ToString::to_string);
            let places = column_places(columns.as_deref());
            let _ = writeln!(out, "    {side}: (");
            for (position, (x, width, align)) in POSITIONS.iter().zip(places) {
                let key = format!("{periphery}_{side}_{position}_content");
                let Some(content) = keys
                    .get(&key)
                    .map(ToString::to_string)
                    .filter(|content| !content.trim().is_empty())
                else {
                    continue;
                };
                let body = content_markup(&content, attributes, theme.dir.as_deref(), files);
                let _ = writeln!(
                    out,
                    "      {position}: (x: {}, width: {}, align: {}, body: ctx => [{body}]),",
                    typst::number(x),
                    typst::number(width),
                    string(align),
                );
            }
            out.push_str("    ),\n");
        }
        out.push_str("  ),\n");
    }
    out.push_str(")\n");
    out
}

/// Where the three columns sit, from a `columns` spec such as `<40% =20% >40%`; without one,
/// each spans the whole width with its own alignment, as in asciidoctor-pdf.
fn column_places(spec: Option<&str>) -> [(f64, f64, &'static str); 3] {
    let Some(spec) = spec else {
        return [
            (0.0, 1.0, "left"),
            (0.0, 1.0, "center"),
            (0.0, 1.0, "right"),
        ];
    };
    let specs: Vec<String> = spec
        .replace(',', " ")
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let specs: [String; 3] = match specs.as_slice() {
        [] => ["0".into(), "100".into(), "0".into()],
        [center] => ["0".into(), center.clone(), "0".into()],
        [left, right] => [left.clone(), "0".into(), right.clone()],
        [left, center, right, ..] => [left.clone(), center.clone(), right.clone()],
    };
    let parsed: Vec<(f64, &'static str)> = specs
        .iter()
        .map(|spec| {
            let (align, rest) = match spec.chars().next() {
                Some('<') => ("left", &spec[1..]),
                Some('=') => ("center", &spec[1..]),
                Some('>') => ("right", &spec[1..]),
                _ => ("left", spec.as_str()),
            };
            (
                crate::theme::value_number(rest.trim_end_matches('%')),
                align,
            )
        })
        .collect();
    let total: f64 = parsed
        .iter()
        .map(|(width, _)| width)
        .sum::<f64>()
        .max(f64::EPSILON);
    let widths: Vec<f64> = parsed.iter().map(|(width, _)| width / total).collect();
    [
        (0.0, widths[0], parsed[0].1),
        (widths[0], widths[1], parsed[1].1),
        (widths[0] + widths[1], widths[2], parsed[2].1),
    ]
}

/// Running content as Typst markup: attribute references substituted (those that change per
/// page read `ctx`), constrained bold, italic and monospace formatting, character references,
/// and an `image:` macro standing alone. A line naming an attribute that is not set is
/// dropped, as asciidoctor-pdf drops it.
pub(crate) fn content_markup(
    content: &str,
    attributes: &BTreeMap<String, String>,
    theme_dir: Option<&Path>,
    files: &mut dyn FnMut(&Path) -> String,
) -> String {
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    let image =
        IMAGE.get_or_init(|| Regex::new(r"^image:(?:\{[^}]+\}/)?([^\[\s]+)\[([^\]]*)\]$").unwrap());
    let trimmed = content.trim();
    if let Some(captures) = image.captures(trimmed) {
        let target = substitute_static(&captures[1], attributes);
        let path =
            theme_dir.map_or_else(|| Path::new(&target).to_path_buf(), |dir| dir.join(&target));
        if path.is_file() {
            let width = captures[2]
                .split(',')
                .find_map(|attribute| attribute.trim().strip_prefix("width="))
                .and_then(|width| {
                    let width = width.trim_matches('"');
                    width
                        .strip_suffix('%')
                        .map(|percent| format!("{percent}%"))
                        .or_else(|| width.parse::<f64>().ok().map(|px| typst::points(px * 0.75)))
                })
                .map(|width| format!(", width: {width}"))
                .unwrap_or_default();
            return format!("#image({}{width})", string(&files(&path)));
        }
    }
    let mut lines = Vec::new();
    for line in content.split('\n') {
        if let Some(markup) = line_markup(line, attributes) {
            lines.push(markup);
        }
    }
    lines.join("#linebreak()")
}

/// Static attribute references replaced; unknown ones left as they are.
fn substitute_static(text: &str, attributes: &BTreeMap<String, String>) -> String {
    reference_regex()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            attributes
                .get(&captures[1])
                .cloned()
                .unwrap_or_else(|| captures[0].to_owned())
        })
        .into_owned()
}

fn reference_regex() -> &'static Regex {
    static REFERENCE: OnceLock<Regex> = OnceLock::new();
    REFERENCE.get_or_init(|| Regex::new(r"\{([A-Za-z0-9_][A-Za-z0-9_-]*)\}").unwrap())
}

/// Placeholders for page values in text being formatted: private-use characters that never
/// occur in a theme.
const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

fn line_markup(line: &str, attributes: &BTreeMap<String, String>) -> Option<String> {
    let mut resolved = String::new();
    let mut last = 0;
    for captures in reference_regex().captures_iter(line) {
        let whole = captures.get(0).unwrap();
        let name = &captures[1];
        resolved.push_str(&line[last..whole.start()]);
        if DYNAMIC.contains(&name) {
            resolved.push(OPEN);
            resolved.push_str(name);
            resolved.push(CLOSE);
        } else {
            // An attribute that is not set drops the line.
            resolved.push_str(&static_attribute(name, attributes)?);
        }
        last = whole.end();
    }
    resolved.push_str(&line[last..]);
    Some(format_markup(&character_references(&resolved)))
}

fn static_attribute(name: &str, attributes: &BTreeMap<String, String>) -> Option<String> {
    if let Some(value) = attributes.get(name) {
        return Some(value.clone());
    }
    let title = || attributes.get("doctitle").cloned();
    match name {
        "document-title" => title().map(|title| partition_title(&title).0),
        "document-subtitle" => Some(
            title()
                .and_then(|title| partition_title(&title).1)
                .unwrap_or_default(),
        ),
        "doctitle" => title(),
        "page-layout" => Some("portrait".to_owned()),
        _ => None,
    }
}

/// The title before its last `: ` and the subtitle after it, as Asciidoctor partitions a
/// document title.
pub(crate) fn partition_title(title: &str) -> (String, Option<String>) {
    match title.rsplit_once(": ") {
        Some((main, subtitle)) => (main.to_owned(), Some(subtitle.to_owned())),
        None => (title.to_owned(), None),
    }
}

/// `&#169;` and `&#xA9;` as the characters they name.
fn character_references(text: &str) -> String {
    static REFERENCE: OnceLock<Regex> = OnceLock::new();
    let reference = REFERENCE.get_or_init(|| Regex::new(r"&#(x[0-9A-Fa-f]+|[0-9]+);").unwrap());
    reference
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let code = &captures[1];
            let value = match code.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => code.parse().ok(),
            };
            value
                .and_then(char::from_u32)
                .map_or_else(|| captures[0].to_owned(), String::from)
        })
        .into_owned()
}

/// Constrained `*bold*`, `_italic_` and `` `monospace` `` as Typst, with page values read from
/// `ctx` and everything else as text.
fn format_markup(text: &str) -> String {
    static MARKS: OnceLock<[Regex; 3]> = OnceLock::new();
    let marks = MARKS.get_or_init(|| {
        [
            Regex::new(r"(?:^|[^\w])(\*(\S|\S.*?\S)\*)(?:$|[^\w])").unwrap(),
            Regex::new(r"(?:^|[^\w])(_(\S|\S.*?\S)_)(?:$|[^\w])").unwrap(),
            Regex::new(r"(?:^|[^\w])(`(\S|\S.*?\S)`)(?:$|[^\w])").unwrap(),
        ]
    });
    let earliest = marks
        .iter()
        .enumerate()
        .filter_map(|(kind, regex)| {
            regex.captures(text).map(|captures| {
                let whole = captures.get(1).unwrap();
                let inner = captures.get(2).unwrap();
                (whole.start(), kind, whole.range(), inner.range())
            })
        })
        .min_by_key(|(start, ..)| *start);
    let Some((_, kind, whole, inner)) = earliest else {
        return plain(text);
    };
    let wrap = ["#strong", "#emph", "#adoc-codespan"][kind];
    format!(
        "{}{wrap}[{}]{}",
        plain(&text[..whole.start]),
        format_markup(&text[inner]),
        format_markup(&text[whole.end..])
    )
}

/// Text with page-value placeholders read from `ctx`.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find(OPEN) {
        out.push_str(&typst::text(&rest[..open]));
        let after = &rest[open + OPEN.len_utf8()..];
        let close = after.find(CLOSE).unwrap_or(after.len());
        let _ = write!(out, "#ctx.at({})", string(&after[..close]));
        rest = after.get(close + CLOSE.len_utf8()..).unwrap_or("");
    }
    out.push_str(&text_or_empty(rest));
    out
}

fn text_or_empty(value: &str) -> String {
    text(value)
}

/// The title page's parts, as a call to the template's `adoc-title-page`.
pub(crate) struct TitlePage<'a> {
    pub title: String,
    pub subtitle: Option<String>,
    pub attributes: &'a BTreeMap<String, String>,
}

impl TitlePage<'_> {
    pub(crate) fn markup(
        &self,
        theme: &LoadedTheme,
        files: &mut dyn FnMut(&Path) -> String,
    ) -> String {
        let attributes = self.attributes;
        let authors = attributes
            .get("authors")
            .or_else(|| attributes.get("author"))
            .map(|authors| {
                let delimiter = theme
                    .keys
                    .get("title_page_authors_delimiter")
                    .map_or_else(|| ", ".to_owned(), ToString::to_string);
                authors
                    .split(',')
                    .map(str::trim)
                    .filter(|author| !author.is_empty())
                    .collect::<Vec<_>>()
                    .join(&delimiter)
            });
        let revision = {
            let delimiter = theme
                .keys
                .get("title_page_revision_delimiter")
                .map_or_else(|| ", ".to_owned(), ToString::to_string);
            let label = attributes
                .get("version-label")
                .map_or("Version", String::as_str);
            let parts: Vec<String> = [
                attributes
                    .get("revnumber")
                    .map(|number| format!("{label} {}", number.trim_start_matches(['v', 'V']))),
                attributes.get("revdate").cloned(),
            ]
            .into_iter()
            .flatten()
            .collect();
            (!parts.is_empty()).then(|| {
                let mut revision = parts.join(&delimiter);
                if let Some(remark) = attributes.get("revremark") {
                    let _ = write!(revision, ": {remark}");
                }
                revision
            })
        };
        let logo = attributes
            .get("title-logo-image")
            .map(|value| (value.clone(), false))
            .or_else(|| {
                theme
                    .keys
                    .get("title_page_logo_image")
                    .filter(|value| !value.is_null())
                    .map(|value| (value.to_string(), true))
            })
            .and_then(|(value, from_theme)| {
                let (target, attributes) = match value.strip_prefix("image:") {
                    Some(macro_text) => {
                        let (target, rest) =
                            macro_text.split_once('[').unwrap_or((macro_text, "]"));
                        (
                            target.trim_start_matches(':').to_owned(),
                            rest.trim_end_matches(']').to_owned(),
                        )
                    }
                    None => (value, String::new()),
                };
                let base = if from_theme {
                    theme.dir.clone()
                } else {
                    self.attributes.get("docdir").map(Into::into)
                };
                let path =
                    base.map_or_else(|| Path::new(&target).to_path_buf(), |dir| dir.join(&target));
                path.is_file().then(|| {
                    let width = attributes
                        .split(',')
                        .find_map(|attribute| attribute.trim().strip_prefix("width="))
                        .and_then(|width| {
                            width.strip_suffix('%').map(|percent| format!("{percent}%"))
                        })
                        .map(|width| format!(", width: {width}"))
                        .unwrap_or_default();
                    format!("image({}{width})", string(&files(&path)))
                })
            });
        let optional = |value: Option<String>| {
            value.map_or_else(|| "none".to_owned(), |value| format!("[{}]", text(&value)))
        };
        format!(
            "#adoc-title-page(title: [{}], subtitle: {}, authors: {}, revision: {}, logo: {})\n\n",
            text(&self.title),
            optional(self.subtitle.clone()),
            optional(authors),
            optional(revision),
            logo.unwrap_or_else(|| "none".to_owned()),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{column_places, content_markup, partition_title};

    fn markup(content: &str) -> String {
        let attributes = BTreeMap::from([
            ("doctitle".to_owned(), "Guide: The Subtitle".to_owned()),
            ("revnumber".to_owned(), "1.2".to_owned()),
        ]);
        content_markup(content, &attributes, None, &mut |_| String::new())
    }

    #[test]
    fn page_values_come_from_ctx() {
        assert_eq!(markup("{page-number}"), "#ctx.at(\"page-number\")");
        assert_eq!(
            markup("{section-or-chapter-title} | *{page-number}*"),
            "#ctx.at(\"section-or-chapter-title\")#\" | \"#strong[#ctx.at(\"page-number\")]"
        );
    }

    #[test]
    fn static_attributes_are_substituted_and_unknown_lines_dropped() {
        assert_eq!(markup("{document-title} v{revnumber}"), "#\"Guide v1.2\"");
        assert_eq!(markup("{nowhere}\n_draft_"), "#emph[#\"draft\"]");
        assert_eq!(markup("&#169; 2026"), "#\"\u{a9} 2026\"");
    }

    #[test]
    fn columns_follow_the_spec() {
        let places = column_places(Some("<40% =20% >40%"));
        assert_eq!(places[1], (0.4, 0.2, "center"));
        assert_eq!(places[2].2, "right");
        assert_eq!(column_places(None)[0], (0.0, 1.0, "left"));
    }

    #[test]
    fn titles_partition_at_the_last_colon() {
        assert_eq!(
            partition_title("A: B: C"),
            ("A: B".to_owned(), Some("C".to_owned()))
        );
    }
}
