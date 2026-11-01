//! Blocks: each view block becomes an element. Clicking a block reports its source span; the
//! innermost block under the pointer wins.

use std::{ops::Range, rc::Rc};

use adoc_render::{
    DiagramView, ListKind, MediaKind, SourceSpan, ViewBlock, ViewImage, ViewInline, ViewKind,
    ViewTable, ViewTree,
};
use gpui::{
    div, img, prelude::FluentBuilder as _, px, relative, AnyElement, App, ElementId, FontWeight,
    HighlightStyle, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Window,
};
use gpui_component::{h_flex, v_flex};

use super::inlines::{self, InlineStyle, LinkHandler};

pub type BlockHandler = Rc<dyn Fn(&SourceSpan, &mut Window, &mut App)>;
pub type Highlighter<'a> = &'a dyn Fn(&str, &str) -> super::Highlights;

/// Colours for block decoration.
#[derive(Clone)]
pub struct Palette {
    pub border: Hsla,
    pub surface: Hsla,
    pub muted: Hsla,
    pub note: Hsla,
    pub tip: Hsla,
    pub important: Hsla,
    pub warning: Hsla,
    pub caution: Hsla,
    pub danger: Hsla,
}

pub struct BlockContext<'a> {
    pub inline: InlineStyle,
    pub extents: super::sync::Extents,
    /// The list the blocks are drawn in, when their extents are recorded.
    pub list: Option<gpui::ListState>,
    pub palette: Palette,
    pub radius: Pixels,
    pub on_link: LinkHandler,
    pub on_block: BlockHandler,
    pub highlight: Highlighter<'a>,
    /// Inline text as laid out in earlier frames, when the view keeps it.
    pub inline_cache: Option<Rc<inlines::InlineCache>>,
}

/// The element id of the block at `trail`, as [`adoc_render::block_at`] returns it.
#[must_use]
pub fn block_id(trail: &[usize]) -> ElementId {
    let path: Vec<String> = trail.iter().map(usize::to_string).collect();
    ElementId::Name(SharedString::from(format!("block-{}", path.join("-"))))
}

fn text_id(trail: &[usize], part: &str) -> ElementId {
    let path: Vec<String> = trail.iter().map(usize::to_string).collect();
    ElementId::Name(SharedString::from(format!(
        "text-{}-{part}",
        path.join("-")
    )))
}

/// A row of the list the preview draws: the title, the contents, a block, or a section's
/// heading alone, whose blocks are the rows after it, so that a page of sections is laid out a
/// block at a time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Row {
    Title,
    Contents,
    Block {
        trail: Vec<usize>,
        /// Only the section's heading.
        heading: bool,
    },
}

/// The rows of `view`, in document order.
#[must_use]
pub fn rows(view: &ViewTree) -> Vec<Row> {
    fn visit(blocks: &[&ViewBlock], trail: &mut Vec<usize>, out: &mut Vec<Row>) {
        for (index, block) in blocks.iter().enumerate() {
            trail.push(index);
            let section = matches!(block.kind, ViewKind::Section { .. });
            out.push(Row::Block {
                trail: trail.clone(),
                heading: section,
            });
            if section {
                visit(&block.child_refs(), trail, out);
            }
            trail.pop();
        }
    }
    let mut out = Vec::new();
    if view.title.is_some() {
        out.push(Row::Title);
    }
    // `:toc: macro` places the contents at a `toc::[]` block instead.
    if !view.toc.is_empty() && view.attributes.get("toc").map(String::as_str) != Some("macro") {
        out.push(Row::Contents);
    }
    let roots: Vec<&ViewBlock> = view.blocks.iter().collect();
    visit(&roots, &mut Vec::new(), &mut out);
    out
}

/// The index of the row holding the block at `trail`.
#[must_use]
pub fn row_of(rows: &[Row], trail: &[usize]) -> Option<usize> {
    // The title and contents come first, then blocks in the order of their trails: the last
    // at or before `trail` is the one holding it.
    rows.partition_point(|row| match row {
        Row::Block { trail: other, .. } => other.as_slice() <= trail,
        Row::Title | Row::Contents => true,
    })
    .checked_sub(1)
    .filter(|index| matches!(rows[*index], Row::Block { .. }))
}

/// Row `index` of `rows`, the rows of `view`.
pub fn row(view: &ViewTree, rows: &[Row], index: usize, context: &BlockContext<'_>) -> AnyElement {
    let Some(row) = rows.get(index) else {
        return gpui::Empty.into_any_element();
    };
    let element = match row {
        Row::Title => view.title.as_ref().map(|title| {
            div()
                .text_3xl()
                .pb_2()
                .child(inline_text("document-title".into(), context, || {
                    inlines::layout_bold(title, &context.inline)
                }))
                .into_any_element()
        }),
        Row::Contents => Some(toc_element(view, context)),
        Row::Block { trail, heading } => adoc_render::block_by_trail(view, trail).map(|block| {
            match section_heading(block, trail, context) {
                Some(title) if *heading => framed(block, trail, title, context),
                _ => block_element(block, &mut trail.clone(), view, context),
            }
        }),
    };
    div()
        .w_full()
        .max_w(px(900.))
        .px_4()
        .pt(row_gap(rows, index))
        .when(index + 1 == rows.len(), |row| row.pb_4())
        .children(element)
        .into_any_element()
}

/// The space above row `index`: the page's margin above the first, then the gap a column of the
/// page's blocks (`gap_3`) or a section's (`gap_2`) puts between them.
#[must_use]
pub fn row_gap(rows: &[Row], index: usize) -> Pixels {
    let in_section = matches!(rows.get(index), Some(Row::Block { trail, .. }) if trail.len() > 1);
    if index == 0 {
        px(16.)
    } else if in_section {
        px(8.)
    } else {
        px(12.)
    }
}

/// One block of `view` at `trail`, as the preview draws it: for content the visual editor shows
/// but does not edit.
pub fn block(
    block: &ViewBlock,
    trail: &[usize],
    view: &ViewTree,
    context: &BlockContext<'_>,
) -> AnyElement {
    block_element(block, &mut trail.to_vec(), view, context)
}

fn children(
    blocks: &[&ViewBlock],
    trail: &mut Vec<usize>,
    view: &ViewTree,
    context: &BlockContext<'_>,
) -> Vec<AnyElement> {
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            trail.push(index);
            let element = block_element(block, trail, view, context);
            trail.pop();
            element
        })
        .collect()
}

fn text(
    content: &[ViewInline],
    trail: &[usize],
    part: &str,
    context: &BlockContext<'_>,
) -> AnyElement {
    inline_text(text_id(trail, part), context, || {
        inlines::layout(content, &context.inline)
    })
}

/// The element for inline text laid out by `lay_out`, or as it was laid out for the element
/// `id` while the preview shows the same view tree in the same style.
fn inline_text(
    id: ElementId,
    context: &BlockContext<'_>,
    lay_out: impl FnOnce() -> inlines::InlineText,
) -> AnyElement {
    let text = match &context.inline_cache {
        Some(cache) => {
            let kept = cache.borrow().get(&id).cloned();
            kept.unwrap_or_else(|| {
                let text = lay_out();
                cache.borrow_mut().insert(id.clone(), text.clone());
                text
            })
        }
        None => lay_out(),
    };
    inlines::element(id, text, &context.on_link)
}

pub(crate) fn title(
    block: &ViewBlock,
    trail: &[usize],
    context: &BlockContext<'_>,
) -> Option<AnyElement> {
    if block.title.is_empty() {
        return None;
    }
    let mut content = Vec::new();
    if let Some(caption) = &block.caption {
        content.push(ViewInline::Text(format!("{caption} ")));
    }
    content.extend(block.title.iter().cloned());
    Some(
        div()
            .text_sm()
            .pb_1()
            .child(inline_text(text_id(trail, "title"), context, || {
                inlines::layout_italic(&content, &context.inline)
            }))
            .into_any_element(),
    )
}

fn block_element(
    block: &ViewBlock,
    trail: &mut Vec<usize>,
    view: &ViewTree,
    context: &BlockContext<'_>,
) -> AnyElement {
    let palette = &context.palette;
    let child_refs = block.child_refs();
    let body: AnyElement = match &block.kind {
        ViewKind::Section { .. } => v_flex()
            .gap_2()
            .children(section_heading(block, trail, context))
            .children(children(&child_refs, trail, view, context))
            .into_any_element(),
        ViewKind::Heading {
            level,
            title: heading,
        } => heading_element(*level, heading, trail, context),
        ViewKind::Paragraph(content) => v_flex()
            .children(title(block, trail, context))
            .child(text(content, trail, "body", context))
            .into_any_element(),
        ViewKind::List { kind, items } => {
            let mut nested = 0;
            v_flex()
                .gap_1()
                .children(title(block, trail, context))
                .children(items.iter().enumerate().map(|(index, item)| {
                    let marker = match (kind, item.checked) {
                        (_, Some(true)) => "☑".to_owned(),
                        (_, Some(false)) => "☐".to_owned(),
                        (ListKind::Ordered, None) => format!("{}.", index + 1),
                        _ => "•".to_owned(),
                    };
                    let item_children: Vec<&ViewBlock> = item.blocks.iter().collect();
                    let first = nested;
                    nested += item_children.len();
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().min_w(px(18.)).child(marker))
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_1()
                                .child(text(&item.content, trail, &format!("item{index}"), context))
                                .children(offset_children(
                                    &item_children,
                                    first,
                                    trail,
                                    view,
                                    context,
                                )),
                        )
                }))
                .into_any_element()
        }
        ViewKind::DescriptionList(items) => {
            let mut nested = 0;
            v_flex()
                .gap_1()
                .children(title(block, trail, context))
                .children(items.iter().enumerate().map(|(index, item)| {
                    let item_children: Vec<&ViewBlock> = item.blocks.iter().collect();
                    let first = nested;
                    nested += item_children.len();
                    v_flex()
                        .child(inline_text(
                            text_id(trail, &format!("term{index}")),
                            context,
                            || inlines::layout_bold(&item.term, &context.inline),
                        ))
                        .child(
                            v_flex()
                                .pl_4()
                                .gap_1()
                                .child(text(&item.content, trail, &format!("dd{index}"), context))
                                .children(offset_children(
                                    &item_children,
                                    first,
                                    trail,
                                    view,
                                    context,
                                )),
                        )
                }))
                .into_any_element()
        }
        ViewKind::CalloutList(items) => {
            let mut nested = 0;
            v_flex()
                .gap_1()
                .children(items.iter().enumerate().map(|(index, item)| {
                    let item_children: Vec<&ViewBlock> = item.blocks.iter().collect();
                    let first = nested;
                    nested += item_children.len();
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(callout_badge(item.number, context))
                        .child(
                            v_flex()
                                .flex_1()
                                .child(text(&item.content, trail, &format!("co{index}"), context))
                                .children(offset_children(
                                    &item_children,
                                    first,
                                    trail,
                                    view,
                                    context,
                                )),
                        )
                }))
                .into_any_element()
        }
        ViewKind::Admonition { kind, .. } => {
            let colour = match kind {
                adoc_render::AdmonitionKind::Note => palette.note,
                adoc_render::AdmonitionKind::Tip => palette.tip,
                adoc_render::AdmonitionKind::Important => palette.important,
                adoc_render::AdmonitionKind::Warning => palette.warning,
                adoc_render::AdmonitionKind::Caution => palette.caution,
            };
            h_flex()
                .items_start()
                .gap_3()
                .p_2()
                .border_l_4()
                .border_color(colour)
                .bg(palette.surface)
                .rounded(context.radius)
                .child(
                    div()
                        .min_w(px(80.))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(colour)
                        .child(kind.label()),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .children(title(block, trail, context))
                        .children(children(&child_refs, trail, view, context)),
                )
                .into_any_element()
        }
        ViewKind::Code {
            language,
            text: code,
            callouts,
        } => {
            let styles = language.as_deref().map_or_else(
                || Rc::from([]),
                |language| (context.highlight)(language, code),
            );
            v_flex()
                .children(title(block, trail, context))
                .child(
                    v_flex()
                        .p_2()
                        .bg(palette.surface)
                        .border_1()
                        .border_color(palette.border)
                        .rounded(context.radius)
                        .font_family(context.inline.mono_family.clone())
                        .text_sm()
                        .children(code_lines(code, &styles, callouts, context)),
                )
                .into_any_element()
        }
        ViewKind::Literal(literal) => v_flex()
            .children(title(block, trail, context))
            .child(preformatted(literal, context))
            .into_any_element(),
        ViewKind::Quote {
            attribution,
            citation,
            ..
        } => v_flex()
            .children(title(block, trail, context))
            .child(
                v_flex()
                    .pl_4()
                    .border_l_4()
                    .border_color(palette.border)
                    .italic()
                    .gap_2()
                    .children(children(&child_refs, trail, view, context)),
            )
            .children(attribution_line(
                attribution.as_deref(),
                citation.as_deref(),
                palette,
            ))
            .into_any_element(),
        ViewKind::Verse {
            attribution,
            citation,
            text: verse,
        } => v_flex()
            .children(title(block, trail, context))
            .child(
                div()
                    .pl_4()
                    .border_l_4()
                    .border_color(palette.border)
                    .italic()
                    .whitespace_normal()
                    .child(verse.clone()),
            )
            .children(attribution_line(
                attribution.as_deref(),
                citation.as_deref(),
                palette,
            ))
            .into_any_element(),
        ViewKind::Sidebar(_) => container(block, trail, view, context, palette.surface, true),
        ViewKind::Example(_) => {
            container(block, trail, view, context, Hsla::transparent_black(), true)
        }
        ViewKind::Open(_) => container(
            block,
            trail,
            view,
            context,
            Hsla::transparent_black(),
            false,
        ),
        ViewKind::Table(table) => v_flex()
            .children(title(block, trail, context))
            .child(table_element(table, trail, view, context))
            .into_any_element(),
        ViewKind::Image(image) => v_flex()
            .gap_1()
            .child(image_element(image, context))
            .children(title(block, trail, context))
            .into_any_element(),
        ViewKind::Media { kind, targets } => {
            let label = match kind {
                MediaKind::Audio => "Audio",
                MediaKind::Video => "Video",
            };
            placeholder(
                &format!("{label}: {}", targets.join(", ")),
                palette.muted,
                context,
            )
        }
        ViewKind::Diagram(diagram) => v_flex()
            .gap_1()
            .child(match diagram {
                DiagramView::Svg(path) => img(path.clone()).max_w_full().into_any_element(),
                DiagramView::Pending { kind, source } => v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette.muted)
                            .child(format!("Rendering {kind} diagram…")),
                    )
                    .child(preformatted(source, context))
                    .into_any_element(),
                DiagramView::Failed {
                    message, source, ..
                } => v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette.danger)
                            .child(message.clone()),
                    )
                    .child(preformatted(source, context))
                    .into_any_element(),
            })
            .children(title(block, trail, context))
            .into_any_element(),
        ViewKind::Stem {
            source, rendered, ..
        } => v_flex()
            .children(title(block, trail, context))
            .child(match rendered {
                // Typeset black on a white card, readable in either theme, as images are.
                DiagramView::Svg(path) => h_flex()
                    .w_full()
                    .justify_center()
                    .child(
                        div()
                            .debug_selector(|| "stem-image".to_owned())
                            .p_1()
                            .rounded(context.radius)
                            .bg(gpui::white())
                            .child(img(path.clone()).max_w_full()),
                    )
                    .into_any_element(),
                DiagramView::Failed { message, .. } => v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette.muted)
                            .child(message.clone()),
                    )
                    .child(preformatted(source, context))
                    .into_any_element(),
                DiagramView::Pending { .. } => preformatted(source, context),
            })
            .into_any_element(),
        ViewKind::Passthrough(content) => preformatted(content, context),
        ViewKind::ThematicBreak => div().h(px(1.)).my_2().bg(palette.border).into_any_element(),
        ViewKind::PageBreak => div()
            .my_2()
            .border_t_1()
            .border_dashed()
            .border_color(palette.border)
            .into_any_element(),
        ViewKind::Toc => toc_element(view, context),
        ViewKind::Unsupported { name, source } => v_flex()
            .gap_1()
            .p_2()
            .border_1()
            .border_dashed()
            .border_color(palette.warning)
            .rounded(context.radius)
            .child(
                div()
                    .text_xs()
                    .text_color(palette.warning)
                    .child(format!("Not rendered: {name}")),
            )
            .child(preformatted(source, context))
            .into_any_element(),
    };
    framed(block, trail, body, context)
}

/// `body`, the drawing of `block` at `trail`: its extent recorded, and a click on it showing
/// its source.
fn framed(
    block: &ViewBlock,
    trail: &[usize],
    body: AnyElement,
    context: &BlockContext<'_>,
) -> AnyElement {
    let span = block.span.clone();
    let on_block = context.on_block.clone();
    div()
        .id(block_id(trail))
        .relative()
        .w_full()
        .child(body)
        .children(
            context
                .list
                .as_ref()
                .map(|list| super::sync::recorder(trail.to_vec(), &context.extents, list)),
        )
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_block(&span, window, cx);
        })
        .into_any_element()
}

/// The text of a visual editor node at `place`, drawn as the preview draws it, for a node
/// whose own reading of the text disagrees with the parser's (`Node::drawn_from_view`).
/// A click goes to its source, as a click on a read-only block does. `None` when the view
/// tree has nothing there.
pub(crate) fn parsed_text(
    view: &ViewTree,
    origin: Option<&[usize]>,
    place: &adoc_engine::ViewText,
    selector: String,
    context: &BlockContext<'_>,
) -> Option<AnyElement> {
    use adoc_engine::ViewText;
    let at = |trail: &[usize]| adoc_render::block_by_trail(view, trail);
    let (span, body) = match place {
        ViewText::Block => {
            let trail = origin?;
            let block = at(trail)?;
            let body = match &block.kind {
                ViewKind::Paragraph(content) => text(content, trail, "body", context),
                _ => block_element(block, &mut trail.to_vec(), view, context),
            };
            (block.span.clone(), body)
        }
        ViewText::Title => {
            let trail = origin?;
            let block = at(trail)?;
            let body = match &block.kind {
                ViewKind::Section { .. } => section_heading(block, trail, context)?,
                ViewKind::Heading { level, title } => {
                    heading_element(*level, title, trail, context)
                }
                _ => return None,
            };
            (block.span.clone(), body)
        }
        ViewText::Item { list, index } => {
            let ViewKind::List { items, .. } = &at(list)?.kind else {
                return None;
            };
            let item = items.get(*index)?;
            let body = text(&item.content, list, &format!("item{index}"), context);
            (item.span.clone(), body)
        }
        ViewText::Term { list, index } | ViewText::Description { list, index } => {
            let ViewKind::DescriptionList(items) = &at(list)?.kind else {
                return None;
            };
            let item = items.get(*index)?;
            let body = if matches!(place, ViewText::Term { .. }) {
                inline_text(text_id(list, &format!("term{index}")), context, || {
                    inlines::layout_bold(&item.term, &context.inline)
                })
            } else {
                text(&item.content, list, &format!("dd{index}"), context)
            };
            (item.span.clone(), body)
        }
    };
    let on_block = context.on_block.clone();
    let id = ElementId::Name(SharedString::from(selector.clone()));
    Some(
        div()
            .id(id)
            .debug_selector(move || selector)
            .child(body)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_block(&span, window, cx);
            })
            .into_any_element(),
    )
}

/// A section's numbered heading.
fn section_heading(
    block: &ViewBlock,
    trail: &[usize],
    context: &BlockContext<'_>,
) -> Option<AnyElement> {
    let ViewKind::Section {
        level,
        number,
        title: heading,
        ..
    } = &block.kind
    else {
        return None;
    };
    let mut content = Vec::new();
    if let Some(number) = number {
        content.push(ViewInline::Text(format!("{number} ")));
    }
    content.extend(heading.iter().cloned());
    Some(heading_element(*level, &content, trail, context))
}

/// Blocks nested in list items and cells are numbered across the whole list, as
/// `ViewBlock::child_refs` numbers them.
pub(crate) fn offset_children(
    blocks: &[&ViewBlock],
    first: usize,
    trail: &mut Vec<usize>,
    view: &ViewTree,
    context: &BlockContext<'_>,
) -> Vec<AnyElement> {
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            trail.push(first + index);
            let element = block_element(block, trail, view, context);
            trail.pop();
            element
        })
        .collect()
}

fn heading_element(
    level: u8,
    content: &[ViewInline],
    trail: &[usize],
    context: &BlockContext<'_>,
) -> AnyElement {
    let element = div()
        .pt_2()
        .child(inline_text(text_id(trail, "heading"), context, || {
            inlines::layout_bold(content, &context.inline)
        }));
    match level {
        0 | 1 => element.text_2xl(),
        2 => element.text_xl(),
        3 => element.text_lg(),
        _ => element.text_base(),
    }
    .into_any_element()
}

fn toc_element(view: &ViewTree, context: &BlockContext<'_>) -> AnyElement {
    v_flex()
        .gap_1()
        .p_3()
        .border_1()
        .border_color(context.palette.border)
        .rounded(context.radius)
        .text_sm()
        .child(
            div().font_weight(FontWeight::BOLD).child(
                view.attributes
                    .get("toc-title")
                    .cloned()
                    .unwrap_or_else(|| "Table of Contents".to_owned()),
            ),
        )
        .children(view.toc.iter().map(|entry| {
            div()
                .pl(px(12. * f32::from(entry.level.saturating_sub(1))))
                .child(match &entry.number {
                    Some(number) => format!("{number} {}", entry.title),
                    None => entry.title.clone(),
                })
        }))
        .into_any_element()
}

fn container(
    block: &ViewBlock,
    trail: &mut Vec<usize>,
    view: &ViewTree,
    context: &BlockContext<'_>,
    background: Hsla,
    bordered: bool,
) -> AnyElement {
    v_flex()
        .gap_2()
        .when(bordered, |element| {
            element
                .p_3()
                .border_1()
                .border_color(context.palette.border)
                .rounded(context.radius)
        })
        .bg(background)
        .children(title(block, trail, context))
        .children(children(&block.child_refs(), trail, view, context))
        .into_any_element()
}

fn preformatted(source: &str, context: &BlockContext<'_>) -> AnyElement {
    div()
        .p_2()
        .bg(context.palette.surface)
        .rounded(context.radius)
        .font_family(context.inline.mono_family.clone())
        .text_sm()
        .child(source.to_owned())
        .into_any_element()
}

fn placeholder(label: &str, colour: Hsla, context: &BlockContext<'_>) -> AnyElement {
    div()
        .p_3()
        .border_1()
        .border_dashed()
        .border_color(colour)
        .rounded(context.radius)
        .text_sm()
        .text_color(colour)
        .child(label.to_owned())
        .into_any_element()
}

fn image_element(image: &ViewImage, context: &BlockContext<'_>) -> AnyElement {
    let element = match (&image.path, &image.url) {
        (Some(path), _) => img(path.clone()),
        (None, Some(url)) => img(SharedString::from(url.clone())),
        (None, None) => {
            return placeholder(
                &format!("Missing image: {}", image.target),
                context.palette.danger,
                context,
            )
        }
    };
    element
        .max_w_full()
        .when_some(image.width.as_deref().and_then(pixels), |element, width| {
            element.w(width)
        })
        .when_some(
            image.height.as_deref().and_then(pixels),
            |element, height| element.h(height),
        )
        .into_any_element()
}

fn pixels(value: &str) -> Option<Pixels> {
    value.trim_end_matches("px").parse::<f32>().ok().map(px)
}

fn attribution_line(
    attribution: Option<&str>,
    citation: Option<&str>,
    palette: &Palette,
) -> Option<AnyElement> {
    let line = match (attribution, citation) {
        (None, None) => return None,
        (Some(attribution), None) => format!("— {attribution}"),
        (None, Some(citation)) => format!("— {citation}"),
        (Some(attribution), Some(citation)) => format!("— {attribution}, {citation}"),
    };
    Some(
        div()
            .text_sm()
            .text_color(palette.muted)
            .child(line)
            .into_any_element(),
    )
}

fn callout_badge(number: usize, context: &BlockContext<'_>) -> AnyElement {
    div()
        .px_1()
        .rounded(px(8.))
        .bg(context.palette.muted)
        .text_color(context.inline.code_background)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(number.to_string())
        .into_any_element()
}

/// The styles of each line of `code`, relative to the line: each style given to the lines it
/// covers, in one pass over the styles.
fn styles_by_line(
    code: &str,
    styles: &[(Range<usize>, HighlightStyle)],
) -> Vec<Vec<(Range<usize>, HighlightStyle)>> {
    let mut lines: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    for line in code.split('\n') {
        lines.push(start..start + line.len());
        start += line.len() + 1;
    }
    let mut by_line: Vec<Vec<(Range<usize>, HighlightStyle)>> = vec![Vec::new(); lines.len()];
    for (style_range, style) in styles {
        let first = lines.partition_point(|line| line.end <= style_range.start);
        for (range, line_styles) in lines[first..].iter().zip(&mut by_line[first..]) {
            if range.start >= style_range.end {
                break;
            }
            let from = style_range.start.max(range.start);
            let to = style_range.end.min(range.end);
            if from < to {
                line_styles.push((from - range.start..to - range.start, *style));
            }
        }
    }
    by_line
}

/// One element per source line, highlighted, with each line's callout badges after it.
fn code_lines(
    code: &str,
    styles: &[(Range<usize>, HighlightStyle)],
    callouts: &[adoc_render::Callout],
    context: &BlockContext<'_>,
) -> Vec<AnyElement> {
    code.split('\n')
        .zip(styles_by_line(code, styles))
        .enumerate()
        .map(|(line_number, (line, line_styles))| {
            let content = if line.is_empty() {
                // An empty line still takes a line's height.
                StyledText::new(" ")
            } else {
                StyledText::new(line.to_owned()).with_highlights(line_styles)
            };
            h_flex()
                .gap_2()
                .child(content)
                .children(
                    callouts
                        .iter()
                        .filter(|callout| callout.line == line_number)
                        .map(|callout| callout_badge(callout.number, context)),
                )
                .into_any_element()
        })
        .collect()
}

fn table_element(
    table: &ViewTable,
    trail: &mut Vec<usize>,
    view: &ViewTree,
    context: &BlockContext<'_>,
) -> AnyElement {
    let weights: Vec<f32> = table
        .columns
        .iter()
        .map(|column| match column.width {
            adoc_render::ColumnWidth::Proportional(width) => width as f32,
            adoc_render::ColumnWidth::Percentage(width) => width as f32,
            adoc_render::ColumnWidth::Auto => 1.,
        })
        .collect();
    let total: f32 = weights.iter().sum::<f32>().max(1.);
    let mut nested = 0;
    let mut row = |cells: &[adoc_render::ViewCell], header: bool, trail: &mut Vec<usize>| {
        let mut column = 0;
        h_flex()
            .w_full()
            .border_b_1()
            .border_color(context.palette.border)
            .children(cells.iter().map(|cell| {
                let span = cell.colspan.max(1);
                let weight: f32 = weights.iter().skip(column).take(span).sum::<f32>().max(1.);
                column += span;
                let cell_children: Vec<&ViewBlock> = cell.blocks.iter().collect();
                let first = nested;
                nested += cell_children.len();
                v_flex()
                    .w(relative(weight / total))
                    .p_1()
                    .gap_1()
                    .when(header, |cell| cell.font_weight(FontWeight::BOLD))
                    .children(offset_children(&cell_children, first, trail, view, context))
            }))
            .into_any_element()
    };
    let mut rows = Vec::new();
    if let Some(header) = &table.header {
        rows.push(row(header, true, trail));
    }
    for cells in &table.rows {
        rows.push(row(cells, false, trail));
    }
    if let Some(footer) = &table.footer {
        rows.push(row(footer, false, trail));
    }
    v_flex()
        .w_full()
        .border_1()
        .border_color(context.palette.border)
        .rounded(context.radius)
        .children(rows)
        .into_any_element()
}

#[cfg(test)]
mod code_tests {
    use super::*;

    #[test]
    fn each_line_gets_the_styles_over_it() {
        let code = "let a = 1;\n\n/* one\ntwo */ b\n";
        let style = |weight| HighlightStyle {
            font_weight: Some(gpui::FontWeight(weight)),
            ..HighlightStyle::default()
        };
        // A keyword, a comment across two lines, one on the newline, and a style past the end.
        let styles = [
            (0..3, style(100.)),
            (12..24, style(200.)),
            (10..11, style(300.)),
            (25..40, style(400.)),
        ];
        // As each line filtering every style found them.
        let mut expected = Vec::new();
        let mut start = 0;
        for line in code.split('\n') {
            let range = start..start + line.len();
            start = range.end + 1;
            expected.push(
                styles
                    .iter()
                    .filter_map(|(style_range, style)| {
                        let from = style_range.start.max(range.start);
                        let to = style_range.end.min(range.end);
                        (from < to).then(|| (from - range.start..to - range.start, *style))
                    })
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(styles_by_line(code, &styles), expected);
        assert_eq!(styles_by_line(code, &styles)[2], [(0..6, style(200.))]);
    }
}
