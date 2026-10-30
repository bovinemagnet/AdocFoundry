//! Language features for open documents, delegated to `adoc-lang` so the engine and the
//! language server behave identically.

use std::{
    fs,
    path::{Path, PathBuf},
};

use adoc_antora::{NavError, NavTree};

use adoc_core::{Diagnostic, SourceRange};
use adoc_lang::{
    CompletionItem, DefinitionTarget, DocumentSymbol, Hover, WorkspaceSymbol, XrefLink,
};

use crate::{DocumentId, Engine};

impl Engine {
    /// Where the construct at `offset` in document `id` leads.
    #[must_use]
    pub fn definition(&self, id: DocumentId, offset: usize) -> Option<DefinitionTarget> {
        let document = self.document(id)?;
        adoc_lang::definition_at_offset(
            &self.workspace,
            &document.path,
            &document.parsed.text,
            offset,
        )
    }

    #[must_use]
    pub fn hover(&self, id: DocumentId, offset: usize) -> Option<Hover> {
        let document = self.document(id)?;
        adoc_lang::hover_at_offset(
            &self.workspace,
            &document.path,
            &document.parsed.text,
            offset,
        )
    }

    #[must_use]
    pub fn completions(&self, id: DocumentId, offset: usize) -> Vec<CompletionItem> {
        let Some(document) = self.document(id) else {
            return Vec::new();
        };
        adoc_lang::completion_at_offset(
            &self.workspace,
            &document.path,
            &document.parsed.text,
            offset,
        )
    }

    /// The references to what is at `offset` in document `id`: to the anchor there, or to the
    /// page (the xrefs that link to it and the includes that include it).
    #[must_use]
    pub fn references(&self, id: DocumentId, offset: usize) -> Vec<adoc_lang::Location> {
        let Some(document) = self.document(id) else {
            return Vec::new();
        };
        adoc_lang::references::references_at(
            &self.workspace,
            &document.path,
            &document.parsed.text,
            offset,
        )
    }

    /// The id of the anchor at `offset` in document `id`: an `[[id]]` or `[#id]` there, a
    /// section heading's id (declared or generated), or the anchor a reference there names.
    #[must_use]
    pub fn anchor_at(&self, id: DocumentId, offset: usize) -> Option<String> {
        let document = self.document(id)?;
        adoc_lang::references::anchor_at(&self.workspace, &document.path, &document.parsed, offset)
            .map(|anchor| anchor.id)
    }

    /// The edits that rename the anchor at `offset` in document `id`, and every reference to
    /// it, to `new_id`, by file. `None` when there is no anchor there or `new_id` is not an id.
    #[must_use]
    pub fn anchor_rename_edits(
        &self,
        id: DocumentId,
        offset: usize,
        new_id: &str,
    ) -> Option<Vec<crate::FileEdits>> {
        let document = self.document(id)?;
        let edits = adoc_lang::references::rename_anchor(
            &self.workspace,
            &document.path,
            &document.parsed.text,
            offset,
            new_id,
        )?;
        let mut files: Vec<crate::FileEdits> = Vec::new();
        for (location, text) in edits {
            let edit = crate::TextEdit::new(location.range, text);
            match files.last_mut() {
                Some(file) if file.path == location.path => file.edits.push(edit),
                _ => {
                    // The ranges come from the index's text, as rename's do.
                    let base = self
                        .workspace
                        .index
                        .file(&location.path)
                        .map(|file| file.document.text.clone())
                        .unwrap_or_default();
                    files.push(crate::FileEdits {
                        path: location.path,
                        edits: vec![edit],
                        base,
                    });
                }
            }
        }
        Some(files)
    }

    /// The link document `id` makes to the page at `to`, or to its section whose title is at
    /// `section` (an outline entry's selection range), as a page dragged into an editor
    /// inserts it.
    #[must_use]
    pub fn xref_to(
        &self,
        id: DocumentId,
        to: &Path,
        section: Option<SourceRange>,
    ) -> Option<XrefLink> {
        let from = &self.document(id)?.path;
        let target = match self
            .document_for_path(to)
            .and_then(|open| self.document(open))
        {
            Some(open) => &open.parsed,
            None => &self.workspace.index.file(to)?.document,
        };
        adoc_lang::xref_to(&self.workspace, from, to, target, section)
    }

    /// The outline of document `id`: the navigation tree for an Antora navigation file,
    /// otherwise its sections.
    #[must_use]
    pub fn outline(&self, id: DocumentId) -> Vec<DocumentSymbol> {
        let Some(document) = self.document(id) else {
            return Vec::new();
        };
        if self.workspace.antora.is_nav_file(&document.path) {
            adoc_lang::symbols::nav_symbols(&document.parsed)
        } else {
            adoc_lang::document_symbols(&document.parsed)
        }
    }

    /// Diagnostics for every file in the workspace, open buffers included, sorted by path.
    #[must_use]
    pub fn workspace_diagnostics(&self) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        adoc_lang::workspace_diagnostics(&self.workspace)
    }

    #[must_use]
    pub fn workspace_symbols(&self, query: &str) -> Vec<WorkspaceSymbol> {
        adoc_lang::workspace_symbols(&self.workspace, query)
    }

    /// The navigation trees of a component version, reading open documents from their buffers.
    pub fn nav_trees(
        &self,
        component: &str,
        version: Option<&str>,
    ) -> Result<Vec<NavTree>, NavError> {
        let read = |path: &Path| match self.document_for_path(path) {
            Some(id) => Ok(self
                .document(id)
                .map(|document| document.text.to_string())
                .unwrap_or_default()),
            None => fs::read_to_string(path),
        };
        adoc_antora::nav_tree(&self.workspace.antora, component, version, &read)
    }

    /// The navigation trees of `files`, in that order, from open buffers or the disk.
    pub fn nav_trees_of(&self, files: &[std::path::PathBuf]) -> Result<Vec<NavTree>, NavError> {
        let read = |path: &Path| match self.document_for_path(path) {
            Some(id) => Ok(self
                .document(id)
                .map(|document| document.text.to_string())
                .unwrap_or_default()),
            None => fs::read_to_string(path),
        };
        adoc_antora::nav_trees_of(&self.workspace.antora, files, &read)
    }
}
