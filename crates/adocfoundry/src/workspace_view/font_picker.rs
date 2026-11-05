//! Select Font Set…, which draws each set as it is highlighted, keeps the one confirmed with Enter
//! and puts back what was shown on Escape or any other close, as Select Theme… does. The choice is
//! saved through the engine's `appearance.save_font_set`, whose `SettingsChanged` then shows it.

use std::{cell::Cell, rc::Rc};

use adoc_engine::{FontSet, Settings};
use gpui_component::command::CommandItem;

use super::*;
use crate::{appearance, editor::bindings::SelectFontSet, fonts};

/// The families a set uses, each once, for its row in the picker.
fn families(set: &FontSet) -> String {
    let mut names: Vec<&str> = Vec::new();
    for name in [
        Some(set.interface.as_deref().unwrap_or("system font")),
        Some(set.source.as_str()),
        set.reading.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names.join(", ")
}

impl WorkspaceView {
    pub(super) fn on_select_font_set(
        &mut self,
        _: &SelectFontSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings: Settings = self.app.read(cx).engine.settings().clone();
        let chosen = settings.appearance.chosen_font_set().id;
        let sets = Rc::new(settings.appearance.font_sets());
        let items: Vec<CommandItem> = sets
            .iter()
            .map(|set| {
                let families = families(set);
                // Marked by a word as well as a check, never by colour alone.
                let label = if set.id == chosen {
                    format!("✓ {} — {families}, chosen", set.name)
                } else {
                    format!("{} — {families}", set.name)
                };
                CommandItem::new()
                    .label(label)
                    .keywords([SharedString::from(set.name.clone()), families.into()])
            })
            .collect();
        let saved = settings.fonts();
        let settings = Rc::new(settings);
        let committed = Rc::new(Cell::new(false));
        let state = cx.new(|cx| CommandState::new(window, cx));
        let focus = state.read(cx).focus_handle(cx);
        appearance::set_previewing(true, cx);
        let workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            window.focus(&focus, cx);
            let restore = {
                let (saved, committed) = (saved.clone(), committed.clone());
                move |cx: &mut App| {
                    if committed.get() {
                        return;
                    }
                    committed.set(true);
                    appearance::set_previewing(false, cx);
                    fonts::show(saved.clone(), true, cx);
                }
            };
            let restore = Rc::new(restore);
            dialog
                .close_button(false)
                .p_0()
                .on_close({
                    let restore = restore.clone();
                    move |_, _, cx| restore(cx)
                })
                .child(
                    Command::new(&state)
                        .bordered(false)
                        .placeholder("Select a font set")
                        .max_h(px(420.))
                        .items(items.clone())
                        .on_select({
                            let (sets, settings) = (sets.clone(), settings.clone());
                            move |index, _, cx| {
                                if let Some(set) = sets.get(index.row) {
                                    fonts::show(settings.fonts_of(set), true, cx);
                                }
                            }
                        })
                        .on_confirm({
                            let (sets, settings) = (sets.clone(), settings.clone());
                            let (committed, workspace) = (committed.clone(), workspace.clone());
                            move |index, window, cx| {
                                let Some(set) = sets.get(index.row) else {
                                    return;
                                };
                                committed.set(true);
                                appearance::set_previewing(false, cx);
                                let fonts = settings.fonts_of(set);
                                let id = set.id.clone();
                                window.close_dialog(cx);
                                let _ = workspace.update(cx, |workspace, cx| {
                                    workspace.save_font_set(id, fonts, window, cx);
                                });
                            }
                        })
                        .on_cancel({
                            let restore = restore.clone();
                            move |_, cx| restore(cx)
                        }),
                )
        });
    }

    /// Saves the set `id`; if it cannot be saved, shows `shown` for this session and says why.
    fn save_font_set(
        &mut self,
        id: String,
        shown: adoc_engine::Fonts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = self.app.update(cx, |app, cx| {
            app.invoke(ids::APPEARANCE_SAVE_FONT_SET, CommandArgs::FontSet(id), cx)
        });
        if let Err(error) = saved {
            fonts::show(shown, true, cx);
            window.push_notification(
                format!("The font set is shown for this session but not saved: {error}"),
                cx,
            );
        }
    }
}
