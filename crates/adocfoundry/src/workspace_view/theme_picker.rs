//! Themes (#357): Select Theme…, which previews each theme as it is highlighted, keeps the one
//! confirmed with Enter and puts back what was shown on Escape or any other close; the mode
//! commands (Follow System, Light, Dark, and Toggle Light/Dark Theme, which saves the mode it
//! switches to); and Reload Themes. Choices are saved through the engine's `appearance.save`,
//! whose `SettingsChanged` then shows them; a choice that cannot be saved is shown for the
//! session, with a warning.

use std::{cell::Cell, rc::Rc};

use adoc_engine::{AppearanceChoice, AppearanceSettings};
use gpui_component::command::CommandItem;

use super::*;
use crate::{
    appearance,
    editor::bindings::{
        FollowSystemAppearance, ReloadThemes, SelectTheme, UseDarkAppearance, UseLightAppearance,
    },
};

impl WorkspaceView {
    /// Shows the settings' appearance, telling the user once about any theme it names that
    /// cannot be shown.
    pub(super) fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let appearance = self.app.read(cx).engine.settings().appearance.clone();
        let warnings = appearance::apply(&appearance, Some(window), false, cx);
        self.warn_about_themes(warnings, window, cx);
    }

    fn warn_about_themes(
        &mut self,
        warnings: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fresh: Vec<String> = warnings
            .into_iter()
            .filter(|warning| self.theme_warnings.insert(warning.clone()))
            .collect();
        if !fresh.is_empty() {
            // After the current update: while the window is being opened it has no root yet.
            let message = fresh.join("\n");
            cx.defer_in(window, move |_, window, cx| {
                window.push_notification(message, cx)
            });
        }
    }

    /// Saves `choice`; if it cannot be saved, shows it for this session and says why.
    fn save_appearance(
        &mut self,
        choice: AppearanceSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = self.app.update(cx, |app, cx| {
            app.invoke(
                ids::APPEARANCE_SAVE,
                CommandArgs::Appearance(AppearanceChoice {
                    mode: choice.mode,
                    light_theme: choice.light_theme.clone(),
                    dark_theme: choice.dark_theme.clone(),
                }),
                cx,
            )
        });
        if let Err(error) = saved {
            appearance::apply(&choice, Some(window), true, cx);
            window.push_notification(
                format!("The theme is shown for this session but not saved: {error}"),
                cx,
            );
        }
    }

    fn save_mode(&mut self, mode: ThemePreference, window: &mut Window, cx: &mut Context<Self>) {
        let mut choice = self.app.read(cx).engine.settings().appearance.clone();
        choice.mode = mode;
        self.save_appearance(choice, window, cx);
    }

    pub(super) fn on_toggle_theme(
        &mut self,
        _: &ToggleTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = if cx.theme().mode.is_dark() {
            ThemePreference::Light
        } else {
            ThemePreference::Dark
        };
        self.save_mode(mode, window, cx);
    }

    pub(super) fn on_follow_system(
        &mut self,
        _: &FollowSystemAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_mode(ThemePreference::Auto, window, cx);
    }

    pub(super) fn on_light_appearance(
        &mut self,
        _: &UseLightAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_mode(ThemePreference::Light, window, cx);
    }

    pub(super) fn on_dark_appearance(
        &mut self,
        _: &UseDarkAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_mode(ThemePreference::Dark, window, cx);
    }

    pub(super) fn on_reload_themes(
        &mut self,
        _: &ReloadThemes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let appearance = self.app.read(cx).engine.settings().appearance.clone();
        let mut warnings = appearance::load(appearance.themes_dir.as_deref(), cx);
        warnings.extend(appearance::apply(&appearance, Some(window), false, cx));
        // Said again after a reload, which the user asked for.
        self.theme_warnings.clear();
        let count = cx
            .try_global::<appearance::ThemeCatalog>()
            .map_or(0, |catalog| catalog.sorted().len());
        self.warn_about_themes(warnings, window, cx);
        window.push_notification(format!("{count} themes available."), cx);
    }

    /// Select Theme…: every theme, light first, the chosen ones marked; highlighting one shows
    /// it, Enter keeps it, and closing any other way puts back what was shown.
    pub(super) fn on_select_theme(
        &mut self,
        _: &SelectTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = self.app.read(cx).engine.settings().appearance.clone();
        let (light, dark, _) = appearance::resolve(&saved, cx);
        let themes: Rc<Vec<_>> = Rc::new(
            cx.try_global::<appearance::ThemeCatalog>()
                .map(appearance::ThemeCatalog::sorted)
                .unwrap_or_default(),
        );
        let auto = saved.mode == ThemePreference::Auto;
        let items: Vec<CommandItem> = themes
            .iter()
            .map(|theme| {
                let dark_theme = theme.mode.is_dark();
                let chosen = if dark_theme {
                    theme.name == dark.name
                } else {
                    theme.name == light.name
                };
                let kind = if dark_theme { "Dark" } else { "Light" };
                // Marked by a word as well as a check, never by colour alone.
                let label = if chosen {
                    format!("✓ {} — {kind}, chosen", theme.name)
                } else {
                    format!("{} — {kind}", theme.name)
                };
                CommandItem::new()
                    .label(label)
                    .keywords([theme.name.clone(), kind.into()])
            })
            .collect();
        let placeholder = if auto {
            "Select a theme: following the system, it is used whenever the system is light or dark as the theme is"
        } else {
            "Select a theme"
        };
        let committed = Rc::new(Cell::new(false));
        let state = cx.new(|cx| CommandState::new(window, cx));
        let focus = state.read(cx).focus_handle(cx);
        appearance::set_previewing(true, cx);
        let workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            window.focus(&focus, cx);
            let restore = {
                let (saved, committed) = (saved.clone(), committed.clone());
                move |window: &mut Window, cx: &mut App| {
                    if committed.get() {
                        return;
                    }
                    committed.set(true);
                    appearance::set_previewing(false, cx);
                    appearance::apply(&saved, Some(window), true, cx);
                }
            };
            let restore = Rc::new(restore);
            dialog
                .close_button(false)
                .p_0()
                .on_close({
                    let restore = restore.clone();
                    move |_, window, cx| restore(window, cx)
                })
                .child(
                    Command::new(&state)
                        .bordered(false)
                        .placeholder(placeholder)
                        .max_h(px(420.))
                        .items(items.clone())
                        .on_select({
                            let themes = themes.clone();
                            move |index, _, cx| {
                                if let Some(theme) = themes.get(index.row) {
                                    appearance::preview(theme, cx);
                                }
                            }
                        })
                        .on_confirm({
                            let (themes, saved) = (themes.clone(), saved.clone());
                            let (committed, workspace) = (committed.clone(), workspace.clone());
                            move |index, window, cx| {
                                let Some(theme) = themes.get(index.row) else {
                                    return;
                                };
                                committed.set(true);
                                appearance::set_previewing(false, cx);
                                let choice = appearance::with_theme(&saved, theme);
                                window.close_dialog(cx);
                                let _ = workspace.update(cx, |workspace, cx| {
                                    workspace.save_appearance(choice, window, cx);
                                });
                            }
                        })
                        .on_cancel({
                            let restore = restore.clone();
                            move |window, cx| restore(window, cx)
                        }),
                )
        });
    }
}
