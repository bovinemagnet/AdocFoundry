//! A text block drawn for editing: styled text whose layout is recorded for hit testing, with
//! the selection and caret painted over it. A typeset STEM macro is shown as its formula, drawn
//! over a gap that takes the place of its text, as in the preview.

use std::{cell::RefCell, collections::HashMap, ops::Range, path::PathBuf, rc::Rc};

use adoc_engine::{Atom, BlockPath, Mark, MarkSet, Piece, TextBlock, TextContext};
use adoc_render::FormulaKey;
use gpui::{
    canvas, fill, px, size, AnyElement, Bounds, FontStyle, FontWeight, HighlightStyle, Hsla,
    IntoElement as _, ListState, ParentElement as _, Pixels, Point, SharedString,
    StrikethroughStyle, Styled as _, StyledText, TextLayout, TextRun, UnderlineStyle,
};

use crate::preview::inlines::{place_formula, Gap, InlineStyle};

/// The layout of every text block drawn in the last frame, by path.
pub type Layouts = Rc<RefCell<HashMap<BlockPath, TextLayout>>>;

/// The typeset inline formulas of the document, by notation and source, each with its gap as
/// last measured.
#[derive(Clone, Debug, Default)]
pub struct FormulaGaps {
    pub gaps: HashMap<FormulaKey, (Gap, PathBuf)>,
    /// Whether `stem:[…]` is LaTeX on the page ([`adoc_render::ViewTree::stem_is_latex`]).
    pub stem_latex: bool,
}

impl FormulaGaps {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The formula a STEM macro named `name` with `source` draws. The view reads `:stem:` after
    /// the header, so for `stem:[…]` the other notation's formula is taken when the page has
    /// none in this one, as when a body attribute entry changes `:stem:`.
    fn get(&self, name: &str, source: &str) -> Option<&(Gap, PathBuf)> {
        let key = FormulaKey::of_macro(name, source, self.stem_latex);
        self.gaps.get(&key).or_else(|| {
            (name == "stem")
                .then(|| {
                    self.gaps.get(&FormulaKey {
                        latex: !key.latex,
                        ..key
                    })
                })
                .flatten()
        })
    }
}

/// A typeset formula shown over a gap in place of its macro's text.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownFormula {
    /// The macro's text in the display text, and its gap in the shown text.
    pub display: Range<usize>,
    pub shown: Range<usize>,
    pub gap: Gap,
    pub path: PathBuf,
}

/// Text a pending suggestion would insert (#763), shown where it would go but not in the
/// display text: never selected, edited or copied, the caret stepping over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownInsertion {
    /// The display offset it is shown at, a range of no length.
    pub at: usize,
    /// Its range in the shown text.
    pub shown: Range<usize>,
    /// Whether text typed at `at` goes before it (a suggestion that only adds text before its
    /// range), so the caret there is drawn before it; otherwise after it.
    pub typed_before: bool,
}

/// A text block's display text as it is shaped, with an IME composition in progress shown at
/// the caret but not yet in the document, typeset formulas in place of their macros, and what
/// pending suggestions would insert.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub text: String,
    /// The composition's range in `text`.
    pub composing: Option<Range<usize>>,
    /// The display offset the composition is at.
    composing_at: usize,
    /// The display length, without the composition or the insertions.
    pub display_len: usize,
    pub formulas: Vec<ShownFormula>,
    /// In the order shown; at one offset, those typed text follows, then the composition, then
    /// those typed text goes before.
    pub insertions: Vec<ShownInsertion>,
}

impl Shown {
    /// The index in the shown text of display offset `display`; the caret at a composition sits
    /// after it, and at an insertion on the side typed text goes, and an offset inside a
    /// formula's macro is taken to its end.
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
        for insertion in &self.insertions {
            if display > insertion.at || (display == insertion.at && !insertion.typed_before) {
                index += insertion.shown.len();
            }
        }
        match &self.composing {
            Some(composing) if display >= self.composing_at => index + composing.len(),
            _ => index,
        }
    }

    /// The range in the shown text of the display range `display`: an end where an insertion
    /// is shown stops before it, and a start there starts after it, so a strike, a fill or a
    /// selection never begins or ends covering what the page does not hold.
    #[must_use]
    pub fn span(&self, display: Range<usize>) -> Range<usize> {
        let start = self.index(display.start)
            + self
                .insertions
                .iter()
                .filter(|insertion| insertion.typed_before && insertion.at == display.start)
                .map(|insertion| insertion.shown.len())
                .sum::<usize>();
        let mut end = self.index(display.end);
        let inserted: usize = self
            .insertions
            .iter()
            .filter(|insertion| !insertion.typed_before && insertion.at == display.end)
            .map(|insertion| insertion.shown.len())
            .sum();
        if inserted > 0 {
            // The insertions there, and a composition after them.
            end -= inserted;
            if let Some(composing) = self
                .composing
                .as_ref()
                .filter(|_| self.composing_at == display.end)
            {
                end -= composing.len();
            }
        }
        start..end.max(start)
    }

    /// The display offset of index `index` in the shown text: inside a formula's gap, the
    /// nearer end of its macro; inside a composition or an insertion, where it is shown.
    #[must_use]
    pub fn display(&self, index: usize) -> usize {
        // What the shown text holds in place of display text, in the order it is shown, and
        // whether it stands for no display text.
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
        replaced.extend(
            self.insertions
                .iter()
                .map(|insertion| (insertion.shown.clone(), insertion.at..insertion.at, true)),
        );
        replaced.sort_by_key(|(shown, ..)| shown.start);
        let mut shown_extra = 0;
        let mut display_extra = 0;
        for (shown, display, point) in replaced {
            if index >= shown.end {
                shown_extra += shown.len();
                display_extra += display.len();
            } else if index > shown.start {
                return if point || index - shown.start <= shown.len() / 2 {
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
/// formula's gap. Each of `insertions` (a display offset, the text a pending suggestion would
/// insert there, and whether text typed there goes before it) is shown at its offset, or after
/// a formula whose macro holds it.
#[must_use]
pub fn shown(
    block: &TextBlock,
    composing: Option<(usize, &str)>,
    insertions: &[(usize, String, bool)],
    formulas: &FormulaGaps,
) -> Shown {
    let mut text = String::with_capacity(block.display.len());
    let mut shown_formulas = Vec::new();
    let mut after_break = false;
    for run in runs_of(block) {
        let part = &block.display[run.range.clone()];
        let formula = match run.atom {
            Some(Atom::Macro { name })
                if block.context != TextContext::Verbatim && is_formula(run.atom) =>
            {
                formulas.get(name, part)
            }
            _ => None,
        };
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
        insertions: Vec::new(),
    };
    let mut placed: Vec<(usize, &str, bool)> = insertions
        .iter()
        .filter(|(at, inserted, _)| {
            !inserted.is_empty()
                && *at <= block.display.len()
                && block.display.is_char_boundary(*at)
        })
        .map(|(at, inserted, typed_before)| {
            // Never inside a formula's macro, whose gap is drawn whole.
            let at = shown
                .formulas
                .iter()
                .find(|formula| formula.display.start < *at && *at < formula.display.end)
                .map_or(*at, |formula| formula.display.end);
            (at, inserted.as_str(), *typed_before)
        })
        .collect();
    // At one offset, those typed text follows before those it goes before.
    placed.sort_by_key(|(at, _, typed_before)| (*at, *typed_before));
    for (at, inserted, typed_before) in placed {
        // Shown as source text, on the block's lines: in flowing text a line break is a space.
        let inserted: String = inserted
            .chars()
            .map(|character| match character {
                '\n' if block.context == TextContext::Verbatim => '\n',
                '\n' | '\r' => ' ',
                other => other,
            })
            .collect();
        // After those already shown here, which `index` leaves out when typed text goes
        // before them.
        let index = shown.index(at)
            + shown
                .insertions
                .iter()
                .filter(|insertion| insertion.typed_before && insertion.at == at)
                .map(|insertion| insertion.shown.len())
                .sum::<usize>();
        if !shown.text.is_char_boundary(index) {
            continue;
        }
        shown.text.insert_str(index, &inserted);
        for formula in &mut shown.formulas {
            if formula.shown.start >= index {
                formula.shown.start += inserted.len();
                formula.shown.end += inserted.len();
            }
        }
        shown.insertions.push(ShownInsertion {
            at,
            shown: index..index + inserted.len(),
            typed_before,
        });
    }
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
        for insertion in &mut shown.insertions {
            if insertion.shown.start >= index {
                insertion.shown.start += composition.len();
                insertion.shown.end += composition.len();
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
    // What suggestions would insert, as plain text where it is shown; `with_suggestions`
    // colours it. The runs so far are of the text without it: placed in order, each goes
    // where it is shown.
    for insertion in &shown.insertions {
        let mut run = Vec::with_capacity(1);
        push(&mut run, insertion.shown.len(), MarkSet::EMPTY, None, false);
        runs = spliced(runs, insertion.shown.start, run);
    }
    if shown.text.is_empty() {
        // An empty block is shaped as a space, so the caret has a line to sit on.
        push(&mut runs, 1, MarkSet::EMPTY, None, false);
    }
    runs
}

/// `runs` with `inserted` placed at byte `at` of the text they cover, splitting the run there.
fn spliced(runs: Vec<TextRun>, at: usize, inserted: Vec<TextRun>) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + inserted.len() + 1);
    let mut start = 0;
    let mut inserted = Some(inserted);
    for run in runs {
        let end = start + run.len;
        if start < at && at < end {
            out.push(TextRun {
                len: at - start,
                ..run.clone()
            });
            out.extend(inserted.take().unwrap_or_default());
            out.push(TextRun {
                len: end - at,
                ..run
            });
        } else {
            if start == at {
                out.extend(inserted.take().unwrap_or_default());
            }
            out.push(run);
        }
        start = end;
    }
    out.extend(inserted.unwrap_or_default());
    out
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
    pub reveal: Option<(ListState, Rc<std::cell::Cell<bool>>)>,
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

/// Scrolls `list` so `caret` is inside its visible bounds.
fn scroll_into_view(list: &ListState, caret: Bounds<Pixels>) {
    let visible = list.viewport_bounds();
    let margin = caret.size.height;
    if caret.top() < visible.top() + margin {
        list.scroll_by(caret.top() - (visible.top() + margin));
    } else if caret.bottom() > visible.bottom() - margin {
        list.scroll_by(caret.bottom() - (visible.bottom() - margin));
    }
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

/// `runs` with a wavy underline in `colour` under each of `ranges` (indices into the shown
/// text), splitting runs where a range begins or ends.
#[must_use]
pub fn with_underlines(runs: Vec<TextRun>, ranges: &[Range<usize>], colour: Hsla) -> Vec<TextRun> {
    if ranges.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + ranges.len() * 2);
    let mut at = 0;
    for run in runs {
        let end = at + run.len;
        let mut cuts: Vec<usize> = ranges
            .iter()
            .flat_map(|range| [range.start, range.end])
            .filter(|cut| *cut > at && *cut < end)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(end);
        let mut start = at;
        for cut in cuts {
            let mut piece = run.clone();
            piece.len = cut - start;
            if ranges
                .iter()
                .any(|range| range.start <= start && cut <= range.end)
            {
                piece.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(colour),
                    wavy: true,
                });
            }
            out.push(piece);
            start = cut;
        }
        at = end;
    }
    out
}

/// `runs` with `colour` behind the parts in `ranges` (bytes of the same text): a commented
/// passage. The layout, the caret and the selection are unchanged.
#[must_use]
pub fn with_backgrounds(runs: Vec<TextRun>, ranges: &[Range<usize>], colour: Hsla) -> Vec<TextRun> {
    if ranges.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + ranges.len() * 2);
    let mut at = 0;
    for run in runs {
        let end = at + run.len;
        let mut cuts: Vec<usize> = ranges
            .iter()
            .flat_map(|range| [range.start, range.end])
            .filter(|cut| *cut > at && *cut < end)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(end);
        let mut start = at;
        for cut in cuts {
            let mut piece = run.clone();
            piece.len = cut - start;
            if ranges
                .iter()
                .any(|range| range.start <= start && cut <= range.end)
            {
                piece.background_color = Some(colour);
            }
            out.push(piece);
            start = cut;
        }
        at = end;
    }
    out
}

/// `runs` with a pending suggestion's marks (#763) in `colour`: the parts in `struck` struck
/// through, and what it would insert, the parts in `inserted`, in the colour and underlined
/// (indices into the shown text). The layout is unchanged.
#[must_use]
pub fn with_suggestions(
    runs: Vec<TextRun>,
    struck: &[Range<usize>],
    inserted: &[Range<usize>],
    colour: Hsla,
) -> Vec<TextRun> {
    if struck.is_empty() && inserted.is_empty() {
        return runs;
    }
    let within = |ranges: &[Range<usize>], start: usize, end: usize| {
        ranges
            .iter()
            .any(|range| range.start <= start && end <= range.end)
    };
    let mut out = Vec::with_capacity(runs.len() + (struck.len() + inserted.len()) * 2);
    let mut at = 0;
    for run in runs {
        let end = at + run.len;
        let mut cuts: Vec<usize> = struck
            .iter()
            .chain(inserted)
            .flat_map(|range| [range.start, range.end])
            .filter(|cut| *cut > at && *cut < end)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(end);
        let mut start = at;
        for cut in cuts {
            let mut piece = run.clone();
            piece.len = cut - start;
            if within(inserted, start, cut) {
                piece.color = colour;
                piece.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(colour),
                    wavy: false,
                });
            } else if within(struck, start, cut) {
                piece.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(colour),
                });
            }
            out.push(piece);
            start = cut;
        }
        at = end;
    }
    out
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
        let shown = shown(&block, None, &[], &FormulaGaps::new());
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
        let shown = shown(&block, None, &[], &FormulaGaps::new());
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
        let formulas = FormulaGaps {
            gaps: HashMap::from([(
                FormulaKey::of_macro("stem", "x^2", false),
                (gap, PathBuf::from("/x.svg")),
            )]),
            stem_latex: false,
        };

        let plain = shown(&block, None, &[], &formulas);
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
        let composing = shown(&block, Some((5, "é")), &[], &formulas);
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
        let before = shown(&block, Some((2, "é")), &[], &formulas);
        assert_eq!(before.text, "a é\u{a0}\u{a0} b");
        assert_eq!(before.formulas[0].shown, 4..8);
        assert_eq!(before.index(5), 8);
    }

    /// Issue #473: `stem:[x/2]` and `latexmath:[x/2]` are drawn with their own formulas.
    #[test]
    fn each_notation_draws_its_own_formula() {
        // `stem:[x/2] latexmath:[x/2]`, each macro showing its source.
        let atom = |name: &str, range| Piece::Atom {
            atom: Atom::Macro {
                name: name.to_owned(),
            },
            range,
            shown: "x/2".to_owned(),
        };
        let block = TextBlock {
            range: SourceRange::new(0, 26),
            pieces: vec![
                atom("stem", SourceRange::new(0, 10)),
                Piece::Text {
                    range: SourceRange::new(10, 11),
                },
                atom("latexmath", SourceRange::new(11, 26)),
            ],
            plain: false,
            display: "x/2 x/2".to_owned(),
            context: TextContext::Flow,
        };
        let gap = Gap {
            text: "\u{a0}".to_owned(),
            first: (1, px(4.)),
            width: px(4.),
            size: size(px(4.), px(10.)),
            baseline: px(7.),
        };
        let entry = |latex: bool, path: &str| {
            (
                FormulaKey {
                    latex,
                    source: "x/2".to_owned(),
                },
                (gap.clone(), PathBuf::from(path)),
            )
        };
        let both = FormulaGaps {
            gaps: HashMap::from([entry(false, "/ascii.svg"), entry(true, "/latex.svg")]),
            stem_latex: false,
        };
        let paths = |formulas: &FormulaGaps| {
            shown(&block, None, &[], formulas)
                .formulas
                .into_iter()
                .map(|formula| formula.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paths(&both),
            [PathBuf::from("/ascii.svg"), PathBuf::from("/latex.svg")]
        );
        // On a page whose `:stem:` is `latexmath`, `stem:` is LaTeX too.
        let latex_page = FormulaGaps {
            stem_latex: true,
            ..both.clone()
        };
        assert_eq!(
            paths(&latex_page),
            [PathBuf::from("/latex.svg"), PathBuf::from("/latex.svg")]
        );
        // `stem:` takes the other notation's formula when its own is not there: the page's
        // `:stem:` was changed after the header.
        let only_latex = FormulaGaps {
            gaps: HashMap::from([entry(true, "/latex.svg")]),
            stem_latex: false,
        };
        assert_eq!(
            paths(&only_latex),
            [PathBuf::from("/latex.svg"), PathBuf::from("/latex.svg")]
        );
        // `latexmath:` never does.
        let only_ascii = FormulaGaps {
            gaps: HashMap::from([entry(false, "/ascii.svg")]),
            stem_latex: false,
        };
        assert_eq!(paths(&only_ascii), [PathBuf::from("/ascii.svg")]);
    }

    #[test]
    fn underlines_split_the_runs_they_cover() {
        let run = TextRun {
            len: 10,
            font: style().font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let red = gpui::red();
        let runs = with_underlines(
            vec![run.clone(), TextRun { len: 4, ..run }],
            &[2..5, 9..12],
            red,
        );
        let lengths: Vec<usize> = runs.iter().map(|run| run.len).collect();
        assert_eq!(lengths, [2, 3, 4, 1, 2, 2]);
        let wavy: Vec<bool> = runs
            .iter()
            .map(|run| run.underline.is_some_and(|underline| underline.wavy))
            .collect();
        assert_eq!(wavy, [false, true, false, true, true, false]);
        assert_eq!(runs[1].underline.unwrap().color, Some(red));
    }

    #[test]
    fn comments_fill_the_runs_they_cover() {
        let run = TextRun {
            len: 10,
            font: style().font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let amber = gpui::yellow();
        let runs = with_backgrounds(
            vec![run.clone(), TextRun { len: 4, ..run }],
            &[2..5, 9..12],
            amber,
        );
        let lengths: Vec<usize> = runs.iter().map(|run| run.len).collect();
        assert_eq!(lengths, [2, 3, 4, 1, 2, 2]);
        let filled: Vec<bool> = runs
            .iter()
            .map(|run| run.background_color == Some(amber))
            .collect();
        assert_eq!(filled, [false, true, false, true, true, false]);
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), 14);
    }

    /// `a words b`, plain text.
    fn plain(display: &str) -> TextBlock {
        TextBlock {
            range: SourceRange::new(0, display.len()),
            pieces: vec![Piece::Text {
                range: SourceRange::new(0, display.len()),
            }],
            plain: true,
            display: display.to_owned(),
            context: TextContext::Flow,
        }
    }

    /// `a stem:[x^2] b`, the macro showing its source, three bytes, as a gap of four.
    fn with_formula() -> (TextBlock, FormulaGaps) {
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
        let formulas = FormulaGaps {
            gaps: HashMap::from([(
                FormulaKey::of_macro("stem", "x^2", false),
                (gap, PathBuf::from("/x.svg")),
            )]),
            stem_latex: false,
        };
        (block, formulas)
    }

    fn lengths(runs: &[TextRun]) -> Vec<usize> {
        runs.iter().map(|run| run.len).collect()
    }

    #[test]
    fn an_insertion_is_shown_but_never_in_the_display_text() {
        // `a words b`, with `text` suggested after `words`.
        let block = plain("a words b");
        let shown = shown(
            &block,
            None,
            &[(7, "text".to_owned(), false)],
            &FormulaGaps::new(),
        );
        assert_eq!(shown.text, "a wordstext b");
        assert_eq!(
            shown.insertions,
            [ShownInsertion {
                at: 7,
                shown: 7..11,
                typed_before: false,
            }]
        );
        assert_eq!(shown.display_len, 9, "the display text is the page's own");
        // Every display offset round-trips; those after it keep their places.
        for display in 0..=9 {
            assert_eq!(shown.display(shown.index(display)), display, "{display}");
        }
        assert_eq!(
            [0, 2, 6, 7, 8, 9].map(|display| shown.index(display)),
            [0, 2, 6, 11, 12, 13]
        );
        // A range ending where it is shown stops before it; one starting there, after it.
        assert_eq!(shown.span(2..7), 2..7);
        assert_eq!(shown.span(7..9), 11..13);
        // Its runs are part of the text's; in flowing text a line break is shown as a space.
        let runs = text_runs(&block, &shown, &style(), false);
        assert_eq!(lengths(&runs), [7, 4, 2]);
        let broken = super::shown(
            &block,
            None,
            &[(7, "a\nb".to_owned(), false)],
            &FormulaGaps::new(),
        );
        assert_eq!(broken.text, "a wordsa b b");
        // Nothing to insert, nothing shown.
        let empty = super::shown(
            &block,
            None,
            &[(7, String::new(), false)],
            &FormulaGaps::new(),
        );
        assert_eq!(empty.text, "a words b");
        assert!(empty.insertions.is_empty());
    }

    #[test]
    fn the_caret_steps_over_an_insertion() {
        let block = plain("a words b");
        let shown = shown(
            &block,
            None,
            &[(7, "text".to_owned(), false)],
            &FormulaGaps::new(),
        );
        // No display offset puts the caret inside it: from just before it to just after it.
        let carets: Vec<usize> = (0..=9).map(|display| shown.index(display)).collect();
        assert!(
            carets.iter().all(|index| !(7..11).contains(index)),
            "{carets:?}"
        );
        assert_eq!(shown.index(6), 6);
        assert_eq!(shown.index(7), 11);
        // A click anywhere on it goes to where it is shown.
        assert_eq!(
            [7, 8, 9, 10, 11].map(|index| shown.display(index)),
            [7, 7, 7, 7, 7]
        );
        assert_eq!(shown.display(12), 8);
    }

    #[test]
    fn suggestions_strike_and_underline_their_runs() {
        let block = plain("a words b");
        let shown = shown(
            &block,
            None,
            &[(7, "text".to_owned(), false)],
            &FormulaGaps::new(),
        );
        let runs = text_runs(&block, &shown, &style(), false);
        let blue = gpui::blue();
        let struck = shown.span(2..7);
        let inserted = shown.insertions[0].shown.clone();
        let runs = with_suggestions(runs, &[struck], &[inserted], blue);
        assert_eq!(lengths(&runs), [2, 5, 4, 2]);
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            shown.text.len()
        );
        let strikes: Vec<bool> = runs.iter().map(|run| run.strikethrough.is_some()).collect();
        assert_eq!(strikes, [false, true, false, false]);
        assert_eq!(runs[1].strikethrough.unwrap().color, Some(blue));
        assert_eq!(runs[1].color, style().color, "struck text keeps its colour");
        // What would be inserted is in the colour, underlined.
        assert_eq!(runs[2].color, blue);
        let underline = runs[2].underline.unwrap();
        assert_eq!(underline.color, Some(blue));
        assert!(!underline.wavy);
        assert!(runs[2].strikethrough.is_none());
        assert!(runs[3].underline.is_none());
        assert_eq!(runs[3].color, style().color);
    }

    #[test]
    fn an_insertion_beside_a_formula_and_a_composition_keeps_both_mappings() {
        let (block, formulas) = with_formula();
        // After the formula, with a composition at the same place: the gap, what would be
        // inserted, then the composition, where typed text will go.
        let shown = shown(
            &block,
            Some((5, "é")),
            &[(5, "+1".to_owned(), false)],
            &formulas,
        );
        assert_eq!(shown.text, "a \u{a0}\u{a0}+1é b");
        assert_eq!(shown.formulas[0].shown, 2..6);
        assert_eq!(
            shown.insertions,
            [ShownInsertion {
                at: 5,
                shown: 6..8,
                typed_before: false
            }]
        );
        assert_eq!(shown.composing, Some(8..10));
        assert_eq!(
            [0, 2, 3, 5, 6, 7].map(|display| shown.index(display)),
            [0, 2, 10, 10, 11, 12]
        );
        assert_eq!(
            [2, 3, 6, 7, 8, 9, 10, 11].map(|index| shown.display(index)),
            [2, 2, 5, 5, 5, 5, 5, 6]
        );
        // A range ending at the formula's end stops before the insertion and the composition.
        assert_eq!(shown.span(0..5), 0..6);
        let runs = text_runs(&block, &shown, &style(), false);
        assert_eq!(lengths(&runs), [2, 4, 2, 2, 2]);
        assert_eq!(runs[1].color, gpui::transparent_black(), "the gap");
        assert!(runs[3].underline.is_some(), "the composition");

        // Before the formula: the insertion, then the gap, which moves along.
        let before = super::shown(&block, None, &[(2, "+1".to_owned(), false)], &formulas);
        assert_eq!(before.text, "a +1\u{a0}\u{a0} b");
        assert_eq!(before.insertions[0].shown, 2..4);
        assert_eq!(before.formulas[0].shown, 4..8);
        assert_eq!(before.index(2), 4);
        assert_eq!(before.index(5), 8);
        assert_eq!(before.display(3), 2);
        assert_eq!(before.display(8), 5);
        assert_eq!(
            lengths(&text_runs(&block, &before, &style(), false)),
            [2, 2, 4, 2]
        );

        // Inside the macro's text: after the formula, never splitting its gap.
        let inside = super::shown(&block, None, &[(3, "+1".to_owned(), false)], &formulas);
        assert_eq!(inside.text, "a \u{a0}\u{a0}+1 b");
        assert_eq!(inside.insertions[0].at, 5);

        // A composition before an insertion moves it along.
        let composing = super::shown(
            &block,
            Some((0, "é")),
            &[(7, "+1".to_owned(), false)],
            &formulas,
        );
        assert_eq!(composing.text, "éa \u{a0}\u{a0} b+1");
        assert_eq!(composing.insertions[0].shown, 10..12);
        assert_eq!(composing.display(11), 7);
        assert_eq!(composing.index(7), 12);
        let runs = text_runs(&block, &composing, &style(), false);
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            composing.text.len()
        );
    }

    #[test]
    fn a_leading_insertion_keeps_the_caret_before_it() {
        // `a words b`, with `the ` suggested before `words`: text typed there goes before the
        // suggestion's range, so before what it would insert, and the caret is drawn there.
        let block = plain("a words b");
        let shown = shown(
            &block,
            None,
            &[(2, "the ".to_owned(), true)],
            &FormulaGaps::new(),
        );
        assert_eq!(shown.text, "a the words b");
        assert_eq!(shown.insertions[0].shown, 2..6);
        assert_eq!(shown.index(2), 2, "before `the `");
        assert_eq!(shown.index(3), 7);
        for display in 0..=9 {
            assert_eq!(shown.display(shown.index(display)), display, "{display}");
        }
        assert_eq!([3, 4, 5, 6].map(|index| shown.display(index)), [2, 2, 2, 2]);
        // A range ending there stops before it; one starting there starts after it.
        assert_eq!(shown.span(0..2), 0..2);
        assert_eq!(shown.span(2..7), 6..11);
        assert_eq!(
            lengths(&text_runs(&block, &shown, &style(), false)),
            [2, 4, 7]
        );

        // A composition there is drawn where it will be written: before the insertion.
        let composing = super::shown(
            &block,
            Some((2, "é")),
            &[(2, "the ".to_owned(), true)],
            &FormulaGaps::new(),
        );
        assert_eq!(composing.text, "a éthe words b");
        assert_eq!(composing.composing, Some(2..4));
        assert_eq!(composing.insertions[0].shown, 4..8);
        assert_eq!(composing.index(2), 4, "the caret after the composition");
        assert_eq!(composing.span(0..2), 0..4);

        // Beside one that typed text follows, at the same offset: that one, the caret, this one.
        let both = super::shown(
            &block,
            None,
            &[(2, "the ".to_owned(), true), (2, "X".to_owned(), false)],
            &FormulaGaps::new(),
        );
        assert_eq!(both.text, "a Xthe words b");
        assert_eq!(both.index(2), 3);
        assert_eq!(both.span(0..2), 0..2);
        assert_eq!(both.span(2..7), 7..12);
    }
}
