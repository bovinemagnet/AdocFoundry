//! The keymap: every engine command's `default_binding` bound to the action that invokes it,
//! plus the few actions that exist only in the UI.

use adoc_engine::{commands::ids, CommandId, Engine};
use gpui::{actions, Action, App, KeyBinding, KeyBindingContextPredicate};

actions!(
    adocfoundry,
    [
        OpenFolder,
        OpenFile,
        QuickOpen,
        OpenRecent,
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
        GoToSymbol,
        GoToSymbolInPage,
        FindInWorkspace,
        ReplaceInWorkspace,
        RenamePage,
        NewPage,
        NewModule,
        NewProject,
        BuildSite,
        AnalyseImpact,
        CompareRevisions,
        InspectImpact,
        DeleteFile,
        UndoDelete,
        RenameAnchor,
        Reindex,
        OpenSettings,
        ReloadSettings,
        CheckForUpdates,
        ShowDiagnostics,
        ShowAbout,
        Quit,
        HideApp,
        HideOtherApps,
        ShowAllApps,
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
        RunAudit,
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
        ShowGraph,
        ShowLocalGraph,
        ResetGraphLayout,
        ExportGraph,
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

/// Whether command `id`'s default binding is left out on this platform: Windows quits with
/// Alt-F4, which closes the window, and gives File › Exit no key of its own.
fn unbound_here(id: CommandId) -> bool {
    cfg!(target_os = "windows") && id == ids::APP_QUIT
}

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

/// A binding of `keys` to `action` in `context`. Unlike `KeyBinding::new`, which takes only
/// static keys and refuses `secondary-`, the keys may be built at run time and use
/// `secondary-` for the platform's primary modifier (Command on macOS, Control elsewhere).
pub fn key_binding<A: Action>(
    keys: &str,
    action: A,
    context: Option<&str>,
    cx: &App,
) -> KeyBinding {
    let predicate = context.map(|context| {
        KeyBindingContextPredicate::parse(context)
            .expect("a key context is a valid predicate")
            .into()
    });
    KeyBinding::load(keys, Box::new(action), predicate, false, None, cx)
        .unwrap_or_else(|error| panic!("binding {keys:?}: {error:?}"))
}

fn binding_for(id: CommandId, keys: &str, cx: &App) -> Option<KeyBinding> {
    let workspace = Some(WORKSPACE_CONTEXT);
    let editor = Some(EDITOR_CONTEXT);
    let visual = Some(crate::visual::VISUAL_CONTEXT);
    Some(match id {
        ids::WORKSPACE_OPEN_FOLDER => key_binding(keys, OpenFolder, workspace, cx),
        ids::DOCUMENT_OPEN => key_binding(keys, OpenFile, workspace, cx),
        ids::WORKSPACE_QUICK_OPEN => key_binding(keys, QuickOpen, workspace, cx),
        ids::WORKSPACE_OPEN_RECENT => key_binding(keys, OpenRecent, workspace, cx),
        ids::DOCUMENT_SAVE => key_binding(keys, Save, workspace, cx),
        ids::DOCUMENT_SAVE_ALL => key_binding(keys, SaveAll, workspace, cx),
        ids::DOCUMENT_CLOSE => key_binding(keys, CloseDocument, workspace, cx),
        ids::DOCUMENT_CLOSE_OTHERS => key_binding(keys, CloseOthers, workspace, cx),
        ids::DOCUMENT_CLOSE_LEFT => key_binding(keys, CloseLeft, workspace, cx),
        ids::DOCUMENT_CLOSE_RIGHT => key_binding(keys, CloseRight, workspace, cx),
        ids::DOCUMENT_CLOSE_CLEAN => key_binding(keys, CloseClean, workspace, cx),
        ids::DOCUMENT_CLOSE_ALL => key_binding(keys, CloseAll, workspace, cx),
        ids::DOCUMENT_KEEP_OPEN => key_binding(keys, KeepOpen, workspace, cx),
        ids::IMAGE_RELOAD => key_binding(
            keys,
            ReloadImage,
            Some(crate::image_view::IMAGE_CONTEXT),
            cx,
        ),
        ids::SIDEBAR_EXPAND_ALL => key_binding(keys, ExpandSidebar, workspace, cx),
        ids::SIDEBAR_COLLAPSE_ALL => key_binding(keys, CollapseSidebar, workspace, cx),
        ids::SIDEBAR_COLLAPSE_ONE_LEVEL => key_binding(keys, CollapseSidebarLevel, workspace, cx),
        ids::SIDEBAR_RESET_EXPANSION => key_binding(keys, ResetSidebar, workspace, cx),
        ids::EDIT_UNDO => key_binding(keys, Undo, editor, cx),
        ids::EDIT_REDO => key_binding(keys, Redo, editor, cx),
        ids::NAVIGATE_DEFINITION => key_binding(keys, GoToDefinition, editor, cx),
        ids::NAVIGATE_FOLLOW_LINK => key_binding(keys, FollowLink, editor, cx),
        ids::NAVIGATE_BACK => key_binding(keys, GoBack, workspace, cx),
        ids::NAVIGATE_FORWARD => key_binding(keys, GoForward, workspace, cx),
        ids::NAVIGATE_PEEK_INCLUDE => key_binding(keys, PeekInclude, editor, cx),
        ids::NAVIGATE_FIND_REFERENCES => key_binding(keys, FindReferences, editor, cx),
        ids::NAVIGATE_GO_TO_SYMBOL => key_binding(keys, GoToSymbol, workspace, cx),
        ids::NAVIGATE_GO_TO_SYMBOL_IN_PAGE => key_binding(keys, GoToSymbolInPage, workspace, cx),
        ids::WORKSPACE_SEARCH => key_binding(keys, FindInWorkspace, workspace, cx),
        ids::WORKSPACE_REPLACE => key_binding(keys, ReplaceInWorkspace, workspace, cx),
        ids::PAGE_RENAME => key_binding(keys, RenamePage, workspace, cx),
        ids::PAGE_NEW => key_binding(keys, NewPage, workspace, cx),
        ids::MODULE_NEW => key_binding(keys, NewModule, workspace, cx),
        ids::PROJECT_NEW => key_binding(keys, NewProject, workspace, cx),
        ids::PROJECT_BUILD_SITE => key_binding(keys, BuildSite, workspace, cx),
        ids::IMPACT_ANALYSE_CHANGES => key_binding(keys, AnalyseImpact, workspace, cx),
        ids::IMPACT_COMPARE_REVISIONS => key_binding(keys, CompareRevisions, workspace, cx),
        ids::IMPACT_INSPECT => key_binding(keys, InspectImpact, workspace, cx),
        ids::FILE_DELETE => key_binding(keys, DeleteFile, workspace, cx),
        ids::FILE_UNDO_DELETE => key_binding(keys, UndoDelete, workspace, cx),
        ids::ANCHOR_RENAME => key_binding(keys, RenameAnchor, workspace, cx),
        ids::WORKSPACE_REINDEX => key_binding(keys, Reindex, workspace, cx),
        ids::QUALITY_RUN_AUDIT => key_binding(keys, RunAudit, workspace, cx),
        ids::GIT_REFRESH => key_binding(keys, RefreshGit, workspace, cx),
        ids::GIT_REVERT_HUNK => key_binding(keys, RevertChange, workspace, cx),
        ids::EDIT_FORMAT_TABLE => key_binding(keys, FormatTable, workspace, cx),
        ids::GIT_NEXT_CHANGE => key_binding(keys, NextChange, workspace, cx),
        ids::GIT_PREVIOUS_CHANGE => key_binding(keys, PreviousChange, workspace, cx),
        ids::GIT_SHOW_CHANGES => key_binding(keys, ShowChanges, workspace, cx),
        ids::NAVIGATE_OPEN_IN_VERSION => key_binding(keys, OpenInVersion, workspace, cx),
        ids::GIT_SHOW_BLAME => key_binding(keys, ToggleBlame, workspace, cx),
        ids::NAVIGATE_OPEN_ON_REMOTE => key_binding(keys, OpenOnRemote, workspace, cx),
        ids::NAVIGATE_COPY_REMOTE_LINK => key_binding(keys, CopyRemoteLink, workspace, cx),
        ids::GIT_STAGE => key_binding(keys, StageFile, workspace, cx),
        ids::GIT_UNSTAGE => key_binding(keys, UnstageFile, workspace, cx),
        ids::GIT_STAGE_ALL_ADOC => key_binding(keys, StageAll, workspace, cx),
        ids::GIT_COMMIT => key_binding(keys, CommitChanges, workspace, cx),
        ids::GIT_PUSH => key_binding(keys, Push, workspace, cx),
        ids::GIT_SAVE_AND_COMMIT => key_binding(keys, SaveAndCommit, workspace, cx),
        ids::EXPORT_PDF_PAGE => key_binding(keys, ExportPagePdf, workspace, cx),
        ids::EXPORT_HTML_PAGE => key_binding(keys, ExportPageHtml, workspace, cx),
        ids::DOCUMENT_LOCAL_HISTORY => key_binding(keys, LocalHistory, workspace, cx),
        ids::DOCUMENT_RECOVER_UNSAVED => key_binding(keys, RecoverUnsaved, workspace, cx),
        ids::EXPORT_PDF_COMPONENT => key_binding(keys, ExportComponentPdf, workspace, cx),
        ids::EXPORT_PDF_PROFILE => key_binding(keys, ExportPdfProfile, workspace, cx),
        ids::SETTINGS_OPEN => key_binding(keys, OpenSettings, workspace, cx),
        ids::APP_CHECK_FOR_UPDATES => key_binding(keys, CheckForUpdates, workspace, cx),
        ids::APP_DIAGNOSTICS => key_binding(keys, ShowDiagnostics, workspace, cx),
        ids::APP_ABOUT => key_binding(keys, ShowAbout, workspace, cx),
        // From any window, the graph and the detached preview too: the app handles it.
        ids::APP_QUIT => key_binding(keys, Quit, None, cx),
        ids::SETTINGS_RELOAD => key_binding(keys, ReloadSettings, workspace, cx),
        ids::VIEW_TOGGLE_PREVIEW => key_binding(keys, TogglePreview, workspace, cx),
        ids::VIEW_DETACH_PREVIEW => key_binding(keys, DetachPreview, workspace, cx),
        ids::VIEW_TOGGLE_SCROLL_SYNC => key_binding(keys, TogglePreviewScrollSync, workspace, cx),
        ids::VIEW_TOGGLE_PDF_PREVIEW => key_binding(keys, TogglePdfPreview, workspace, cx),
        ids::PREVIEW_PIN => key_binding(keys, PinPreview, workspace, cx),
        ids::VIEW_SHOW_SOURCE => key_binding(keys, ShowSource, workspace, cx),
        ids::VIEW_SHOW_VISUAL => key_binding(keys, ShowVisual, workspace, cx),
        ids::VIEW_SHOW_SPLIT => key_binding(keys, ShowSplit, workspace, cx),
        ids::VIEW_GRAPH => key_binding(keys, ShowGraph, workspace, cx),
        ids::VIEW_LOCAL_GRAPH => key_binding(keys, ShowLocalGraph, workspace, cx),
        ids::GRAPH_RESET_LAYOUT => key_binding(keys, ResetGraphLayout, workspace, cx),
        ids::GRAPH_EXPORT => key_binding(keys, ExportGraph, workspace, cx),
        ids::EDIT_TOGGLE_STRONG => key_binding(keys, ToggleStrong, visual, cx),
        ids::EDIT_TOGGLE_EMPHASIS => key_binding(keys, ToggleEmphasis, visual, cx),
        ids::EDIT_TOGGLE_CODE => key_binding(keys, ToggleCode, visual, cx),
        ids::EDIT_TOGGLE_HIGHLIGHT => key_binding(keys, ToggleHighlight, visual, cx),
        ids::EDIT_INSERT_LINK => key_binding(keys, InsertLink, visual, cx),
        ids::EDIT_REMOVE_LINK => key_binding(keys, RemoveLink, visual, cx),
        ids::VISUAL_SPLIT_BLOCK => key_binding(keys, NewParagraph, visual, cx),
        ids::EDIT_INDENT_LIST_ITEM => key_binding(keys, IndentListItem, visual, cx),
        ids::EDIT_OUTDENT_LIST_ITEM => key_binding(keys, OutdentListItem, visual, cx),
        ids::EDIT_TOGGLE_BULLET_LIST => key_binding(keys, ToggleBulletList, visual, cx),
        ids::EDIT_TOGGLE_NUMBERED_LIST => key_binding(keys, ToggleNumberedList, visual, cx),
        ids::EDIT_TOGGLE_CHECKLIST => key_binding(keys, ToggleChecklist, visual, cx),
        _ => return None,
    })
}

/// The action that invokes engine command `id` from the UI, if it has one.
pub fn action_for(id: CommandId) -> Option<Box<dyn gpui::Action>> {
    Some(match id {
        ids::WORKSPACE_OPEN_FOLDER => Box::new(OpenFolder),
        ids::DOCUMENT_OPEN => Box::new(OpenFile),
        ids::WORKSPACE_QUICK_OPEN => Box::new(QuickOpen),
        ids::WORKSPACE_OPEN_RECENT => Box::new(OpenRecent),
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
        ids::NAVIGATE_GO_TO_SYMBOL => Box::new(GoToSymbol),
        ids::NAVIGATE_GO_TO_SYMBOL_IN_PAGE => Box::new(GoToSymbolInPage),
        ids::WORKSPACE_SEARCH => Box::new(FindInWorkspace),
        ids::WORKSPACE_REPLACE => Box::new(ReplaceInWorkspace),
        ids::PAGE_RENAME => Box::new(RenamePage),
        ids::PAGE_NEW => Box::new(NewPage),
        ids::MODULE_NEW => Box::new(NewModule),
        ids::PROJECT_NEW => Box::new(NewProject),
        ids::PROJECT_BUILD_SITE => Box::new(BuildSite),
        ids::IMPACT_ANALYSE_CHANGES => Box::new(AnalyseImpact),
        ids::IMPACT_COMPARE_REVISIONS => Box::new(CompareRevisions),
        ids::IMPACT_INSPECT => Box::new(InspectImpact),
        ids::FILE_DELETE => Box::new(DeleteFile),
        ids::FILE_UNDO_DELETE => Box::new(UndoDelete),
        ids::ANCHOR_RENAME => Box::new(RenameAnchor),
        ids::WORKSPACE_REINDEX => Box::new(Reindex),
        ids::QUALITY_RUN_AUDIT => Box::new(RunAudit),
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
        ids::APP_QUIT => Box::new(Quit),
        ids::SETTINGS_RELOAD => Box::new(ReloadSettings),
        ids::VIEW_TOGGLE_PREVIEW => Box::new(TogglePreview),
        ids::VIEW_DETACH_PREVIEW => Box::new(DetachPreview),
        ids::VIEW_TOGGLE_SCROLL_SYNC => Box::new(TogglePreviewScrollSync),
        ids::VIEW_TOGGLE_PDF_PREVIEW => Box::new(TogglePdfPreview),
        ids::PREVIEW_PIN => Box::new(PinPreview),
        ids::VIEW_SHOW_SOURCE => Box::new(ShowSource),
        ids::VIEW_SHOW_VISUAL => Box::new(ShowVisual),
        ids::VIEW_SHOW_SPLIT => Box::new(ShowSplit),
        ids::VIEW_GRAPH => Box::new(ShowGraph),
        ids::VIEW_LOCAL_GRAPH => Box::new(ShowLocalGraph),
        ids::GRAPH_RESET_LAYOUT => Box::new(ResetGraphLayout),
        ids::GRAPH_EXPORT => Box::new(ExportGraph),
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
        .filter(|spec| !unbound_here(spec.id))
        .filter_map(|spec| {
            let keys = platform_binding(spec.default_binding?);
            binding_for(spec.id, &keys, cx)
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
            ids::EDIT_UNDO => bindings.push(key_binding(&keys, Undo, visual, cx)),
            ids::EDIT_REDO => bindings.push(key_binding(&keys, Redo, visual, cx)),
            ids::EDIT_TOGGLE_STRONG => bindings.push(key_binding(&keys, ToggleStrong, editor, cx)),
            ids::EDIT_TOGGLE_EMPHASIS => {
                bindings.push(key_binding(&keys, ToggleEmphasis, editor, cx));
            }
            ids::EDIT_TOGGLE_CODE => bindings.push(key_binding(&keys, ToggleCode, editor, cx)),
            ids::EDIT_INSERT_LINK => bindings.push(key_binding(&keys, InsertLink, editor, cx)),
            _ => {}
        }
    }
    // The Navigation and Modules panels' menu, from the keyboard, where a tree has focus.
    bindings.push(KeyBinding::new("shift-f10", SidebarMenu, Some("Tree")));
    let workspace = Some(WORKSPACE_CONTEXT);
    bindings.push(key_binding(
        &platform_binding("cmd-shift-p"),
        CommandPalette,
        workspace,
        cx,
    ));
    bindings.push(key_binding(
        &platform_binding("cmd-k cmd-t"),
        ToggleTheme,
        workspace,
        cx,
    ));
    bindings.push(key_binding(
        &platform_binding("cmd-k cmd-n"),
        OpenMirrorWindow,
        workspace,
        cx,
    ));
    // The app menu's keys on macOS, as every Mac app has them.
    if cfg!(target_os = "macos") {
        bindings.push(KeyBinding::new("cmd-h", HideApp, None));
        bindings.push(KeyBinding::new("cmd-alt-h", HideOtherApps, None));
    }
    cx.bind_keys(bindings);
    crate::visual::bind(cx);
    crate::files_view::bind(cx);
    crate::image_view::bind(cx);
    crate::graph_view::bind(cx);
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
            ids::PAGE_NEW_PARTIAL,
            // The Navigation, Modules and Files panels' menus, on the row they were opened on.
            ids::FILE_NEW,
            ids::NAV_ADD_PAGE,
            ids::EXPORT_PDF_PAGES,
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
