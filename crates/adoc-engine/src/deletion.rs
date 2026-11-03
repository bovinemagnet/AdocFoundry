//! Deleting a file (#100): `file.delete` moves a file of the workspace aside into the state
//! folder ([`crate::Settings::state_dir`]), closing its tab and telling the index, catalog,
//! graph and Files view as the watcher would; `file.undo_delete` puts back the latest deletion
//! made since the engine started, refusing to replace a file made at its path since. Undo is
//! the engine's per page, so a deletion, whose page has no tab left, is undone by its own
//! command. Deletions kept from earlier sessions are removed once older than
//! `[history] keep_days`. What removing a file would break is Inspect Impact's to show
//! (`impact.inspect`), which a front end asks before deleting.
//!
//! Each deletion is a folder in the store, `<milliseconds>-<process>-<count>`, holding the
//! file as `file` and its path as `path`. Without a state folder nothing is deleted, so only
//! the app deletes.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use adoc_index::{is_asciidoc_path, normalize_path};

use crate::{
    commands::{ids, register, spec},
    local_history::{millis, DAY_MILLIS},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, EngineEvent, OpenDisposition, TabItem,
};

/// The folder in the state folder that holds deleted files.
pub const DELETED_DIR: &str = "deleted";

const FILE: &str = "file";
const PATH_FILE: &str = "path";

/// A deletion this session can undo.
#[derive(Clone, Debug)]
pub(crate) struct Deleted {
    path: PathBuf,
    folder: PathBuf,
    /// Whether it had a tab, so putting it back opens it again.
    was_open: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Deletions {
    /// Latest last.
    kept: Vec<Deleted>,
    count: u64,
}

pub(crate) fn register_deletion(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::FILE_DELETE,
            "Delete File…",
            CommandCategory::Document,
            None,
            CommandContext::Deletion,
        ),
        |engine, args| {
            let path = match args {
                CommandArgs::Path(path) => path,
                CommandArgs::Document(id) => engine
                    .document(id)
                    .ok_or(EngineError::UnknownDocument(id))?
                    .path
                    .clone(),
                CommandArgs::Image(id) => engine
                    .image(id)
                    .ok_or_else(|| {
                        EngineError::NotFound(format!("image {} is not open", id.get()))
                    })?
                    .path
                    .clone(),
                CommandArgs::None => engine
                    .active_item()
                    .and_then(|item| engine.tab_path(item))
                    .ok_or_else(|| EngineError::NotFound("no file is open to delete".to_owned()))?,
                _ => {
                    return Err(EngineError::InvalidArguments {
                        command: ids::FILE_DELETE,
                        expected: "a file, a document, an image or nothing (the active tab)",
                    })
                }
            };
            engine.delete_file(&path).map(|()| CommandOutcome::Done)
        },
    );
    register(
        registry,
        spec(
            ids::FILE_UNDO_DELETE,
            "Undo Delete",
            CommandCategory::Edit,
            None,
            CommandContext::DeletedFile,
        ),
        |engine, _| engine.undo_delete().map(|_| CommandOutcome::Done),
    );
}

impl Engine {
    /// Whether files can be deleted: only with a state folder to keep them in.
    #[must_use]
    pub fn can_delete_files(&self) -> bool {
        self.deleted_store().is_some()
    }

    /// The file the latest deletion this session removed, which `file.undo_delete` puts back.
    #[must_use]
    pub fn last_deleted(&self) -> Option<&Path> {
        self.deletions
            .kept
            .last()
            .map(|deleted| deleted.path.as_path())
    }

    /// Deletes the file at `path`, a file (not a folder or a link) inside a root reached
    /// without a link, keeping it in the state folder for [`Engine::undo_delete`]. Its tab
    /// closes, discarding any unsaved edits.
    pub fn delete_file(&mut self, path: &Path) -> Result<(), EngineError> {
        let store = self.deleted_store().ok_or_else(|| {
            EngineError::Refused("files are deleted only where they can be kept".to_owned())
        })?;
        let path = self.deletable(path)?;
        let item = self.tab_for_path(&path);
        if let Some(TabItem::Document(id)) = item {
            // A save in flight would write the file back.
            while self.writer.is_saving(id) {
                if !self.apply_next_worker_message() {
                    break;
                }
            }
        }
        sweep(
            &store,
            self.settings.history.keep_days,
            &self.deletions.kept,
        );
        self.deletions.count += 1;
        let folder = store.join(format!(
            "{}-{}-{}",
            millis(SystemTime::now()),
            std::process::id(),
            self.deletions.count
        ));
        fs::create_dir_all(&folder).map_err(|error| EngineError::io(&folder, error))?;
        let _ = fs::write(folder.join(PATH_FILE), path.display().to_string());
        if let Err(error) = move_file(&path, &folder.join(FILE), false) {
            let _ = fs::remove_dir_all(&folder);
            return Err(EngineError::io(&path, error));
        }
        match item {
            Some(TabItem::Document(id)) => {
                let _ = self.close(id);
            }
            Some(TabItem::Image(id)) => {
                let _ = self.close_image(id);
            }
            None => {}
        }
        self.deletions.kept.push(Deleted {
            path: path.clone(),
            folder,
            was_open: item.is_some(),
        });
        self.file_came_or_went(&path);
        self.emit(EngineEvent::FileDeleted(path));
        Ok(())
    }

    /// Puts back the latest deletion this session made, opening it again if it was open.
    /// Refused, keeping the deletion, while a file is at its path.
    pub fn undo_delete(&mut self) -> Result<PathBuf, EngineError> {
        let deleted =
            self.deletions.kept.last().cloned().ok_or_else(|| {
                EngineError::NotFound("nothing was deleted to put back".to_owned())
            })?;
        if fs::symlink_metadata(&deleted.path).is_ok() {
            return Err(EngineError::Refused(format!(
                "{} exists again, so the deleted file was not put back",
                deleted.path.display()
            )));
        }
        let kept = deleted.folder.join(FILE);
        if !kept.is_file() {
            self.deletions.kept.pop();
            return Err(EngineError::NotFound(format!(
                "the deleted {} is no longer kept",
                deleted.path.display()
            )));
        }
        if let Some(parent) = deleted.path.parent() {
            fs::create_dir_all(parent).map_err(|error| EngineError::io(parent, error))?;
        }
        move_file(&kept, &deleted.path, true)
            .map_err(|error| EngineError::io(&deleted.path, error))?;
        self.deletions.kept.pop();
        let _ = fs::remove_dir_all(&deleted.folder);
        self.file_came_or_went(&deleted.path);
        if deleted.was_open {
            let _ = self.open_item(&deleted.path, OpenDisposition::Persistent);
        }
        self.emit(EngineEvent::FileRestored(deleted.path.clone()));
        Ok(deleted.path)
    }

    fn deleted_store(&self) -> Option<PathBuf> {
        self.settings
            .state_dir()
            .map(|directory| directory.join(DELETED_DIR))
    }

    /// `path`, normalised, if it is a file in a root, reached without a link.
    fn deletable(&self, path: &Path) -> Result<PathBuf, EngineError> {
        let path = normalize_path(path);
        let parent = path
            .parent()
            .ok_or_else(|| EngineError::Refused(format!("{} is not a file", path.display())))?;
        self.listable(parent)?;
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| EngineError::io(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(EngineError::Refused(format!(
                "{} is a link, which is not deleted",
                path.display()
            )));
        }
        if !metadata.is_file() {
            return Err(EngineError::Refused(format!(
                "{} is a folder; only files are deleted",
                path.display()
            )));
        }
        Ok(path)
    }

    fn tab_for_path(&self, path: &Path) -> Option<TabItem> {
        self.document_for_path(path)
            .map(TabItem::Document)
            .or_else(|| self.image_for_path(path).map(TabItem::Image))
    }

    fn tab_path(&self, item: TabItem) -> Option<PathBuf> {
        match item {
            TabItem::Document(id) => self.document(id).map(|document| document.path.clone()),
            TabItem::Image(id) => self.image(id).map(|image| image.path.clone()),
        }
    }

    /// Tells everything that knows the file at `path` that it was deleted or put back, as the
    /// watcher would.
    fn file_came_or_went(&mut self, path: &Path) {
        let name = path.file_name().and_then(|name| name.to_str());
        if self.is_settings_file(path) {
            self.settings_files_changed();
        } else if name == Some("antora.yml") {
            self.reload_descriptor(path);
        } else if name == Some(adoc_antora::PLAYBOOK_FILE) {
            self.reindex();
        } else {
            if is_asciidoc_path(path) && self.document_for_path(path).is_none() {
                match fs::read_to_string(path) {
                    Ok(text) => {
                        self.workspace.index.index_file(path, &text);
                    }
                    Err(_) => self.workspace.remove_document(path),
                }
                self.graph_file_changed(path);
                self.preview_source_changed(path);
                self.visual_source_changed(path);
                self.emit(EngineEvent::DiagnosticsUpdated {
                    path: path.to_path_buf(),
                });
            }
            // Pages, partials and images are the catalog's resources.
            self.catalog_changed_at(path);
            self.refresh_open_diagnostics(None);
            self.audit_changed_everywhere();
        }
        self.directories_changed(&[path.to_path_buf()]);
    }
}

/// Moves `from` to `to`, copying across file systems; `to` must not exist when `new` says so.
fn move_file(from: &Path, to: &Path, new: bool) -> io::Result<()> {
    if new && fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a file is there already",
        ));
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let mut source = fs::File::open(from)?;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(new)
        .create(true)
        .truncate(true)
        .open(to)?;
    let copied = io::copy(&mut source, &mut target).and_then(|_| target.sync_all());
    drop(target);
    if let Err(error) = copied.and_then(|()| fs::remove_file(from)) {
        let _ = fs::remove_file(to);
        return Err(error);
    }
    Ok(())
}

/// Removes the deletions older than `keep_days`, apart from this session's.
fn sweep(store: &Path, keep_days: u32, kept: &[Deleted]) {
    let Ok(entries) = fs::read_dir(store) else {
        return;
    };
    let oldest = millis(SystemTime::now()).saturating_sub(u64::from(keep_days) * DAY_MILLIS);
    for entry in entries.flatten() {
        let folder = entry.path();
        let made = entry
            .file_name()
            .to_str()
            .and_then(|name| name.split('-').next()?.parse::<u64>().ok());
        if made.is_some_and(|made| made < oldest)
            && !kept.iter().any(|deleted| deleted.folder == folder)
        {
            let _ = fs::remove_dir_all(folder);
        }
    }
}
