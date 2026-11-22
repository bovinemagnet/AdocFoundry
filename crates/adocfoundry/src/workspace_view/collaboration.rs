//! Collaboration in the workspace window (COLLAB 6/8, #693): Share Workspace…, Join Session…,
//! Copy Invitation, Stop Sharing and Leave Session, and the Session panel, a sidebar tab shown
//! while a session runs. Every decision is the engine's, through its collaboration commands; the
//! app holds only the transport (`crate::collaboration`). Nothing listens until Share is
//! confirmed, and a guest sends nothing until the safety code is compared.

use std::{cell::RefCell, net::IpAddr, rc::Rc};

use adoc_engine::{
    protocol::{commit_email, FileKind, PeerId, Role},
    CollaborationStep, SharePlan, SharedSaveState,
};
use adoc_share::{Connecting, Invitation, ShareError, Timings};
use gpui::{AnyElement, ClipboardItem};

use super::*;
use crate::editor::bindings::{
    CopyInvitation, JoinSession, LeaveSession, ShareWorkspace, StopFollowing, StopSharing,
};

/// The arguments naming `peer` to the commands that act on a participant.
fn peer_args(peer: PeerId) -> CommandArgs {
    CommandArgs::Custom(serde_json::json!({ "peer": peer.0 }))
}

/// A role as people read it.
fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "Host",
        Role::Editor => "Editor",
        Role::Viewer => "Viewer",
    }
}

/// A participant's colour, as their caret is drawn in, beside their name.
fn swatch(peer: PeerId, cx: &App) -> gpui::Div {
    div()
        .size_2()
        .rounded_full()
        .flex_none()
        .bg(crate::collaboration::peer_colour(peer, cx))
}

/// Where `peer` is, said in words (` · page.adoc, line 12`), so their colour is never all that
/// tells their caret apart: the line of their caret in a page open here, else only the shared
/// page they say they are on (`place`); empty when they are on none.
fn caret_place(engine: &adoc_engine::Engine, peer: PeerId, place: Option<u64>) -> String {
    let manifest = engine.session_manifest();
    manifest
        .iter()
        .find_map(|entry| {
            let id = engine.session_document(entry.file)?;
            let caret = engine
                .peer_carets(id)
                .into_iter()
                .find(|caret| caret.peer == peer)?;
            let text = &engine.document(id)?.text;
            let line =
                text.byte_to_line_idx(caret.head.min(text.len()), ropey::LineType::LF_CR) + 1;
            Some(format!(" · {}, line {line}", entry.path))
        })
        .or_else(|| {
            let entry = manifest.iter().find(|entry| Some(entry.file) == place)?;
            Some(format!(" · {}", entry.path))
        })
        .unwrap_or_default()
}

/// How a shared file stands, said briefly, for the panel and the status bar.
pub(super) fn save_state_label(state: &SharedSaveState) -> &'static str {
    match state {
        SharedSaveState::Saved { .. } => "Saved",
        SharedSaveState::Unsaved => "Unsaved changes",
        SharedSaveState::Conflict => "Changed on disk",
        SharedSaveState::Unknown => "Not known saved",
    }
}

impl WorkspaceView {
    /// Share Workspace…: shows what would be shared and where, and listens only once a place is
    /// chosen.
    pub(super) fn on_share_workspace(
        &mut self,
        _: &ShareWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.invoke(ids::COLLAB_SHARE, CommandArgs::None, window, cx),
            Some(CommandOutcome::Collaboration(CollaborationStep::Share))
        ) {
            return;
        }
        let engine = &self.app.read(cx).engine;
        let Some(root) = engine.roots().first().cloned() else {
            return;
        };
        let plan = Rc::new(engine.share_plan(&root));
        let interfaces = crate::collaboration::interfaces();
        let name = crate::collaboration::own_name();
        let workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let pages = plan
                .files
                .iter()
                .filter(|path| !path.components().any(|part| part.as_os_str() == "partials"))
                .count();
            let partials = plan.files.len() - pages;
            let mut summary = v_flex()
                .gap_2()
                .debug_selector(|| "share-confirm".to_owned())
                .child(format!("Share {}", plan.root.display()))
                .child(format!(
                    "{pages} page(s) and {partials} partial(s) will be offered to whoever you \
                     admit, as {name}. Nothing else: no images, settings or other files."
                ));
            if plan.left_out > 0 {
                summary = summary.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} AsciiDoc file(s) are left out: hidden, ignored, leading outside \
                             the folder or too large.",
                            plan.left_out
                        )),
                );
            }
            summary = summary.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Choose where to listen. Nothing listens until you do, and each person \
                         you invite waits for you to admit them.",
                    ),
            );
            let mut choices = h_flex().gap_2().justify_end();
            for (index, interface) in interfaces.iter().enumerate() {
                let (workspace, plan, name, address) = (
                    workspace.clone(),
                    plan.clone(),
                    name.clone(),
                    interface.address,
                );
                choices = choices.child(
                    Button::new(("share-start", index))
                        .label(format!("Share on {}", interface.label))
                        .debug_selector(move || format!("share-start-{index}"))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = workspace.update(cx, |this, cx| {
                                this.start_sharing(address, &plan, &name, window, cx);
                            });
                        }),
                );
            }
            dialog
                .title("Share Workspace")
                .width(px(600.))
                .child(v_flex().gap_3().child(summary).child(choices))
        });
    }

    fn start_sharing(
        &mut self,
        address: IpAddr,
        plan: &SharePlan,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let started = self.app.update(cx, |app, cx| {
            app.start_hosting(address, &plan.root, &plan.files, name, cx)
        });
        match started {
            Ok(()) => {
                self.sidebar_tab = SidebarTab::Session;
                window.push_notification(
                    "Sharing. Copy an invitation from the Session panel and send it to the \
                     person you are working with.",
                    cx,
                );
                cx.notify();
            }
            Err(error) => window.push_notification(format!("Sharing could not start: {error}"), cx),
        }
    }

    /// Copy Invitation: a new single-use invitation on the clipboard.
    pub(super) fn on_copy_invitation(
        &mut self,
        _: &CopyInvitation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.invoke(ids::COLLAB_COPY_INVITATION, CommandArgs::None, window, cx),
            Some(CommandOutcome::Collaboration(
                CollaborationStep::CopyInvitation
            ))
        ) {
            return;
        }
        let invited = self.app.update(cx, |app, _| app.new_invitation());
        match invited {
            Ok((line, lifetime)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(line));
                let code = self
                    .app
                    .read(cx)
                    .collaboration
                    .host()
                    .map(|host| host.safety_code().to_string())
                    .unwrap_or_default();
                window.push_notification(
                    format!(
                        "Invitation copied: one person may use it, within {} minutes. They \
                         should see the safety code {code}.",
                        lifetime.as_secs() / 60
                    ),
                    cx,
                );
                cx.notify();
            }
            Err(error) => window.push_notification(error, cx),
        }
    }

    /// Stop Sharing: every connection closes and the listener with them; the pages stay open.
    pub(super) fn on_stop_sharing(
        &mut self,
        _: &StopSharing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.invoke(ids::COLLAB_STOP, CommandArgs::None, window, cx),
            Some(CommandOutcome::Collaboration(CollaborationStep::Ended))
        ) {
            self.app.update(cx, |app, cx| app.end_transport(cx));
            window.push_notification("Sharing stopped.", cx);
        }
    }

    /// Stop Following: this participant's page no longer follows anyone's (#767).
    pub(super) fn on_stop_following(
        &mut self,
        _: &StopFollowing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(ids::COLLAB_STOP_FOLLOWING, CommandArgs::None, window, cx);
    }

    /// Leave Session: the connection closes; the shared tabs stay, no longer shared.
    pub(super) fn on_leave_session(
        &mut self,
        _: &LeaveSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.invoke(ids::COLLAB_LEAVE, CommandArgs::None, window, cx),
            Some(CommandOutcome::Collaboration(CollaborationStep::Ended))
        ) {
            self.app.update(cx, |app, cx| app.end_transport(cx));
            window.push_notification("You left the session.", cx);
        }
    }

    /// Join Session…: asks for the invitation, a name and, optionally, an address to be credited
    /// by in the host's commits, connects off the window's thread and checks the host's
    /// certificate, and goes on only once the safety code is compared.
    pub(super) fn on_join_session(
        &mut self,
        _: &JoinSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.invoke(ids::COLLAB_JOIN, CommandArgs::None, window, cx),
            Some(CommandOutcome::Collaboration(CollaborationStep::Join))
        ) {
            return;
        }
        let invitation =
            cx.new(|cx| InputState::new(window, cx).placeholder("adocfoundry:join?v=1&…"));
        let engine = &self.app.read(cx).engine;
        let (own, address) = (
            crate::collaboration::guest_name(engine),
            engine.comment_author().email.unwrap_or_default(),
        );
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Your name")
                .default_value(own)
        });
        let email = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("E-mail for commit credit (optional)")
                .default_value(address)
        });
        let first = invitation.clone();
        let workspace = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let (invitation, name, email, workspace) = (
                invitation.clone(),
                name.clone(),
                email.clone(),
                workspace.clone(),
            );
            let invalid = |email: &Entity<InputState>, cx: &App| {
                let typed = email.read(cx).value();
                let typed = typed.trim();
                !typed.is_empty() && commit_email(typed).is_err()
            };
            let problem = invalid(&email, cx).then(|| {
                div()
                    .debug_selector(|| "join-email-problem".to_owned())
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child("Not an e-mail address")
            });
            dialog
                .title("Join Session")
                .width(px(600.))
                .child(
                    v_flex()
                        .gap_2()
                        .debug_selector(|| "join-form".to_owned())
                        .child("Paste the invitation you were sent.")
                        .child(Input::new(&invitation))
                        .child(Input::new(&name))
                        .child(
                            div()
                                .debug_selector(|| "join-email".to_owned())
                                .child(Input::new(&email)),
                        )
                        .children(problem)
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "Only the host is told it, to credit your edits in its \
                                     commits.",
                                ),
                        ),
                )
                .confirm()
                .ok_text("Connect")
                .on_ok(move |_, window, cx| {
                    if invalid(&email, cx) {
                        return false;
                    }
                    let line = invitation.read(cx).value().to_string();
                    let who = name.read(cx).value().trim().to_owned();
                    let address = email.read(cx).value().trim().to_owned();
                    let address = (!address.is_empty()).then_some(address);
                    let _ = workspace.update(cx, |this, cx| {
                        this.connect(&line, who, address, window, cx);
                    });
                    true
                })
        });
        first.update(cx, |input, cx| input.focus(window, cx));
    }

    fn connect(
        &mut self,
        line: &str,
        name: String,
        email: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let invitation = match line.parse::<Invitation>() {
            Ok(invitation) => invitation,
            Err(_) => {
                window.push_notification("That is not an invitation.", cx);
                return;
            }
        };
        window.push_notification("Connecting…", cx);
        let connecting = cx
            .background_spawn(async move { adoc_share::connect(&invitation, Timings::default()) });
        cx.spawn_in(window, async move |this, cx| {
            let result = connecting.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.compare_safety_code(result, name, email, window, cx);
            });
        })
        .detach();
    }

    /// The host proved it holds the invited certificate: the person compares the safety code
    /// with the host's before anything, even their name, is sent.
    fn compare_safety_code(
        &mut self,
        result: Result<Connecting, ShareError>,
        name: String,
        email: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connecting = match result {
            Ok(connecting) => connecting,
            Err(ShareError::Fingerprint) => {
                window.push_notification(
                    "The host is not the one the invitation names. Nothing was sent.",
                    cx,
                );
                return;
            }
            Err(error) => {
                window.push_notification(format!("Could not connect: {error}"), cx);
                return;
            }
        };
        let code = connecting.safety_code().to_string();
        let held = Rc::new(RefCell::new(Some(connecting)));
        let workspace = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (held, workspace, name, email) =
                (held.clone(), workspace.clone(), name.clone(), email.clone());
            dialog
                .title("Compare the safety code")
                .child(
                    v_flex()
                        .gap_2()
                        .debug_selector(|| "join-safety-code".to_owned())
                        .child(
                            "The host's Session panel shows a safety code. Go on only if it is \
                             this one:",
                        )
                        .child(
                            div()
                                .debug_selector(|| "join-code".to_owned())
                                .text_lg()
                                .child(code.clone()),
                        ),
                )
                .confirm()
                .ok_text("Join")
                .on_ok(move |_, window, cx| {
                    let Some(connecting) = held.borrow_mut().take() else {
                        return true;
                    };
                    let _ = workspace.update(cx, |this, cx| {
                        this.authenticate(connecting, &name, email.as_deref(), window, cx);
                    });
                    true
                })
        });
    }

    fn authenticate(
        &mut self,
        connecting: Connecting,
        name: &str,
        email: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waker = self.app.read(cx).guest_waker();
        let joined = connecting
            .authenticate(name, email, env!("CARGO_PKG_VERSION"), waker)
            .map_err(|error| error.to_string())
            .and_then(|(link, control)| {
                self.app.update(cx, |app, cx| app.joined(link, control, cx))
            });
        match joined {
            Ok(()) => {
                self.sidebar_tab = SidebarTab::Session;
                window.push_notification("Waiting for the host to let you in…", cx);
                cx.notify();
            }
            Err(error) => window.push_notification(format!("Could not join: {error}"), cx),
        }
    }

    /// Opens a shared file the host offers, in a tab marked as shared, once its text arrives.
    fn open_shared_file(&mut self, file: u64, window: &mut Window, cx: &mut Context<Self>) {
        let args = CommandArgs::Custom(serde_json::json!({ "file": file }));
        match self.invoke(ids::COLLAB_OPEN_FILE, args, window, cx) {
            Some(CommandOutcome::Opened(id)) => {
                self.show_opened(TabItem::Document(id), SourceRange::default(), window, cx);
            }
            Some(_) => self.pending_shared_open = Some(file),
            None => {}
        }
    }

    /// The session changed: a shared file asked for may have arrived, and a guest whose link
    /// dropped is told so.
    pub(super) fn session_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let engine = &self.app.read(cx).engine;
        if let Some(file) = self.pending_shared_open {
            if let Some(id) = engine.session_document(file) {
                self.pending_shared_open = None;
                self.show_opened(TabItem::Document(id), SourceRange::default(), window, cx);
            }
        }
        let engine = &self.app.read(cx).engine;
        let in_session = engine.in_session();
        let connected = in_session.then(|| engine.session_connected());
        if self.shown_connected == Some(true) && connected == Some(false) {
            window.push_notification(
                "The connection to the host dropped. You cannot edit until it is back; \
                 Reconnect is in the Session panel.",
                cx,
            );
        }
        self.shown_connected = connected;
        if !in_session && self.sidebar_tab == SidebarTab::Session {
            self.sidebar_tab = SidebarTab::Navigation;
        }
        cx.notify();
    }

    /// The Session panel: for a host, the invitation, who is waiting, who is in and the shared
    /// files; for a guest, the connection, who is in and the files offered.
    pub(super) fn render_session(&self, cx: &mut Context<Self>) -> AnyElement {
        let app = self.app.read(cx);
        let engine = &app.engine;
        let muted = cx.theme().muted_foreground;
        let workspace_name = engine.session_workspace().unwrap_or_default();
        let mut panel = v_flex()
            .id("session-panel")
            .debug_selector(|| "session-panel".to_owned())
            .size_full()
            .overflow_y_scroll()
            .p_2()
            .gap_2()
            .text_sm();
        let action =
            |selector: String, label: &str, command: adoc_engine::CommandId, args: CommandArgs| {
                Button::new(SharedString::from(selector.clone()))
                    .label(label.to_owned())
                    .small()
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.invoke(command, args.clone(), window, cx);
                    }))
            };
        // Follow, or Stop Following for the one followed (#767).
        let following = engine.following();
        let follow = |peer: PeerId| {
            if following == Some(peer) {
                action(
                    format!("session-follow-{}", peer.0),
                    "Stop Following",
                    ids::COLLAB_STOP_FOLLOWING,
                    CommandArgs::None,
                )
            } else {
                action(
                    format!("session-follow-{}", peer.0),
                    "Follow",
                    ids::COLLAB_FOLLOW,
                    peer_args(peer),
                )
            }
        };
        if engine.is_session_host() {
            let collaboration = &app.collaboration;
            let address = collaboration
                .address
                .map(|address| address.to_string())
                .unwrap_or_default();
            let code = collaboration
                .host()
                .map(|host| host.safety_code().to_string())
                .unwrap_or_default();
            let issued = collaboration.issued.map_or_else(
                || "No invitation copied yet.".to_owned(),
                |issued| {
                    format!(
                        "Last invitation {}.",
                        crate::collaboration::minutes_left(issued.expires)
                    )
                },
            );
            panel = panel
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(format!("Sharing {workspace_name}")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("Listening on {address}")),
                )
                .child(
                    div()
                        .debug_selector(|| "session-safety-code".to_owned())
                        .child(format!("Safety code {code}")),
                )
                .child(div().text_xs().text_color(muted).child(issued))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("session-copy-invitation")
                                .label("Copy Invitation")
                                .small()
                                .debug_selector(|| "session-copy-invitation".to_owned())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_copy_invitation(&CopyInvitation, window, cx);
                                })),
                        )
                        .child(
                            Button::new("session-stop")
                                .label("Stop Sharing")
                                .small()
                                .danger()
                                .debug_selector(|| "session-stop".to_owned())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_stop_sharing(&StopSharing, window, cx);
                                })),
                        ),
                );
            let knocks = engine.session_knocks();
            if !knocks.is_empty() {
                panel = panel.child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Waiting"),
                );
                for (peer, name) in knocks {
                    panel = panel.child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(format!("{name} asks to join")))
                            .child(action(
                                format!("session-admit-{}", peer.0),
                                "Admit",
                                ids::COLLAB_ADMIT,
                                peer_args(peer),
                            ))
                            .child(action(
                                format!("session-deny-{}", peer.0),
                                "Turn Away",
                                ids::COLLAB_DENY,
                                peer_args(peer),
                            )),
                    );
                }
            }
            let participants = engine.participants();
            panel = panel.child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Participants"),
            );
            if participants.is_empty() {
                panel = panel.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Nobody has joined yet."),
                );
            }
            for participant in participants {
                let peer = participant.peer;
                let status = if participant.connected {
                    "connected"
                } else {
                    "away"
                };
                let (label, command) = if participant.role == Role::Editor {
                    ("Make Viewer", ids::COLLAB_REVOKE_EDIT)
                } else {
                    ("Let Edit", ids::COLLAB_GRANT_EDIT)
                };
                panel = panel.child(
                    h_flex()
                        .gap_2()
                        .debug_selector(move || format!("session-participant-{}", peer.0))
                        .child(swatch(peer, cx))
                        .child(div().flex_1().child(format!(
                            "{} · {} · {status}{}",
                            participant.name,
                            role_name(participant.role),
                            caret_place(engine, peer, participant.place)
                        )))
                        .when(participant.connected, |row| row.child(follow(peer)))
                        .child(action(
                            format!("session-role-{}", peer.0),
                            label,
                            command,
                            peer_args(peer),
                        ))
                        .child(action(
                            format!("session-remove-{}", peer.0),
                            "Remove",
                            ids::COLLAB_REMOVE,
                            peer_args(peer),
                        )),
                );
            }
            panel = panel.child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Shared files open"),
            );
            for entry in engine.session_manifest() {
                let Some(id) = engine.session_document(entry.file) else {
                    continue;
                };
                let state = engine
                    .shared_save_state(id)
                    .map_or("", |state| save_state_label(&state));
                let background = !engine.has_tab(id);
                let file = entry.file;
                panel = panel.child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().text_xs().child(format!(
                            "{} · {state}{}",
                            entry.path,
                            if background {
                                " · in the background"
                            } else {
                                ""
                            }
                        )))
                        .when(background, |row| {
                            row.child(
                                Button::new(("session-show", file as usize))
                                    .label("Show")
                                    .small()
                                    .debug_selector(move || format!("session-show-{file}"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.show_opened(
                                            TabItem::Document(id),
                                            SourceRange::default(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                        }),
                );
            }
        } else if engine.in_session() {
            let connected = engine.session_connected();
            let me = engine.session_me();
            let participants = engine.participants();
            let host = participants
                .iter()
                .find(|participant| participant.role == Role::Owner)
                .map(|participant| participant.name.clone())
                .unwrap_or_default();
            let joined = me.is_some();
            panel = panel
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(if joined {
                            format!("Joined {workspace_name}")
                        } else {
                            "Waiting to be let in…".to_owned()
                        }),
                )
                .when(!host.is_empty(), |panel| {
                    panel.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("Hosted by {host}")),
                    )
                })
                .when_some(me, |panel, (_, role)| {
                    panel.child(div().debug_selector(|| "session-my-role".to_owned()).child(
                        match role {
                            Role::Editor => "You may edit.".to_owned(),
                            _ => "You may read and copy, not edit.".to_owned(),
                        },
                    ))
                })
                .child(
                    div()
                        .debug_selector(|| "session-connection".to_owned())
                        .child(if connected {
                            "Connected"
                        } else {
                            "Disconnected: not known saved"
                        }),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .when(!connected, |row| {
                            row.child(
                                Button::new("session-reconnect")
                                    .label("Reconnect")
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.app.read(cx).reconnect();
                                    })),
                            )
                        })
                        .child(
                            Button::new("session-leave")
                                .label("Leave Session")
                                .small()
                                .debug_selector(|| "session-leave".to_owned())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_leave_session(&LeaveSession, window, cx);
                                })),
                        ),
                );
            if !participants.is_empty() {
                panel = panel.child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Participants"),
                );
                for participant in participants {
                    let you = me.is_some_and(|(peer, _)| peer == participant.peer);
                    let peer = participant.peer;
                    panel = panel.child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .debug_selector(move || format!("session-member-{}", peer.0))
                            .when(!you, |row| row.child(swatch(peer, cx)))
                            .child(format!(
                                "{}{} · {}{}{}",
                                participant.name,
                                if you { " (you)" } else { "" },
                                role_name(participant.role),
                                if participant.connected {
                                    ""
                                } else {
                                    " · away"
                                },
                                if you {
                                    String::new()
                                } else {
                                    caret_place(engine, peer, participant.place)
                                }
                            ))
                            .when(!you && participant.connected && connected, |row| {
                                row.child(follow(peer))
                            }),
                    );
                }
            }
            let manifest = engine.session_manifest();
            if !manifest.is_empty() {
                panel = panel.child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Shared files"),
                );
            }
            for entry in manifest {
                let file = entry.file;
                let open = engine.session_document(file);
                let state = open
                    .and_then(|id| engine.shared_save_state(id))
                    .map(|state| format!(" · {}", save_state_label(&state)))
                    .unwrap_or_default();
                let kind = match entry.kind {
                    FileKind::Page => "",
                    FileKind::Partial => " (partial)",
                };
                panel = panel.child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .child(format!("{}{kind}{state}", entry.path)),
                        )
                        .child(
                            Button::new(("session-open", file as usize))
                                .label("Open")
                                .small()
                                .debug_selector(move || format!("session-open-{file}"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_shared_file(file, window, cx);
                                })),
                        ),
                );
            }
        }
        panel.into_any_element()
    }
}
