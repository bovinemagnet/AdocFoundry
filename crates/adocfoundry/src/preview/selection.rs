//! Selecting and copying the preview's text (#794).

use std::{rc::Rc, sync::Arc};

use adoc_engine::{
    commands::ids, CommandArgs, CommandId, CommandOutcome, CopySource, RenderedPreview,
};
use adoc_render::fragment::{Flow, LeafKey, Place};
use gpui::{
    App, Context, InteractiveElement as _, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, Window,
};

use super::{
    select::{self, Selected},
    PreviewView,
};
use crate::{
    copying::{Copy, CopyAsAsciiDoc, CopyAsFormatted, CopyAsMarkdown, CopyAsPlain, SelectAll},
    editor::bindings::key_binding,
};

/// The key context of the rendered preview.
pub const PREVIEW_CONTEXT: &str = "Preview";

/// Binds the preview's keys.
pub fn bind(cx: &mut App) {
    let context = Some(PREVIEW_CONTEXT);
    let bindings: [KeyBinding; 2] = [
        key_binding("secondary-c", Copy, context, cx),
        key_binding("secondary-a", SelectAll, context, cx),
    ];
    cx.bind_keys(bindings);
}

/// The preview's selection: two places in its text, and what is being dragged.
#[derive(Default)]
pub(super) struct Selection {
    /// Where it started and where it has got to.
    pub ends: Option<((LeafKey, usize), (LeafKey, usize))>,
    /// Where the pointer was pressed, while it is down.
    pressed: Option<(LeafKey, usize)>,
    /// A click about to show the source of what it was on.
    showing: Option<gpui::Task<()>>,
    flow: Option<(Arc<RenderedPreview>, Rc<Flow>)>,
    menu: Option<crate::copying::Menu>,
}

impl PreviewView {
    /// The text of the page shown, in order.
    fn flow(&mut self) -> Option<Rc<Flow>> {
        let shown = self.shown.clone()?;
        if let Some((of, flow)) = &self.selection.flow {
            if Arc::ptr_eq(of, &shown) {
                return Some(flow.clone());
            }
        }
        let flow = Rc::new(Flow::new(&shown.view));
        self.selection.flow = Some((shown, flow.clone()));
        Some(flow)
    }

    /// The selection's places in order, when it covers anything.
    fn selected_places(&mut self) -> Option<(Rc<Flow>, Place, Place)> {
        let (one, other) = self.selection.ends.clone()?;
        let flow = self.flow()?;
        let one = flow.place(&one.0, one.1)?;
        let other = flow.place(&other.0, other.1)?;
        (one != other).then(|| (flow, one.min(other), one.max(other)))
    }

    /// What is selected, for drawing.
    pub(super) fn selected(&mut self) -> Option<Selected> {
        let (flow, from, to) = self.selected_places()?;
        Some(Selected::between(flow, from, to))
    }

    /// Keeps the selection over a new version of the page while the text at its ends is the
    /// same, else lets it go.
    pub(super) fn keep_selection(&mut self, before: Option<&Arc<RenderedPreview>>) {
        let Some((one, other)) = self.selection.ends.clone() else {
            return;
        };
        let kept = before
            .zip(self.shown.clone())
            .filter(|(before, after)| before.document == after.document)
            .is_some_and(|(before, after)| {
                let (before, after) = (Flow::new(&before.view), Flow::new(&after.view));
                [one, other].iter().all(|(key, _)| {
                    let text = |flow: &Flow| {
                        flow.position(key)
                            .map(|leaf| flow.leaves()[leaf].text.clone())
                    };
                    text(&before).is_some() && text(&before) == text(&after)
                })
            });
        if !kept {
            self.selection.ends = None;
            self.selection.pressed = None;
        }
    }

    /// The place under `position`, or the nearest to it.
    fn place_at(&self, position: Point<Pixels>) -> Option<(LeafKey, usize)> {
        let pieces = self.layer.pieces();
        let piece = select::nearest(&pieces, position)?;
        Some((piece.key.clone(), piece.offset_at(position)))
    }

    pub(super) fn pressed(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.layer.set_dragged(false);
        self.selection.showing = None;
        let place = self.place_at(event.position);
        if event.click_count > 1 {
            if let Some((key, offset)) = place {
                self.select_word_or_text(key, offset, event.click_count, window, cx);
            }
            return;
        }
        let Some(place) = place else {
            return;
        };
        if event.modifiers.shift {
            if let Some((anchor, _)) = self.selection.ends.clone() {
                self.selection.ends = Some((anchor, place));
                self.layer.set_dragged(true);
                window.focus(&self.focus_handle, cx);
                cx.notify();
                return;
            }
        }
        self.selection.pressed = Some(place);
        if self.selection.ends.take().is_some() {
            cx.notify();
        }
    }

    fn select_word_or_text(
        &mut self,
        key: LeafKey,
        offset: usize,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(flow) = self.flow() else {
            return;
        };
        let Some(leaf) = flow.position(&key) else {
            return;
        };
        let text = &flow.leaves()[leaf].text;
        let range = if clicks == 2 {
            adoc_engine::word_at(text, offset.min(text.len()))
        } else {
            0..text.len()
        };
        self.selection.pressed = None;
        self.selection.ends = Some(((key.clone(), range.start), (key, range.end)));
        self.layer.set_dragged(true);
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    pub(super) fn dragged(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(anchor) = self.selection.pressed.clone() else {
            return;
        };
        let Some(place) = self.place_at(event.position) else {
            return;
        };
        if self.selection.ends.is_none() && place == anchor {
            return;
        }
        let ends = Some((anchor, place));
        if self.selection.ends != ends {
            if !self.layer.dragged() {
                self.layer.set_dragged(true);
                // For the keys that copy it.
                window.focus(&self.focus_handle, cx);
            }
            self.selection.ends = ends;
            cx.notify();
        }
    }

    pub(super) fn released(&mut self) {
        self.selection.pressed = None;
    }

    /// Shows the source of a block clicked, unless another click follows to select a word: what
    /// is shown may change the page the preview shows.
    pub(super) fn show_source_later(
        &mut self,
        position: adoc_engine::NavigationPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection.showing = Some(cx.spawn_in(window, async move |view, cx| {
            cx.background_executor().timer(select::DOUBLE_CLICK).await;
            let _ = view.update(cx, |view, cx| {
                view.selection.showing = None;
                // A failure is reported by the window.
                let _ = view.app.update(cx, |app, cx| {
                    app.invoke(ids::NAVIGATE_REVEAL, CommandArgs::Position(position), cx)
                });
            });
        }));
    }

    /// Selects the whole page.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = self.flow() else {
            return;
        };
        let Some((from, to)) = flow.ends() else {
            return;
        };
        let end = |place: Place| (flow.leaves()[place.leaf].key.clone(), place.offset);
        self.selection.ends = Some((end(from), end(to)));
        cx.notify();
    }

    /// Copies the selection, if there is one.
    fn copy(&mut self, command: CommandId, cx: &mut Context<Self>) {
        if self.selected_places().is_none() {
            return;
        }
        let (Some(shown), Some((from, to))) = (&self.shown, self.selection.ends.clone()) else {
            return;
        };
        let source = CopySource::Preview {
            id: shown.document,
            from,
            to,
        };
        // A failure is reported by the window.
        if let Ok(CommandOutcome::Copied(copied)) = self.app.update(cx, |app, cx| {
            app.invoke(command, CommandArgs::Copy(source), cx)
        }) {
            crate::copying::write(&copied, cx);
        }
    }

    /// Copies the selection in a form asked for, or says that there is none.
    fn copy_as(&mut self, command: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_places().is_some() {
            self.copy(command, cx);
        } else {
            crate::copying::nothing_selected(window, cx);
        }
    }

    /// Opens the menu of a right click.
    pub(super) fn open_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shown.is_none() {
            return;
        }
        // The menu takes the focus, not what was clicked.
        cx.stop_propagation();
        window.prevent_default();
        window.focus(&self.focus_handle, cx);
        let selected = self.selected_places().is_some();
        let menu = crate::copying::menu(
            Vec::new(),
            crate::copying::Can {
                cut: false,
                copy: selected,
                paste: false,
            },
            self.focus_handle.clone(),
            window,
            cx,
        );
        self.selection.menu = Some(crate::copying::Menu::open(
            position,
            menu,
            window,
            cx,
            |this: &mut Self, _| this.selection.menu = None,
        ));
        cx.notify();
    }

    /// Where `needle` starts or ends on screen, in the first text drawn that holds it.
    #[cfg(test)]
    pub fn drawn_point_for_test(&mut self, needle: &str, end: bool) -> Option<Point<Pixels>> {
        let flow = self.flow()?;
        let (leaf, at) = flow
            .leaves()
            .iter()
            .find_map(|leaf| leaf.text.find(needle).map(|at| (leaf, at)))?;
        let offset = if end { at + needle.len() } else { at };
        let (point, height) = self.layer.holding(&leaf.key, offset)?.point_of(offset)?;
        Some(gpui::point(point.x, point.y + height / 2.))
    }

    /// Each text of the page.
    #[cfg(test)]
    pub fn texts_for_test(&mut self) -> Vec<adoc_render::fragment::Leaf> {
        self.flow()
            .map_or_else(Vec::new, |flow| flow.leaves().to_vec())
    }

    /// Whether the start of text `leaf` is on screen.
    #[cfg(test)]
    pub fn leaf_drawn_for_test(&mut self, leaf: usize) -> bool {
        let key = self.texts_for_test()[leaf].key.clone();
        self.layer.holding(&key, 0).is_some()
    }

    /// Scrolls to the row holding text `leaf`.
    #[cfg(test)]
    pub fn reveal_leaf_for_test(&mut self, leaf: usize, cx: &mut Context<Self>) {
        let trail = self.texts_for_test()[leaf].key.trail.clone();
        let row = super::blocks::row_of(&self.rows, &trail).unwrap_or(0);
        self.list.scroll_to(gpui::ListOffset {
            item_ix: row,
            offset_in_item: gpui::px(0.),
        });
        cx.notify();
    }

    /// The menu open, to draw over the preview.
    pub(super) fn menu_element(&self) -> Option<gpui::AnyElement> {
        self.selection
            .menu
            .as_ref()
            .map(crate::copying::Menu::element)
    }

    /// The page's pointer and key handlers.
    pub(super) fn selecting(
        view: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        view.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.pressed(event, window, cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.open_menu(event.position, window, cx);
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
            this.dragged(event, window, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.released()),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.released()),
        )
        .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
        .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(ids::EDIT_COPY_PLAIN, cx)))
        .on_action(cx.listener(|this, _: &CopyAsPlain, window, cx| {
            this.copy_as(ids::EDIT_COPY_PLAIN, window, cx);
        }))
        .on_action(cx.listener(|this, _: &CopyAsAsciiDoc, window, cx| {
            this.copy_as(ids::EDIT_COPY_ASCIIDOC, window, cx);
        }))
        .on_action(cx.listener(|this, _: &CopyAsMarkdown, window, cx| {
            this.copy_as(ids::EDIT_COPY_MARKDOWN, window, cx);
        }))
        .on_action(cx.listener(|this, _: &CopyAsFormatted, window, cx| {
            this.copy_as(ids::EDIT_COPY_FORMATTED, window, cx);
        }))
    }
}
