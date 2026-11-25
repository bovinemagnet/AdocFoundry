//! Copying rendered text (#794): the actions the visual editor and the preview share, the menu
//! of a right click on their text, and putting what is copied on the clipboard.

use adoc_engine::CopiedText;
use gpui::{
    actions, anchored, deferred, div, px, AnyElement, App, ClipboardItem, Context, DismissEvent,
    Entity, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement as _,
    ParentElement as _, Pixels, Point, Subscription, Window,
};
use gpui_component::{
    menu::{PopupMenu, PopupMenuItem},
    WindowExt as _,
};

actions!(
    text,
    [
        Copy,
        Cut,
        Paste,
        SelectAll,
        CopyAsPlain,
        CopyAsAsciiDoc,
        CopyAsMarkdown,
        CopyAsFormatted,
    ]
);

/// Puts `copied` on the clipboard.
pub(crate) fn write(copied: &CopiedText, cx: &mut App) {
    if copied.text.is_empty() {
        return;
    }
    if let Some(html) = &copied.html {
        if crate::rich_clipboard::write(html, &copied.text) {
            return;
        }
    }
    cx.write_to_clipboard(ClipboardItem::new_string(copied.text.clone()));
}

/// Says what Copy As copies, when it was chosen with nothing for it to copy.
pub(crate) fn nothing_selected(window: &mut Window, cx: &mut App) {
    window.push_notification(
        "Select text in the visual editor or the preview to copy it.",
        cx,
    );
}

/// What a menu's commands can act on.
pub(crate) struct Can {
    pub cut: bool,
    pub copy: bool,
    pub paste: bool,
}

/// The menu of a right click: `first`, then the commands on the selection, dispatched to
/// `focus`.
pub(crate) fn menu<V: 'static>(
    first: Vec<PopupMenuItem>,
    can: Can,
    focus: FocusHandle,
    window: &mut Window,
    cx: &mut Context<V>,
) -> Entity<PopupMenu> {
    let formats = {
        let focus = focus.clone();
        PopupMenu::build(window, cx, move |menu, _, _| {
            let menu = menu
                .action_context(focus)
                .menu("Plain Text", Box::new(CopyAsPlain))
                .menu("AsciiDoc", Box::new(CopyAsAsciiDoc))
                .menu("Markdown", Box::new(CopyAsMarkdown));
            if crate::rich_clipboard::AVAILABLE {
                menu.menu("Formatted Text", Box::new(CopyAsFormatted))
            } else {
                menu
            }
        })
    };
    PopupMenu::build(window, cx, move |mut menu, _, _| {
        menu = menu.action_context(focus);
        let chosen = !first.is_empty();
        for item in first {
            menu = menu.item(item);
        }
        if chosen {
            menu = menu.separator();
        }
        menu = menu
            .menu_with_disabled("Cut", Box::new(Cut), !can.cut)
            .menu_with_disabled("Copy", Box::new(Copy), !can.copy);
        menu = if can.copy {
            menu.item(PopupMenuItem::submenu("Copy As", formats))
        } else {
            menu.item(PopupMenuItem::new("Copy As").disabled(true))
        };
        menu.menu_with_disabled("Paste", Box::new(Paste), !can.paste)
            .separator()
            .menu("Select All", Box::new(SelectAll))
    })
}

/// A menu open over a view's text.
pub(crate) struct Menu {
    position: Point<Pixels>,
    menu: Entity<PopupMenu>,
    _closed: Subscription,
}

impl Menu {
    /// Opens `menu` at `position`; `closed` is called once it closes.
    pub fn open<V: 'static>(
        position: Point<Pixels>,
        menu: Entity<PopupMenu>,
        window: &mut Window,
        cx: &mut Context<V>,
        closed: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) -> Self {
        let subscription =
            cx.subscribe_in(&menu, window, move |this, _, _: &DismissEvent, _, cx| {
                closed(this, cx);
                cx.notify();
            });
        menu.read(cx).focus_handle(cx).focus(window, cx);
        Self {
            position,
            menu,
            _closed: subscription,
        }
    }

    pub fn element(&self) -> AnyElement {
        deferred(
            anchored()
                .position(self.position)
                .snap_to_window_with_margin(px(8.))
                .child(div().occlude().child(self.menu.clone())),
        )
        .into_any_element()
    }
}
