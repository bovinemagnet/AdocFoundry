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
    DirectoryListed {
        request: u64,
        dir: PathBuf,
        listing: crate::Listing,
    },
    Indexed {
        generation: u64,
        roots: Vec<PathBuf>,
        state: Box<WorkspaceState>,
        files: usize,
    },
    IndexFailed {
        generation: u64,
        error: String,
    },
    Search {
        id: crate::SearchId,
        results: Vec<crate::SearchMatch>,
    },
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
        id: crate::DocumentId,
        version: u64,
        hunks: Result<Vec<adoc_git::Hunk>, String>,
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
    // `(resolved, as opened)` for each root reached through a symlink. Some back ends (FSEvents
    // on macOS) report resolved paths, which would match nothing the engine holds.
    aliases: Vec<(PathBuf, PathBuf)>,
}

impl FileWatcher {
    fn start(roots: &[PathBuf]) -> notify::Result<Self> {
        let (sender, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        let mut aliases = Vec::new();
        for root in roots {
            watcher.watch(root, RecursiveMode::Recursive)?;
            // Roots exist (checked by `open_workspace`), so resolving them is safe.
            if let Ok(resolved) = fs::canonicalize(root) {
                if resolved != *root {
                    aliases.push((resolved, root.clone()));
                }
            }
        }
        Ok(Self {
            _watcher: watcher,
            events,
            pending: BTreeMap::new(),
            aliases,
        })
    }

    /// Also watches a repository's `.git` directory for the files that change on a commit, a
    /// branch switch or staging. Watching is a convenience: a failure is ignored.
    #[cfg(feature = "git")]
    pub(crate) fn watch_git_dir(&mut self, git_dir: &Path) {
        let _ = self._watcher.watch(git_dir, RecursiveMode::NonRecursive);
        let refs = git_dir.join("refs");
        if refs.is_dir() {
            let _ = self._watcher.watch(&refs, RecursiveMode::Recursive);
        }
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
        self.open_expansion();
        self.files.clear();
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

    /// Re-indexes every root on a worker thread.
    pub fn reindex(&mut self) {
        let roots = self.roots.clone();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        self.indexing += 1;
        self.index_generation += 1;
        let generation = self.index_generation;
        thread::spawn(move || {
            let mut state = WorkspaceState::new();
            let message = match state.index_roots(&roots) {
                Ok(files) => WorkerMessage::Indexed {
                    generation,
                    roots,
                    state: Box::new(state),
                    files,
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
            || self.visual.has_due()
            || self.git.has_pending()
            || self.has_due_word_count()
            || self.has_due_spelling()
            || self.has_due_auto_save()
            || self.has_due_recovery()
            || self.has_due_html_export()
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
            WorkerMessage::DirectoryListed {
                request,
                dir,
                listing,
            } => self.directory_listed(request, dir, listing),
            WorkerMessage::Indexed {
                generation,
                roots,
                state,
                files,
            } => {
                self.indexing = self.indexing.saturating_sub(1);
                if generation != self.index_generation {
                    return; // A newer index, of these roots or others, replaced this one.
                }
                self.workspace = *state;
                self.restore_branch_versions();
                // Open documents are newer than the files the worker read.
                let open: Vec<_> = self
                    .documents
                    .values()
                    .filter(|document| document.indexed())
                    .map(|document| (document.path.clone(), document.parsed.clone()))
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
            WorkerMessage::Search { id, results } => self.search_finished(id, results),
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
            WorkerMessage::GitHunks { id, version, hunks } => self.git_hunks(id, version, hunks),
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
            roots: roots.to_vec(),
            state: Box::new(state),
            files,
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
