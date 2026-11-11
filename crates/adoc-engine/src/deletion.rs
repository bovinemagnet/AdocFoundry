//! Deleting a file (#100): `file.delete` moves a file of the workspace aside into the state
//! folder ([`crate::Settings::state_dir`]), closing its tab and telling the index, catalog,
//! graph and Files view as the watcher would; `file.undo_delete` puts back the latest deletion
//! made since the engine started, refusing to replace a file made at its path since. Undo is
//! the engine's per page, so a deletion, whose page has no tab left, is undone by its own
//! command. `file.restore_deleted` puts back any deletion still kept, from this session or an
//! earlier one (#434), listed by [`Engine::kept_deletions`]. Deletions kept from earlier
//! sessions are removed once older than `[history] keep_days`. What removing a file would break
//! is Inspect Impact's to show (`impact.inspect`), which a front end asks before deleting.
//!
//! Each deletion is a folder in the store, `<milliseconds>-<process>-<count>`, the time it was
//! made, holding the file as `file` and its path, as UTF-8 text, as `path`, written whole
//! before the file is moved. The store is read as untrusted: links are never followed, and a
//! recorded path is restored only when it is absolute, has no `..` and lies in a root,
//! reached without a link. Without a state folder nothing is deleted, so only the app deletes.

use std::{
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
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

/// The longest `path` record read; longer is not a path this engine wrote.
const MAX_PATH_RECORD: u64 = 64 * 1024;

/// A deletion kept in the state folder, which `file.restore_deleted` can put back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeptDeletion {
    /// Names the deletion to `file.restore_deleted`.
    pub id: String,
    /// Where the file was.
    pub path: PathBuf,
    /// When it was deleted.
    pub deleted: SystemTime,
    /// Whether a file is at `path` now, which restoring would refuse to replace.
    pub occupied: bool,
}

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
    register(
        registry,
        spec(
            ids::FILE_RESTORE_DELETED,
            "Restore Deleted File…",
            CommandCategory::Document,
            None,
            CommandContext::KeptDeletions,
        ),
        |engine, args| {
            let deletion = match &args {
                CommandArgs::Custom(value) => value.get("deletion").and_then(|id| id.as_str()),
                _ => None,
            };
            let deletion = deletion.ok_or(EngineError::InvalidArguments {
                command: ids::FILE_RESTORE_DELETED,
                expected: "a deletion, by its id from `Engine::kept_deletions`",
            })?;
            engine.restore_deleted(deletion)
        },
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
        if let Err(error) = record_path(&folder, &path) {
            let _ = fs::remove_dir_all(&folder);
            return Err(EngineError::io(&folder, error));
        }
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

    /// The deletions kept in the state folder of files in the workspace's roots, from this
    /// session and earlier ones, newest first. Reads only the state folder (and whether a file
    /// is at each path); entries it cannot trust, or with no record of their path, are left out.
    #[must_use]
    pub fn kept_deletions(&self) -> Vec<KeptDeletion> {
        let Some(store) = self.deleted_store() else {
            return Vec::new();
        };
        if !is_real_dir(&store) {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(&store) else {
            return Vec::new();
        };
        let mut kept: Vec<((u64, u64), KeptDeletion)> = entries
            .flatten()
            .filter_map(|entry| {
                let id = entry.file_name().to_str()?.to_owned();
                let order = entry_order(&id)?;
                let kept = read_entry(&store, &id)?;
                let path = self.restorable(&kept.path).ok()?;
                Some((order, KeptDeletion { path, ..kept }))
            })
            .collect();
        kept.sort_by(|(a, _), (b, _)| b.cmp(a));
        kept.into_iter().map(|(_, kept)| kept).collect()
    }

    /// Puts back the deletion `id` from [`Engine::kept_deletions`], making any folders it was
    /// in, and opens it if it is a page. Refused, keeping the deletion, while a file is at its
    /// path, and for a path outside the roots or reached through a link.
    fn restore_deleted(&mut self, id: &str) -> Result<CommandOutcome, EngineError> {
        let store = self.deleted_store().ok_or_else(|| {
            EngineError::Refused("deleted files are kept only in a state folder".to_owned())
        })?;
        let not_kept = || EngineError::NotFound(format!("no deletion {id} is kept"));
        if entry_order(id).is_none() || !is_real_dir(&store) {
            return Err(not_kept());
        }
        let kept = read_entry(&store, id).ok_or_else(not_kept)?;
        let path = self.restorable(&kept.path)?;
        if fs::symlink_metadata(&path).is_ok() {
            return Err(EngineError::Refused(format!(
                "{} exists, so the deleted file was not put back",
                path.display()
            )));
        }
        let folder = store.join(id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| EngineError::io(parent, error))?;
        }
        move_file(&folder.join(FILE), &path, true)
            .map_err(|error| EngineError::io(&path, error))?;
        let _ = fs::remove_dir_all(&folder);
        self.deletions
            .kept
            .retain(|deleted| deleted.folder != folder);
        self.file_came_or_went(&path);
        let outcome = if is_asciidoc_path(&path) {
            self.open_item(&path, OpenDisposition::Persistent)
                .map_or(CommandOutcome::Done, CommandOutcome::opened)
        } else {
            CommandOutcome::Done
        };
        self.emit(EngineEvent::FileRestored(path));
        Ok(outcome)
    }

    /// `path`, normalised, if a deleted file may be put back there: absolute, with no `..`, in
    /// a root and reached without a link.
    fn restorable(&self, path: &Path) -> Result<PathBuf, EngineError> {
        let plain = path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, Component::ParentDir));
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(not_a_place(path));
        };
        if !plain {
            return Err(not_a_place(path));
        }
        Ok(self.listable(parent)?.join(name))
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

fn not_a_place(path: &Path) -> EngineError {
    EngineError::Refused(format!(
        "{} is not a path a deleted file is put back at",
        path.display()
    ))
}

/// Writes the record of where the file deleted into `folder` was, whole: to a temporary file
/// first, then renamed. A path that is not UTF-8 is not recorded, so it can only be undone.
fn record_path(folder: &Path, path: &Path) -> io::Result<()> {
    let Some(text) = path.to_str() else {
        return Ok(());
    };
    let partial = folder.join(format!("{PATH_FILE}.partial"));
    fs::write(&partial, text)?;
    fs::rename(&partial, folder.join(PATH_FILE))
}

/// The time and count an entry's name `<milliseconds>-<process>-<count>` gives, to order
/// entries by; `None` for any other name, which is not an entry.
fn entry_order(id: &str) -> Option<(u64, u64)> {
    let mut parts = id.split('-');
    let made = parts.next()?.parse().ok()?;
    parts.next()?.parse::<u64>().ok()?;
    let count = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((made, count))
}

/// Whether `path` is a folder itself, not a link to one.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Whether `path` is a file itself, not a link to one.
fn is_real_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// The entry `id` of the store, if it is a folder holding a file and the record of a path.
/// Following no link; whether the path may be restored to is the caller's to check.
fn read_entry(store: &Path, id: &str) -> Option<KeptDeletion> {
    let (made, _) = entry_order(id)?;
    let folder = store.join(id);
    let record = folder.join(PATH_FILE);
    if !is_real_dir(&folder) || !is_real_file(&folder.join(FILE)) || !is_real_file(&record) {
        return None;
    }
    let mut text = String::new();
    fs::File::open(&record)
        .ok()?
        .take(MAX_PATH_RECORD)
        .read_to_string(&mut text)
        .ok()?;
    let path = PathBuf::from(text);
    let occupied = fs::symlink_metadata(&path).is_ok();
    Some(KeptDeletion {
        id: id.to_owned(),
        path,
        deleted: UNIX_EPOCH + Duration::from_millis(made),
        occupied,
    })
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
