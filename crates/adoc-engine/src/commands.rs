//! The built-in command set. Ids are stable: front ends, the CLI and future integrations bind to
//! them.

use crate::{
    command::FnCommand, engine::UndoMode, CommandArgs, CommandCategory, CommandContext, CommandId,
    CommandOutcome, CommandRegistry, CommandSpec, Engine, EngineError, TextEdit,
};

/// Stable command ids.
pub mod ids {
    use crate::CommandId;

    pub const WORKSPACE_OPEN_FOLDER: CommandId = CommandId::new("workspace.open_folder");
    pub const WORKSPACE_REINDEX: CommandId = CommandId::new("workspace.reindex");
    pub const DOCUMENT_OPEN: CommandId = CommandId::new("document.open");
    pub const DOCUMENT_SAVE: CommandId = CommandId::new("document.save");
    pub const DOCUMENT_SAVE_ALL: CommandId = CommandId::new("document.save_all");
    pub const DOCUMENT_CLOSE: CommandId = CommandId::new("document.close");
    pub const EDIT_INSERT_TEXT: CommandId = CommandId::new("edit.insert_text");
    pub const EDIT_REPLACE_RANGE: CommandId = CommandId::new("edit.replace_range");
    pub const EDIT_UNDO: CommandId = CommandId::new("edit.undo");
    pub const EDIT_REDO: CommandId = CommandId::new("edit.redo");
    pub const NAVIGATE_DEFINITION: CommandId = CommandId::new("navigate.definition");
    pub const NAVIGATE_FOLLOW_LINK: CommandId = CommandId::new("navigate.follow_link");
    pub const NAVIGATE_BACK: CommandId = CommandId::new("navigate.back");
    pub const NAVIGATE_FORWARD: CommandId = CommandId::new("navigate.forward");
    pub const NAVIGATE_PEEK_INCLUDE: CommandId = CommandId::new("navigate.peek_include");
    pub const WORKSPACE_SEARCH: CommandId = CommandId::new("workspace.search");
    pub const WORKSPACE_REPLACE: CommandId = CommandId::new("workspace.replace");
    pub const WORKSPACE_APPLY_PREVIEW: CommandId = CommandId::new("workspace.apply_preview");
    pub const PAGE_RENAME: CommandId = CommandId::new("page.rename");
    pub const SETTINGS_OPEN: CommandId = CommandId::new("settings.open");
    pub const SETTINGS_RELOAD: CommandId = CommandId::new("settings.reload");
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
    use CommandCategory::{Document, Edit, View, Workspace};
    use CommandContext::{ActiveDocument, Always};

    let builtin: [(CommandSpec, Run); 12] = [
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
                ids::DOCUMENT_CLOSE,
                "Close",
                Document,
                Some("cmd-w"),
                ActiveDocument,
            ),
            document_close,
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
    ];
    for (spec, run) in builtin {
        register(registry, spec, run);
    }
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
    engine.open(&path).map(CommandOutcome::Opened)
}

fn document_save(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(invalid(ids::DOCUMENT_SAVE, "a document"));
    };
    engine.save(id).map(|()| CommandOutcome::Done)
}

fn document_save_all(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let dirty: Vec<_> = engine
        .documents()
        .filter(|document| document.dirty && !document.untitled)
        .map(|document| document.id)
        .collect();
    for id in dirty {
        engine.save(id)?;
    }
    Ok(CommandOutcome::Done)
}

fn document_close(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Document(id) = args else {
        return Err(invalid(ids::DOCUMENT_CLOSE, "a document"));
    };
    engine.close(id).map(|()| CommandOutcome::Done)
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

fn workspace_reindex(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    engine.reindex();
    Ok(CommandOutcome::Done)
}

/// Returns the user settings file, creating an empty one so it can be opened for editing.
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
