//! The engine's writer: saves, recovery records and state files are written on one thread of
//! their own, in the order they were asked for, so no write (or its `fsync`) runs on the thread
//! that serves the front end, and no later write is overtaken by an earlier one. Each job is
//! counted in [`Engine::has_pending`] and reported back through `pump`.

use std::{
    collections::BTreeMap,
    fs, io,
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, SystemTime},
};

use ropey::Rope;

use crate::{
    document::hash_text, engine::write_replacing, local_history::HistoryJob,
    workspace::WorkerMessage, DocumentId, Engine, EngineError, EngineEvent,
};

/// How long a save made by hand waits for its write before it gives up.
const SAVE_TIMEOUT: Duration = Duration::from_secs(60);

/// A page to write, as it stood when the save was asked for.
pub(crate) struct SaveJob {
    pub(crate) ticket: u64,
    pub(crate) id: DocumentId,
    pub(crate) version: u64,
    pub(crate) path: PathBuf,
    pub(crate) text: Rope,
    /// The hash of the file as last read or written: one that no longer matches is not
    /// overwritten. `None` writes whatever is there.
    pub(crate) expected: Option<u64>,
    pub(crate) history: Option<HistoryJob>,
    /// An auto-save, whose failure is reported as an event; a save by hand returns it.
    pub(crate) background: bool,
}

/// The file as written: what the page is clean against.
#[derive(Debug)]
pub(crate) struct Written {
    hash: u64,
    len: usize,
}

#[derive(Debug)]
pub(crate) enum SaveFailure {
    /// The file changed on disk since it was last read or written.
    Conflict,
    Io(io::Error),
}

pub(crate) enum Job {
    Save(Box<SaveJob>),
    /// Writes a file, creating its folder, with the contents worked out on the writer. A
    /// failure costs only the file.
    Write {
        file: PathBuf,
        contents: Box<dyn FnOnce() -> Option<String> + Send>,
    },
    Remove(PathBuf),
    /// Answers once every job before it is done.
    Flush(Sender<()>),
}

#[derive(Default)]
pub(crate) struct WriterState {
    jobs: Option<Sender<Job>>,
    next_ticket: u64,
    /// The saves of each page still being written.
    saving: BTreeMap<DocumentId, usize>,
}

impl WriterState {
    pub(crate) fn is_saving(&self, id: DocumentId) -> bool {
        self.saving.contains_key(&id)
    }
}

impl Engine {
    /// Hands `job` to the writer, starting it the first time.
    fn send_job(&mut self, job: Job) {
        let counted = !matches!(job, Job::Flush(_));
        let jobs = self.writer.jobs.get_or_insert_with(|| {
            let (jobs, queue) = mpsc::channel::<Job>();
            let sender = self.worker_sender.clone();
            thread::spawn(move || {
                for job in queue {
                    let message = match job {
                        Job::Save(job) => {
                            // A panic fails this save; the writer goes on to the next job.
                            let result =
                                crate::worker::guarded(|| save(&job)).unwrap_or_else(|error| {
                                    Err(SaveFailure::Io(io::Error::other(format!(
                                        "saving crashed: {error}"
                                    ))))
                                });
                            WorkerMessage::Saved {
                                ticket: job.ticket,
                                id: job.id,
                                version: job.version,
                                background: job.background,
                                result,
                            }
                        }
                        Job::Write { file, contents } => {
                            let _ = crate::worker::guarded(|| {
                                if let Some(contents) = contents() {
                                    let _ = file
                                        .parent()
                                        .map_or(Ok(()), fs::create_dir_all)
                                        .and_then(|()| write_replacing(&file, &contents));
                                }
                            });
                            WorkerMessage::Written
                        }
                        Job::Remove(file) => {
                            let _ = fs::remove_file(file);
                            WorkerMessage::Written
                        }
                        Job::Flush(done) => {
                            let _ = done.send(());
                            continue;
                        }
                    };
                    let _ = sender.send(message);
                }
            });
            jobs
        });
        if jobs.send(job).is_ok() && counted {
            self.workers_running += 1;
        }
    }

    /// Writes `file` on the writer, with `contents` worked out there.
    pub(crate) fn write_later(
        &mut self,
        file: PathBuf,
        contents: impl FnOnce() -> Option<String> + Send + 'static,
    ) {
        self.send_job(Job::Write {
            file,
            contents: Box::new(contents),
        });
    }

    /// Removes `file` on the writer, after every write asked for before.
    pub(crate) fn remove_later(&mut self, file: PathBuf) {
        self.send_job(Job::Remove(file));
    }

    /// Blocks until every write asked for so far is on disk: for an app about to quit.
    pub(crate) fn flush_writes(&mut self) {
        if self.writer.jobs.is_none() {
            return;
        }
        let (done, flushed) = mpsc::channel();
        self.send_job(Job::Flush(done));
        let _ = flushed.recv_timeout(SAVE_TIMEOUT);
    }

    /// Starts writing document `id` to its file as it stands. Returns the job's ticket.
    pub(crate) fn start_save(
        &mut self,
        id: DocumentId,
        overwrite: bool,
        background: bool,
    ) -> Result<u64, EngineError> {
        let document = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?;
        if document.untitled {
            return Err(EngineError::NotFound(format!(
                "{} has never been saved and has no file",
                document.path.display()
            )));
        }
        let (version, path, text) = (
            document.version,
            document.path.clone(),
            document.text.clone(),
        );
        let expected = if overwrite {
            None
        } else {
            self.states.get(&id).map(|state| state.saved_hash)
        };
        let history = self.history_job(&path);
        self.writer.next_ticket += 1;
        let ticket = self.writer.next_ticket;
        *self.writer.saving.entry(id).or_default() += 1;
        self.send_job(Job::Save(Box::new(SaveJob {
            ticket,
            id,
            version,
            path,
            text,
            expected,
            history,
            background,
        })));
        Ok(ticket)
    }

    /// Saves document `id` and waits for the write, after any save of it already being written,
    /// so its conflict check sees what that one wrote.
    pub(crate) fn save_now(&mut self, id: DocumentId, overwrite: bool) -> Result<(), EngineError> {
        while self.writer.is_saving(id) {
            if !self.apply_next_worker_message() {
                break;
            }
        }
        let ticket = self.start_save(id, overwrite, false)?;
        loop {
            match self.worker_receiver.recv_timeout(SAVE_TIMEOUT) {
                Ok(WorkerMessage::Saved {
                    ticket: done,
                    id,
                    version,
                    result,
                    ..
                }) if done == ticket => {
                    self.workers_running = self.workers_running.saturating_sub(1);
                    return self.document_written(id, version, result);
                }
                Ok(message) => self.apply_worker_message(message),
                Err(_) => {
                    let path = self
                        .documents
                        .get(&id)
                        .map(|document| document.path.clone());
                    return Err(EngineError::io(
                        path.unwrap_or_default(),
                        io::Error::new(io::ErrorKind::TimedOut, "the file was not written in time"),
                    ));
                }
            }
        }
    }

    /// Waits for the next worker message and applies it; false when none came in time.
    fn apply_next_worker_message(&mut self) -> bool {
        match self.worker_receiver.recv_timeout(SAVE_TIMEOUT) {
            Ok(message) => {
                self.apply_worker_message(message);
                true
            }
            Err(_) => false,
        }
    }

    /// Applies a finished save: the page is clean against what was written, unless it was
    /// edited since; an auto-save's failure is reported and the page left for a save by hand.
    pub(crate) fn save_finished(
        &mut self,
        id: DocumentId,
        version: u64,
        background: bool,
        result: Result<Written, SaveFailure>,
    ) {
        if let Err(error) = self.document_written(id, version, result) {
            if background {
                self.auto_save_refused(id, &error);
            }
        }
    }

    fn document_written(
        &mut self,
        id: DocumentId,
        version: u64,
        result: Result<Written, SaveFailure>,
    ) -> Result<(), EngineError> {
        if let Some(saving) = self.writer.saving.get_mut(&id) {
            *saving -= 1;
            if *saving == 0 {
                self.writer.saving.remove(&id);
            }
        }
        let Some(document) = self.documents.get_mut(&id) else {
            return Ok(()); // Closed while it was written.
        };
        let written = match result {
            Ok(written) => written,
            Err(SaveFailure::Conflict) => {
                return Err(EngineError::SaveConflict {
                    path: document.path.clone(),
                })
            }
            Err(SaveFailure::Io(error)) => return Err(EngineError::io(&document.path, error)),
        };
        if let Some(state) = self.states.get_mut(&id) {
            state.saved_hash = written.hash;
            state.saved_len = written.len;
            state.changed_on_disk = false;
            document.dirty = document.version != version && !state.is_saved(&document.text);
        }
        self.auto_save_settled(id);
        self.recovery_changed(id);
        self.emit(EngineEvent::DocumentSaved(id));
        self.keep_open(id);
        self.schedule_hunks(id, false);
        self.forget_blame(id);
        Ok(())
    }
}

/// Writes a page, unless the file changed on disk since it was last read or written, and keeps
/// the version in the local history.
fn save(job: &SaveJob) -> Result<Written, SaveFailure> {
    let on_disk = fs::read_to_string(&job.path);
    if let Some(expected) = job.expected {
        match &on_disk {
            Ok(on_disk) if hash_text(on_disk) == expected => {}
            // Deleted outside: writing it again loses nothing.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            // Changed, or no longer text.
            _ => return Err(SaveFailure::Conflict),
        }
    }
    // The text replaced, for the local history.
    let previous = on_disk.ok().map(|text| {
        let modified = fs::metadata(&job.path).and_then(|metadata| metadata.modified());
        (text, modified.unwrap_or_else(|_| SystemTime::now()))
    });
    let text = job.text.to_string();
    write_replacing(&job.path, &text).map_err(SaveFailure::Io)?;
    if let Some(history) = &job.history {
        history.record(&job.path, previous, &text);
    }
    Ok(Written {
        hash: hash_text(&text),
        len: text.len(),
    })
}
