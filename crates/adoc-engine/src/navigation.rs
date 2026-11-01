//! Link following and back/forward history. History is engine state so that every front end,
//! and the CLI tests, share it.

use std::{collections::BTreeMap, path::Path, path::PathBuf};

use adoc_core::SourceRange;
use adoc_lang::{DefinitionTarget, Location};

use crate::{
    is_image_path, CommandOutcome, DocumentId, Engine, EngineError, EngineEvent, OpenDisposition,
    TabItem, TextEdit,
};

/// A place in the workspace the user has been.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationPosition {
    pub path: PathBuf,
    pub offset: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct History {
    back: Vec<NavigationPosition>,
    forward: Vec<NavigationPosition>,
    current: Option<NavigationPosition>,
    /// Each document's caret, as its front end last reported it.
    carets: BTreeMap<DocumentId, usize>,
}

impl Engine {
    /// Records that document `id`'s caret is at `offset`, so leaving the page keeps the place
    /// to come back to.
    pub fn caret_moved(&mut self, id: DocumentId, offset: usize) {
        self.history.carets.insert(id, offset);
    }

    /// Moves the positions in `path` that history keeps, and document `id`'s caret, through
    /// `edits`, so going back arrives where the text it left now is.
    pub(crate) fn navigation_edited(&mut self, id: DocumentId, path: &Path, edits: &[TextEdit]) {
        let mapping = adoc_model::Mapping::new(
            &edits
                .iter()
                .map(|edit| adoc_model::SourceEdit::new(edit.range, edit.replacement.clone()))
                .collect::<Vec<_>>(),
        );
        let map = |offset: usize| mapping.map(offset, adoc_model::Assoc::Before);
        let history = &mut self.history;
        for position in history
            .back
            .iter_mut()
            .chain(history.forward.iter_mut())
            .chain(history.current.iter_mut())
            .filter(|position| position.path == path)
        {
            position.offset = map(position.offset);
        }
        if let Some(caret) = history.carets.get_mut(&id) {
            *caret = map(*caret);
        }
    }

    pub(crate) fn forget_caret(&mut self, id: DocumentId) {
        self.history.carets.remove(&id);
    }

    #[must_use]
    pub fn can_go_back(&self) -> bool {
        !self.history.back.is_empty()
    }

    #[must_use]
    pub fn can_go_forward(&self) -> bool {
        !self.history.forward.is_empty()
    }

    /// The position the last navigation arrived at, for the front end to scroll to.
    #[must_use]
    pub fn navigation_position(&self) -> Option<&NavigationPosition> {
        self.history.current.as_ref()
    }

    /// Follows the link at `offset` in document `id`: opens and activates the target and
    /// records where the user came from. An image opens in an image tab; attachments are
    /// returned as `External`.
    pub(crate) fn follow_link(
        &mut self,
        id: DocumentId,
        offset: usize,
    ) -> Result<CommandOutcome, EngineError> {
        let from = self
            .document(id)
            .ok_or(EngineError::UnknownDocument(id))?
            .path
            .clone();
        match self.definition(id, offset) {
            Some(DefinitionTarget::External(path)) if is_image_path(&path) => {
                let image = self.open_image_as(&path, OpenDisposition::Preview)?;
                self.history
                    .back
                    .push(NavigationPosition { path: from, offset });
                self.history.forward.clear();
                self.history.current = Some(NavigationPosition {
                    path: adoc_index::normalize_path(&path),
                    offset: 0,
                });
                self.emit(EngineEvent::NavigationChanged);
                Ok(CommandOutcome::OpenedImage(image))
            }
            Some(DefinitionTarget::External(path)) => Ok(CommandOutcome::External(path)),
            Some(DefinitionTarget::Location(location)) => {
                let target = self.open_as(&location.path, OpenDisposition::Preview)?;
                self.history
                    .back
                    .push(NavigationPosition { path: from, offset });
                self.history.forward.clear();
                self.history.current = Some(NavigationPosition {
                    path: location.path.clone(),
                    offset: location.range.start,
                });
                self.set_active(target);
                self.emit(EngineEvent::NavigationChanged);
                Ok(CommandOutcome::Location(location))
            }
            None => Err(EngineError::NotFound(format!(
                "nothing to follow at offset {offset}"
            ))),
        }
    }

    pub(crate) fn go_back(&mut self) -> Result<CommandOutcome, EngineError> {
        let target = self
            .history
            .back
            .pop()
            .ok_or_else(|| EngineError::NotFound("no earlier location".to_owned()))?;
        if let Some(current) = self
            .history
            .current
            .take()
            .or_else(|| self.active_position())
        {
            self.history.forward.push(current);
        }
        self.arrive(target)
    }

    pub(crate) fn go_forward(&mut self) -> Result<CommandOutcome, EngineError> {
        let target = self
            .history
            .forward
            .pop()
            .ok_or_else(|| EngineError::NotFound("no later location".to_owned()))?;
        if let Some(current) = self
            .history
            .current
            .take()
            .or_else(|| self.active_position())
        {
            self.history.back.push(current);
        }
        self.arrive(target)
    }

    /// Opens and activates the file at `position` and reveals it there. With `record`, the
    /// active position is kept in the back history, as following a link keeps it.
    pub(crate) fn go_to(
        &mut self,
        position: NavigationPosition,
        record: bool,
    ) -> Result<CommandOutcome, EngineError> {
        if record {
            let active = self.active_position();
            let from = match (self.history.current.clone(), active) {
                (Some(current), Some(active)) if current.path == active.path => Some(current),
                (_, active) => active,
            };
            if let Some(from) = from {
                self.history.back.push(from);
                self.history.forward.clear();
            }
        }
        self.arrive(position)
    }

    fn arrive(&mut self, position: NavigationPosition) -> Result<CommandOutcome, EngineError> {
        let item = self.open_item(&position.path, OpenDisposition::Preview)?;
        self.activate(item);
        self.history.current = Some(position);
        self.emit(EngineEvent::NavigationChanged);
        Ok(CommandOutcome::opened(item))
    }

    fn active_position(&self) -> Option<NavigationPosition> {
        let (path, offset) = match self.active_item()? {
            TabItem::Document(id) => (
                self.document(id)?.path.clone(),
                self.history.carets.get(&id).copied().unwrap_or(0),
            ),
            TabItem::Image(id) => (self.image(id)?.path.clone(), 0),
        };
        Some(NavigationPosition { path, offset })
    }

    /// The included file at `offset`, for a read-only inline view. Only include directives
    /// can be peeked.
    pub(crate) fn peek_include(
        &self,
        id: DocumentId,
        offset: usize,
    ) -> Result<CommandOutcome, EngineError> {
        let document = self.document(id).ok_or(EngineError::UnknownDocument(id))?;
        let on_include = document
            .parsed()
            .includes
            .iter()
            .any(|include| include.range.start <= offset && offset < include.range.end);
        if !on_include {
            return Err(EngineError::NotFound(format!(
                "no include directive at offset {offset}"
            )));
        }
        match self.definition(id, offset) {
            Some(DefinitionTarget::Location(location)) => {
                let text = match self.document_for_path(&location.path) {
                    Some(open) => self.documents[&open].text.to_string(),
                    None => match self
                        .workspace
                        .index
                        .file(&location.path)
                        .and_then(adoc_index::FileEntry::text)
                    {
                        Some(text) => text.into_owned(),
                        None => self.read_text(&location.path)?,
                    },
                };
                Ok(CommandOutcome::Peek {
                    location: Location {
                        path: location.path,
                        range: SourceRange::new(0, text.len()),
                    },
                    text,
                })
            }
            _ => Err(EngineError::NotFound(format!(
                "the include at offset {offset} does not resolve"
            ))),
        }
    }
}
