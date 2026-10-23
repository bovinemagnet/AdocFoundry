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
        TogglePreview,
        DetachPreview,
        TogglePreviewScrollSync,
        PinPreview,
        ShowSource,
        ShowVisual,
        ShowSplit,
        ToggleStrong,
        ToggleEmphasis,
        ToggleCode,
        ToggleHighlight,
        InsertLink,
        RemoveLink,
        NewParagraph,
        IndentListItem,
        OutdentListItem,
        ToggleBulletList,
        ToggleNumberedList,
        ToggleChecklist,
        ToggleChecked,
        InsertCodeBlock,
        InsertAdmonition,
        InsertTable,
        InsertTableRow,
        DeleteTableRow,
        InsertTableColumn,
        DeleteTableColumn,
        InsertImage,
        EditImage,
        RefreshGit,
        RevertChange,
        NextChange,
        PreviousChange,
        ShowChanges,
        OpenInVersion,
        ToggleBlame,
        OpenOnRemote,
        CopyRemoteLink,
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
    let visual = Some(crate::visual::VISUAL_CONTEXT);
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
        ids::GIT_REFRESH => KeyBinding::new(keys, RefreshGit, workspace),
        ids::GIT_REVERT_HUNK => KeyBinding::new(keys, RevertChange, workspace),
        ids::GIT_NEXT_CHANGE => KeyBinding::new(keys, NextChange, workspace),
        ids::GIT_PREVIOUS_CHANGE => KeyBinding::new(keys, PreviousChange, workspace),
        ids::GIT_SHOW_CHANGES => KeyBinding::new(keys, ShowChanges, workspace),
        ids::NAVIGATE_OPEN_IN_VERSION => KeyBinding::new(keys, OpenInVersion, workspace),
        ids::GIT_SHOW_BLAME => KeyBinding::new(keys, ToggleBlame, workspace),
        ids::NAVIGATE_OPEN_ON_REMOTE => KeyBinding::new(keys, OpenOnRemote, workspace),
        ids::NAVIGATE_COPY_REMOTE_LINK => KeyBinding::new(keys, CopyRemoteLink, workspace),
        ids::SETTINGS_OPEN => KeyBinding::new(keys, OpenSettings, workspace),
        ids::SETTINGS_RELOAD => KeyBinding::new(keys, ReloadSettings, workspace),
        ids::VIEW_TOGGLE_PREVIEW => KeyBinding::new(keys, TogglePreview, workspace),
        ids::VIEW_DETACH_PREVIEW => KeyBinding::new(keys, DetachPreview, workspace),
        ids::VIEW_TOGGLE_SCROLL_SYNC => KeyBinding::new(keys, TogglePreviewScrollSync, workspace),
        ids::PREVIEW_PIN => KeyBinding::new(keys, PinPreview, workspace),
        ids::VIEW_SHOW_SOURCE => KeyBinding::new(keys, ShowSource, workspace),
        ids::VIEW_SHOW_VISUAL => KeyBinding::new(keys, ShowVisual, workspace),
        ids::VIEW_SHOW_SPLIT => KeyBinding::new(keys, ShowSplit, workspace),
        ids::EDIT_TOGGLE_STRONG => KeyBinding::new(keys, ToggleStrong, visual),
        ids::EDIT_TOGGLE_EMPHASIS => KeyBinding::new(keys, ToggleEmphasis, visual),
        ids::EDIT_TOGGLE_CODE => KeyBinding::new(keys, ToggleCode, visual),
        ids::EDIT_TOGGLE_HIGHLIGHT => KeyBinding::new(keys, ToggleHighlight, visual),
        ids::EDIT_INSERT_LINK => KeyBinding::new(keys, InsertLink, visual),
        ids::EDIT_REMOVE_LINK => KeyBinding::new(keys, RemoveLink, visual),
        ids::VISUAL_SPLIT_BLOCK => KeyBinding::new(keys, NewParagraph, visual),
        ids::EDIT_INDENT_LIST_ITEM => KeyBinding::new(keys, IndentListItem, visual),
        ids::EDIT_OUTDENT_LIST_ITEM => KeyBinding::new(keys, OutdentListItem, visual),
        ids::EDIT_TOGGLE_BULLET_LIST => KeyBinding::new(keys, ToggleBulletList, visual),
        ids::EDIT_TOGGLE_NUMBERED_LIST => KeyBinding::new(keys, ToggleNumberedList, visual),
        ids::EDIT_TOGGLE_CHECKLIST => KeyBinding::new(keys, ToggleChecklist, visual),
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
        ids::GIT_REFRESH => Box::new(RefreshGit),
        ids::GIT_REVERT_HUNK => Box::new(RevertChange),
        ids::GIT_NEXT_CHANGE => Box::new(NextChange),
        ids::GIT_PREVIOUS_CHANGE => Box::new(PreviousChange),
        ids::GIT_SHOW_CHANGES => Box::new(ShowChanges),
        ids::NAVIGATE_OPEN_IN_VERSION => Box::new(OpenInVersion),
        ids::GIT_SHOW_BLAME => Box::new(ToggleBlame),
        ids::NAVIGATE_OPEN_ON_REMOTE => Box::new(OpenOnRemote),
        ids::NAVIGATE_COPY_REMOTE_LINK => Box::new(CopyRemoteLink),
        ids::SETTINGS_OPEN => Box::new(OpenSettings),
        ids::SETTINGS_RELOAD => Box::new(ReloadSettings),
        ids::VIEW_TOGGLE_PREVIEW => Box::new(TogglePreview),
        ids::VIEW_DETACH_PREVIEW => Box::new(DetachPreview),
        ids::VIEW_TOGGLE_SCROLL_SYNC => Box::new(TogglePreviewScrollSync),
        ids::PREVIEW_PIN => Box::new(PinPreview),
        ids::VIEW_SHOW_SOURCE => Box::new(ShowSource),
        ids::VIEW_SHOW_VISUAL => Box::new(ShowVisual),
        ids::VIEW_SHOW_SPLIT => Box::new(ShowSplit),
        ids::EDIT_TOGGLE_STRONG => Box::new(ToggleStrong),
        ids::EDIT_TOGGLE_EMPHASIS => Box::new(ToggleEmphasis),
        ids::EDIT_TOGGLE_CODE => Box::new(ToggleCode),
        ids::EDIT_TOGGLE_HIGHLIGHT => Box::new(ToggleHighlight),
        ids::EDIT_INSERT_LINK => Box::new(InsertLink),
        ids::EDIT_REMOVE_LINK => Box::new(RemoveLink),
        ids::VISUAL_SPLIT_BLOCK => Box::new(NewParagraph),
        ids::EDIT_INDENT_LIST_ITEM => Box::new(IndentListItem),
        ids::EDIT_OUTDENT_LIST_ITEM => Box::new(OutdentListItem),
        ids::EDIT_TOGGLE_BULLET_LIST => Box::new(ToggleBulletList),
        ids::EDIT_TOGGLE_NUMBERED_LIST => Box::new(ToggleNumberedList),
        ids::EDIT_TOGGLE_CHECKLIST => Box::new(ToggleChecklist),
        ids::EDIT_TOGGLE_CHECKED => Box::new(ToggleChecked),
        ids::EDIT_INSERT_CODE_BLOCK => Box::new(InsertCodeBlock),
        ids::EDIT_INSERT_ADMONITION => Box::new(InsertAdmonition),
        ids::EDIT_INSERT_TABLE => Box::new(InsertTable),
        ids::EDIT_INSERT_TABLE_ROW => Box::new(InsertTableRow),
        ids::EDIT_DELETE_TABLE_ROW => Box::new(DeleteTableRow),
        ids::EDIT_INSERT_TABLE_COLUMN => Box::new(InsertTableColumn),
        ids::EDIT_DELETE_TABLE_COLUMN => Box::new(DeleteTableColumn),
        ids::EDIT_INSERT_IMAGE => Box::new(InsertImage),
        ids::EDIT_EDIT_IMAGE => Box::new(EditImage),
        _ => return None,
    })
}

/// Whether command `id` is handled by the visual editor, and so must be dispatched from it.
pub fn in_visual_editor(id: CommandId) -> bool {
    matches!(
        id,
        ids::EDIT_TOGGLE_STRONG
            | ids::EDIT_TOGGLE_EMPHASIS
            | ids::EDIT_TOGGLE_CODE
            | ids::EDIT_TOGGLE_HIGHLIGHT
            | ids::EDIT_INSERT_LINK
            | ids::EDIT_REMOVE_LINK
            | ids::VISUAL_SPLIT_BLOCK
            | ids::EDIT_INDENT_LIST_ITEM
            | ids::EDIT_OUTDENT_LIST_ITEM
            | ids::EDIT_TOGGLE_BULLET_LIST
            | ids::EDIT_TOGGLE_NUMBERED_LIST
            | ids::EDIT_TOGGLE_CHECKLIST
            | ids::EDIT_TOGGLE_CHECKED
            | ids::EDIT_INSERT_CODE_BLOCK
            | ids::EDIT_INSERT_ADMONITION
            | ids::EDIT_INSERT_TABLE
            | ids::EDIT_INSERT_TABLE_ROW
            | ids::EDIT_DELETE_TABLE_ROW
            | ids::EDIT_INSERT_TABLE_COLUMN
            | ids::EDIT_DELETE_TABLE_COLUMN
            | ids::EDIT_INSERT_IMAGE
            | ids::EDIT_EDIT_IMAGE
    )
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
    // Undo and redo in the visual editor are the engine's, as in the source editor.
    let visual = Some(crate::visual::VISUAL_CONTEXT);
    for spec in engine.commands().all() {
        let Some(keys) = spec.default_binding.map(platform_binding) else {
            continue;
        };
        match spec.id {
            ids::EDIT_UNDO => bindings.push(KeyBinding::new(&keys, Undo, visual)),
            ids::EDIT_REDO => bindings.push(KeyBinding::new(&keys, Redo, visual)),
            _ => {}
        }
    }
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
    crate::visual::bind(cx);
}

#[cfg(test)]
mod tests {
    use adoc_engine::Settings;

    use super::*;

    #[test]
    fn maps_cmd_to_the_platform_modifier() {
        assert_eq!(platform_binding("cmd-s"), "secondary-s");
        assert_eq!(platform_binding("cmd-shift-z"), "secondary-shift-z");
        assert_eq!(platform_binding("cmd-k cmd-t"), "secondary-k secondary-t");
        assert_eq!(platform_binding("f12"), "f12");
        assert_eq!(platform_binding("ctrl--"), "ctrl--");
    }

    #[test]
    fn every_command_is_in_the_palette() {
        // Driven by typing, clicks and dialogs rather than chosen by name.
        let driven = [
            ids::EDIT_INSERT_TEXT,
            ids::EDIT_REPLACE_RANGE,
            ids::NAVIGATE_GO_TO,
            ids::NAVIGATE_REVEAL,
            ids::WORKSPACE_APPLY_PREVIEW,
            ids::VISUAL_INSERT_TEXT,
            ids::VISUAL_INSERT_BLOCK,
            ids::VISUAL_DELETE_BACKWARD,
            ids::VISUAL_DELETE_FORWARD,
            ids::EDIT_SET_BLOCK_TYPE,
        ];
        let engine = Engine::new(Settings::default());
        let missing: Vec<&str> = engine
            .commands()
            .all()
            .filter(|spec| !driven.contains(&spec.id) && action_for(spec.id).is_none())
            .map(|spec| spec.id.as_str())
            .collect();
        assert!(missing.is_empty(), "no action for {missing:?}");
    }
}
