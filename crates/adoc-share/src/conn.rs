//! One authenticated connection: a reader task hands each message to the engine's queue, and a
//! writer sends what the engine queued, heartbeats while quiet, and an `Error` before closing.
//! Neither ever waits for the engine's thread: the engine queues without waiting
//! ([`Outbound::send`]), and a peer whose queue is full is closed as slow.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use adoc_collab::{
    protocol::{ErrorCode, Message, PeerId, MAX_FRAME, MAX_QUEUED_BYTES, MAX_QUEUED_FRAMES},
    LinkError, LinkEvent,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, watch},
    time::timeout,
};

use crate::{
    frames::{read_frame, write_frame, write_message, ReadError},
    Timings, Waker,
};

/// How long a closing connection may take to say why.
const FAREWELL: Duration = Duration::from_secs(1);

type Closing = Option<(ErrorCode, String)>;

/// The engine's side of a connection's queue.
#[derive(Clone)]
pub(crate) struct Outbound {
    frames: mpsc::Sender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    close: Arc<watch::Sender<Closing>>,
}

/// The connection's side of its queue.
pub(crate) struct Queue {
    frames: mpsc::Receiver<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    close: watch::Receiver<Closing>,
}

pub(crate) fn queue() -> (Outbound, Queue) {
    let (frames, receiver) = mpsc::channel(MAX_QUEUED_FRAMES);
    let queued = Arc::new(AtomicUsize::new(0));
    let (close, closing) = watch::channel(None);
    (
        Outbound {
            frames,
            queued: Arc::clone(&queued),
            close: Arc::new(close),
        },
        Queue {
            frames: receiver,
            queued,
            close: closing,
        },
    )
}

impl Outbound {
    /// Queues `message` without waiting. Over [`MAX_QUEUED_FRAMES`] or [`MAX_QUEUED_BYTES`] the
    /// connection is closed as slow.
    pub(crate) fn send(&self, message: &Message) -> Result<(), LinkError> {
        let frame = message.encode().map_err(LinkError::Frame)?;
        let size = frame.len();
        if self.queued.fetch_add(size, Ordering::SeqCst) + size > MAX_QUEUED_BYTES {
            self.queued.fetch_sub(size, Ordering::SeqCst);
            self.close(ErrorCode::Slow, "too much waiting to be sent");
            return Err(LinkError::Slow);
        }
        match self.frames.try_send(frame) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.queued.fetch_sub(size, Ordering::SeqCst);
                self.close(ErrorCode::Slow, "too much waiting to be sent");
                Err(LinkError::Slow)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.queued.fetch_sub(size, Ordering::SeqCst);
                Err(LinkError::Closed)
            }
        }
    }

    /// Closes the connection, telling the peer `code` if it is still reading.
    pub(crate) fn close(&self, code: ErrorCode, message: &str) {
        self.close.send_if_modified(|closing| {
            if closing.is_some() {
                return false;
            }
            *closing = Some((code, message.to_owned()));
            true
        });
    }
}

/// Runs an authenticated connection until it ends, returning why. Each message read goes to
/// `events` as from `peer`, `seen` looking at it first; heartbeats and handshake messages never
/// do, and a handshake message, a malformed frame or silence past `timings.link_timeout`
/// ends the connection.
pub(crate) async fn run<S>(
    stream: S,
    peer: PeerId,
    mut queue: Queue,
    events: mpsc::Sender<LinkEvent>,
    waker: Option<Waker>,
    timings: Timings,
    mut seen: impl FnMut(&Message) + Send + 'static,
) -> String
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (finished, mut reader_done) = oneshot::channel::<String>();
    let link_timeout = timings.link_timeout;
    let _reading = AbortOnDrop(tokio::spawn(async move {
        let reason = loop {
            let message = match timeout(link_timeout, read_frame(&mut reader, MAX_FRAME)).await {
                Err(_) => break "the link was silent for too long".to_owned(),
                Ok(Err(ReadError::Closed)) => break "the connection closed".to_owned(),
                Ok(Err(ReadError::Frame(error))) => break format!("a malformed frame: {error}"),
                Ok(Ok(message)) => message,
            };
            match message {
                Message::Heartbeat => continue,
                Message::Hello { .. } | Message::Authenticate { .. } => {
                    break "a handshake message after the handshake".to_owned()
                }
                _ => {}
            }
            seen(&message);
            let leaving = matches!(message, Message::Leave { .. });
            if events
                .send(LinkEvent::Message { peer, message })
                .await
                .is_err()
            {
                break "the session ended".to_owned();
            }
            if let Some(waker) = &waker {
                waker();
            }
            if leaving {
                break "left".to_owned();
            }
        };
        let _ = finished.send(reason);
    }));

    let mut heartbeat = tokio::time::interval(timings.heartbeat);
    heartbeat.tick().await;
    let reason = loop {
        let closing = queue.close.borrow_and_update().clone();
        if let Some((code, message)) = closing {
            let farewell = Message::Error {
                code,
                message: message.clone(),
            };
            let _ = timeout(FAREWELL, write_message(&mut writer, &farewell)).await;
            break message;
        }
        // A queued frame counts against the queue's bytes until written; a heartbeat never did.
        // A close comes first: removing a peer signals it, then drops the queue's sender, and
        // the farewell must not be lost to the queue's end.
        let (frame, queued) = tokio::select! {
            biased;
            _ = queue.close.changed() => continue,
            frame = queue.frames.recv() => match frame {
                Some(frame) => (frame, true),
                None if queue.close.borrow().is_some() => continue,
                None => break "the session ended".to_owned(),
            },
            _ = heartbeat.tick() => match Message::Heartbeat.encode() {
                Ok(frame) => (frame, false),
                Err(_) => continue,
            },
            reason = &mut reader_done => break reason.unwrap_or_default(),
        };
        let written = tokio::select! {
            written = write_frame(&mut writer, &frame) => written.is_ok(),
            _ = queue.close.changed() => continue,
        };
        if queued {
            queue.queued.fetch_sub(frame.len(), Ordering::SeqCst);
        }
        if !written {
            break "the connection closed".to_owned();
        }
    };
    reason
}

/// Ends the reader with the connection, however the connection's future ends: the reader holds
/// half of the stream, which would otherwise keep the connection open.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
