//! The visual editor: the engine's model of a document drawn as rendered text that can be
//! edited in place.
//!
//! The view holds a selection as source offsets and nothing of the document's meaning. Typing,
//! deleting and formatting are engine commands on that selection; the engine edits the source
//! and publishes the model afterwards, which the view draws. Edits made elsewhere move the
//! selection with the text.

mod blocks;
mod format;
mod input;
mod text;

use std::{cell::Cell, rc::Rc, sync::Arc};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, Assoc, BlockPath, CommandArgs, CommandId, CommandOutcome, DocumentId,
    EngineEvent, Mapping, MarkSet, SourceEdit, TextBlock, VisualArgs, VisualDocument, VisualValue,
};
use gpui::{
    canvas, div, prelude::FluentBuilder as _, px, App, Context, ElementInputHandler, Entity,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::{v_flex, ActiveTheme as _, WindowExt as _};

use crate::app::AppState;

pub use input::bind;
use text::Layouts;

/// The key context of the visual editor.
pub const VISUAL_CONTEXT: &str = "VisualEditor";

/// A selection: where it started and where the caret is, as source offsets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    #[must_use]
    pub const fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    #[must_use]
    pub fn start(self) -> usize {
        self.anchor.min(self.head)
    }

    #[must_use]
    pub fn end(self) -> usize {
        self.anchor.max(self.head)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }
}

/// What the visual editor tells the window around it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisualEvent {
    /// The caret moved to this source offset.
    CaretMoved(usize),
}

pub struct VisualView {
    app: Entity<AppState>,
    document: DocumentId,
    focus: FocusHandle,
    shown: Option<Arc<VisualDocument>>,
    selection: Selection,
    /// The document version the selection is in.
    version: u64,
    /// The marks typed text carries.
    marks: MarkSet,
    /// IME text being composed at the caret, not yet in the document.
    composing: Option<String>,
    /// The horizontal position kept while moving up and down.
    goal_x: Option<gpui::Pixels>,
    dragging: bool,
    scroll: ScrollHandle,
    layouts: Layouts,
    reveal: Rc<Cell<bool>>,
    _subscriptions: Vec<Subscription>,
}

impl VisualView {
    pub fn new(
        app: Entity<AppState>,
        document: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe_in(&app, window, |this, _, event, _, cx| {
            this.on_engine_event(event, cx);
        });
        let (shown, version) = {
            let engine = &app.read(cx).engine;
            (
                engine.visual(document),
                engine.document(document).map_or(0, |open| open.version),
            )
        };
        let mut view = Self {
            app,
            document,
            focus: cx.focus_handle(),
            shown,
            selection: Selection::default(),
            version,
            marks: MarkSet::EMPTY,
            composing: None,
            goal_x: None,
            dragging: false,
            scroll: ScrollHandle::new(),
            layouts: Layouts::default(),
            reveal: Rc::default(),
            _subscriptions: vec![subscription],
        };
        if let Some(first) = view
            .text_blocks()
            .into_iter()
            .find(|(_, editable, _)| *editable)
        {
            view.set_selection(Selection::caret(first.2.range.start), cx);
        }
        view
    }

    pub fn document(&self) -> DocumentId {
        self.document
    }

    #[cfg(test)]
    pub fn selection(&self) -> Selection {
        self.selection
    }

    #[cfg(test)]
    pub fn shown(&self) -> Option<&Arc<VisualDocument>> {
        self.shown.as_ref()
    }

    #[cfg(test)]
    pub fn layouts(&self) -> &Layouts {
        &self.layouts
    }

    fn on_engine_event(&mut self, event: &EngineEvent, cx: &mut Context<Self>) {
        match event {
            EngineEvent::ModelChanged { id, .. } if *id == self.document => {
                self.refresh(cx);
            }
            // An edit from elsewhere moves the selection with the text.
            EngineEvent::DocumentChanged { id, version, edits }
                if *id == self.document && *version > self.version =>
            {
                let edits: Vec<SourceEdit> = edits
                    .iter()
                    .map(|edit| SourceEdit::new(edit.range, edit.replacement.clone()))
                    .collect();
                let mapping = Mapping::new(&edits);
                self.selection = Selection {
                    anchor: mapping.map(self.selection.anchor, Assoc::After),
                    head: mapping.map(self.selection.head, Assoc::After),
                };
                self.version = *version;
                self.composing = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// Shows the engine's latest model, keeping the selection inside the text.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.shown = self.app.read(cx).engine.visual(self.document);
        if let Some(shown) = &self.shown {
            let length = shown.model.source.len();
            self.selection.anchor = self.selection.anchor.min(length);
            self.selection.head = self.selection.head.min(length);
        }
        cx.notify();
    }

    /// The text blocks in document order: path, whether editable, and the block.
    fn text_blocks(&self) -> Vec<(BlockPath, bool, TextBlock)> {
        self.shown
            .as_ref()
            .map(|shown| {
                shown
                    .model
                    .text_blocks()
                    .map(|(path, node, text)| (path, node.is_editable(), text.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The text block holding source offset `offset`.
    fn block_at(&self, offset: usize) -> Option<(BlockPath, TextBlock)> {
        let shown = self.shown.as_ref()?;
        let (path, _, text) = shown.model.text_block_at(offset)?;
        Some((path, text.clone()))
    }

    /// Moves the selection, taking the marks of the text before the caret for typing.
    fn set_selection(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.selection = selection;
        self.composing = None;
        if let Some((_, text)) = self.block_at(selection.head) {
            let display = text.display_offset(selection.head).unwrap_or(0);
            self.marks = text.marks_at(display);
        }
        self.reveal.set(true);
        cx.emit(VisualEvent::CaretMoved(selection.head));
        cx.notify();
    }

    /// Selects source offsets `anchor` to `head`.
    #[cfg(test)]
    pub fn select(&mut self, anchor: usize, head: usize, cx: &mut Context<Self>) {
        self.set_selection(Selection { anchor, head }, cx);
    }

    /// Places the caret at source offset `offset`, for tests and navigation.
    pub fn place_caret(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.set_selection(Selection::caret(offset), cx);
    }

    /// Runs a visual command on the selection; the command's selection becomes the view's.
    fn run(
        &mut self,
        command: CommandId,
        value: VisualValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let args = VisualArgs {
            id: self.document,
            version: self.version,
            selection: SourceRange::new(self.selection.anchor, self.selection.head),
            marks: self.marks,
            value,
        };
        let result = self.app.update(cx, |app, cx| {
            app.invoke(command, CommandArgs::Visual(args), cx)
        });
        match result {
            Ok(outcome) => {
                self.version = self
                    .app
                    .read(cx)
                    .engine
                    .document(self.document)
                    .map_or(self.version, |open| open.version);
                self.shown = self.app.read(cx).engine.visual(self.document);
                if let CommandOutcome::Selection(selection) = outcome {
                    let marks = self.marks;
                    self.set_selection(
                        Selection {
                            anchor: selection.start,
                            head: selection.end,
                        },
                        cx,
                    );
                    // Typing keeps the marks it was typed with.
                    if command == ids::VISUAL_INSERT_TEXT {
                        self.marks = marks;
                    }
                }
                self.goal_x = None;
                cx.notify();
                true
            }
            Err(error) => {
                window.push_notification(error.to_string(), cx);
                false
            }
        }
    }

    /// Types `text` over the selection.
    pub fn type_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.run(
            ids::VISUAL_INSERT_TEXT,
            VisualValue::Text(text.to_owned()),
            window,
            cx,
        );
    }

    // --- caret movement ----------------------------------------------------------------------

    /// The caret moved horizontally by a character, or to the next text block at an edge.
    fn step(&self, forward: bool) -> Option<usize> {
        let (path, text) = self.block_at(self.selection.head)?;
        let display = text.display_offset(self.selection.head)?;
        let next = if forward {
            text.display[display..]
                .chars()
                .next()
                .map(|character| display + character.len_utf8())
        } else {
            text.display[..display]
                .chars()
                .next_back()
                .map(|character| display - character.len_utf8())
        };
        match next {
            Some(next) => text.source_offset(next, text.marks_at(next)),
            None => {
                let blocks = self.text_blocks();
                let index = blocks.iter().position(|(other, _, _)| *other == path)?;
                let neighbour = if forward {
                    blocks.get(index + 1)
                } else {
                    index.checked_sub(1).and_then(|index| blocks.get(index))
                }?;
                let block = &neighbour.2;
                Some(if forward {
                    block.range.start
                } else {
                    block.range.end
                })
            }
        }
    }

    /// The caret moved to the start or end of its text block.
    fn edge(&self, end: bool) -> Option<usize> {
        let (_, text) = self.block_at(self.selection.head)?;
        Some(if end {
            text.range.end
        } else {
            text.range.start
        })
    }

    /// The caret moved a line up or down: in its block when it has more lines, else into the
    /// block above or below, keeping its horizontal position.
    fn vertical(&mut self, down: bool) -> Option<usize> {
        let (path, block) = self.block_at(self.selection.head)?;
        let display = block.display_offset(self.selection.head)?;
        let composing = self.composing.as_deref().map(|text| (display, text));
        let shown = text::shown(&block, composing);
        let (point, line_height) = text::caret_point(&self.layouts, &path, &shown, display)?;
        let x = *self.goal_x.get_or_insert(point.x);
        let target_y = if down {
            point.y + line_height * 1.5
        } else {
            point.y - line_height * 0.5
        };
        let bounds = text::bounds_of(&self.layouts, &path)?;
        if target_y >= bounds.top() && target_y <= bounds.bottom() {
            let index = text::hit(&self.layouts, &path, gpui::point(x, target_y), &shown)?;
            return block.source_offset(index, block.marks_at(index));
        }
        // The nearest block above or below that was drawn.
        let blocks = self.text_blocks();
        let index = blocks.iter().position(|(other, _, _)| *other == path)?;
        let candidates: Vec<&(BlockPath, bool, TextBlock)> = if down {
            blocks[index + 1..].iter().collect()
        } else {
            blocks[..index].iter().rev().collect()
        };
        for (other, _, other_block) in candidates {
            let Some(other_bounds) = text::bounds_of(&self.layouts, other) else {
                continue;
            };
            let y = if down {
                other_bounds.top() + px(1.)
            } else {
                other_bounds.bottom() - px(1.)
            };
            let other_shown = text::shown(other_block, None);
            let index = text::hit(&self.layouts, other, gpui::point(x, y), &other_shown)?;
            return other_block.source_offset(index, other_block.marks_at(index));
        }
        Some(if down {
            block.range.end
        } else {
            block.range.start
        })
    }

    fn move_to(&mut self, offset: Option<usize>, select: bool, cx: &mut Context<Self>) {
        let Some(offset) = offset else {
            return;
        };
        let selection = if select {
            Selection {
                anchor: self.selection.anchor,
                head: offset,
            }
        } else {
            Selection::caret(offset)
        };
        self.set_selection(selection, cx);
    }

    // --- mouse -------------------------------------------------------------------------------

    /// The source offset under `position` in the text block at `path`.
    fn offset_at(&self, path: &[usize], position: gpui::Point<gpui::Pixels>) -> Option<usize> {
        let shown_document = self.shown.as_ref()?;
        let node = shown_document.model.node(path)?;
        let block = node.text.as_ref()?;
        let caret_display = (self
            .block_at(self.selection.head)
            .map(|(at, _)| at)
            .as_deref()
            == Some(path))
        .then(|| block.display_offset(self.selection.head))
        .flatten();
        let composing = caret_display.zip(self.composing.as_deref());
        let shown = text::shown(block, composing);
        let display = text::hit(&self.layouts, path, position, &shown)?;
        block.source_offset(display, block.marks_at(display))
    }

    fn mouse_down(
        &mut self,
        path: &[usize],
        position: gpui::Point<gpui::Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let editable = self
            .shown
            .as_ref()
            .and_then(|shown| shown.model.node(path))
            .is_some_and(adoc_engine::Node::is_editable);
        if !editable {
            window.push_notification(
                "This text can only be edited in the source: Show Source and Visual Editor (Cmd/Ctrl-K D).",
                cx,
            );
            return;
        }
        let Some(offset) = self.offset_at(path, position) else {
            return;
        };
        self.goal_x = None;
        self.dragging = true;
        self.move_to(Some(offset), extend, cx);
    }

    fn mouse_drag(
        &mut self,
        path: &[usize],
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging {
            return;
        }
        if let Some(offset) = self.offset_at(path, position) {
            if offset != self.selection.head {
                self.move_to(Some(offset), true, cx);
            }
        }
    }

    // --- reading the selection ---------------------------------------------------------------

    /// The text the selection shows, blocks separated by blank lines.
    fn selected_text(&self) -> String {
        let (start, end) = (self.selection.start(), self.selection.end());
        let mut parts = Vec::new();
        for (_, _, block) in self.text_blocks() {
            if block.range.end < start || block.range.start > end {
                continue;
            }
            let from = block
                .display_offset(start.max(block.range.start))
                .unwrap_or(0);
            let to = block
                .display_offset_towards(end.min(block.range.end), true)
                .unwrap_or(block.display.len());
            if from < to {
                parts.push(block.display[from..to].to_owned());
            }
        }
        parts.join("\n\n")
    }
}

impl gpui::EventEmitter<VisualEvent> for VisualView {}

impl Focusable for VisualView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for VisualView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Repopulated as this frame's text blocks are drawn.
        self.layouts.borrow_mut().clear();
        let body = self
            .shown
            .clone()
            .map(|shown| blocks::document(self, &shown, window, cx));
        let entity = cx.entity();
        let focus = self.focus.clone();
        v_flex()
            .id("visual-editor")
            .key_context(VISUAL_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .cursor_text()
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .map(|view| input::actions(view, cx))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("visual-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        v_flex()
                            .p_4()
                            .gap_3()
                            .max_w(px(900.))
                            .when_some(body, |content, body| content.children(body))
                            .when(self.shown.is_none(), |content| {
                                content.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Preparing the visual editor…"),
                                )
                            }),
                    ),
            )
    }
}
