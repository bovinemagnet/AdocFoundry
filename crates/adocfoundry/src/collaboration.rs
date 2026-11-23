//! Collaboration's transport, held by the app (COLLAB 6/8, #693): the engine runs the session
//! over the link `adoc-share` gives it, and the app keeps the other end, the listener or the
//! guest's connection, which only it may hold (`TLS_ALLOWED` in `xtask/src/layers.rs`). Nothing
//! listens until the person confirms Share; ending a session hands the transport to a background
//! task, since stopping it waits for its thread.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use adoc_share::{GuestControl, GuestLink, Host, Timings};
use gpui::{AppContext as _, Context};

use crate::app::AppState;

/// The transport the app holds while a session runs.
pub(crate) enum Transport {
    Hosting(Host),
    Joined(GuestControl),
}

/// An invitation handed out, as the Session panel shows it: only its expiry, never its secret
/// once copied.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Issued {
    pub(crate) expires: Instant,
}

#[derive(Default)]
pub struct Collaboration {
    transport: Option<Transport>,
    /// The last invitation copied.
    pub(crate) issued: Option<Issued>,
    /// The address shared on, for the Session panel.
    pub(crate) address: Option<SocketAddr>,
}

impl Collaboration {
    /// Whether nothing is listening or connected.
    #[must_use]
    pub(crate) fn is_idle(&self) -> bool {
        self.transport.is_none()
    }

    pub(crate) fn host(&self) -> Option<&Host> {
        match &self.transport {
            Some(Transport::Hosting(host)) => Some(host),
            _ => None,
        }
    }

    pub(crate) fn guest(&self) -> Option<&GuestControl> {
        match &self.transport {
            Some(Transport::Joined(control)) => Some(control),
            _ => None,
        }
    }
}

/// An address Share may listen on, with what to call it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Interface {
    pub(crate) label: String,
    pub(crate) address: IpAddr,
}

/// The addresses Share offers: this computer only, and its address on the local network, found
/// by asking the system which local address would reach a private one (no packet is sent). Never
/// every interface at once.
pub(crate) fn interfaces() -> Vec<Interface> {
    let mut found = vec![Interface {
        label: "This computer only".to_owned(),
        address: IpAddr::V4(Ipv4Addr::LOCALHOST),
    }];
    let local = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect((Ipv4Addr::new(192, 168, 0, 1), 9))?;
            socket.local_addr()
        })
        .map(|address| address.ip());
    if let Ok(address) = local {
        if !address.is_loopback() && !address.is_unspecified() {
            found.push(Interface {
                label: format!("Local network ({address})"),
                address,
            });
        }
    }
    found
}

/// The name shown to guests: the account's, else a plain one.
pub(crate) fn own_name() -> String {
    login_name().unwrap_or_else(|| "Host".to_owned())
}

/// The name a guest joins with: the login name, else the name comments are written under.
pub(crate) fn guest_name(engine: &adoc_engine::Engine) -> String {
    login_name().unwrap_or_else(|| engine.comment_author().name)
}

fn login_name() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|name| {
            name.trim()
                .chars()
                .take(adoc_engine::protocol::MAX_NAME)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
}

/// The app's version, which each side names in its `Hello`.
const APP: &str = env!("CARGO_PKG_VERSION");

impl AppState {
    /// A waker for the transport's threads: the pump's. Under test none, since GPUI's test
    /// scheduler fails a test woken from another thread.
    fn share_waker(&self) -> Option<adoc_share::Waker> {
        #[cfg(test)]
        {
            None
        }
        #[cfg(not(test))]
        {
            Some(self.engine.share_waker())
        }
    }

    /// Starts listening on `address` and hosts a session offering `files` from `root`.
    pub(crate) fn start_hosting(
        &mut self,
        address: IpAddr,
        root: &Path,
        files: &[PathBuf],
        name: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.collaboration.is_idle() {
            return Err("a session is already running".to_owned());
        }
        let waker = self.share_waker();
        let (host, link) = Host::start(SocketAddr::new(address, 0), APP, Timings::default(), waker)
            .map_err(|error| error.to_string())?;
        let bound = host.address();
        if let Err(error) = self.engine.host_session(Box::new(link), root, files, name) {
            drop_later(Transport::Hosting(host), cx);
            return Err(error.to_string());
        }
        self.collaboration.transport = Some(Transport::Hosting(host));
        self.collaboration.address = Some(bound);
        cx.notify();
        Ok(())
    }

    /// A new invitation as one line to copy, with how long it lasts.
    pub(crate) fn new_invitation(&mut self) -> Result<(String, Duration), String> {
        let host = self
            .collaboration
            .host()
            .ok_or_else(|| "nothing is being shared".to_owned())?;
        let invitation = host.invite().map_err(|error| error.to_string())?;
        let lifetime = Timings::default().invite_lifetime;
        self.collaboration.issued = Some(Issued {
            expires: Instant::now() + lifetime,
        });
        Ok((invitation.to_string(), lifetime))
    }

    /// Joins a session through a connection the person has just authenticated.
    pub(crate) fn joined(
        &mut self,
        link: GuestLink,
        control: GuestControl,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.collaboration.is_idle() {
            drop_later(Transport::Joined(control), cx);
            return Err("a session is already running".to_owned());
        }
        if let Err(error) = self.engine.join_session(Box::new(link)) {
            drop_later(Transport::Joined(control), cx);
            return Err(error.to_string());
        }
        self.collaboration.transport = Some(Transport::Joined(control));
        cx.notify();
        Ok(())
    }

    /// Closes the transport once the engine's session has ended.
    pub(crate) fn end_transport(&mut self, cx: &mut Context<Self>) {
        if let Some(transport) = self.collaboration.transport.take() {
            drop_later(transport, cx);
        }
        self.collaboration.issued = None;
        self.collaboration.address = None;
        cx.notify();
    }

    /// A guest's way back after its link dropped: reconnects with its resume token.
    pub(crate) fn reconnect(&self) {
        if let Some(control) = self.collaboration.guest() {
            control.resume();
        }
    }

    /// The waker the app's guest connection takes: the pump's, outside tests.
    pub(crate) fn guest_waker(&self) -> Option<adoc_share::Waker> {
        self.share_waker()
    }
}

/// Stops a transport on a background task: stopping waits for its thread.
fn drop_later(transport: Transport, cx: &mut Context<AppState>) {
    cx.background_spawn(async move { drop(transport) }).detach();
}

/// How long ago, or how soon, `at` is, said briefly: `in 9 min`.
pub(crate) fn minutes_left(at: Instant) -> String {
    let left = at.saturating_duration_since(Instant::now());
    if left.is_zero() {
        "expired".to_owned()
    } else {
        format!("valid for {} min more", left.as_secs().div_ceil(60))
    }
}

/// A participant's colour, from the theme's chart colours, the same in every view; the host's
/// is the first. Never the only sign of who someone is: the Session panel names them.
pub(crate) fn peer_colour(peer: adoc_engine::protocol::PeerId, cx: &gpui::App) -> gpui::Hsla {
    use gpui_component::ActiveTheme as _;
    let theme = cx.theme();
    let colours = [
        theme.chart_1,
        theme.chart_2,
        theme.chart_3,
        theme.chart_4,
        theme.chart_5,
    ];
    colours[peer.0 as usize % colours.len()]
}
