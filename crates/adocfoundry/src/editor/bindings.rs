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
        Reload,
        Overwrite,
        LocalHistory,
        RecoverUnsaved,
        CloseDocument,
        CloseOthers,
        CloseLeft,
        CloseRight,
        CloseClean,
        CloseAll,
        KeepOpen,
        ExpandSidebar,
        CollapseSidebar,
        CollapseSidebarLevel,
        ResetSidebar,
        SidebarMenu,
        RevealInFiles,
        ReloadImage,
        Undo,
        Redo,
        GoToDefinition,
        FollowLink,
        GoBack,
        GoForward,
        PeekInclude,
        FindReferences,
        FindInWorkspace,
        ReplaceInWorkspace,
        RenamePage,
        Reindex,
        OpenSettings,
        ReloadSettings,
        CheckForUpdates,
        ShowDiagnostics,
        ShowAbout,
        CommandPalette,
        ToggleTheme,
        OpenMirrorWindow,
        TogglePreview,
        DetachPreview,
        TogglePreviewScrollSync,
        TogglePdfPreview,
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
        FormatTable,
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
        StageFile,
        UnstageFile,
        StageAll,
        CommitChanges,
        Push,
        SaveAndCommit,
        ExportPagePdf,
        ExportPageHtml,
        ExportComponentPdf,
        ExportPdfProfile,
        ExportPagePdfWithTheme,
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
    ("Reveal in Files", ""),
    ("Export Page to PDF with Theme…", ""),
    ("Zoom In", "secondary-="),
    ("Zoom Out", "secondary--"),
    ("Actual Size", "secondary-0"),
    ("Fit to Window", "secondary-9"),
];

/// The first of the [`UI_ACTIONS`] that act on the image viewer, offered only while an image is
/// the active tab.
pub const IMAGE_UI_ACTIONS: usize = 5;

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
        ids::DOCUMENT_CLOSE_OTHERS => KeyBinding::new(keys, CloseOthers, workspace),
        ids::DOCUMENT_CLOSE_LEFT => KeyBinding::new(keys, CloseLeft, workspace),
        ids::DOCUMENT_CLOSE_RIGHT => KeyBinding::new(keys, CloseRight, workspace),
        ids::DOCUMENT_CLOSE_CLEAN => KeyBinding::new(keys, CloseClean, workspace),
        ids::DOCUMENT_CLOSE_ALL => KeyBinding::new(keys, CloseAll, workspace),
        ids::DOCUMENT_KEEP_OPEN => KeyBinding::new(keys, KeepOpen, workspace),
        ids::IMAGE_RELOAD => {
            KeyBinding::new(keys, ReloadImage, Some(crate::image_view::IMAGE_CONTEXT))
        }
        ids::SIDEBAR_EXPAND_ALL => KeyBinding::new(keys, ExpandSidebar, workspace),
        ids::SIDEBAR_COLLAPSE_ALL => KeyBinding::new(keys, CollapseSidebar, workspace),
        ids::SIDEBAR_COLLAPSE_ONE_LEVEL => KeyBinding::new(keys, CollapseSidebarLevel, workspace),
        ids::SIDEBAR_RESET_EXPANSION => KeyBinding::new(keys, ResetSidebar, workspace),
        ids::EDIT_UNDO => KeyBinding::new(keys, Undo, editor),
        ids::EDIT_REDO => KeyBinding::new(keys, Redo, editor),
        ids::NAVIGATE_DEFINITION => KeyBinding::new(keys, GoToDefinition, editor),
        ids::NAVIGATE_FOLLOW_LINK => KeyBinding::new(keys, FollowLink, editor),
        ids::NAVIGATE_BACK => KeyBinding::new(keys, GoBack, workspace),
        ids::NAVIGATE_FORWARD => KeyBinding::new(keys, GoForward, workspace),
        ids::NAVIGATE_PEEK_INCLUDE => KeyBinding::new(keys, PeekInclude, editor),
        ids::NAVIGATE_FIND_REFERENCES => KeyBinding::new(keys, FindReferences, editor),
        ids::WORKSPACE_SEARCH => KeyBinding::new(keys, FindInWorkspace, workspace),
        ids::WORKSPACE_REPLACE => KeyBinding::new(keys, ReplaceInWorkspace, workspace),
        ids::PAGE_RENAME => KeyBinding::new(keys, RenamePage, workspace),
        ids::WORKSPACE_REINDEX => KeyBinding::new(keys, Reindex, workspace),
        ids::GIT_REFRESH => KeyBinding::new(keys, RefreshGit, workspace),
        ids::GIT_REVERT_HUNK => KeyBinding::new(keys, RevertChange, workspace),
        ids::EDIT_FORMAT_TABLE => KeyBinding::new(keys, FormatTable, workspace),
        ids::GIT_NEXT_CHANGE => KeyBinding::new(keys, NextChange, workspace),
        ids::GIT_PREVIOUS_CHANGE => KeyBinding::new(keys, PreviousChange, workspace),
        ids::GIT_SHOW_CHANGES => KeyBinding::new(keys, ShowChanges, workspace),
        ids::NAVIGATE_OPEN_IN_VERSION => KeyBinding::new(keys, OpenInVersion, workspace),
        ids::GIT_SHOW_BLAME => KeyBinding::new(keys, ToggleBlame, workspace),
        ids::NAVIGATE_OPEN_ON_REMOTE => KeyBinding::new(keys, OpenOnRemote, workspace),
        ids::NAVIGATE_COPY_REMOTE_LINK => KeyBinding::new(keys, CopyRemoteLink, workspace),
        ids::GIT_STAGE => KeyBinding::new(keys, StageFile, workspace),
        ids::GIT_UNSTAGE => KeyBinding::new(keys, UnstageFile, workspace),
        ids::GIT_STAGE_ALL_ADOC => KeyBinding::new(keys, StageAll, workspace),
        ids::GIT_COMMIT => KeyBinding::new(keys, CommitChanges, workspace),
        ids::GIT_PUSH => KeyBinding::new(keys, Push, workspace),
        ids::GIT_SAVE_AND_COMMIT => KeyBinding::new(keys, SaveAndCommit, workspace),
        ids::EXPORT_PDF_PAGE => KeyBinding::new(keys, ExportPagePdf, workspace),
        ids::EXPORT_HTML_PAGE => KeyBinding::new(keys, ExportPageHtml, workspace),
        ids::DOCUMENT_LOCAL_HISTORY => KeyBinding::new(keys, LocalHistory, workspace),
        ids::DOCUMENT_RECOVER_UNSAVED => KeyBinding::new(keys, RecoverUnsaved, workspace),
        ids::EXPORT_PDF_COMPONENT => KeyBinding::new(keys, ExportComponentPdf, workspace),
        ids::EXPORT_PDF_PROFILE => KeyBinding::new(keys, ExportPdfProfile, workspace),
        ids::SETTINGS_OPEN => KeyBinding::new(keys, OpenSettings, workspace),
        ids::APP_CHECK_FOR_UPDATES => KeyBinding::new(keys, CheckForUpdates, workspace),
        ids::APP_DIAGNOSTICS => KeyBinding::new(keys, ShowDiagnostics, workspace),
        ids::APP_ABOUT => KeyBinding::new(keys, ShowAbout, workspace),
        ids::SETTINGS_RELOAD => KeyBinding::new(keys, ReloadSettings, workspace),
        ids::VIEW_TOGGLE_PREVIEW => KeyBinding::new(keys, TogglePreview, workspace),
        ids::VIEW_DETACH_PREVIEW => KeyBinding::new(keys, DetachPreview, workspace),
        ids::VIEW_TOGGLE_SCROLL_SYNC => KeyBinding::new(keys, TogglePreviewScrollSync, workspace),
        ids::VIEW_TOGGLE_PDF_PREVIEW => KeyBinding::new(keys, TogglePdfPreview, workspace),
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
        ids::DOCUMENT_RELOAD => Box::new(Reload),
        ids::DOCUMENT_OVERWRITE => Box::new(Overwrite),
        ids::DOCUMENT_CLOSE => Box::new(CloseDocument),
        ids::DOCUMENT_CLOSE_OTHERS => Box::new(CloseOthers),
        ids::DOCUMENT_CLOSE_LEFT => Box::new(CloseLeft),
        ids::DOCUMENT_CLOSE_RIGHT => Box::new(CloseRight),
        ids::DOCUMENT_CLOSE_CLEAN => Box::new(CloseClean),
        ids::DOCUMENT_CLOSE_ALL => Box::new(CloseAll),
        ids::DOCUMENT_KEEP_OPEN => Box::new(KeepOpen),
        ids::IMAGE_RELOAD => Box::new(ReloadImage),
        ids::SIDEBAR_EXPAND_ALL => Box::new(ExpandSidebar),
        ids::SIDEBAR_COLLAPSE_ALL => Box::new(CollapseSidebar),
        ids::SIDEBAR_COLLAPSE_ONE_LEVEL => Box::new(CollapseSidebarLevel),
        ids::SIDEBAR_RESET_EXPANSION => Box::new(ResetSidebar),
        ids::EDIT_UNDO => Box::new(Undo),
        ids::EDIT_REDO => Box::new(Redo),
        ids::NAVIGATE_DEFINITION => Box::new(GoToDefinition),
        ids::NAVIGATE_FOLLOW_LINK => Box::new(FollowLink),
        ids::NAVIGATE_BACK => Box::new(GoBack),
        ids::NAVIGATE_FORWARD => Box::new(GoForward),
        ids::NAVIGATE_PEEK_INCLUDE => Box::new(PeekInclude),
        ids::NAVIGATE_FIND_REFERENCES => Box::new(FindReferences),
        ids::WORKSPACE_SEARCH => Box::new(FindInWorkspace),
        ids::WORKSPACE_REPLACE => Box::new(ReplaceInWorkspace),
        ids::PAGE_RENAME => Box::new(RenamePage),
        ids::WORKSPACE_REINDEX => Box::new(Reindex),
        ids::GIT_REFRESH => Box::new(RefreshGit),
        ids::GIT_REVERT_HUNK => Box::new(RevertChange),
        ids::EDIT_FORMAT_TABLE => Box::new(FormatTable),
        ids::GIT_NEXT_CHANGE => Box::new(NextChange),
        ids::GIT_PREVIOUS_CHANGE => Box::new(PreviousChange),
        ids::GIT_SHOW_CHANGES => Box::new(ShowChanges),
        ids::NAVIGATE_OPEN_IN_VERSION => Box::new(OpenInVersion),
        ids::GIT_SHOW_BLAME => Box::new(ToggleBlame),
        ids::NAVIGATE_OPEN_ON_REMOTE => Box::new(OpenOnRemote),
        ids::NAVIGATE_COPY_REMOTE_LINK => Box::new(CopyRemoteLink),
        ids::GIT_STAGE => Box::new(StageFile),
        ids::GIT_UNSTAGE => Box::new(UnstageFile),
        ids::GIT_STAGE_ALL_ADOC => Box::new(StageAll),
        ids::GIT_COMMIT => Box::new(CommitChanges),
        ids::GIT_PUSH => Box::new(Push),
        ids::GIT_SAVE_AND_COMMIT => Box::new(SaveAndCommit),
        ids::EXPORT_PDF_PAGE => Box::new(ExportPagePdf),
        ids::EXPORT_HTML_PAGE => Box::new(ExportPageHtml),
        ids::DOCUMENT_LOCAL_HISTORY => Box::new(LocalHistory),
        ids::DOCUMENT_RECOVER_UNSAVED => Box::new(RecoverUnsaved),
        ids::EXPORT_PDF_COMPONENT => Box::new(ExportComponentPdf),
        ids::EXPORT_PDF_PROFILE => Box::new(ExportPdfProfile),
        ids::SETTINGS_OPEN => Box::new(OpenSettings),
        ids::APP_CHECK_FOR_UPDATES => Box::new(CheckForUpdates),
        ids::APP_DIAGNOSTICS => Box::new(ShowDiagnostics),
        ids::APP_ABOUT => Box::new(ShowAbout),
        ids::SETTINGS_RELOAD => Box::new(ReloadSettings),
        ids::VIEW_TOGGLE_PREVIEW => Box::new(TogglePreview),
        ids::VIEW_DETACH_PREVIEW => Box::new(DetachPreview),
        ids::VIEW_TOGGLE_SCROLL_SYNC => Box::new(TogglePreviewScrollSync),
        ids::VIEW_TOGGLE_PDF_PREVIEW => Box::new(TogglePdfPreview),
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

/// The UI-only action at `index` of [`UI_ACTIONS`].
pub fn ui_action(index: usize) -> Box<dyn gpui::Action> {
    match index {
        0 => Box::new(CommandPalette),
        1 => Box::new(ToggleTheme),
        2 => Box::new(OpenMirrorWindow),
        3 => Box::new(RevealInFiles),
        4 => Box::new(ExportPagePdfWithTheme),
        5 => Box::new(crate::image_view::ZoomIn),
        6 => Box::new(crate::image_view::ZoomOut),
        7 => Box::new(crate::image_view::ActualSize),
        _ => Box::new(crate::image_view::FitToWindow),
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
        // The marks and Insert Link work in the source editor too, on its selection.
        let editor = Some(EDITOR_CONTEXT);
        match spec.id {
            ids::EDIT_UNDO => bindings.push(KeyBinding::new(&keys, Undo, visual)),
            ids::EDIT_REDO => bindings.push(KeyBinding::new(&keys, Redo, visual)),
            ids::EDIT_TOGGLE_STRONG => bindings.push(KeyBinding::new(&keys, ToggleStrong, editor)),
            ids::EDIT_TOGGLE_EMPHASIS => {
                bindings.push(KeyBinding::new(&keys, ToggleEmphasis, editor));
            }
            ids::EDIT_TOGGLE_CODE => bindings.push(KeyBinding::new(&keys, ToggleCode, editor)),
            ids::EDIT_INSERT_LINK => bindings.push(KeyBinding::new(&keys, InsertLink, editor)),
            _ => {}
        }
    }
    // The Navigation and Modules panels' menu, from the keyboard, where a tree has focus.
    bindings.push(KeyBinding::new("shift-f10", SidebarMenu, Some("Tree")));
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
    crate::files_view::bind(cx);
    crate::image_view::bind(cx);
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
            ids::DOCUMENT_BROWSE,
            ids::WORKSPACE_APPLY_PREVIEW,
            ids::VISUAL_INSERT_TEXT,
            ids::VISUAL_INSERT_BLOCK,
            ids::VISUAL_DELETE_BACKWARD,
            ids::VISUAL_DELETE_FORWARD,
            ids::EDIT_SET_BLOCK_TYPE,
            ids::DOCUMENT_RECOVER,
            ids::DOCUMENT_DISCARD_RECOVERY,
            ids::DOCUMENT_RESTORE_VERSION,
            ids::SPELLING_ADD_TO_WORKSPACE,
            ids::SPELLING_ADD_TO_USER,
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
