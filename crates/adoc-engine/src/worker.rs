//! Worker threads that always report back. A worker that panics sends the failure its job
//! reports instead of its result, so nothing waits for it: not `workers_running`, nor a
//! diagram's slot, an export's stage or a job running flag.

use std::{
    any::Any,
    panic::{self, AssertUnwindSafe},
    sync::{mpsc::Sender, Arc, OnceLock},
    thread,
};

use crate::{commands::ids, workspace::WorkerMessage, DocumentId, Engine, EngineEvent};

/// What a front end gives [`Engine::set_waker`] to be called when the engine has something new.
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

/// The front end's waker, once it gives one: shared by every sender, so a sender made before
/// it was given calls it too.
pub(crate) type SharedWaker = Arc<OnceLock<Waker>>;

/// Calls the front end's waker, if it gave one.
pub(crate) fn wake(waker: &SharedWaker) {
    if let Some(waker) = waker.get() {
        waker();
    }
}

/// The engine a worker was sending to has been dropped.
#[derive(Debug)]
pub(crate) struct EngineGone;

/// Sends a worker's messages to the engine and wakes the front end, which pumps them (#222).
#[derive(Clone)]
pub(crate) struct WorkerSender {
    sender: Sender<WorkerMessage>,
    waker: SharedWaker,
}

impl WorkerSender {
    pub(crate) fn new(sender: Sender<WorkerMessage>, waker: SharedWaker) -> Self {
        Self { sender, waker }
    }

    pub(crate) fn send(&self, message: WorkerMessage) -> Result<(), EngineGone> {
        let sent = self.sender.send(message).map_err(|_| EngineGone);
        wake(&self.waker);
        sent
    }
}

/// A job with no failure of its own to report, named by what to settle when its worker panics.
pub(crate) enum Crashed {
    /// Parsing a page for the preview, its result shared with `ticket`.
    Preview {
        id: DocumentId,
        version: u64,
        ticket: u64,
    },
    /// Parsing the active page for its consumers to share (`parses.rs`).
    Parse {
        id: DocumentId,
        version: u64,
        ticket: u64,
    },
    /// Parsing a page for the index after a pause in typing, its answer kept with `ticket`.
    IndexSettle { id: DocumentId, ticket: u64 },
    /// Counting the active page's words. Nothing waits for it, and the page is counted again
    /// once it changes.
    WordCount,
    /// Spell checking `id` at `version` with the word lists of `generation`.
    Spelling {
        id: DocumentId,
        version: u64,
        generation: u64,
    },
    /// The whole workspace's problems, pass `generation`.
    Problems { generation: u64 },
    /// The whole quality audit, build `generation`. The parts found before stay.
    Audit { generation: u64 },
    /// The whole link graph, build `generation`. The graph built before stays.
    Graph { generation: u64 },
}

impl Engine {
    /// Settles a job whose worker panicked. The preview and the problems say so; a word
    /// count or spell check that fails is only missing, its panic printed.
    pub(crate) fn worker_crashed(&mut self, job: Crashed, error: String) {
        let failed = match job {
            // The preview keeps the view it had until the page next changes.
            Crashed::Preview {
                id,
                version,
                ticket,
            } => {
                self.parse_crashed(id, version, ticket);
                Some((ids::VIEW_TOGGLE_PREVIEW, "the preview"))
            }
            // Its consumers go without, as each would had it parsed the page itself.
            Crashed::Parse {
                id,
                version,
                ticket,
            } => {
                self.parse_crashed(id, version, ticket);
                None
            }
            // The index keeps the page as it was, as it would had the parse panicked here.
            Crashed::IndexSettle { id, ticket } => {
                self.settle_crashed(id, ticket);
                None
            }
            Crashed::WordCount => None,
            Crashed::Spelling {
                id,
                version,
                generation,
            } => {
                self.spelling_crashed(id, version, generation);
                None
            }
            Crashed::Graph { generation } => {
                self.graph_crashed(generation);
                Some((ids::VIEW_GRAPH, "building the link graph"))
            }
            Crashed::Problems { generation } => {
                self.problems_crashed(generation);
                Some((ids::WORKSPACE_REINDEX, "finding the workspace's problems"))
            }
            Crashed::Audit { generation } => {
                self.audit_crashed(generation);
                Some((ids::WORKSPACE_REINDEX, "auditing the workspace"))
            }
        };
        if let Some((command, what)) = failed {
            self.emit(EngineEvent::CommandFailed {
                command,
                error: format!("{what} crashed: {error}"),
            });
        }
    }
}

/// Runs `body` on a new thread; it sends its own result through the sender it is given. If it
/// panics, the message `crashed` builds from what the panic said is sent instead.
pub(crate) fn spawn<B, C>(sender: WorkerSender, crashed: C, body: B)
where
    B: FnOnce(&WorkerSender) + Send + 'static,
    C: FnOnce(String) -> WorkerMessage + Send + 'static,
{
    thread::spawn(move || {
        if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| body(&sender))) {
            let _ = sender.send(crashed(panic_message(payload.as_ref())));
        }
    });
}

/// Runs `body`, returning what it said if it panics: for a thread that outlives its jobs.
pub(crate) fn guarded<T>(body: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(body)).map_err(|payload| panic_message(payload.as_ref()))
}

/// The text a panic was raised with.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no reason given".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_s_message_is_kept() {
        let message = |payload: Box<dyn Any + Send>| panic_message(payload.as_ref());
        assert_eq!(message(Box::new("plain")), "plain");
        assert_eq!(message(Box::new(format!("with {}", 3))), "with 3");
        assert_eq!(message(Box::new(7)), "no reason given");
    }

    #[test]
    fn a_worker_that_panics_sends_its_failure() {
        let (sender, receiver) = std::sync::mpsc::channel();
        spawn(
            WorkerSender::new(sender, SharedWaker::default()),
            |error| WorkerMessage::HtmlExported {
                output: "page.html".into(),
                result: Err(error),
            },
            |_| panic!("gone"),
        );
        match receiver.recv().unwrap() {
            WorkerMessage::HtmlExported { result, .. } => assert_eq!(result.unwrap_err(), "gone"),
            _ => panic!("another message"),
        }
    }
}
