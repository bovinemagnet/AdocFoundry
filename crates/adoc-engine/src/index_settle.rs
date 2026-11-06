//! The index settle (#238). Once typing pauses for [`crate::INDEX_SETTLE`], a worker parses the
//! page and prepares its index entry (`adoc_index::PreparedFile`: anchors, section ids, symbols
//! and include targets), so the engine thread only files it and diagnoses. The answer is kept
//! only while the page is still at the version and path parsed; a command that reads the index
//! while the worker runs indexes the page at once instead, and the answer is dropped.

use std::{collections::BTreeMap, path::PathBuf};

use adoc_core::Document;
use adoc_index::PreparedFile;

use crate::{
    document::uri_for,
    worker::{self, Crashed},
    workspace::WorkerMessage,
    DocumentId, Engine,
};

/// The pages whose settle is running on a worker.
#[derive(Default)]
pub(crate) struct Settling {
    /// Each page and the ticket its answer is kept with.
    running: BTreeMap<DocumentId, u64>,
    next_ticket: u64,
    /// What filing an answer took out of the index, and answers dropped, freed by the next
    /// settle's worker rather than here: a large page's parse is slow to free.
    garbage: Vec<Box<dyn Send>>,
}

/// A page parsed and prepared for the index by a worker.
#[derive(Debug)]
pub(crate) struct Prepared {
    pub(crate) id: DocumentId,
    pub(crate) version: u64,
    pub(crate) ticket: u64,
    pub(crate) path: PathBuf,
    /// The page's parse, for [`crate::OpenDocument::parsed`].
    pub(crate) parsed: Document,
    pub(crate) entry: PreparedFile,
}

impl Engine {
    /// Parses and prepares `id` for the index on a worker.
    pub(crate) fn start_settle(&mut self, id: DocumentId) {
        let Some(document) = self
            .documents
            .get(&id)
            .filter(|document| document.indexed())
        else {
            return;
        };
        let (version, path, text) = (
            document.version,
            document.path.clone(),
            // Shares the rope's chunks: the copy into one string is made on the worker.
            document.text.clone(),
        );
        let shared = self.workspace.index.shared_path(&path);
        self.settling.next_ticket += 1;
        let ticket = self.settling.next_ticket;
        self.settling.running.insert(id, ticket);
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::Crashed {
            job: Crashed::IndexSettle { id, ticket },
            error,
        };
        let garbage = std::mem::take(&mut self.settling.garbage);
        worker::spawn(sender, crashed, move |sender| {
            drop(garbage);
            let text = text.to_string();
            let parsed = adoc_parser::parse(&uri_for(&path), &text).document;
            let entry = PreparedFile::new(&path, shared, parsed.clone());
            let _ = sender.send(WorkerMessage::IndexPrepared(Box::new(Prepared {
                id,
                version,
                ticket,
                path,
                parsed,
                entry,
            })));
        });
    }

    /// Files a page a worker prepared, if it is still the page as it stands.
    pub(crate) fn settle_arrived(&mut self, prepared: Box<Prepared>) {
        let (id, version, ticket) = (prepared.id, prepared.version, prepared.ticket);
        let wanted = self.settling.running.get(&id) == Some(&ticket);
        let current = self.documents.get(&id).is_some_and(|document| {
            document.version == version && document.path == prepared.path && document.indexed()
        });
        if wanted {
            self.settling.running.remove(&id);
        }
        if !(wanted && current) {
            // Indexed at once by a command since, superseded, or the page was typed into again
            // (and is pending once more), moved or closed.
            self.settling.garbage.push(prepared);
            return;
        }
        let Prepared {
            path,
            parsed,
            entry,
            ..
        } = *prepared;
        let document = self.documents.get_mut(&id).expect("checked above");
        // Something may have read the parse already, on this thread.
        if let Err(parsed) = document.parsed.set(parsed) {
            self.settling.garbage.push(Box::new(parsed));
        }
        let before = adoc_lang::diagnosed_with(&self.workspace, &path);
        let retired = self.workspace.index.insert_prepared(entry);
        self.settling.garbage.push(Box::new(retired));
        self.graph_file_changed(&path);
        self.refresh_diagnosed_with(&path, before);
    }

    /// A settle's worker panicked: the index keeps the page as it was until it is next edited.
    pub(crate) fn settle_crashed(&mut self, id: DocumentId, ticket: u64) {
        if self.settling.running.get(&id) == Some(&ticket) {
            self.settling.running.remove(&id);
        }
    }

    /// The pages whose settle is running, which are no longer listed as pending: the running
    /// settles are forgotten, their answers dropped when they come.
    pub(crate) fn take_settling(&mut self) -> Vec<DocumentId> {
        std::mem::take(&mut self.settling.running)
            .into_keys()
            .collect()
    }
}
