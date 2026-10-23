//! A text block drawn for editing: styled text whose layout is recorded for hit testing, with
//! the selection and caret painted over it.

use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};

use adoc_engine::{Atom, BlockPath, Mark, MarkSet, Piece, TextBlock, TextContext};
use gpui::{
    canvas, fill, px, size, AnyElement, Bounds, FontStyle, FontWeight, HighlightStyle, Hsla,
    IntoElement as _, ParentElement as _, Pixels, Point, ScrollHandle, SharedString, Styled as _,
    StyledText, TextLayout, TextRun, UnderlineStyle,
};

use crate::preview::inlines::InlineStyle;

/// The layout of every text block drawn in the last frame, by path.
pub type Layouts = Rc<RefCell<HashMap<BlockPath, TextLayout>>>;

/// A text block's display text as it is shaped, with an IME composition in progress shown at
/// the caret but not yet in the document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub text: String,
    /// The composition's range in `text`.
    pub composing: Option<Range<usize>>,
    /// The display length, without the composition.
    pub display_len: usize,
}

impl Shown {
    /// The index in the shown text of display offset `display`; the caret at a composition
    /// sits after it.
    #[must_use]
    pub fn index(&self, display: usize) -> usize {
        match &self.composing {
            Some(composing) if display >= composing.start => display + composing.len(),
            _ => display,
        }
    }

    /// The display offset of index `index` in the shown text.
    #[must_use]
    pub fn display(&self, index: usize) -> usize {
        let display = match &self.composing {
            Some(composing) if index >= composing.end => index - composing.len(),
            Some(composing) if index > composing.start => composing.start,
            _ => index,
        };
        display.min(self.display_len)
    }
}

/// A stretch of display text and how it is drawn.
struct Run<'t> {
    range: Range<usize>,
    marks: MarkSet,
    atom: Option<&'t Atom>,
}

fn runs_of(block: &TextBlock) -> Vec<Run<'_>> {
    fn visit<'t>(pieces: &'t [Piece], marks: MarkSet, at: &mut usize, out: &mut Vec<Run<'t>>) {
        for piece in pieces {
            match piece {
                Piece::Text { range } => {
                    out.push(Run {
                        range: *at..*at + range.len(),
                        marks,
                        atom: None,
                    });
                    *at += range.len();
                }
                Piece::Atom { atom, shown, .. } => {
                    out.push(Run {
                        range: *at..*at + shown.len(),
                        marks,
                        atom: Some(atom),
                    });
                    *at += shown.len();
                }
                Piece::Mark { mark, children, .. } => visit(children, marks.with(*mark), at, out),
            }
        }
    }
    let mut out = Vec::new();
    visit(&block.pieces, MarkSet::EMPTY, &mut 0, &mut out);
    out
}

/// The text to shape for `block`, with `composing` inserted at display offset `caret`. Line
/// breaks in flowing text are shown as spaces (they are one byte either way); hard line
/// breaks and code keep theirs.
#[must_use]
pub fn shown(block: &TextBlock, composing: Option<(usize, &str)>) -> Shown {
    let mut text = String::with_capacity(block.display.len());
    let mut after_break = false;
    for run in runs_of(block) {
        let part = &block.display[run.range.clone()];
        if block.context == TextContext::Verbatim {
            text.push_str(part);
        } else if matches!(run.atom, Some(Atom::LineBreak)) {
            // ` +` is drawn as a space; the source's own line break after it breaks the line.
            text.push_str(&" ".repeat(part.len()));
            after_break = true;
            continue;
        } else if run.atom.is_some() {
            text.push_str(part);
        } else if let Some(rest) = part.strip_prefix('\n').filter(|_| after_break) {
            text.push('\n');
            text.push_str(&rest.replace('\n', " "));
        } else {
            text.push_str(&part.replace('\n', " "));
        }
        after_break = false;
    }
    let display_len = text.len();
    let composing = composing.map(|(caret, composition)| {
        let caret = caret.min(text.len());
        text.insert_str(caret, composition);
        caret..caret + composition.len()
    });
    Shown {
        text,
        composing,
        display_len,
    }
}

/// The text runs for `shown`: fonts and colours by mark, atoms drawn as references, the
/// composition underlined.
#[must_use]
pub fn text_runs(
    block: &TextBlock,
    shown: &Shown,
    style: &InlineStyle,
    bold: bool,
) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut push = |length: usize, marks: MarkSet, atom: Option<&Atom>, composing: bool| {
        if length == 0 {
            return;
        }
        let mut font = if block.context == TextContext::Verbatim
            || marks.contains(Mark::Code)
            || matches!(atom, Some(Atom::Macro { .. }))
        {
            gpui::Font {
                family: style.mono_family.clone(),
                ..style.font.clone()
            }
        } else {
            style.font.clone()
        };
        if bold || marks.contains(Mark::Strong) {
            font.weight = FontWeight::BOLD;
        }
        if marks.contains(Mark::Emphasis) {
            font.style = FontStyle::Italic;
        }
        let link = matches!(atom, Some(Atom::Xref { .. } | Atom::Link { .. }));
        let quiet = matches!(
            atom,
            Some(Atom::Footnote { .. } | Atom::Image { .. } | Atom::Anchor { .. })
        ) || marks.contains(Mark::Superscript)
            || marks.contains(Mark::Subscript);
        let color = if link {
            style.link
        } else if quiet {
            style.muted
        } else {
            style.color
        };
        let underline = (link || composing).then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: false,
        });
        let background_color = if marks.contains(Mark::Highlight) {
            Some(style.mark)
        } else if marks.contains(Mark::Code) {
            Some(style.code_background)
        } else {
            None
        };
        runs.push(TextRun {
            len: length,
            font,
            color,
            background_color,
            underline,
            strikethrough: None,
        });
    };
    let composing = shown.composing.clone();
    let composition_marks = composing
        .as_ref()
        .map(|range| block.marks_at(range.start))
        .unwrap_or_default();
    let mut composed = false;
    for run in runs_of(block) {
        if let Some(range) = composing.as_ref().filter(|_| !composed) {
            if run.range.end >= range.start && run.range.start <= range.start {
                let before = range.start - run.range.start;
                push(before, run.marks, run.atom, false);
                push(range.len(), composition_marks, None, true);
                push(run.range.len() - before, run.marks, run.atom, false);
                composed = true;
                continue;
            }
        }
        push(run.range.len(), run.marks, run.atom, false);
    }
    if let Some(range) = composing.filter(|_| !composed) {
        push(range.len(), composition_marks, None, true);
    }
    if shown.text.is_empty() {
        // An empty block is shaped as a space, so the caret has a line to sit on.
        push(1, MarkSet::EMPTY, None, false);
    }
    runs
}

/// What to paint over a text block.
#[derive(Clone)]
pub struct Overlay {
    /// The caret's index in the shown text.
    pub caret: Option<usize>,
    /// The selection's range in the shown text.
    pub selection: Option<Range<usize>>,
    pub caret_colour: Hsla,
    pub selection_colour: Hsla,
    /// Scroll so the caret is visible, once.
    pub reveal: Option<(ScrollHandle, Rc<std::cell::Cell<bool>>)>,
}

/// The element for a text block: its text, and an overlay that records the layout under
/// `path` and paints the selection and caret.
pub fn element(
    path: BlockPath,
    shown: &Shown,
    runs: Vec<TextRun>,
    layouts: &Layouts,
    overlay: Overlay,
) -> AnyElement {
    let text: SharedString = if shown.text.is_empty() {
        " ".into()
    } else {
        shown.text.clone().into()
    };
    let styled = StyledText::new(text.clone()).with_runs(runs);
    let layout = styled.layout().clone();
    let layouts = layouts.clone();
    let length = text.len();
    gpui::div()
        .relative()
        .w_full()
        .child(styled)
        .child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    layouts.borrow_mut().insert(path.clone(), layout.clone());
                    if let Some(selection) = &overlay.selection {
                        for rect in selection_rects(&layout, selection.clone(), &text) {
                            window.paint_quad(fill(rect, overlay.selection_colour));
                        }
                    }
                    let Some(caret) = overlay.caret else {
                        return;
                    };
                    let Some(position) = layout.position_for_index(caret.min(length)) else {
                        return;
                    };
                    let line_height = layout.line_height();
                    let bounds = Bounds::new(position, size(px(2.), line_height));
                    window.paint_quad(fill(bounds, overlay.caret_colour));
                    if let Some((scroll, reveal)) = &overlay.reveal {
                        if reveal.replace(false) {
                            scroll_into_view(scroll, bounds);
                            window.refresh();
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .into_any_element()
}

/// Adjusts `scroll` so `caret` is inside its visible bounds.
fn scroll_into_view(scroll: &ScrollHandle, caret: Bounds<Pixels>) {
    let visible = scroll.bounds();
    let mut offset = scroll.offset();
    let margin = caret.size.height;
    if caret.top() < visible.top() + margin {
        offset.y += visible.top() + margin - caret.top();
    } else if caret.bottom() > visible.bottom() - margin {
        offset.y -= caret.bottom() - (visible.bottom() - margin);
    }
    offset.y = offset.y.min(px(0.));
    scroll.set_offset(offset);
}

/// The rectangles covering `range` of the laid-out text, one per line.
fn selection_rects(layout: &TextLayout, range: Range<usize>, text: &str) -> Vec<Bounds<Pixels>> {
    let line_height = layout.line_height();
    let mut rects: Vec<Bounds<Pixels>> = Vec::new();
    let mut boundaries: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .filter(|index| range.contains(index))
        .collect();
    boundaries.push(range.end.min(text.len()));
    for pair in boundaries.windows(2) {
        let (Some(from), Some(to)) = (
            layout.position_for_index(pair[0]),
            layout.position_for_index(pair[1]),
        ) else {
            continue;
        };
        // A character that ends a wrapped line is given a nominal width.
        let right = if to.y == from.y {
            to.x
        } else {
            from.x + line_height / 3.
        };
        let rect = Bounds::new(from, size(right - from.x, line_height));
        match rects.last_mut() {
            Some(last)
                if last.origin.y == rect.origin.y && last.right() >= rect.left() - px(1.) =>
            {
                last.size.width = rect.right() - last.left();
            }
            _ => rects.push(rect),
        }
    }
    rects
}

/// The display offset under `position` in the block at `path`, if it was drawn.
#[must_use]
pub fn hit(
    layouts: &Layouts,
    path: &[usize],
    position: Point<Pixels>,
    shown: &Shown,
) -> Option<usize> {
    let layouts = layouts.borrow();
    let layout = layouts.get(path)?;
    let index = match layout.index_for_position(position) {
        Ok(index) | Err(index) => index,
    };
    Some(shown.display(index))
}

/// The point at the top of display offset `display` in the block at `path`, and its line
/// height.
#[must_use]
pub fn caret_point(
    layouts: &Layouts,
    path: &[usize],
    shown: &Shown,
    display: usize,
) -> Option<(Point<Pixels>, Pixels)> {
    let layouts = layouts.borrow();
    let layout = layouts.get(path)?;
    let position = layout.position_for_index(shown.index(display).min(shown.text.len().max(1)))?;
    Some((position, layout.line_height()))
}

/// The bounds of the block at `path`, if it was drawn.
#[must_use]
pub fn bounds_of(layouts: &Layouts, path: &[usize]) -> Option<Bounds<Pixels>> {
    layouts.borrow().get(path).map(TextLayout::bounds)
}

/// `runs` with syntax highlighting laid over them: each run is split where a highlight starts or
/// ends, and a highlighted piece takes its colour, weight and style. Both are in bytes of the
/// same text, so the layout, the caret and the selection are unchanged.
#[must_use]
pub fn with_highlights(
    runs: Vec<TextRun>,
    highlights: &[(std::ops::Range<usize>, HighlightStyle)],
) -> Vec<TextRun> {
    if highlights.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + highlights.len() * 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        // The cut points inside this run.
        let mut cuts: Vec<usize> = highlights
            .iter()
            .flat_map(|(range, _)| [range.start, range.end])
            .filter(|cut| *cut > start && *cut < end)
            .collect();
        cuts.push(end);
        cuts.sort_unstable();
        cuts.dedup();
        let mut from = start;
        for to in cuts {
            let mut piece = run.clone();
            piece.len = to - from;
            if let Some((_, style)) = highlights
                .iter()
                .find(|(range, _)| range.start <= from && to <= range.end)
            {
                if let Some(color) = style.color {
                    piece.color = color;
                }
                if let Some(weight) = style.font_weight {
                    piece.font.weight = weight;
                }
                if let Some(font_style) = style.font_style {
                    piece.font.style = font_style;
                }
            }
            out.push(piece);
            from = to;
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use adoc_core::SourceRange;
    use adoc_engine::{Atom, Piece, TextBlock, TextContext};
    use gpui::{font, hsla};

    use super::*;

    fn style() -> InlineStyle {
        InlineStyle {
            font: font("Sans"),
            mono_family: "Mono".into(),
            color: hsla(0., 0., 0., 1.),
            muted: hsla(0., 0., 0.5, 1.),
            link: hsla(0.6, 1., 0.5, 1.),
            unresolved: hsla(0., 1., 0.5, 1.),
            mark: hsla(0.15, 1., 0.5, 1.),
            code_background: hsla(0., 0., 0.9, 1.),
        }
    }

    #[test]
    fn code_is_drawn_in_the_monospace_font_with_its_lines() {
        let block = TextBlock {
            range: SourceRange::new(0, 9),
            pieces: vec![Piece::Text {
                range: SourceRange::new(0, 9),
            }],
            plain: true,
            display: "a = 1\nb()".to_owned(),
            context: TextContext::Verbatim,
        };
        let shown = shown(&block, None);
        assert_eq!(shown.text, "a = 1\nb()");
        let runs = text_runs(&block, &shown, &style(), false);
        assert!(runs.iter().all(|run| run.font.family.as_ref() == "Mono"));
    }

    #[test]
    fn a_hard_line_break_breaks_at_the_source_line_break() {
        // `line +\nnext`: text, the ` +` atom, then the newline starting the next line.
        let block = TextBlock {
            range: SourceRange::new(0, 11),
            pieces: vec![
                Piece::Text {
                    range: SourceRange::new(0, 4),
                },
                Piece::Atom {
                    atom: Atom::LineBreak,
                    range: SourceRange::new(4, 6),
                    shown: "\n".to_owned(),
                },
                Piece::Text {
                    range: SourceRange::new(6, 11),
                },
            ],
            plain: false,
            display: "line\n\nnext".to_owned(),
            context: TextContext::Flow,
        };
        let shown = shown(&block, None);
        assert_eq!(shown.text, "line \nnext", "no space starts the second line");
        assert_eq!(
            shown.text.len(),
            block.display.len(),
            "byte offsets are kept"
        );
    }

    #[test]
    fn highlights_split_runs_without_changing_the_text() {
        let base = TextRun {
            len: 0,
            font: font("Mono"),
            color: hsla(0., 0., 0., 1.),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        // "fn main()": two runs, 3 and 6 bytes; a keyword highlight over "fn".
        let runs = vec![
            TextRun {
                len: 3,
                ..base.clone()
            },
            TextRun {
                len: 6,
                ..base.clone()
            },
        ];
        let keyword = HighlightStyle {
            color: Some(hsla(0.6, 1., 0.5, 1.)),
            font_weight: Some(FontWeight::BOLD),
            ..HighlightStyle::default()
        };
        let out = with_highlights(runs, &[(0..2, keyword)]);
        assert_eq!(out.iter().map(|run| run.len).collect::<Vec<_>>(), [2, 1, 6]);
        assert_eq!(out[0].color, hsla(0.6, 1., 0.5, 1.));
        assert_eq!(out[0].font.weight, FontWeight::BOLD);
        assert_eq!(out[1].color, base.color);
        assert_eq!(out.iter().map(|run| run.len).sum::<usize>(), 9);
        // No highlights: the runs as they were.
        assert_eq!(
            with_highlights(vec![TextRun { len: 4, ..base }], &[]).len(),
            1
        );
    }
}
