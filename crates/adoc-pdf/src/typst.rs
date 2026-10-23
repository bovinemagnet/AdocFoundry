//! Typst literals: strings, lengths, colours and theme values, written so that nothing taken
//! from a document is ever read as Typst code.

use std::fmt::Write as _;

use crate::theme::{Color, ThemeValue};

/// `text` as a Typst string literal.
pub(crate) fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Text in markup: a string literal behind `#`, which Typst shows verbatim.
pub(crate) fn text(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!("#{}", string(text))
    }
}

/// A number as Typst reads it.
pub(crate) fn number(value: f64) -> String {
    if value.is_finite() {
        let text = format!("{value}");
        if text.contains(['.', 'e', 'E']) {
            text
        } else {
            format!("{text}.0")
        }
    } else {
        "0.0".to_owned()
    }
}

pub(crate) fn points(value: f64) -> String {
    format!("{}pt", trim(value))
}

fn trim(value: f64) -> String {
    if value.is_finite() {
        let text = format!("{value:.4}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        if text.is_empty() || text == "-" {
            "0".to_owned()
        } else {
            text.to_owned()
        }
    } else {
        "0".to_owned()
    }
}

pub(crate) fn color(color: &Color) -> String {
    match color {
        Color::Rgb([r, g, b]) => format!("rgb({r}, {g}, {b})"),
        Color::Cmyk([c, m, y, k]) => format!(
            "cmyk({}%, {}%, {}%, {}%)",
            trim(*c),
            trim(*m),
            trim(*y),
            trim(*k)
        ),
        Color::Transparent => "none".to_owned(),
    }
}

/// A relative length written as text in a theme (`1.2em`, `10%`), or `None`.
pub(crate) fn relative_length(text: &str) -> Option<String> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))?;
    let (number, unit) = text.split_at(split);
    let value: f64 = number.parse().ok()?;
    match unit {
        "em" | "rem" => Some(format!("{}em", trim(value))),
        "%" => Some(format!("{}%", trim(value))),
        _ => None,
    }
}

/// Whether a theme key holds a length, so that its numbers are points.
pub(crate) fn is_length_key(key: &str) -> bool {
    const SUFFIXES: &[&str] = &[
        "_size",
        "_width",
        "_height",
        "_margin",
        "_margin_top",
        "_margin_bottom",
        "_margin_inner",
        "_margin_outer",
        "_margin_inside",
        "_margin_outside",
        "_margin_page_top",
        "_padding",
        "_indent",
        "_spacing",
        "_gap",
        "_radius",
        "_offset",
        "_top",
        "_left",
        "_right",
        "_bottom",
        "_length",
        "_min_height_after",
        "_rhythm",
        "_after_code",
        "_line_gap",
    ];
    if key.ends_with("line_height") || key == "page_size" {
        return false;
    }
    key.contains("font_size") || SUFFIXES.iter().any(|suffix| key.ends_with(suffix))
}

/// A theme value as a Typst value; numbers under length keys become points.
pub(crate) fn theme_value(key: &str, value: &ThemeValue) -> String {
    let length = is_length_key(key);
    match value {
        ThemeValue::Null => "none".to_owned(),
        ThemeValue::Bool(value) => value.to_string(),
        ThemeValue::Number(value) if length => points(*value),
        ThemeValue::Number(value) => number(*value),
        ThemeValue::Text(text) if length => relative_length(text).unwrap_or_else(|| string(text)),
        ThemeValue::Text(text) => string(text),
        ThemeValue::Color(value) => color(value),
        ThemeValue::List(items) => {
            let items: Vec<String> = items.iter().map(|item| theme_value(key, item)).collect();
            format!("({},)", items.join(", "))
        }
        ThemeValue::Map(entries) => {
            let entries: Vec<String> = entries
                .iter()
                .map(|(name, item)| format!("{}: {}", string(name), theme_value(name, item)))
                .collect();
            if entries.is_empty() {
                "(:)".to_owned()
            } else {
                format!("({})", entries.join(", "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_length_key, relative_length, string, theme_value};
    use crate::theme::{Color, ThemeValue};

    #[test]
    fn strings_escape_only_what_typst_strings_need() {
        assert_eq!(string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(string("#*_`$<>@[]~=-+/"), "\"#*_`$<>@[]~=-+/\"");
        assert_eq!(string("line\nnext\u{7}"), "\"line\\nnext\\u{7}\"");
    }

    #[test]
    fn theme_values_become_typst_values() {
        assert_eq!(
            theme_value("base_font_size", &ThemeValue::Number(10.5)),
            "10.5pt"
        );
        assert_eq!(
            theme_value("base_line_height", &ThemeValue::Number(1.2)),
            "1.2"
        );
        assert_eq!(
            theme_value("role_big_font_size", &ThemeValue::Text("1.2em".into())),
            "1.2em"
        );
        assert_eq!(
            theme_value("title_page_title_top", &ThemeValue::Text("55%".into())),
            "55%"
        );
        assert_eq!(
            theme_value("page_size", &ThemeValue::Text("A4".into())),
            "\"A4\""
        );
        assert_eq!(
            theme_value(
                "base_font_color",
                &ThemeValue::Color(Color::Rgb([51, 51, 51]))
            ),
            "rgb(51, 51, 51)"
        );
        assert_eq!(
            theme_value(
                "admonition_padding",
                &ThemeValue::List(vec![ThemeValue::Number(4.0), ThemeValue::Number(12.0)])
            ),
            "(4pt, 12pt,)"
        );
        assert!(is_length_key("heading_min_height_after"));
        assert!(!is_length_key("heading_line_height"));
        assert_eq!(relative_length("0.8em"), Some("0.8em".into()));
        assert_eq!(relative_length("FitH"), None);
    }
}
