//! Comments in a session (#762, COMMENTS 2/4; `docs/prd/PRD-AdocFoundry_Comments.md` §7).
//!
//! The host holds every shared file's comments, as it holds them alone (`crate::comments`): byte
//! ranges in its text, kept in the page's metadata file. It decides who may do what and tells
//! each guest with the file open every comment (`Message::CommentThread`), with its place as a
//! selection relative to the shared text, after the file's snapshot and again whenever the
//! comment changes or its place would resolve otherwise on the guest's side. A guest holds the
//! threads it was told and resolves their selections in its replica when asked, as remote
//! carets are, so every participant shows a comment on the same text. A guest never reads or
//! writes a metadata file: it asks the host (`Message::Comment`), and a refusal comes back as
//! `Message::CommentRefused`, the connection staying.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use adoc_collab::protocol::{
    CommentChange, Message, PeerId, Role, SharedReply, SharedResolution, SharedThread,
    MAX_COMMENTS, MAX_NAME, MAX_TEXT,
};
use adoc_core::SourceRange;

use super::{Session, Side};
use crate::{
    comments::{Actor, ActorKind, Change},
    page_meta::cut,
    Allowed, Author, Comment, CommentId, DocumentId, Engine, EngineError, EngineEvent, Reply,
};

/// The most comments told to one guest for one file in a pump, so a page with many long
/// comments never fills the guest's queue at once; the rest follow in the next pumps.
const THREADS_PER_PUMP: usize = 32;

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn time(seconds: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH)
}

/// A name as the protocol takes it: some text, within [`MAX_NAME`], no control character.
fn shown_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    let clean = cut(clean.trim(), MAX_NAME).to_owned();
    if clean.is_empty() {
        "Unknown".to_owned()
    } else {
        clean
    }
}

/// A comment's text as the protocol takes it: control characters but line breaks and tabs
/// shown as U+FFFD, and some text.
fn shown_text(text: &str) -> String {
    let clean: String = text
        .chars()
        .map(|c| {
            if c.is_control() && !matches!(c, '\n' | '\t') {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect();
    if clean.trim().is_empty() {
        "\u{fffd}".to_owned()
    } else {
        clean
    }
}

/// A host's comment as `guest` is told it. E-mail addresses in the page's metadata file stay on
/// the host's side: a guest is told names only, and whether each comment and reply is its own,
/// which the host decides by address (else by name).
fn thread_of(comment: &Comment, guest: &Actor) -> SharedThread {
    SharedThread {
        id: comment.id.0,
        author: shown_name(&comment.author.name),
        own: guest.owns(&comment.author.name, comment.author.email.as_deref()),
        created: seconds(comment.created),
        edited: comment.edited.map(seconds),
        quote: cut(&comment.quote, adoc_collab::protocol::MAX_COMMENT_QUOTE).to_owned(),
        text: shown_text(&comment.text),
        resolved: comment
            .resolved
            .as_ref()
            .map(|resolution| SharedResolution {
                by: shown_name(&resolution.by),
                at: seconds(resolution.at),
            }),
        detached: comment.detached,
        suggestion: None,
        replies: comment
            .replies
            .iter()
            .map(|reply| SharedReply {
                id: reply.id.0,
                author: shown_name(&reply.author.name),
                own: guest.owns(&reply.author.name, reply.author.email.as_deref()),
                created: seconds(reply.created),
                edited: reply.edited.map(seconds),
                text: shown_text(&reply.text),
            })
            .collect(),
    }
}

/// A guest as the host's comments see it: its name, and the address it gave.
fn peer_actor(entry: &super::Peer) -> Actor {
    Actor {
        author: Author {
            name: entry.name.clone(),
            email: entry.email.clone(),
        },
        kind: match entry.role {
            Role::Viewer => ActorKind::Viewer,
            Role::Editor | Role::Owner => ActorKind::Editor,
        },
    }
}

fn digest(thread: &SharedThread) -> u64 {
    let mut hasher = DefaultHasher::new();
    serde_json::to_vec(thread)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// Why an engine error refuses a guest's request, in words for the guest.
fn reason(error: &EngineError) -> String {
    let reason = match error {
        EngineError::Refused(reason)
        | EngineError::InvalidRequest(reason)
        | EngineError::NotFound(reason) => reason.clone(),
        other => other.to_string(),
    };
    cut(&shown_text(&reason).replace('\n', " "), MAX_TEXT).to_owned()
}

impl Engine {
    /// Who the person here is to document `id`'s comments in a session: the host of a shared
    /// page, or a guest on one; `None` for any other page.
    pub(crate) fn session_actor(&self, id: DocumentId) -> Option<Actor> {
        let session = self.collab_session.as_ref()?;
        match &session.side {
            Side::Host(host) => host
                .offered
                .values()
                .any(|offered| offered.document == Some(id))
                .then(|| Actor {
                    author: self.comment_author(),
                    kind: ActorKind::Host,
                }),
            Side::Guest(guest) => {
                Self::session_file(session, id)?;
                let (me, role) = guest.me?;
                let name = guest
                    .roster
                    .iter()
                    .find(|member| member.peer == me)
                    .map_or_else(|| "Unknown".to_owned(), |member| member.name.clone());
                Some(Actor {
                    author: Author { name, email: None },
                    kind: match role {
                        Role::Viewer => ActorKind::Viewer,
                        Role::Editor | Role::Owner => ActorKind::Editor,
                    },
                })
            }
        }
    }

    /// The comments of a guest's shared document `id`, as the host told them, placed in its
    /// replica; `None` for any other document.
    pub(crate) fn guest_comments(&self, id: DocumentId) -> Option<Vec<Comment>> {
        let session = self.collab_session.as_ref()?;
        let Side::Guest(guest) = &session.side else {
            return None;
        };
        let file = Self::session_file(session, id)?;
        let threads = &guest.files.get(&file)?.threads;
        let actor = self.session_actor(id)?;
        let text = &self.documents.get(&id)?.text;
        let comments = threads
            .values()
            .map(|(thread, selection)| {
                let placed = selection
                    .as_ref()
                    .and_then(|selection| self.shared_resolve(id, selection))
                    .map(|(start, end)| SourceRange::new(start.min(end), start.max(end)))
                    .filter(|range| !range.is_empty());
                let detached = thread.detached || placed.is_none();
                let range = placed.unwrap_or_default();
                Comment {
                    id: CommentId(thread.id),
                    author: Author {
                        name: thread.author.clone(),
                        email: None,
                    },
                    created: time(thread.created),
                    edited: thread.edited.map(time),
                    range,
                    quote: if detached {
                        thread.quote.clone()
                    } else {
                        text.slice(range.start..range.end).to_string()
                    },
                    text: thread.text.clone(),
                    resolved: thread.resolved.as_ref().map(|resolution| {
                        crate::page_meta::Resolution {
                            by: resolution.by.clone(),
                            at: time(resolution.at),
                        }
                    }),
                    detached,
                    // Suggestions reach a guest with SUGGEST 2/4.
                    suggestion: None,
                    replies: thread
                        .replies
                        .iter()
                        .map(|reply| Reply {
                            id: CommentId(reply.id),
                            author: Author {
                                name: reply.author.clone(),
                                email: None,
                            },
                            created: time(reply.created),
                            edited: reply.edited.map(time),
                            text: reply.text.clone(),
                            can: actor.allowed_own(reply.own),
                        })
                        .collect(),
                    can: actor.allowed_own(thread.own),
                }
            })
            .collect();
        Some(comments)
    }

    /// Asks the host to make `change` to a guest's shared document `id`'s comments. Refused here
    /// when the host would refuse it, or while the page cannot reach the host.
    pub(crate) fn guest_comment(
        &mut self,
        id: DocumentId,
        change: Change,
    ) -> Result<(), EngineError> {
        let comments = self.guest_comments(id).unwrap_or_default();
        let allowed = |comment: u64, reply: Option<u64>| -> Result<Allowed, EngineError> {
            let thread = comments
                .iter()
                .find(|thread| thread.id.0 == comment)
                .ok_or_else(|| EngineError::NotFound(format!("comment {}", CommentId(comment))))?;
            match reply {
                None => Ok(thread.can),
                Some(reply) => thread
                    .replies
                    .iter()
                    .find(|stored| stored.id.0 == reply)
                    .map(|stored| stored.can)
                    .ok_or_else(|| EngineError::NotFound(format!("reply {}", CommentId(reply)))),
            }
        };
        let refused = |what: &str| Err(EngineError::Refused(format!("You cannot {what}.")));
        let checked = |text: String| -> Result<String, EngineError> {
            let text = text.trim_end().to_owned();
            if text.trim().is_empty() {
                return Err(EngineError::Refused("a comment needs some text".to_owned()));
            }
            if text.len() > adoc_collab::protocol::MAX_COMMENT_TEXT {
                return Err(EngineError::Refused(
                    crate::page_meta::MetaProblem::TooLong.to_string(),
                ));
            }
            Ok(text)
        };
        let (change, selection) = match change {
            Change::Add { range, text } => {
                if range.is_empty() {
                    return Err(EngineError::InvalidRequest(
                        "a comment is on a range of the page's text".to_owned(),
                    ));
                }
                if comments.len() >= MAX_COMMENTS {
                    return Err(EngineError::Refused(
                        crate::page_meta::MetaProblem::TooMany.to_string(),
                    ));
                }
                let selection = self.shared_range(id, range).ok_or_else(|| {
                    EngineError::InvalidRequest(
                        "a comment is on a range of the page's text".to_owned(),
                    )
                })?;
                (
                    CommentChange::Add {
                        text: checked(text)?,
                        suggestion: None,
                    },
                    Some(selection),
                )
            }
            Change::Reply { comment, text } => {
                allowed(comment, None)?;
                (
                    CommentChange::Reply {
                        comment,
                        text: checked(text)?,
                    },
                    None,
                )
            }
            Change::Resolve { comment, resolved } => {
                if !allowed(comment, None)?.resolve {
                    return refused("resolve or reopen this comment");
                }
                (CommentChange::Resolve { comment, resolved }, None)
            }
            Change::Edit {
                comment,
                reply,
                text,
            } => {
                if !allowed(comment, reply)?.edit {
                    return refused("edit someone else's comment");
                }
                (
                    CommentChange::Edit {
                        comment,
                        reply,
                        text: checked(text)?,
                    },
                    None,
                )
            }
            Change::Delete { comment, reply } => {
                if !allowed(comment, reply)?.delete {
                    return refused("delete someone else's comment");
                }
                (CommentChange::Delete { comment, reply }, None)
            }
            // Suggestions reach the host with SUGGEST 2/4.
            Change::Suggest { .. }
            | Change::Accept { .. }
            | Change::Reject { .. }
            | Change::EditSuggestion { .. } => {
                return Err(EngineError::Refused(
                    "Suggestions cannot be made in a session yet.".to_owned(),
                ));
            }
        };
        let Some(session) = self.collab_session.as_mut() else {
            return Err(EngineError::Refused("the session has ended".to_owned()));
        };
        let file = Self::session_file(session, id)
            .ok_or_else(|| EngineError::Refused("the page is not shared".to_owned()))?;
        let Side::Guest(guest) = &mut session.side else {
            return Err(EngineError::Refused("not a guest".to_owned()));
        };
        let reachable = guest.connected
            && guest
                .files
                .get(&file)
                .is_some_and(|state| !state.waiting && !state.resuming);
        if !reachable {
            return Err(EngineError::Refused(
                "The host cannot be reached; try again once reconnected.".to_owned(),
            ));
        }
        let epoch = guest.files.get(&file).map_or(0, |state| state.epoch);
        guest.next_request += 1;
        let request = guest.next_request;
        session
            .link
            .send(
                PeerId::HOST,
                Message::Comment {
                    file,
                    epoch,
                    request,
                    change,
                    selection,
                },
            )
            .map_err(|_| {
                EngineError::Refused(
                    "The host cannot be reached; try again once reconnected.".to_owned(),
                )
            })
    }

    /// A guest's request to change `file`'s comments: made as the guest, if the guest may, and
    /// refused with a reason otherwise. A request from an earlier epoch is refused, never
    /// applied: its selection belongs to text the host has since replaced.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn host_comment(
        &mut self,
        session: &mut Session,
        peer: PeerId,
        file: u64,
        epoch: u32,
        request: u64,
        change: CommentChange,
        selection: Option<adoc_collab::RelativeSelection>,
    ) -> Result<(), String> {
        let state_epoch = self.host_file_state(session, peer, file)?.epoch;
        let Side::Host(host) = &session.side else {
            return Ok(());
        };
        let Some(id) = host.offered.get(&file).and_then(|offered| offered.document) else {
            return Err("a file not shared".to_owned());
        };
        let Some(entry) = host.peers.get(&peer) else {
            return Ok(());
        };
        let actor = peer_actor(entry);
        let refuse = |session: &mut Session, reason: String| {
            let _ = session.link.send(
                peer,
                Message::CommentRefused {
                    file,
                    request,
                    reason,
                },
            );
        };
        if epoch != state_epoch {
            refuse(
                session,
                "The page was started again from the host's text; try again.".to_owned(),
            );
            return Ok(());
        }
        let change = match change {
            CommentChange::Add {
                text,
                suggestion: None,
            } => {
                let range = selection
                    .and_then(|selection| {
                        adoc_collab::RelativeSelection::from_wire(selection.anchor, selection.head)
                            .ok()
                    })
                    .and_then(|selection| self.shared_resolve(id, &selection))
                    .map(|(start, end)| SourceRange::new(start.min(end), start.max(end)))
                    .filter(|range| !range.is_empty());
                let Some(range) = range else {
                    refuse(
                        session,
                        "The text commented on is not in the host's page; try again.".to_owned(),
                    );
                    return Ok(());
                };
                Change::Add { range, text }
            }
            CommentChange::Reply { comment, text } => Change::Reply { comment, text },
            CommentChange::Resolve { comment, resolved } => Change::Resolve { comment, resolved },
            CommentChange::Edit {
                comment,
                reply,
                text,
            } => Change::Edit {
                comment,
                reply,
                text,
            },
            CommentChange::Delete { comment, reply } => Change::Delete { comment, reply },
            CommentChange::Add { .. }
            | CommentChange::Accept { .. }
            | CommentChange::Reject { .. }
            | CommentChange::EditSuggestion { .. } => {
                refuse(
                    session,
                    "Suggested changes are not yet supported in a session.".to_owned(),
                );
                return Ok(());
            }
        };
        if let Err(error) = self.change_comments(id, change, &actor) {
            refuse(session, reason(&error));
        }
        Ok(())
    }

    /// Whether any guest with a shared file open is yet to be told how its comments stand.
    pub(super) fn host_comments_unannounced(&self, session: &Session) -> bool {
        let Side::Host(host) = &session.side else {
            return false;
        };
        host.offered.iter().any(|(file, offered)| {
            offered.document.is_some_and(|id| {
                let seen = self.comments_seen(id);
                host.peers.values().any(|entry| {
                    entry.connected
                        && entry
                            .files
                            .get(file)
                            .is_some_and(|open| open.live && open.comments_seen != Some(seen))
                })
            })
        })
    }

    /// What a guest's knowledge of document `id`'s comments is checked against: the text's
    /// version and the comments' generation.
    fn comments_seen(&self, id: DocumentId) -> (u64, u64) {
        (
            self.documents
                .get(&id)
                .map_or(0, |document| document.version),
            self.comments_generation(id),
        )
    }

    /// Tells each live guest with `file` open the comments of document `id` that changed since
    /// it was told, or whose place would resolve otherwise on its side, and the ones deleted.
    pub(super) fn host_announce_comments(
        &mut self,
        session: &mut Session,
        file: u64,
        id: DocumentId,
    ) {
        let seen = self.comments_seen(id);
        let comments = self.comments(id);
        let peers: Vec<PeerId> = {
            let Side::Host(host) = &session.side else {
                return;
            };
            host.peers
                .iter()
                .filter(|(_, entry)| entry.connected)
                .filter(|(_, entry)| {
                    entry
                        .files
                        .get(&file)
                        .is_some_and(|open| open.live && open.comments_seen != Some(seen))
                })
                .map(|(peer, _)| *peer)
                .collect()
        };
        for peer in peers {
            let mut messages = Vec::new();
            let mut complete = true;
            let threads: Vec<(SharedThread, Option<SourceRange>)> = {
                let Side::Host(host) = &session.side else {
                    return;
                };
                let Some(entry) = host.peers.get(&peer) else {
                    continue;
                };
                let actor = peer_actor(entry);
                comments
                    .iter()
                    .map(|comment| {
                        (
                            thread_of(comment, &actor),
                            (!comment.detached).then_some(comment.range),
                        )
                    })
                    .collect()
            };
            {
                let Side::Host(host) = &session.side else {
                    return;
                };
                let Some(open) = host
                    .peers
                    .get(&peer)
                    .and_then(|entry| entry.files.get(&file))
                else {
                    continue;
                };
                for (thread, range) in &threads {
                    let digest = digest(thread);
                    let told = open.threads.get(&thread.id);
                    let placed =
                        told.and_then(|(_, selection)| selection.as_ref())
                            .and_then(|selection| {
                                self.shared_resolve(id, selection)
                                    .map(|(start, end)| SourceRange::new(start, end))
                            });
                    let stale = match told {
                        None => true,
                        Some((told, _)) => *told != digest || placed != *range,
                    };
                    if !stale {
                        continue;
                    }
                    if messages.len() == THREADS_PER_PUMP {
                        complete = false;
                        break;
                    }
                    let selection = range.and_then(|range| self.shared_range(id, range));
                    let mut thread = thread.clone();
                    if selection.is_none() {
                        thread.detached = true;
                    }
                    messages.push((digest, thread, selection));
                }
            }
            let Side::Host(host) = &mut session.side else {
                return;
            };
            let Some(open) = host
                .peers
                .get_mut(&peer)
                .and_then(|entry| entry.files.get_mut(&file))
            else {
                continue;
            };
            let epoch = open.epoch;
            let removed: Vec<u64> = open
                .threads
                .keys()
                .filter(|told| !threads.iter().any(|(thread, _)| thread.id == **told))
                .copied()
                .collect();
            for comment in removed {
                open.threads.remove(&comment);
                let _ = session.link.send(
                    peer,
                    Message::CommentRemoved {
                        file,
                        epoch,
                        comment,
                    },
                );
            }
            for (digest, thread, selection) in messages {
                open.threads.insert(thread.id, (digest, selection.clone()));
                let _ = session.link.send(
                    peer,
                    Message::CommentThread {
                        file,
                        epoch,
                        thread: Box::new(thread),
                        selection,
                    },
                );
            }
            if complete {
                open.comments_seen = Some(seen);
            }
        }
    }

    /// The host's comment as it now stands, or a guest's request refused.
    pub(super) fn guest_comment_message(&mut self, session: &mut Session, message: Message) {
        let Side::Guest(guest) = &mut session.side else {
            return;
        };
        let (file, document) = match &message {
            Message::CommentThread { file, epoch, .. }
            | Message::CommentRemoved { file, epoch, .. } => {
                let Some(state) = guest.files.get(file).filter(|state| state.epoch == *epoch)
                else {
                    return;
                };
                (*file, state.document)
            }
            Message::CommentRefused { file, .. } => (
                *file,
                guest.files.get(file).and_then(|state| state.document),
            ),
            _ => return,
        };
        match message {
            Message::CommentThread {
                thread, selection, ..
            } => {
                let Some(state) = guest.files.get_mut(&file) else {
                    return;
                };
                if !state.threads.contains_key(&thread.id) && state.threads.len() >= MAX_COMMENTS {
                    return;
                }
                state.threads.insert(thread.id, (*thread, selection));
            }
            Message::CommentRemoved { comment, .. } => {
                if let Some(state) = guest.files.get_mut(&file) {
                    state.threads.remove(&comment);
                }
            }
            Message::CommentRefused { reason, .. } => {
                if let Some(id) = document {
                    self.emit(EngineEvent::CommentRefused { id, reason });
                }
                return;
            }
            _ => return,
        }
        if let Some(id) = document {
            self.emit(EngineEvent::CommentsChanged(id));
        }
    }
}
