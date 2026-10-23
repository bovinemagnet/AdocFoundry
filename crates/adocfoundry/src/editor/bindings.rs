//! The keymap: every engine command's `default_binding` bound to the action that invokes it,
//! plus the few actions that exist only in the UI.

use adoc_engine::{commands::ids, CommandId, Engine};
use gpui::{actions, App, KeyBinding};

actions!(
    adocfoundry,
    [
        OpenFolder,
        OpenFile,
        Save,
        SaveAll,
        CloseDocument,
        Undo,
        Redo,
        GoToDefinition,
        FollowLink,
        GoBack,
        GoForward,
        PeekInclude,
        FindInWorkspace,
        ReplaceInWorkspace,
        RenamePage,
        Reindex,
        OpenSettings,
        ReloadSettings,
        CommandPalette,
        ToggleTheme,
        OpenMirrorWindow,
    ]
);

/// The key context of the workspace window.
pub const WORKSPACE_CONTEXT: &str = "Workspace";
/// The key context of gpui-component's code editor. Bindings here take precedence over the
/// widget's own, so undo, redo and definition go through the engine.
pub const EDITOR_CONTEXT: &str = "Input && mode == editor";

/// UI-only actions, which have no engine command: `(title, binding)`.
pub const UI_ACTIONS: &[(&str, &str)] = &[
    ("Command Palette", "secondary-shift-p"),
    ("Toggle Light/Dark Theme", "secondary-k secondary-t"),
    ("Open Read-Only Window", "secondary-k secondary-n"),
];

/// Translates a `cmd-s` style binding to GPUI's platform-neutral form: `cmd` is the platform's
/// primary modifier (Command on macOS, Control elsewhere).
pub fn platform_binding(binding: &str) -> String {
    binding
        .split(' ')
        .map(|keystroke| {
            keystroke
                .split('-')
                .map(|part| if part == "cmd" { "secondary" } else { part })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn binding_for(id: CommandId, keys: &str) -> Option<KeyBinding> {
    let workspace = Some(WORKSPACE_CONTEXT);
    let editor = Some(EDITOR_CONTEXT);
    Some(match id {
        ids::WORKSPACE_OPEN_FOLDER => KeyBinding::new(keys, OpenFolder, workspace),
        ids::DOCUMENT_OPEN => KeyBinding::new(keys, OpenFile, workspace),
        ids::DOCUMENT_SAVE => KeyBinding::new(keys, Save, workspace),
        ids::DOCUMENT_SAVE_ALL => KeyBinding::new(keys, SaveAll, workspace),
        ids::DOCUMENT_CLOSE => KeyBinding::new(keys, CloseDocument, workspace),
        ids::EDIT_UNDO => KeyBinding::new(keys, Undo, editor),
        ids::EDIT_REDO => KeyBinding::new(keys, Redo, editor),
        ids::NAVIGATE_DEFINITION => KeyBinding::new(keys, GoToDefinition, editor),
        ids::NAVIGATE_FOLLOW_LINK => KeyBinding::new(keys, FollowLink, editor),
        ids::NAVIGATE_BACK => KeyBinding::new(keys, GoBack, workspace),
        ids::NAVIGATE_FORWARD => KeyBinding::new(keys, GoForward, workspace),
        ids::NAVIGATE_PEEK_INCLUDE => KeyBinding::new(keys, PeekInclude, editor),
        ids::WORKSPACE_SEARCH => KeyBinding::new(keys, FindInWorkspace, workspace),
        ids::WORKSPACE_REPLACE => KeyBinding::new(keys, ReplaceInWorkspace, workspace),
        ids::PAGE_RENAME => KeyBinding::new(keys, RenamePage, workspace),
        ids::WORKSPACE_REINDEX => KeyBinding::new(keys, Reindex, workspace),
        ids::SETTINGS_OPEN => KeyBinding::new(keys, OpenSettings, workspace),
        ids::SETTINGS_RELOAD => KeyBinding::new(keys, ReloadSettings, workspace),
        _ => return None,
    })
}

/// The action that invokes engine command `id` from the UI, if it has one.
pub fn action_for(id: CommandId) -> Option<Box<dyn gpui::Action>> {
    Some(match id {
        ids::WORKSPACE_OPEN_FOLDER => Box::new(OpenFolder),
        ids::DOCUMENT_OPEN => Box::new(OpenFile),
        ids::DOCUMENT_SAVE => Box::new(Save),
        ids::DOCUMENT_SAVE_ALL => Box::new(SaveAll),
        ids::DOCUMENT_CLOSE => Box::new(CloseDocument),
        ids::EDIT_UNDO => Box::new(Undo),
        ids::EDIT_REDO => Box::new(Redo),
        ids::NAVIGATE_DEFINITION => Box::new(GoToDefinition),
        ids::NAVIGATE_FOLLOW_LINK => Box::new(FollowLink),
        ids::NAVIGATE_BACK => Box::new(GoBack),
        ids::NAVIGATE_FORWARD => Box::new(GoForward),
        ids::NAVIGATE_PEEK_INCLUDE => Box::new(PeekInclude),
        ids::WORKSPACE_SEARCH => Box::new(FindInWorkspace),
        ids::WORKSPACE_REPLACE => Box::new(ReplaceInWorkspace),
        ids::PAGE_RENAME => Box::new(RenamePage),
        ids::WORKSPACE_REINDEX => Box::new(Reindex),
        ids::SETTINGS_OPEN => Box::new(OpenSettings),
        ids::SETTINGS_RELOAD => Box::new(ReloadSettings),
        _ => return None,
    })
}

/// The UI-only action at `index` of [`UI_ACTIONS`].
pub fn ui_action(index: usize) -> Box<dyn gpui::Action> {
    match index {
        0 => Box::new(CommandPalette),
        1 => Box::new(ToggleTheme),
        _ => Box::new(OpenMirrorWindow),
    }
}

/// Binds every command's default binding. Call after `gpui_component::init` so these bindings
/// take precedence over the editor widget's own.
pub fn bind(engine: &Engine, cx: &mut App) {
    let mut bindings: Vec<KeyBinding> = engine
        .commands()
        .all()
        .filter_map(|spec| {
            let keys = platform_binding(spec.default_binding?);
            binding_for(spec.id, &keys)
        })
        .collect();
    let workspace = Some(WORKSPACE_CONTEXT);
    bindings.push(KeyBinding::new(
        &platform_binding("cmd-shift-p"),
        CommandPalette,
        workspace,
    ));
    bindings.push(KeyBinding::new(
        &platform_binding("cmd-k cmd-t"),
        ToggleTheme,
        workspace,
    ));
    bindings.push(KeyBinding::new(
        &platform_binding("cmd-k cmd-n"),
        OpenMirrorWindow,
        workspace,
    ));
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::platform_binding;

    #[test]
    fn maps_cmd_to_the_platform_modifier() {
        assert_eq!(platform_binding("cmd-s"), "secondary-s");
        assert_eq!(platform_binding("cmd-shift-z"), "secondary-shift-z");
        assert_eq!(platform_binding("cmd-k cmd-t"), "secondary-k secondary-t");
        assert_eq!(platform_binding("f12"), "f12");
        assert_eq!(platform_binding("ctrl--"), "ctrl--");
    }
}
