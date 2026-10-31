//! Git change markers in the source editor's gutter, and the card a click on one opens.
//!
//! The markers are painted over the editor's left edge from the editor's own layout, after the
//! editor has laid out the frame, so they follow scrolling, wrapping and folding without ever
//! touching the text layout.

use std::{cell::RefCell, rc::Rc};

use adoc_engine::{Hunk, HunkKind};
use gpui::{
    canvas, fill, point, px, size, App, Bounds, DispatchPhase, Entity, Hsla, IntoElement,
    MouseDownEvent, PathBuilder, Pixels, Styled as _, WeakEntity, Window,
};
use gpui_component::input::EditorState;

use super::SourceEditorView;

/// Width of a marker bar.
const BAR: Pixels = px(3.);
/// Markers are easier to click than to see: this much wider.
const HIT: Pixels = px(8.);

/// One painted marker: which hunk, of what kind, where.
#[derive(Clone, Debug, PartialEq)]
pub struct GutterMark {
    pub hunk: usize,
    pub kind: HunkKind,
    pub bounds: Bounds<Pixels>,
}

/// The colours of added, modified and deleted lines.
#[derive(Clone, Copy)]
pub struct MarkColours {
    pub added: Hsla,
    pub modified: Hsla,
    pub deleted: Hsla,
}

/// Paints a marker for each of `hunks` at the left of the editor, recording them in `marks`;
/// a click on one opens it in `view`.
pub fn overlay(
    view: WeakEntity<SourceEditorView>,
    state: Entity<EditorState>,
    hunks: Vec<(usize, Hunk)>,
    marks: Rc<RefCell<Vec<GutterMark>>>,
    colours: MarkColours,
) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            let placed = place(state.read(cx), &hunks, bounds);
            marks.borrow_mut().clone_from(&placed);
            placed
        },
        move |_, placed: Vec<GutterMark>, window, _| {
            for mark in &placed {
                paint(mark, colours, window);
            }
            if placed.is_empty() {
                return;
            }
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx: &mut App| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let Some(mark) = placed
                    .iter()
                    .find(|mark| hit_area(mark).contains(&event.position))
                else {
                    return;
                };
                let (hunk, position) = (mark.hunk, event.position);
                let _ = view.update(cx, |view, cx| view.open_change(hunk, position, cx));
                cx.stop_propagation();
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The hunks of `hunks`, each with its index among them, near the rows the editor last laid
/// out: those rows and a screenful either side, so a hunk scrolled into view this frame is
/// placed too. The rest are neither copied nor measured.
pub fn near_view(state: &EditorState, hunks: &[Hunk]) -> Vec<(usize, Hunk)> {
    let rows = state.visible_row_range();
    let range = rows.map(|rows| {
        // Only rows that exist are mapped (one past the end maps to the first line), and the
        // margin is added in lines.
        let margin = rows.end.saturating_sub(rows.start).max(1);
        let map = state.display_map();
        let first = map
            .display_row_to_buffer_line(rows.start)
            .saturating_sub(margin);
        let last =
            map.display_row_to_buffer_line(rows.end.saturating_sub(1).max(rows.start)) + margin;
        let text = state.text();
        let lines = text.len_lines(ropey::LineType::LF_CR);
        let start = text.line_to_byte_idx(first.min(lines), ropey::LineType::LF_CR);
        let end = text.line_to_byte_idx((last + 1).min(lines), ropey::LineType::LF_CR);
        start..end
    });
    hunks
        .iter()
        .enumerate()
        .filter(|(_, hunk)| {
            range
                .as_ref()
                .is_none_or(|range| hunk.range.start <= range.end && hunk.range.end >= range.start)
        })
        .map(|(index, hunk)| (index, hunk.clone()))
        .collect()
}

/// Where each hunk's marker goes in window coordinates, for the hunks the editor shows.
fn place(state: &EditorState, hunks: &[(usize, Hunk)], area: Bounds<Pixels>) -> Vec<GutterMark> {
    let at = |offset: usize| state.range_to_bounds(&(offset..offset));
    hunks
        .iter()
        .filter_map(|(index, hunk)| {
            let index = *index;
            let (top, bottom) = if hunk.kind == HunkKind::Deleted {
                let line = at(hunk.range.start)?;
                (line.top(), line.top())
            } else {
                // The hunk's last character is the newline of its last line.
                let last = hunk.range.end.saturating_sub(1).max(hunk.range.start);
                match (at(hunk.range.start), at(last)) {
                    (Some(first), Some(last)) => (first.top(), last.bottom()),
                    (None, Some(last)) => (area.top(), last.bottom()),
                    (Some(first), None) => (first.top(), area.bottom()),
                    (None, None) => return None,
                }
            };
            Some(GutterMark {
                hunk: index,
                kind: hunk.kind,
                bounds: Bounds::new(point(area.left(), top), size(BAR, bottom - top)),
            })
        })
        .collect()
}

fn paint(mark: &GutterMark, colours: MarkColours, window: &mut Window) {
    match mark.kind {
        HunkKind::Added => window.paint_quad(fill(mark.bounds, colours.added)),
        HunkKind::Modified => window.paint_quad(fill(mark.bounds, colours.modified)),
        HunkKind::Deleted => {
            // A small triangle pointing into the text, between the lines either side.
            let (x, y) = (mark.bounds.left(), mark.bounds.top());
            let mut path = PathBuilder::fill();
            path.move_to(point(x, y - px(4.)));
            path.line_to(point(x + px(6.), y));
            path.line_to(point(x, y + px(4.)));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, colours.deleted);
            }
        }
    }
}

fn hit_area(mark: &GutterMark) -> Bounds<Pixels> {
    let height = mark.bounds.size.height.max(HIT);
    Bounds::new(
        point(
            mark.bounds.left(),
            mark.bounds.top() - (height - mark.bounds.size.height) / 2.,
        ),
        size(HIT, height),
    )
}
