//! `[editor] auto_save`: pages saved through `document.save` once typing stops, or when the
//! page or the window loses focus.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use crate::{commands::ids, AutoSave, CommandArgs, DocumentId, Engine};

/// How long a page must go unedited before `after_delay` saves it.
pub const AUTO_SAVE_DELAY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(crate) struct AutoSaveState {
    /// When each page waiting to be saved was last edited.
    edited: BTreeMap<DocumentId, Instant>,
    /// Pages whose save was refused (changed on disk, or unwritable): reported once, and left
    /// alone until saved, overwritten or reloaded by hand.
    refused: BTreeSet<DocumentId>,
}

impl Engine {
    /// Tells the engine the window lost focus: with `on_focus_change`, every edited page is
    /// saved.
    pub fn focus_lost(&mut self) {
        if self.settings.editor.auto_save != AutoSave::OnFocusChange {
            return;
        }
        let ids: Vec<DocumentId> = self.documents.keys().copied().collect();
        for id in ids {
            self.auto_save(id);
        }
    }

    /// Notes an edit of `id`, which `after_delay` saves once typing stops.
    pub(crate) fn auto_save_edited(&mut self, id: DocumentId) {
        if self.settings.editor.auto_save == AutoSave::AfterDelay
            && !self.auto_saves.refused.contains(&id)
            && self
                .documents
                .get(&id)
                .is_some_and(|document| !document.untitled)
        {
            self.auto_saves.edited.insert(id, self.clock.now());
        }
    }

    /// With `on_focus_change`, saves the page whose tab was left.
    pub(crate) fn auto_save_left(&mut self, id: DocumentId) {
        if self.settings.editor.auto_save == AutoSave::OnFocusChange {
            self.auto_save(id);
        }
    }

    /// True while an edited page waits for `after_delay`.
    pub(crate) fn has_due_auto_save(&self) -> bool {
        !self.auto_saves.edited.is_empty()
    }

    /// Saves every page left unedited for [`AUTO_SAVE_DELAY`]. Returns the number saved or
    /// refused.
    pub(crate) fn save_due_documents(&mut self) -> usize {
        if self.settings.editor.auto_save != AutoSave::AfterDelay {
            self.auto_saves.edited.clear();
            return 0;
        }
        let now = self.clock.now();
        let due: Vec<DocumentId> = self
            .auto_saves
            .edited
            .iter()
            .filter(|(_, edited)| now >= **edited + AUTO_SAVE_DELAY)
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            self.auto_saves.edited.remove(id);
            self.auto_save(*id);
        }
        due.len()
    }

    /// The page's file was written or read back by hand, so a refused page may be saved again.
    pub(crate) fn auto_save_settled(&mut self, id: DocumentId) {
        self.auto_saves.refused.remove(&id);
    }

    pub(crate) fn forget_auto_save(&mut self, id: DocumentId) {
        self.auto_saves.edited.remove(&id);
        self.auto_saves.refused.remove(&id);
    }

    /// Saves `id` through `document.save` when it has unsaved edits and a file, and has not
    /// been refused already. A refusal is reported as the command's failure.
    fn auto_save(&mut self, id: DocumentId) {
        let wanted = self
            .documents
            .get(&id)
            .is_some_and(|document| document.dirty && !document.untitled);
        if !wanted || self.auto_saves.refused.contains(&id) {
            return;
        }
        if self
            .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
            .is_err()
        {
            self.auto_saves.refused.insert(id);
        }
    }
}
