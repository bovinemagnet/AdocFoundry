//! Workspace roots, background indexing and the file watcher.
//!
//! Indexing runs on a worker thread that hands its finished [`WorkspaceState`] back through a
//! channel; file-system events are collected from `notify` and debounced. Both are applied only
//! by [`Engine::pump`], on the thread that owns the engine.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use adoc_index::{is_asciidoc_path, normalize_path};
use adoc_lang::WorkspaceState;
use notify::{event::CreateKind, event::RemoveKind, EventKind, RecursiveMode, Watcher};

use crate::{Engine, EngineError, EngineEvent, TextEdit};

/// File-system events closer together than this for one path are applied once.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);

pub(crate) enum WorkerMessage {
    Indexed {
        roots: Vec<PathBuf>,
        state: Box<WorkspaceState>,
        files: usize,
    },
    IndexFailed {
        error: String,
    },
    Search {
        id: crate::SearchId,
        results: Vec<crate::SearchMatch>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Change {
    File,
    Directory,
}

pub(crate) struct FileWatcher {
    // Held for its side effect: dropping it stops watching.
    _watcher: notify::RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    pending: BTreeMap<PathBuf, (Instant, Change)>,
}

impl FileWatcher {
    fn start(roots: &[PathBuf]) -> notify::Result<Self> {
        let (sender, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        for root in roots {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }
        Ok(Self {
            _watcher: watcher,
            events,
            pending: BTreeMap::new(),
        })
    }
}

impl Engine {
    /// Sets the workspace roots, loads the workspace settings layer from the first root, starts
    /// indexing on a worker thread and starts watching the roots for changes.
    pub fn open_workspace(&mut self, roots: Vec<PathBuf>) -> Result<(), EngineError> {
        let roots: Vec<PathBuf> = roots.iter().map(|root| normalize_path(root)).collect();
        for root in &roots {
            if !root.is_dir() {
                return Err(EngineError::NotFound(format!(
                    "{} is not a directory",
                    root.display()
                )));
            }
        }
        self.roots = roots;
        self.settings
            .set_workspace_root(self.roots.first().map(PathBuf::as_path));
        self.reload_settings()?;
        self.watcher = match FileWatcher::start(&self.roots) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                // Watching is a convenience; the workspace still works without it.
                self.emit(EngineEvent::CommandFailed {
                    command: crate::commands::ids::WORKSPACE_OPEN_FOLDER,
                    error: format!("file watching unavailable: {error}"),
                });
                None
            }
        };
        self.reindex();
        Ok(())
    }

    /// The roots of the open workspace.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Re-indexes every root on a worker thread.
    pub fn reindex(&mut self) {
        let roots = self.roots.clone();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let mut state = WorkspaceState::new();
            let message = match state.index_roots(&roots) {
                Ok(files) => WorkerMessage::Indexed {
                    roots,
                    state: Box::new(state),
                    files,
                },
                Err(error) => WorkerMessage::IndexFailed {
                    error: format!("indexing failed: {error}"),
                },
            };
            let _ = sender.send(message);
        });
    }

    /// True while indexing runs or file-system changes wait to be applied.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.workers_running > 0
            || self
                .watcher
                .as_ref()
                .is_some_and(|watcher| !watcher.pending.is_empty())
    }

    /// Applies results from worker threads and debounced file-system changes. Returns the number
    /// applied. Front ends call this on a timer while [`Engine::has_pending`] is true.
    pub fn pump(&mut self) -> usize {
        let mut applied = 0;
        while let Ok(message) = self.worker_receiver.try_recv() {
            self.apply_worker_message(message);
            applied += 1;
        }
        applied + self.apply_file_changes(Instant::now())
    }

    /// Blocks until every running worker has reported back, or `timeout` passes, applying the
    /// results. Used by the CLI and tests, which have no event loop.
    pub fn wait_for_workers(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.workers_running > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.worker_receiver.recv_timeout(remaining) {
                Ok(message) => self.apply_worker_message(message),
                Err(_) => return false,
            }
        }
        true
    }

    fn apply_worker_message(&mut self, message: WorkerMessage) {
        self.workers_running = self.workers_running.saturating_sub(1);
        match message {
            WorkerMessage::Indexed {
                roots,
                state,
                files,
            } => {
                if roots != self.roots {
                    return; // A newer workspace replaced the one this worker indexed.
                }
                self.workspace = *state;
                // Open documents are newer than the files the worker read.
                let open: Vec<_> = self
                    .documents
                    .values()
                    .filter(|document| !document.untitled)
                    .map(|document| (document.path.clone(), document.parsed.clone()))
                    .collect();
                for (path, parsed) in open {
                    self.workspace.replace_document(path, parsed);
                }
                self.emit(EngineEvent::WorkspaceIndexed { roots, files });
                self.refresh_open_diagnostics(None);
            }
            WorkerMessage::Search { id, results } => self.search_finished(id, results),
            WorkerMessage::IndexFailed { error } => {
                self.emit(EngineEvent::CommandFailed {
                    command: crate::commands::ids::WORKSPACE_REINDEX,
                    error,
                });
            }
        }
    }

    fn apply_file_changes(&mut self, now: Instant) -> usize {
        let Some(watcher) = self.watcher.as_mut() else {
            return 0;
        };
        while let Ok(event) = watcher.events.try_recv() {
            let Ok(event) = event else { continue };
            if matches!(event.kind, EventKind::Access(_)) {
                continue;
            }
            for path in event.paths {
                if is_ignored(&path, &self.settings.workspace.ignore) {
                    continue;
                }
                let change = match event.kind {
                    EventKind::Create(CreateKind::Folder)
                    | EventKind::Remove(RemoveKind::Folder) => Change::Directory,
                    _ if path.is_dir() => Change::Directory,
                    _ => Change::File,
                };
                watcher.pending.insert(normalize_path(&path), (now, change));
            }
        }
        let ready: Vec<(PathBuf, Change)> = watcher
            .pending
            .iter()
            .filter(|(_, (seen, _))| now.saturating_duration_since(*seen) >= WATCH_DEBOUNCE)
            .map(|(path, (_, change))| (path.clone(), *change))
            .collect();
        for (path, _) in &ready {
            watcher.pending.remove(path);
        }

        let mut reindex = false;
        for (path, change) in &ready {
            if *change == Change::Directory {
                reindex = true;
            } else if path.file_name().is_some_and(|name| name == "antora.yml") {
                self.reload_descriptor(path);
            } else if path
                .file_name()
                .is_some_and(|name| name == adoc_antora::PLAYBOOK_FILE)
            {
                // Playbook attributes apply to every page: index everything again.
                reindex = true;
            } else if is_asciidoc_path(path) {
                self.file_changed_on_disk(path);
            }
        }
        if reindex {
            self.reindex();
        }
        ready.len()
    }

    /// Re-reads one component descriptor and refreshes the diagnostics of open documents.
    pub(crate) fn reload_descriptor(&mut self, descriptor: &Path) {
        // A failure is recorded in the workspace's descriptor issues and reported as a diagnostic.
        let _ = self.workspace.reload_descriptor(descriptor);
        self.emit(EngineEvent::CatalogChanged {
            descriptor: descriptor.to_path_buf(),
        });
        self.refresh_open_diagnostics(None);
    }

    fn file_changed_on_disk(&mut self, path: &Path) {
        let on_disk = fs::read_to_string(path).ok();
        if let Some(id) = self.document_for_path(path) {
            let Some(document) = self.documents.get(&id) else {
                return;
            };
            let Some(text) = on_disk else {
                return; // Deleted underneath an open document: keep the buffer.
            };
            if document.text == text.as_str() {
                return; // Our own save, or an identical write.
            }
            if document.dirty {
                self.emit(EngineEvent::DocumentChangedOnDisk(id));
                return;
            }
            let whole = adoc_core::SourceRange::new(0, document.text.len());
            let group = self.new_group();
            if self
                .apply_edits(
                    id,
                    vec![TextEdit::new(whole, text.clone())],
                    crate::engine::UndoMode::Group(group),
                )
                .is_ok()
            {
                self.mark_saved(id, &text);
            }
            return;
        }
        match on_disk {
            Some(text) => {
                self.workspace.index.index_source(path, &text);
            }
            None => self.workspace.remove_document(path),
        }
        self.emit(EngineEvent::DiagnosticsUpdated {
            path: path.to_path_buf(),
        });
        self.refresh_open_diagnostics(None);
    }
}

fn is_ignored(path: &Path, ignore: &[String]) -> bool {
    path.components().any(|component| {
        let component = component.as_os_str().to_string_lossy();
        ignore.iter().any(|ignored| *ignored == component)
    })
}
