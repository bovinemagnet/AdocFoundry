//! Staged changes (MCP PRD §7.5): files to make, edits and moves, worked out and checked
//! against the workspace without touching it, then applied later, as one undo group, only while
//! every file still holds what the change was worked out against. A front end keeps the
//! [`ChangeSet`] (the MCP server keeps one per session); the engine keeps nothing until it is
//! applied.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use adoc_antora::{AntoraResource, ResourceFamily};
use adoc_core::Diagnostic;
use adoc_index::{is_asciidoc_path, normalize_path};
use ropey::Rope;
use sha2::{Digest as _, Sha256};

use crate::{
    edit, DocumentId, Engine, EngineError, FileEdits, FileMove, NewPage, TextEdit, UndoGroup,
};

/// `edits`, checked against `base`, and the text they leave.
fn edited(base: &str, edits: Vec<TextEdit>) -> Result<(Vec<TextEdit>, String), EngineError> {
    let mut rope = Rope::from_str(base);
    let edits = edit::validate(&rope, edits)?;
    edit::apply(&mut rope, &edits);
    Ok((edits, rope.to_string()))
}

/// A file a change makes, with its text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileCreate {
    pub path: PathBuf,
    pub text: String,
}

/// Files to make, edits to make to files as they are now, and files to move: a change worked
/// out but not made.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangeSet {
    pub creates: Vec<FileCreate>,
    pub edits: Vec<FileEdits>,
    pub moves: Vec<FileMove>,
}

impl ChangeSet {
    /// Adds `other` to this change. Edits to one file join (each worked out against the same
    /// text, which must not differ); a file made twice, or both made and edited, is refused.
    pub fn merge(&mut self, other: Self) -> Result<(), EngineError> {
        for create in other.creates {
            if self.touches(&create.path) {
                return Err(EngineError::InvalidRequest(format!(
                    "{} is changed twice",
                    create.path.display()
                )));
            }
            self.creates.push(create);
        }
        for file in other.edits {
            if self.creates.iter().any(|create| create.path == file.path) {
                return Err(EngineError::InvalidRequest(format!(
                    "{} is both made and edited",
                    file.path.display()
                )));
            }
            match self.edits.iter_mut().find(|known| known.path == file.path) {
                Some(known) if known.base == file.base => known.edits.extend(file.edits),
                Some(_) => {
                    return Err(EngineError::PreviewStale { path: file.path });
                }
                None => self.edits.push(file),
            }
        }
        for file_move in other.moves {
            if self.moves.iter().any(|known| known.from == file_move.from) {
                return Err(EngineError::InvalidRequest(format!(
                    "{} is moved twice",
                    file_move.from.display()
                )));
            }
            self.moves.push(file_move);
        }
        Ok(())
    }

    fn touches(&self, path: &Path) -> bool {
        self.creates.iter().any(|create| create.path == path)
            || self.edits.iter().any(|file| file.path == path)
            || self
                .moves
                .iter()
                .any(|file| file.from == path || file.to == path)
    }

    /// Every path the change reads or writes, sorted.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .creates
            .iter()
            .map(|create| create.path.clone())
            .chain(self.edits.iter().map(|file| file.path.clone()))
            .chain(
                self.moves
                    .iter()
                    .flat_map(|file| [file.from.clone(), file.to.clone()]),
            )
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// The bytes of text the change holds: what a front end keeping it pays.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.creates
            .iter()
            .map(|create| create.text.len())
            .chain(self.edits.iter().map(|file| {
                file.base.len()
                    + file
                        .edits
                        .iter()
                        .map(|edit| edit.replacement.len())
                        .sum::<usize>()
            }))
            .sum()
    }
}

/// One file as a staged change would leave it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedFile {
    pub path: PathBuf,
    /// Its text now; `None` for a file the change makes.
    pub before: Option<String>,
    /// Its text after; the same as before for a file only moved.
    pub after: String,
    /// Where a move takes it.
    pub moved_to: Option<PathBuf>,
    /// The diagnostics it would have after, at its new path for one moved.
    pub diagnostics: Vec<Diagnostic>,
}

/// The SHA-256 of `text`, in hex: what a staged change expects a file to hold.
#[must_use]
pub fn text_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Engine {
    /// The text of the file at `path` now: its buffer when open, else what is on disk.
    #[must_use]
    pub fn current_text(&self, path: &Path) -> Option<String> {
        let path = normalize_path(path);
        if let Some(document) = self
            .document_for_path(&path)
            .and_then(|id| self.document(id))
        {
            return Some(document.text_string());
        }
        self.file_access
            .permits(&path)
            .then(|| std::fs::read_to_string(&path).ok())
            .flatten()
    }

    /// A change making `edits` (ranges into its text now) to the file at `path`.
    pub fn stage_edits(&self, path: &Path, edits: Vec<TextEdit>) -> Result<ChangeSet, EngineError> {
        let path = normalize_path(path);
        let base = self
            .current_text(&path)
            .ok_or_else(|| EngineError::NotFound(format!("{} cannot be read", path.display())))?;
        let (edits, _) = edited(&base, edits)?;
        Ok(ChangeSet {
            edits: vec![FileEdits { path, edits, base }],
            ..ChangeSet::default()
        })
    }

    /// The change `page.new` would make for `request`, the new page and its navigation entry
    /// (at the navigation's end, as there is no active page to follow), made nowhere.
    pub fn stage_new_page(&self, request: &NewPage) -> Result<ChangeSet, EngineError> {
        let plan = self.plan_new_page(request, None)?;
        let mut changes = ChangeSet {
            creates: vec![FileCreate {
                path: plan.path,
                text: plan.text,
            }],
            ..ChangeSet::default()
        };
        if let Some((nav, edit)) = plan.nav {
            changes.merge(self.stage_edits(&nav, vec![edit])?)?;
        }
        Ok(changes)
    }

    /// The change `page.new_partial` would make: a new file at `path`, holding `text`.
    pub fn stage_new_partial(&self, path: &Path, text: &str) -> Result<ChangeSet, EngineError> {
        let path = normalize_path(path);
        self.check_new_file(&path)?;
        Ok(ChangeSet {
            creates: vec![FileCreate {
                path,
                text: text.to_owned(),
            }],
            ..ChangeSet::default()
        })
    }

    /// Refuses a new file outside the roots, not AsciiDoc, or already there.
    pub(crate) fn check_new_file(&self, path: &Path) -> Result<(), EngineError> {
        let inside = self.roots().iter().any(|root| path.starts_with(root));
        if !inside
            || !is_asciidoc_path(path)
            || path.exists()
            || self.document_for_path(path).is_some()
        {
            return Err(EngineError::InvalidRequest(format!(
                "{} is not a new AsciiDoc file in the workspace",
                path.display()
            )));
        }
        Ok(())
    }

    /// The change `page.rename` would make: the move and every xref, navigation entry and
    /// include it rewrites. Opens the page if it is not open (closing it again), and keeps no
    /// preview.
    pub fn stage_rename(&mut self, path: &Path, new_path: &Path) -> Result<ChangeSet, EngineError> {
        let path = normalize_path(path);
        let (id, opened) = match self.document_for_path(&path) {
            Some(id) => (id, false),
            None => (self.open_unshown(&path)?, true),
        };
        let preview = self.rename_preview(id, new_path);
        if opened {
            let _ = self.close(id);
        }
        let preview = preview?;
        self.discard_preview(preview.id);
        Ok(ChangeSet {
            creates: Vec::new(),
            edits: preview.edits,
            moves: preview.moves,
        })
    }

    /// Each file `changes` touches as it would be left, with its diagnostics then, worked out
    /// on a copy of the workspace's index and catalog; nothing of the engine's changes.
    pub fn check_staged(&self, changes: &ChangeSet) -> Result<Vec<StagedFile>, EngineError> {
        let mut workspace = self.workspace.clone();
        let mut files: BTreeMap<PathBuf, StagedFile> = BTreeMap::new();
        for create in &changes.creates {
            files.insert(
                create.path.clone(),
                StagedFile {
                    path: create.path.clone(),
                    before: None,
                    after: create.text.clone(),
                    moved_to: None,
                    diagnostics: Vec::new(),
                },
            );
        }
        for file in &changes.edits {
            let (_, after) = edited(&file.base, file.edits.clone())?;
            files.insert(
                file.path.clone(),
                StagedFile {
                    path: file.path.clone(),
                    before: Some(file.base.clone()),
                    after,
                    moved_to: None,
                    diagnostics: Vec::new(),
                },
            );
        }
        for file_move in &changes.moves {
            let entry = match files.remove(&file_move.from) {
                Some(entry) => entry,
                None => {
                    let text = self.current_text(&file_move.from).ok_or_else(|| {
                        EngineError::NotFound(format!(
                            "{} cannot be read",
                            file_move.from.display()
                        ))
                    })?;
                    StagedFile {
                        path: file_move.from.clone(),
                        before: Some(text.clone()),
                        after: text,
                        moved_to: None,
                        diagnostics: Vec::new(),
                    }
                }
            };
            workspace.remove_document(&file_move.from);
            workspace.antora.remove_source(&file_move.from);
            files.insert(
                file_move.from.clone(),
                StagedFile {
                    moved_to: Some(file_move.to.clone()),
                    ..entry
                },
            );
        }
        // The copy as the change leaves it: every file at its place after, with its text after.
        for file in files.values() {
            let at = file.moved_to.as_ref().unwrap_or(&file.path);
            if !is_asciidoc_path(at) {
                continue;
            }
            let document = adoc_parser::parse(&crate::document::uri_for(at), &file.after).document;
            workspace.replace_document(at.clone(), document);
            if workspace.antora.resource_for_path(at).is_none() {
                if let Some(resource) = self.resource_at(at) {
                    workspace.antora.insert(resource);
                }
            }
        }
        for file in files.values_mut() {
            let at = file.moved_to.as_ref().unwrap_or(&file.path);
            if is_asciidoc_path(at) {
                file.diagnostics = adoc_lang::diagnostics(&workspace, at);
            }
        }
        Ok(files.into_values().collect())
    }

    /// Applies `changes` as one undo group: the files made, then the edits, then the moves.
    /// Every edited file must still hold the text its edits were worked out against
    /// (`PreviewStale` otherwise) and every file to make must still not exist; on any failure
    /// nothing of it stays. Edits are left in the pages' buffers, unsaved; made files are
    /// written as they are made, as `page.new` does.
    pub fn apply_staged(&mut self, changes: &ChangeSet) -> Result<UndoGroup, EngineError> {
        for create in &changes.creates {
            self.check_new_file(&create.path)?;
        }
        let mut targets: Vec<(DocumentId, Vec<TextEdit>)> = Vec::new();
        for file in &changes.edits {
            let document = self.open_unshown(&file.path)?;
            let text = &self.documents[&document].text;
            if *text != file.base.as_str() {
                return Err(EngineError::PreviewStale {
                    path: file.path.clone(),
                });
            }
            targets.push((document, edit::validate(text, file.edits.clone())?));
        }
        crate::search::validate_moves(&changes.moves)?;
        self.applying = true;
        let group = self.new_group();
        let mut made = Vec::new();
        let mut result = Ok(());
        for create in &changes.creates {
            match self.create_in_group(group, &create.path, &create.text) {
                Ok(()) => made.push(create.clone()),
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        if result.is_ok() {
            result = self.apply_group(group, targets, &changes.moves);
        }
        self.applying = false;
        if let Err(error) = result {
            self.roll_back(group);
            for create in made.iter().rev() {
                if let Some(id) = self.document_for_path(&create.path) {
                    let _ = self.close(id);
                }
                let _ = std::fs::remove_file(&create.path);
                self.workspace.remove_document(&create.path);
                self.catalog_changed_at(&create.path);
            }
            return Err(error);
        }
        let documents = self.documents_with_top_group(group, false);
        self.group_documents.insert(group, documents);
        if !made.is_empty() {
            self.group_created.insert(
                group,
                made.into_iter()
                    .map(|create| crate::new_page::CreatedFile::new(create.path, create.text))
                    .collect(),
            );
        }
        self.refresh_open_diagnostics(None);
        self.audit_changed_everywhere();
        Ok(group)
    }

    /// Makes the file at `path` with `text` and opens it, without showing it, in `group`.
    fn create_in_group(
        &mut self,
        group: UndoGroup,
        path: &Path,
        text: &str,
    ) -> Result<(), EngineError> {
        crate::new_page::create_file(path, text)?;
        self.catalog_changed_at(path);
        let opened = match self.open_unshown(path) {
            Ok(opened) => opened,
            Err(error) => {
                let _ = std::fs::remove_file(path);
                self.catalog_changed_at(path);
                return Err(error);
            }
        };
        self.push_marker(opened, group);
        Ok(())
    }

    /// The resource a file at `path` would be, for a file not yet in the catalog.
    fn resource_at(&self, path: &Path) -> Option<AntoraResource> {
        ResourceFamily::ALL.into_iter().find_map(|family| {
            self.coordinate_in(path, family)
                .map(|coordinate| AntoraResource {
                    coordinate,
                    source_path: path.to_path_buf(),
                    aliases: Vec::new(),
                })
        })
    }
}
