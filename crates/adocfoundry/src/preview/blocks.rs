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
pub type Highlighter<'a> = &'a dyn Fn(&str, &str) -> Vec<(Range<usize>, HighlightStyle)>;

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
    pub scroll: gpui::ScrollHandle,
    pub palette: Palette,
    pub radius: Pixels,
    pub on_link: LinkHandler,
    pub on_block: BlockHandler,
    pub highlight: Highlighter<'a>,
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

/// The whole document: title, then each top-level block.
pub fn document(view: &ViewTree, context: &BlockContext<'_>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if let Some(title) = &view.title {
        out.push(
            div()
                .text_3xl()
                .pb_2()
                .child(inlines::element(
                    "document-title".into(),
                    inlines::layout_bold(title, &context.inline),
                    &context.on_link,
                ))
                .into_any_element(),
        );
    }
    // `:toc: macro` places the contents at a `toc::[]` block instead.
    if !view.toc.is_empty() && view.attributes.get("toc").map(String::as_str) != Some("macro") {
        out.push(toc_element(view, context));
    }
    let mut trail = Vec::new();
    out.extend(children(
        &view.blocks.iter().collect::<Vec<_>>(),
        &mut trail,
        view,
        context,
    ));
    out
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
    inlines::element(
        text_id(trail, part),
        inlines::layout(content, &context.inline),
        &context.on_link,
    )
}

fn title(block: &ViewBlock, trail: &[usize], context: &BlockContext<'_>) -> Option<AnyElement> {
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
            .child(inlines::element(
                text_id(trail, "title"),
                inlines::layout_italic(&content, &context.inline),
                &context.on_link,
            ))
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
        ViewKind::Section {
            level,
            number,
            title: heading,
            ..
        } => {
            let mut content = Vec::new();
            if let Some(number) = number {
                content.push(ViewInline::Text(format!("{number} ")));
            }
            content.extend(heading.iter().cloned());
            v_flex()
                .gap_2()
                .child(heading_element(*level, &content, trail, context))
                .children(children(&child_refs, trail, view, context))
                .into_any_element()
        }
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
                        .child(inlines::element(
                            text_id(trail, &format!("term{index}")),
                            inlines::layout_bold(&item.term, &context.inline),
                            &context.on_link,
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
            let styles = language
                .as_deref()
                .map(|language| (context.highlight)(language, code))
                .unwrap_or_default();
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
        ViewKind::Stem { source, .. } => v_flex()
            .children(title(block, trail, context))
            .child(preformatted(source, context))
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
    let span = block.span.clone();
    let on_block = context.on_block.clone();
    div()
        .id(block_id(trail))
        .relative()
        .w_full()
        .child(body)
        .child(super::sync::recorder(
            trail.to_vec(),
            &context.extents,
            &context.scroll,
        ))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_block(&span, window, cx);
        })
        .into_any_element()
}

/// Blocks nested in list items and cells are numbered across the whole list, as
/// `ViewBlock::child_refs` numbers them.
fn offset_children(
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
    let element = div().pt_2().child(inlines::element(
        text_id(trail, "heading"),
        inlines::layout_bold(content, &context.inline),
        &context.on_link,
    ));
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

/// One element per source line, highlighted, with each line's callout badges after it.
fn code_lines(
    code: &str,
    styles: &[(Range<usize>, HighlightStyle)],
    callouts: &[adoc_render::Callout],
    context: &BlockContext<'_>,
) -> Vec<AnyElement> {
    let mut start = 0;
    code.split('\n')
        .enumerate()
        .map(|(line_number, line)| {
            let range = start..start + line.len();
            start = range.end + 1;
            let line_styles: Vec<(Range<usize>, HighlightStyle)> = styles
                .iter()
                .filter_map(|(style_range, style)| {
                    let from = style_range.start.max(range.start);
                    let to = style_range.end.min(range.end);
                    (from < to).then(|| (from - range.start..to - range.start, *style))
                })
                .collect();
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
