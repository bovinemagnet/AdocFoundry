//! Selecting text the visual editor shows but does not edit (#794).
//!
//! The selection edits act on is a pair of source offsets. Text drawn from the view tree, such as
//! all that an include brings in, has no offsets of its own, so a selection with an end in it is
//! kept apart, as a *reading* selection: it can be copied, and every edit is refused while there
//! is one. What either covers is worked out by the engine.

use std::{rc::Rc, sync::Arc};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, BlockPath, CommandArgs, CommandId, CommandOutcome, CopySource, TextPlace,
    VisualDocument, VisualFlow, VisualValue,
};
use adoc_render::fragment::Place;
use gpui::{
    px, Bounds, Context, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Window,
};
use gpui_component::{menu::PopupMenuItem, WindowExt as _};

use super::{text, ReadOnlyNotice, Selection, VisualView};
use crate::{
    copying::{Can, Menu},
    preview::select::{self, Piece, Selected},
};

/// A selection with an end in text drawn from the view tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub anchor: TextPlace,
    pub head: TextPlace,
}

/// What is selected, worked out for a frame's rows.
#[derive(Clone)]
pub struct Highlight {
    flow: Rc<VisualFlow>,
    from: Place,
    to: Place,
}

impl Highlight {
    /// The selected part of the text block at `path`, in its display text.
    pub fn block(&self, path: &[usize]) -> Option<std::ops::Range<usize>> {
        let leaf = (self.from.leaf..=self.to.leaf)
            .find(|leaf| self.flow.block(*leaf).is_some_and(|block| block == path))?;
        self.flow.flow().selected(leaf, self.from, self.to)
    }

    /// The selected part of text drawn from the view tree.
    pub fn drawn(&self) -> Selected {
        let highlight = self.clone();
        Selected::new(move |key| {
            let leaf = highlight.flow.flow().position(key)?;
            highlight
                .flow
                .is_drawn(leaf)
                .then(|| {
                    highlight
                        .flow
                        .flow()
                        .selected(leaf, highlight.from, highlight.to)
                })
                .flatten()
        })
    }
}

/// How a key extends the selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Step {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    Home,
    End,
}

/// Text on screen that a point can be in.
enum Target {
    Block(BlockPath),
    Drawn(Piece),
}

impl VisualView {
    /// The reading selection, if there is one.
    pub fn reading(&self) -> Option<&Reading> {
        self.reading.as_ref()
    }

    /// The page's text in order, as shown.
    fn flow(&self) -> Option<(Arc<VisualDocument>, Rc<VisualFlow>)> {
        let shown = self.shown.clone()?;
        let mut kept = self.flow.borrow_mut();
        if let Some((of, flow)) = kept.as_ref() {
            if Arc::ptr_eq(of, &shown) {
                return Some((shown, flow.clone()));
            }
        }
        let flow = Rc::new(VisualFlow::new(&shown));
        *kept = Some((shown.clone(), flow.clone()));
        Some((shown, flow))
    }

    /// The selection's ends, where it started first.
    fn ends(&self) -> (TextPlace, TextPlace) {
        match &self.reading {
            Some(reading) => (reading.anchor.clone(), reading.head.clone()),
            None => (
                TextPlace::Source(self.selection.anchor),
                TextPlace::Source(self.selection.head),
            ),
        }
    }

    /// Whether anything is selected, in either way.
    pub fn has_selection(&self) -> bool {
        self.reading.is_some() || !self.selection.is_empty()
    }

    /// What is selected, for drawing.
    pub fn highlight(&self) -> Option<Highlight> {
        if !self.has_selection() {
            return None;
        }
        let (document, flow) = self.flow()?;
        let (anchor, head) = self.ends();
        let (from, to) = flow.ordered(&document, &anchor, &head)?;
        Some(Highlight { flow, from, to })
    }

    /// Selects between two places: as source offsets when both have them, so that edits can act
    /// on it.
    pub(super) fn select_between(
        &mut self,
        anchor: TextPlace,
        head: TextPlace,
        cx: &mut Context<Self>,
    ) {
        match (&anchor, &head) {
            (TextPlace::Source(anchor), TextPlace::Source(head)) => {
                let selection = Selection {
                    anchor: *anchor,
                    head: *head,
                };
                if self.reading.is_some() || self.selection != selection {
                    self.set_selection(selection, cx);
                }
            }
            _ if anchor == head => {
                if self.reading.take().is_some() {
                    cx.notify();
                }
            }
            _ => {
                // Where the caret is while there is one: what acts on source offsets has none
                // of it selected.
                if let TextPlace::Source(offset) = anchor {
                    self.selection = Selection::caret(offset);
                } else {
                    self.selection = Selection::caret(self.selection.head);
                }
                let reading = Some(Reading { anchor, head });
                if self.reading != reading {
                    self.reading = reading;
                    self.composing = None;
                    self.pending = None;
                    cx.notify();
                }
            }
        }
    }

    /// Keeps a reading selection over a new model while the text it ends in is the same, else
    /// lets it go.
    pub(super) fn keep_reading(&mut self, before: Option<&Arc<VisualDocument>>) {
        let Some(reading) = &self.reading else {
            return;
        };
        let kept = before
            .zip(self.shown.as_ref())
            .is_some_and(|(before, after)| {
                let (before, after) = (VisualFlow::new(before), VisualFlow::new(after));
                let text = |flow: &VisualFlow, place: &TextPlace| match place {
                    TextPlace::Source(_) => Some(String::new()),
                    TextPlace::Drawn { leaf, .. } => flow
                        .flow()
                        .position(leaf)
                        .map(|leaf| flow.flow().leaves()[leaf].text.clone()),
                };
                [&reading.anchor, &reading.head].into_iter().all(|place| {
                    let before = text(&before, place);
                    before.is_some() && before == text(&after, place)
                })
            });
        if !kept {
            self.reading = None;
            self.pressed = None;
        }
    }

    /// Says that the selection holds text that is not edited here, when an edit is refused.
    pub(super) fn refuse_reading(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.reading.is_none() {
            return false;
        }
        self.notice(window, cx);
        true
    }

    /// Says that text is edited in the source, once however often it is said.
    pub(super) fn notice(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification(
            gpui_component::notification::Notification::new()
                .message(
                    "This text can only be edited in the source: Show Source and Visual \
                     Editor (Cmd/Ctrl-K D).",
                )
                .id::<ReadOnlyNotice>(),
            cx,
        );
    }

    /// Where `needle` starts or ends on screen, in the first text drawn that holds it.
    #[cfg(test)]
    pub fn drawn_point_for_test(&self, needle: &str, end: bool) -> Option<Point<Pixels>> {
        let (document, flow) = self.flow()?;
        let (leaf, at) = flow
            .flow()
            .leaves()
            .iter()
            .enumerate()
            .find_map(|(index, leaf)| leaf.text.find(needle).map(|at| (index, at)))?;
        let offset = if end { at + needle.len() } else { at };
        let place = self.text_place(&document, &flow, Place::new(leaf, offset))?;
        let (_, _, point, height) = self.on_screen(&place)?;
        Some(gpui::point(point.x, point.y + height / 2.))
    }

    /// Each text of the page.
    #[cfg(test)]
    pub fn texts_for_test(&self) -> Vec<adoc_render::fragment::Leaf> {
        self.flow()
            .map_or_else(Vec::new, |(_, flow)| flow.flow().leaves().to_vec())
    }

    /// Whether the start of text `leaf` is on screen.
    #[cfg(test)]
    pub fn leaf_drawn_for_test(&self, leaf: usize) -> bool {
        self.flow()
            .and_then(|(document, flow)| self.text_place(&document, &flow, Place::new(leaf, 0)))
            .is_some_and(|place| self.on_screen(&place).is_some())
    }

    /// Scrolls to the row holding text `leaf`.
    #[cfg(test)]
    pub fn reveal_leaf_for_test(&mut self, leaf: usize, cx: &mut Context<Self>) {
        let Some((document, flow)) = self.flow() else {
            return;
        };
        let trail = flow.flow().leaves()[leaf].key.trail.clone();
        let path = flow.block(leaf).cloned().or_else(|| {
            // The innermost node drawn from a block around it.
            document
                .model
                .walk()
                .into_iter()
                .filter(|(_, node)| {
                    node.origin
                        .as_ref()
                        .is_some_and(|origin| trail.starts_with(origin))
                })
                .max_by_key(|(_, node)| node.origin.as_ref().map(Vec::len))
                .map(|(path, _)| path)
        });
        if let Some(row) = path.and_then(|path| super::blocks::row_of(&self.rows, &path)) {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: row,
                offset_in_item: px(0.),
            });
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn menu_open_for_test(&self) -> bool {
        self.menu.is_some()
    }

    // --- where things are on screen ----------------------------------------------------------

    /// The text on screen, with its bounds.
    fn targets(&self) -> Vec<(Bounds<Pixels>, Target)> {
        let mut out: Vec<(Bounds<Pixels>, Target)> = Vec::new();
        if let Some(shown) = &self.shown {
            for (path, layout) in self.layouts.borrow().iter() {
                if shown
                    .model
                    .node(path)
                    .is_none_or(|node| node.text.is_none())
                {
                    continue;
                }
                out.push((layout.bounds(), Target::Block(path.clone())));
            }
        }
        out.extend(
            self.layer
                .pieces()
                .into_iter()
                .map(|piece| (piece.bounds(), Target::Drawn(piece))),
        );
        out
    }

    fn place_in(&self, target: &Target, position: Point<Pixels>) -> Option<TextPlace> {
        match target {
            Target::Block(path) => self.offset_at(path, position).map(TextPlace::Source),
            Target::Drawn(piece) => Some(TextPlace::Drawn {
                leaf: piece.key.clone(),
                offset: piece.offset_at(position),
            }),
        }
    }

    /// The place nearest `position`.
    fn nearest_place(&self, position: Point<Pixels>) -> Option<TextPlace> {
        let targets = self.targets();
        let (_, target) = targets.iter().min_by(|(one, _), (other, _)| {
            select::distance(one, position)
                .partial_cmp(&select::distance(other, position))
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        self.place_in(target, position)
    }

    /// The target `place` is in, the top of its line there, and the line's height.
    fn on_screen(
        &self,
        place: &TextPlace,
    ) -> Option<(Bounds<Pixels>, Target, Point<Pixels>, Pixels)> {
        match place {
            TextPlace::Source(offset) => {
                let (path, block) = self.block_at(*offset)?;
                let display = block.display_offset(*offset)?;
                let shown = self.shown_text(&block, None);
                let (point, height) = text::caret_point(&self.layouts, &path, &shown, display)?;
                let bounds = text::bounds_of(&self.layouts, &path)?;
                Some((bounds, Target::Block(path), point, height))
            }
            TextPlace::Drawn { leaf, offset } => {
                let piece = self.layer.holding(leaf, *offset)?;
                let (point, height) = piece.point_of(*offset)?;
                Some((piece.bounds(), Target::Drawn(piece), point, height))
            }
        }
    }

    // --- the keyboard ------------------------------------------------------------------------

    /// Extends the selection by a key, through read-only text too.
    pub(super) fn extend(&mut self, step: Step, cx: &mut Context<Self>) {
        let (anchor, head) = self.ends();
        let goal = match step {
            Step::Up | Step::Down => self.goal_x,
            _ => None,
        };
        let target = match step {
            Step::Up | Step::Down => self.line_away(&head, step == Step::Down, goal),
            Step::Home | Step::End => self.line_end(&head, step == Step::End),
            _ => self.along(&head, step),
        };
        let Some((place, goal)) = target else {
            return;
        };
        self.select_between(anchor, place, cx);
        self.goal_x = goal;
    }

    /// The place a character or a word along from `place`, into the text before or after it at
    /// an edge.
    fn along(&self, place: &TextPlace, step: Step) -> Option<(TextPlace, Option<Pixels>)> {
        let (document, flow) = self.flow()?;
        let forward = matches!(step, Step::Right | Step::WordRight);
        // From a block with no text, such as an empty cell, on to the next.
        if let TextPlace::Source(offset) = place {
            if let Some((path, _)) = self.block_at(*offset) {
                if !flow.has_text(&path) {
                    let edge = self.neighbour_edge(&path, forward)?;
                    return Some((TextPlace::Source(edge), None));
                }
            }
        }
        let at = flow.place(&document, place, false)?;
        let text = &flow.flow().leaves()[at.leaf].text;
        let offset = at.offset.min(text.len());
        let next = match step {
            Step::Right => text[offset..]
                .chars()
                .next()
                .map(|character| offset + character.len_utf8()),
            Step::Left => text[..offset]
                .chars()
                .next_back()
                .map(|character| offset - character.len_utf8()),
            Step::WordRight => {
                Some(adoc_engine::next_word_end(text, offset)).filter(|next| *next != offset)
            }
            _ => {
                Some(adoc_engine::previous_word_start(text, offset)).filter(|next| *next != offset)
            }
        };
        let next = match next {
            Some(next) => Place::new(at.leaf, next),
            None if forward => Place::new(at.leaf + 1, 0),
            None => {
                let leaf = at.leaf.checked_sub(1)?;
                Place::new(leaf, flow.flow().leaves()[leaf].text.len())
            }
        };
        Some((self.text_place(&document, &flow, next)?, None))
    }

    /// `place` of the flow as a place in the editor.
    fn text_place(
        &self,
        document: &VisualDocument,
        flow: &VisualFlow,
        place: Place,
    ) -> Option<TextPlace> {
        let leaf = flow.flow().leaves().get(place.leaf)?;
        Some(match flow.block(place.leaf) {
            Some(path) => {
                let text = document.model.node(path)?.text.as_ref()?;
                TextPlace::Source(text.source_offset(place.offset, text.marks_at(place.offset))?)
            }
            None => TextPlace::Drawn {
                leaf: leaf.key.clone(),
                offset: place.offset,
            },
        })
    }

    /// The start or end of the line `place` is on.
    fn line_end(&self, place: &TextPlace, end: bool) -> Option<(TextPlace, Option<Pixels>)> {
        let (bounds, target, point, height) = self.on_screen(place)?;
        let x = if end {
            bounds.right() + px(10_000.)
        } else {
            bounds.left() - px(1.)
        };
        let place = self.place_in(&target, gpui::point(x, point.y + height / 2.))?;
        Some((place, None))
    }

    /// The place a line above or below `place`, and the horizontal position kept for the next.
    fn line_away(
        &self,
        place: &TextPlace,
        down: bool,
        goal: Option<Pixels>,
    ) -> Option<(TextPlace, Option<Pixels>)> {
        let Some((bounds, target, point, height)) = self.on_screen(place) else {
            return self.along(place, if down { Step::Right } else { Step::Left });
        };
        let x = goal.unwrap_or(point.x);
        let y = if down {
            point.y + height * 1.5
        } else {
            point.y - height * 0.5
        };
        if y >= bounds.top() && y <= bounds.bottom() {
            return Some((self.place_in(&target, gpui::point(x, y))?, Some(x)));
        }
        // The nearest text above or below, preferring what is under the caret.
        let targets = self.targets();
        let best = targets
            .iter()
            .filter(|(other, _)| {
                if down {
                    other.top() >= bounds.bottom() - px(1.)
                } else {
                    other.bottom() <= bounds.top() + px(1.)
                }
            })
            .min_by(|(one, _), (other, _)| {
                let rank = |bounds: &Bounds<Pixels>| {
                    let (away, across) = select::distance(bounds, gpui::point(x, point.y));
                    // Text on one row competes on how far across it is.
                    ((f32::from(away) / f32::from(height)).floor(), across)
                };
                rank(one)
                    .partial_cmp(&rank(other))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        match best {
            Some((other, target)) => {
                let y = if down {
                    other.top() + px(1.)
                } else {
                    other.bottom() - px(1.)
                };
                Some((self.place_in(target, gpui::point(x, y))?, Some(x)))
            }
            // Nothing laid out beyond: on to the next text in order.
            None => {
                let (document, flow) = self.flow()?;
                let at = flow.place(&document, place, false)?;
                let next = if down {
                    Place::new(at.leaf + 1, 0)
                } else {
                    let leaf = at.leaf.checked_sub(1)?;
                    Place::new(leaf, flow.flow().leaves()[leaf].text.len())
                };
                Some((self.text_place(&document, &flow, next)?, Some(x)))
            }
        }
    }

    /// Selects the whole page.
    pub(super) fn select_page(&mut self, cx: &mut Context<Self>) {
        let Some((document, flow)) = self.flow() else {
            return;
        };
        let Some((from, to)) = flow.flow().ends() else {
            return;
        };
        let (Some(anchor), Some(head)) = (
            self.text_place(&document, &flow, from),
            self.text_place(&document, &flow, to),
        ) else {
            return;
        };
        // The whole of its blocks, marks and all, where they are the editor's own.
        let edge = |place: TextPlace, end: bool| match place {
            TextPlace::Source(offset) => {
                let range = document
                    .model
                    .text_block_at(offset)
                    .map_or(SourceRange::new(offset, offset), |(_, _, text)| text.range);
                TextPlace::Source(if end { range.end } else { range.start })
            }
            drawn => drawn,
        };
        self.select_between(edge(anchor, false), edge(head, true), cx);
    }

    // --- the pointer -------------------------------------------------------------------------

    /// The pointer was pressed on what the editor does not edit.
    pub(super) fn press_drawn(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.layer.set_dragged(false);
        self.showing_source = None;
        let Some(piece) = self.layer.at(event.position) else {
            return;
        };
        window.focus(&self.focus, cx);
        let offset = piece.offset_at(event.position);
        let place = TextPlace::Drawn {
            leaf: piece.key.clone(),
            offset,
        };
        self.goal_x = None;
        if event.modifiers.shift {
            let (anchor, _) = self.ends();
            self.layer.set_dragged(true);
            self.select_between(anchor, place, cx);
            return;
        }
        if event.click_count > 1 {
            let Some((_, flow)) = self.flow() else {
                return;
            };
            let Some(leaf) = flow.flow().position(&piece.key) else {
                return;
            };
            let text = &flow.flow().leaves()[leaf].text;
            let range = if event.click_count == 2 {
                adoc_engine::word_at(text, offset.min(text.len()))
            } else {
                0..text.len()
            };
            let at = |offset| TextPlace::Drawn {
                leaf: piece.key.clone(),
                offset,
            };
            self.pressed = None;
            self.layer.set_dragged(true);
            self.select_between(at(range.start), at(range.end), cx);
            return;
        }
        self.pressed = Some(place);
        self.dragging = true;
        if self.reading.take().is_some() {
            cx.notify();
        }
    }

    /// The pointer moved over the page.
    pub(super) fn drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(anchor) = self.pressed.clone() else {
            return;
        };
        let Some(place) = self.nearest_place(event.position) else {
            return;
        };
        if place == anchor && !self.has_selection() {
            return;
        }
        self.layer.set_dragged(true);
        self.select_between(anchor, place, cx);
    }

    /// Shows the source of a read-only block beside the editor, unless another click follows.
    pub(super) fn show_source_later(
        &mut self,
        position: adoc_engine::NavigationPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.showing_source = Some(cx.spawn_in(window, async move |view, cx| {
            cx.background_executor().timer(select::DOUBLE_CLICK).await;
            let _ = view.update(cx, |view, cx| {
                view.showing_source = None;
                // A failure is reported by the window.
                let _ = view.app.update(cx, |app, cx| {
                    app.invoke(ids::VIEW_SHOW_SPLIT, CommandArgs::None, cx)?;
                    app.invoke(ids::NAVIGATE_REVEAL, CommandArgs::Position(position), cx)
                });
            });
        }));
    }

    // --- copying -----------------------------------------------------------------------------

    /// What the copy commands copy.
    fn copy_source(&self) -> Option<CopySource> {
        if !self.has_selection() {
            return None;
        }
        let (from, to) = self.ends();
        Some(CopySource::Visual {
            id: self.document,
            version: self.version,
            from,
            to,
        })
    }

    /// What `command` copies of the selection.
    fn copied(
        &mut self,
        command: CommandId,
        cx: &mut Context<Self>,
    ) -> Option<adoc_engine::CopiedText> {
        let source = self.copy_source()?;
        // A failure is reported by the window.
        match self.app.update(cx, |app, cx| {
            app.invoke(command, CommandArgs::Copy(source), cx)
        }) {
            Ok(CommandOutcome::Copied(copied)) => Some(copied),
            _ => None,
        }
    }

    pub(super) fn copy(&mut self, command: CommandId, cx: &mut Context<Self>) {
        if let Some(copied) = self.copied(command, cx) {
            crate::copying::write(&copied, cx);
        }
    }

    pub(super) fn copy_as(
        &mut self,
        command: CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_selection() {
            self.copy(command, cx);
        } else {
            crate::copying::nothing_selected(window, cx);
        }
    }

    pub(super) fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_selection() || self.refuse_reading(window, cx) {
            return;
        }
        // Only what was cut goes to the clipboard: the engine may refuse the delete.
        let copied = self.copied(ids::EDIT_COPY_PLAIN, cx);
        if self.run(ids::VISUAL_DELETE_BACKWARD, VisualValue::None, window, cx) {
            if let Some(copied) = copied {
                crate::copying::write(&copied, cx);
            }
        }
    }

    /// Opens the menu of a right click at `position`, in the text block at `path` if it is in
    /// one: with the spelling choices for the word there.
    pub(super) fn open_menu(
        &mut self,
        path: Option<&[usize]>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The menu takes the focus, not what was clicked.
        cx.stop_propagation();
        window.prevent_default();
        window.focus(&self.focus, cx);
        let offset = path.and_then(|path| self.offset_at(path, position));
        // A click outside the selection moves the caret there, as it does in the source editor.
        if let Some(offset) = offset {
            if !self.has_selection() {
                self.set_selection(Selection::caret(offset), cx);
            }
        }
        let choices = offset.map_or_else(Vec::new, |offset| {
            crate::spelling::choices(&self.app.read(cx).engine, self.document, offset)
        });
        let mut items = Vec::new();
        let mut suggested = false;
        for (label, choice) in choices {
            let replace = matches!(choice, crate::spelling::Choice::Replace { .. });
            if !replace && suggested {
                items.push(PopupMenuItem::separator());
                suggested = false;
            }
            suggested |= replace;
            let (app, document) = (self.app.clone(), self.document);
            items.push(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                // A failure is reported by the window.
                let _ = crate::spelling::apply(&app, document, &choice, cx);
            }));
        }
        let editable = self.reading.is_none();
        let can = Can {
            cut: editable && !self.selection.is_empty(),
            copy: self.has_selection(),
            paste: editable,
        };
        let menu = crate::copying::menu(items, can, self.focus.clone(), window, cx);
        self.menu = Some(Menu::open(
            position,
            menu,
            window,
            cx,
            |this: &mut Self, _| this.menu = None,
        ));
        cx.notify();
    }
}
