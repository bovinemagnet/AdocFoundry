//! Attribute values at the end of their lines in the source editor (#87).
//!
//! The editor widget cannot put text between characters, so each line that references
//! attributes has their values drawn, faded, after its last character, from the editor's own
//! layout once it has laid out the frame, as the change markers are.

use std::rc::Rc;

use adoc_engine::AttributeValue;
use gpui::{
    canvas, point, px, App, Bounds, ContentMask, Entity, Font, Hsla, IntoElement, Pixels,
    SharedString, Styled as _, TextAlign, TextRun,
};
use gpui_component::input::{EditorState, RopeExt as _};

/// The most characters of one value shown, its ellipsis included.
const VALUE_CHARS: usize = 40;
/// Between the line's text and its values, in widths of the font's `m`.
const GAP_EMS: f32 = 2.;
/// Between two values on one line.
const SEPARATOR: &str = "  ·  ";

/// One line's values as drawn after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineValues {
    pub line: usize,
    pub text: String,
}

/// The values of `values` by line of `text`: `name = value` for each reference, in order, an
/// attribute named again on its line with the same value shown once.
#[must_use]
pub fn by_line(text: &ropey::Rope, values: &[AttributeValue]) -> Vec<LineValues> {
    let mut lines: Vec<(usize, Vec<(&str, &str)>)> = Vec::new();
    for value in values {
        if value.range.start > text.len() {
            continue;
        }
        let line = text.byte_to_line_idx(value.range.start, ropey::LineType::LF_CR);
        let entry = (value.name.as_str(), value.value.as_str());
        match lines.last_mut() {
            Some((last, entries)) if *last == line => {
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
            _ => lines.push((line, vec![entry])),
        }
    }
    lines
        .into_iter()
        .map(|(line, entries)| LineValues {
            line,
            text: entries
                .iter()
                .map(|(name, value)| format!("{name} = {}", shortened(value)))
                .collect::<Vec<_>>()
                .join(SEPARATOR),
        })
        .collect()
}

/// `value` on one line, cut to [`VALUE_CHARS`] with an ellipsis.
fn shortened(value: &str) -> String {
    let one_line: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let mut chars = one_line.chars();
    let short: String = chars.by_ref().take(VALUE_CHARS - 1).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// Paints each of `lines` the editor shows after the line's last character, in `font` at
/// `font_size`, clipped to the editor.
pub fn overlay(
    state: Entity<EditorState>,
    lines: Rc<Vec<LineValues>>,
    font: Font,
    font_size: Pixels,
    colour: Hsla,
) -> impl IntoElement {
    canvas(
        move |_, _, cx| place(state.read(cx), &lines),
        move |area, placed, window, cx| {
            let gap = {
                let text_system = window.text_system();
                let id = text_system.resolve_font(&font);
                text_system
                    .em_advance(id, font_size)
                    .unwrap_or(font_size / 2.)
                    * GAP_EMS
            };
            let mask = ContentMask { bounds: area };
            window.with_content_mask(Some(mask), |window| {
                paint(&placed, &font, font_size, colour, gap, window, cx);
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Where each line's values start: the end of its text, for the lines the editor laid out.
fn place(state: &EditorState, lines: &[LineValues]) -> Vec<(Bounds<Pixels>, SharedString)> {
    let Some(rows) = state.visible_row_range() else {
        return Vec::new();
    };
    let map = state.display_map();
    let first = map.display_row_to_buffer_line(rows.start);
    let last = map.display_row_to_buffer_line(rows.end.saturating_sub(1).max(rows.start));
    let text = state.text();
    lines
        .iter()
        .filter(|values| (first..=last).contains(&values.line))
        .filter_map(|values| {
            let mut end = text.line_end_offset(values.line);
            if end > text.line_start_offset(values.line) && text.slice(end - 1..end) == "\r" {
                end -= 1;
            }
            let bounds = state.range_to_bounds(&(end..end))?;
            Some((bounds, values.text.clone().into()))
        })
        .collect()
}

fn paint(
    placed: &[(Bounds<Pixels>, SharedString)],
    font: &Font,
    font_size: Pixels,
    colour: Hsla,
    gap: Pixels,
    window: &mut gpui::Window,
    cx: &mut App,
) {
    for (bounds, text) in placed {
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color: colour,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(text.clone(), font_size, &[run], None);
        let _ = shaped.paint(
            point(bounds.right() + gap, bounds.top()),
            bounds.size.height.max(px(0.)),
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use adoc_core::SourceRange;
    use adoc_engine::AttributeValue;

    use super::{by_line, LineValues};

    fn value(text: &str, nth: usize, name: &str, value: &str) -> AttributeValue {
        let written = format!("{{{name}}}");
        let start = text
            .match_indices(&written)
            .nth(nth)
            .expect("the reference")
            .0;
        AttributeValue {
            range: SourceRange::new(start, start + written.len()),
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn values_are_grouped_by_line_in_order() {
        let text = "= {product}\r\n\r\n{product} {version} and {product} again\n";
        let rope = ropey::Rope::from(text);
        let values = [
            value(text, 0, "product", "Foundry"),
            value(text, 1, "product", "Foundry"),
            value(text, 0, "version", "2.0"),
            value(text, 2, "product", "Foundry"),
        ];

        assert_eq!(
            by_line(&rope, &values),
            [
                LineValues {
                    line: 0,
                    text: "product = Foundry".to_owned()
                },
                LineValues {
                    line: 2,
                    text: "product = Foundry  ·  version = 2.0".to_owned()
                },
            ]
        );
    }

    #[test]
    fn a_long_value_is_cut_and_kept_to_one_line() {
        let text = "{note}\n";
        let long = format!("first\tline {}", "x".repeat(60));
        let values = [value(text, 0, "note", &long)];

        let shown = &by_line(&ropey::Rope::from(text), &values)[0].text;

        assert!(shown.starts_with("note = first line xx"), "{shown}");
        assert!(shown.ends_with('…'), "{shown}");
        assert_eq!(shown.chars().count(), "note = ".len() + super::VALUE_CHARS);
    }
}
