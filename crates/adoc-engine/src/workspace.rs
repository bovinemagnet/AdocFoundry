//! Workspace roots, background indexing and the file watcher.
//!
//! Indexing runs on a worker thread that hands its finished [`WorkspaceState`] back through a
//! channel, placing the file watcher's watches as it walks; file-system events are collected
//! from `notify` and debounced. Both are applied only by [`Engine::pump`], on the thread that
//! owns the engine.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex, MutexGuard, PoisonError,
    },
    time::{Duration, Instant},
};

use adoc_index::{is_asciidoc_path, normalize_path};
use adoc_lang::WorkspaceState;
use notify::{event::CreateKind, event::RemoveKind, EventKind, RecursiveMode, Watcher};

use crate::{quick_open::QuickOpenCache, Engine, EngineError, EngineEvent, TextEdit};

/// File-system events closer together than this for one path are applied once.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);

pub(crate) enum WorkerMessage {
    ProblemsComputed {
        generation: u64,
        problems: std::collections::BTreeMap<PathBuf, Vec<crate::Problem>>,
    },
    DirectoryListed {
        request: u64,
        dir: PathBuf,
        listing: crate::Listing,
    },
    Indexed {
        generation: u64,
        roots: Vec<PathBuf>,
        state: Box<WorkspaceState>,
        /// Quick Open's items, built from `state`.
        quick_open: Box<QuickOpenCache>,
        files: usize,
        /// The folders and files that could not be read.
        unreadable: Vec<PathBuf>,
        /// Watching folder by folder, the folders the index entered.
        entered: Option<BTreeSet<PathBuf>>,
    },
    IndexFailed {
        generation: u64,
        error: String,
    },
    /// A batch of a search's matches; only the last, `done`, ends its worker.
    Search {
        id: crate::SearchId,
        results: Vec<crate::SearchMatch>,
        done: bool,
        limited: bool,
    },
    Saved {
        ticket: u64,
        id: crate::DocumentId,
        version: u64,
        background: bool,
        result: Result<crate::writer::Written, crate::writer::SaveFailure>,
    },
    /// A state file or recovery record written or removed.
    Written,
    PreviewParsed {
        id: crate::DocumentId,
        generation: u64,
        version: u64,
        dom: Box<adoc_dom::Dom>,
        sources: std::collections::BTreeSet<PathBuf>,
    },
    DiagramRendered {
        checksum: String,
        result: Result<PathBuf, String>,
    },
    WordsCounted {
        id: crate::DocumentId,
        version: u64,
        count: crate::WordCount,
    },
    SpellingChecked {
        id: crate::DocumentId,
        version: u64,
        generation: u64,
        misspellings: Vec<crate::Misspelling>,
    },
    #[cfg(feature = "git")]
    GitDiscovered {
        generation: u64,
        found: Vec<(crate::GitRepository, Vec<adoc_git::FileStatus>)>,
        errors: Vec<String>,
    },
    #[cfg(feature = "git")]
    GitRefreshed {
        generation: u64,
        workdir: PathBuf,
        refresh: u64,
        result: Result<crate::git::Snapshot, String>,
    },
    #[cfg(feature = "git")]
    GitHunks {
        workdir: PathBuf,
        generation: u64,
        results: Vec<crate::git::HunkResult>,
    },
    #[cfg(feature = "git")]
    BranchVersions {
        generation: u64,
        versions: Option<adoc_antora::BranchVersions>,
    },
    #[cfg(feature = "git")]
    GitBlame {
        id: crate::DocumentId,
        generation: u64,
        blame: Result<Vec<adoc_git::BlameLine>, String>,
    },
    #[cfg(feature = "git")]
    Pushed {
        workdir: PathBuf,
        result: Result<String, String>,
    },
    HtmlExported {
        output: PathBuf,
        result: Result<(), String>,
    },
    #[cfg(feature = "typst")]
    ExportParsed {
        job: u64,
        doms: Vec<(PathBuf, adoc_dom::Dom)>,
    },
    #[cfg(feature = "typst")]
    ExportFinished {
        job: u64,
        result: Result<crate::ExportSummary, String>,
    },
    #[cfg(feature = "typst")]
    PdfPreviewTypeset {
        preview: Box<crate::PdfPreview>,
    },
    /// A worker whose job has no failure of its own panicked, saying `error`.
    Crashed {
        job: crate::worker::Crashed,
        error: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Change {
    File,
    Directory,
}

/// How a workspace's roots are watched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WatchMode {
    /// One recursive watch for each root, which FSEvents and `ReadDirectoryChangesW` keep
    /// without walking the tree.
    Roots,
    /// One watch for each folder the index enters. inotify watches a single folder, so a
    /// recursive watch would walk and watch every folder beneath the root, ignored or not.
    Folders,
}

impl WatchMode {
    pub(crate) const PLATFORM: Self = if cfg!(target_os = "linux") {
        Self::Folders
    } else {
        Self::Roots
    };
}

/// The watches in place, shared with the index worker, which adds them as it walks.
pub(crate) struct Watches {
    watcher: notify::RecommendedWatcher,
    mode: WatchMode,
    /// The roots watched, or the folders.
    watched: BTreeSet<PathBuf>,
    /// The first watch that failed, until it is reported.
    error: Option<String>,
    /// The system's limit on watches was reached: no more are tried.
    full: bool,
}

impl Watches {
    /// Watches `path`; one already watched is left alone.
    fn watch(&mut self, path: &Path, recursive: RecursiveMode) {
        if self.full || self.watched.contains(path) {
            return;
        }
        match self.watcher.watch(path, recursive) {
            Ok(()) => {
                self.watched.insert(path.to_path_buf());
            }
            // Gone since it was listed: nothing to watch.
            Err(error) if matches!(error.kind, notify::ErrorKind::PathNotFound) => {}
            Err(error) => {
                self.full = matches!(error.kind, notify::ErrorKind::MaxFilesWatch);
                self.error
                    .get_or_insert_with(|| format!("{}: {error}", path.display()));
            }
        }
    }

    /// Stops watching the folders an index no longer entered: removed, or ignored now.
    fn keep_only(&mut self, entered: &BTreeSet<PathBuf>) {
        let gone: Vec<PathBuf> = self.watched.difference(entered).cloned().collect();
        for folder in gone {
            // The system drops the watch of a removed folder itself.
            let _ = self.watcher.unwatch(&folder);
            self.watched.remove(&folder);
        }
    }
}

fn lock(watches: &Mutex<Watches>) -> MutexGuard<'_, Watches> {
    watches.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct FileWatcher {
    watches: Arc<Mutex<Watches>>,
    events: Receiver<notify::Result<notify::Event>>,
    pending: BTreeMap<PathBuf, (Instant, Change)>,
    // `(resolved, as opened)` for each root reached through a symlink. Some back ends (FSEvents
    // on macOS) report resolved paths, which would match nothing the engine holds.
    aliases: Vec<(PathBuf, PathBuf)>,
}

impl FileWatcher {
    /// A watcher watching nothing yet: the index worker places the watches as it walks, so
    /// nothing here walks the roots. Links are never followed.
    fn start(roots: &[PathBuf], mode: WatchMode) -> notify::Result<Self> {
        let (sender, events) = mpsc::channel();
        let watcher = notify::RecommendedWatcher::new(
            move |event| {
                let _ = sender.send(event);
            },
            notify::Config::default().with_follow_symlinks(false),
        )?;
        let mut aliases = Vec::new();
        for root in roots {
            // Roots exist (checked by `open_workspace`), so resolving them is safe.
            if let Ok(resolved) = fs::canonicalize(root) {
                if resolved != *root {
                    aliases.push((resolved, root.clone()));
                }
            }
        }
        Ok(Self {
            watches: Arc::new(Mutex::new(Watches {
                watcher,
                mode,
                watched: BTreeSet::new(),
                error: None,
                full: false,
            })),
            events,
            pending: BTreeMap::new(),
            aliases,
        })
    }

    /// Also watches a repository's `.git` directory for the files that change on a commit, a
    /// branch switch or staging. Watching is a convenience: a failure is ignored.
    #[cfg(feature = "git")]
    pub(crate) fn watch_git_dir(&mut self, git_dir: &Path) {
        let mut watches = lock(&self.watches);
        let _ = watches.watcher.watch(git_dir, RecursiveMode::NonRecursive);
        let refs = git_dir.join("refs");
        if refs.is_dir() {
            let _ = watches.watcher.watch(&refs, RecursiveMode::Recursive);
        }
    }

    /// The folders watched one by one, for tests.
    #[cfg(test)]
    pub(crate) fn watched(&self) -> BTreeSet<PathBuf> {
        lock(&self.watches).watched.clone()
    }
}

impl Engine {
    /// Sets the workspace roots, loads the workspace settings layer from the first root, and
    /// starts indexing on a worker thread, which places the watches for changes as it walks.
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
        self.open_expansion();
        self.files.clear();
        self.watcher = match FileWatcher::start(&self.roots, self.watch_mode) {
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
        self.record_recent_folders();
        self.reindex();
        self.discover_git();
        Ok(())
    }

    /// The roots of the open workspace.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Re-indexes every root on a worker thread, watching whatever the walk enters that is not
    /// watched yet.
    pub fn reindex(&mut self) {
        let roots = self.roots.clone();
        let ignore = self.settings.workspace.ignore.clone();
        let watches = self
            .watcher
            .as_ref()
            .map(|watcher| Arc::clone(&watcher.watches));
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        self.indexing += 1;
        self.index_generation += 1;
        let generation = self.index_generation;
        let crashed = move |error| WorkerMessage::IndexFailed {
            generation,
            error: format!("indexing crashed: {error}"),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let mode = watches.as_deref().map(|watches| lock(watches).mode);
            if let (Some(watches), Some(WatchMode::Roots)) = (&watches, mode) {
                let mut watches = lock(watches);
                for root in &roots {
                    watches.watch(root, RecursiveMode::Recursive);
                }
            }
            let mut entered = (mode == Some(WatchMode::Folders)).then(BTreeSet::new);
            let mut state = WorkspaceState::new();
            let result = state.index_roots_with(&roots, &ignore, |folder| {
                if let (Some(watches), Some(entered)) = (&watches, entered.as_mut()) {
                    lock(watches).watch(folder, RecursiveMode::NonRecursive);
                    entered.insert(folder.to_path_buf());
                }
            });
            let message = match result {
                Ok(report) => WorkerMessage::Indexed {
                    generation,
                    quick_open: Box::new(QuickOpenCache::new(&state, &roots)),
                    roots,
                    state: Box::new(state),
                    files: report.files,
                    unreadable: report.unreadable,
                    entered,
                },
                Err(error) => WorkerMessage::IndexFailed {
                    generation,
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
            || !self.pending_index.is_empty()
            || self.rendered.has_scheduled()
            || self.visual.has_due(self.active)
            || self.git.has_pending()
            || self.has_due_word_count()
            || self.has_due_spelling()
            || self.has_due_auto_save()
            || self.has_due_recovery()
            || self.has_due_html_export()
            || self.has_due_problems()
            || self
                .watcher
                .as_ref()
                .is_some_and(|watcher| !watcher.pending.is_empty())
    }

    /// True while the workspace is being indexed: a whole-workspace index is running, or
    /// file-system changes wait to be applied. Unlike [`Engine::has_pending`], the re-parse,
    /// preview and model settles that follow typing do not count.
    #[must_use]
    pub fn is_indexing(&self) -> bool {
        self.indexing > 0
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
        let due = self.clock.now();
        applied
            + self.index_pending(Some(due))
            + self.apply_file_changes(Instant::now())
            + self.start_due_previews()
            + self.rebuild_due_visuals()
            + self.start_due_hunks()
            + self.save_due_expansion()
            + self.start_due_word_count()
            + self.start_due_spelling()
            + self.save_due_documents()
            + self.write_due_recovery()
            + self.write_due_html_exports()
            + self.update_due_problems()
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

    pub(crate) fn apply_worker_message(&mut self, message: WorkerMessage) {
        if !matches!(message, WorkerMessage::Search { done: false, .. }) {
            self.workers_running = self.workers_running.saturating_sub(1);
        }
        match message {
            WorkerMessage::ProblemsComputed {
                generation,
                problems,
            } => self.problems_computed(generation, problems),
            WorkerMessage::DirectoryListed {
                request,
                dir,
                listing,
            } => self.directory_listed(request, dir, listing),
            WorkerMessage::Indexed {
                generation,
                roots,
                state,
                quick_open,
                files,
                unreadable,
                entered,
            } => {
                self.indexing = self.indexing.saturating_sub(1);
                if generation != self.index_generation {
                    return; // A newer index, of these roots or others, replaced this one.
                }
                self.watched(entered.as_ref());
                if let Some(first) = unreadable.first() {
                    let more = match unreadable.len() - 1 {
                        0 => String::new(),
                        1 => " and 1 more".to_owned(),
                        more => format!(" and {more} more"),
                    };
                    self.emit(EngineEvent::CommandFailed {
                        command: crate::commands::ids::WORKSPACE_REINDEX,
                        error: format!(
                            "could not read {}{more}; the rest of the workspace is indexed",
                            first.display()
                        ),
                    });
                }
                self.workspace = *state;
                *self.quick_open.borrow_mut() = *quick_open;
                self.restore_branch_versions();
                // Open documents are newer than the files the worker read.
                let open: Vec<_> = self
                    .documents
                    .values()
                    .filter(|document| document.indexed())
                    .map(|document| (document.path.clone(), document.parsed().clone()))
                    .collect();
                for (path, parsed) in open {
                    self.workspace.replace_document(path, parsed);
                }
                self.emit(EngineEvent::WorkspaceIndexed { roots, files });
                self.refresh_open_diagnostics(None);
                // The playbook may have changed with the files.
                #[cfg(feature = "git")]
                if !self.git.repositories().is_empty() {
                    self.refresh_branch_versions();
                }
                self.preview_workspace_changed();
                self.visual_workspace_changed();
            }
            WorkerMessage::Search {
                id,
                results,
                done,
                limited,
            } => self.search_found(id, results, done, limited),
            WorkerMessage::Saved {
                id,
                version,
                background,
                result,
                ..
            } => self.save_finished(id, version, background, result),
            WorkerMessage::Written => {}
            WorkerMessage::PreviewParsed {
                id,
                generation,
                version,
                dom,
                sources,
            } => self.preview_parsed(id, generation, version, *dom, sources),
            WorkerMessage::DiagramRendered { checksum, result } => {
                self.diagram_rendered(&checksum, result);
            }
            WorkerMessage::WordsCounted { id, version, count } => {
                self.words_counted(id, version, count);
            }
            WorkerMessage::SpellingChecked {
                id,
                version,
                generation,
                misspellings,
            } => self.spelling_checked(id, version, generation, misspellings),
            #[cfg(feature = "git")]
            WorkerMessage::GitDiscovered {
                generation,
                found,
                errors,
            } => self.git_discovered(generation, found, errors),
            #[cfg(feature = "git")]
            WorkerMessage::GitRefreshed {
                generation,
                workdir,
                refresh,
                result,
            } => self.git_refreshed(generation, &workdir, refresh, result),
            #[cfg(feature = "git")]
            WorkerMessage::GitHunks {
                workdir,
                generation,
                results,
            } => self.git_hunks(&workdir, generation, results),
            #[cfg(feature = "git")]
            WorkerMessage::BranchVersions {
                generation,
                versions,
            } => self.branch_versions_found(generation, versions),
            #[cfg(feature = "git")]
            WorkerMessage::GitBlame {
                id,
                generation,
                blame,
            } => self.git_blamed(id, generation, blame),
            #[cfg(feature = "git")]
            WorkerMessage::Pushed { workdir, result } => self.pushed(workdir, result),
            WorkerMessage::HtmlExported { output, result } => self.html_exported(output, result),
            #[cfg(feature = "typst")]
            WorkerMessage::ExportParsed { job, doms } => self.export_parsed(job, doms),
            #[cfg(feature = "typst")]
            WorkerMessage::ExportFinished { job, result } => self.export_finished(job, result),
            #[cfg(feature = "typst")]
            WorkerMessage::PdfPreviewTypeset { preview } => self.pdf_preview_typeset(*preview),
            WorkerMessage::Crashed { job, error } => self.worker_crashed(job, error),
            WorkerMessage::IndexFailed { generation, error } => {
                self.indexing = self.indexing.saturating_sub(1);
                if generation != self.index_generation {
                    return;
                }
                self.emit(EngineEvent::CommandFailed {
                    command: crate::commands::ids::WORKSPACE_REINDEX,
                    error,
                });
            }
        }
    }

    /// Settles the watches after an index: folders it no longer entered are let go, and a watch
    /// that failed is reported once.
    fn watched(&mut self, entered: Option<&BTreeSet<PathBuf>>) {
        let Some(watcher) = self.watcher.as_ref() else {
            return;
        };
        let error = {
            let mut watches = lock(&watcher.watches);
            if let Some(entered) = entered {
                watches.keep_only(entered);
            }
            watches.error.take()
        };
        if let Some(error) = error {
            // Watching is a convenience; the workspace still works without it.
            self.emit(EngineEvent::CommandFailed {
                command: crate::commands::ids::WORKSPACE_OPEN_FOLDER,
                error: format!("file watching is incomplete: {error}"),
            });
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
                let path = as_opened(&path, &watcher.aliases);
                // `.git` is ignored, but its `HEAD`, `index` and `refs` show branch switches,
                // staging and commits.
                if is_ignored(&path, &self.settings.workspace.ignore)
                    && !self.git.is_metadata_path(&path)
                {
                    continue;
                }
                let change = match event.kind {
                    EventKind::Create(CreateKind::Folder)
                    | EventKind::Remove(RemoveKind::Folder) => Change::Directory,
                    _ if path.is_dir() => Change::Directory,
                    _ => Change::File,
                };
                // A folder removed and then touched is still a folder that changed.
                let pending = watcher
                    .pending
                    .entry(normalize_path(&path))
                    .or_insert((now, change));
                pending.0 = now;
                if change == Change::Directory {
                    pending.1 = Change::Directory;
                }
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
        let changed: Vec<PathBuf> = ready.iter().map(|(path, _)| path.clone()).collect();
        let metadata = self.git_paths_changed(&changed);
        let ready: Vec<(PathBuf, Change)> = ready
            .into_iter()
            .filter(|(path, _)| !metadata.contains(path))
            .collect();

        let listed: Vec<PathBuf> = ready.iter().map(|(path, _)| path.clone()).collect();
        self.directories_changed(&listed);
        self.images_changed(&listed);
        self.word_lists_changed(&listed);
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
        self.preview_workspace_changed();
        self.visual_workspace_changed();
    }

    fn file_changed_on_disk(&mut self, path: &Path) {
        let on_disk = fs::read_to_string(path).ok();
        if let Some(id) = self.document_for_path(path) {
            if self.writer.is_saving(id) {
                return; // Our own write, which the save's result settles.
            }
            let Some(document) = self.documents.get(&id) else {
                return;
            };
            let Some(text) = on_disk else {
                return; // Deleted underneath an open document: keep the buffer.
            };
            if document.text == text.as_str() {
                return; // Our own save, or an identical write.
            }
            let Some(state) = self.states.get_mut(&id) else {
                return;
            };
            // What was last saved or read: our own write, though typing went on since.
            if text.len() == state.saved_len
                && crate::document::hash_text(&text) == state.saved_hash
            {
                return;
            }
            if document.dirty {
                if !state.changed_on_disk {
                    state.changed_on_disk = true;
                    self.emit(EngineEvent::DocumentChangedOnDisk(id));
                }
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
                self.workspace.index.index_file(path, &text);
            }
            None => self.workspace.remove_document(path),
        }
        self.preview_source_changed(path);
        self.visual_source_changed(path);
        self.emit(EngineEvent::DiagnosticsUpdated {
            path: path.to_path_buf(),
        });
        self.refresh_open_diagnostics(None);
    }
}

/// Maps a path reported under a resolved root back to the root as it was opened.
fn as_opened(path: &Path, aliases: &[(PathBuf, PathBuf)]) -> PathBuf {
    aliases
        .iter()
        .find_map(|(resolved, opened)| {
            path.strip_prefix(resolved)
                .ok()
                .map(|rest| opened.join(rest))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

fn is_ignored(path: &Path, ignore: &[String]) -> bool {
    path.components().any(|component| {
        let component = component.as_os_str().to_string_lossy();
        ignore.iter().any(|ignored| *ignored == component)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_paths_map_back_to_the_root_as_opened() {
        let aliases = vec![(
            PathBuf::from("/private/var/docs"),
            PathBuf::from("/var/docs"),
        )];
        assert_eq!(
            as_opened(Path::new("/private/var/docs/modules/page.adoc"), &aliases),
            PathBuf::from("/var/docs/modules/page.adoc")
        );
        assert_eq!(
            as_opened(Path::new("/elsewhere/page.adoc"), &aliases),
            PathBuf::from("/elsewhere/page.adoc")
        );
    }

    /// Indexes `roots` as a worker would, as the result of reindex `generation`.
    fn indexed(roots: &[PathBuf], generation: u64) -> WorkerMessage {
        let mut state = WorkspaceState::new();
        let files = state.index_roots(roots).unwrap();
        WorkerMessage::Indexed {
            generation,
            quick_open: Box::default(),
            roots: roots.to_vec(),
            state: Box::new(state),
            files,
            unreadable: Vec::new(),
            entered: None,
        }
    }

    #[test]
    fn a_superseded_index_result_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_path_buf()];
        fs::write(dir.path().join("old.adoc"), "= Old\n").unwrap();
        let older = indexed(&roots, 1);
        fs::write(dir.path().join("new.adoc"), "= New\n").unwrap();
        let newer = indexed(&roots, 2);

        let mut engine = Engine::new(crate::Settings::default().with_git_opt_in());
        engine.roots.clone_from(&roots);
        // Two reindexes started; the newer finishes first.
        engine.index_generation = 2;
        engine.workers_running = 2;
        engine.indexing = 2;
        engine.worker_sender.send(newer).unwrap();
        engine.worker_sender.send(older).unwrap();
        engine.pump();

        assert!(!engine.is_indexing());
        assert!(
            engine
                .workspace
                .index
                .file(&dir.path().join("new.adoc"))
                .is_some(),
            "the newer result is kept when the older finishes last"
        );
    }

    /// Pumps `engine` until `done` holds or ten seconds pass.
    fn pump_until(engine: &mut Engine, mut done: impl FnMut(&Engine) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            engine.pump();
            if done(engine) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn watching_folder_by_folder_skips_ignored_folders_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("pages/nested")).unwrap();
        fs::create_dir_all(root.join("node_modules/package")).unwrap();
        fs::write(root.join("pages/page.adoc"), "= Page\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("pages"), root.join("link")).unwrap();

        let mut engine = Engine::new(crate::Settings::default().with_git_opt_in());
        engine.watch_mode = WatchMode::Folders;
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        let watched = |engine: &Engine| engine.watcher.as_ref().unwrap().watched();
        assert_eq!(
            watched(&engine),
            BTreeSet::from([root.clone(), root.join("pages"), root.join("pages/nested")]),
            "neither node_modules nor the link is watched"
        );

        // A new folder is watched once the index enters it, and pages in it are reported.
        fs::create_dir(root.join("later")).unwrap();
        assert!(pump_until(&mut engine, |engine| watched(engine)
            .contains(&root.join("later"))));
        let page = root.join("later/page.adoc");
        fs::write(&page, "= Later\n").unwrap();
        assert!(pump_until(&mut engine, |engine| engine
            .workspace
            .index
            .file(&page)
            .is_some()));

        // A removed folder is let go. Windows reports a removal without saying it was of a
        // folder, and there each root is watched whole, never folder by folder.
        #[cfg(not(windows))]
        {
            fs::remove_dir(root.join("pages/nested")).unwrap();
            assert!(pump_until(&mut engine, |engine| !watched(engine)
                .contains(&root.join("pages/nested"))));
        }
    }

    #[test]
    fn a_superseded_index_failure_is_not_reported() {
        let mut engine = Engine::new(crate::Settings::default().with_git_opt_in());
        let events = engine.subscribe();
        engine.index_generation = 2;
        engine.workers_running = 1;
        engine.indexing = 1;
        engine
            .worker_sender
            .send(WorkerMessage::IndexFailed {
                generation: 1,
                error: "indexing failed: gone".to_owned(),
            })
            .unwrap();
        engine.pump();

        assert!(!engine.is_indexing());
        assert!(events.try_iter().next().is_none());
    }
}
