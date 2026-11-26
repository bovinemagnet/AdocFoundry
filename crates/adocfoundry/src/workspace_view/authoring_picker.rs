//! Select Authoring Profile… (#102): the profiles `[authoring.profiles.<name>]` gives, and None,
//! the chosen one marked. Confirming one chooses it through the engine's
//! `preview.authoring_profile`, and the preview, the visual editor and exports read pages with
//! its attributes.

use gpui_component::command::CommandItem;

use super::*;
use crate::editor::bindings::SelectAuthoringProfile;

/// The row for no profile, before the profiles.
const NONE: &str = "None";

impl WorkspaceView {
    pub(super) fn on_select_authoring_profile(
        &mut self,
        _: &SelectAuthoringProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let chosen = engine
            .authoring_profile()
            .map(|profile| profile.name.clone());
        // `None` first, then each profile by name.
        let choices: Rc<Vec<Option<String>>> = Rc::new(
            std::iter::once(None)
                .chain(
                    engine
                        .authoring_profiles()
                        .iter()
                        .map(|profile| Some(profile.name.clone())),
                )
                .collect(),
        );
        let items: Vec<CommandItem> = choices
            .iter()
            .map(|choice| {
                let name = choice.as_deref().unwrap_or(NONE);
                // Marked by a word as well as a check, never by colour alone.
                let label = if *choice == chosen {
                    format!("✓ {name}, chosen")
                } else {
                    name.to_owned()
                };
                CommandItem::new()
                    .label(label)
                    .keywords([SharedString::from(name.to_owned())])
            })
            .collect();
        let state = cx.new(|cx| CommandState::new(window, cx));
        let focus = state.read(cx).focus_handle(cx);
        let workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            window.focus(&focus, cx);
            dialog.close_button(false).p_0().child(
                Command::new(&state)
                    .bordered(false)
                    .placeholder("Select an authoring profile")
                    .max_h(px(420.))
                    .items(items.clone())
                    .on_confirm({
                        let (choices, workspace) = (choices.clone(), workspace.clone());
                        move |index, window, cx| {
                            let Some(choice) = choices.get(index.row).cloned() else {
                                return;
                            };
                            window.close_dialog(cx);
                            let _ = workspace.update(cx, |workspace, cx| {
                                workspace.choose_authoring_profile(choice, window, cx);
                            });
                        }
                    }),
            )
        });
    }

    /// Chooses the authoring profile `name`, or none, saying why when it cannot be chosen.
    pub(super) fn choose_authoring_profile(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chosen = self.app.update(cx, |app, cx| {
            app.invoke(
                ids::PREVIEW_AUTHORING_PROFILE,
                CommandArgs::AuthoringProfile(name),
                cx,
            )
        });
        if let Err(error) = chosen {
            window.push_notification(format!("The profile was not chosen: {error}"), cx);
        }
    }
}
