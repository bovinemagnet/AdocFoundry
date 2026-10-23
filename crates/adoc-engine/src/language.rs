//! Language features for open documents, delegated to `adoc-lang` so the engine and the
//! language server behave identically.

use std::path::PathBuf;

use adoc_core::Diagnostic;
use adoc_lang::{CompletionItem, DefinitionTarget, DocumentSymbol, Hover, WorkspaceSymbol};

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
}
