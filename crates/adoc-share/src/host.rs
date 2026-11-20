//! The host's listener (design §5, §8). It listens only on the address given, never on every
//! interface; each connection must complete TLS, a `Hello` and an `Authenticate` within the
//! handshake time, holding frames to [`HANDSHAKE_FRAME`] until then. A valid invitation makes a
//! knock the engine admits or refuses; nothing else is sent before Admit. A valid resume token
//! puts a dropped peer back. Everything runs on a thread of its own, with a current-thread
//! runtime; the engine's thread only queues and takes events, never waiting.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use adoc_collab::{
    protocol::{Credential, ErrorCode, Message, PeerId, Role, SessionId, PROTOCOL_VERSION},
    Link, LinkError, LinkEvent,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
    time::timeout,
};
use tokio_rustls::{server::TlsStream, TlsAcceptor};

use crate::{
    conn::{self, Outbound, Queue},
    frames::{read_frame, write_message, HANDSHAKE_FRAME},
    invite::{hash, random_secret, Invitations},
    tls, Fingerprint, HostIdentity, Invitation, SafetyCode, ShareError, Timings, Waker,
    EVENT_QUEUE, MAX_PENDING,
};

/// How long stopping waits for connections to say goodbye.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// The application's own control of a host: invitations, its identity, stopping.
pub struct Host {
    inner: Arc<Inner>,
    address: SocketAddr,
    fingerprint: Fingerprint,
    stop: Option<watch::Sender<bool>>,
    thread: Option<JoinHandle<()>>,
}

/// The engine's side of a host: the session's [`Link`].
pub struct HostLink {
    inner: Arc<Inner>,
    events: mpsc::Receiver<LinkEvent>,
}

struct Inner {
    state: Mutex<State>,
    timings: Timings,
    app: String,
}

struct State {
    session: SessionId,
    invitations: Invitations,
    next_peer: u32,
    slots: BTreeMap<PeerId, Slot>,
    /// Connections accepted and not yet admitted or turned away.
    pending: usize,
    stopped: bool,
    next_generation: u64,
}

#[derive(Default)]
struct Slot {
    /// The hash of the peer's resume token, once admitted.
    resume: Option<[u8; 32]>,
    /// A knock waiting for the engine's decision.
    waiting: Option<oneshot::Sender<Queue>>,
    /// The live connection, and its generation.
    connection: Option<(u64, Outbound)>,
}

impl Inner {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Host {
    /// Starts listening on `bind`, which must name one interface's address (port 0 lets the
    /// system choose). `waker` is called whenever an event arrives for the [`HostLink`].
    pub fn start(
        bind: SocketAddr,
        app: &str,
        timings: Timings,
        waker: Option<Waker>,
    ) -> Result<(Self, HostLink), ShareError> {
        if bind.ip().is_unspecified() {
            return Err(ShareError::Address(
                "the host listens on one chosen address, never on every interface".to_owned(),
            ));
        }
        let identity = HostIdentity::generate()?;
        let config = tls::server_config(&identity)?;
        let listener =
            std::net::TcpListener::bind(bind).map_err(|error| ShareError::Io(error.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| ShareError::Io(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| ShareError::Io(error.to_string()))?;
        let mut session = [0; 16];
        session.copy_from_slice(&random_secret()?.0[..16]);
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                session: SessionId(session),
                invitations: Invitations::default(),
                next_peer: 1,
                slots: BTreeMap::new(),
                pending: 0,
                stopped: false,
                next_generation: 0,
            }),
            timings,
            app: app.to_owned(),
        });
        let (events_in, events) = mpsc::channel(EVENT_QUEUE);
        let (stop, stopping) = watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ShareError::Io(error.to_string()))?;
        let serving = Arc::clone(&inner);
        let thread = std::thread::Builder::new()
            .name("adoc-share host".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = TcpListener::from_std(listener) else {
                        return;
                    };
                    listen(
                        listener,
                        TlsAcceptor::from(config),
                        serving,
                        events_in,
                        waker,
                        stopping,
                    )
                    .await;
                });
            })
            .map_err(|error| ShareError::Io(error.to_string()))?;
        Ok((
            Self {
                inner: Arc::clone(&inner),
                address,
                fingerprint: identity.fingerprint(),
                stop: Some(stop),
                thread: Some(thread),
            },
            HostLink { inner, events },
        ))
    }

    /// The address and port listened on.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    #[must_use]
    pub fn safety_code(&self) -> SafetyCode {
        self.fingerprint.safety_code()
    }

    /// A new single-use invitation, valid for `timings.invite_lifetime`.
    pub fn invite(&self) -> Result<Invitation, ShareError> {
        let mut state = self.inner.state();
        if state.stopped {
            return Err(ShareError::Stopped);
        }
        let secret = state
            .invitations
            .issue(Instant::now(), self.inner.timings.invite_lifetime)?;
        Ok(Invitation {
            address: self.address,
            fingerprint: self.fingerprint,
            secret,
        })
    }

    /// Stops sharing: invitations are forgotten, every connection is closed and the listener
    /// shut. Waits for the transport's thread.
    pub fn stop(&mut self) {
        {
            let mut state = self.inner.state();
            state.stopped = true;
            state.invitations.clear();
            for slot in std::mem::take(&mut state.slots).into_values() {
                if let Some((_, connection)) = slot.connection {
                    connection.close(ErrorCode::Closed, "the host stopped sharing");
                }
            }
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn listen(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    inner: Arc<Inner>,
    events: mpsc::Sender<LinkEvent>,
    waker: Option<Waker>,
    mut stopping: watch::Receiver<bool>,
) {
    let mut connections = JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stopping.changed() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
        };
        let Ok((tcp, _)) = accepted else {
            continue;
        };
        {
            let mut state = inner.state();
            if state.stopped || state.pending >= MAX_PENDING {
                continue;
            }
            state.pending += 1;
        }
        connections.spawn(serve(
            tcp,
            acceptor.clone(),
            Arc::clone(&inner),
            events.clone(),
            waker.clone(),
        ));
    }
    drop(listener);
    let _ = timeout(STOP_WAIT, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
}

/// Releases a connection's place among those pending when dropped.
struct PendingPlace(Option<Arc<Inner>>);

impl PendingPlace {
    fn release(&mut self) {
        if let Some(inner) = self.0.take() {
            let mut state = inner.state();
            state.pending = state.pending.saturating_sub(1);
        }
    }
}

impl Drop for PendingPlace {
    fn drop(&mut self) {
        self.release();
    }
}

async fn serve(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    inner: Arc<Inner>,
    events: mpsc::Sender<LinkEvent>,
    waker: Option<Waker>,
) {
    let mut place = PendingPlace(Some(Arc::clone(&inner)));
    let handshake = inner.timings.handshake;
    let Ok(Ok(mut stream)) = timeout(handshake, acceptor.accept(tcp)).await else {
        return;
    };
    let Ok(Some(entry)) = timeout(handshake, introduce(&mut stream, &inner)).await else {
        return;
    };
    let (peer, queue, generation) = match entry {
        Entry::Invited { name } => {
            match knock(&mut stream, &inner, name, &events, waker.as_ref()).await {
                Some(admitted) => admitted,
                None => return,
            }
        }
        Entry::Resumed {
            peer,
            queue,
            generation,
        } => {
            if events.send(LinkEvent::Resumed { peer }).await.is_err() {
                return;
            }
            if let Some(waker) = &waker {
                waker();
            }
            (peer, queue, generation)
        }
    };
    place.release();
    let reason = conn::run(
        stream,
        peer,
        queue,
        events.clone(),
        waker.clone(),
        inner.timings,
        |_| {},
    )
    .await;
    let current = {
        let mut state = inner.state();
        match state.slots.get_mut(&peer) {
            Some(slot)
                if slot
                    .connection
                    .as_ref()
                    .is_some_and(|(held, _)| *held == generation) =>
            {
                slot.connection = None;
                true
            }
            _ => false,
        }
    };
    if current && events.send(LinkEvent::Gone { peer, reason }).await.is_ok() {
        if let Some(waker) = &waker {
            waker();
        }
    }
}

enum Entry {
    Invited {
        name: String,
    },
    Resumed {
        peer: PeerId,
        queue: Queue,
        generation: u64,
    },
}

/// The handshake: `Hello` both ways, then the guest's `Authenticate`. An invitation's secret is
/// used up here; a resume token is replaced, and the `Join` carrying the new one queued first.
async fn introduce(stream: &mut TlsStream<TcpStream>, inner: &Inner) -> Option<Entry> {
    let Ok(Message::Hello { min, max, .. }) = read_frame(stream, HANDSHAKE_FRAME).await else {
        return None;
    };
    if !(min..=max).contains(&PROTOCOL_VERSION) {
        let _ = write_message(
            stream,
            &Message::Error {
                code: ErrorCode::Incompatible,
                message: format!("this host speaks protocol version {PROTOCOL_VERSION}"),
            },
        )
        .await;
        return None;
    }
    let hello = Message::Hello {
        min: PROTOCOL_VERSION,
        max: PROTOCOL_VERSION,
        app: inner.app.clone(),
    };
    write_message(stream, &hello).await.ok()?;
    let Ok(Message::Authenticate { credential, name }) = read_frame(stream, HANDSHAKE_FRAME).await
    else {
        return None;
    };
    let entry = {
        let mut state = inner.state();
        if state.stopped {
            None
        } else {
            match credential {
                Credential::Invitation(secret) => state
                    .invitations
                    .redeem(&secret, Instant::now())
                    .then_some(Entry::Invited { name }),
                Credential::Resume(token) => resume(&mut state, &token),
            }
        }
    };
    if entry.is_none() {
        let _ = write_message(
            stream,
            &Message::Error {
                code: ErrorCode::Denied,
                message: "the invitation or token is not valid".to_owned(),
            },
        )
        .await;
    }
    entry
}

/// A resume token's peer, given a new token and connection, the old one closed.
fn resume(state: &mut State, token: &adoc_collab::protocol::Secret) -> Option<Entry> {
    let wanted = hash(token);
    let session = state.session;
    let generation = state.next_generation;
    let (peer, slot) = state
        .slots
        .iter_mut()
        .find(|(_, slot)| slot.resume == Some(wanted))?;
    let fresh = random_secret().ok()?;
    let (outbound, queue) = conn::queue();
    outbound
        .send(&Message::Join {
            peer: *peer,
            role: Role::Viewer,
            session,
            resume: fresh.clone(),
        })
        .ok()?;
    if let Some((_, old)) = slot.connection.replace((generation, outbound)) {
        old.close(ErrorCode::Closed, "the session resumed elsewhere");
    }
    slot.resume = Some(hash(&fresh));
    let peer = *peer;
    state.next_generation += 1;
    Some(Entry::Resumed {
        peer,
        queue,
        generation,
    })
}

/// An invited guest knocks and waits for the engine's decision, sending nothing meanwhile.
/// Anything it sends before then, or closing, abandons the knock.
async fn knock(
    stream: &mut TlsStream<TcpStream>,
    inner: &Inner,
    name: String,
    events: &mpsc::Sender<LinkEvent>,
    waker: Option<&Waker>,
) -> Option<(PeerId, Queue, u64)> {
    let (decided, decision) = oneshot::channel();
    let peer = {
        let mut state = inner.state();
        let peer = PeerId(state.next_peer);
        state.next_peer += 1;
        state.slots.insert(
            peer,
            Slot {
                waiting: Some(decided),
                ..Slot::default()
            },
        );
        peer
    };
    events.send(LinkEvent::Knock { peer, name }).await.ok()?;
    if let Some(waker) = waker {
        waker();
    }
    let admitted = tokio::select! {
        decision = timeout(inner.timings.admit_wait, decision) => decision.ok().and_then(Result::ok),
        _ = read_frame(stream, HANDSHAKE_FRAME) => None,
    };
    let Some(queue) = admitted else {
        let removed = inner.state().slots.remove(&peer).is_some();
        let _ = write_message(
            stream,
            &Message::Error {
                code: ErrorCode::Denied,
                message: "the host did not let you in".to_owned(),
            },
        )
        .await;
        if removed {
            let _ = events
                .send(LinkEvent::Gone {
                    peer,
                    reason: "not admitted".to_owned(),
                })
                .await;
            if let Some(waker) = waker {
                waker();
            }
        }
        return None;
    };
    let generation = inner
        .state()
        .slots
        .get(&peer)
        .and_then(|slot| slot.connection.as_ref().map(|(generation, _)| *generation))?;
    Some((peer, queue, generation))
}

impl Link for HostLink {
    fn try_recv(&mut self) -> Option<LinkEvent> {
        self.events.try_recv().ok()
    }

    fn send(&mut self, peer: PeerId, message: Message) -> Result<(), LinkError> {
        let state = self.inner.state();
        let outbound = state
            .slots
            .get(&peer)
            .and_then(|slot| slot.connection.as_ref())
            .map(|(_, outbound)| outbound.clone())
            .ok_or(LinkError::Closed)?;
        drop(state);
        outbound.send(&message)
    }

    fn admit(&mut self, peer: PeerId) -> Result<(), LinkError> {
        let mut state = self.inner.state();
        let session = state.session;
        let generation = state.next_generation;
        state.next_generation += 1;
        let slot = state.slots.get_mut(&peer).ok_or(LinkError::Closed)?;
        let waiting = slot.waiting.take().ok_or(LinkError::Closed)?;
        let token = random_secret().map_err(|_| LinkError::Closed)?;
        let (outbound, queue) = conn::queue();
        outbound.send(&Message::Join {
            peer,
            role: Role::Viewer,
            session,
            resume: token.clone(),
        })?;
        slot.resume = Some(hash(&token));
        slot.connection = Some((generation, outbound));
        if waiting.send(queue).is_err() {
            state.slots.remove(&peer);
            return Err(LinkError::Closed);
        }
        Ok(())
    }

    fn remove(&mut self, peer: PeerId, code: ErrorCode, message: &str) {
        let slot = self.inner.state().slots.remove(&peer);
        if let Some((_, connection)) = slot.and_then(|slot| slot.connection) {
            connection.close(code, message);
        }
    }
}
