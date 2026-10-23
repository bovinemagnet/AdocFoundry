//! Formatting: marks on the selection (or on the text typed next, with nothing selected) and
//! links.

use adoc_engine::{commands::ids, Atom, CommandId, Mark, Piece, VisualValue};
use gpui::{px, AppContext as _, Context, ParentElement as _, Styled as _, Window};
use gpui_component::{
    input::{Input, InputState},
    v_flex, WindowExt as _,
};

use super::VisualView;

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
    }
}
