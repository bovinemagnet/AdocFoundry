//! Language features for open documents, delegated to `adoc-lang` so the engine and the
//! language server behave identically.

use std::{
    fs,
    path::{Path, PathBuf},
};

use adoc_antora::{NavError, NavTree};

use adoc_core::{Diagnostic, LineIndex, SourceRange};
use adoc_lang::{
    CompletionItem, DefinitionTarget, DocumentSymbol, Hover, WorkspaceSymbol, WorkspaceSymbolKind,
    XrefLink,
};

use crate::{DocumentId, Engine, NavigationPosition, SearchMatch};

/// The most symbols a workspace query returns.
pub const SYMBOL_RESULTS: usize = 100;

/// A section, anchor or page title to go to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolMatch {
    pub name: String,
    pub kind: WorkspaceSymbolKind,
    /// Where it is: the page's title (or `Page` for a page title) and, in Antora, the
    /// component version and module (`Guide · alpha 1.0 › ROOT`), else the page's path in
    /// the workspace.
    pub detail: String,
    pub position: NavigationPosition,
}

impl Engine {
    /// Where the construct at `offset` in document `id` leads.
    #[must_use]
    pub fn definition(&self, id: DocumentId, offset: usize) -> Option<DefinitionTarget> {
        let document = self.document(id)?;
        adoc_lang::definition_in(&self.workspace, &document.path, document.parsed(), offset)
    }

    #[must_use]
    pub fn hover(&self, id: DocumentId, offset: usize) -> Option<Hover> {
        let document = self.document(id)?;
        adoc_lang::hover_in(&self.workspace, &document.path, document.parsed(), offset)
    }

    /// Whether typing `typed` at `offset` in document `id` puts the caret inside something
    /// that completes: a target being typed, `<<`, or an attribute reference. Only the line is
    /// read, so a front end can ask on every keystroke, and ask for [`Engine::completions`]
    /// only when this holds.
    #[must_use]
    pub fn completes_after(&self, id: DocumentId, offset: usize, typed: &str) -> bool {
        let Some(document) = self.document(id) else {
            return false;
        };
        let text = &document.text;
        let offset = text.floor_char_boundary(offset.min(text.len()));
        // Back from the caret to the line's start: lines are short, and ropey's line metrics
        // depend on its features.
        let mut start = offset;
        for character in text.chars_at(offset).reversed() {
            if character == '\n' {
                break;
            }
            start -= character.len_utf8();
        }
        let mut line = text.slice(start..offset).to_string();
        line.push_str(typed);
        adoc_parser::completion_context(&line, line.len()).is_some()
    }

    #[must_use]
    pub fn completions(&self, id: DocumentId, offset: usize) -> Vec<CompletionItem> {
        let Some(document) = self.document(id) else {
            return Vec::new();
        };
        adoc_lang::completion_in(&self.workspace, &document.path, document.parsed(), offset)
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
            &document.parsed().text,
            offset,
        )
    }

    /// The references to what is at `offset` in document `id`, as [`Engine::references`] finds
    /// them, each with its line for listing (from the open buffer when the file is open), in
    /// file order.
    #[must_use]
    pub fn reference_matches(&self, id: DocumentId, offset: usize) -> Vec<SearchMatch> {
        let mut locations = self.references(id, offset);
        locations.sort();
        locations
            .into_iter()
            .filter_map(|location| {
                let text = match self.document_for_path(&location.path) {
                    Some(open) => self.document(open)?.text_string(),
                    None => self
                        .workspace
                        .index
                        .file(&location.path)?
                        .document
                        .text
                        .clone(),
                };
                let lines = LineIndex::new(&text);
                let (line, _) = lines.line_column(location.range.start);
                let start = lines.offset(line, 0).unwrap_or(0);
                let end = text[start..]
                    .find('\n')
                    .map_or(text.len(), |end| start + end);
                Some(SearchMatch {
                    line,
                    line_text: text[start..end].trim_end_matches('\r').to_owned(),
                    path: location.path,
                    range: location.range,
                })
            })
            .collect()
    }

    /// The id of the anchor at `offset` in document `id`: an `[[id]]` or `[#id]` there, a
    /// section heading's id (declared or generated), or the anchor a reference there names.
    #[must_use]
    pub fn anchor_at(&self, id: DocumentId, offset: usize) -> Option<String> {
        let document = self.document(id)?;
        adoc_lang::references::anchor_at(&self.workspace, &document.path, document.parsed(), offset)
            .map(|anchor| anchor.id)
    }

    /// The sections, anchors and page titles matching `query`: across the workspace, each
    /// word of the query starting a word of the name (those starting with the query first, at
    /// most [`SYMBOL_RESULTS`]); or in document `page`, those whose name contains the query,
    /// every one for an empty query, in page order.
    #[must_use]
    pub fn symbols(&self, query: &str, page: Option<DocumentId>) -> Vec<SymbolMatch> {
        let found: Vec<WorkspaceSymbol> = match page {
            Some(id) => {
                let Some(document) = self.document(id) else {
                    return Vec::new();
                };
                let parsed = &document.parsed();
                let symbol = |name: &str, kind, range: SourceRange| WorkspaceSymbol {
                    name: name.to_owned(),
                    kind,
                    path: document.path.clone(),
                    range,
                    container: parsed.title.as_ref().map(|title| title.text.clone()),
                };
                let mut found: Vec<WorkspaceSymbol> = parsed
                    .title
                    .iter()
                    .map(|title| {
                        symbol(
                            &title.text,
                            WorkspaceSymbolKind::Title,
                            title.selection_range,
                        )
                    })
                    .chain(parsed.sections.iter().map(|section| {
                        symbol(
                            &section.title,
                            WorkspaceSymbolKind::Section,
                            section.selection_range,
                        )
                    }))
                    .chain(parsed.anchors.iter().map(|anchor| {
                        symbol(&anchor.id, WorkspaceSymbolKind::Anchor, anchor.range)
                    }))
                    .filter(|symbol| symbol.name.to_lowercase().contains(&query.to_lowercase()))
                    .collect();
                found.sort_by_key(|symbol| symbol.range.start);
                found
            }
            None => {
                let query = query.trim().to_lowercase();
                let mut found: Vec<WorkspaceSymbol> = self
                    .workspace_symbols(&query)
                    .into_iter()
                    .filter(|symbol| symbol.kind != WorkspaceSymbolKind::Attribute)
                    .collect();
                found.sort_by_key(|symbol| !symbol.name.to_lowercase().starts_with(&query));
                found.truncate(SYMBOL_RESULTS);
                found
            }
        };
        found
            .into_iter()
            .map(|symbol| SymbolMatch {
                detail: self.symbol_detail(&symbol),
                position: NavigationPosition {
                    path: symbol.path,
                    offset: symbol.range.start,
                },
                name: symbol.name,
                kind: symbol.kind,
            })
            .collect()
    }

    /// Where `symbol` is, for listing it: its page and the page's place in the workspace.
    fn symbol_detail(&self, symbol: &WorkspaceSymbol) -> String {
        let page = match symbol.kind {
            WorkspaceSymbolKind::Title => "Page".to_owned(),
            _ => symbol.container.clone().unwrap_or_else(|| {
                symbol
                    .path
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
            }),
        };
        let place = match self.workspace.antora.context_for_path(&symbol.path) {
            Some(context) => match context.version {
                Some(version) => format!("{} {version} › {}", context.component, context.module),
                None => format!("{} › {}", context.component, context.module),
            },
            None => self
                .roots()
                .iter()
                .find_map(|root| symbol.path.strip_prefix(root).ok())
                .unwrap_or(&symbol.path)
                .display()
                .to_string(),
        };
        format!("{page} · {place}")
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
            &document.parsed().text,
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
            Some(open) => open.parsed(),
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
            adoc_lang::symbols::nav_symbols(document.parsed())
        } else {
            adoc_lang::document_symbols(document.parsed())
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
