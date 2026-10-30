//! The application menu, as in Zed: a ☰ button in the title bar that opens the first menu and
//! shows every menu's name in its place; pointing at a name opens that menu, and closing the menu
//! folds the names back into the button.
//!
//! The menus name commands, titled from the engine's command specs and dispatched as the actions
//! the keymap and the palette use, from the element that had focus when the menu opened.

use adoc_engine::{commands::ids, CommandId, Engine};
use gpui::{
    anchored, deferred, div, prelude::FluentBuilder as _, px, Action, Context, DismissEvent,
    Entity, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenu,
    IconName, Selectable as _, Sizable as _,
};

use crate::{
    app::AppState,
    editor::bindings::{action_for, ui_action, IMAGE_UI_ACTIONS, UI_ACTIONS},
};

/// An entry in a menu's definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Entry {
    Command(CommandId),
    /// One of [`UI_ACTIONS`], by its index.
    Ui(usize),
    Separator,
}

use Entry::{Command, Separator, Ui};

/// The menus, in title bar order. Commands this build does not register are left out, and those
/// unavailable just now (no page open, no Antora component) are shown disabled.
const MENUS: &[(&str, &[Entry])] = &[
    (
        "File",
        &[
            Command(ids::DOCUMENT_OPEN),
            Command(ids::WORKSPACE_OPEN_FOLDER),
            Separator,
            Command(ids::DOCUMENT_SAVE),
            Command(ids::DOCUMENT_SAVE_ALL),
            Command(ids::DOCUMENT_RELOAD),
            Command(ids::DOCUMENT_LOCAL_HISTORY),
            Command(ids::DOCUMENT_RECOVER_UNSAVED),
            Separator,
            Command(ids::EXPORT_PDF_PAGE),
            Ui(4),
            Command(ids::EXPORT_PDF_COMPONENT),
            Command(ids::EXPORT_PDF_PROFILE),
            Command(ids::EXPORT_HTML_PAGE),
            Separator,
            Command(ids::DOCUMENT_KEEP_OPEN),
            Command(ids::DOCUMENT_CLOSE),
            Command(ids::DOCUMENT_CLOSE_OTHERS),
            Command(ids::DOCUMENT_CLOSE_CLEAN),
            Command(ids::DOCUMENT_CLOSE_ALL),
            Separator,
            Command(ids::SETTINGS_OPEN),
            Command(ids::SETTINGS_RELOAD),
            Command(ids::WORKSPACE_REINDEX),
        ],
    ),
    (
        "Edit",
        &[
            Command(ids::EDIT_UNDO),
            Command(ids::EDIT_REDO),
            Separator,
            Command(ids::WORKSPACE_SEARCH),
            Command(ids::WORKSPACE_REPLACE),
            Separator,
            Command(ids::PAGE_RENAME),
            Command(ids::GIT_REVERT_HUNK),
        ],
    ),
    (
        "Format",
        &[
            Command(ids::EDIT_TOGGLE_STRONG),
            Command(ids::EDIT_TOGGLE_EMPHASIS),
            Command(ids::EDIT_TOGGLE_CODE),
            Command(ids::EDIT_TOGGLE_HIGHLIGHT),
            Separator,
            Command(ids::EDIT_INSERT_LINK),
            Command(ids::EDIT_REMOVE_LINK),
            Separator,
            Command(ids::EDIT_TOGGLE_BULLET_LIST),
            Command(ids::EDIT_TOGGLE_NUMBERED_LIST),
            Command(ids::EDIT_TOGGLE_CHECKLIST),
            Command(ids::EDIT_TOGGLE_CHECKED),
            Command(ids::EDIT_INDENT_LIST_ITEM),
            Command(ids::EDIT_OUTDENT_LIST_ITEM),
            Separator,
            Command(ids::EDIT_INSERT_CODE_BLOCK),
            Command(ids::EDIT_INSERT_ADMONITION),
            Command(ids::EDIT_INSERT_IMAGE),
            Command(ids::EDIT_EDIT_IMAGE),
            Separator,
            Command(ids::EDIT_INSERT_TABLE),
            Command(ids::EDIT_INSERT_TABLE_ROW),
            Command(ids::EDIT_DELETE_TABLE_ROW),
            Command(ids::EDIT_INSERT_TABLE_COLUMN),
            Command(ids::EDIT_DELETE_TABLE_COLUMN),
            Command(ids::EDIT_FORMAT_TABLE),
        ],
    ),
    (
        "View",
        &[
            Ui(0),
            Separator,
            Command(ids::VIEW_SHOW_SOURCE),
            Command(ids::VIEW_SHOW_VISUAL),
            Command(ids::VIEW_SHOW_SPLIT),
            Separator,
            Command(ids::VIEW_TOGGLE_PREVIEW),
            Command(ids::VIEW_TOGGLE_PDF_PREVIEW),
            Command(ids::VIEW_DETACH_PREVIEW),
            Command(ids::PREVIEW_PIN),
            Command(ids::VIEW_TOGGLE_SCROLL_SYNC),
            Separator,
            Command(ids::GIT_SHOW_CHANGES),
            Command(ids::GIT_SHOW_BLAME),
            Separator,
            Command(ids::SIDEBAR_EXPAND_ALL),
            Command(ids::SIDEBAR_COLLAPSE_ALL),
            Command(ids::SIDEBAR_COLLAPSE_ONE_LEVEL),
            Command(ids::SIDEBAR_RESET_EXPANSION),
            Ui(3),
            Separator,
            Ui(5),
            Ui(6),
            Ui(7),
            Ui(8),
            Command(ids::IMAGE_RELOAD),
            Separator,
            Ui(1),
            Ui(2),
        ],
    ),
    (
        "Go",
        &[
            Command(ids::NAVIGATE_BACK),
            Command(ids::NAVIGATE_FORWARD),
            Separator,
            Command(ids::NAVIGATE_DEFINITION),
            Command(ids::NAVIGATE_FOLLOW_LINK),
            Command(ids::NAVIGATE_PEEK_INCLUDE),
            Command(ids::NAVIGATE_FIND_REFERENCES),
            Separator,
            Command(ids::GIT_NEXT_CHANGE),
            Command(ids::GIT_PREVIOUS_CHANGE),
            Separator,
            Command(ids::NAVIGATE_OPEN_IN_VERSION),
        ],
    ),
    (
        "Git",
        &[
            Command(ids::GIT_REFRESH),
            Separator,
            Command(ids::GIT_STAGE),
            Command(ids::GIT_UNSTAGE),
            Command(ids::GIT_STAGE_ALL_ADOC),
            Separator,
            Command(ids::GIT_COMMIT),
            Command(ids::GIT_SAVE_AND_COMMIT),
            Command(ids::GIT_PUSH),
            Separator,
            Command(ids::NAVIGATE_OPEN_ON_REMOTE),
            Command(ids::NAVIGATE_COPY_REMOTE_LINK),
        ],
    ),
    (
        "Help",
        &[
            Command(ids::APP_CHECK_FOR_UPDATES),
            Command(ids::APP_DIAGNOSTICS),
            Separator,
            Command(ids::APP_ABOUT),
        ],
    ),
];

/// Commands with an action that no menu offers: overwriting is offered only when a file changed
/// on disk, a new paragraph is Enter, and closing to one side acts on a tab, from the tab's menu.
#[cfg(test)]
const NOT_IN_MENUS: &[CommandId] = &[
    ids::DOCUMENT_OVERWRITE,
    ids::VISUAL_SPLIT_BLOCK,
    ids::DOCUMENT_CLOSE_LEFT,
    ids::DOCUMENT_CLOSE_RIGHT,
];

/// An item of a menu as shown.
pub(crate) enum Item {
    Action {
        label: SharedString,
        action: Box<dyn Action>,
        disabled: bool,
    },
    Separator,
}

impl Item {
    fn boxed_clone(&self) -> Self {
        match self {
            Self::Action {
                label,
                action,
                disabled,
            } => Self::Action {
                label: label.clone(),
                action: action.boxed_clone(),
                disabled: *disabled,
            },
            Self::Separator => Self::Separator,
        }
    }
}

/// A menu as shown: its name and items.
pub(crate) struct Menu {
    pub name: &'static str,
    pub items: Vec<Item>,
}

/// The menus as this engine offers them: unregistered commands dropped, unavailable ones
/// disabled, and no separator at either end of a menu or next to another. A menu left empty is
/// dropped.
pub(crate) fn menus(engine: &Engine) -> Vec<Menu> {
    MENUS
        .iter()
        .filter_map(|(name, entries)| {
            let mut items: Vec<Item> = Vec::new();
            for entry in *entries {
                let item = match *entry {
                    Separator => {
                        if matches!(items.last(), Some(Item::Action { .. })) {
                            items.push(Item::Separator);
                        }
                        continue;
                    }
                    Ui(index) => Item::Action {
                        label: UI_ACTIONS[index].0.into(),
                        action: ui_action(index),
                        // The zoom commands act on an image.
                        disabled: index >= IMAGE_UI_ACTIONS && engine.active_image().is_none(),
                    },
                    Command(id) => {
                        let (Some(command), Some(action)) =
                            (engine.commands().get(id), action_for(id))
                        else {
                            continue;
                        };
                        Item::Action {
                            label: command.spec().title.into(),
                            action,
                            disabled: !engine.command_available(id),
                        }
                    }
                };
                items.push(item);
            }
            if matches!(items.last(), Some(Item::Separator)) {
                items.pop();
            }
            (!items.is_empty()).then_some(Menu { name, items })
        })
        .collect()
}

/// The ☰ button, and the menus it opens into.
pub struct AppMenu {
    app: Entity<AppState>,
    /// Where actions go when nothing else had focus.
    fallback: FocusHandle,
    /// The menus, built when the button opens them.
    menus: Vec<Menu>,
    /// The open menu, by index, and its popup.
    open: Option<(usize, Entity<PopupMenu>)>,
    /// What had focus when the menus opened: actions are dispatched from it.
    action_context: Option<FocusHandle>,
    _dismiss: Option<Subscription>,
}

impl AppMenu {
    pub fn new(app: Entity<AppState>, fallback: FocusHandle) -> Self {
        Self {
            app,
            fallback,
            menus: Vec::new(),
            open: None,
            action_context: None,
            _dismiss: None,
        }
    }

    /// Opens menu `index`, closing any other. The first menu to open notes where focus was.
    fn open(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_none() {
            self.menus = menus(&self.app.read(cx).engine);
            self.action_context = Some(window.focused(cx).unwrap_or(self.fallback.clone()));
        }
        let Some(menu) = self.menus.get(index) else {
            return;
        };
        let items: Vec<Item> = menu.items.iter().map(Item::boxed_clone).collect();
        let context = self.action_context.clone().unwrap_or(self.fallback.clone());
        let popup = PopupMenu::build(window, cx, move |popup, _, _| {
            items
                .into_iter()
                .fold(popup.action_context(context), |popup, item| match item {
                    Item::Action {
                        label,
                        action,
                        disabled,
                    } => popup.menu_with_disabled(label, action, disabled),
                    Item::Separator => popup.separator(),
                })
        });
        self._dismiss = Some(cx.subscribe_in(&popup, window, Self::on_dismiss));
        popup.read(cx).focus_handle(cx).focus(window, cx);
        self.open = Some((index, popup));
        cx.notify();
    }

    /// Folds the menus back into the button, handing focus back.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = None;
        self._dismiss = None;
        if let Some(context) = self.action_context.take() {
            window.focus(&context, cx);
        }
        cx.notify();
    }

    fn on_dismiss(
        &mut self,
        _: &Entity<PopupMenu>,
        _: &DismissEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
    }
}

impl Render for AppMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((open, popup)) = self.open.clone() else {
            return h_flex().child(
                div().debug_selector(|| "app-menu".to_owned()).child(
                    Button::new("app-menu")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Menu)
                        .tooltip("Menu")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                // Not a drag of the window by its title bar.
                                window.prevent_default();
                                cx.stop_propagation();
                                this.open(0, window, cx);
                            }),
                        ),
                ),
            );
        };
        h_flex()
            .gap_0p5()
            .children(self.menus.iter().enumerate().map(|(index, menu)| {
                div()
                    .id(index)
                    .relative()
                    .debug_selector(move || format!("app-menu-{}", menu.name))
                    .child(
                        Button::new("menu")
                            .ghost()
                            .xsmall()
                            .compact()
                            .label(menu.name)
                            .selected(index == open)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    if index == open {
                                        this.close(window, cx);
                                    } else {
                                        this.open(index, window, cx);
                                    }
                                }),
                            ),
                    )
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        let switching = this.open.as_ref().is_some_and(|(open, _)| *open != index);
                        if *hovered && switching {
                            this.open(index, window, cx);
                        }
                    }))
                    .when(index == open, |this| {
                        this.child(deferred(
                            anchored()
                                .anchor(gpui::Anchor::TopLeft)
                                .snap_to_window_with_margin(px(8.))
                                .child(div().occlude().top_1().child(popup.clone())),
                        ))
                    })
            }))
    }
}

#[cfg(test)]
mod tests {
    use adoc_engine::Settings;

    use super::*;

    fn labels(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| match item {
                Item::Action { label, .. } => label.to_string(),
                Item::Separator => "-".to_owned(),
            })
            .collect()
    }

    #[test]
    fn every_command_is_in_a_menu() {
        let listed: Vec<CommandId> = MENUS
            .iter()
            .flat_map(|(_, entries)| entries.iter())
            .filter_map(|entry| match entry {
                Command(id) => Some(*id),
                _ => None,
            })
            .collect();
        let engine = Engine::new(Settings::default());
        let missing: Vec<&str> = engine
            .commands()
            .all()
            .map(|spec| spec.id)
            .filter(|id| action_for(*id).is_some())
            .filter(|id| !listed.contains(id) && !NOT_IN_MENUS.contains(id))
            .map(CommandId::as_str)
            .collect();
        assert!(missing.is_empty(), "in no menu: {missing:?}");
        for (index, (title, _)) in UI_ACTIONS.iter().enumerate() {
            assert!(
                MENUS
                    .iter()
                    .any(|(_, entries)| entries.contains(&Ui(index))),
                "{title} is in no menu"
            );
        }
    }

    #[test]
    fn menus_take_their_titles_from_the_commands() {
        let menus = menus(&Engine::new(Settings::default()));
        let names: Vec<&str> = menus.iter().map(|menu| menu.name).collect();
        assert_eq!(
            names,
            ["File", "Edit", "Format", "View", "Go", "Git", "Help"]
        );
        let edit = labels(&menus[1]);
        assert_eq!(
            edit[..3],
            ["Undo".to_owned(), "Redo".to_owned(), "-".to_owned()]
        );
        assert_eq!(labels(&menus[3])[0], "Command Palette");
    }

    #[test]
    fn separators_only_divide_items() {
        for menu in menus(&Engine::new(Settings::default())) {
            let labels = labels(&menu);
            assert_ne!(
                labels.first().map(String::as_str),
                Some("-"),
                "{}",
                menu.name
            );
            assert_ne!(
                labels.last().map(String::as_str),
                Some("-"),
                "{}",
                menu.name
            );
            assert!(
                !labels
                    .windows(2)
                    .any(|pair| pair[0] == "-" && pair[1] == "-"),
                "{}: {labels:?}",
                menu.name
            );
        }
    }
}
