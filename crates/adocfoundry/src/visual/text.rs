//! A text block drawn for editing: styled text whose layout is recorded for hit testing, with
//! the selection and caret painted over it. A typeset STEM macro is shown as its formula, drawn
//! over a gap that takes the place of its text, as in the preview.

use std::{cell::RefCell, collections::HashMap, ops::Range, path::PathBuf, rc::Rc};

use adoc_engine::{Atom, BlockPath, Mark, MarkSet, Piece, TextBlock, TextContext};
use gpui::{
    canvas, fill, px, size, AnyElement, Bounds, FontStyle, FontWeight, HighlightStyle, Hsla,
    IntoElement as _, ParentElement as _, Pixels, Point, ScrollHandle, SharedString, Styled as _,
    StyledText, TextLayout, TextRun, UnderlineStyle,
};

use crate::preview::inlines::{place_formula, Gap, InlineStyle};

/// The layout of every text block drawn in the last frame, by path.
pub type Layouts = Rc<RefCell<HashMap<BlockPath, TextLayout>>>;

/// The typeset inline formulas of the document, by source, each with its gap as last measured.
pub type FormulaGaps = HashMap<String, (Gap, PathBuf)>;

/// A typeset formula shown over a gap in place of its macro's text.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownFormula {
    /// The macro's text in the display text, and its gap in the shown text.
    pub display: Range<usize>,
    pub shown: Range<usize>,
    pub gap: Gap,
    pub path: PathBuf,
}

/// A text block's display text as it is shaped, with an IME composition in progress shown at
/// the caret but not yet in the document, and typeset formulas in place of their macros.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub text: String,
    /// The composition's range in `text`.
    pub composing: Option<Range<usize>>,
    /// The display offset the composition is at.
    composing_at: usize,
    /// The display length, without the composition.
    pub display_len: usize,
    pub formulas: Vec<ShownFormula>,
}

impl Shown {
    /// The index in the shown text of display offset `display`; the caret at a composition
    /// sits after it, and an offset inside a formula's macro is taken to its end.
    #[must_use]
    pub fn index(&self, display: usize) -> usize {
        let mut index = display;
        for formula in &self.formulas {
            if display >= formula.display.end {
                index = index + formula.shown.len() - formula.display.len();
            } else if display > formula.display.start {
                return self.index(formula.display.end);
            }
        }
        match &self.composing {
            Some(composing) if display >= self.composing_at => index + composing.len(),
            _ => index,
        }
    }

    /// The display offset of index `index` in the shown text: inside a formula's gap, the
    /// nearer end of its macro.
    #[must_use]
    pub fn display(&self, index: usize) -> usize {
        // What the shown text holds in place of display text, in the order it is shown.
        let mut replaced: Vec<(Range<usize>, Range<usize>, bool)> = self
            .formulas
            .iter()
            .map(|formula| (formula.shown.clone(), formula.display.clone(), false))
            .collect();
        if let Some(composing) = &self.composing {
            replaced.push((
                composing.clone(),
                self.composing_at..self.composing_at,
                true,
            ));
        }
        replaced.sort_by_key(|(shown, ..)| shown.start);
        let mut shown_extra = 0;
        let mut display_extra = 0;
        for (shown, display, composition) in replaced {
            if index >= shown.end {
                shown_extra += shown.len();
                display_extra += display.len();
            } else if index > shown.start {
                return if composition || index - shown.start <= shown.len() / 2 {
                    display.start
                } else {
                    display.end
                };
            } else {
                break;
            }
        }
        (index + display_extra - shown_extra).min(self.display_len)
    }
}

/// Whether `atom` is a STEM macro, which shows its formula's source.
fn is_formula(atom: Option<&Atom>) -> bool {
    matches!(atom, Some(Atom::Macro { name }) if matches!(name.as_str(), "stem" | "asciimath" | "latexmath"))
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
/// breaks and code keep theirs. A STEM macro whose formula is in `formulas` is shown as the
/// formula's gap.
#[must_use]
pub fn shown(block: &TextBlock, composing: Option<(usize, &str)>, formulas: &FormulaGaps) -> Shown {
    let mut text = String::with_capacity(block.display.len());
    let mut shown_formulas = Vec::new();
    let mut after_break = false;
    for run in runs_of(block) {
        let part = &block.display[run.range.clone()];
        let formula = (block.context != TextContext::Verbatim && is_formula(run.atom))
            .then(|| formulas.get(part.trim()))
            .flatten();
        if let Some((gap, path)) = formula {
            let start = text.len();
            text.push_str(&gap.text);
            shown_formulas.push(ShownFormula {
                display: run.range.clone(),
                shown: start..text.len(),
                gap: gap.clone(),
                path: path.clone(),
            });
        } else if block.context == TextContext::Verbatim {
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
    let mut shown = Shown {
        display_len: block.display.len(),
        text,
        composing: None,
        composing_at: 0,
        formulas: shown_formulas,
    };
    if let Some((caret, composition)) = composing {
        let caret = caret.min(shown.display_len);
        let index = shown.index(caret);
        shown.text.insert_str(index, composition);
        for formula in &mut shown.formulas {
            if formula.shown.start >= index {
                formula.shown.start += composition.len();
                formula.shown.end += composition.len();
            }
        }
        shown.composing = Some(index..index + composition.len());
        shown.composing_at = caret;
    }
    shown
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
    let push_gap = |length: usize, runs: &mut Vec<TextRun>| {
        if length > 0 {
            runs.push(TextRun {
                len: length,
                font: style.font.clone(),
                color: gpui::transparent_black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
    };
    let push = |runs: &mut Vec<TextRun>,
                length: usize,
                marks: MarkSet,
                atom: Option<&Atom>,
                composing: bool| {
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
    let caret = shown.composing_at;
    let composition_marks = composing
        .as_ref()
        .map(|_| block.marks_at(caret))
        .unwrap_or_default();
    let mut composed = false;
    for run in runs_of(block) {
        let formula = shown
            .formulas
            .iter()
            .find(|formula| formula.display == run.range);
        let length = formula.map_or(run.range.len(), |formula| formula.shown.len());
        // A formula's gap is never split: the composition goes before it or after it.
        let piece = |runs: &mut Vec<TextRun>, length: usize| match formula {
            Some(_) => push_gap(length, runs),
            None => push(runs, length, run.marks, run.atom, false),
        };
        if let Some(range) = composing.as_ref().filter(|_| !composed) {
            if run.range.end >= caret && run.range.start <= caret {
                let before = match formula {
                    Some(_) if caret == run.range.start => 0,
                    Some(_) => length,
                    None => caret - run.range.start,
                };
                piece(&mut runs, before);
                push(&mut runs, range.len(), composition_marks, None, true);
                piece(&mut runs, length - before);
                composed = true;
                continue;
            }
        }
        piece(&mut runs, length);
    }
    if let Some(range) = composing.filter(|_| !composed) {
        push(&mut runs, range.len(), composition_marks, None, true);
    }
    if shown.text.is_empty() {
        // An empty block is shaped as a space, so the caret has a line to sit on.
        push(&mut runs, 1, MarkSet::EMPTY, None, false);
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
    /// Behind a typeset formula.
    pub formula_backing: Option<Hsla>,
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
    let formulas = shown.formulas.clone();
    let backing = overlay.formula_backing;
    let placing = layout.clone();
    gpui::div()
        .relative()
        .w_full()
        .child(styled)
        .child(
            canvas(
                // After the text, so its layout is known.
                move |_, window, cx| {
                    formulas
                        .iter()
                        .filter_map(|formula| {
                            place_formula(
                                &placing,
                                formula.shown.start,
                                &formula.gap,
                                &formula.path,
                                backing,
                                window,
                                cx,
                            )
                        })
                        .collect::<Vec<AnyElement>>()
                },
                move |_, images, window, cx| {
                    for mut image in images {
                        image.paint(window, cx);
                    }
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
            formula_backing: None,
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
        let shown = shown(&block, None, &FormulaGaps::new());
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
        let shown = shown(&block, None, &FormulaGaps::new());
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

    #[test]
    fn formulas_take_the_place_of_their_macros() {
        // `a stem:[x^2] b`: the macro shows its source, three bytes, as a gap of four.
        let block = TextBlock {
            range: SourceRange::new(0, 14),
            pieces: vec![
                Piece::Text {
                    range: SourceRange::new(0, 2),
                },
                Piece::Atom {
                    atom: Atom::Macro {
                        name: "stem".to_owned(),
                    },
                    range: SourceRange::new(2, 12),
                    shown: "x^2".to_owned(),
                },
                Piece::Text {
                    range: SourceRange::new(12, 14),
                },
            ],
            plain: false,
            display: "a x^2 b".to_owned(),
            context: TextContext::Flow,
        };
        let gap = Gap {
            text: "\u{a0}\u{a0}".to_owned(),
            first: (2, px(4.)),
            width: px(8.),
            size: size(px(8.), px(10.)),
            baseline: px(7.),
        };
        let formulas = FormulaGaps::from([("x^2".to_owned(), (gap, PathBuf::from("/x.svg")))]);

        let plain = shown(&block, None, &formulas);
        assert_eq!(plain.text, "a \u{a0}\u{a0} b");
        assert_eq!(plain.formulas[0].shown, 2..6);
        assert_eq!(plain.display_len, 7);
        assert_eq!(
            [0, 2, 3, 5, 7].map(|display| plain.index(display)),
            [0, 2, 6, 6, 8]
        );
        assert_eq!(
            [2, 3, 4, 5, 6, 8].map(|index| plain.display(index)),
            [2, 2, 2, 5, 5, 7]
        );
        let runs = text_runs(&block, &plain, &style(), false);
        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            [2, 4, 2]
        );
        assert_eq!(runs[1].color, gpui::transparent_black());

        // Composing right after the formula: the composition follows the gap.
        let composing = shown(&block, Some((5, "é")), &formulas);
        assert_eq!(composing.text, "a \u{a0}\u{a0}é b");
        assert_eq!(composing.composing, Some(6..8));
        assert_eq!(composing.index(5), 8);
        assert_eq!(composing.display(8), 5);
        let runs = text_runs(&block, &composing, &style(), false);
        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            [2, 4, 2, 2]
        );
        // Before it, the gap follows the composition.
        let before = shown(&block, Some((2, "é")), &formulas);
        assert_eq!(before.text, "a é\u{a0}\u{a0} b");
        assert_eq!(before.formulas[0].shown, 4..8);
        assert_eq!(before.index(5), 8);
    }
}
