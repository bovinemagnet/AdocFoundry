//! The keymap: every engine command's `default_binding` bound to the action that invokes it,
//! plus the few actions that exist only in the UI.

use adoc_engine::{commands::ids, CommandId, Engine};
use gpui::{actions, Action, App, AsKeystroke as _, KeyBinding, KeyBindingContextPredicate};

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
        PinTab,
        UnpinTab,
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
        InspectAttribute,
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
        RestoreDeletedFile,
        RenameAnchor,
        Reindex,
        OpenSettings,
        OpenSettingsFile,
        ReloadSettings,
        CheckForUpdates,
        ShowDiagnostics,
        ShowDocumentation,
        ShowAbout,
        ShareWorkspace,
        JoinSession,
        CopyInvitation,
        StopSharing,
        LeaveSession,
        StopFollowing,
        AddComment,
        SuggestChange,
        NextComment,
        PreviousComment,
        Quit,
        HideApp,
        HideOtherApps,
        ShowAllApps,
        CommandPalette,
        ToggleTheme,
        SelectTheme,
        SelectFontSet,
        SelectAuthoringProfile,
        FollowSystemAppearance,
        UseLightAppearance,
        UseDarkAppearance,
        ReloadThemes,
        OpenMirrorWindow,
        TogglePreview,
        DetachPreview,
        TogglePreviewScrollSync,
        TogglePdfPreview,
        TogglePdfWindow,
        PinPreview,
        RetryDiagrams,
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
        ToggleLineComment,
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
        ExportPageDocx,
        ExportComponentDocx,
        ExportDocxProfile,
        ExportPageKindle,
        ExportComponentKindle,
        ExportKindleProfile,
        ExportPageEpub,
        ExportComponentEpub,
        ExportEpubProfile,
        ExportPageDocBook,
        ExportComponentDocBook,
        ExportDocBookProfile,
        ExportPageDocxThemed,
        ExportComponentDocxThemed,
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
    ("Select Theme…", ""),
    ("Appearance: Follow System", ""),
    ("Appearance: Light", ""),
    ("Appearance: Dark", ""),
    ("Reload Themes", ""),
    ("Select Font Set…", ""),
    ("Zoom In", "secondary-="),
    ("Zoom Out", "secondary--"),
    ("Actual Size", "secondary-0"),
    ("Fit to Window", "secondary-9"),
    ("Fit Width", ""),
    ("Select Authoring Profile…", ""),
];

/// The [`UI_ACTIONS`] that zoom, offered only while an image is the active tab or the PDF
/// preview is shown.
pub const ZOOM_UI_ACTIONS: std::ops::Range<usize> = 11..16;

/// Whether command `id`'s default binding is left out on this platform: Windows quits with
/// Alt-F4, which closes the window, and gives File › Exit no key of its own.
pub fn unbound_here(keymap: &adoc_engine::Keymap, id: CommandId) -> bool {
    cfg!(target_os = "windows") && id == ids::APP_QUIT && !keymap.is_custom(id)
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
    boxed_binding(keys, Box::new(action), context, cx)
        .unwrap_or_else(|error| panic!("binding {keys:?}: {error:?}"))
}

fn boxed_binding(
    keys: &str,
    action: Box<dyn Action>,
    context: Option<&str>,
    cx: &App,
) -> Result<KeyBinding, gpui::InvalidKeystrokeError> {
    let predicate = context.map(|context| {
        KeyBindingContextPredicate::parse(context)
            .expect("a key context is a valid predicate")
            .into()
    });
    KeyBinding::load(keys, action, predicate, false, None, cx)
}

/// The key contexts command `id` is bound in.
fn contexts(id: CommandId) -> &'static [Option<&'static str>] {
    const WORKSPACE: &[Option<&str>] = &[Some(WORKSPACE_CONTEXT)];
    const EDITOR: &[Option<&str>] = &[Some(EDITOR_CONTEXT)];
    const VISUAL: &[Option<&str>] = &[Some(crate::visual::VISUAL_CONTEXT)];
    const EITHER: &[Option<&str>] = &[Some(EDITOR_CONTEXT), Some(crate::visual::VISUAL_CONTEXT)];
    match id {
        // From any window, the graph and the detached preview too: the app handles it.
        ids::APP_QUIT => &[None],
        ids::IMAGE_RELOAD => &[Some(crate::image_view::IMAGE_CONTEXT)],
        ids::NAVIGATE_DEFINITION
        | ids::NAVIGATE_FOLLOW_LINK
        | ids::NAVIGATE_PEEK_INCLUDE
        | ids::NAVIGATE_FIND_REFERENCES
        | ids::NAVIGATE_INSPECT_ATTRIBUTE
        | ids::EDIT_TOGGLE_LINE_COMMENT => EDITOR,
        // Undo and redo in the visual editor are the engine's, as in the source editor; the
        // marks and Insert Link work in the source editor too, on its selection.
        ids::EDIT_UNDO
        | ids::EDIT_REDO
        | ids::EDIT_TOGGLE_STRONG
        | ids::EDIT_TOGGLE_EMPHASIS
        | ids::EDIT_TOGGLE_CODE
        | ids::EDIT_INSERT_LINK => EITHER,
        ids::EDIT_TOGGLE_HIGHLIGHT
        | ids::EDIT_REMOVE_LINK
        | ids::VISUAL_SPLIT_BLOCK
        | ids::EDIT_INDENT_LIST_ITEM
        | ids::EDIT_OUTDENT_LIST_ITEM
        | ids::EDIT_TOGGLE_BULLET_LIST
        | ids::EDIT_TOGGLE_NUMBERED_LIST
        | ids::EDIT_TOGGLE_CHECKLIST => VISUAL,
        _ => WORKSPACE,
    }
}

/// The key contexts command `id` is bound in with keys the settings give it: those of the
/// window's commands include the editors', so that the keys win there over the editors' own.
fn custom_contexts(id: CommandId) -> Vec<Option<&'static str>> {
    let mut contexts = contexts(id).to_vec();
    if contexts.contains(&Some(WORKSPACE_CONTEXT)) {
        contexts.extend([Some(EDITOR_CONTEXT), Some(crate::visual::VISUAL_CONTEXT)]);
    }
    contexts
}

/// The bindings that were there before the app's, the widgets' own, which a rebinding keeps.
struct BaseBindings(Vec<KeyBinding>);

impl gpui::Global for BaseBindings {}

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
        ids::DOCUMENT_PIN_TAB => Box::new(PinTab),
        ids::DOCUMENT_UNPIN_TAB => Box::new(UnpinTab),
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
        ids::NAVIGATE_INSPECT_ATTRIBUTE => Box::new(InspectAttribute),
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
        ids::FILE_RESTORE_DELETED => Box::new(RestoreDeletedFile),
        ids::ANCHOR_RENAME => Box::new(RenameAnchor),
        ids::WORKSPACE_REINDEX => Box::new(Reindex),
        ids::QUALITY_RUN_AUDIT => Box::new(RunAudit),
        ids::GIT_REFRESH => Box::new(RefreshGit),
        ids::GIT_REVERT_HUNK => Box::new(RevertChange),
        ids::EDIT_FORMAT_TABLE => Box::new(FormatTable),
        ids::EDIT_TOGGLE_LINE_COMMENT => Box::new(ToggleLineComment),
        ids::EDIT_COPY_PLAIN => Box::new(crate::copying::CopyAsPlain),
        ids::EDIT_COPY_ASCIIDOC => Box::new(crate::copying::CopyAsAsciiDoc),
        ids::EDIT_COPY_MARKDOWN => Box::new(crate::copying::CopyAsMarkdown),
        ids::EDIT_COPY_FORMATTED if crate::rich_clipboard::AVAILABLE => {
            Box::new(crate::copying::CopyAsFormatted)
        }
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
        ids::EXPORT_DOCX_PAGE => Box::new(ExportPageDocx),
        ids::EXPORT_DOCX_COMPONENT => Box::new(ExportComponentDocx),
        ids::EXPORT_DOCX_PROFILE => Box::new(ExportDocxProfile),
        ids::EXPORT_KINDLE_PAGE => Box::new(ExportPageKindle),
        ids::EXPORT_KINDLE_COMPONENT => Box::new(ExportComponentKindle),
        ids::EXPORT_KINDLE_PROFILE => Box::new(ExportKindleProfile),
        ids::EXPORT_EPUB_PAGE => Box::new(ExportPageEpub),
        ids::EXPORT_EPUB_COMPONENT => Box::new(ExportComponentEpub),
        ids::EXPORT_EPUB_PROFILE => Box::new(ExportEpubProfile),
        ids::EXPORT_DOCBOOK_PAGE => Box::new(ExportPageDocBook),
        ids::EXPORT_DOCBOOK_COMPONENT => Box::new(ExportComponentDocBook),
        ids::EXPORT_DOCBOOK_PROFILE => Box::new(ExportDocBookProfile),
        ids::EXPORT_DOCX_PAGE_THEMED => Box::new(ExportPageDocxThemed),
        ids::EXPORT_DOCX_COMPONENT_THEMED => Box::new(ExportComponentDocxThemed),
        ids::SETTINGS_OPEN => Box::new(OpenSettings),
        ids::SETTINGS_OPEN_FILE => Box::new(OpenSettingsFile),
        ids::APP_CHECK_FOR_UPDATES => Box::new(CheckForUpdates),
        ids::APP_DIAGNOSTICS => Box::new(ShowDiagnostics),
        ids::APP_DOCUMENTATION => Box::new(ShowDocumentation),
        ids::APP_ABOUT => Box::new(ShowAbout),
        ids::COLLAB_SHARE => Box::new(ShareWorkspace),
        ids::COLLAB_JOIN => Box::new(JoinSession),
        ids::COLLAB_COPY_INVITATION => Box::new(CopyInvitation),
        ids::COLLAB_STOP => Box::new(StopSharing),
        ids::COLLAB_LEAVE => Box::new(LeaveSession),
        ids::COLLAB_STOP_FOLLOWING => Box::new(StopFollowing),
        ids::COMMENTS_ADD => Box::new(AddComment),
        ids::COMMENTS_SUGGEST => Box::new(SuggestChange),
        ids::COMMENTS_NEXT => Box::new(NextComment),
        ids::COMMENTS_PREVIOUS => Box::new(PreviousComment),
        ids::APP_QUIT => Box::new(Quit),
        ids::SETTINGS_RELOAD => Box::new(ReloadSettings),
        ids::VIEW_TOGGLE_PREVIEW => Box::new(TogglePreview),
        ids::VIEW_DETACH_PREVIEW => Box::new(DetachPreview),
        ids::VIEW_TOGGLE_SCROLL_SYNC => Box::new(TogglePreviewScrollSync),
        ids::VIEW_TOGGLE_PDF_PREVIEW => Box::new(TogglePdfPreview),
        ids::VIEW_TOGGLE_PDF_WINDOW => Box::new(TogglePdfWindow),
        ids::PREVIEW_PIN => Box::new(PinPreview),
        ids::PREVIEW_RETRY_DIAGRAMS => Box::new(RetryDiagrams),
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
        5 => Box::new(SelectTheme),
        6 => Box::new(FollowSystemAppearance),
        7 => Box::new(UseLightAppearance),
        8 => Box::new(UseDarkAppearance),
        9 => Box::new(ReloadThemes),
        10 => Box::new(SelectFontSet),
        11 => Box::new(crate::image_view::ZoomIn),
        12 => Box::new(crate::image_view::ZoomOut),
        13 => Box::new(crate::image_view::ActualSize),
        14 => Box::new(crate::image_view::FitToWindow),
        15 => Box::new(crate::image_view::FitWidth),
        _ => Box::new(SelectAuthoringProfile),
    }
}

/// Binds every command's keys, the settings' `[keymap]` over its own. Call after
/// `gpui_component::init` so these bindings take precedence over the editor widget's own, and
/// once: [`rebind`] follows a change of the settings.
pub fn bind(engine: &Engine, cx: &mut App) {
    let base = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(BaseBindings(base));
    bind_over_base(engine, cx);
}

/// Binds every command's keys anew, as the settings give them now.
pub fn rebind(engine: &Engine, cx: &mut App) {
    let Some(base) = cx.try_global::<BaseBindings>() else {
        return;
    };
    let base = base.0.clone();
    cx.clear_key_bindings();
    cx.bind_keys(base);
    bind_over_base(engine, cx);
}

fn bind_over_base(engine: &Engine, cx: &mut App) {
    let keymap = engine.keymap();
    let mut bindings = Vec::new();
    // What the settings change, which comes after everything else.
    let mut custom = Vec::new();
    for spec in engine.commands().all() {
        let Some(action) = action_for(spec.id) else {
            continue;
        };
        let keys = keymap.keys(spec.id);
        if keymap.is_custom(spec.id) {
            let keys = keys.map(platform_binding);
            for context in keys.iter().flat_map(|_| custom_contexts(spec.id)) {
                let keys = keys.as_deref().unwrap_or_default();
                custom.extend(boxed_binding(keys, action.boxed_clone(), context, cx));
            }
        } else if let Some(keys) = keys.filter(|_| !unbound_here(&keymap, spec.id)) {
            let keys = platform_binding(keys);
            bindings.extend(contexts(spec.id).iter().filter_map(|context| {
                boxed_binding(&keys, action.boxed_clone(), *context, cx).ok()
            }));
        }
        if let Some(default) = spec
            .default_binding
            .filter(|default| keys != Some(*default))
        {
            bindings.extend(unbind_widgets(spec.id, default, cx));
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
    crate::preview::bind(cx);
    crate::files_view::bind(cx);
    crate::image_view::bind(cx);
    crate::graph_view::bind(cx);
    cx.bind_keys(custom);
}

/// Bindings that keep the widgets' own actions on `default`, keys command `id` no longer has,
/// from coming back where the command stood in for them: the editor widget's Undo has a history
/// of its own.
fn unbind_widgets(id: CommandId, default: &str, cx: &App) -> Vec<KeyBinding> {
    let keys = platform_binding(default);
    let base = cx.try_global::<BaseBindings>();
    contexts(id)
        .iter()
        .filter(|context| **context == Some(EDITOR_CONTEXT))
        .filter_map(|context| {
            let probe = boxed_binding(&keys, Box::new(gpui::NoAction), *context, cx).ok()?;
            Some((context, probe))
        })
        .flat_map(|(context, probe)| {
            base.iter()
                .flat_map(|base| &base.0)
                .filter(|binding| binding.keystrokes() == probe.keystrokes())
                .filter_map(|binding| {
                    let unbind = gpui::Unbind(binding.action().name().into());
                    boxed_binding(&keys, Box::new(unbind), *context, cx).ok()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What the settings files could not be followed in: the settings' own warnings, the keymap's,
/// and the commands it gives keys that nothing can run from a key.
pub fn settings_warnings(engine: &Engine) -> Vec<String> {
    let mut warnings = engine.settings().warnings.clone();
    let keymap = engine.keymap();
    warnings.extend(keymap.problems().iter().cloned());
    warnings.extend(
        engine
            .commands()
            .all()
            .filter(|spec| keymap.is_custom(spec.id) && action_for(spec.id).is_none())
            .map(|spec| {
                format!(
                    "`keymap.{}`: this command cannot be run from a key",
                    spec.id.as_str()
                )
            }),
    );
    warnings
}

/// The strokes of `keys` (an engine binding) as they are shown.
pub fn shown_keys(keys: &str, cx: &App) -> Vec<gpui::Keystroke> {
    boxed_binding(&platform_binding(keys), Box::new(gpui::NoAction), None, cx)
        .map(|binding| {
            binding
                .keystrokes()
                .iter()
                .map(|stroke| stroke.as_keystroke().clone())
                .collect()
        })
        .unwrap_or_default()
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

    /// Every key the settings file can name is one GPUI reads, as that key.
    #[gpui::test]
    fn every_key_the_engine_reads_is_one_gpui_does(cx: &mut gpui::TestAppContext) {
        let named = [
            "enter",
            "escape",
            "tab",
            "space",
            "backspace",
            "delete",
            "insert",
            "up",
            "down",
            "left",
            "right",
            "home",
            "end",
            "pageup",
            "pagedown",
        ];
        let keys = named
            .iter()
            .map(|key| (*key).to_owned())
            .chain((1..=24).map(|number| format!("f{number}")))
            .chain(
                ('!'..='~')
                    .filter(|c| !c.is_ascii_uppercase())
                    .map(String::from),
            );
        cx.update(|cx| {
            for key in keys {
                let parsed =
                    adoc_engine::Keys::parse(&format!("cmd-ctrl-alt-shift-{key}")).unwrap();
                let shown = shown_keys(parsed.as_str(), cx);
                assert_eq!(shown.len(), 1, "{key}");
                assert_eq!(shown[0].key, key);
                assert!(shown[0].modifiers.alt && shown[0].modifiers.shift);
            }
            assert_eq!(shown_keys("cmd-k cmd-x", cx).len(), 2);
        });
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
            // Dragging a tab.
            ids::DOCUMENT_MOVE_TAB,
            ids::WORKSPACE_APPLY_PREVIEW,
            ids::VISUAL_INSERT_TEXT,
            ids::VISUAL_INSERT_BLOCK,
            ids::EDIT_PLACE_IMAGES,
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
            // Saved by Select Theme… and the appearance commands, which are the app's own.
            ids::APPEARANCE_SAVE,
            // The settings window's fields.
            ids::SETTINGS_SAVE,
            // Saved by Select Font Set…, which is the app's own.
            ids::APPEARANCE_SAVE_FONT_SET,
            // Chosen by Select Authoring Profile…, which is the app's own.
            ids::PREVIEW_AUTHORING_PROFILE,
            // The Session panel's buttons, each on the participant or file of its row.
            ids::COLLAB_ADMIT,
            ids::COLLAB_DENY,
            ids::COLLAB_GRANT_EDIT,
            ids::COLLAB_REVOKE_EDIT,
            ids::COLLAB_REMOVE,
            ids::COLLAB_OPEN_FILE,
            ids::COLLAB_FOLLOW,
            // The Comments panel's buttons, each on the comment of its card.
            ids::COMMENTS_REPLY,
            ids::COMMENTS_RESOLVE,
            ids::COMMENTS_REOPEN,
            ids::COMMENTS_EDIT,
            ids::COMMENTS_DELETE,
            ids::COMMENTS_ACCEPT,
            ids::COMMENTS_REJECT,
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
