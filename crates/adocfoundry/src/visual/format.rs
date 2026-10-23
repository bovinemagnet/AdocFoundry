//! Formatting: marks on the selection (or on the text typed next, with nothing selected) and
//! links.

use adoc_engine::{commands::ids, Atom, CommandId, Mark, Piece, VisualValue};
use gpui::{px, AppContext as _, Context, ParentElement as _, Styled as _, Window};
use gpui_component::{
    input::{Input, InputState},
    v_flex, WindowExt as _,
};

use super::{Selection, VisualView};

impl VisualView {
    /// Toggles `mark` on the selection; with nothing selected, on the text typed next.
    pub(crate) fn toggle_mark(
        &mut self,
        command: CommandId,
        mark: Mark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selection.is_empty() {
            self.marks = if self.marks.contains(mark) {
                self.marks.without(mark)
            } else {
                self.marks.with(mark)
            };
            cx.notify();
            return;
        }
        self.run(command, VisualValue::None, window, cx);
    }

    /// The target and text of the link the caret is in.
    fn link_at_caret(&self) -> Option<(String, Option<String>)> {
        let (_, block) = self.block_at(self.selection.head)?;
        fn find(pieces: &[Piece], at: usize) -> Option<(String, Option<String>)> {
            pieces.iter().find_map(|piece| match piece {
                Piece::Atom {
                    atom: Atom::Link { target, text } | Atom::Xref { target, text },
                    range,
                    ..
                } if range.start <= at && at <= range.end => Some((target.clone(), text.clone())),
                Piece::Mark { children, .. } => find(children, at),
                _ => None,
            })
        }
        find(&block.pieces, self.selection.head)
    }

    /// Asks for a link's target and text, then links the selection (or changes the link the
    /// caret is in).
    pub(crate) fn edit_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (target, text) = match self.link_at_caret() {
            Some((target, text)) => (target, text.unwrap_or_default()),
            None => (String::new(), self.selected_text()),
        };
        let original_text = text.clone();
        let target_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://example.com, page.adoc#section or an anchor id")
                .default_value(target)
        });
        let text_input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let view = cx.weak_entity();
        let first = target_input.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (target_input, text_input, view) =
                (target_input.clone(), text_input.clone(), view.clone());
            let original_text = original_text.clone();
            dialog
                .title("Link")
                .width(px(560.))
                .child(
                    v_flex()
                        .gap_2()
                        .child("Target")
                        .child(Input::new(&target_input))
                        .child("Text")
                        .child(Input::new(&text_input)),
                )
                .confirm()
                .ok_text("Apply")
                .on_ok(move |_, window, cx| {
                    let target = target_input.read(cx).value().trim().to_owned();
                    let text = text_input.read(cx).value().to_string();
                    if target.is_empty() {
                        return true;
                    }
                    // Unchanged text is left as written.
                    let text = (text != original_text && !text.is_empty()).then_some(text);
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |view, cx| {
                            view.run(
                                ids::EDIT_INSERT_LINK,
                                VisualValue::Link { target, text },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
        });
        // Typing goes to the target straight away.
        first.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Asks for an image's target and alt text, then inserts it after the caret's block, or
    /// (with `existing`, the image's offset) changes that image.
    pub(crate) fn image_dialog(
        &mut self,
        existing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = existing.and_then(|at| {
            self.shown.as_ref().and_then(|shown| {
                shown
                    .model
                    .walk()
                    .into_iter()
                    .find_map(|(_, node)| match &node.kind {
                        adoc_engine::NodeKind::Image { target, alt }
                            if node.range.start <= at && at <= node.range.end =>
                        {
                            Some((target.clone(), alt.clone()))
                        }
                        _ => None,
                    })
            })
        });
        let (target, alt) = current.unwrap_or_default();
        let target_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("diagram.png, or image$diagram.png in Antora")
                .default_value(target)
        });
        let alt_input = cx.new(|cx| InputState::new(window, cx).default_value(alt));
        let view = cx.weak_entity();
        let first = target_input.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (target_input, alt_input, view) =
                (target_input.clone(), alt_input.clone(), view.clone());
            dialog
                .title(if existing.is_some() {
                    "Edit Image"
                } else {
                    "Insert Image"
                })
                .width(px(560.))
                .child(
                    v_flex()
                        .gap_2()
                        .child("Image")
                        .child(Input::new(&target_input))
                        .child("Alt text")
                        .child(Input::new(&alt_input)),
                )
                .confirm()
                .ok_text("Apply")
                .on_ok(move |_, window, cx| {
                    let target = target_input.read(cx).value().trim().to_owned();
                    let alt = alt_input.read(cx).value().to_string();
                    if target.is_empty() {
                        return true;
                    }
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |view, cx| {
                            let command = match existing {
                                Some(at) => {
                                    // At the image itself: the caret's usual snap to the next
                                    // text block would leave the image behind.
                                    view.set_selection(Selection::caret(at), cx);
                                    ids::EDIT_EDIT_IMAGE
                                }
                                None => ids::EDIT_INSERT_IMAGE,
                            };
                            view.run(command, VisualValue::Image { target, alt }, window, cx);
                        });
                    }
                    true
                })
        });
        // Typing goes to the target straight away.
        first.update(cx, |input, cx| input.focus(window, cx));
    }
}
