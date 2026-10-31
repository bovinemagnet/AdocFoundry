//! Local history: the versions of each page as it was saved, kept in the app's state folder
//! ([`crate::Settings::state_dir`]) independently of Git, to compare with the page and restore.
//!
//! Each page has a folder in the store, named from a digest of its path, holding one file per
//! version named `<milliseconds>-<digest of the text>`. Every save keeps the text written, and
//! first the text it replaces when the history does not hold that already: the file as it was
//! before the first save, or a change made outside the app. Versions older than
//! `[history] keep_days`, and all but the newest `max_versions`, are removed. Only a front end
//! that calls [`Engine::enable_local_history`] (the app) keeps one.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use adoc_core::SourceRange;
use sha2::{Digest, Sha256};

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    line_diff, CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry,
    DiffLine, DocumentId, Engine, EngineError, HistorySettings, TextEdit,
};

/// The store's folder, in the app's state folder.
pub const HISTORY_DIR: &str = "history";

/// Pages larger than this are not kept.
pub const HISTORY_MAX_PAGE: usize = 4 * 1024 * 1024;

/// The file in each page's folder that names the page.
const PATH_FILE: &str = "path";

/// How often a save also prunes every page's folder and keeps the whole history under
/// `[history] max_size_mb`.
pub const HISTORY_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

const DAY_MILLIS: u64 = 24 * 60 * 60 * 1000;
const MEGABYTE: u64 = 1024 * 1024;

#[derive(Default)]
pub(crate) struct LocalHistory {
    enabled: bool,
    /// When every page's folder was last pruned, not only the one saved.
    swept: Option<Instant>,
}

/// A saved version of a page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedVersion {
    /// Names the version to [`Engine::saved_version`] and `document.restore_version`.
    pub id: String,
    pub saved: SystemTime,
    /// In bytes.
    pub size: u64,
}

/// A version's file name, read back.
struct Stored {
    id: String,
    millis: u64,
    digest: String,
}

pub(crate) fn register_local_history(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::DOCUMENT_LOCAL_HISTORY,
            "Local History…",
            CommandCategory::Document,
            None,
            CommandContext::ActiveDocument,
        ),
        |engine, args| {
            let id = match args {
                CommandArgs::Document(id) => id,
                _ => engine
                    .active()
                    .ok_or_else(|| EngineError::NotFound("no page is open".to_owned()))?,
            };
            let document = engine
                .document(id)
                .ok_or(EngineError::UnknownDocument(id))?;
            let versions = if document.untitled {
                Vec::new()
            } else {
                engine.local_history(&document.path)
            };
            Ok(CommandOutcome::LocalHistory {
                document: id,
                versions,
            })
        },
    );
    register(
        registry,
        spec(
            ids::DOCUMENT_RESTORE_VERSION,
            "Restore Saved Version",
            CommandCategory::Document,
            None,
            CommandContext::ActiveDocument,
        ),
        restore_version,
    );
}

/// Takes `CommandArgs::Custom` with `document` and `version`, and puts that version's text in
/// the page as one undo step, unsaved.
fn restore_version(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let invalid = EngineError::InvalidArguments {
        command: ids::DOCUMENT_RESTORE_VERSION,
        expected: "a document and a version from its local history",
    };
    let CommandArgs::Custom(value) = args else {
        return Err(invalid);
    };
    let (Some(id), Some(version)) = (
        value
            .get("document")
            .and_then(serde_json::Value::as_u64)
            .map(DocumentId),
        value.get("version").and_then(serde_json::Value::as_str),
    ) else {
        return Err(invalid);
    };
    let document = engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?;
    let text = engine.saved_version(&document.path, version)?;
    let current = document.text.to_string();
    if current != text {
        let group = engine.new_group();
        engine.apply_edits(
            id,
            vec![TextEdit::new(SourceRange::new(0, current.len()), text)],
            UndoMode::Group(group),
        )?;
    }
    Ok(CommandOutcome::Done)
}

impl Engine {
    /// Keeps each page's saved versions in the local history store. Only the app keeps one.
    pub fn enable_local_history(&mut self) {
        self.local_history.enabled = true;
    }

    /// The saved versions of the page at `path`, newest first.
    #[must_use]
    pub fn local_history(&self, path: &Path) -> Vec<SavedVersion> {
        let Some(folder) = self.history_folder(path) else {
            return Vec::new();
        };
        stored(&folder)
            .into_iter()
            .rev()
            .map(|version| SavedVersion {
                size: fs::metadata(folder.join(&version.id)).map_or(0, |metadata| metadata.len()),
                saved: UNIX_EPOCH + Duration::from_millis(version.millis),
                id: version.id,
            })
            .collect()
    }

    /// The text of version `id` of the page at `path`: only a version its history lists.
    pub fn saved_version(&self, path: &Path, id: &str) -> Result<String, EngineError> {
        let missing =
            || EngineError::NotFound(format!("{id} is not a saved version of {}", path.display()));
        let folder = self.history_folder(path).ok_or_else(missing)?;
        if !stored(&folder).iter().any(|version| version.id == id) {
            return Err(missing());
        }
        let file = folder.join(id);
        fs::read_to_string(&file).map_err(|error| EngineError::io(file, error))
    }

    /// The lines document `id` has changed since version `version` of its page.
    pub fn version_diff(
        &self,
        id: DocumentId,
        version: &str,
    ) -> Result<Vec<DiffLine>, EngineError> {
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        let saved = self.saved_version(&document.path, version)?;
        Ok(line_diff(&saved, &document.text.to_string()))
    }

    /// Where a save of the page at `path` keeps its versions, and whether it also sweeps the
    /// whole store; `None` when no history is kept. Decided here, written by the saver.
    pub(crate) fn history_job(&mut self, path: &Path) -> Option<HistoryJob> {
        let store = self.history_store()?;
        let folder = self.history_folder(path)?;
        let now = self.clock.now();
        let sweep = self
            .local_history
            .swept
            .is_none_or(|swept| now >= swept + HISTORY_SWEEP_INTERVAL);
        if sweep {
            self.local_history.swept = Some(now);
        }
        Some(HistoryJob {
            store,
            folder,
            settings: self.settings.history.clone(),
            sweep,
        })
    }

    /// Moves the history of the page at `from` to its new path `to`, joining any history
    /// `to` had. A failure leaves the versions where they were.
    pub(crate) fn move_history(&self, from: &Path, to: &Path) {
        let (Some(old), Some(new)) = (self.history_folder(from), self.history_folder(to)) else {
            return;
        };
        if old == new || !old.is_dir() || fs::create_dir_all(&new).is_err() {
            return;
        }
        for version in stored(&old) {
            let _ = fs::rename(old.join(&version.id), new.join(&version.id));
        }
        let _ = fs::write(new.join(PATH_FILE), to.display().to_string());
        if stored(&old).is_empty() {
            let _ = fs::remove_file(old.join(PATH_FILE));
            let _ = fs::remove_dir(&old);
        }
        prune_history(&new, &self.settings.history);
    }

    fn history_store(&self) -> Option<PathBuf> {
        if !self.local_history.enabled || !self.settings.history.enabled {
            return None;
        }
        self.settings
            .state_dir()
            .map(|directory| directory.join(HISTORY_DIR))
    }

    fn history_folder(&self, path: &Path) -> Option<PathBuf> {
        let key = digest(&path.display().to_string());
        self.history_store().map(|store| store.join(key))
    }
}

/// The versions in `folder`, oldest first. Files not named as versions are ignored.
fn stored(folder: &Path) -> Vec<Stored> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut versions: Vec<Stored> = entries
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_str()?.to_owned();
            let (at, digest) = id.split_once('-')?;
            let is_digest = digest.len() == 16
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
            let millis = at.parse().ok().filter(|_| is_digest)?;
            let digest = digest.to_owned();
            Some(Stored { id, millis, digest })
        })
        .collect();
    versions.sort_by(|a, b| a.millis.cmp(&b.millis).then(a.id.cmp(&b.id)));
    versions
}

/// The first 16 hex digits of the SHA-256 of `text`.
fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// Where and how one save keeps the page's versions.
pub(crate) struct HistoryJob {
    store: PathBuf,
    folder: PathBuf,
    settings: HistorySettings,
    /// Whether this save also prunes every page's folder and the store's size.
    sweep: bool,
}

impl HistoryJob {
    /// Keeps `saved`, just written to `path`, and first `previous`, the file it replaced with
    /// when it was last modified, when the history does not end with them already. A failure
    /// costs only the version.
    pub(crate) fn record(&self, path: &Path, previous: Option<(String, SystemTime)>, saved: &str) {
        let folder = &self.folder;
        if fs::create_dir_all(folder).is_err() {
            return;
        }
        let path_file = folder.join(PATH_FILE);
        if !path_file.exists() {
            let _ = fs::write(&path_file, path.display().to_string());
        }
        let mut latest = stored(folder)
            .pop()
            .map(|version| (version.millis, version.digest));
        let candidates = previous
            .map(|(text, modified)| (text, millis(modified)))
            .into_iter()
            .chain([(saved.to_owned(), millis(SystemTime::now()))]);
        for (text, at) in candidates {
            let digest = digest(&text);
            if text.len() > HISTORY_MAX_PAGE
                || latest.as_ref().is_some_and(|(_, last)| *last == digest)
            {
                continue;
            }
            // Strictly after the version before, so the order is the order of saving.
            let at = latest.as_ref().map_or(at, |(last, _)| at.max(last + 1));
            if fs::write(folder.join(format!("{at}-{digest}")), &text).is_ok() {
                latest = Some((at, digest));
            }
        }
        prune_history(folder, &self.settings);
        if self.sweep {
            sweep_history(&self.store, &self.settings);
        }
    }
}

/// Removes the versions in `folder` older than `keep_days` and beyond `max_versions`, and
/// the folder once it holds none.
fn prune_history(folder: &Path, settings: &HistorySettings) {
    let oldest =
        millis(SystemTime::now()).saturating_sub(u64::from(settings.keep_days) * DAY_MILLIS);
    let versions = stored(folder);
    let surplus = versions
        .len()
        .saturating_sub(settings.max_versions as usize);
    let mut kept = 0;
    for (index, version) in versions.iter().enumerate() {
        if index < surplus || version.millis < oldest {
            let _ = fs::remove_file(folder.join(&version.id));
        } else {
            kept += 1;
        }
    }
    if kept == 0 {
        let _ = fs::remove_file(folder.join(PATH_FILE));
        let _ = fs::remove_dir(folder);
    }
}

/// Prunes every page's folder, for the pages not saved for a while, then removes the
/// oldest versions of any page until the whole history is under `max_size_mb`.
fn sweep_history(store: &Path, settings: &HistorySettings) {
    let Ok(entries) = fs::read_dir(store) else {
        return;
    };
    let folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|folder| folder.is_dir())
        .collect();
    let mut versions = Vec::new();
    for folder in &folders {
        prune_history(folder, settings);
        for version in stored(folder) {
            let file = folder.join(&version.id);
            let size = fs::metadata(&file).map_or(0, |metadata| metadata.len());
            versions.push((version.millis, file, size));
        }
    }
    versions.sort();
    let limit = u64::from(settings.max_size_mb) * MEGABYTE;
    let mut total: u64 = versions.iter().map(|(_, _, size)| size).sum();
    for (_, file, size) in versions {
        if total <= limit {
            break;
        }
        if fs::remove_file(&file).is_ok() {
            total -= size;
        }
    }
    for folder in &folders {
        if stored(folder).is_empty() {
            let _ = fs::remove_file(folder.join(PATH_FILE));
            let _ = fs::remove_dir(folder);
        }
    }
}
