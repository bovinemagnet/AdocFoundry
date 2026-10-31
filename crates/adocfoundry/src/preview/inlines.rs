//! Inline content: view inlines flattened into one styled text with clickable link ranges.
//!
//! A typeset inline formula sits in the text as a gap of non-breaking spaces as wide as the
//! formula, which [`FormulaText`] paints the formula's image over once the text is laid out:
//! `StyledText` cannot hold an element, but its layout tells where each index is drawn.

use std::{ops::Range, path::PathBuf, rc::Rc};

use adoc_render::{FormulaImage, ResolvedLink, ViewInline};
use gpui::{
    div, img, point, prelude::FluentBuilder as _, px, size, AnyElement, App, AvailableSpace,
    Bounds, Element, ElementId, Font, FontStyle, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement as _, InteractiveText, IntoElement, LayoutId,
    ParentElement as _, Pixels, SharedString, Size, Styled as _, StyledText, TextLayout, TextRun,
    UnderlineStyle, Window,
};
use gpui_component::tooltip::Tooltip;

/// What a click on a stretch of inline text does.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Url(String),
    /// A cross reference, with the source span it was written at.
    Xref {
        link: ResolvedLink,
        source: adoc_core::Location,
    },
    /// An inline image opens in the system's viewer.
    File(PathBuf),
}

pub type LinkHandler = Rc<dyn Fn(&Link, &mut Window, &mut App)>;

/// The fonts and colours inline text is drawn with.
#[derive(Clone, PartialEq)]
pub struct InlineStyle {
    pub font: Font,
    pub mono_family: SharedString,
    pub color: Hsla,
    pub muted: Hsla,
    pub link: Hsla,
    pub unresolved: Hsla,
    pub mark: Hsla,
    pub code_background: Hsla,
    /// Behind a typeset formula, which is drawn black: white in a dark theme.
    pub formula_backing: Option<Hsla>,
}

/// Inline text laid out for the preview, by the id of its element.
pub type InlineCache = std::cell::RefCell<std::collections::HashMap<ElementId, InlineText>>;

/// Inline content laid out as one string.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineText {
    pub text: String,
    pub runs: Vec<TextRun>,
    pub links: Vec<(Range<usize>, Link)>,
    /// Typeset inline formulas, each at the index of `text` where its gap goes, between runs.
    pub formulas: Vec<InlineFormula>,
}

/// A typeset inline formula and where it goes in its text.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineFormula {
    pub at: usize,
    /// What its tooltip shows.
    pub source: String,
    pub image: FormulaImage,
    /// The font its gap is measured in.
    pub font: Font,
    pub backing: Option<Hsla>,
}

#[derive(Clone, Copy, Default)]
struct State {
    bold: bool,
    italic: bool,
    mono: bool,
    mark: bool,
    muted: bool,
    underline: bool,
    link: Option<LinkColour>,
}

#[derive(Clone, Copy)]
enum LinkColour {
    Resolved,
    Unresolved,
}

struct Builder<'s> {
    style: &'s InlineStyle,
    out: InlineText,
}

impl Builder<'_> {
    fn push(&mut self, text: &str, state: State) {
        if text.is_empty() {
            return;
        }
        let style = self.style;
        let mut font = if state.mono {
            Font {
                family: style.mono_family.clone(),
                ..style.font.clone()
            }
        } else {
            style.font.clone()
        };
        if state.bold {
            font.weight = FontWeight::BOLD;
        }
        if state.italic {
            font.style = FontStyle::Italic;
        }
        let color = match state.link {
            Some(LinkColour::Resolved) => style.link,
            Some(LinkColour::Unresolved) => style.unresolved,
            None if state.muted => style.muted,
            None => style.color,
        };
        let underline = (state.underline || state.link.is_some()).then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: matches!(state.link, Some(LinkColour::Unresolved)),
        });
        let background_color = if state.mark {
            Some(style.mark)
        } else if state.mono {
            Some(style.code_background)
        } else {
            None
        };
        self.out.text.push_str(text);
        self.out.runs.push(TextRun {
            len: text.len(),
            font,
            color,
            background_color,
            underline,
            strikethrough: None,
        });
    }

    fn inlines(&mut self, inlines: &[ViewInline], state: State) {
        for inline in inlines {
            self.inline(inline, state);
        }
    }

    fn linked(&mut self, link: Link, colour: LinkColour, content: &[ViewInline], state: State) {
        let start = self.out.text.len();
        self.inlines(
            content,
            State {
                link: Some(colour),
                ..state
            },
        );
        let end = self.out.text.len();
        if end > start {
            self.out.links.push((start..end, link));
        }
    }

    fn inline(&mut self, inline: &ViewInline, state: State) {
        match inline {
            ViewInline::Text(text) => self.push(text, state),
            ViewInline::Raw(text) => self.push(text, state),
            ViewInline::Strong(content) => self.inlines(
                content,
                State {
                    bold: true,
                    ..state
                },
            ),
            ViewInline::Emphasis(content) => self.inlines(
                content,
                State {
                    italic: true,
                    ..state
                },
            ),
            ViewInline::Code(content) => self.inlines(
                content,
                State {
                    mono: true,
                    ..state
                },
            ),
            ViewInline::Mark(content) => self.inlines(
                content,
                State {
                    mark: true,
                    ..state
                },
            ),
            ViewInline::Superscript(content) | ViewInline::Subscript(content) => {
                self.inlines(
                    content,
                    State {
                        muted: true,
                        ..state
                    },
                );
            }
            // A flow index term shows its text; a concealed one shows nothing.
            ViewInline::IndexTerm { content, .. } => self.inlines(content, state),
            ViewInline::Span { roles, content } => {
                let underline = roles.iter().any(|role| role == "underline");
                self.inlines(
                    content,
                    State {
                        underline: state.underline || underline,
                        ..state
                    },
                );
            }
            ViewInline::Link { url, content } => {
                self.linked(Link::Url(url.clone()), LinkColour::Resolved, content, state);
            }
            ViewInline::Xref {
                link,
                content,
                span,
                ..
            } => {
                let colour = if link.unresolved {
                    LinkColour::Unresolved
                } else {
                    LinkColour::Resolved
                };
                let source = adoc_core::Location {
                    path: span.path.clone(),
                    range: span.range,
                };
                self.linked(
                    Link::Xref {
                        link: link.clone(),
                        source,
                    },
                    colour,
                    content,
                    state,
                );
            }
            ViewInline::Anchor(_) => {}
            ViewInline::Image(image) => {
                let label = format!("[{}]", image.alt);
                match &image.path {
                    Some(path) => self.linked(
                        Link::File(path.clone()),
                        LinkColour::Resolved,
                        &[ViewInline::Text(label)],
                        state,
                    ),
                    None => self.push(
                        &label,
                        State {
                            link: Some(LinkColour::Unresolved),
                            ..state
                        },
                    ),
                }
            }
            ViewInline::Icon(name) => self.push(
                &format!("[{name}]"),
                State {
                    muted: true,
                    ..state
                },
            ),
            ViewInline::FootnoteRef(number) => {
                self.push(
                    &format!("[{number}]"),
                    State {
                        muted: true,
                        ..state
                    },
                );
            }
            ViewInline::LineBreak => self.push("\n", state),
            ViewInline::Keyboard(keys) => self.push(
                &keys.join("+"),
                State {
                    mono: true,
                    ..state
                },
            ),
            ViewInline::Button(label) => self.push(
                &format!("[{label}]"),
                State {
                    bold: true,
                    ..state
                },
            ),
            ViewInline::Menu(items) => self.push(
                &items.join(" › "),
                State {
                    bold: true,
                    ..state
                },
            ),
            ViewInline::Stem {
                source,
                rendered: Some(image),
                ..
            } => self.out.formulas.push(InlineFormula {
                at: self.out.text.len(),
                source: source.trim().to_owned(),
                image: image.clone(),
                font: self.style.font.clone(),
                backing: self.style.formula_backing,
            }),
            // Until it is typeset, or when it cannot be, a formula shows its source.
            ViewInline::Stem { source, .. } => self.push(
                source,
                State {
                    mono: true,
                    ..state
                },
            ),
        }
    }
}

/// Lays out `inlines` as one styled string.
#[must_use]
pub fn layout(inlines: &[ViewInline], style: &InlineStyle) -> InlineText {
    layout_with(inlines, style, State::default())
}

/// As [`layout`], all in bold: for headings and terms.
#[must_use]
pub fn layout_bold(inlines: &[ViewInline], style: &InlineStyle) -> InlineText {
    layout_with(
        inlines,
        style,
        State {
            bold: true,
            ..State::default()
        },
    )
}

/// As [`layout`], all in italics: for block titles.
#[must_use]
pub fn layout_italic(inlines: &[ViewInline], style: &InlineStyle) -> InlineText {
    layout_with(
        inlines,
        style,
        State {
            italic: true,
            ..State::default()
        },
    )
}

fn layout_with(inlines: &[ViewInline], style: &InlineStyle, state: State) -> InlineText {
    adoc_core::probe::hit(adoc_core::probe::Probe::InlineLayout);
    let mut builder = Builder {
        style,
        out: InlineText::default(),
    };
    builder.inlines(inlines, state);
    builder.out
}

/// The element for laid-out text; link ranges call `on_link`, and typeset formulas are drawn
/// in the line, their source in a tooltip.
pub fn element(id: ElementId, text: InlineText, on_link: &LinkHandler) -> AnyElement {
    if text.text.is_empty() && text.formulas.is_empty() {
        return gpui::Empty.into_any_element();
    }
    if text.formulas.is_empty() {
        return interactive(id, text, Vec::new(), on_link).0;
    }
    FormulaText {
        id,
        text: Some(text),
        on_link: on_link.clone(),
        inner: None,
        layout: None,
        placed: Vec::new(),
        images: Vec::new(),
    }
    .into_any_element()
}

/// `text` as an element, with its links and the tooltips of `formulas` (each a range of the
/// text and the source it shows), and the layout the text will have.
fn interactive(
    id: ElementId,
    text: InlineText,
    formulas: Vec<(Range<usize>, String)>,
    on_link: &LinkHandler,
) -> (AnyElement, TextLayout) {
    let styled = StyledText::new(text.text).with_runs(text.runs);
    let layout = styled.layout().clone();
    if text.links.is_empty() && formulas.is_empty() {
        return (styled.into_any_element(), layout);
    }
    let (ranges, links): (Vec<_>, Vec<_>) = text.links.into_iter().unzip();
    let on_link = on_link.clone();
    let mut element = InteractiveText::new(id, styled);
    if !ranges.is_empty() {
        element = element.on_click(ranges, move |index, window, cx| {
            if let Some(link) = links.get(index) {
                // The enclosing block's click-to-source would otherwise undo the navigation.
                cx.stop_propagation();
                on_link(link, window, cx);
            }
        });
    }
    if !formulas.is_empty() {
        element = element.tooltip(move |index, window, cx| {
            formulas
                .iter()
                .find(|(range, _)| range.contains(&index))
                .map(|(_, source)| Tooltip::new(source.clone()).build(window, cx))
        });
    }
    (element.into_any_element(), layout)
}

/// The size of type an inline formula is rendered at, in points, which the preview scales to
/// the size of the text beside it.
const FORMULA_POINTS: f32 = adoc_render::INLINE_FORMULA_POINTS;

/// A formula may be at most this many lines tall; a taller one is drawn smaller.
const FORMULA_MAX_LINES: f32 = 1.5;

/// The characters a formula's gap is made of: neither breaks a line, so the gap moves to the
/// next line whole.
const WIDE: char = '\u{a0}';
const NARROW: char = '\u{202f}';

/// How many [`WIDE`] and [`NARROW`] characters, `wide` and `narrow` pixels across, come
/// closest to `width`: at least one, so the formula always has a place.
#[must_use]
pub fn glue(width: f32, wide: f32, narrow: f32) -> (usize, usize) {
    if !(wide > 0.0 && narrow > 0.0 && width.is_finite()) {
        return (1, 0);
    }
    let mut best = (1, 0);
    let mut best_error = (wide - width).abs();
    let most = (width / wide).ceil().max(0.0) as usize + 1;
    for wides in 0..=most {
        let rest = width - wides as f32 * wide;
        let narrows = ((rest / narrow).round().max(0.0) as usize).max(usize::from(wides == 0));
        let error = (wides as f32 * wide + narrows as f32 * narrow - width).abs();
        if error < best_error {
            best = (wides, narrows);
            best_error = error;
        }
    }
    best
}

/// The size a formula is drawn at, and its baseline from its top, in pixels: type the size of
/// `font_size`, but no taller than [`FORMULA_MAX_LINES`] lines.
#[must_use]
pub fn drawn_size(
    image: &FormulaImage,
    font_size: f32,
    line_height: f32,
) -> (Size<Pixels>, Pixels) {
    let mut scale = font_size / FORMULA_POINTS;
    let most = FORMULA_MAX_LINES * line_height;
    if image.height * scale > most && image.height > 0.0 {
        scale = most / image.height;
    }
    (
        size(px(image.width * scale), px(image.height * scale)),
        px(image.baseline * scale),
    )
}

/// `text` with each formula's gap put in at its index, a run of its own drawn in nothing, and
/// the links moved to match; with each gap's range.
#[must_use]
pub fn splice(text: InlineText, gaps: &[String]) -> (InlineText, Vec<Range<usize>>) {
    // Where a gap goes and how long it is, in the order of the text.
    let inserts: Vec<(usize, usize)> = text
        .formulas
        .iter()
        .zip(gaps)
        .map(|(formula, gap)| (formula.at, gap.len()))
        .collect();
    let shifted = |index: usize, inclusive: bool| {
        index
            + inserts
                .iter()
                .filter(|(at, _)| if inclusive { *at <= index } else { *at < index })
                .map(|(_, len)| len)
                .sum::<usize>()
    };

    let mut out = InlineText::default();
    let mut ranges = Vec::new();
    let mut gaps_left = text.formulas.iter().zip(gaps).peekable();
    let mut insert_due = |at: usize, out: &mut InlineText| {
        while let Some((formula, gap)) = gaps_left.next_if(|(formula, _)| formula.at <= at) {
            let start = out.text.len();
            out.text.push_str(gap);
            out.runs.push(TextRun {
                len: gap.len(),
                font: formula.font.clone(),
                color: gpui::transparent_black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            ranges.push(start..out.text.len());
        }
    };
    let mut at = 0;
    for run in text.runs {
        insert_due(at, &mut out);
        out.text.push_str(&text.text[at..at + run.len]);
        at += run.len;
        out.runs.push(run);
    }
    insert_due(usize::MAX, &mut out);
    out.links = text
        .links
        .into_iter()
        .map(|(range, link)| (shifted(range.start, true)..shifted(range.end, false), link))
        .collect();
    (out, ranges)
}

/// A formula's gap and how it is drawn over it, measured in the text's font at its size.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    /// The characters that hold the formula's place.
    pub text: String,
    /// The width of the first of them, and of all of them.
    pub first: (usize, Pixels),
    pub width: Pixels,
    /// The size the formula is drawn at, and its baseline from its top.
    pub size: Size<Pixels>,
    pub baseline: Pixels,
}

impl Gap {
    /// The gap for `image` in `font`, as the window's text style sets its size.
    pub fn measure(image: &FormulaImage, font: &Font, window: &Window) -> Self {
        adoc_core::probe::hit(adoc_core::probe::Probe::MeasureFormula);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style.line_height_in_pixels(window.rem_size());
        let advance = |character: char| {
            let mut buffer = [0; 4];
            let character = character.encode_utf8(&mut buffer);
            window
                .text_system()
                .layout_line(
                    character,
                    font_size,
                    &[TextRun {
                        len: character.len(),
                        font: font.clone(),
                        color: gpui::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
                .width
        };
        let (drawn, baseline) = drawn_size(image, f32::from(font_size), f32::from(line_height));
        let (wide, narrow) = (advance(WIDE), advance(NARROW));
        let (wides, narrows) = glue(f32::from(drawn.width), f32::from(wide), f32::from(narrow));
        let mut text = WIDE.to_string().repeat(wides);
        text.push_str(&NARROW.to_string().repeat(narrows));
        Self {
            text,
            first: if wides > 0 {
                (WIDE.len_utf8(), wide)
            } else {
                (NARROW.len_utf8(), narrow)
            },
            width: wide * wides as f32 + narrow * narrows as f32,
            size: drawn,
            baseline,
        }
    }
}

/// The image of a formula whose gap starts at index `start` of `layout`, laid out and prepainted
/// on the gap's line with its baseline on the line's: for the paint pass to paint. `None` when
/// the gap is not in the layout.
pub fn place_formula(
    layout: &TextLayout,
    start: usize,
    gap: &Gap,
    path: &std::path::Path,
    backing: Option<Hsla>,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    // After the gap's first character, which is on the gap's line even when the gap starts one:
    // the index before it may be the end of the line above.
    let after_first = start + gap.first.0;
    let position = layout.position_for_index(after_first)?;
    let line = layout.line_layout_for_index(after_first)?;
    let line = &line.unwrapped_layout;
    let padding = (layout.line_height() - line.ascent - line.descent) / 2.;
    let baseline = position.y + padding + line.ascent;
    let origin = point(
        position.x - gap.first.1 + (gap.width - gap.size.width) / 2.,
        baseline - gap.baseline,
    );
    let mut image = div()
        .debug_selector(|| "stem-inline".to_owned())
        .w(gap.size.width)
        .h(gap.size.height)
        .when_some(backing, |this, backing| this.bg(backing).rounded(px(2.)))
        .child(img(path.to_path_buf()).size_full())
        .into_any_element();
    image.prepaint_as_root(
        origin,
        size(
            AvailableSpace::Definite(gap.size.width),
            AvailableSpace::Definite(gap.size.height),
        ),
        window,
        cx,
    );
    Some(image)
}

/// What a text's formula gaps were measured for: each formula's image and font, and the size
/// of the text.
#[derive(PartialEq)]
struct GapKey {
    formulas: Vec<(FormulaImage, Font)>,
    font_size: Pixels,
    line_height: Pixels,
}

/// The gaps of `formulas` in the text of element `id`, measured again only when the formulas
/// or the text's size changed since the last frame.
fn measured_gaps(
    id: Option<&GlobalElementId>,
    formulas: &[InlineFormula],
    window: &mut Window,
) -> Vec<Gap> {
    let style = window.text_style();
    let key = GapKey {
        formulas: formulas
            .iter()
            .map(|formula| (formula.image.clone(), formula.font.clone()))
            .collect(),
        font_size: style.font_size.to_pixels(window.rem_size()),
        line_height: style.line_height_in_pixels(window.rem_size()),
    };
    window.with_optional_element_state(id, |kept: Option<Option<(GapKey, Vec<Gap>)>>, window| {
        if let Some((kept_key, gaps)) = kept.flatten().filter(|(kept, _)| *kept == key) {
            return (gaps.clone(), Some((kept_key, gaps)));
        }
        let gaps: Vec<Gap> = formulas
            .iter()
            .map(|formula| Gap::measure(&formula.image, &formula.font, window))
            .collect();
        (gaps.clone(), Some((key, gaps)))
    })
}

/// A formula placed in the text: where its gap is, and how it is drawn.
struct Placed {
    start: usize,
    gap: Gap,
    path: PathBuf,
    backing: Option<Hsla>,
}

/// Text holding typeset formulas: the text is laid out with a gap for each, and each formula's
/// image is painted over its gap, on the line's baseline.
pub struct FormulaText {
    id: ElementId,
    /// Until it is laid out.
    text: Option<InlineText>,
    on_link: LinkHandler,
    inner: Option<AnyElement>,
    layout: Option<TextLayout>,
    placed: Vec<Placed>,
    images: Vec<AnyElement>,
}

impl IntoElement for FormulaText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FormulaText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let text = self.text.take().unwrap_or_default();
        let gaps = measured_gaps(id, &text.formulas, window);
        let formulas = text.formulas.clone();
        let texts: Vec<String> = gaps.iter().map(|gap| gap.text.clone()).collect();
        let (spliced, ranges) = splice(text, &texts);
        let tooltips = ranges
            .iter()
            .zip(&formulas)
            .map(|(range, formula)| (range.clone(), formula.source.clone()))
            .collect();
        self.placed = formulas
            .into_iter()
            .zip(ranges)
            .zip(gaps)
            .map(|((formula, range), gap)| Placed {
                start: range.start,
                gap,
                path: formula.image.path,
                backing: formula.backing,
            })
            .collect();
        let (mut inner, layout) = interactive(self.id.clone(), spliced, tooltips, &self.on_link);
        let layout_id = inner.request_layout(window, cx);
        self.inner = Some(inner);
        self.layout = Some(layout);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (Some(inner), Some(layout)) = (self.inner.as_mut(), self.layout.as_ref()) else {
            return;
        };
        inner.prepaint(window, cx);
        self.images = self
            .placed
            .iter()
            .filter_map(|placed| {
                place_formula(
                    layout,
                    placed.start,
                    &placed.gap,
                    &placed.path,
                    placed.backing,
                    window,
                    cx,
                )
            })
            .collect();
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(inner) = self.inner.as_mut() {
            inner.paint(window, cx);
        }
        for image in &mut self.images {
            image.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use adoc_render::{ResolvedLink, ViewInline};
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
    fn runs_cover_the_text_and_links_are_ranges() {
        let inlines = vec![
            ViewInline::Text("See ".into()),
            ViewInline::Strong(vec![ViewInline::Text("bold".into())]),
            ViewInline::Text(" and ".into()),
            ViewInline::Xref {
                target: "other.adoc".into(),
                link: ResolvedLink {
                    unresolved: true,
                    ..ResolvedLink::default()
                },
                content: vec![ViewInline::Text("other".into())],
                span: adoc_render::SourceSpan::default(),
            },
            ViewInline::Code(vec![ViewInline::Text("x".into())]),
        ];
        let laid = layout(&inlines, &style());
        assert_eq!(laid.text, "See bold and otherx");
        assert_eq!(
            laid.runs.iter().map(|run| run.len).sum::<usize>(),
            laid.text.len()
        );
        assert_eq!(laid.runs[1].font.weight, FontWeight::BOLD);
        assert_eq!(laid.runs.last().unwrap().font.family.as_ref(), "Mono");
        assert_eq!(laid.links.len(), 1);
        assert_eq!(&laid.text[laid.links[0].0.clone()], "other");
        let unresolved = &laid.runs[3];
        assert!(unresolved.underline.is_some_and(|underline| underline.wavy));
    }

    fn formula(width: f32, height: f32, baseline: f32) -> ViewInline {
        ViewInline::Stem {
            latex: false,
            source: " x^2 ".into(),
            rendered: Some(FormulaImage {
                path: PathBuf::from("/cache/x.svg"),
                width,
                height,
                baseline,
            }),
        }
    }

    #[test]
    fn inline_stem_reserves_its_width() {
        let inlines = vec![
            ViewInline::Text("a ".into()),
            formula(14.7, 12.0, 9.5),
            ViewInline::Text(" b ".into()),
            ViewInline::Link {
                url: "https://example.org".into(),
                content: vec![ViewInline::Text("link".into())],
            },
            ViewInline::Stem {
                latex: false,
                source: "y".into(),
                rendered: None,
            },
        ];
        let laid = layout(&inlines, &style());
        // The typeset formula holds a place between runs; the pending one shows its source.
        assert_eq!(laid.text, "a  b linky");
        assert_eq!(laid.formulas.len(), 1);
        assert_eq!(laid.formulas[0].at, 2);
        assert_eq!(laid.formulas[0].source, "x^2");

        // Drawn at 16 px type, 13 pt in the image become 16 px.
        let (drawn, baseline) = drawn_size(&laid.formulas[0].image, 16.0, 24.0);
        assert!((f32::from(drawn.width) - 14.7 * 16.0 / 13.0).abs() < 1e-3);
        assert!((f32::from(baseline) - 9.5 * 16.0 / 13.0).abs() < 1e-3);
        let (wide, narrow) = (4.0, 3.2);
        let (wides, narrows) = glue(f32::from(drawn.width), wide, narrow);
        let gap_width = wides as f32 * wide + narrows as f32 * narrow;
        assert!(
            (gap_width - f32::from(drawn.width)).abs() <= 0.5,
            "{gap_width} for {drawn:?}"
        );

        let gap = format!(
            "{}{}",
            WIDE.to_string().repeat(wides),
            NARROW.to_string().repeat(narrows)
        );
        let (spliced, ranges) = splice(laid, std::slice::from_ref(&gap));
        assert_eq!(spliced.text, format!("a {gap} b linky"));
        assert_eq!(ranges, vec![2..2 + gap.len()]);
        assert_eq!(
            spliced.runs.iter().map(|run| run.len).sum::<usize>(),
            spliced.text.len()
        );
        // The gap is drawn in nothing, and the link still covers its text.
        assert_eq!(spliced.runs[1].len, gap.len());
        assert_eq!(spliced.runs[1].color, gpui::transparent_black());
        assert_eq!(&spliced.text[spliced.links[0].0.clone()], "link");
    }

    #[test]
    fn tall_formulas_are_drawn_smaller_and_gaps_are_never_empty() {
        let image = FormulaImage {
            path: PathBuf::new(),
            width: 20.0,
            height: 40.0,
            baseline: 25.0,
        };
        let (drawn, baseline) = drawn_size(&image, 13.0, 20.0);
        assert_eq!(f32::from(drawn.height), 30.0);
        assert_eq!(f32::from(drawn.width), 15.0);
        assert_eq!(f32::from(baseline), 18.75);
        assert_eq!(glue(0.5, 4.0, 3.2), (0, 1));
        assert_eq!(glue(f32::NAN, 4.0, 3.2), (1, 0));
        assert_eq!(glue(8.0, 4.0, 3.2), (2, 0));
    }
}
