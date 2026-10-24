//! Git when the engine is built without the `git` feature: no repository is looked for and no
//! Git state or event is produced, as with `[git] enabled = false`.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use crate::{DocumentId, Engine};

// Braced, so it is built as `GitState::default()` like the real one.
#[derive(Default)]
pub(crate) struct GitState {}

impl GitState {
    pub(crate) fn has_pending(&self) -> bool {
        false
    }

    pub(crate) fn is_metadata_path(&self, _: &Path) -> bool {
        false
    }
}

impl Engine {
    pub(crate) fn discover_git(&mut self) {}

    pub(crate) fn git_paths_changed(&mut self, _: &[PathBuf]) -> BTreeSet<PathBuf> {
        BTreeSet::new()
    }

    pub(crate) fn schedule_hunks(&mut self, _: DocumentId, _: bool) {}

    pub(crate) fn forget_hunks(&mut self, _: DocumentId) {}

    pub(crate) fn forget_blame(&mut self, _: DocumentId) {}

    pub(crate) fn start_due_hunks(&mut self) -> usize {
        0
    }

    pub(crate) fn restore_branch_versions(&mut self) {}
}
