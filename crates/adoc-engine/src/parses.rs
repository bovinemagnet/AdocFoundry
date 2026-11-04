//! One parse of the active page, shared (#183). The preview, the visual model, the word count
//! and the spell check all want the active page's DOM at its current version; whichever asks
//! first starts the parse, on a worker, and the others wait for it and take it from here, so a
//! pause in typing parses the page once rather than once for each.
//!
//! A parse is kept only while what it read is unchanged: the page's version, and every file
//! and setting it was assembled from (`visual_source_changed` and `visual_workspace_changed`
//! forget it, and the result of one still running).

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use adoc_dom::Dom;

use crate::{
    rendered::OwnedInput,
    worker::{self, Crashed},
    workspace::WorkerMessage,
    DocumentId, Engine,
};

/// A page parsed at one version, with the text parsed and every file it was assembled from.
pub(crate) struct SharedParse {
    pub(crate) id: DocumentId,
    pub(crate) version: u64,
    pub(crate) text: Arc<str>,
    pub(crate) dom: Arc<Dom>,
    pub(crate) sources: BTreeSet<PathBuf>,
}

#[derive(Default)]
pub(crate) struct Parses {
    /// The active page's latest parse, while nothing it read has changed.
    current: Option<Arc<SharedParse>>,
    /// The parse running whose result is to be kept: its document, version and ticket.
    running: Option<(DocumentId, u64, u64)>,
    /// A page and version whose parse crashed: nothing waits for it or parses it again.
    failed: Option<(DocumentId, u64)>,
    next_ticket: u64,
}

/// Where the active page's parse at its current version stands.
pub(crate) enum Lookup {
    Ready(Arc<SharedParse>),
    Running,
    Failed,
    Missing,
}

impl Engine {
    /// The shared parse of `id` at its current version. Only the active page has one.
    pub(crate) fn shared_parse(&self, id: DocumentId) -> Lookup {
        let Some(version) = self
            .documents
            .get(&id)
            .map(|document| document.version)
            .filter(|_| self.active == Some(id))
        else {
            return Lookup::Missing;
        };
        let parses = &self.parses;
        if let Some(parse) = parses
            .current
            .as_ref()
            .filter(|parse| parse.id == id && parse.version == version)
        {
            Lookup::Ready(parse.clone())
        } else if parses.failed == Some((id, version)) {
            Lookup::Failed
        } else if parses
            .running
            .is_some_and(|(running, at, _)| running == id && at == version)
        {
            Lookup::Running
        } else {
            Lookup::Missing
        }
    }

    /// Notes that a parse of `id` at `version` begins whose result others can share, returning
    /// the ticket its result is kept with; 0, never kept, for a page that is not active.
    pub(crate) fn share_next_parse(&mut self, id: DocumentId, version: u64) -> u64 {
        if self.active != Some(id) {
            return 0;
        }
        self.parses.next_ticket += 1;
        let ticket = self.parses.next_ticket;
        self.parses.running = Some((id, version, ticket));
        ticket
    }

    /// Parses the active page `id` on a worker, for whichever of its consumers wants it next.
    pub(crate) fn start_shared_parse(&mut self, id: DocumentId) -> bool {
        let Some(input) = self.parse_input(id) else {
            return false;
        };
        let version = input.version;
        let ticket = self.share_next_parse(id, version);
        if ticket == 0 {
            return false;
        }
        let parser = self.document_parser();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::Crashed {
            job: Crashed::Parse {
                id,
                version,
                ticket,
            },
            error,
        };
        worker::spawn(sender, crashed, move |sender| {
            let text = input.text.clone();
            let (dom, sources) = input.parse(parser.as_ref());
            let _ = sender.send(WorkerMessage::Parsed {
                id,
                version,
                ticket,
                text,
                dom: Arc::new(dom),
                sources,
            });
        });
        true
    }

    /// Keeps a parse that arrived, if it is still the one wanted.
    pub(crate) fn parse_arrived(&mut self, ticket: u64, parse: SharedParse) {
        let parses = &mut self.parses;
        if ticket == 0
            || parses.running.map(|(_, _, running)| running) != Some(ticket)
            || self.active != Some(parse.id)
        {
            return;
        }
        parses.running = None;
        parses.current = Some(Arc::new(parse));
        // Those already due were waiting for it.
        self.start_due_previews();
        self.rebuild_due_visuals();
        self.start_due_word_count();
        self.start_due_spelling();
    }

    /// Keeps a parse made on the engine thread, for the active page's other consumers.
    pub(crate) fn remember_parse(&mut self, parse: Arc<SharedParse>) {
        let current = self
            .documents
            .get(&parse.id)
            .is_some_and(|document| document.version == parse.version);
        if current && self.active == Some(parse.id) {
            self.parses.current = Some(parse);
        }
    }

    /// A shared parse panicked: its page at that version is not parsed for sharing again.
    pub(crate) fn parse_crashed(&mut self, id: DocumentId, version: u64, ticket: u64) {
        let parses = &mut self.parses;
        if ticket != 0 && parses.running.map(|(_, _, running)| running) == Some(ticket) {
            parses.running = None;
            parses.failed = Some((id, version));
        }
    }

    /// The file at `path` changed, or with `None` the catalog or index: the kept parse is
    /// dropped if it read what changed, and a running one always, as what it reads is not yet
    /// known.
    pub(crate) fn parses_changed(&mut self, path: Option<&Path>) {
        let documents = &self.documents;
        let parses = &mut self.parses;
        let read = |parse: &SharedParse| {
            path.is_none_or(|path| {
                parse.sources.contains(path)
                    || documents
                        .get(&parse.id)
                        .is_some_and(|document| document.path == path)
            })
        };
        if parses.current.as_deref().is_some_and(read) {
            parses.current = None;
        }
        parses.running = None;
        parses.failed = None;
    }

    pub(crate) fn forget_parse(&mut self, id: DocumentId) {
        let parses = &mut self.parses;
        if parses.current.as_ref().is_some_and(|parse| parse.id == id) {
            parses.current = None;
        }
        if parses.running.is_some_and(|(running, _, _)| running == id) {
            parses.running = None;
        }
    }
}

impl OwnedInput {
    /// Parses the input on this thread, as a parse to share.
    pub(crate) fn parse_shared(
        self,
        id: DocumentId,
        parser: &dyn adoc_dom::DocumentParser,
    ) -> SharedParse {
        let (version, text) = (self.version, self.text.clone());
        let (dom, sources) = self.parse(parser);
        SharedParse {
            id,
            version,
            text,
            dom: Arc::new(dom),
            sources,
        }
    }
}
