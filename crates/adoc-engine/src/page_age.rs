//! How long ago each page last changed (#289): for each repository, one walk of `HEAD`'s
//! history on a worker whenever `HEAD`'s commit moves (`adoc_git::Repository::last_changed`,
//! over the AsciiDoc files of `HEAD`'s tree), kept until it moves again. A file with changes
//! not committed, on disk or in an unsaved buffer, changed now. Without Git there are no ages.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use adoc_git::{Change, Repository, HISTORY_LIMIT};
use adoc_graph::LastChanged;
use adoc_index::{is_asciidoc_path, normalize_path};

use crate::{workspace::WorkerMessage, Engine};

/// The ages of one repository's files at one commit.
#[derive(Debug, Default)]
pub(crate) struct Ages {
    commit: Option<String>,
    /// When each AsciiDoc file of `HEAD` last changed, by absolute path; a file older than all
    /// the history read has the oldest commit's time.
    changed: HashMap<PathBuf, i64>,
}

#[derive(Debug, Default)]
pub(crate) struct PageAges {
    /// By working tree.
    ages: BTreeMap<PathBuf, Ages>,
    /// The latest computation started for each working tree; older results are dropped.
    running: BTreeMap<PathBuf, u64>,
    next: u64,
}

/// The files and times one computation found.
pub(crate) type FoundAges = Result<HashMap<PathBuf, i64>, String>;

impl Engine {
    /// When the file at `path` last changed, if it is in a repository whose history has been
    /// read: now when it has changes not committed.
    #[must_use]
    pub fn page_age(&self, path: &Path) -> Option<LastChanged> {
        let path = normalize_path(path);
        let repository = self.git.repository_for(&path)?;
        let ages = self.git.page_ages.ages.get(&repository.workdir)?;
        let changed_on_disk = self.git.status_for(&path).is_some_and(|status| {
            status.index != Change::Unmodified || status.worktree != Change::Unmodified
        });
        let edited = self
            .documents
            .values()
            .any(|document| document.dirty && normalize_path(&document.path) == path);
        if changed_on_disk || edited {
            return Some(LastChanged::Uncommitted);
        }
        ages.changed.get(&path).map(|&time| LastChanged::At(time))
    }

    /// Whether any repository's history has been read, so pages have ages.
    #[must_use]
    pub fn has_page_ages(&self) -> bool {
        !self.git.page_ages.ages.is_empty()
    }

    /// Reads the history of the repository at `workdir`, its `HEAD` at `commit`, on a worker,
    /// unless its ages are of that commit already.
    pub(crate) fn start_page_ages(&mut self, workdir: &Path, commit: Option<String>) {
        let known = &self.git.page_ages;
        if commit.is_none()
            || known
                .ages
                .get(workdir)
                .is_some_and(|ages| ages.commit == commit)
        {
            return;
        }
        self.git.page_ages.next += 1;
        let run = self.git.page_ages.next;
        self.git
            .page_ages
            .running
            .insert(workdir.to_path_buf(), run);
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let (workdir, failed) = (workdir.to_path_buf(), workdir.to_path_buf());
        let (commit, failed_commit) = (commit.clone(), commit);
        let crashed = move |error| WorkerMessage::PageAges {
            workdir: failed,
            run,
            commit: failed_commit,
            found: Err(format!("reading the history crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let found = read_ages(&workdir);
            let _ = sender.send(WorkerMessage::PageAges {
                workdir,
                run,
                commit,
                found,
            });
        });
    }

    pub(crate) fn page_ages_read(
        &mut self,
        workdir: PathBuf,
        run: u64,
        commit: Option<String>,
        found: FoundAges,
    ) {
        if self.git.page_ages.running.get(&workdir) != Some(&run) {
            return;
        }
        self.git.page_ages.running.remove(&workdir);
        // A failure leaves the pages with no age rather than stopping anything else.
        let Ok(changed) = found else {
            return;
        };
        self.git
            .page_ages
            .ages
            .insert(workdir, Ages { commit, changed });
        self.graph_ages_changed();
    }

    /// The repositories are being looked for again: their ages go with them.
    pub(crate) fn forget_page_ages(&mut self) {
        let had = !self.git.page_ages.ages.is_empty();
        self.git.page_ages.ages.clear();
        self.git.page_ages.running.clear();
        if had {
            self.graph_ages_changed();
        }
    }

    /// What is committed or not changed: ages that were read show it again.
    pub(crate) fn page_status_changed(&mut self) {
        if self.has_page_ages() {
            self.graph_ages_changed();
        }
    }
}

/// When each AsciiDoc file of `HEAD` in the repository at `workdir` last changed.
fn read_ages(workdir: &Path) -> FoundAges {
    let repo = Repository::discover(workdir)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{} is no longer a repository", workdir.display()))?;
    let head = repo
        .resolve_revision("HEAD")
        .map_err(|error| error.to_string())?;
    let files: Vec<PathBuf> = repo
        .tree_files(&head)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|path| is_asciidoc_path(path))
        .collect();
    let found = repo
        .last_changed(&files, HISTORY_LIMIT)
        .map_err(|error| error.to_string())?;
    let root = normalize_path(repo.workdir());
    Ok(files
        .iter()
        .filter_map(|file| {
            let time = found
                .changes
                .get(file)
                .map(|change| change.time)
                .or(found.cut_short)?;
            Some((normalize_path(&root.join(file)), time))
        })
        .collect())
}
