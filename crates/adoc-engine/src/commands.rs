//! The built-in command set. Ids are stable: front ends, the CLI and future integrations bind to
//! them.

use crate::{
    command::FnCommand, engine::UndoMode, CommandArgs, CommandCategory, CommandContext, CommandId,
    CommandOutcome, CommandRegistry, CommandSpec, Engine, EngineError, TabItem, TextEdit,
};

/// Stable command ids.
pub mod ids {
    use crate::CommandId;

    pub const WORKSPACE_OPEN_FOLDER: CommandId = CommandId::new("workspace.open_folder");
    pub const WORKSPACE_REINDEX: CommandId = CommandId::new("workspace.reindex");
    pub const DOCUMENT_OPEN: CommandId = CommandId::new("document.open");
    pub const DOCUMENT_BROWSE: CommandId = CommandId::new("document.browse");
    pub const DOCUMENT_KEEP_OPEN: CommandId = CommandId::new("document.keep_open");
    pub const IMAGE_RELOAD: CommandId = CommandId::new("image.reload");
    pub const SIDEBAR_EXPAND_ALL: CommandId = CommandId::new("sidebar.expand_all");
    pub const SIDEBAR_COLLAPSE_ALL: CommandId = CommandId::new("sidebar.collapse_all");
    pub const SIDEBAR_COLLAPSE_ONE_LEVEL: CommandId = CommandId::new("sidebar.collapse_one_level");
    pub const SIDEBAR_RESET_EXPANSION: CommandId = CommandId::new("sidebar.reset_expansion");
    pub const DOCUMENT_SAVE: CommandId = CommandId::new("document.save");
    pub const DOCUMENT_SAVE_ALL: CommandId = CommandId::new("document.save_all");
    pub const DOCUMENT_OVERWRITE: CommandId = CommandId::new("document.overwrite");
    pub const DOCUMENT_RELOAD: CommandId = CommandId::new("document.reload");
    pub const DOCUMENT_CLOSE: CommandId = CommandId::new("document.close");
    pub const DOCUMENT_CLOSE_OTHERS: CommandId = CommandId::new("document.close_others");
    pub const DOCUMENT_CLOSE_LEFT: CommandId = CommandId::new("document.close_left");
    pub const DOCUMENT_CLOSE_RIGHT: CommandId = CommandId::new("document.close_right");
    pub const DOCUMENT_CLOSE_CLEAN: CommandId = CommandId::new("document.close_clean");
    pub const DOCUMENT_CLOSE_ALL: CommandId = CommandId::new("document.close_all");
    pub const EDIT_INSERT_TEXT: CommandId = CommandId::new("edit.insert_text");
    pub const EDIT_REPLACE_RANGE: CommandId = CommandId::new("edit.replace_range");
    pub const EDIT_UNDO: CommandId = CommandId::new("edit.undo");
    pub const EDIT_REDO: CommandId = CommandId::new("edit.redo");
    pub const NAVIGATE_DEFINITION: CommandId = CommandId::new("navigate.definition");
    pub const NAVIGATE_FOLLOW_LINK: CommandId = CommandId::new("navigate.follow_link");
    pub const NAVIGATE_BACK: CommandId = CommandId::new("navigate.back");
    pub const NAVIGATE_FORWARD: CommandId = CommandId::new("navigate.forward");
    pub const NAVIGATE_PEEK_INCLUDE: CommandId = CommandId::new("navigate.peek_include");
    pub const NAVIGATE_GO_TO: CommandId = CommandId::new("navigate.go_to");
    pub const NAVIGATE_REVEAL: CommandId = CommandId::new("navigate.reveal");
    pub const WORKSPACE_SEARCH: CommandId = CommandId::new("workspace.search");
    pub const WORKSPACE_REPLACE: CommandId = CommandId::new("workspace.replace");
    pub const WORKSPACE_APPLY_PREVIEW: CommandId = CommandId::new("workspace.apply_preview");
    pub const PAGE_RENAME: CommandId = CommandId::new("page.rename");
    pub const SETTINGS_OPEN: CommandId = CommandId::new("settings.open");
    pub const APP_CHECK_FOR_UPDATES: CommandId = CommandId::new("app.check_for_updates");
    pub const APP_DIAGNOSTICS: CommandId = CommandId::new("app.diagnostics");
    pub const SETTINGS_RELOAD: CommandId = CommandId::new("settings.reload");
    pub const VIEW_TOGGLE_PREVIEW: CommandId = CommandId::new("view.toggle_preview");
    pub const VIEW_DETACH_PREVIEW: CommandId = CommandId::new("view.detach_preview");
    pub const VIEW_TOGGLE_SCROLL_SYNC: CommandId = CommandId::new("view.toggle_scroll_sync");
    pub const PREVIEW_PIN: CommandId = CommandId::new("preview.pin");
    pub const VIEW_SHOW_SOURCE: CommandId = CommandId::new("view.show_source");
    pub const VIEW_SHOW_VISUAL: CommandId = CommandId::new("view.show_visual");
    pub const VIEW_SHOW_SPLIT: CommandId = CommandId::new("view.show_split");
    pub const VISUAL_INSERT_TEXT: CommandId = CommandId::new("visual.insert_text");
    pub const VISUAL_DELETE_BACKWARD: CommandId = CommandId::new("visual.delete_backward");
    pub const VISUAL_DELETE_FORWARD: CommandId = CommandId::new("visual.delete_forward");
    pub const EDIT_TOGGLE_STRONG: CommandId = CommandId::new("edit.toggle_strong");
    pub const EDIT_TOGGLE_EMPHASIS: CommandId = CommandId::new("edit.toggle_emphasis");
    pub const EDIT_TOGGLE_CODE: CommandId = CommandId::new("edit.toggle_code");
    pub const EDIT_TOGGLE_HIGHLIGHT: CommandId = CommandId::new("edit.toggle_highlight");
    pub const EDIT_INSERT_LINK: CommandId = CommandId::new("edit.insert_link");
    pub const EDIT_REMOVE_LINK: CommandId = CommandId::new("edit.remove_link");
    pub const VISUAL_SPLIT_BLOCK: CommandId = CommandId::new("visual.split_block");
    pub const VISUAL_INSERT_BLOCK: CommandId = CommandId::new("visual.insert_block");
    pub const EDIT_SET_BLOCK_TYPE: CommandId = CommandId::new("edit.set_block_type");
    pub const EDIT_TOGGLE_BULLET_LIST: CommandId = CommandId::new("edit.toggle_bullet_list");
    pub const EDIT_TOGGLE_NUMBERED_LIST: CommandId = CommandId::new("edit.toggle_numbered_list");
    pub const EDIT_TOGGLE_CHECKLIST: CommandId = CommandId::new("edit.toggle_checklist");
    pub const EDIT_TOGGLE_CHECKED: CommandId = CommandId::new("edit.toggle_checked");
    pub const EDIT_INDENT_LIST_ITEM: CommandId = CommandId::new("edit.indent_list_item");
    pub const EDIT_OUTDENT_LIST_ITEM: CommandId = CommandId::new("edit.outdent_list_item");
    pub const EDIT_INSERT_CODE_BLOCK: CommandId = CommandId::new("edit.insert_code_block");
    pub const EDIT_INSERT_ADMONITION: CommandId = CommandId::new("edit.insert_admonition");
    pub const EDIT_INSERT_TABLE: CommandId = CommandId::new("edit.insert_table");
    pub const EDIT_INSERT_TABLE_ROW: CommandId = CommandId::new("edit.insert_table_row");
    pub const EDIT_DELETE_TABLE_ROW: CommandId = CommandId::new("edit.delete_table_row");
    pub const EDIT_INSERT_TABLE_COLUMN: CommandId = CommandId::new("edit.insert_table_column");
    pub const EDIT_DELETE_TABLE_COLUMN: CommandId = CommandId::new("edit.delete_table_column");
    pub const EDIT_INSERT_IMAGE: CommandId = CommandId::new("edit.insert_image");
    pub const EDIT_EDIT_IMAGE: CommandId = CommandId::new("edit.edit_image");
    pub const GIT_REFRESH: CommandId = CommandId::new("git.refresh");
    pub const GIT_REVERT_HUNK: CommandId = CommandId::new("git.revert_hunk");
    pub const GIT_NEXT_CHANGE: CommandId = CommandId::new("git.next_change");
    pub const GIT_PREVIOUS_CHANGE: CommandId = CommandId::new("git.previous_change");
    pub const GIT_SHOW_CHANGES: CommandId = CommandId::new("git.show_changes");
    pub const NAVIGATE_OPEN_IN_VERSION: CommandId = CommandId::new("navigate.open_in_version");
    pub const GIT_SHOW_BLAME: CommandId = CommandId::new("git.show_blame");
    pub const NAVIGATE_OPEN_ON_REMOTE: CommandId = CommandId::new("navigate.open_on_remote");
    pub const NAVIGATE_COPY_REMOTE_LINK: CommandId = CommandId::new("navigate.copy_remote_link");
    pub const GIT_STAGE: CommandId = CommandId::new("git.stage");
    pub const GIT_UNSTAGE: CommandId = CommandId::new("git.unstage");
    pub const GIT_STAGE_ALL_ADOC: CommandId = CommandId::new("git.stage_all_adoc");
    pub const GIT_COMMIT: CommandId = CommandId::new("git.commit");
    pub const GIT_PUSH: CommandId = CommandId::new("git.push");
    pub const GIT_SAVE_AND_COMMIT: CommandId = CommandId::new("git.save_and_commit");
    pub const EXPORT_PDF_PAGE: CommandId = CommandId::new("export.pdf_page");
    pub const EXPORT_PDF_COMPONENT: CommandId = CommandId::new("export.pdf_component");
}

type Run = fn(&mut Engine, CommandArgs) -> Result<CommandOutcome, EngineError>;

pub(crate) const fn spec(
    id: CommandId,
    title: &'static str,
    category: CommandCategory,
    default_binding: Option<&'static str>,
    when: CommandContext,
) -> CommandSpec {
    CommandSpec {
        id,
        title,
        category,
        default_binding,
        when,
    }
}

pub(crate) fn register_builtin(registry: &mut CommandRegistry) {
    use CommandCategory::{Document, Edit, Help, Navigate, View, Workspace};
    use CommandContext::{ActiveDocument, ActiveTab, Always};

    let builtin: [(CommandSpec, Run); 35] = [
        (
            spec(
                ids::WORKSPACE_SEARCH,
                "Find in Workspace",
                Workspace,
                Some("cmd-shift-f"),
                Always,
            ),
            workspace_search,
        ),
        (
            spec(
                ids::WORKSPACE_REPLACE,
                "Replace in Workspace",
                Workspace,
                Some("cmd-shift-h"),
                Always,
            ),
            workspace_replace,
        ),
        (
            spec(
                ids::WORKSPACE_APPLY_PREVIEW,
                "Apply Preview",
                Workspace,
                None,
                Always,
            ),
            workspace_apply_preview,
        ),
        (
            spec(
                ids::PAGE_RENAME,
                "Rename Page",
                Workspace,
                Some("f2"),
                ActiveDocument,
            ),
            page_rename,
        ),
        (
            spec(
                ids::NAVIGATE_FOLLOW_LINK,
                "Follow Link",
                Navigate,
                Some("cmd-enter"),
                ActiveDocument,
            ),
            navigate_follow_link,
        ),
        (
            spec(
                ids::NAVIGATE_BACK,
                "Go Back",
                Navigate,
                Some("ctrl--"),
                Always,
            ),
            navigate_back,
        ),
        (
            spec(
                ids::NAVIGATE_FORWARD,
                "Go Forward",
                Navigate,
                Some("ctrl-shift--"),
                Always,
            ),
            navigate_forward,
        ),
        (
            spec(
                ids::NAVIGATE_PEEK_INCLUDE,
                "Peek Include",
                Navigate,
                Some("alt-f12"),
                ActiveDocument,
            ),
            navigate_peek_include,
        ),
        (
            spec(
                ids::NAVIGATE_DEFINITION,
                "Go to Definition",
                Navigate,
                Some("f12"),
                ActiveDocument,
            ),
            navigate_definition,
        ),
        (
            spec(
                ids::WORKSPACE_OPEN_FOLDER,
                "Open Folder",
                Workspace,
                Some("cmd-shift-o"),
                Always,
            ),
            workspace_open_folder,
        ),
        (
            spec(
                ids::WORKSPACE_REINDEX,
                "Re-index Workspace",
                Workspace,
                None,
                Always,
            ),
            workspace_reindex,
        ),
        (
            spec(
                ids::SETTINGS_OPEN,
                "Open Settings",
                View,
                Some("cmd-,"),
                Always,
            ),
            settings_open,
        ),
        (
            spec(
                ids::APP_CHECK_FOR_UPDATES,
                "Check for Updates",
                Help,
                None,
                Always,
            ),
            check_for_updates,
        ),
        (
            spec(
                ids::DOCUMENT_OPEN,
                "Open File",
                Document,
                Some("cmd-o"),
                Always,
            ),
            document_open,
        ),
        (
            spec(
                ids::DOCUMENT_SAVE,
                "Save",
                Document,
                Some("cmd-s"),
                ActiveDocument,
            ),
            document_save,
        ),
        (
            spec(
                ids::DOCUMENT_SAVE_ALL,
                "Save All",
                Document,
                Some("cmd-alt-s"),
                Always,
            ),
            document_save_all,
        ),
        (
            spec(
                ids::DOCUMENT_OVERWRITE,
                "Save, Overwriting the File on Disk",
                Document,
                None,
                ActiveDocument,
            ),
            document_overwrite,
        ),
        (
            spec(
                ids::DOCUMENT_RELOAD,
                "Reload from Disk",
                Document,
                None,
                ActiveDocument,
            ),
            document_reload,
        ),
        (
            spec(
                ids::DOCUMENT_CLOSE,
                "Close",
                Document,
                Some("cmd-w"),
                ActiveTab,
            ),
            document_close,
        ),
        // Tab order is the order documents were opened in, as the app shows them; the keys are
        // Zed's, but for Close All, whose `cmd-k w` is Show Visual here.
        (
            spec(
                ids::DOCUMENT_CLOSE_OTHERS,
                "Close Others",
                Document,
                Some("cmd-alt-t"),
                ActiveTab,
            ),
            |engine, args| document_close_many(engine, ids::DOCUMENT_CLOSE_OTHERS, args),
        ),
        (
            spec(
                ids::DOCUMENT_CLOSE_LEFT,
                "Close Left",
                Document,
                Some("cmd-k e"),
                ActiveTab,
            ),
            |engine, args| document_close_many(engine, ids::DOCUMENT_CLOSE_LEFT, args),
        ),
        (
            spec(
                ids::DOCUMENT_CLOSE_RIGHT,
                "Close Right",
                Document,
                Some("cmd-k t"),
                ActiveTab,
            ),
            |engine, args| document_close_many(engine, ids::DOCUMENT_CLOSE_RIGHT, args),
        ),
        (
            spec(
                ids::DOCUMENT_CLOSE_CLEAN,
                "Close Clean",
                Document,
                Some("cmd-k u"),
                ActiveTab,
            ),
            |engine, args| document_close_many(engine, ids::DOCUMENT_CLOSE_CLEAN, args),
        ),
        (
            spec(
                ids::DOCUMENT_CLOSE_ALL,
                "Close All",
                Document,
                Some("cmd-k cmd-w"),
                ActiveTab,
            ),
            |engine, args| document_close_many(engine, ids::DOCUMENT_CLOSE_ALL, args),
        ),
        (
            spec(
                ids::EDIT_INSERT_TEXT,
                "Insert Text",
                Edit,
                None,
                ActiveDocument,
            ),
            edit_insert_text,
        ),
        (
            spec(
                ids::EDIT_REPLACE_RANGE,
                "Replace Range",
                Edit,
                None,
                ActiveDocument,
            ),
            edit_replace_range,
        ),
        (
            spec(ids::EDIT_UNDO, "Undo", Edit, Some("cmd-z"), ActiveDocument),
            edit_undo,
        ),
        (
            spec(
                ids::EDIT_REDO,
                "Redo",
                Edit,
                Some("cmd-shift-z"),
                ActiveDocument,
            ),
            edit_redo,
        ),
        (
            spec(ids::SETTINGS_RELOAD, "Reload Settings", View, None, Always),
            settings_reload,
        ),
        (
            spec(
                ids::VIEW_TOGGLE_PREVIEW,
                "Toggle Preview",
                View,
                Some("cmd-k v"),
                Always,
            ),
            view_toggle_preview,
        ),
        (
            spec(
                ids::VIEW_DETACH_PREVIEW,
                "Detach Preview",
                View,
                None,
                Always,
            ),
            view_detach_preview,
        ),
        (
            spec(
                ids::VIEW_TOGGLE_SCROLL_SYNC,
                "Toggle Preview Scroll Sync",
                View,
                None,
                Always,
            ),
            view_toggle_scroll_sync,
        ),
        (
            spec(ids::PREVIEW_PIN, "Pin Preview", View, None, Always),
            preview_pin,
        ),
        (
            spec(
                ids::NAVIGATE_GO_TO,
                "Go to Location",
                Navigate,
                None,
                Always,
            ),
            navigate_go_to,
        ),
        (
            spec(
                ids::NAVIGATE_REVEAL,
                "Reveal in Editor",
                Navigate,
                None,
                Always,
            ),
            navigate_reveal,
        ),
    ];
    for (spec, run) in builtin {
        register(registry, spec, run);
    }
    crate::visual::register_visual(registry);
    crate::tabs::register_tabs(registry);
    crate::images::register_images(registry);
    crate::doctor::register_doctor(registry);
    crate::expansion::register_expansion(registry);
    #[cfg(feature = "git")]
    {
        crate::git::register_git(registry);
        crate::versions::register_versions(registry);
        crate::commit::register_commit(registry);
    }
    #[cfg(feature = "typst")]
    crate::export::register_export(registry);
}

pub(crate) fn register(registry: &mut CommandRegistry, spec: CommandSpec, run: Run) {
    registry
        .register(Box::new(FnCommand { spec, run }))
        .expect("built-in command ids are unique");
}

fn invalid(command: CommandId, expected: &'static str) -> EngineError {
    EngineError::InvalidArguments { command, expected }
}

fn document_open(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Path(path) = args else {
        return Err(invalid(ids::DOCUMENT_OPEN, "a path"));
    };
    engine
        .open_item(&path, crate::OpenDisposition::Persistent)
        .map(CommandOutcome::opened)
}

fn document_save(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(invalid(ids::DOCUMENT_SAVE, "a document"));
    };
    engine.save(id).map(|()| CommandOutcome::Done)
}

fn document_overwrite(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(invalid(ids::DOCUMENT_OVERWRITE, "a document"));
    };
    engine.overwrite(id).map(|()| CommandOutcome::Done)
}

fn document_reload(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(invalid(ids::DOCUMENT_RELOAD, "a document"));
    };
    engine.reload(id).map(|()| CommandOutcome::Done)
}

fn document_save_all(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let dirty: Vec<_> = engine
        .documents()
        .filter(|document| document.dirty && !document.untitled)
        .map(|document| document.id)
        .collect();
    // A file changed outside is left for the user to decide about; the rest are saved.
    let mut conflict = None;
    for id in dirty {
        match engine.save(id) {
            Err(error @ EngineError::SaveConflict { .. }) => {
                conflict.get_or_insert(error);
            }
            result => result?,
        }
    }
    conflict.map_or(Ok(CommandOutcome::Done), Err)
}

fn document_close(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let item = match args {
        CommandArgs::Document(id) => TabItem::Document(id),
        CommandArgs::Image(id) => TabItem::Image(id),
        _ => return Err(invalid(ids::DOCUMENT_CLOSE, "a document or image")),
    };
    engine.close_item(item).map(|()| CommandOutcome::Done)
}

/// Close Others, Left, Right, Clean and All, around the tab of `CommandArgs::Document` or
/// `CommandArgs::Image` (Clean and All need none), discarding unsaved edits as Close does. The
/// tab acted on stays active when the active one closes.
fn document_close_many(
    engine: &mut Engine,
    command: CommandId,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let id = match args {
        CommandArgs::Document(id) => {
            if engine.document(id).is_none() {
                return Err(EngineError::UnknownDocument(id));
            }
            Some(TabItem::Document(id))
        }
        CommandArgs::Image(id) => {
            if engine.image(id).is_none() {
                return Err(EngineError::NotFound(format!(
                    "image {} is not open",
                    id.get()
                )));
            }
            Some(TabItem::Image(id))
        }
        CommandArgs::None
            if matches!(command, ids::DOCUMENT_CLOSE_CLEAN | ids::DOCUMENT_CLOSE_ALL) =>
        {
            None
        }
        _ => return Err(invalid(command, "a document or image")),
    };
    let closing = match id {
        Some(id) => engine.closed_by(command, id),
        None => engine.closed_by_any(command),
    };
    let active = engine.active_item();
    for closed in closing.iter().copied() {
        engine.close_item(closed)?;
    }
    if let (Some(id), Some(active)) = (id, active) {
        if closing.contains(&active) && engine.tab_order().contains(&id) {
            engine.activate(id);
        }
    }
    Ok(CommandOutcome::Done)
}

fn edit_insert_text(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Text { id, range, text } = args else {
        return Err(invalid(ids::EDIT_INSERT_TEXT, "a document, range and text"));
    };
    let single_character = range.is_empty() && text.chars().count() == 1;
    let mode = if single_character {
        UndoMode::Typing
    } else {
        UndoMode::Group(engine.new_group())
    };
    engine.apply_edits(id, vec![TextEdit::new(range, text)], mode)?;
    Ok(CommandOutcome::Done)
}

fn edit_replace_range(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Text { id, range, text } = args else {
        return Err(invalid(
            ids::EDIT_REPLACE_RANGE,
            "a document, range and text",
        ));
    };
    let group = engine.new_group();
    engine.apply_edits(id, vec![TextEdit::new(range, text)], UndoMode::Group(group))?;
    Ok(CommandOutcome::Done)
}

fn edit_undo(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(invalid(ids::EDIT_UNDO, "a document"));
    };
    engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?;
    if let Some(group) = engine.undo_stack(id).and_then(|stack| stack.top_group()) {
        engine.check_group(group, false)?;
    }
    engine.undo(id);
    Ok(CommandOutcome::Done)
}

fn edit_redo(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(invalid(ids::EDIT_REDO, "a document"));
    };
    engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?;
    if let Some(group) = engine.undo_stack(id).and_then(|stack| stack.redo_group()) {
        engine.check_group(group, true)?;
    }
    engine.redo(id);
    Ok(CommandOutcome::Done)
}

fn settings_reload(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    engine.reload_settings().map(|()| CommandOutcome::Done)
}

fn workspace_open_folder(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Path(path) = args else {
        return Err(invalid(ids::WORKSPACE_OPEN_FOLDER, "a path"));
    };
    engine
        .open_workspace(vec![path])
        .map(|()| CommandOutcome::Done)
}

fn view_toggle_preview(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let mut layout = engine.preview_layout();
    layout.visible = !layout.visible;
    engine.set_preview_layout(layout);
    Ok(CommandOutcome::Done)
}

/// Moves the preview into its own window, or back beside the editor.
fn view_detach_preview(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let mut layout = engine.preview_layout();
    layout.detached = !layout.detached || !layout.visible;
    layout.visible = true;
    engine.set_preview_layout(layout);
    Ok(CommandOutcome::Done)
}

fn view_toggle_scroll_sync(
    engine: &mut Engine,
    _: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let mut layout = engine.preview_layout();
    layout.scroll_sync = !layout.scroll_sync;
    engine.set_preview_layout(layout);
    Ok(CommandOutcome::Done)
}

/// Pins the preview to the given document, or the active one; unpins when already pinned.
fn preview_pin(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let mut layout = engine.preview_layout();
    if layout.pinned.is_some() {
        layout.pinned = None;
    } else {
        let id = match args {
            CommandArgs::Document(id) => id,
            _ => engine
                .active()
                .ok_or_else(|| EngineError::NotFound("no document to pin".to_owned()))?,
        };
        if engine.document(id).is_none() {
            return Err(EngineError::UnknownDocument(id));
        }
        layout.pinned = Some(id);
        layout.visible = true;
    }
    engine.set_preview_layout(layout);
    Ok(CommandOutcome::Done)
}

fn workspace_reindex(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    engine.reindex();
    Ok(CommandOutcome::Done)
}

/// Returns the user settings file, creating an empty one so it can be opened for editing.
/// The app reads the feed: the engine makes no request.
fn check_for_updates(_: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    Ok(CommandOutcome::CheckForUpdates(
        crate::updates::FEED.to_owned(),
    ))
}

fn settings_open(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let path = engine
        .settings()
        .user_file()
        .map(std::path::Path::to_path_buf)
        .or_else(crate::Settings::default_user_file)
        .ok_or_else(|| EngineError::NotFound("no configuration directory".to_owned()))?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| EngineError::io(parent, error))?;
        }
        std::fs::write(&path, "").map_err(|error| EngineError::io(&path, error))?;
    }
    Ok(CommandOutcome::External(path))
}

fn navigate_definition(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Offset { id, offset } = args else {
        return Err(invalid(ids::NAVIGATE_DEFINITION, "a document and offset"));
    };
    engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?;
    match engine.definition(id, offset) {
        Some(crate::DefinitionTarget::Location(location)) => Ok(CommandOutcome::Location(location)),
        Some(crate::DefinitionTarget::External(path)) => Ok(CommandOutcome::External(path)),
        None => Err(EngineError::NotFound(format!(
            "nothing to follow at offset {offset}"
        ))),
    }
}

fn navigate_follow_link(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Offset { id, offset } = args else {
        return Err(invalid(ids::NAVIGATE_FOLLOW_LINK, "a document and offset"));
    };
    engine.follow_link(id, offset)
}

/// Opens a location, keeping where the user was in the back history: a link in the preview.
fn navigate_go_to(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Position(position) = args else {
        return Err(invalid(ids::NAVIGATE_GO_TO, "a position"));
    };
    engine.go_to(position, true)
}

/// Opens a location without recording history: the source of a block clicked in the preview.
fn navigate_reveal(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Position(position) = args else {
        return Err(invalid(ids::NAVIGATE_REVEAL, "a position"));
    };
    engine.go_to(position, false)
}

fn navigate_back(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    engine.go_back()
}

fn navigate_forward(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    engine.go_forward()
}

fn navigate_peek_include(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Offset { id, offset } = args else {
        return Err(invalid(ids::NAVIGATE_PEEK_INCLUDE, "a document and offset"));
    };
    engine.peek_include(id, offset)
}

fn workspace_search(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Search(request) = args else {
        return Err(invalid(ids::WORKSPACE_SEARCH, "a search request"));
    };
    engine.search(request).map(|_| CommandOutcome::Done)
}

#[derive(serde::Deserialize)]
struct ReplaceArgs {
    request: crate::SearchRequest,
    replacement: String,
    /// Only these matches; every match when absent.
    matches: Option<Vec<SelectedMatch>>,
}

#[derive(serde::Deserialize)]
struct SelectedMatch {
    path: std::path::PathBuf,
    start: usize,
    end: usize,
}

/// `Custom { request, replacement, matches? }` → a preview to apply with
/// `workspace.apply_preview`.
fn workspace_replace(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    const EXPECTED: &str = "`{ request, replacement, matches? }`";
    let CommandArgs::Custom(value) = args else {
        return Err(invalid(ids::WORKSPACE_REPLACE, EXPECTED));
    };
    let args: ReplaceArgs =
        serde_json::from_value(value).map_err(|_| invalid(ids::WORKSPACE_REPLACE, EXPECTED))?;
    let selected: Option<Vec<_>> = args.matches.map(|matches| {
        matches
            .into_iter()
            .map(|selected| {
                (
                    selected.path,
                    adoc_core::SourceRange::new(selected.start, selected.end),
                )
            })
            .collect()
    });
    engine
        .replace_preview(&args.request, &args.replacement, selected.as_deref())
        .map(CommandOutcome::Preview)
}

/// `Custom { preview_id }`.
fn workspace_apply_preview(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let id = match args {
        CommandArgs::Custom(value) => value.get("preview_id").and_then(serde_json::Value::as_u64),
        _ => None,
    }
    .ok_or_else(|| invalid(ids::WORKSPACE_APPLY_PREVIEW, "`{ preview_id }`"))?;
    engine
        .apply_preview(crate::PreviewId::from_raw(id))
        .map(|_| CommandOutcome::Done)
}

fn page_rename(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Rename { id, new_path } = args else {
        return Err(invalid(ids::PAGE_RENAME, "a document and new path"));
    };
    engine
        .rename_preview(id, &new_path)
        .map(CommandOutcome::Preview)
}
