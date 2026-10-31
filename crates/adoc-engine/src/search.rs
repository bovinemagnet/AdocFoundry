//! Workspace search, replace with preview, and page rename.
//!
//! Multi-file changes are computed as an [`EditPreview`] first; `workspace.apply_preview`
//! applies one as a single undo group spanning every document it touches, including any file
//! move, so one undo restores everything.

use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

use adoc_antora::{parse_resource_id, AntoraContext, AntoraCoordinate, ResourceFamily};
use adoc_core::{LineIndex, SourceRange};
use adoc_index::normalize_path;
use adoc_lang::DefinitionTarget;
use adoc_parser::parse;
use regex::{Regex, RegexBuilder};

use crate::{
    document::uri_for, edit, engine::UndoMode, workspace::WorkerMessage, DocumentId, EditPreview,
    Engine, EngineError, EngineEvent, FileEdits, FileMove, PreviewId, SearchId, SearchMatch,
    SearchRequest, TextEdit, UndoEntry, UndoGroup,
};

fn build_regex(request: &SearchRequest) -> Result<Regex, EngineError> {
    if request.query.is_empty() {
        return Err(EngineError::InvalidRequest(
            "the search query is empty".to_owned(),
        ));
    }
    let pattern = if request.regex {
        request.query.clone()
    } else {
        regex::escape(&request.query)
    };
    let pattern = if request.whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!request.case_sensitive)
        .multi_line(true)
        .build()
        .map_err(|error| EngineError::InvalidRequest(error.to_string()))
}

/// The most matches a workspace search reports; past it the search stops and says so.
pub const SEARCH_MATCH_LIMIT: usize = 10_000;

/// A search reports what it found in batches of this many matches, or fewer when finding them
/// takes longer than [`SEARCH_BATCH_INTERVAL`].
const SEARCH_BATCH: usize = 500;
const SEARCH_BATCH_INTERVAL: Duration = Duration::from_millis(50);

/// Adds to `out` the first `limit` matches of `regex` in the file at `path`, its text `text`.
/// Lines are counted only in a file with a match.
fn find_matches(regex: &Regex, path: &Path, text: &str, limit: usize, out: &mut Vec<SearchMatch>) {
    let mut lines = None;
    for found in regex
        .find_iter(text)
        .filter(|found| !found.is_empty())
        .take(limit)
    {
        let lines = lines.get_or_insert_with(|| LineIndex::new(text));
        let (line, _) = lines.line_column(found.start());
        let line_start = lines.offset(line, 0).unwrap_or(0);
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |end| line_start + end);
        out.push(SearchMatch {
            path: path.to_path_buf(),
            range: SourceRange::new(found.start(), found.end()),
            line,
            line_text: text[line_start..line_end].trim_end_matches('\r').to_owned(),
        });
    }
}

impl Engine {
    /// The text of every indexed file in `scope`, open buffers included, in path order.
    fn searchable_files(&self, scope: Option<&Path>) -> Vec<(&Path, &str)> {
        let scope = scope.map(normalize_path);
        let mut files: Vec<(&Path, &str)> = self
            .workspace
            .index
            .files()
            .filter(|file| {
                scope
                    .as_ref()
                    .is_none_or(|scope| file.path.starts_with(scope))
            })
            .map(|file| (file.path.as_path(), file.document.text.as_str()))
            .collect();
        files.sort_by(|left, right| left.0.cmp(right.0));
        files
    }

    /// Starts a workspace search on a worker thread, which reads the files there: an open
    /// page's buffer, else the file. Results arrive as `SearchResults` events, applied by
    /// [`Engine::pump`], in batches with `done: false` and then one with `done: true`. It stops
    /// at [`SEARCH_MATCH_LIMIT`] matches, saying `limited`, and as soon as another search
    /// starts.
    pub fn search(&mut self, request: SearchRequest) -> Result<SearchId, EngineError> {
        let regex = build_regex(&request)?;
        let scope = request.scope.as_deref().map(normalize_path);
        let open: BTreeMap<&Path, String> = self
            .documents
            .values()
            .filter(|document| document.indexed())
            .map(|document| (document.path.as_path(), document.text.to_string()))
            .collect();
        let mut files: Vec<(PathBuf, Option<String>)> = self
            .workspace
            .index
            .files()
            .filter(|file| {
                scope
                    .as_ref()
                    .is_none_or(|scope| file.path.starts_with(scope))
            })
            .map(|file| (file.path.clone(), open.get(file.path.as_path()).cloned()))
            .collect();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        let id = SearchId(self.next_search);
        self.next_search += 1;
        self.latest_search.store(id.get(), Ordering::Relaxed);
        let latest = self.latest_search.clone();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let mut batch = Vec::new();
            let mut found = 0;
            let mut sent = Instant::now();
            for (path, text) in files {
                if latest.load(Ordering::Relaxed) != id.get() {
                    break;
                }
                let Some(text) = text.or_else(|| fs::read_to_string(&path).ok()) else {
                    continue;
                };
                let before = batch.len();
                // One past the limit, to tell whether there are more.
                find_matches(
                    &regex,
                    &path,
                    &text,
                    SEARCH_MATCH_LIMIT + 1 - found,
                    &mut batch,
                );
                found += batch.len() - before;
                if found > SEARCH_MATCH_LIMIT {
                    break;
                }
                if batch.len() >= SEARCH_BATCH
                    || (!batch.is_empty() && sent.elapsed() >= SEARCH_BATCH_INTERVAL)
                {
                    let results = std::mem::take(&mut batch);
                    let _ = sender.send(WorkerMessage::Search {
                        id,
                        results,
                        done: false,
                        limited: false,
                    });
                    sent = Instant::now();
                }
            }
            let limited = found > SEARCH_MATCH_LIMIT;
            if limited {
                batch.pop();
            }
            let _ = sender.send(WorkerMessage::Search {
                id,
                results: batch,
                done: true,
                limited,
            });
        });
        Ok(id)
    }

    pub(crate) fn search_found(
        &mut self,
        request: SearchId,
        results: Vec<SearchMatch>,
        done: bool,
        limited: bool,
    ) {
        self.emit(EngineEvent::SearchResults {
            request,
            results,
            done,
            limited,
        });
    }

    /// Computes the edits that replace matches of `request` with `replacement`: every match, or
    /// only those whose path and range appear in `selected`. With a regex, `$1`-style capture
    /// references in `replacement` are expanded.
    pub fn replace_preview(
        &mut self,
        request: &SearchRequest,
        replacement: &str,
        selected: Option<&[(PathBuf, SourceRange)]>,
    ) -> Result<EditPreview, EngineError> {
        let regex = build_regex(request)?;
        let mut edits = Vec::new();
        for (path, text) in self.searchable_files(request.scope.as_deref()) {
            let mut file_edits = Vec::new();
            for captures in regex.captures_iter(text) {
                let Some(found) = captures.get(0).filter(|found| !found.is_empty()) else {
                    continue;
                };
                let range = SourceRange::new(found.start(), found.end());
                if selected.is_some_and(|selected| {
                    !selected.iter().any(|(selected_path, selected_range)| {
                        normalize_path(selected_path) == *path && *selected_range == range
                    })
                }) {
                    continue;
                }
                let mut new_text = String::new();
                if request.regex {
                    captures.expand(replacement, &mut new_text);
                } else {
                    new_text.push_str(replacement);
                }
                file_edits.push(TextEdit::new(range, new_text));
            }
            if !file_edits.is_empty() {
                edits.push(FileEdits {
                    path: path.to_path_buf(),
                    edits: file_edits,
                    base: text.to_owned(),
                });
            }
        }
        let count: usize = edits.iter().map(|file| file.edits.len()).sum();
        Ok(self.store_preview(
            format!("Replace {count} occurrence(s) of `{}`", request.query),
            Vec::new(),
            edits,
        ))
    }

    fn store_preview(
        &mut self,
        title: String,
        moves: Vec<FileMove>,
        edits: Vec<FileEdits>,
    ) -> EditPreview {
        let preview = EditPreview {
            id: PreviewId(self.next_preview),
            title,
            moves,
            edits,
        };
        self.next_preview += 1;
        self.previews.insert(preview.id, preview.clone());
        preview
    }

    /// A pending preview, if it has not been applied or discarded.
    #[must_use]
    pub fn preview(&self, id: PreviewId) -> Option<&EditPreview> {
        self.previews.get(&id)
    }

    pub fn discard_preview(&mut self, id: PreviewId) {
        self.previews.remove(&id);
    }

    /// Applies a preview as one undo group across every document and file move it contains.
    pub fn apply_preview(&mut self, id: PreviewId) -> Result<UndoGroup, EngineError> {
        let preview = self
            .previews
            .remove(&id)
            .ok_or_else(|| EngineError::NotFound(format!("no preview {}", id.get())))?;
        validate_moves(&preview.moves)?;
        let previously_active = self.active;

        // Open everything and check every edit before changing anything: each file must still
        // hold the text its edits were computed against.
        let mut targets: Vec<(DocumentId, Vec<TextEdit>)> = Vec::new();
        for file in &preview.edits {
            let document = self.open(&file.path)?;
            let text = &self.documents[&document].text;
            if *text != file.base.as_str() {
                return Err(EngineError::PreviewStale {
                    path: file.path.clone(),
                });
            }
            let edits = edit::validate(text, file.edits.clone())?;
            targets.push((document, edits));
        }
        let group = self.new_group();
        if let Err(error) = self.apply_group(group, targets, &preview.moves) {
            self.roll_back(group);
            return Err(error);
        }
        let documents = self.documents_with_top_group(group, false);
        self.group_documents.insert(group, documents);
        if let Some(active) = previously_active.filter(|id| self.documents.contains_key(id)) {
            self.set_active(active);
        }
        self.refresh_open_diagnostics(None);
        Ok(group)
    }

    fn apply_group(
        &mut self,
        group: UndoGroup,
        targets: Vec<(DocumentId, Vec<TextEdit>)>,
        moves: &[FileMove],
    ) -> Result<(), EngineError> {
        for (document, edits) in targets {
            self.apply_edits(document, edits, UndoMode::Group(group))?;
        }
        for file_move in moves {
            let document = self.open(&file_move.from)?;
            if self
                .undo_stack(document)
                .and_then(|stack| stack.top_group())
                != Some(group)
            {
                self.push_marker(document, group);
            }
            self.move_file(&file_move.from, &file_move.to)?;
            self.group_moves
                .entry(group)
                .or_default()
                .push(file_move.clone());
        }
        Ok(())
    }

    /// Undoes what a failed `apply_group` made of `group`, leaving nothing of it in the undo
    /// or redo history.
    fn roll_back(&mut self, group: UndoGroup) {
        self.discard_group(group);
        if let Some(moves) = self.group_moves.remove(&group) {
            for file_move in moves.iter().rev() {
                let _ = self.move_file(&file_move.to, &file_move.from);
            }
        }
        self.refresh_open_diagnostics(None);
    }

    /// Records an undo entry with no edits, so a document whose only change is a move takes
    /// part in the group's undo.
    pub(crate) fn push_marker(&mut self, id: DocumentId, group: UndoGroup) {
        let version = self
            .documents
            .get(&id)
            .map_or(0, |document| document.version);
        if let Some(state) = self.states.get_mut(&id) {
            state.undo.push(UndoEntry {
                group,
                inverse: Vec::new(),
                forward: Vec::new(),
                version_before: version,
                version_after: version,
                typing: None,
            });
        }
    }

    /// Moves a file on disk and everything the engine knows about it.
    pub(crate) fn move_file(&mut self, from: &Path, to: &Path) -> Result<(), EngineError> {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|error| EngineError::io(parent, error))?;
        }
        fs::rename(from, to).map_err(|error| EngineError::io(from, error))?;
        let from = normalize_path(from);
        let to = normalize_path(to);
        self.move_history(&from, &to);
        self.workspace.remove_document(&from);
        if let Some(id) = self.document_for_path(&from) {
            if let Some(document) = self.documents.get_mut(&id) {
                document.path.clone_from(&to);
                let text = document.text.to_string();
                document.parsed = std::cell::OnceCell::from(parse(&uri_for(&to), &text).document);
                let parsed = document.parsed().clone();
                self.workspace.replace_document(to.clone(), parsed);
            }
            self.emit(EngineEvent::DocumentMoved {
                id,
                from: from.clone(),
                to: to.clone(),
            });
            self.visual_source_changed(&to);
            self.git_document_moved(id);
            // Unsaved text is recovered at the new path.
            self.recovery_changed(id);
        } else if let Ok(text) = fs::read_to_string(&to) {
            self.workspace.index.index_source(&to, &text);
        }
        // The page's Antora coordinate changed: re-read its component.
        let descriptors: Vec<PathBuf> = self
            .workspace
            .antora
            .components()
            .filter(|component| {
                from.starts_with(&component.root) || to.starts_with(&component.root)
            })
            .map(|component| component.root.join("antora.yml"))
            .collect();
        for descriptor in descriptors {
            let _ = self.workspace.reload_descriptor(&descriptor);
        }
        Ok(())
    }

    /// Re-applies or reverses the file moves of `group` after its text edits were redone or
    /// undone.
    pub(crate) fn replay_moves(&mut self, group: UndoGroup, reverse: bool) {
        let Some(moves) = self.group_moves.get(&group).cloned() else {
            return;
        };
        if reverse {
            for file_move in moves.iter().rev() {
                let _ = self.move_file(&file_move.to, &file_move.from);
            }
        } else {
            for file_move in &moves {
                let _ = self.move_file(&file_move.from, &file_move.to);
            }
        }
        self.refresh_open_diagnostics(None);
    }

    /// Computes renaming the anchor at `offset` in document `id` to `new_id`, and every
    /// reference to it in every page; a section without an anchor gets one.
    pub fn anchor_rename_preview(
        &mut self,
        id: DocumentId,
        offset: usize,
        new_id: &str,
    ) -> Result<EditPreview, EngineError> {
        let old_id = self
            .anchor_at(id, offset)
            .ok_or_else(|| EngineError::InvalidRequest("there is no anchor here".to_owned()))?;
        let edits = self
            .anchor_rename_edits(id, offset, new_id)
            .ok_or_else(|| {
                EngineError::InvalidRequest(format!("`{new_id}` cannot be an anchor id"))
            })?;
        Ok(self.store_preview(
            format!("Rename anchor `{old_id}` to `{new_id}`"),
            Vec::new(),
            edits,
        ))
    }

    /// Computes moving the page of document `id` to `new_path` and rewriting every xref that
    /// resolves to it and every include that includes it.
    pub fn rename_preview(
        &mut self,
        id: DocumentId,
        new_path: &Path,
    ) -> Result<EditPreview, EngineError> {
        let document = self.document(id).ok_or(EngineError::UnknownDocument(id))?;
        if document.untitled {
            return Err(EngineError::InvalidRequest(
                "an untitled document has no file to rename".to_owned(),
            ));
        }
        let from = document.path.clone();
        let to = normalize_path(new_path);
        if to.exists() {
            return Err(EngineError::InvalidRequest(format!(
                "{} already exists",
                to.display()
            )));
        }
        let new_coordinate = self.page_coordinate(&to);

        let mut edits: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
        for location in adoc_lang::references_to(&self.workspace, &from) {
            // The page's references to itself are rewritten with its own, below.
            if location.path == from {
                continue;
            }
            let Some(file) = self.workspace.index.file(&location.path) else {
                continue;
            };
            let text = &file.document.text;
            let Some((range, file_part)) = xref_file_part(text, location.range) else {
                continue;
            };
            let context = self.workspace.antora.context_for_path(&location.path);
            let replacement = match (&context, &new_coordinate) {
                (Some(context), Some(coordinate)) => antora_id(context, coordinate, file_part),
                _ => relative_path(location.path.parent().unwrap_or_else(|| Path::new("")), &to),
            };
            if replacement != file_part {
                edits
                    .entry(location.path.clone())
                    .or_default()
                    .push(TextEdit::new(range, replacement));
            }
        }
        for location in adoc_lang::references::includes_of(&self.workspace, &from) {
            if location.path == from {
                continue;
            }
            let target = location.include.target.as_str();
            // A target written through an attribute is left as written.
            if target.contains('{') {
                continue;
            }
            let Some(file) = self.workspace.index.file(&location.path) else {
                continue;
            };
            let Some(range) = include_target_range(&file.document.text, location.include.range)
            else {
                continue;
            };
            let antora = parse_resource_id(target)
                .ok()
                .filter(|_| target.contains('$'))
                .and_then(|id| {
                    let context = self.workspace.antora.context_for_path(&location.path)?;
                    let family = id.family?;
                    let coordinate = self.coordinate_in(&to, family)?;
                    Some(with_family(
                        antora_id(&context, &coordinate, target),
                        family,
                    ))
                });
            let replacement = antora.unwrap_or_else(|| {
                relative_path(location.path.parent().unwrap_or_else(|| Path::new("")), &to)
            });
            if replacement != target {
                edits
                    .entry(location.path.clone())
                    .or_default()
                    .push(TextEdit::new(range, replacement));
            }
        }
        let outgoing = self.outgoing_edits(&from, &to);
        if !outgoing.is_empty() {
            edits.entry(from.clone()).or_default().extend(outgoing);
        }
        for file_edits in edits.values_mut() {
            file_edits.sort_by_key(|edit| edit.range.start);
        }
        // The ranges come from the index's text, which a buffer edited since may not match yet;
        // applying checks it still does.
        let edits = edits
            .into_iter()
            .map(|(path, edits)| {
                let base = self
                    .workspace
                    .index
                    .file(&path)
                    .map(|file| file.document.text.clone())
                    .unwrap_or_default();
                FileEdits { path, edits, base }
            })
            .collect();
        Ok(self.store_preview(
            format!("Rename {} to {}", from.display(), to.display()),
            vec![FileMove { from, to }],
            edits,
        ))
    }

    /// The edits that keep the moved page's own xrefs, includes and images naming what they
    /// named, once it is at `to`: targets relative to the page's directory change with the
    /// directory, and Antora resource ids with the module, component or version.
    fn outgoing_edits(&self, from: &Path, to: &Path) -> Vec<TextEdit> {
        let Some(file) = self.workspace.index.file(from) else {
            return Vec::new();
        };
        let document = &file.document;
        let text = document.text.as_str();
        let move_ = Move {
            from,
            to,
            old: self.workspace.antora.context_for_path(from),
            new: self.workspace.antora.context_for_path(to),
        };
        let mut targets: Vec<(SourceRange, &str, Written)> = Vec::new();
        for reference in &document.references {
            if reference.kind != adoc_core::ReferenceKind::Xref {
                continue;
            }
            if let Some((range, part)) = xref_file_part(text, reference.range) {
                targets.push((range, part, Written::Xref));
            }
        }
        for include in &document.includes {
            if let Some(range) = include_target_range(text, include.range) {
                targets.push((range, include.target.as_str(), Written::Include));
            }
        }
        for image in &document.images {
            if let Some(range) = image_target_range(text, image.range) {
                targets.push((range, image.target.as_str(), Written::Image));
            }
        }
        let mut edits = Vec::new();
        for (range, written, kind) in targets {
            // A pure `#fragment`, a target written through an attribute, and a URL are left.
            if written.is_empty() || written.contains('{') || written.contains("://") {
                continue;
            }
            let resolved =
                match adoc_lang::definition_at_offset(&self.workspace, from, text, range.start) {
                    Some(DefinitionTarget::Location(location)) => location.path,
                    Some(DefinitionTarget::External(path)) => path,
                    None => continue,
                };
            let target = if resolved == from {
                to.to_path_buf()
            } else {
                resolved
            };
            if let Some(replacement) = self.rewritten(&move_, written, &target, kind) {
                if replacement != written {
                    edits.push(TextEdit::new(range, replacement));
                }
            }
        }
        edits.sort_by_key(|edit| edit.range.start);
        edits
    }

    /// What `written` must become to name `target` from the page's new place, or `None` when
    /// the move does not change what it names.
    fn rewritten(
        &self,
        move_: &Move<'_>,
        written: &str,
        target: &Path,
        kind: Written,
    ) -> Option<String> {
        let relative = move_.old.is_none()
            || written.starts_with("./")
            || written.starts_with("../")
            || (kind == Written::Include && !written.contains('$'));
        if relative {
            let (old_dir, new_dir) = (move_.from.parent()?, move_.to.parent()?);
            if old_dir == new_dir {
                return None;
            }
            let path = relative_path(new_dir, target);
            return Some(if written.starts_with("./") && !path.starts_with("..") {
                format!("./{path}")
            } else {
                path
            });
        }
        let (old, new) = (move_.old.as_ref()?, move_.new.as_ref()?);
        if (&old.component, &old.version, &old.module)
            == (&new.component, &new.version, &new.module)
        {
            return None;
        }
        let family = match kind {
            Written::Xref => ResourceFamily::Page,
            Written::Image => ResourceFamily::Image,
            Written::Include => parse_resource_id(written).ok()?.family?,
        };
        let coordinate = self.coordinate_in(target, family)?;
        let id = antora_id(new, &coordinate, written);
        Some(if kind == Written::Include {
            with_family(id, family)
        } else {
            id
        })
    }

    /// The page coordinate a file at `path` would have, if it lies in a module's `pages`.
    fn page_coordinate(&self, path: &Path) -> Option<AntoraCoordinate> {
        self.coordinate_in(path, ResourceFamily::Page)
    }

    /// The coordinate a file at `path` would have in `family`, if it lies in that family's
    /// directory of a module.
    fn coordinate_in(&self, path: &Path, family: ResourceFamily) -> Option<AntoraCoordinate> {
        self.workspace.antora.modules().find_map(|module| {
            let relative = family
                .directories()
                .iter()
                .find_map(|directory| path.strip_prefix(module.root.join(directory)).ok())?;
            Some(AntoraCoordinate {
                component: module.component.clone(),
                version: module.version.clone(),
                module: module.name.clone(),
                family,
                relative_path: relative.to_path_buf(),
            })
        })
    }
}

/// The range and text of the file part of the xref target in `text[reference]`: the target
/// up to any `#fragment`.
fn xref_file_part(text: &str, reference: SourceRange) -> Option<(SourceRange, &str)> {
    let source = text.get(reference.start..reference.end)?;
    let after_prefix = source.strip_prefix("xref:")?;
    let open = after_prefix.find('[')?;
    let raw_target = &after_prefix[..open];
    let leading = raw_target.len() - raw_target.trim_start().len();
    let target = raw_target.trim();
    let file_part = target.split_once('#').map_or(target, |(file, _)| file);
    let start = reference.start + "xref:".len() + leading;
    Some((SourceRange::new(start, start + file_part.len()), file_part))
}

/// The range of the target in `text[directive]`, an `include::target[…]` line.
fn include_target_range(text: &str, directive: SourceRange) -> Option<SourceRange> {
    let source = text.get(directive.start..directive.end)?;
    let after_prefix = source.strip_prefix("include::")?;
    let target = &after_prefix[..after_prefix.find('[')?];
    let start = directive.start + "include::".len();
    Some(SourceRange::new(start, start + target.len()))
}

/// A page being moved, and the Antora contexts of where it is and where it goes.
struct Move<'a> {
    from: &'a Path,
    to: &'a Path,
    old: Option<AntoraContext>,
    new: Option<AntoraContext>,
}

/// Which macro a target is written in.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Written {
    Xref,
    Include,
    Image,
}

/// The range of the target in `text[directive]`, an `image::target[…]` line.
fn image_target_range(text: &str, directive: SourceRange) -> Option<SourceRange> {
    let source = text.get(directive.start..directive.end)?;
    let after_prefix = source.strip_prefix("image::")?;
    let target = &after_prefix[..after_prefix.find('[')?];
    let start = directive.start + "image::".len();
    Some(SourceRange::new(start, start + target.len()))
}

/// `id` with `family$` before its path: `partial$notes/token.adoc` from `notes/token.adoc`, or
/// `security:partial$token.adoc` from `security:token.adoc`.
fn with_family(id: String, family: ResourceFamily) -> String {
    match id.rsplit_once(':') {
        Some((coordinates, path)) => format!("{coordinates}:{family}${path}"),
        None => format!("{family}${id}"),
    }
}

/// The shortest resource id that names `coordinate` from `context`, keeping the explicit
/// coordinates the original target `original` used.
fn antora_id(context: &AntoraContext, coordinate: &AntoraCoordinate, original: &str) -> String {
    let path = slash_path(&coordinate.relative_path);
    let original = parse_resource_id(original).ok();
    let had_component = original.as_ref().is_some_and(|id| id.component.is_some());
    let had_version = original.as_ref().is_some_and(|id| id.version.is_some());
    let had_module = original.as_ref().is_some_and(|id| id.module.is_some());
    let same_component_version =
        coordinate.component == context.component && coordinate.version == context.version;
    if same_component_version && !had_component {
        if coordinate.module == context.module && !had_module {
            path
        } else {
            format!("{}:{path}", coordinate.module)
        }
    } else {
        // A version is written only where the original pinned one; otherwise the latest
        // version is meant, as before.
        let version = match &coordinate.version {
            Some(version) if had_version => format!("{version}@"),
            _ => String::new(),
        };
        format!(
            "{version}{}:{}:{path}",
            coordinate.component, coordinate.module
        )
    }
}

/// `to` relative to the directory `from`, with `/` separators.
fn relative_path(from: &Path, to: &Path) -> String {
    let from = normalize_path(from);
    let from: Vec<Component> = from.components().collect();
    let to_components: Vec<Component> = to.components().collect();
    let common = from
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_owned(), from.len() - common).collect();
    parts.extend(
        to_components[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Checks that every move can happen before anything changes: its source is a file, nothing is
/// at its destination, the destination's nearest existing ancestor is a directory, and no two
/// moves share a destination.
fn validate_moves(moves: &[FileMove]) -> Result<(), EngineError> {
    let mut destinations = std::collections::HashSet::new();
    for file_move in moves {
        if !file_move.from.is_file() {
            return Err(EngineError::NotFound(format!(
                "{} does not exist",
                file_move.from.display()
            )));
        }
        if file_move.to.exists() {
            return Err(EngineError::InvalidRequest(format!(
                "{} already exists",
                file_move.to.display()
            )));
        }
        if !destinations.insert(&file_move.to) {
            return Err(EngineError::InvalidRequest(format!(
                "two files would move to {}",
                file_move.to.display()
            )));
        }
        let mut ancestor = file_move.to.parent();
        while let Some(directory) = ancestor.filter(|directory| !directory.exists()) {
            ancestor = directory.parent();
        }
        if let Some(directory) = ancestor.filter(|directory| !directory.is_dir()) {
            return Err(EngineError::InvalidRequest(format!(
                "{} is a file, so {} cannot be made inside it",
                directory.display(),
                file_move.to.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(component: &str, version: Option<&str>, module: &str) -> AntoraContext {
        AntoraContext {
            component: component.to_owned(),
            version: version.map(str::to_owned),
            module: module.to_owned(),
            family: ResourceFamily::Page,
        }
    }

    fn coordinate(
        component: &str,
        version: Option<&str>,
        module: &str,
        path: &str,
    ) -> AntoraCoordinate {
        AntoraCoordinate {
            component: component.to_owned(),
            version: version.map(str::to_owned),
            module: module.to_owned(),
            family: ResourceFamily::Page,
            relative_path: PathBuf::from(path),
        }
    }

    #[test]
    fn writes_the_shortest_antora_id() {
        let here = context("alpha", Some("1.0"), "ROOT");
        let same = coordinate("alpha", Some("1.0"), "ROOT", "howto/setup.adoc");
        assert_eq!(antora_id(&here, &same, "guide.adoc"), "howto/setup.adoc");
        let other_module = coordinate("alpha", Some("1.0"), "ops", "setup.adoc");
        assert_eq!(
            antora_id(&here, &other_module, "guide.adoc"),
            "ops:setup.adoc"
        );
        let other_component = coordinate("beta", None, "ROOT", "setup.adoc");
        assert_eq!(
            antora_id(&here, &other_component, "beta::guide.adoc"),
            "beta:ROOT:setup.adoc"
        );
        let pinned = coordinate("alpha", Some("1.0"), "ROOT", "setup.adoc");
        assert_eq!(
            antora_id(
                &context("beta", None, "ROOT"),
                &pinned,
                "1.0@alpha::guide.adoc"
            ),
            "1.0@alpha:ROOT:setup.adoc"
        );
    }

    #[test]
    fn writes_relative_paths_outside_antora() {
        assert_eq!(
            relative_path(Path::new("/docs/a"), Path::new("/docs/b/c.adoc")),
            "../b/c.adoc"
        );
        assert_eq!(
            relative_path(Path::new("/docs"), Path::new("/docs/c.adoc")),
            "c.adoc"
        );
    }

    #[test]
    fn finds_the_file_part_of_an_xref_target() {
        let text = "See xref:guide.adoc#setup[Setup].";
        let reference = SourceRange::new(4, 32);
        let (range, file) = xref_file_part(text, reference).unwrap();
        assert_eq!(file, "guide.adoc");
        assert_eq!(&text[range.start..range.end], "guide.adoc");
    }

    #[test]
    fn rejects_empty_and_invalid_queries() {
        assert!(build_regex(&SearchRequest::default()).is_err());
        assert!(build_regex(&SearchRequest {
            query: "(".to_owned(),
            regex: true,
            ..SearchRequest::default()
        })
        .is_err());
    }
}
