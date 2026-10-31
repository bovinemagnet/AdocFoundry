//! Crash recovery: the text of each page with unsaved edits is written to a recovery store
//! in the app's state folder ([`crate::Settings::state_dir`]) once typing pauses for [`RECOVERY_SETTLE`], and removed once
//! the page is saved, closed or back to its saved text. A session that ended without saving
//! leaves its records behind, and the next session offers them back.
//!
//! Each session holds a lock on a file of its own in the store for as long as it runs, so the
//! records of an app still running elsewhere are never offered. Only a front end that calls
//! [`Engine::enable_recovery`] (the app) writes records: a language server's buffers belong to
//! its editor.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use adoc_core::SourceRange;
use serde::{Deserialize, Serialize};

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    line_diff, CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry,
    DiffLine, DocumentId, Engine, EngineError, TextEdit,
};

/// How long a page must go unedited before its unsaved text is written to the store.
pub const RECOVERY_SETTLE: Duration = Duration::from_secs(2);

/// The store's folder, in the app's state folder.
pub const RECOVERY_DIR: &str = "recovery";

const FORMAT_VERSION: u64 = 1;
const RECORD_EXTENSION: &str = "json";
const LOCK_EXTENSION: &str = "lock";

#[derive(Default)]
pub(crate) struct RecoveryState {
    enabled: bool,
    /// This session, once it has written a record.
    session: Option<Session>,
    /// When each page waiting to be written was last edited.
    due: BTreeMap<DocumentId, Instant>,
    /// The record written for each page.
    written: BTreeMap<DocumentId, PathBuf>,
}

/// A session's name, which starts each of its records' names, and the lock it holds while it
/// runs.
struct Session {
    name: String,
    _lock: fs::File,
}

/// Unsaved text a session that has ended left behind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recoverable {
    /// The record, which names it to `document.recover` and `document.discard_recovery`.
    pub record: PathBuf,
    /// The page's file, or the name an untitled page had.
    pub path: PathBuf,
    pub untitled: bool,
    /// When the text was written to the store.
    pub written: SystemTime,
    pub text: String,
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: u64,
    path: PathBuf,
    untitled: bool,
    /// Milliseconds since the Unix epoch.
    written: u64,
    text: String,
}

pub(crate) fn register_recovery(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::DOCUMENT_RECOVER_UNSAVED,
            "Recover Unsaved Changes…",
            CommandCategory::Document,
            None,
            CommandContext::Always,
        ),
        |engine, _| Ok(CommandOutcome::Recoverable(engine.recoverable())),
    );
    register(
        registry,
        spec(
            ids::DOCUMENT_RECOVER,
            "Recover Unsaved Page",
            CommandCategory::Document,
            None,
            CommandContext::Always,
        ),
        recover,
    );
    register(
        registry,
        spec(
            ids::DOCUMENT_DISCARD_RECOVERY,
            "Discard Unsaved Page",
            CommandCategory::Document,
            None,
            CommandContext::Always,
        ),
        |engine, args| {
            let item = engine.offered(ids::DOCUMENT_DISCARD_RECOVERY, args)?;
            engine.remove_offered(&item);
            Ok(CommandOutcome::Done)
        },
    );
}

/// Opens the page of an offered record, or a new untitled page, and puts the recovered text
/// in it as one undo step, unsaved.
fn recover(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let item = engine.offered(ids::DOCUMENT_RECOVER, args)?;
    let id = if item.untitled {
        engine.open_untitled()
    } else if item.path.exists() {
        engine.open(&item.path)?
    } else {
        // Deleted since: the page is recovered at its path, to be saved there again.
        let id = engine.open_text(&item.path, "");
        engine.set_active(id);
        id
    };
    let current = engine
        .document(id)
        .map(|document| document.text.to_string())
        .unwrap_or_default();
    if current != item.text {
        let group = engine.new_group();
        engine.apply_edits(
            id,
            vec![TextEdit::new(
                SourceRange::new(0, current.len()),
                item.text.clone(),
            )],
            UndoMode::Group(group),
        )?;
    }
    engine.remove_offered(&item);
    Ok(CommandOutcome::Opened(id))
}

impl Engine {
    /// Writes the unsaved text of edited pages to the recovery store, for the next session to
    /// offer back if this one ends first. Only the app keeps one.
    pub fn enable_recovery(&mut self) {
        self.recovery.enabled = true;
    }

    /// What sessions that have ended left unsaved, newest first. Never this session's own, nor
    /// those of an app still running.
    #[must_use]
    pub fn recoverable(&self) -> Vec<Recoverable> {
        let Some(store) = self.recovery_store() else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(&store) else {
            return Vec::new();
        };
        let own = self.recovery.session.as_ref().map(|session| &session.name);
        let mut items: Vec<Recoverable> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| has_extension(path, RECORD_EXTENSION))
            .filter(|path| session_of(path).is_some_and(|session| Some(&session) != own))
            .filter(|path| session_of(path).is_some_and(|session| !is_running(&store, &session)))
            .filter_map(|path| read_record(&path))
            .collect();
        items.sort_by(|a, b| b.written.cmp(&a.written).then(a.path.cmp(&b.path)));
        items
    }

    /// The lines the recovered text changes in the page's file as it is now (in nothing, for
    /// an untitled page or a deleted file).
    #[must_use]
    pub fn recovery_diff(&self, item: &Recoverable) -> Vec<DiffLine> {
        let on_disk = if item.untitled {
            String::new()
        } else {
            fs::read_to_string(&item.path).unwrap_or_default()
        };
        line_diff(&on_disk, &item.text)
    }

    /// Writes every page waiting for [`RECOVERY_SETTLE`] now, and waits for every write the
    /// engine has started, saves included: for an app about to quit.
    pub fn write_recovery_now(&mut self) {
        let due: Vec<DocumentId> = self.recovery.due.keys().copied().collect();
        for id in due {
            self.recovery.due.remove(&id);
            self.write_record(id);
        }
        self.flush_writes();
    }

    /// Notes that `id` changed, or was saved or read back: an unsaved page is written once
    /// typing pauses, and a clean one's record goes at once.
    pub(crate) fn recovery_changed(&mut self, id: DocumentId) {
        if !self.recovery.enabled {
            return;
        }
        if self
            .documents
            .get(&id)
            .is_some_and(|document| document.dirty)
        {
            let due = self.clock.now() + RECOVERY_SETTLE;
            self.recovery.due.insert(id, due);
        } else {
            self.forget_recovery(id);
        }
    }

    /// The page closed, or its edits are gone: its record goes with them.
    pub(crate) fn forget_recovery(&mut self, id: DocumentId) {
        self.recovery.due.remove(&id);
        if let Some(record) = self.recovery.written.remove(&id) {
            // After the write that made it, which may not have happened yet.
            self.remove_later(record);
        }
    }

    pub(crate) fn has_due_recovery(&self) -> bool {
        !self.recovery.due.is_empty()
    }

    /// Writes the records of pages left unedited for [`RECOVERY_SETTLE`]; called from `pump`.
    pub(crate) fn write_due_recovery(&mut self) -> usize {
        let now = self.clock.now();
        let due: Vec<DocumentId> = self
            .recovery
            .due
            .iter()
            .filter(|(_, due)| now >= **due)
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            self.recovery.due.remove(id);
            self.write_record(*id);
        }
        due.len()
    }

    /// Writes `id`'s text as its record, replacing the one before, on the writer. A failure
    /// costs only the chance to recover it.
    fn write_record(&mut self, id: DocumentId) {
        let Some(document) = self.documents.get(&id).filter(|document| document.dirty) else {
            self.forget_recovery(id);
            return;
        };
        let (path, untitled, text) = (
            document.path.clone(),
            document.untitled,
            document.text.clone(),
        );
        let Some(session) = self.recovery_session() else {
            return;
        };
        let Some(store) = self.recovery_store() else {
            return;
        };
        let file = store.join(format!("{session}-{}.{RECORD_EXTENSION}", id.get()));
        // The text is copied and written on the writer, so a long page costs typing nothing.
        self.write_later(file.clone(), move || {
            serde_json::to_string(&Record {
                version: FORMAT_VERSION,
                path,
                untitled,
                written: millis(SystemTime::now()),
                text: text.to_string(),
            })
            .ok()
        });
        self.recovery.written.insert(id, file);
    }

    /// This session's name, starting the session (creating the store and taking its lock)
    /// the first time a record is written.
    fn recovery_session(&mut self) -> Option<String> {
        if let Some(session) = &self.recovery.session {
            return Some(session.name.clone());
        }
        let store = self.recovery_store()?;
        fs::create_dir_all(&store).ok()?;
        let started = format!("{}-{}", std::process::id(), millis(SystemTime::now()));
        // A new lock file, never one another session holds or left behind.
        let (name, lock) = (0..64).find_map(|attempt| {
            let name = format!("{started}.{attempt}");
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(store.join(format!("{name}.{LOCK_EXTENSION}")))
                .ok()?;
            Some((name, lock))
        })?;
        lock.try_lock().ok()?;
        self.recovery.session = Some(Session {
            name: name.clone(),
            _lock: lock,
        });
        Some(name)
    }

    fn recovery_store(&self) -> Option<PathBuf> {
        if !self.recovery.enabled {
            return None;
        }
        self.settings
            .state_dir()
            .map(|directory| directory.join(RECOVERY_DIR))
    }

    /// The offered record `args` names; anything else is refused, so these commands never
    /// touch a file outside the store.
    fn offered(
        &self,
        command: crate::CommandId,
        args: CommandArgs,
    ) -> Result<Recoverable, EngineError> {
        let CommandArgs::Path(record) = args else {
            return Err(EngineError::InvalidArguments {
                command,
                expected: "a recovery record",
            });
        };
        self.recoverable()
            .into_iter()
            .find(|item| item.record == record)
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "{} is not unsaved text that can be recovered",
                    record.display()
                ))
            })
    }

    /// Removes an offered record, and its session's lock once nothing of the session is left.
    fn remove_offered(&mut self, item: &Recoverable) {
        let _ = fs::remove_file(&item.record);
        let (Some(store), Some(session)) = (self.recovery_store(), session_of(&item.record)) else {
            return;
        };
        let left = fs::read_dir(&store).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                has_extension(&path, RECORD_EXTENSION)
                    && session_of(&path).as_ref() == Some(&session)
            })
        });
        if !left {
            let _ = fs::remove_file(store.join(format!("{session}.{LOCK_EXTENSION}")));
        }
    }
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|found| found == extension)
}

/// The session that wrote `record`: its name less the page's number.
fn session_of(record: &Path) -> Option<String> {
    let stem = record.file_stem()?.to_str()?;
    stem.rsplit_once('-').map(|(session, _)| session.to_owned())
}

/// Whether `session` still runs: its lock is held. A session whose lock file is gone ended.
fn is_running(store: &Path, session: &str) -> bool {
    let Ok(lock) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.join(format!("{session}.{LOCK_EXTENSION}")))
    else {
        return false;
    };
    // Taking the lock succeeds only once the session has ended; dropping it lets go again.
    lock.try_lock().is_err()
}

fn read_record(path: &Path) -> Option<Recoverable> {
    let text = fs::read_to_string(path).ok()?;
    let record: Record = serde_json::from_str(&text).ok()?;
    (record.version == FORMAT_VERSION).then(|| Recoverable {
        record: path.to_path_buf(),
        path: record.path,
        untitled: record.untitled,
        written: UNIX_EPOCH + Duration::from_millis(record.written),
        text: record.text,
    })
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}
