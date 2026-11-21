//! A guest's connection (design §5): TLS to the invitation's address accepting only the
//! certificate it names, then nothing sent until the person has compared the safety code and
//! goes on ([`Connecting::authenticate`]). After a drop, [`GuestControl::resume`] reconnects with
//! the resume token the host's last `Join` carried, which is held in memory only.

use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use adoc_collab::{
    protocol::{Credential, ErrorCode, Message, PeerId, Secret, PROTOCOL_VERSION},
    Link, LinkError, LinkEvent,
};
use tokio::{net::TcpStream, runtime::Runtime, sync::mpsc, time::timeout};
use tokio_rustls::{client::TlsStream, TlsConnector};

use crate::{
    conn::{self, Outbound},
    frames::{read_frame, write_message, HANDSHAKE_FRAME},
    tls, Invitation, SafetyCode, ShareError, Timings, Waker, EVENT_QUEUE,
};

/// A connection whose host has proved it holds the invited certificate, before anything is
/// sent to it.
pub struct Connecting {
    runtime: Runtime,
    stream: TlsStream<TcpStream>,
    invitation: Invitation,
    timings: Timings,
}

impl std::fmt::Debug for Connecting {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Connecting")
            .field("invitation", &self.invitation)
            .finish_non_exhaustive()
    }
}

/// The engine's side of a guest: the session's [`Link`].
pub struct GuestLink {
    shared: Arc<Shared>,
    events: mpsc::Receiver<LinkEvent>,
}

/// The application's control of a guest's connection.
pub struct GuestControl {
    shared: Arc<Shared>,
    commands: mpsc::UnboundedSender<Command>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    connection: Option<Outbound>,
    resume: Option<Secret>,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

enum Command {
    Resume,
    Drop,
    Stop,
}

/// Connects to the invitation's host and checks its certificate, sending nothing else.
pub fn connect(invitation: &Invitation, timings: Timings) -> Result<Connecting, ShareError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ShareError::Io(error.to_string()))?;
    let stream = runtime.block_on(open(invitation, timings))?;
    Ok(Connecting {
        runtime,
        stream,
        invitation: invitation.clone(),
        timings,
    })
}

async fn open(
    invitation: &Invitation,
    timings: Timings,
) -> Result<TlsStream<TcpStream>, ShareError> {
    let connector = TlsConnector::from(tls::client_config(invitation.fingerprint)?);
    let connecting = async {
        let tcp = TcpStream::connect(invitation.address)
            .await
            .map_err(|error| ShareError::Io(error.to_string()))?;
        connector
            .connect(tls::server_name(), tcp)
            .await
            .map_err(|error| {
                let wrong = error
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                    .is_some_and(|inner| matches!(inner, rustls::Error::InvalidCertificate(_)));
                if wrong {
                    ShareError::Fingerprint
                } else {
                    ShareError::Tls(error.to_string())
                }
            })
    };
    timeout(timings.handshake, connecting)
        .await
        .map_err(|_| ShareError::TimedOut)?
}

impl Connecting {
    /// The code to compare with the one the host shows before going on.
    #[must_use]
    pub fn safety_code(&self) -> SafetyCode {
        self.invitation.fingerprint.safety_code()
    }

    /// Introduces this guest as `name` with the invitation's secret, and runs the connection
    /// on a thread of its own. The host's `Join` (or its refusal) arrives on the link.
    pub fn authenticate(
        self,
        name: &str,
        app: &str,
        waker: Option<Waker>,
    ) -> Result<(GuestLink, GuestControl), ShareError> {
        let introduction = Message::Authenticate {
            credential: Credential::Invitation(self.invitation.secret.clone()),
            name: name.to_owned(),
        };
        introduction.encode().map_err(|_| ShareError::Name)?;
        let shared = Arc::new(Shared::default());
        let (events_in, events) = mpsc::channel(EVENT_QUEUE);
        let (commands, orders) = mpsc::unbounded_channel();
        let running = Arc::clone(&shared);
        let Self {
            runtime,
            stream,
            invitation,
            timings,
        } = self;
        let name = name.to_owned();
        let app = app.to_owned();
        let thread = std::thread::Builder::new()
            .name("adoc-share guest".to_owned())
            .spawn(move || {
                runtime.block_on(guest(
                    stream, invitation, name, app, timings, running, events_in, waker, orders,
                ));
            })
            .map_err(|error| ShareError::Io(error.to_string()))?;
        Ok((
            GuestLink {
                shared: Arc::clone(&shared),
                events,
            },
            GuestControl {
                shared,
                commands,
                thread: Some(thread),
            },
        ))
    }
}

#[allow(clippy::too_many_arguments)]
async fn guest(
    stream: TlsStream<TcpStream>,
    invitation: Invitation,
    name: String,
    app: String,
    timings: Timings,
    shared: Arc<Shared>,
    events: mpsc::Sender<LinkEvent>,
    waker: Option<Waker>,
    mut orders: mpsc::UnboundedReceiver<Command>,
) {
    let wake = |waker: &Option<Waker>| {
        if let Some(waker) = waker {
            waker();
        }
    };
    let mut stream = Some(stream);
    let mut credential = Credential::Invitation(invitation.secret.clone());
    let mut resumed = false;
    loop {
        if let Some(mut connected) = stream.take() {
            let reason = match introduce(&mut connected, &credential, &name, &app, timings).await {
                Err(reason) => reason,
                Ok(()) => {
                    let (outbound, queue) = conn::queue();
                    shared.state().connection = Some(outbound);
                    if resumed {
                        let _ = events.send(LinkEvent::Resumed { peer: PeerId::HOST }).await;
                        wake(&waker);
                    }
                    let keeper = Arc::clone(&shared);
                    let running = conn::run(
                        connected,
                        PeerId::HOST,
                        queue,
                        events.clone(),
                        waker.clone(),
                        timings,
                        move |message| {
                            if let Message::Join { resume, .. } = message {
                                keeper.state().resume = Some(resume.clone());
                            }
                        },
                    );
                    tokio::pin!(running);
                    loop {
                        tokio::select! {
                            reason = &mut running => break reason,
                            order = orders.recv() => match order {
                                Some(Command::Resume) => {}
                                Some(Command::Drop) => break "the connection was dropped".to_owned(),
                                Some(Command::Stop) | None => {
                                    if let Some(connection) = shared.state().connection.take() {
                                        connection.close(ErrorCode::Closed, "left");
                                    }
                                    let _ = (&mut running).await;
                                    return;
                                }
                            },
                        }
                    }
                }
            };
            shared.state().connection = None;
            let _ = events
                .send(LinkEvent::Gone {
                    peer: PeerId::HOST,
                    reason,
                })
                .await;
            wake(&waker);
        }
        match orders.recv().await {
            Some(Command::Resume) => {
                let Some(token) = shared.state().resume.clone() else {
                    continue;
                };
                match open(&invitation, timings).await {
                    Ok(reopened) => {
                        stream = Some(reopened);
                        credential = Credential::Resume(token);
                        resumed = true;
                    }
                    Err(error) => {
                        let _ = events
                            .send(LinkEvent::Gone {
                                peer: PeerId::HOST,
                                reason: error.to_string(),
                            })
                            .await;
                        wake(&waker);
                    }
                }
            }
            Some(Command::Drop) => {}
            Some(Command::Stop) | None => return,
        }
    }
}

/// `Hello` both ways, then `Authenticate`. The host's answer to it comes as the first message
/// of the connection.
async fn introduce(
    stream: &mut TlsStream<TcpStream>,
    credential: &Credential,
    name: &str,
    app: &str,
    timings: Timings,
) -> Result<(), String> {
    let handshake = async {
        let hello = Message::Hello {
            min: PROTOCOL_VERSION,
            max: PROTOCOL_VERSION,
            app: app.to_owned(),
        };
        write_message(stream, &hello)
            .await
            .map_err(|error| error.to_string())?;
        match read_frame(stream, HANDSHAKE_FRAME).await {
            Ok(Message::Hello { min, max, .. }) if (min..=max).contains(&PROTOCOL_VERSION) => {}
            Ok(Message::Error { message, .. }) => return Err(message),
            _ => return Err("the host does not speak this protocol".to_owned()),
        }
        let authenticate = Message::Authenticate {
            credential: credential.clone(),
            name: name.to_owned(),
        };
        write_message(stream, &authenticate)
            .await
            .map_err(|error| error.to_string())
    };
    timeout(timings.handshake, handshake)
        .await
        .map_err(|_| "the handshake took too long".to_owned())?
}

impl GuestControl {
    /// Reconnects after a drop, with the token from the host's last `Join`. Its `Resumed`, or
    /// another `Gone`, arrives on the link.
    pub fn resume(&self) {
        let _ = self.commands.send(Command::Resume);
    }

    /// Closes the connection as a network failure would, keeping the token to resume with.
    pub fn drop_connection(&self) {
        let _ = self.commands.send(Command::Drop);
    }

    /// Whether the connection is up.
    #[must_use]
    pub fn connected(&self) -> bool {
        self.shared.state().connection.is_some()
    }
}

impl Drop for GuestControl {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Link for GuestLink {
    fn try_recv(&mut self) -> Option<LinkEvent> {
        self.events.try_recv().ok()
    }

    fn send(&mut self, _: PeerId, message: Message) -> Result<(), LinkError> {
        let connection = self
            .shared
            .state()
            .connection
            .clone()
            .ok_or(LinkError::Closed)?;
        connection.send(&message)
    }

    fn admit(&mut self, _: PeerId) -> Result<(), LinkError> {
        Err(LinkError::Closed)
    }

    fn remove(&mut self, _: PeerId, code: ErrorCode, message: &str) {
        let mut state = self.shared.state();
        state.resume = None;
        if let Some(connection) = state.connection.take() {
            connection.close(code, message);
        }
    }
}
