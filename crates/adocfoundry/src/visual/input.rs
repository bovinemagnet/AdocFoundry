//! Keyboard and text input: caret movement and deletion as actions, typed text and IME
//! composition through the platform input handler.

use std::ops::Range;

use adoc_engine::{commands::ids, VisualValue};
use gpui::{
    actions, App, Bounds, ClipboardItem, Context, EntityInputHandler, InteractiveElement as _,
    KeyBinding, Pixels, Point, Stateful, UTF16Selection, Window,
};

use super::{text, Selection, VisualView, VISUAL_CONTEXT};

actions!(
    visual,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
    ]
);

/// Binds the visual editor's keys.
pub fn bind(cx: &mut App) {
    let context = Some(VISUAL_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("secondary-a", SelectAll, context),
        KeyBinding::new("secondary-c", Copy, context),
        KeyBinding::new("secondary-x", Cut, context),
        KeyBinding::new("secondary-v", Paste, context),
    ]);
}

/// The view's action handlers.
pub fn actions(view: Stateful<gpui::Div>, cx: &mut Context<VisualView>) -> Stateful<gpui::Div> {
    view.on_action(cx.listener(|this, _: &Backspace, window, cx| {
        this.run(ids::VISUAL_DELETE_BACKWARD, VisualValue::None, window, cx);
    }))
    .on_action(cx.listener(|this, _: &Delete, window, cx| {
        this.run(ids::VISUAL_DELETE_FORWARD, VisualValue::None, window, cx);
    }))
    .on_action(cx.listener(|this, _: &Left, _, cx| {
        let target = if this.selection.is_empty() {
            this.step(false)
        } else {
            Some(this.selection.start())
        };
        this.goal_x = None;
        this.move_to(target, false, cx);
    }))
    .on_action(cx.listener(|this, _: &Right, _, cx| {
        let target = if this.selection.is_empty() {
            this.step(true)
        } else {
            Some(this.selection.end())
        };
        this.goal_x = None;
        this.move_to(target, false, cx);
    }))
    .on_action(cx.listener(|this, _: &SelectLeft, _, cx| {
        let target = this.step(false);
        this.goal_x = None;
        this.move_to(target, true, cx);
    }))
    .on_action(cx.listener(|this, _: &SelectRight, _, cx| {
        let target = this.step(true);
        this.goal_x = None;
        this.move_to(target, true, cx);
    }))
    .on_action(cx.listener(|this, _: &Up, _, cx| {
        let target = this.vertical(false);
        let goal = this.goal_x;
        this.move_to(target, false, cx);
        this.goal_x = goal;
    }))
    .on_action(cx.listener(|this, _: &Down, _, cx| {
        let target = this.vertical(true);
        let goal = this.goal_x;
        this.move_to(target, false, cx);
        this.goal_x = goal;
    }))
    .on_action(cx.listener(|this, _: &SelectUp, _, cx| {
        let target = this.vertical(false);
        let goal = this.goal_x;
        this.move_to(target, true, cx);
        this.goal_x = goal;
    }))
    .on_action(cx.listener(|this, _: &SelectDown, _, cx| {
        let target = this.vertical(true);
        let goal = this.goal_x;
        this.move_to(target, true, cx);
        this.goal_x = goal;
    }))
    .on_action(cx.listener(|this, _: &Home, _, cx| {
        let target = this.edge(false);
        this.goal_x = None;
        this.move_to(target, false, cx);
    }))
    .on_action(cx.listener(|this, _: &End, _, cx| {
        let target = this.edge(true);
        this.goal_x = None;
        this.move_to(target, false, cx);
    }))
    .on_action(cx.listener(|this, _: &SelectHome, _, cx| {
        let target = this.edge(false);
        this.move_to(target, true, cx);
    }))
    .on_action(cx.listener(|this, _: &SelectEnd, _, cx| {
        let target = this.edge(true);
        this.move_to(target, true, cx);
    }))
    .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
        let blocks = this.text_blocks();
        if let (Some(first), Some(last)) = (blocks.first(), blocks.last()) {
            let selection = Selection {
                anchor: first.2.range.start,
                head: last.2.range.end,
            };
            this.set_selection(selection, cx);
        }
    }))
    .on_action(cx.listener(|this, _: &Copy, _, cx| {
        if !this.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(this.selected_text()));
        }
    }))
    .on_action(cx.listener(|this, _: &Cut, window, cx| {
        if !this.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(this.selected_text()));
            this.run(ids::VISUAL_DELETE_BACKWARD, VisualValue::None, window, cx);
        }
    }))
    .on_action(cx.listener(|this, _: &Paste, window, cx| {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            this.type_text(&text, window, cx);
        }
    }))
}

impl VisualView {
    /// The text the platform input handler sees: the caret's text block as shown, with any
    /// composition, and the caret's display offset in it.
    fn input_text(&self) -> Option<(text::Shown, usize)> {
        let (_, block) = self.block_at(self.selection.head)?;
        let display = block.display_offset(self.selection.head)?;
        let composing = self.composing.as_deref().map(|text| (display, text));
        Some((text::shown(&block, composing), display))
    }

    /// The selection's display range in the caret's block, clipped to it.
    fn selection_in_block(&self) -> Option<Range<usize>> {
        let (_, block) = self.block_at(self.selection.head)?;
        let clip = |offset: usize| offset.clamp(block.range.start, block.range.end);
        let start = block.display_offset(clip(self.selection.start()))?;
        let end = block.display_offset_towards(clip(self.selection.end()), true)?;
        Some(start..end)
    }

    /// Selects the source range a display range of the caret's block covers.
    fn select_display(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let Some((_, block)) = self.block_at(self.selection.head) else {
            return;
        };
        let (Some(start), Some(end)) = (
            block.source_offset(range.start, block.marks_at(range.start)),
            block.source_offset(range.end, block.marks_at(range.end)),
        ) else {
            return;
        };
        let marks = self.marks;
        self.set_selection(
            Selection {
                anchor: start,
                head: end,
            },
            cx,
        );
        self.marks = marks;
    }
}

fn to_utf16(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].encode_utf16().count()
}

fn from_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        if units >= offset {
            return index;
        }
        units += character.len_utf16();
    }
    text.len()
}

impl EntityInputHandler for VisualView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (shown, _) = self.input_text()?;
        let start = from_utf16(&shown.text, range.start);
        let end = from_utf16(&shown.text, range.end);
        adjusted.replace(to_utf16(&shown.text, start)..to_utf16(&shown.text, end));
        Some(shown.text[start..end].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (shown, _) = self.input_text()?;
        let range = self.selection_in_block()?;
        let start = shown.index(range.start);
        let end = shown.index(range.end);
        Some(UTF16Selection {
            range: to_utf16(&shown.text, start)..to_utf16(&shown.text, end),
            reversed: self.selection.head < self.selection.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let (shown, _) = self.input_text()?;
        let composing = shown.composing.clone()?;
        Some(to_utf16(&shown.text, composing.start)..to_utf16(&shown.text, composing.end))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The composition stays as it is, committed.
        if let Some(text) = self.composing.take() {
            self.type_text(&text, window, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composing.take().is_none() {
            if let (Some(range), Some((shown, _))) = (range, self.input_text()) {
                let start = shown.display(from_utf16(&shown.text, range.start));
                let end = shown.display(from_utf16(&shown.text, range.end));
                self.select_display(start..end, cx);
            }
        }
        self.type_text(text, window, cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composing.is_none() {
            if let (Some(range), Some((shown, _))) = (range, self.input_text()) {
                let start = shown.display(from_utf16(&shown.text, range.start));
                let end = shown.display(from_utf16(&shown.text, range.end));
                self.select_display(start..end, cx);
            }
        }
        self.composing = (!text.is_empty()).then(|| text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (shown, _) = self.input_text()?;
        let (path, _) = self.block_at(self.selection.head)?;
        let display = shown.display(from_utf16(&shown.text, range.start));
        let (point, line_height) = text::caret_point(&self.layouts, &path, &shown, display)?;
        Some(Bounds::new(point, gpui::size(gpui::px(2.), line_height)))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let (shown, _) = self.input_text()?;
        let (path, _) = self.block_at(self.selection.head)?;
        let display = text::hit(&self.layouts, &path, point, &shown)?;
        Some(to_utf16(&shown.text, shown.index(display)))
    }
}
