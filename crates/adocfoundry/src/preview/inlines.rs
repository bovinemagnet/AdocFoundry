//! Inline content: view inlines flattened into one styled text with clickable link ranges.

use std::{ops::Range, path::PathBuf, rc::Rc};

use adoc_render::{ResolvedLink, ViewInline};
use gpui::{
    px, AnyElement, App, ElementId, Font, FontStyle, FontWeight, Hsla, InteractiveText,
    IntoElement as _, SharedString, StyledText, TextRun, UnderlineStyle, Window,
};

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
#[derive(Clone)]
pub struct InlineStyle {
    pub font: Font,
    pub mono_family: SharedString,
    pub color: Hsla,
    pub muted: Hsla,
    pub link: Hsla,
    pub unresolved: Hsla,
    pub mark: Hsla,
    pub code_background: Hsla,
}

/// Inline content laid out as one string.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineText {
    pub text: String,
    pub runs: Vec<TextRun>,
    pub links: Vec<(Range<usize>, Link)>,
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
    let mut builder = Builder {
        style,
        out: InlineText::default(),
    };
    builder.inlines(inlines, state);
    builder.out
}

/// The element for laid-out text; link ranges call `on_link`.
pub fn element(id: ElementId, text: InlineText, on_link: &LinkHandler) -> AnyElement {
    if text.text.is_empty() {
        return gpui::Empty.into_any_element();
    }
    let styled = StyledText::new(text.text).with_runs(text.runs);
    if text.links.is_empty() {
        return styled.into_any_element();
    }
    let (ranges, links): (Vec<_>, Vec<_>) = text.links.into_iter().unzip();
    let on_link = on_link.clone();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |index, window, cx| {
            if let Some(link) = links.get(index) {
                // The enclosing block's click-to-source would otherwise undo the navigation.
                cx.stop_propagation();
                on_link(link, window, cx);
            }
        })
        .into_any_element()
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
}
