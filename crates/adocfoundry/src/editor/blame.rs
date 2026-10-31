//! Line blame beside the source editor: a column of who last changed each line, and the tooltip
//! that hovering the gutter shows.

use std::collections::BTreeSet;

use adoc_engine::BlameLine;
use gpui::{
    canvas, point, px, App, Bounds, DispatchPhase, Entity, Font, Hsla, IntoElement, MouseMoveEvent,
    Pixels, SharedString, Styled as _, TextAlign, TextRun, WeakEntity,
};
use gpui_component::input::{EditorState, RopeExt as _};

use super::SourceEditorView;

/// Width of the blame column.
pub const COLUMN: Pixels = px(240.);

/// The buffer lines the editor shows, each with the bounds of its start in window coordinates.
pub fn visible_lines(state: &EditorState, area: Bounds<Pixels>) -> Vec<(usize, Bounds<Pixels>)> {
    let Some(line_height) = state.line_height() else {
        return Vec::new();
    };
    if line_height <= px(0.) {
        return Vec::new();
    }
    let first = (f32::from(-state.scroll_offset().y) / f32::from(line_height))
        .floor()
        .max(0.) as usize;
    let rows = (f32::from(area.size.height) / f32::from(line_height)).ceil() as usize + 2;
    let text = state.text();
    let mut seen = BTreeSet::new();
    let mut lines = Vec::new();
    for row in first..first + rows {
        let line = state.display_map().display_row_to_buffer_line(row);
        if !seen.insert(line) {
            continue;
        }
        let offset = text.line_start_offset(line);
        if let Some(bounds) = state.range_to_bounds(&(offset..offset)) {
            lines.push((line, bounds));
        }
    }
    lines
}

/// How long ago `time` (seconds since the Unix epoch) was, as of `now`: `3 days ago`.
#[must_use]
pub fn relative_time(time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    let (amount, unit) = match seconds {
        0..=59 => return "just now".to_owned(),
        60..=3_599 => (seconds / 60, "minute"),
        3_600..=86_399 => (seconds / 3_600, "hour"),
        86_400..=2_591_999 => (seconds / 86_400, "day"),
        2_592_000..=31_535_999 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    let plural = if amount == 1 { "" } else { "s" };
    format!("{amount} {unit}{plural} ago")
}

/// `abc1234 · Paul Snow · 3 days ago · Fix the install steps`, or `Not committed`.
#[must_use]
pub fn describe(line: &BlameLine, now: i64) -> String {
    match &line.commit {
        Some(commit) => format!(
            "{commit} · {} · {} · {}",
            line.author,
            relative_time(line.time, now),
            line.summary
        ),
        None => line.summary.clone(),
    }
}

/// The blame column's text for one line: shorter than the tooltip's.
fn column_text(line: &BlameLine, now: i64) -> String {
    let text = match &line.commit {
        Some(commit) => format!(
            "{commit} {}, {}",
            line.author,
            relative_time(line.time, now)
        ),
        None => line.summary.clone(),
    };
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(34).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// The seconds since the Unix epoch now.
#[must_use]
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0))
}

/// Paints the blame of each visible line in a column `left` of the element it is placed in.
pub fn column(
    state: Entity<EditorState>,
    blame: std::sync::Arc<Vec<BlameLine>>,
    font: Font,
    colour: Hsla,
    left: Pixels,
) -> impl IntoElement {
    canvas(
        move |area, _, cx| visible_lines(state.read(cx), area),
        move |area, lines, window, cx| {
            let font_size = window.text_style().font_size.to_pixels(window.rem_size()) * 0.85;
            let now = now();
            for (line, bounds) in lines {
                let Some(blamed) = blame.get(line) else {
                    continue;
                };
                let text: SharedString = column_text(blamed, now).into();
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
                    .shape_line(text, font_size, &[run], None);
                let _ = shaped.paint(
                    point(area.left() + px(6.), bounds.top()),
                    bounds.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        },
    )
    .absolute()
    .top_0()
    .left(left)
    .h_full()
    .w(COLUMN)
}

/// Watches the pointer over the editor's gutter, telling `view` which line it rests on.
pub fn hover_watch(
    view: WeakEntity<SourceEditorView>,
    state: Entity<EditorState>,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |area, (), window, _| {
            let state = state.clone();
            let view = view.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx: &mut App| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let state = state.read(cx);
                // The gutter runs from the editor's left edge to where each line's text starts.
                let line = area
                    .contains(&event.position)
                    .then(|| {
                        visible_lines(state, area).into_iter().find(|(_, bounds)| {
                            bounds.top() <= event.position.y
                                && event.position.y < bounds.bottom()
                                && event.position.x < bounds.left()
                        })
                    })
                    .flatten()
                    .map(|(line, _)| line);
                let position = event.position;
                let _ = view.update(cx, |view, cx| view.hover_gutter(line, position, cx));
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_people_say_them() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now - 5, now), "just now");
        assert_eq!(relative_time(now - 60, now), "1 minute ago");
        assert_eq!(relative_time(now - 7_200, now), "2 hours ago");
        assert_eq!(relative_time(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(relative_time(now - 65 * 86_400, now), "2 months ago");
        assert_eq!(relative_time(now - 400 * 86_400, now), "1 year ago");
        assert_eq!(relative_time(now + 50, now), "just now", "a clock ahead");
    }

    #[test]
    fn describes_committed_and_uncommitted_lines() {
        let line = BlameLine {
            line: 0,
            commit: Some("abc1234".to_owned()),
            author: "Paul Snow".to_owned(),
            time: 1_000_000_000 - 3 * 86_400,
            summary: "Fix the install steps".to_owned(),
        };
        assert_eq!(
            describe(&line, 1_000_000_000),
            "abc1234 · Paul Snow · 3 days ago · Fix the install steps"
        );
        let uncommitted = BlameLine {
            commit: None,
            summary: "Not committed".to_owned(),
            ..line
        };
        assert_eq!(describe(&uncommitted, 1_000_000_000), "Not committed");
    }
}
