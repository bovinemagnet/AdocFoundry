//! Model nodes drawn as elements. Text blocks are drawn for editing; what the model keeps
//! read-only (images, diagrams, included content, verbatim source) is drawn as the preview
//! draws it, from the view tree built with the model.

use std::{cell::RefCell, rc::Rc};

use adoc_engine::{
    commands::ids, Access, CommandArgs, Node, NodeKind, ReadOnly, RowSection, TextBlock,
    VisualDocument,
};
use adoc_render::{AdmonitionKind, ListKind};
use gpui::{
    div, prelude::FluentBuilder as _, px, AnyElement, Context, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, Styled as _, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _,
};

use super::{text, VisualView};
use crate::preview::{self, blocks::BlockContext, inlines::InlineStyle};

/// What every node is drawn with.
struct Draw<'a> {
    document: &'a VisualDocument,
    context: BlockContext<'a>,
    inline: InlineStyle,
    caret: Hsla,
    selection: Hsla,
    focused: bool,
}

/// The whole document: each top-level node.
pub fn document(
    view: &VisualView,
    document: &VisualDocument,
    window: &mut Window,
    cx: &mut Context<VisualView>,
) -> Vec<AnyElement> {
    let cache: Rc<RefCell<preview::HighlightCache>> = Rc::default();
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
    let draw = Draw {
        document,
        inline: context.inline.clone(),
        context,
        caret: cx.theme().primary,
        selection: cx.theme().primary.opacity(0.25),
        focused: view.focus.is_focused(window),
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
                    .child(v_flex().flex_1().gap_2().children(self.children(
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
        let mut rows = Vec::new();
        for (row_index, row) in table.children.iter().enumerate() {
            path.push(row_index);
            let header = matches!(
                row.kind,
                NodeKind::Row {
                    section: RowSection::Header
                }
            );
            let mut cells = Vec::new();
            for (cell_index, cell) in row.children.iter().enumerate() {
                path.push(cell_index);
                let content = if cell.text.is_some() {
                    self.text(path, cell, draw, cx)
                } else {
                    self.read_only(cell, draw)
                };
                cells.push(
                    div()
                        .flex_1()
                        .p_1()
                        .border_1()
                        .border_color(palette.border)
                        .when(header, |cell| cell.font_weight(FontWeight::BOLD))
                        .child(content),
                );
                path.pop();
            }
            rows.push(h_flex().items_stretch().children(cells));
            path.pop();
        }
        v_flex().children(rows).into_any_element()
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
    fn text(
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
        let shown = text::shown(block, composing);
        let runs = text::text_runs(block, &shown, &draw.inline);
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
            reveal: caret_here.then(|| (self.scroll.clone(), self.reveal.clone())),
        };
        let element = text::element(path.to_vec(), &shown, runs, &self.layouts, overlay);
        let read_only = !node.is_editable();
        let down_path = path.to_vec();
        let move_path = path.to_vec();
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
                        window,
                        cx,
                    );
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                this.mouse_drag(&move_path, event.position, cx);
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
