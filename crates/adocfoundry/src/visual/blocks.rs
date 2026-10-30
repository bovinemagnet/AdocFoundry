//! Model nodes drawn as elements. Text blocks are drawn for editing; what the model keeps
//! read-only (images, diagrams, included content, verbatim source) is drawn as the preview
//! draws it, from the view tree built with the model.

use std::rc::Rc;

use adoc_engine::{
    commands::ids, Access, CommandArgs, Node, NodeKind, ReadOnly, RowSection, TextBlock,
    VisualDocument,
};
use adoc_render::{
    AdmonitionKind, ColumnWidth, ListKind, ViewBlock, ViewCell, ViewKind, ViewTable,
};
use gpui::{
    div, prelude::FluentBuilder as _, px, relative, AnyElement, Context, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, Styled as _, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _,
};

use super::{text, VisualView};
use crate::{
    preview::{self, blocks::BlockContext, inlines::InlineStyle},
    sidebar::DraggedLink,
};

/// What every node is drawn with.
struct Draw<'a> {
    document: &'a VisualDocument,
    context: BlockContext<'a>,
    inline: InlineStyle,
    caret: Hsla,
    selection: Hsla,
    focused: bool,
    /// The document's changes against `HEAD`, when markers are shown.
    changes: Vec<adoc_engine::Hunk>,
    marks: crate::editor::changes::MarkColours,
}

/// Highlighted blocks kept before the cache is emptied.
const HIGHLIGHT_CACHE_LIMIT: usize = 256;

/// The whole document: each top-level node.
pub fn document(
    view: &VisualView,
    document: &VisualDocument,
    window: &mut Window,
    cx: &mut Context<VisualView>,
) -> Vec<AnyElement> {
    // Typing adds an entry for each version of a block's text; start afresh before that grows.
    if view.highlights.borrow().len() > HIGHLIGHT_CACHE_LIMIT {
        view.highlights.borrow_mut().clear();
    }
    let cache = view.highlights.clone();
    let dark = cx.theme().mode.is_dark();
    let theme = cx.theme().highlight_theme.clone();
    let highlight =
        move |language: &str, code: &str| preview::highlight(&cache, dark, &theme, language, code);
    let app = view.app.clone();
    // A click on read-only content shows its source beside the visual editor.
    let on_block: preview::blocks::BlockHandler = Rc::new(move |span, window, cx| {
        let position = adoc_engine::NavigationPosition {
            path: span.path.clone(),
            offset: span.range.start,
        };
        let result = app.update(cx, |app, cx| {
            app.invoke(ids::VIEW_SHOW_SPLIT, CommandArgs::None, cx)?;
            app.invoke(ids::NAVIGATE_REVEAL, CommandArgs::Position(position), cx)
        });
        if let Err(error) = result {
            window.push_notification(error.to_string(), cx);
        }
    });
    let context = preview::block_context(
        &view.app,
        preview::sync::Extents::default(),
        view.scroll.clone(),
        on_block,
        &highlight,
        window,
        cx,
    );
    let changes = {
        let engine = &view.app.read(cx).engine;
        if engine.settings().git.show_markers {
            engine.git().hunks_for(view.document).to_vec()
        } else {
            Vec::new()
        }
    };
    let draw = Draw {
        document,
        inline: context.inline.clone(),
        context,
        caret: cx.theme().primary,
        selection: cx.theme().primary.opacity(0.25),
        focused: view.focus.is_focused(window),
        changes,
        marks: crate::editor::changes::MarkColours {
            added: cx.theme().success,
            modified: cx.theme().info,
            deleted: cx.theme().danger,
        },
    };
    let mut path = Vec::new();
    document
        .model
        .blocks
        .iter()
        .enumerate()
        .map(|(index, node)| {
            path.push(index);
            let element = view.node(&mut path, node, &draw, cx);
            path.pop();
            element
        })
        .collect()
}

impl VisualView {
    fn children(
        &self,
        path: &mut Vec<usize>,
        nodes: &[Node],
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                path.push(index);
                let element = self.node(path, node, draw, cx);
                path.pop();
                element
            })
            .collect()
    }

    fn node(
        &self,
        path: &mut Vec<usize>,
        node: &Node,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element = self.node_body(path, node, draw, cx);
        // A block without text of its own (an image, a table, read-only content) is marked
        // whole; text blocks are marked where their text is drawn.
        let marked_whole = node.text.is_none()
            && (node.children.is_empty() || matches!(node.kind, NodeKind::Table { .. }))
            && !matches!(node.kind, NodeKind::Row { .. } | NodeKind::Cell);
        let element = if marked_whole {
            with_change_bar(element, node.range, path, draw)
        } else {
            element
        };
        // A block's title (`.Title`) is shown above it and edited in the source. Sections and
        // headings are their titles; read-only blocks are drawn with theirs by the preview.
        let titled = !matches!(
            node.kind,
            NodeKind::Title
                | NodeKind::Section { .. }
                | NodeKind::Heading { .. }
                | NodeKind::ListItem { .. }
                | NodeKind::Row { .. }
                | NodeKind::Cell
                | NodeKind::Term
                | NodeKind::Description
                | NodeKind::Include { .. }
                | NodeKind::Conditional { .. }
                | NodeKind::Verbatim { .. }
        ) && !(matches!(node.kind, NodeKind::Image { .. }) && !node.is_editable());
        let title = node
            .origin
            .as_deref()
            .filter(|_| titled)
            .and_then(|origin| {
                let block = adoc_render::block_by_trail(&draw.document.view, origin)?;
                preview::blocks::title(block, origin, &draw.context)
            });
        match title {
            Some(title) => {
                let selector = format!("visual-title-{}", joined(path));
                v_flex()
                    .child(div().debug_selector(move || selector).child(title))
                    .child(element)
                    .into_any_element()
            }
            None => element,
        }
    }

    fn node_body(
        &self,
        path: &mut Vec<usize>,
        node: &Node,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = &draw.context.palette;
        match &node.kind {
            NodeKind::Title => div()
                .text_3xl()
                .font_weight(FontWeight::BOLD)
                .pb_2()
                .child(self.text(path, node, draw, cx))
                .into_any_element(),
            NodeKind::Section { level } | NodeKind::Heading { level } => {
                let heading = div()
                    .pt_2()
                    .font_weight(FontWeight::BOLD)
                    .child(self.text(path, node, draw, cx));
                let heading = match level {
                    0 | 1 => heading.text_2xl(),
                    2 => heading.text_xl(),
                    3 => heading.text_lg(),
                    _ => heading.text_base(),
                };
                v_flex()
                    .gap_3()
                    .child(heading)
                    .children(self.children(path, &node.children, draw, cx))
                    .into_any_element()
            }
            NodeKind::Paragraph | NodeKind::Description => self.text(path, node, draw, cx),
            NodeKind::Term => div()
                .font_weight(FontWeight::BOLD)
                .child(self.text(path, node, draw, cx))
                .into_any_element(),
            NodeKind::List { kind } => {
                let kind = *kind;
                let mut items = Vec::new();
                for (index, item) in node.children.iter().enumerate() {
                    path.push(index);
                    items.push(self.list_item(path, item, kind, index, draw, cx));
                    path.pop();
                }
                v_flex().gap_1().children(items).into_any_element()
            }
            NodeKind::ListItem { .. } => {
                self.list_item(path, node, ListKind::Unordered, 0, draw, cx)
            }
            NodeKind::DescriptionList => v_flex()
                .gap_2()
                .children(self.children(path, &node.children, draw, cx))
                .into_any_element(),
            NodeKind::DescriptionItem => {
                let mut children = self.children(path, &node.children, draw, cx).into_iter();
                v_flex()
                    .children(children.next())
                    .child(v_flex().pl_4().gap_1().children(children))
                    .into_any_element()
            }
            NodeKind::Admonition { kind, .. } => {
                let colour = match kind {
                    AdmonitionKind::Note => palette.note,
                    AdmonitionKind::Tip => palette.tip,
                    AdmonitionKind::Important => palette.important,
                    AdmonitionKind::Warning => palette.warning,
                    AdmonitionKind::Caution => palette.caution,
                };
                h_flex()
                    .items_start()
                    .gap_3()
                    .p_2()
                    .border_l_4()
                    .border_color(colour)
                    .bg(palette.surface)
                    .rounded(draw.context.radius)
                    .child(
                        div()
                            .min_w(px(80.))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(colour)
                            .child(kind.label()),
                    )
                    .child(v_flex().flex_1().min_w_0().gap_2().children(self.children(
                        path,
                        &node.children,
                        draw,
                        cx,
                    )))
                    .into_any_element()
            }
            NodeKind::Code { .. } | NodeKind::Literal => div()
                .p_2()
                .bg(palette.surface)
                .border_1()
                .border_color(palette.border)
                .rounded(draw.context.radius)
                .font_family(draw.inline.mono_family.clone())
                .text_sm()
                .child(self.text(path, node, draw, cx))
                .into_any_element(),
            NodeKind::Quote => v_flex()
                .pl_4()
                .border_l_4()
                .border_color(palette.border)
                .italic()
                .gap_2()
                .children(self.children(path, &node.children, draw, cx))
                .into_any_element(),
            NodeKind::Sidebar | NodeKind::Example | NodeKind::Open => {
                let bordered = !matches!(node.kind, NodeKind::Open);
                v_flex()
                    .gap_2()
                    .p_3()
                    .when(bordered, |container| {
                        container
                            .border_1()
                            .border_color(palette.border)
                            .rounded(draw.context.radius)
                    })
                    .when(matches!(node.kind, NodeKind::Sidebar), |sidebar| {
                        sidebar.bg(palette.surface)
                    })
                    .children(self.children(path, &node.children, draw, cx))
                    .into_any_element()
            }
            NodeKind::Table { .. } => self.table(path, node, draw, cx),
            NodeKind::Row { .. } | NodeKind::Cell => v_flex()
                .children(self.children(path, &node.children, draw, cx))
                .into_any_element(),
            NodeKind::Include { target } => self.include(path, node, target, draw, cx),
            NodeKind::Conditional { directive } => div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(palette.muted)
                .border_1()
                .border_dashed()
                .border_color(palette.border)
                .rounded(draw.context.radius)
                .child(format!("{directive} … (conditional; shown in the source)"))
                .into_any_element(),
            NodeKind::Image { target, .. } if node.is_editable() => {
                self.image(node, target, draw, cx)
            }
            NodeKind::Image { .. } | NodeKind::Verbatim { .. } => self.read_only(node, draw),
        }
    }

    fn list_item(
        &self,
        path: &mut Vec<usize>,
        item: &Node,
        kind: ListKind,
        index: usize,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let NodeKind::ListItem { checked, .. } = &item.kind else {
            return self.node(path, item, draw, cx);
        };
        let marker = match (kind, checked) {
            (_, Some(true)) => "☑".to_owned(),
            (_, Some(false)) => "☐".to_owned(),
            (ListKind::Ordered, None) => format!("{}.", index + 1),
            _ => "•".to_owned(),
        };
        let checkbox = checked
            .is_some()
            .then(|| item.text.as_ref().map(|text| text.range.start))
            .flatten();
        let marker = div()
            .min_w(px(18.))
            .child(marker)
            .when_some(checkbox, |marker, at| {
                marker.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.place_caret(at, cx);
                        this.run(
                            ids::EDIT_TOGGLE_CHECKED,
                            adoc_engine::VisualValue::None,
                            window,
                            cx,
                        );
                    }),
                )
            });
        h_flex()
            .items_start()
            .gap_2()
            .child(marker)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(self.text(path, item, draw, cx))
                    .children(self.children(path, &item.children, draw, cx)),
            )
            .into_any_element()
    }

    fn table(
        &self,
        path: &mut Vec<usize>,
        table: &Node,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = &draw.context.palette;
        let view = &draw.document.view;
        // The view tree's table gives the column widths, each cell's span and the blocks of
        // cells that are not edited here; used only while its rows match the model's.
        let shape = table
            .origin
            .as_deref()
            .and_then(|origin| adoc_render::block_by_trail(view, origin))
            .and_then(|block| match &block.kind {
                ViewKind::Table(shape) => Some(shape),
                _ => None,
            })
            .filter(|shape| {
                let rows: Vec<&Vec<ViewCell>> = view_rows(shape).collect();
                rows.len() == table.children.len()
                    && rows
                        .iter()
                        .zip(&table.children)
                        .all(|(cells, row)| cells.len() == row.children.len())
            });
        // Without `cols`, every column is as wide as the others.
        let columns = match table.kind {
            NodeKind::Table { columns, .. } => columns,
            _ => 0,
        };
        let weights: Vec<f32> = match shape {
            Some(shape) if !shape.columns.is_empty() => shape
                .columns
                .iter()
                .map(|column| match column.width {
                    ColumnWidth::Proportional(width) | ColumnWidth::Percentage(width) => {
                        width as f32
                    }
                    ColumnWidth::Auto => 1.,
                })
                .collect(),
            _ => vec![1.; columns],
        };
        let total = weights.iter().sum::<f32>().max(1.);
        let mut view_cells = shape.map(|shape| view_rows(shape).flatten());
        let mut trail = table.origin.clone().unwrap_or_default();
        let mut nested = 0;
        let mut rows = Vec::new();
        for (row_index, row) in table.children.iter().enumerate() {
            path.push(row_index);
            let header = matches!(
                row.kind,
                NodeKind::Row {
                    section: RowSection::Header
                }
            );
            let mut column = 0;
            let mut cells = Vec::new();
            for (cell_index, cell) in row.children.iter().enumerate() {
                path.push(cell_index);
                let shown = view_cells.as_mut().and_then(Iterator::next);
                let first = nested;
                nested += shown.map_or(0, |shown| shown.blocks.len());
                let content = match (&cell.text, shown) {
                    (Some(_), _) => self.text(path, cell, draw, cx),
                    (None, Some(shown)) => {
                        let blocks: Vec<&ViewBlock> = shown.blocks.iter().collect();
                        let cell_path = joined(path);
                        v_flex()
                            .gap_1()
                            .children(
                                preview::blocks::offset_children(
                                    &blocks,
                                    first,
                                    &mut trail,
                                    view,
                                    &draw.context,
                                )
                                .into_iter()
                                .enumerate()
                                .map(|(index, block)| {
                                    let selector = format!("visual-cell-{cell_path}-block-{index}");
                                    div().debug_selector(move || selector).child(block)
                                }),
                            )
                            .into_any_element()
                    }
                    (None, None) => self.read_only(cell, draw),
                };
                let span = shown.map_or(1, |shown| shown.colspan.max(1));
                let weight = weights.iter().skip(column).take(span).sum::<f32>();
                column += span;
                let selector = format!("visual-cell-{}", joined(path));
                cells.push(
                    div()
                        .debug_selector(move || selector)
                        .map(|cell| {
                            if weight > 0. {
                                cell.w(relative(weight / total))
                            } else {
                                cell.flex_1()
                            }
                        })
                        .min_w_0()
                        .p_1()
                        .border_1()
                        .border_color(palette.border)
                        .when(header, |cell| cell.font_weight(FontWeight::BOLD))
                        .child(content),
                );
                path.pop();
            }
            rows.push(h_flex().w_full().items_stretch().children(cells));
            path.pop();
        }
        v_flex().w_full().children(rows).into_any_element()
    }

    fn include(
        &self,
        path: &mut Vec<usize>,
        node: &Node,
        target: &str,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = &draw.context.palette;
        let partial = match &node.access {
            Access::ReadOnly(ReadOnly::Included { path, .. }) => Some(path.clone()),
            _ => None,
        };
        let app = self.app.clone();
        let header = h_flex()
            .gap_2()
            .text_xs()
            .text_color(palette.muted)
            .child(format!("Included from {target}"))
            .when_some(partial, |header, partial| {
                header.child(
                    Button::new(gpui::SharedString::from(format!("edit-partial-{path:?}")))
                        .ghost()
                        .xsmall()
                        .label("Edit partial")
                        .on_click(move |_, window, cx| {
                            let result = app.update(cx, |app, cx| {
                                app.invoke(
                                    ids::DOCUMENT_OPEN,
                                    CommandArgs::Path(partial.clone()),
                                    cx,
                                )
                            });
                            if let Err(error) = result {
                                window.push_notification(error.to_string(), cx);
                            }
                        }),
                )
            });
        let children = self.children(path, &node.children, draw, cx);
        v_flex()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_dashed()
            .border_color(palette.border)
            .child(header)
            .children(children)
            .into_any_element()
    }

    /// A block image, drawn from the file it resolves to; clicking it edits it.
    fn image(
        &self,
        node: &Node,
        target: &str,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let resolved = node
            .origin
            .as_deref()
            .and_then(|origin| adoc_render::block_by_trail(&draw.document.view, origin))
            .and_then(|block| match &block.kind {
                adoc_render::ViewKind::Image(image) => image.path.clone(),
                _ => None,
            });
        let at = node.range.start;
        let picture = match resolved {
            Some(path) => gpui::img(path).max_w_full().into_any_element(),
            None => div()
                .p_3()
                .border_1()
                .border_dashed()
                .border_color(draw.context.palette.border)
                .text_sm()
                .text_color(draw.context.palette.muted)
                .child(format!("Missing image: {target}"))
                .into_any_element(),
        };
        div()
            .cursor_pointer()
            .child(picture)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.image_dialog(Some(at), window, cx);
                }),
            )
            .into_any_element()
    }

    /// A node the model keeps as source: drawn as the preview draws it when the view tree has
    /// it, else as its source.
    fn read_only(&self, node: &Node, draw: &Draw<'_>) -> AnyElement {
        let view = &draw.document.view;
        if let Some(block) = node
            .origin
            .as_deref()
            .and_then(|origin| adoc_render::block_by_trail(view, origin))
        {
            return preview::blocks::block(
                block,
                node.origin.as_deref().unwrap_or_default(),
                view,
                &draw.context,
            );
        }
        let source = &draw.document.model.source;
        let text = source
            .get(node.range.start..node.range.end)
            .unwrap_or_default()
            .to_owned();
        div()
            .p_2()
            .text_sm()
            .font_family(draw.inline.mono_family.clone())
            .text_color(draw.context.palette.muted)
            .child(text)
            .into_any_element()
    }

    /// The text block of `node`, drawn for editing, with the caret and selection over it.
    /// The text block of `node`, with a change bar when its lines differ from `HEAD`. A cell's
    /// changes are marked on its table.
    fn text(
        &self,
        path: &[usize],
        node: &Node,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element = self.text_inner(path, node, draw, cx);
        let Some(block) = node.text.as_ref() else {
            return element;
        };
        if matches!(node.kind, NodeKind::Cell) {
            return element;
        }
        // A block with children (a section, a list item) is marked only for its own text.
        let range = if node.children.is_empty() {
            node.range
        } else {
            block.range
        };
        with_change_bar(element, range, path, draw)
    }

    fn text_inner(
        &self,
        path: &[usize],
        node: &Node,
        draw: &Draw<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(block) = node.text.as_ref() else {
            return gpui::Empty.into_any_element();
        };
        let caret_here = self
            .block_at(self.selection.head)
            .is_some_and(|(at, _)| at.as_slice() == path);
        let caret_display = caret_here
            .then(|| block.display_offset(self.selection.head))
            .flatten();
        let composing = caret_display.zip(self.composing.as_deref());
        let shown = text::shown(block, composing, &self.formula_gaps);
        // Titles, headings and terms are bold throughout: runs set their own weight.
        let bold = matches!(
            node.kind,
            NodeKind::Title | NodeKind::Section { .. } | NodeKind::Heading { .. } | NodeKind::Term
        );
        let mut runs = text::text_runs(block, &shown, &draw.inline, bold);
        // An editable code block is highlighted by its language, as the preview draws it.
        if let NodeKind::Code {
            language: Some(language),
        } = &node.kind
        {
            runs = text::with_highlights(runs, &(draw.context.highlight)(language, &shown.text));
        }
        let pending = self.pending.filter(|_| caret_here);
        let caret = caret_display
            .filter(|_| draw.focused && self.selection.is_empty() && pending.is_none())
            .map(|display| shown.index(display));
        let selection = selected_range(block, self.selection.start(), self.selection.end())
            .map(|range| shown.index(range.start)..shown.index(range.end));
        let overlay = text::Overlay {
            caret,
            selection,
            caret_colour: draw.caret,
            selection_colour: draw.selection,
            formula_backing: draw.inline.formula_backing,
            reveal: caret_here.then(|| (self.scroll.clone(), self.reveal.clone())),
        };
        let element = text::element(path.to_vec(), &shown, runs, &self.layouts, overlay);
        let read_only = !node.is_editable();
        let down_path = path.to_vec();
        let move_path = path.to_vec();
        let drop_path = path.to_vec();
        div()
            .w_full()
            .when(read_only, |text| {
                text.border_l_2()
                    .border_dashed()
                    .border_color(draw.context.palette.border)
                    .pl_1()
            })
            .child(element)
            .when_some(pending, |text, pending| {
                text.child(pending_block(pending, draw))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.mouse_down(
                        &down_path,
                        event.position,
                        event.modifiers.shift,
                        event.click_count,
                        window,
                        cx,
                    );
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                this.mouse_drag(&move_path, event.position, cx);
            }))
            .on_drop(cx.listener(move |this, link: &DraggedLink, window, cx| {
                this.drop_link(&drop_path, link, window, cx);
            }))
            .into_any_element()
    }
}

/// The block begun with Enter and not yet typed into: an empty line (with a marker for a
/// list item) holding the caret.
fn pending_block(pending: super::Pending, draw: &Draw<'_>) -> AnyElement {
    let caret = div().w(px(2.)).h(px(18.)).bg(draw.caret);
    h_flex()
        .pt_3()
        .gap_2()
        .when(pending.list_item, |line| {
            line.child(div().min_w(px(18.)).child("•"))
        })
        .child(caret)
        .into_any_element()
}

/// The display range of `block` the source range `start..end` selects, when it selects some.
fn selected_range(block: &TextBlock, start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    if start == end || end < block.range.start || start > block.range.end {
        return None;
    }
    let from = block.display_offset(start.max(block.range.start))?;
    let to = block.display_offset_towards(end.min(block.range.end), true)?;
    (from < to || (start < block.range.start && end > block.range.end && block.is_empty()))
        .then_some(from..to)
}

/// A table's rows in reading order: header, body, footer.
fn view_rows(table: &ViewTable) -> impl Iterator<Item = &Vec<ViewCell>> {
    table.header.iter().chain(&table.rows).chain(&table.footer)
}

/// A node path as it appears in element selectors: `1-0-2`.
fn joined(path: &[usize]) -> String {
    path.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("-")
}

/// `element` with a bar at its left in the colour of the changes its source `range` meets: added,
/// modified or deleted lines; `element` unchanged when it meets none.
fn with_change_bar(
    element: AnyElement,
    range: adoc_core::SourceRange,
    path: &[usize],
    draw: &Draw<'_>,
) -> AnyElement {
    use adoc_engine::HunkKind;
    let kinds: Vec<HunkKind> = draw
        .changes
        .iter()
        .filter(|hunk| {
            if hunk.range.is_empty() {
                range.start <= hunk.range.start && hunk.range.start < range.end
            } else {
                hunk.range.start < range.end && range.start < hunk.range.end
            }
        })
        .map(|hunk| hunk.kind)
        .collect();
    let colour = if kinds.is_empty() {
        return element;
    } else if kinds.iter().all(|kind| *kind == HunkKind::Added) {
        draw.marks.added
    } else if kinds.iter().all(|kind| *kind == HunkKind::Deleted) {
        draw.marks.deleted
    } else {
        draw.marks.modified
    };
    let selector = format!("visual-change-{}", joined(path));
    h_flex()
        .debug_selector(move || selector)
        .items_stretch()
        .gap_1()
        .child(div().w(px(3.)).flex_none().bg(colour))
        .child(div().flex_1().min_w_0().child(element))
        .into_any_element()
}
