//! A collaboration session on the engine's thread (COLLAB 4/8, #691;
//! `docs/design/DESIGN-AdocFoundry_Collaboration.md` §§2, 5, 7, 8, 11).
//!
//! The transport (`adoc-share`) authenticates connections and moves frames; everything a
//! message may do is decided here, against the session's own state, whatever the peer claims:
//! which files are offered, who may open, edit and save them, each guest's `yrs` client id per
//! file and epoch, and the order of every update. The engine owns the session's [`Link`] and
//! drains at most [`SESSION_EVENTS_PER_PUMP`] events a `pump`, so a flooding peer never holds
//! the engine; sending never waits, and a peer that cannot keep up is disconnected by the
//! transport.
//!
//! Sequence numbers count each sender's updates per file, from 1 after every snapshot the host
//! sends that peer: a snapshot replaces whatever the receiver had, so what was in flight before
//! it means nothing after. The host discards a guest's updates for a file after a snapshot until
//! one numbered 1 arrives. When a guest's link comes back, the host tells it the last of its
//! updates applied (`UpdateAck`), the guest sends again those after it and asks to be
//! resynchronised, and the host answers with a checked snapshot.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use adoc_collab::{
    protocol::{
        display_path, ErrorCode, FileKind, ManifestFile, Member, Message, PeerId, Role,
        SaveOutcome, SharedComponent, MANIFEST_PART, MAX_MANIFEST_FILES, MAX_OPEN_FILES,
        MAX_PARTICIPANTS, MAX_QUEUED_FRAMES,
    },
    EncodedUpdate, Link, LinkEvent, RelativeSelection, ReplicaId, Sender, MAX_SHARED_FILE,
};
use adoc_core::FileAccess;

use crate::{
    collab::{revision_of, SharedFileId, SharedReceipt, SharedSaveState},
    normalize_path, DocumentId, Engine, EngineError, EngineEvent,
};

/// Why a guest cannot have a file: said without the reason the host's engine gives, which names
/// the host's own paths.
const CANNOT_OPEN: &str = "the host cannot open the file just now";

/// The most link events one `pump` handles; any more are due at the next.
pub const SESSION_EVENTS_PER_PUMP: usize = 64;

/// Someone in a session, as the host sees them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Participant {
    pub peer: PeerId,
    pub name: String,
    pub role: Role,
    /// Whether their link is up; a disconnected participant may resume.
    pub connected: bool,
}

#[derive(Default)]
struct Presence {
    throttle: adoc_collab::AwarenessThrottle,
    /// The selection waiting to be sent.
    waiting: Option<RelativeSelection>,
}

/// Another participant's caret and selection in a shared document, where they are now in its
/// text, for an editor to draw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerCaret {
    pub peer: PeerId,
    pub name: String,
    /// Byte offsets: where the selection started, and the caret.
    pub anchor: usize,
    pub head: usize,
}

/// A selection another participant last showed in a shared file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerSelection {
    pub peer: PeerId,
    pub selection: RelativeSelection,
}

pub(crate) struct Session {
    link: Box<dyn Link>,
    side: Side,
    /// This participant's own selection in each shared file, paced (design §8: one message
    /// per `AWARENESS_INTERVAL`, the latest winning).
    presence: BTreeMap<u64, Presence>,
    /// The last pump stopped at [`SESSION_EVENTS_PER_PUMP`]: the next is due at once.
    backlog: bool,
}

enum Side {
    Host(Host),
    Guest(Guest),
}

struct Host {
    /// The host's name, and the shared folder's, as guests are shown them.
    name: String,
    workspace: String,
    /// The Antora component versions the offered files belong to, sent after the manifest.
    components: Vec<SharedComponent>,
    access: FileAccess,
    /// The offered files, by their session number.
    offered: BTreeMap<u64, Offered>,
    peers: BTreeMap<PeerId, Peer>,
    /// Connections that showed an invitation and wait for Admit.
    knocks: BTreeMap<PeerId, String>,
    /// Every `yrs` client id given out in the session, the host's own included.
    clients: BTreeSet<u64>,
}

struct Offered {
    path: PathBuf,
    entry: ManifestFile,
    /// The host's document for the file once a guest opened it.
    document: Option<DocumentId>,
    /// The file's epoch as last seen: a change means every peer needs a new snapshot.
    epoch: u32,
}

/// A file's state as the host tells it: the saved revision while the shared text is it, and a
/// conflict.
type FileStatus = (Option<String>, bool);

struct Peer {
    name: String,
    role: Role,
    connected: bool,
    files: BTreeMap<u64, PeerFile>,
    /// Who is in the session, as the peer was last told it.
    told_roster: Option<Vec<Member>>,
}

struct PeerFile {
    epoch: u32,
    client: ReplicaId,
    /// The sequence number the next update from the peer must carry.
    next_in: u64,
    /// After a snapshot: the peer's updates are discarded until one numbered 1 arrives.
    barrier: bool,
    /// The last of the peer's updates applied.
    applied: u64,
    /// The number the next update to the peer carries.
    next_out: u64,
    /// Whether updates are forwarded to the peer: not while it is away or resynchronising.
    live: bool,
    selection: Option<RelativeSelection>,
    /// The file's state as the peer was last told it.
    told: Option<FileStatus>,
}

struct Guest {
    me: Option<(PeerId, Role)>,
    connected: bool,
    manifest: Vec<ManifestFile>,
    manifest_done: bool,
    files: BTreeMap<u64, GuestFile>,
    /// The next save request's number, and which file each one waiting is for.
    next_request: u64,
    requests: BTreeMap<u64, u64>,
    /// The shared folder's name and who is in the session, as the host last said.
    workspace: Option<String>,
    roster: Vec<Member>,
    /// Where the shared files' pages are opened: a folder of this session's that is never on
    /// disk (`is_shared_path`), so nothing is read there.
    virtual_root: Option<PathBuf>,
    /// The component versions the host named, and the workspace the shared pages resolve in:
    /// a catalog of the manifest's files under the virtual root and an index of those open,
    /// apart from this computer's own workspace.
    components: Vec<SharedComponent>,
    shared: Box<adoc_lang::WorkspaceState>,
    indexed: BTreeMap<DocumentId, (u64, PathBuf)>,
}

#[derive(Default)]
struct GuestFile {
    document: Option<DocumentId>,
    epoch: u32,
    next_in: u64,
    next_out: u64,
    /// Updates sent and not yet acknowledged, oldest first, kept to send again after a
    /// reconnection.
    unacked: VecDeque<(u64, EncodedUpdate)>,
    /// A resynchronisation was asked for: updates are ignored and edits refused until the
    /// snapshot that answers it.
    waiting: bool,
    /// The link came back: the host's acknowledgement decides what is sent again.
    resuming: bool,
    selections: BTreeMap<PeerId, RelativeSelection>,
    /// What the host last said of its file: the revision on disk, and a conflict.
    saved: Option<String>,
    conflict: bool,
    /// The answer to this guest's last save request.
    last_save: Option<(u64, SaveOutcome)>,
    /// The document version last hashed for its save state, and the hash.
    hashed: std::cell::RefCell<Option<(u64, String)>>,
    /// Shown when it arrives: it was opened by following a link to it.
    show: bool,
    /// The `client` of the snapshot this side's replica was last made from, which each update
    /// sent names (`Message::Update::base`).
    base: u64,
}

fn refused(message: &str) -> EngineError {
    EngineError::Refused(message.to_owned())
}

/// Why `path` (normalised, absolute) cannot be offered from `root`, if it cannot: it must be an
/// `.adoc` file inside the root both as written and with its links followed, reached through no
/// hidden or ignored folder, within [`MAX_SHARED_FILE`], with a path that can be shown.
fn offer(
    access: &FileAccess,
    root: &Path,
    path: &Path,
    ignore: &[String],
) -> Result<(String, FileKind), String> {
    let shown = path.display();
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{shown} is outside the shared folder"))?;
    if !access.permits(path) {
        return Err(format!("{shown} leads outside the shared folder"));
    }
    let parts: Vec<&str> = relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()
        .ok_or_else(|| format!("{shown} is not a path that can be shown"))?;
    if parts
        .iter()
        .any(|part| part.starts_with('.') || ignore.iter().any(|ignored| ignored == part))
    {
        return Err(format!("{shown} is hidden or ignored"));
    }
    if path.extension().is_none_or(|extension| extension != "adoc") {
        return Err(format!("{shown} is not an AsciiDoc file"));
    }
    let metadata = std::fs::metadata(path).map_err(|error| format!("{shown}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{shown} is not a file"));
    }
    if metadata.len() > MAX_SHARED_FILE as u64 {
        return Err(format!("{shown} is too large to share"));
    }
    let display = parts.join("/");
    display_path(&display).map_err(|_| format!("{shown} is not a path that can be shown"))?;
    let kind = if parts.contains(&"partials") {
        FileKind::Partial
    } else {
        FileKind::Page
    };
    Ok((display, kind))
}

impl Engine {
    /// Hosts a session over `link`, offering `files` (absolute, or relative to `root`) from the
    /// folder `root`. Each is checked as design §11 asks; one that may not be offered refuses the
    /// whole, so the host is never left sharing less than it chose without knowing. Nothing is
    /// sent until a guest is admitted.
    pub fn host_session(
        &mut self,
        link: Box<dyn Link>,
        root: &Path,
        files: &[PathBuf],
        name: &str,
    ) -> Result<Vec<ManifestFile>, EngineError> {
        if self.collab_session.is_some() {
            return Err(refused("a session is already running"));
        }
        let root = normalize_path(
            &std::path::absolute(root).map_err(|error| refused(&error.to_string()))?,
        );
        let access = FileAccess::within([root.clone()]);
        let ignore = self.settings.workspace.ignore.clone();
        let mut offered = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for file in files {
            let path = normalize_path(&root.join(file));
            if !seen.insert(path.clone()) {
                continue;
            }
            if offered.len() == MAX_MANIFEST_FILES {
                return Err(refused("too many files to share"));
            }
            let (display, kind) =
                offer(&access, &root, &path, &ignore).map_err(|reason| refused(&reason))?;
            let number = offered.len() as u64 + 1;
            let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
            offered.insert(
                number,
                Offered {
                    path,
                    entry: ManifestFile {
                        file: number,
                        path: display,
                        kind,
                        size,
                    },
                    document: None,
                    epoch: 0,
                },
            );
        }
        let manifest = offered
            .values()
            .map(|offered| offered.entry.clone())
            .collect();
        self.collab_session = Some(Session {
            link,
            side: Side::Host(Host {
                name: name.to_owned(),
                components: self.shared_components(&root, &offered),
                workspace: root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                access,
                offered,
                peers: BTreeMap::new(),
                knocks: BTreeMap::new(),
                clients: BTreeSet::new(),
            }),
            backlog: false,
            presence: BTreeMap::new(),
        });
        Ok(manifest)
    }

    /// Joins a session over `link`, which has already authenticated with the host. The host's
    /// `Join` and manifest arrive through `pump`.
    pub fn join_session(&mut self, link: Box<dyn Link>) -> Result<(), EngineError> {
        if self.collab_session.is_some() {
            return Err(refused("a session is already running"));
        }
        self.collab_session = Some(Session {
            link,
            side: Side::Guest(Guest {
                me: None,
                connected: true,
                manifest: Vec::new(),
                manifest_done: false,
                files: BTreeMap::new(),
                next_request: 1,
                requests: BTreeMap::new(),
                workspace: None,
                roster: Vec::new(),
                virtual_root: None,
                components: Vec::new(),
                shared: Box::default(),
                indexed: BTreeMap::new(),
            }),
            backlog: false,
            presence: BTreeMap::new(),
        });
        Ok(())
    }

    /// Whether a session is running.
    #[must_use]
    pub fn in_session(&self) -> bool {
        self.collab_session.is_some()
    }

    /// Ends the session: a host closes every connection, a guest leaves. Shared documents stay
    /// open, no longer shared, with empty undo histories.
    pub fn end_session(&mut self) {
        let Some(mut session) = self.collab_session.take() else {
            return;
        };
        match &session.side {
            Side::Host(host) => {
                // Pages kept in the background for guests: those with changes not saved come
                // forward as tabs, so nothing is lost out of sight; the rest close.
                let background: Vec<DocumentId> = host
                    .offered
                    .values()
                    .filter_map(|offered| offered.document)
                    .filter(|id| !self.tabs.order.contains(&crate::TabItem::Document(*id)))
                    .collect();
                for id in background {
                    if self
                        .documents
                        .get(&id)
                        .is_some_and(|document| document.dirty)
                    {
                        self.tabs.order.push(crate::TabItem::Document(id));
                    } else {
                        let _ = self.close(id);
                    }
                }
                let peers: Vec<PeerId> = host
                    .peers
                    .keys()
                    .chain(host.knocks.keys())
                    .copied()
                    .collect();
                for peer in peers {
                    session
                        .link
                        .remove(peer, ErrorCode::Closed, "the host stopped sharing");
                }
                let documents: Vec<DocumentId> = host
                    .offered
                    .values()
                    .filter_map(|offered| offered.document)
                    .collect();
                for id in documents {
                    self.stop_sharing(id);
                }
            }
            Side::Guest(guest) => {
                let _ = session.link.send(
                    PeerId::HOST,
                    Message::Leave {
                        reason: "left".to_owned(),
                    },
                );
                session
                    .link
                    .remove(PeerId::HOST, ErrorCode::Closed, "left the session");
                let documents: Vec<DocumentId> = guest
                    .files
                    .values()
                    .filter_map(|file| file.document)
                    .collect();
                for id in documents {
                    self.stop_sharing(id);
                }
            }
        }
        self.emit(EngineEvent::SessionChanged);
    }

    /// The files of the session: those offered, for a host; those the host offered, for a
    /// guest, once its manifest has arrived whole.
    #[must_use]
    pub fn session_manifest(&self) -> Vec<ManifestFile> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Host(host)) => host
                .offered
                .values()
                .map(|offered| offered.entry.clone())
                .collect(),
            Some(Side::Guest(guest)) if guest.manifest_done => guest.manifest.clone(),
            _ => Vec::new(),
        }
    }

    /// The document for session file `file`, once open.
    #[must_use]
    pub fn session_document(&self, file: u64) -> Option<DocumentId> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Host(host)) => host.offered.get(&file).and_then(|offered| offered.document),
            Some(Side::Guest(guest)) => guest.files.get(&file).and_then(|file| file.document),
            None => None,
        }
    }

    /// Connections waiting for the host to admit or deny them, with the names they gave.
    #[must_use]
    pub fn session_knocks(&self) -> Vec<(PeerId, String)> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Host(host)) => host
                .knocks
                .iter()
                .map(|(peer, name)| (*peer, name.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The admitted participants, for a host; for a guest, itself once joined.
    #[must_use]
    pub fn participants(&self) -> Vec<Participant> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Host(host)) => host
                .peers
                .iter()
                .map(|(peer, entry)| Participant {
                    peer: *peer,
                    name: entry.name.clone(),
                    role: entry.role,
                    connected: entry.connected,
                })
                .collect(),
            Some(Side::Guest(guest)) => guest
                .roster
                .iter()
                .map(|member| Participant {
                    peer: member.peer,
                    name: member.name.clone(),
                    role: member.role,
                    connected: member.connected && guest.connected,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// This guest's own number and role in the session, once joined.
    #[must_use]
    pub fn session_me(&self) -> Option<(PeerId, Role)> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Guest(guest)) => guest.me,
            _ => None,
        }
    }

    /// How many of this guest's changes to shared `file` the host has not yet acknowledged:
    /// sent (or waiting to be sent) and not known to be applied there.
    #[must_use]
    pub fn session_unacknowledged(&self, file: u64) -> usize {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Guest(guest)) => guest
                .files
                .get(&file)
                .map_or(0, |state| state.unacked.len()),
            _ => 0,
        }
    }

    /// The name of the folder shared: the host's own, or the one the host named to a guest.
    #[must_use]
    pub fn session_workspace(&self) -> Option<String> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Host(host)) => Some(host.workspace.clone()),
            Some(Side::Guest(guest)) => guest.workspace.clone(),
            None => None,
        }
    }

    /// Whether this engine hosts the session.
    #[must_use]
    pub fn is_session_host(&self) -> bool {
        matches!(
            self.collab_session.as_ref().map(|session| &session.side),
            Some(Side::Host(_))
        )
    }

    /// The selections other participants last showed in document `id`.
    #[must_use]
    pub fn peer_selections(&self, id: DocumentId) -> Vec<PeerSelection> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Host(host)) => {
                let Some(file) = host
                    .offered
                    .iter()
                    .find(|(_, offered)| offered.document == Some(id))
                    .map(|(file, _)| *file)
                else {
                    return Vec::new();
                };
                host.peers
                    .iter()
                    .filter(|(_, entry)| entry.connected)
                    .filter_map(|(peer, entry)| {
                        let selection = entry.files.get(&file)?.selection.clone()?;
                        Some(PeerSelection {
                            peer: *peer,
                            selection,
                        })
                    })
                    .collect()
            }
            Some(Side::Guest(guest)) if guest.connected => guest
                .files
                .values()
                .filter(|file| file.document == Some(id))
                .flat_map(|file| {
                    file.selections
                        .iter()
                        .filter(|(peer, _)| {
                            guest
                                .roster
                                .iter()
                                .any(|member| member.peer == **peer && member.connected)
                        })
                        .map(|(peer, selection)| PeerSelection {
                            peer: *peer,
                            selection: selection.clone(),
                        })
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Lets a knocking connection in as a Viewer and sends it the manifest. Nothing about the
    /// workspace reaches a guest before this.
    pub fn admit_peer(&mut self, peer: PeerId) -> Result<(), EngineError> {
        let session = self
            .collab_session
            .as_mut()
            .ok_or_else(|| refused("no session"))?;
        let Side::Host(host) = &mut session.side else {
            return Err(refused("only the host admits"));
        };
        if host.peers.len() + 1 >= MAX_PARTICIPANTS {
            return Err(refused("the session is full"));
        }
        let name = host
            .knocks
            .remove(&peer)
            .ok_or_else(|| refused("nobody is waiting with that number"))?;
        session
            .link
            .admit(peer)
            .map_err(|error| refused(&error.to_string()))?;
        host.peers.insert(
            peer,
            Peer {
                name,
                role: Role::Viewer,
                connected: true,
                files: BTreeMap::new(),
                told_roster: None,
            },
        );
        let entries: Vec<ManifestFile> = host
            .offered
            .values()
            .map(|offered| offered.entry.clone())
            .collect();
        let mut parts = entries.chunks(MANIFEST_PART).peekable();
        if parts.peek().is_none() {
            let _ = session.link.send(
                peer,
                Message::Manifest {
                    files: Vec::new(),
                    more: false,
                },
            );
        }
        while let Some(part) = parts.next() {
            let _ = session.link.send(
                peer,
                Message::Manifest {
                    files: part.to_vec(),
                    more: parts.peek().is_some(),
                },
            );
        }
        let _ = session.link.send(
            peer,
            Message::Components {
                components: host.components.clone(),
            },
        );
        self.emit(EngineEvent::SessionChanged);
        Ok(())
    }

    /// Turns a knocking connection away, or removes a participant: its connection closes and it
    /// cannot resume.
    pub fn remove_peer(&mut self, peer: PeerId) {
        let Some(session) = self.collab_session.as_mut() else {
            return;
        };
        let Side::Host(host) = &mut session.side else {
            return;
        };
        let knocked = host.knocks.remove(&peer).is_some();
        let admitted = host.peers.remove(&peer).is_some();
        if knocked || admitted {
            let (code, message) = if knocked {
                (ErrorCode::Denied, "the host did not let you in")
            } else {
                (ErrorCode::Closed, "the host removed you from the session")
            };
            session.link.remove(peer, code, message);
            self.emit(EngineEvent::SessionChanged);
        }
    }

    /// Changes what a participant may do. Taking editing away applies to every update processed
    /// from now on, whenever it was sent.
    pub fn set_peer_role(&mut self, peer: PeerId, role: Role) -> Result<(), EngineError> {
        if role == Role::Owner {
            return Err(refused("only the host owns the session"));
        }
        let session = self
            .collab_session
            .as_mut()
            .ok_or_else(|| refused("no session"))?;
        let Side::Host(host) = &mut session.side else {
            return Err(refused("only the host changes roles"));
        };
        let entry = host
            .peers
            .get_mut(&peer)
            .ok_or_else(|| refused("no such participant"))?;
        entry.role = role;
        if entry.connected {
            let _ = session.link.send(peer, Message::RoleChanged { peer, role });
        }
        self.emit(EngineEvent::SessionChanged);
        Ok(())
    }

    /// Asks the host for session file `file`; its document opens when the snapshot arrives.
    pub fn open_session_file(&mut self, file: u64) -> Result<(), EngineError> {
        let session = self
            .collab_session
            .as_mut()
            .ok_or_else(|| refused("no session"))?;
        let Side::Guest(guest) = &mut session.side else {
            return Err(refused("the host opens files itself"));
        };
        if !guest.manifest.iter().any(|entry| entry.file == file) {
            return Err(refused("the host does not offer that file"));
        }
        if guest.files.contains_key(&file) {
            return Ok(());
        }
        if guest.files.len() >= MAX_OPEN_FILES {
            return Err(refused("too many shared files are open"));
        }
        session
            .link
            .send(PeerId::HOST, Message::OpenFile { file })
            .map_err(|error| refused(&error.to_string()))?;
        guest.files.insert(file, GuestFile::default());
        Ok(())
    }

    /// Whether a guest's link to the host is up.
    #[must_use]
    pub fn session_connected(&self) -> bool {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Guest(guest)) => guest.connected,
            Some(Side::Host(_)) => true,
            None => false,
        }
    }

    /// Asks the host to save shared document `id` (COLLAB 5/8): only the host writes, and only
    /// if this guest may edit. Returns the request's number; the answer is
    /// [`Engine::shared_save_result`].
    pub fn request_shared_save(&mut self, id: DocumentId) -> Result<u64, EngineError> {
        let session = self
            .collab_session
            .as_mut()
            .ok_or_else(|| refused("no session"))?;
        let Side::Guest(guest) = &mut session.side else {
            return Err(refused("the host saves its files itself"));
        };
        if !guest.connected {
            return Err(refused("not connected to the host"));
        }
        let (file, epoch) = guest
            .files
            .iter()
            .find(|(_, state)| state.document == Some(id))
            .map(|(file, state)| (*file, state.epoch))
            .ok_or_else(|| refused("not a shared document"))?;
        let request = guest.next_request;
        guest.next_request += 1;
        session
            .link
            .send(
                PeerId::HOST,
                Message::SaveRequest {
                    file,
                    epoch,
                    request,
                },
            )
            .map_err(|error| refused(&error.to_string()))?;
        guest.requests.insert(request, file);
        Ok(request)
    }

    /// The host's answer to this guest's last save request for document `id`, with its number.
    #[must_use]
    pub fn shared_save_result(&self, id: DocumentId) -> Option<(u64, SaveOutcome)> {
        match self.collab_session.as_ref().map(|session| &session.side) {
            Some(Side::Guest(guest)) => guest
                .files
                .values()
                .find(|state| state.document == Some(id))?
                .last_save
                .clone(),
            _ => None,
        }
    }

    /// Whether shared document `id`'s text is the text in the host's file. A host knows; a
    /// guest compares its own text with the revision the host last announced, and while out of
    /// touch knows nothing (`Unknown`), whatever it was told before.
    #[must_use]
    pub fn shared_save_state(&self, id: DocumentId) -> Option<SharedSaveState> {
        if let Some(state) = self.host_save_state(id) {
            return Some(state);
        }
        let Some(Side::Guest(guest)) = self.collab_session.as_ref().map(|session| &session.side)
        else {
            return None;
        };
        let state = guest
            .files
            .values()
            .find(|state| state.document == Some(id))?;
        if !guest.connected || state.waiting || state.resuming {
            return Some(SharedSaveState::Unknown);
        }
        if state.conflict {
            return Some(SharedSaveState::Conflict);
        }
        let document = self.document(id)?;
        // Hashed once a version, however often the state is asked for.
        let mut hashed = state.hashed.borrow_mut();
        if hashed
            .as_ref()
            .is_none_or(|(version, _)| *version != document.version)
        {
            *hashed = Some((document.version, revision_of(&document.text_string())));
        }
        let own = hashed.as_ref().map(|(_, revision)| revision.as_str());
        Some(match &state.saved {
            Some(revision) if Some(revision.as_str()) == own => SharedSaveState::Saved {
                revision: revision.clone(),
            },
            _ => SharedSaveState::Unsaved,
        })
    }

    /// Shows this participant's selection in shared document `id`, from `anchor` to `head`
    /// (byte offsets), to the others: sent at once, or once `AWARENESS_INTERVAL` has passed
    /// since the last, the latest selection winning.
    pub fn share_selection(&mut self, id: DocumentId, anchor: usize, head: usize) {
        let Some(selection) = self.shared_selection(id, anchor, head) else {
            return;
        };
        let now = self.clock.now();
        let Some(mut session) = self.collab_session.take() else {
            return;
        };
        if let Some(file) = Self::session_file(&session, id) {
            let presence = session.presence.entry(file).or_default();
            if presence.throttle.offer(now) {
                presence.waiting = None;
                Self::send_presence(&mut session, file, selection);
            } else {
                presence.waiting = Some(selection);
            }
        }
        self.collab_session = Some(session);
    }

    /// The other participants' carets and selections in shared document `id`, where they are
    /// now, with their names.
    #[must_use]
    pub fn peer_carets(&self, id: DocumentId) -> Vec<PeerCaret> {
        let names: BTreeMap<PeerId, String> =
            match self.collab_session.as_ref().map(|session| &session.side) {
                Some(Side::Host(host)) => host
                    .peers
                    .iter()
                    .map(|(peer, entry)| (*peer, entry.name.clone()))
                    .collect(),
                Some(Side::Guest(guest)) => guest
                    .roster
                    .iter()
                    .map(|member| (member.peer, member.name.clone()))
                    .collect(),
                None => return Vec::new(),
            };
        self.peer_selections(id)
            .into_iter()
            .filter_map(|shown| {
                let (anchor, head) = self.shared_resolve(id, &shown.selection)?;
                Some(PeerCaret {
                    peer: shown.peer,
                    name: names.get(&shown.peer).cloned().unwrap_or_default(),
                    anchor,
                    head,
                })
            })
            .collect()
    }

    /// The session's number for document `id`.
    fn session_file(session: &Session, id: DocumentId) -> Option<u64> {
        match &session.side {
            Side::Host(host) => host
                .offered
                .iter()
                .find(|(_, offered)| offered.document == Some(id))
                .map(|(file, _)| *file),
            Side::Guest(guest) => guest
                .files
                .iter()
                .find(|(_, state)| state.document == Some(id))
                .map(|(file, _)| *file),
        }
    }

    /// Sends this participant's `selection` in `file`: a host to every live guest with it
    /// open, a guest to the host, which forwards it.
    fn send_presence(session: &mut Session, file: u64, selection: RelativeSelection) {
        match &session.side {
            Side::Host(host) => {
                let targets: Vec<(PeerId, u32)> = host
                    .peers
                    .iter()
                    .filter(|(_, entry)| entry.connected)
                    .filter_map(|(peer, entry)| {
                        let open = entry.files.get(&file).filter(|open| open.live)?;
                        Some((*peer, open.epoch))
                    })
                    .collect();
                for (peer, epoch) in targets {
                    let _ = session.link.send(
                        peer,
                        Message::Awareness {
                            file,
                            epoch,
                            peer: PeerId::HOST,
                            selection: selection.clone(),
                        },
                    );
                }
            }
            Side::Guest(guest) => {
                let Some((me, _)) = guest.me else {
                    return;
                };
                let Some(state) = guest.files.get(&file).filter(|state| !state.waiting) else {
                    return;
                };
                if guest.connected {
                    let _ = session.link.send(
                        PeerId::HOST,
                        Message::Awareness {
                            file,
                            epoch: state.epoch,
                            peer: me,
                            selection,
                        },
                    );
                }
            }
        }
    }

    /// Sends the selections that waited for their interval.
    fn flush_presence(&mut self, session: &mut Session) {
        let now = self.clock.now();
        let due: Vec<(u64, RelativeSelection)> = session
            .presence
            .iter_mut()
            .filter_map(|(file, presence)| {
                if presence.waiting.is_some() && presence.throttle.take_due(now) {
                    Some((*file, presence.waiting.take()?))
                } else {
                    None
                }
            })
            .collect();
        for (file, selection) in due {
            Self::send_presence(session, file, selection);
        }
    }

    /// Due at once while events wait or one of the session's documents has changes to send:
    /// only those its flush sends, so nothing else is due for ever.
    pub(crate) fn due_session(&self) -> Option<std::time::Instant> {
        let session = self.collab_session.as_ref()?;
        let documents: Vec<DocumentId> = match &session.side {
            Side::Host(host) => host
                .offered
                .values()
                .filter_map(|offered| offered.document)
                .collect(),
            Side::Guest(guest) => guest
                .files
                .values()
                .filter_map(|file| file.document)
                .collect(),
        };
        let outgoing = documents.iter().any(|id| {
            self.shared
                .get(id)
                .is_some_and(crate::collab::SharedDocument::has_outbox)
        });
        // A file saved, or changed on disk, since the peers were told.
        let unannounced = match &session.side {
            Side::Host(host) => host.offered.iter().any(|(file, offered)| {
                offered.document.is_some_and(|id| {
                    self.shared.contains_key(&id) && {
                        let status = self.file_state(id);
                        host.peers.values().any(|entry| {
                            entry.connected
                                && entry.files.get(file).is_some_and(|open| {
                                    open.live && open.told.as_ref() != Some(&status)
                                })
                        })
                    }
                })
            }),
            Side::Guest(_) => false,
        };
        let roster = match &session.side {
            Side::Host(host) => {
                let members = Self::roster(host);
                host.peers
                    .values()
                    .any(|entry| entry.connected && entry.told_roster.as_ref() != Some(&members))
            }
            Side::Guest(_) => false,
        };
        let presence = session
            .presence
            .values()
            .filter(|presence| presence.waiting.is_some())
            .filter_map(|presence| presence.throttle.due())
            .min();
        let now = (session.backlog || outgoing || unannounced || roster).then(|| self.clock.now());
        [now, presence].into_iter().flatten().min()
    }

    /// Handles what the link brought, at most [`SESSION_EVENTS_PER_PUMP`] events, and sends
    /// the local author's updates. Returns the number of events handled.
    pub(crate) fn pump_session(&mut self) -> usize {
        let Some(mut session) = self.collab_session.take() else {
            return 0;
        };
        let mut handled = 0;
        let mut changed = false;
        while handled < SESSION_EVENTS_PER_PUMP {
            let Some(event) = session.link.try_recv() else {
                break;
            };
            handled += 1;
            changed |= match &mut session.side {
                Side::Host(_) => self.host_event(&mut session, event),
                Side::Guest(_) => self.guest_event(&mut session, event),
            };
        }
        session.backlog = handled == SESSION_EVENTS_PER_PUMP;
        match &session.side {
            Side::Host(_) => self.host_flush(&mut session),
            Side::Guest(_) => self.guest_flush(&mut session),
        }
        self.flush_presence(&mut session);
        self.collab_session = Some(session);
        if changed {
            self.emit(EngineEvent::SessionChanged);
        }
        handled
    }

    // --- the host ----------------------------------------------------------------------------

    /// Returns whether who is in the session, or what they may do, changed.
    fn host_event(&mut self, session: &mut Session, event: LinkEvent) -> bool {
        let Side::Host(host) = &mut session.side else {
            return false;
        };
        match event {
            LinkEvent::Knock { peer, name } => {
                if host.peers.contains_key(&peer) {
                    return false;
                }
                host.knocks.insert(peer, name);
                true
            }
            LinkEvent::Resumed { peer } => {
                let Some(entry) = host.peers.get_mut(&peer) else {
                    return false;
                };
                entry.connected = true;
                entry.told_roster = None;
                if entry.role != Role::Viewer {
                    let role = entry.role;
                    let _ = session.link.send(peer, Message::RoleChanged { peer, role });
                }
                let files: Vec<u64> = entry.files.keys().copied().collect();
                for file in files {
                    self.host_resume_file(session, peer, file);
                }
                true
            }
            LinkEvent::Gone { peer, .. } => {
                let knocked = host.knocks.remove(&peer).is_some();
                let Some(entry) = host.peers.get_mut(&peer) else {
                    return knocked;
                };
                entry.connected = false;
                for file in entry.files.values_mut() {
                    file.live = false;
                }
                true
            }
            LinkEvent::Message { peer, message } => {
                if !host.peers.contains_key(&peer) {
                    return false;
                }
                if let Err(reason) = self.host_message(session, peer, message) {
                    self.drop_peer(session, peer, ErrorCode::Protocol, &reason);
                    return true;
                }
                false
            }
        }
    }

    /// The link to `peer` came back: if `file`'s epoch is unchanged, tells the peer the last of
    /// its updates applied, and waits for its resynchronisation; otherwise sends the new epoch's
    /// snapshot.
    fn host_resume_file(&mut self, session: &mut Session, peer: PeerId, file: u64) {
        let Side::Host(host) = &mut session.side else {
            return;
        };
        let current = host
            .offered
            .get(&file)
            .and_then(|offered| offered.document)
            .and_then(|id| self.shared_file(id))
            .map(|(_, epoch, _)| epoch);
        let Some(state) = host
            .peers
            .get_mut(&peer)
            .and_then(|entry| entry.files.get_mut(&file))
        else {
            return;
        };
        if current == Some(state.epoch) {
            state.live = false;
            let (epoch, seq) = (state.epoch, state.applied);
            let _ = session
                .link
                .send(peer, Message::UpdateAck { file, epoch, seq });
        } else {
            self.host_snapshot(session, peer, file);
        }
    }

    /// Handles one message from admitted `peer`. An error is a breach of the protocol that no
    /// honest guest commits: the caller closes the connection.
    fn host_message(
        &mut self,
        session: &mut Session,
        peer: PeerId,
        message: Message,
    ) -> Result<(), String> {
        match message {
            Message::OpenFile { file } => self.host_open(session, peer, file),
            Message::CloseFile { file } => {
                if let Side::Host(host) = &mut session.side {
                    if let Some(entry) = host.peers.get_mut(&peer) {
                        entry.files.remove(&file);
                    }
                }
                Ok(())
            }
            Message::Update {
                file,
                epoch,
                seq,
                base,
                update,
            } => self.host_update(session, peer, file, (epoch, base), seq, &update),
            Message::Awareness {
                file,
                epoch,
                selection,
                ..
            } => self.host_awareness(session, peer, file, epoch, selection),
            Message::Resync { file, .. } => {
                // In the current epoch or one before: either way the peer starts again from
                // the host's text. One behind lost a snapshot in flight.
                self.host_file_state(session, peer, file)?;
                self.host_snapshot(session, peer, file);
                Ok(())
            }
            Message::SaveRequest {
                file,
                epoch,
                request,
            } => {
                self.host_save(session, peer, file, epoch, request);
                Ok(())
            }
            Message::Leave { .. } => {
                if let Side::Host(host) = &mut session.side {
                    host.peers.remove(&peer);
                }
                session.link.remove(peer, ErrorCode::Closed, "left");
                Ok(())
            }
            Message::Error { .. } => Ok(()),
            _ => Err("a message only the host sends".to_owned()),
        }
    }

    fn host_file_state<'a>(
        &self,
        session: &'a mut Session,
        peer: PeerId,
        file: u64,
    ) -> Result<&'a mut PeerFile, String> {
        let Side::Host(host) = &mut session.side else {
            return Err("not hosting".to_owned());
        };
        host.peers
            .get_mut(&peer)
            .and_then(|entry| entry.files.get_mut(&file))
            .ok_or_else(|| "a file the peer has not opened".to_owned())
    }

    /// Opens offered `file` for `peer`: the host's shared document for it, then a snapshot.
    fn host_open(&mut self, session: &mut Session, peer: PeerId, file: u64) -> Result<(), String> {
        let Side::Host(host) = &session.side else {
            return Err("not hosting".to_owned());
        };
        if !host.offered.contains_key(&file) {
            return Err("a file the host does not offer".to_owned());
        }
        let entry = host.peers.get(&peer).ok_or("not admitted")?;
        if entry.files.contains_key(&file) {
            return Err("the file is already open".to_owned());
        }
        if entry.files.len() >= MAX_OPEN_FILES {
            return Err("too many files open".to_owned());
        }
        if self.host_ensure_shared(session, file).is_err() {
            let _ = session.link.send(
                peer,
                Message::Error {
                    code: ErrorCode::Stale,
                    message: CANNOT_OPEN.to_owned(),
                },
            );
            return Ok(());
        }
        self.host_snapshot(session, peer, file);
        Ok(())
    }

    /// Makes sure offered `file` has a shared document: opened (its path confined again now)
    /// and shared on the first request, and again in a later epoch if the host closed it
    /// meanwhile. Returns whether it had to be shared anew.
    fn host_ensure_shared(
        &mut self,
        session: &mut Session,
        file: u64,
    ) -> Result<bool, EngineError> {
        let Side::Host(host) = &mut session.side else {
            return Err(refused("not hosting"));
        };
        let offered = host
            .offered
            .get(&file)
            .ok_or_else(|| refused("not offered"))?;
        if offered
            .document
            .is_some_and(|id| self.shared_file(id).is_some())
        {
            return Ok(false);
        }
        let path = offered.path.clone();
        let next_epoch = offered.epoch + 1;
        let access = host.access.clone();
        let me = fresh_client(&mut host.clients);
        let id = self.open_for_session(&access, &path)?;
        self.share_document_at(id, SharedFileId(file), me, next_epoch)?;
        if let Side::Host(host) = &mut session.side {
            if let Some(offered) = host.offered.get_mut(&file) {
                offered.document = Some(id);
            }
        }
        Ok(true)
    }

    /// The host's document for `path`: the one open, with any unsaved text, else the file read
    /// now, after checking again that it is inside the shared folder.
    fn open_for_session(
        &mut self,
        access: &FileAccess,
        path: &Path,
    ) -> Result<DocumentId, EngineError> {
        if let Some(id) = self.document_for_path(path) {
            return Ok(id);
        }
        if !access.permits(path) {
            return Err(refused("the file now leads outside the shared folder"));
        }
        let text = self.read_text(path)?;
        let id = self.insert_document(path.to_path_buf(), &text, false);
        // Opened for a guest, not the host: kept in the background, out of the host's tabs,
        // until the host opens it (the maintainer's choice for COLLAB 7/8).
        self.forget_tab(crate::TabItem::Document(id));
        self.emit(EngineEvent::DocumentOpened(id));
        Ok(id)
    }

    /// Sends `peer` a checked snapshot of `file` with a new client id, and starts its sequence
    /// numbers again. If taking the snapshot rebuilt the file in a new epoch, every other peer
    /// with it open gets one too.
    fn host_snapshot(&mut self, session: &mut Session, peer: PeerId, file: u64) {
        let Side::Host(host) = &mut session.side else {
            return;
        };
        let Some(id) = host.offered.get(&file).and_then(|offered| offered.document) else {
            return;
        };
        let Ok((update, epoch)) = self.shared_snapshot(id) else {
            let _ = session.link.send(
                peer,
                Message::Error {
                    code: ErrorCode::Stale,
                    message: "the file could not be shared".to_owned(),
                },
            );
            return;
        };
        let status = self.file_state(id);
        let Side::Host(host) = &mut session.side else {
            return;
        };
        let moved = host
            .offered
            .get_mut(&file)
            .map(|offered| std::mem::replace(&mut offered.epoch, epoch) != epoch)
            .unwrap_or(false);
        let Some(entry) = host.peers.get_mut(&peer) else {
            return;
        };
        if !entry.connected {
            // Away: what it has stays recorded, in its old epoch, so that resuming sends it
            // this snapshot (`host_resume_file`).
            if moved {
                self.host_epoch_moved(session, file, Some(peer));
            }
            return;
        }
        let client = fresh_client(&mut host.clients);
        let Some(entry) = host.peers.get_mut(&peer) else {
            return;
        };
        // A snapshot starts the peer again: its selection, of the text before, goes too.
        entry.files.remove(&file);
        let selection = None;
        entry.files.insert(
            file,
            PeerFile {
                epoch,
                client,
                next_in: 1,
                barrier: true,
                applied: 0,
                next_out: 1,
                live: entry.connected,
                selection,
                told: entry.connected.then(|| status.clone()),
            },
        );
        if entry.connected {
            let _ = session.link.send(
                peer,
                Message::Snapshot {
                    file,
                    epoch,
                    client: client.get(),
                    update,
                },
            );
            let (saved, conflict) = status;
            let _ = session.link.send(
                peer,
                Message::FileState {
                    file,
                    epoch,
                    saved,
                    conflict,
                },
            );
        }
        if moved {
            self.host_epoch_moved(session, file, Some(peer));
        }
    }

    /// `file` was rebuilt in a new epoch: every peer with it open, but `except`, gets the new
    /// snapshot.
    fn host_epoch_moved(&mut self, session: &mut Session, file: u64, except: Option<PeerId>) {
        let Side::Host(host) = &session.side else {
            return;
        };
        let peers: Vec<PeerId> = host
            .peers
            .iter()
            .filter(|(peer, entry)| Some(**peer) != except && entry.files.contains_key(&file))
            .map(|(peer, _)| *peer)
            .collect();
        for peer in peers {
            self.host_snapshot(session, peer, file);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn host_update(
        &mut self,
        session: &mut Session,
        peer: PeerId,
        file: u64,
        (epoch, base): (u32, u64),
        seq: u64,
        update: &EncodedUpdate,
    ) -> Result<(), String> {
        let Side::Host(host) = &session.side else {
            return Err("not hosting".to_owned());
        };
        let opened = host
            .offered
            .get(&file)
            .and_then(|offered| offered.document)
            .is_some();
        if !opened {
            return Err("a file the peer has not opened".to_owned());
        }
        let reshared = match self.host_ensure_shared(session, file) {
            Ok(reshared) => reshared,
            Err(_) => {
                // The host closed the file and it cannot be opened again (gone, or now leading
                // outside the folder): the peer did nothing wrong.
                let _ = session.link.send(
                    peer,
                    Message::Error {
                        code: ErrorCode::Stale,
                        message: CANNOT_OPEN.to_owned(),
                    },
                );
                return Ok(());
            }
        };
        if reshared {
            // The host closed the file and it was shared anew: everyone starts from its text,
            // and this update, of the old epoch, is spent.
            self.host_epoch_moved(session, file, None);
            return Ok(());
        }
        let Side::Host(host) = &mut session.side else {
            return Err("not hosting".to_owned());
        };
        let id = host
            .offered
            .get(&file)
            .and_then(|offered| offered.document)
            .ok_or("a file the peer has not opened")?;
        if self
            .shared_file(id)
            .is_some_and(|(_, current, _)| current != epoch)
        {
            // The host started the file again (reloaded from disk, or rebuilt) and has not yet
            // told the peer, which wrote this in the epoch before: its snapshot follows in this
            // pump (`host_flush`).
            return Ok(());
        }
        let role = host.peers.get(&peer).map(|entry| entry.role);
        let state = host
            .peers
            .get_mut(&peer)
            .and_then(|entry| entry.files.get_mut(&file))
            .ok_or("a file the peer has not opened")?;
        if state.epoch != epoch || state.client.get() != base {
            // Sent before the peer had the new epoch's snapshot, or the latest snapshot in this
            // epoch: the peer starts again from that snapshot, without this change. Numbers
            // alone cannot tell, since they start again with each snapshot, and a change that
            // only deletes names no author.
            return Ok(());
        }
        if state.barrier {
            if seq != 1 {
                return Ok(());
            }
            state.barrier = false;
        }
        if seq < state.next_in {
            // Applied already: sent again after a reconnection, its acknowledgement having
            // crossed it, as happens when the link comes back before the host has read what
            // came before.
            return Ok(());
        }
        if seq != state.next_in {
            return Err(format!("update {seq} out of order"));
        }
        state.next_in += 1;
        let client = state.client;
        if role != Some(Role::Editor) {
            // A role taken away while edits were on their way: the peer starts again from the
            // host's text.
            let _ = session.link.send(
                peer,
                Message::Error {
                    code: ErrorCode::Unauthorised,
                    message: "you may not edit".to_owned(),
                },
            );
            self.host_snapshot(session, peer, file);
            return Ok(());
        }
        match self.apply_shared_update(id, epoch, update, Sender::Guest(client)) {
            Ok(SharedReceipt::Applied) => {
                let Side::Host(host) = &mut session.side else {
                    return Ok(());
                };
                if let Some(state) = host
                    .peers
                    .get_mut(&peer)
                    .and_then(|entry| entry.files.get_mut(&file))
                {
                    state.applied = seq;
                }
                let _ = session
                    .link
                    .send(peer, Message::UpdateAck { file, epoch, seq });
                self.host_forward(session, file, Some(peer), update);
                Ok(())
            }
            Ok(SharedReceipt::Rebuilt { .. } | SharedReceipt::NeedsResync) => {
                self.drop_peer(
                    session,
                    peer,
                    ErrorCode::Protocol,
                    "an update the host could not use",
                );
                self.host_epoch_moved(session, file, None);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// Sends `update` of `file` to every live peer with it open but `except`.
    fn host_forward(
        &mut self,
        session: &mut Session,
        file: u64,
        except: Option<PeerId>,
        update: &EncodedUpdate,
    ) {
        let Side::Host(host) = &mut session.side else {
            return;
        };
        let mut slow = Vec::new();
        for (peer, entry) in &mut host.peers {
            if Some(*peer) == except || !entry.connected {
                continue;
            }
            let Some(state) = entry.files.get_mut(&file).filter(|state| state.live) else {
                continue;
            };
            let message = Message::Update {
                file,
                epoch: state.epoch,
                seq: state.next_out,
                base: 0,
                update: update.clone(),
            };
            state.next_out += 1;
            if session.link.send(*peer, message).is_err() {
                slow.push(*peer);
            }
        }
        for peer in slow {
            if let Some(entry) = host.peers.get_mut(&peer) {
                entry.connected = false;
                for state in entry.files.values_mut() {
                    state.live = false;
                }
            }
        }
    }

    fn host_awareness(
        &mut self,
        session: &mut Session,
        peer: PeerId,
        file: u64,
        epoch: u32,
        selection: RelativeSelection,
    ) -> Result<(), String> {
        let selection = RelativeSelection::from_wire(selection.anchor, selection.head)
            .map_err(|error| error.to_string())?;
        let state = self.host_file_state(session, peer, file)?;
        if state.epoch != epoch {
            return Ok(());
        }
        state.selection = Some(selection.clone());
        let Side::Host(host) = &session.side else {
            return Ok(());
        };
        let targets: Vec<(PeerId, u32)> = host
            .peers
            .iter()
            .filter(|(other, entry)| **other != peer && entry.connected)
            .filter_map(|(other, entry)| {
                let state = entry.files.get(&file).filter(|state| state.live)?;
                Some((*other, state.epoch))
            })
            .collect();
        for (other, epoch) in targets {
            let _ = session.link.send(
                other,
                Message::Awareness {
                    file,
                    epoch,
                    peer,
                    selection: selection.clone(),
                },
            );
        }
        if let Some(id) = host.offered.get(&file).and_then(|offered| offered.document) {
            self.emit(EngineEvent::PresenceChanged(id));
        }
        Ok(())
    }

    /// A guest's request to save `file`: only an Editor may, only for the file's current epoch,
    /// and only the host writes, through its ordinary save and its conflict check. A request for
    /// a file already saved writes nothing and names the revision on disk. The save is made at
    /// once and waited for, as the host's own Save is.
    fn host_save(
        &mut self,
        session: &mut Session,
        peer: PeerId,
        file: u64,
        epoch: u32,
        request: u64,
    ) {
        let Side::Host(host) = &session.side else {
            return;
        };
        let editor = host
            .peers
            .get(&peer)
            .is_some_and(|entry| entry.role == Role::Editor);
        let id = host.offered.get(&file).and_then(|offered| offered.document);
        let current = id
            .and_then(|id| self.shared_file(id))
            .map(|(_, epoch, _)| epoch);
        let outcome = match id {
            _ if !editor => SaveOutcome::PermissionDenied,
            Some(id) if current == Some(epoch) => match self.host_save_state(id) {
                Some(SharedSaveState::Saved { revision }) => SaveOutcome::Saved { revision },
                Some(SharedSaveState::Conflict) => SaveOutcome::Conflict,
                // The revision is the one written: the snapshot the save took.
                _ => match self.save(id) {
                    Ok(()) => self.shared_saved_revision(id).map_or_else(
                        || SaveOutcome::Failed {
                            reason: "the save named no revision".to_owned(),
                        },
                        |revision| SaveOutcome::Saved { revision },
                    ),
                    Err(EngineError::SaveConflict { .. }) => SaveOutcome::Conflict,
                    // Never the engine's error, which names the host's paths.
                    Err(_) => SaveOutcome::Failed {
                        reason: "the host could not write the file".to_owned(),
                    },
                },
            },
            _ => SaveOutcome::Stale,
        };
        let _ = session
            .link
            .send(peer, Message::SaveResult { request, outcome });
    }

    /// Closes `peer`'s connection for good.
    fn drop_peer(&mut self, session: &mut Session, peer: PeerId, code: ErrorCode, reason: &str) {
        if let Side::Host(host) = &mut session.side {
            host.peers.remove(&peer);
            host.knocks.remove(&peer);
        }
        session.link.remove(peer, code, reason);
    }

    /// Sends the host's own changes to every live peer with the file open.
    /// Who is in the session, the host first, as guests are told it.
    fn roster(host: &Host) -> Vec<Member> {
        std::iter::once(Member {
            peer: PeerId::HOST,
            name: host.name.clone(),
            role: Role::Owner,
            connected: true,
        })
        .chain(host.peers.iter().map(|(peer, entry)| Member {
            peer: *peer,
            name: entry.name.clone(),
            role: entry.role,
            connected: entry.connected,
        }))
        .collect()
    }

    /// Tells each connected peer who is in the session, if it was told otherwise.
    fn host_announce_roster(session: &mut Session) {
        let Side::Host(host) = &mut session.side else {
            return;
        };
        let members = Self::roster(host);
        let workspace = host.workspace.clone();
        for (peer, entry) in &mut host.peers {
            if !entry.connected || entry.told_roster.as_ref() == Some(&members) {
                continue;
            }
            entry.told_roster = Some(members.clone());
            let _ = session.link.send(
                *peer,
                Message::Roster {
                    workspace: workspace.clone(),
                    members: members.clone(),
                },
            );
        }
    }

    fn host_flush(&mut self, session: &mut Session) {
        Self::host_announce_roster(session);
        let Side::Host(host) = &session.side else {
            return;
        };
        let open: Vec<(u64, DocumentId)> = host
            .offered
            .iter()
            .filter_map(|(file, offered)| Some((*file, offered.document?)))
            .collect();
        for (file, id) in open {
            for update in self.take_shared_updates(id) {
                self.host_forward(session, file, None, &update);
            }
            let epoch = self.shared_file(id).map(|(_, epoch, _)| epoch);
            let Side::Host(host) = &session.side else {
                return;
            };
            let known = host.offered.get(&file).map(|offered| offered.epoch);
            if epoch.is_some() && epoch != known {
                // Rebuilt by the engine (a projection failure on a local edit): everyone starts
                // again from the host's text.
                if let Side::Host(host) = &mut session.side {
                    if let Some(offered) = host.offered.get_mut(&file) {
                        offered.epoch = epoch.unwrap_or_default();
                    }
                }
                self.host_epoch_moved(session, file, None);
            }
            self.host_announce(session, file, id);
        }
    }

    /// The saved revision (while the shared text is it) and the conflict of document `id`.
    fn file_state(&self, id: DocumentId) -> FileStatus {
        match self.host_save_state(id) {
            Some(SharedSaveState::Saved { revision }) => (Some(revision), false),
            Some(SharedSaveState::Conflict) => (None, true),
            _ => (None, false),
        }
    }

    /// Tells each live peer with `file` open how its file stands, if it was told otherwise.
    fn host_announce(&mut self, session: &mut Session, file: u64, id: DocumentId) {
        let status = self.file_state(id);
        let Side::Host(host) = &mut session.side else {
            return;
        };
        for (peer, entry) in &mut host.peers {
            if !entry.connected {
                continue;
            }
            let Some(open) = entry.files.get_mut(&file).filter(|open| open.live) else {
                continue;
            };
            if open.told.as_ref() == Some(&status) {
                continue;
            }
            open.told = Some(status.clone());
            let _ = session.link.send(
                *peer,
                Message::FileState {
                    file,
                    epoch: open.epoch,
                    saved: status.0.clone(),
                    conflict: status.1,
                },
            );
        }
    }

    // --- a guest ---------------------------------------------------------------------------

    fn guest_event(&mut self, session: &mut Session, event: LinkEvent) -> bool {
        let Side::Guest(guest) = &mut session.side else {
            return false;
        };
        match event {
            LinkEvent::Knock { .. } => false,
            LinkEvent::Resumed { .. } => {
                guest.connected = true;
                for file in guest.files.values_mut() {
                    file.resuming = true;
                }
                true
            }
            LinkEvent::Gone { .. } => {
                guest.connected = false;
                self.guest_refresh_editing(session);
                true
            }
            LinkEvent::Message { peer, message } => {
                if peer != PeerId::HOST {
                    return false;
                }
                self.guest_message(session, message)
            }
        }
    }

    fn guest_message(&mut self, session: &mut Session, message: Message) -> bool {
        let Side::Guest(guest) = &mut session.side else {
            return false;
        };
        match message {
            Message::Join {
                peer,
                role,
                session: id,
                ..
            } => {
                if guest.virtual_root.is_none() {
                    let name: String = id.0.iter().map(|byte| format!("{byte:02x}")).collect();
                    guest.virtual_root = Some(normalize_path(
                        &std::env::temp_dir().join(format!("{SHARED_PREFIX}{name}")),
                    ));
                    guest.rebuild();
                }
                guest.me = Some((peer, role));
                guest.connected = true;
                self.guest_refresh_editing(session);
                true
            }
            Message::Manifest { files, more } => {
                if guest.manifest_done || guest.manifest.len() + files.len() > MAX_MANIFEST_FILES {
                    return false;
                }
                for entry in files {
                    if !guest.manifest.iter().any(|known| known.file == entry.file) {
                        guest.manifest.push(entry);
                    }
                }
                guest.manifest_done = !more;
                if !more {
                    guest.rebuild();
                }
                !more
            }
            Message::Snapshot {
                file,
                epoch,
                client,
                update,
            } => {
                self.guest_snapshot(session, file, epoch, client, &update);
                true
            }
            Message::Update {
                file,
                epoch,
                seq,
                update,
                ..
            } => {
                self.guest_update(session, file, epoch, seq, &update);
                false
            }
            Message::UpdateAck { file, epoch, seq } => {
                self.guest_ack(session, file, epoch, seq);
                false
            }
            Message::Awareness {
                file,
                epoch,
                peer,
                selection,
            } => {
                let Ok(selection) = RelativeSelection::from_wire(selection.anchor, selection.head)
                else {
                    return false;
                };
                if let Some(state) = guest
                    .files
                    .get_mut(&file)
                    .filter(|state| state.epoch == epoch)
                {
                    state.selections.insert(peer, selection);
                    if let Some(id) = state.document {
                        self.emit(EngineEvent::PresenceChanged(id));
                    }
                }
                false
            }
            Message::Components { components } => {
                guest.components = components;
                guest.rebuild();
                false
            }
            Message::Roster { workspace, members } => {
                guest.workspace = Some(workspace);
                guest.roster = members;
                true
            }
            Message::FileState {
                file,
                epoch,
                saved,
                conflict,
            } => {
                let Some(state) = guest
                    .files
                    .get_mut(&file)
                    .filter(|state| state.epoch == epoch)
                else {
                    return false;
                };
                state.saved = saved;
                state.conflict = conflict;
                true
            }
            Message::SaveResult { request, outcome } => {
                let Some(file) = guest.requests.remove(&request) else {
                    return false;
                };
                if let Some(state) = guest.files.get_mut(&file) {
                    state.last_save = Some((request, outcome));
                }
                true
            }
            Message::RoleChanged { peer, role } => {
                if let Some((me, current)) = &mut guest.me {
                    if *me == peer {
                        *current = role;
                    }
                }
                self.guest_refresh_editing(session);
                true
            }
            Message::Leave { .. } => {
                guest.connected = false;
                self.guest_refresh_editing(session);
                true
            }
            _ => false,
        }
    }

    fn guest_snapshot(
        &mut self,
        session: &mut Session,
        file: u64,
        epoch: u32,
        client: u64,
        update: &EncodedUpdate,
    ) {
        let Side::Guest(guest) = &mut session.side else {
            return;
        };
        let Some(me) = ReplicaId::new(client) else {
            return;
        };
        let Some(name) = guest
            .manifest
            .iter()
            .find(|entry| entry.file == file)
            .and_then(|entry| guest.path_of(&entry.path))
            .map(|path| path.to_string_lossy().into_owned())
        else {
            return;
        };
        let Some(state) = guest.files.get_mut(&file) else {
            return;
        };
        let result = match state.document {
            Some(id) => self.reset_shared(id, epoch, update, me).map(|()| id),
            None => self.open_shared(&name, SharedFileId(file), epoch, update, me, false),
        };
        let Side::Guest(guest) = &mut session.side else {
            return;
        };
        let Some(state) = guest.files.get_mut(&file) else {
            return;
        };
        match result {
            Ok(id) => {
                if std::mem::take(&mut state.show) {
                    self.set_active(id);
                }
                let Side::Guest(guest) = &mut session.side else {
                    return;
                };
                let Some(state) = guest.files.get_mut(&file) else {
                    return;
                };
                state.document = Some(id);
                state.epoch = epoch;
                state.selections.clear();
                state.saved = None;
                state.conflict = false;
                state.next_in = 1;
                state.next_out = 1;
                state.base = client;
                state.unacked.clear();
                state.waiting = false;
                if std::mem::take(&mut state.resuming) {
                    // Sent before the host knew the link had come back, it may be answered by
                    // an acknowledgement after which the host waits to be asked: ask now.
                    self.guest_resync(session, file);
                    return;
                }
            }
            Err(_) => {
                // A snapshot this side cannot use: ask again.
                state.waiting = true;
                let resync = state.document.and_then(|id| {
                    Some(Message::Resync {
                        file,
                        epoch: state.epoch,
                        state: self.shared_state_vector(id).ok()?,
                    })
                });
                if let Some(message) = resync {
                    let _ = session.link.send(PeerId::HOST, message);
                }
            }
        }
        self.guest_refresh_editing(session);
    }

    fn guest_update(
        &mut self,
        session: &mut Session,
        file: u64,
        epoch: u32,
        seq: u64,
        update: &EncodedUpdate,
    ) {
        let Side::Guest(guest) = &mut session.side else {
            return;
        };
        let Some(state) = guest.files.get_mut(&file) else {
            return;
        };
        let Some(id) = state.document else {
            return;
        };
        if state.waiting || state.epoch != epoch {
            return;
        }
        let in_order = seq == state.next_in;
        state.next_in += 1;
        let receipt = if in_order {
            self.apply_shared_update(id, epoch, update, Sender::Host)
        } else {
            Ok(SharedReceipt::NeedsResync)
        };
        if !matches!(receipt, Ok(SharedReceipt::Applied)) {
            self.guest_resync(session, file);
        }
    }

    /// Asks the host to resynchronise `file`, refusing edits to it until the answer.
    fn guest_resync(&mut self, session: &mut Session, file: u64) {
        // Changes made here and not yet sent go first, so that the snapshot answering holds
        // them rather than the host applying them after it.
        let pending = match &session.side {
            Side::Guest(guest) => guest.files.get(&file).and_then(|state| state.document),
            Side::Host(_) => None,
        }
        .map(|id| self.take_shared_updates(id))
        .unwrap_or_default();
        let Side::Guest(guest) = &mut session.side else {
            return;
        };
        let connected = guest.connected;
        let Some(state) = guest.files.get_mut(&file) else {
            return;
        };
        for update in pending {
            let seq = state.next_out;
            state.next_out += 1;
            state.unacked.push_back((seq, update.clone()));
            if connected && !state.resuming {
                let _ = session.link.send(
                    PeerId::HOST,
                    Message::Update {
                        file,
                        epoch: state.epoch,
                        seq,
                        base: state.base,
                        update,
                    },
                );
            }
        }
        state.waiting = true;
        let epoch = state.epoch;
        if let Some(vector) = state
            .document
            .and_then(|id| self.shared_state_vector(id).ok())
        {
            let _ = session.link.send(
                PeerId::HOST,
                Message::Resync {
                    file,
                    epoch,
                    state: vector,
                },
            );
        }
        self.guest_refresh_editing(session);
    }

    /// The host applied this guest's updates to `file` up to `seq`. After a reconnection it is
    /// the cue to send again those after it, then ask to be resynchronised.
    fn guest_ack(&mut self, session: &mut Session, file: u64, epoch: u32, seq: u64) {
        let Side::Guest(guest) = &mut session.side else {
            return;
        };
        let Some(state) = guest.files.get_mut(&file) else {
            return;
        };
        if state.epoch != epoch {
            // The host is in another epoch: resuming, this side missed its snapshot.
            if std::mem::take(&mut state.resuming) {
                self.guest_resync(session, file);
            }
            return;
        }
        while state.unacked.front().is_some_and(|(sent, _)| *sent <= seq) {
            state.unacked.pop_front();
        }
        if state.resuming {
            state.resuming = false;
            for (sent, update) in &state.unacked {
                let _ = session.link.send(
                    PeerId::HOST,
                    Message::Update {
                        file,
                        epoch,
                        seq: *sent,
                        base: state.base,
                        update: update.clone(),
                    },
                );
            }
            self.guest_resync(session, file);
            return;
        }
        self.guest_refresh_editing(session);
    }

    /// Sends this guest's changes, keeping each until acknowledged.
    fn guest_flush(&mut self, session: &mut Session) {
        let Side::Guest(guest) = &mut session.side else {
            return;
        };
        // The shared workspace's index follows the shared pages open here: a page closed
        // leaves it.
        let documents = &self.documents;
        let shared = &mut guest.shared;
        guest.indexed.retain(|id, (_, path)| {
            let open = documents.contains_key(id);
            if !open {
                shared.index.remove(path);
            }
            open
        });
        for state in guest.files.values() {
            let Some(id) = state.document else {
                continue;
            };
            let Some(document) = self.documents.get(&id) else {
                continue;
            };
            // While typing goes on the page is parsed once it pauses, the shared index with it.
            if self.pending_index.contains_key(&id) {
                continue;
            }
            if guest.indexed.get(&id).map(|(version, _)| *version) != Some(document.version) {
                // The document's own parse, shared with whatever else reads this version.
                let parsed = document.parsed().clone();
                guest.shared.index.replace(document.path.clone(), parsed);
                guest
                    .indexed
                    .insert(id, (document.version, document.path.clone()));
            }
        }
        // A remote tab closed here: the host stops sending it, and it may be opened again.
        let closed: Vec<u64> = guest
            .files
            .iter()
            .filter(|(_, state)| {
                state
                    .document
                    .is_some_and(|id| !self.documents.contains_key(&id))
            })
            .map(|(file, _)| *file)
            .collect();
        for file in closed {
            guest.files.remove(&file);
            if guest.connected {
                let _ = session.link.send(PeerId::HOST, Message::CloseFile { file });
            }
        }
        let open: Vec<(u64, DocumentId)> = guest
            .files
            .iter()
            .filter_map(|(file, state)| Some((*file, state.document?)))
            .collect();
        let mut full = false;
        for (file, id) in open {
            let updates = self.take_shared_updates(id);
            let Side::Guest(guest) = &mut session.side else {
                return;
            };
            let connected = guest.connected;
            let Some(state) = guest.files.get_mut(&file) else {
                continue;
            };
            for update in updates {
                let seq = state.next_out;
                state.next_out += 1;
                state.unacked.push_back((seq, update.clone()));
                if connected && !state.resuming {
                    let _ = session.link.send(
                        PeerId::HOST,
                        Message::Update {
                            file,
                            epoch: state.epoch,
                            seq,
                            base: state.base,
                            update,
                        },
                    );
                }
            }
            full |= state.unacked.len() >= MAX_QUEUED_FRAMES;
        }
        if full {
            self.guest_refresh_editing(session);
        }
    }

    /// Lets this guest edit each open file only while it is an Editor, connected, not waiting
    /// for a resynchronisation and not too far ahead of the host's acknowledgements.
    fn guest_refresh_editing(&mut self, session: &mut Session) {
        let Side::Guest(guest) = &session.side else {
            return;
        };
        let editor = guest.me.is_some_and(|(_, role)| role == Role::Editor) && guest.connected;
        let documents: Vec<(DocumentId, bool)> = guest
            .files
            .values()
            .filter_map(|state| {
                let id = state.document?;
                let can = editor
                    && !state.waiting
                    && !state.resuming
                    && state.unacked.len() < MAX_QUEUED_FRAMES;
                Some((id, can))
            })
            .collect();
        for (id, can) in documents {
            self.set_shared_editing(id, can);
        }
    }
}

/// A `yrs` client id not given before in the session.
fn fresh_client(clients: &mut BTreeSet<u64>) -> ReplicaId {
    loop {
        let id = ReplicaId::random();
        if clients.insert(id.get()) {
            return id;
        }
    }
}

/// What Share would offer from a folder: the files that may be shared, and how many AsciiDoc
/// files there were left out (hidden, ignored, leading outside, too large or unshowable).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SharePlan {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub left_out: usize,
}

impl Engine {
    /// The indexed AsciiDoc files under `root` Share would offer, as `host_session` checks
    /// them, for the front end to show before anything listens.
    #[must_use]
    pub fn share_plan(&self, root: &Path) -> SharePlan {
        let root =
            normalize_path(&std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()));
        let access = FileAccess::within([root.clone()]);
        let ignore = &self.settings.workspace.ignore;
        let mut plan = SharePlan {
            root: root.clone(),
            ..SharePlan::default()
        };
        let mut candidates: Vec<PathBuf> = self
            .workspace
            .index
            .files()
            .map(|file| normalize_path(&file.path))
            .filter(|path| path.starts_with(&root) && adoc_index::is_asciidoc_path(path))
            .collect();
        candidates.sort();
        candidates.dedup();
        for path in candidates {
            if offer(&access, &root, &path, ignore).is_ok() {
                plan.files.push(path);
            } else {
                plan.left_out += 1;
            }
        }
        plan
    }
}

impl Engine {
    /// Whether host document `id` is open for a guest, so closing its tab keeps it shared in
    /// the background.
    pub(crate) fn session_keeps(&self, id: DocumentId) -> bool {
        let Some(Side::Host(host)) = self.collab_session.as_ref().map(|session| &session.side)
        else {
            return false;
        };
        host.offered
            .iter()
            .filter(|(_, offered)| offered.document == Some(id))
            .any(|(file, _)| {
                host.peers
                    .values()
                    .any(|peer| peer.files.contains_key(file))
            })
    }

    /// Takes a host's shared page out of the tab strip, keeping it open and shared.
    pub(crate) fn hide_shared_tab(&mut self, id: DocumentId) {
        let at = self.forget_tab(crate::TabItem::Document(id));
        if self.active == Some(id) {
            self.active = None;
            self.activate_neighbour(at.unwrap_or(usize::MAX));
            self.emit(EngineEvent::NavigationChanged);
        }
        self.sync_preview_target();
        self.emit(EngineEvent::TabHidden(id));
    }

    /// Whether document `id` is open in a tab, rather than kept in the background.
    #[must_use]
    pub fn has_tab(&self, id: DocumentId) -> bool {
        self.tabs.order.contains(&crate::TabItem::Document(id))
    }
}

/// Why Share and Join refuse while collaboration is off.
const OFF: &str = "collaboration is off: set `[collaboration] enabled = true` in the user settings";

impl Engine {
    /// Whether the collaboration commands are offered: collaboration is on, or a session runs.
    pub(crate) fn collaboration_offered(&self) -> bool {
        self.settings.collaboration.enabled || self.in_session()
    }
}

/// The collaboration commands (COLLAB 6/8). Those that listen, connect or copy an invitation
/// hand their step to the front end, which holds the transport; the others act on the session
/// here, and refuse when there is none.
pub(crate) fn register_collaboration(registry: &mut crate::CommandRegistry) {
    use crate::{
        commands::{ids, register, spec},
        CollaborationStep, CommandArgs, CommandCategory, CommandContext, CommandOutcome,
    };

    fn step(step: CollaborationStep) -> Result<CommandOutcome, EngineError> {
        Ok(CommandOutcome::Collaboration(step))
    }

    fn number(
        command: crate::CommandId,
        args: &CommandArgs,
        key: &str,
    ) -> Result<u64, EngineError> {
        match args {
            CommandArgs::Custom(value) => value.get(key).and_then(serde_json::Value::as_u64),
            _ => None,
        }
        .ok_or(EngineError::InvalidArguments {
            command,
            expected: "a participant or file number",
        })
    }

    fn peer(command: crate::CommandId, args: &CommandArgs) -> Result<PeerId, EngineError> {
        let value = number(command, args, "peer")?;
        u32::try_from(value)
            .map(PeerId)
            .map_err(|_| EngineError::InvalidArguments {
                command,
                expected: "a participant number",
            })
    }

    // Offered only while collaboration is on, or a session runs (so it can always be ended).
    let simple = |id, title| {
        spec(
            id,
            title,
            CommandCategory::Workspace,
            None,
            CommandContext::Collaboration,
        )
    };
    // The Session panel's, shown only during a session.
    let panel = |id, title| {
        spec(
            id,
            title,
            CommandCategory::Workspace,
            None,
            CommandContext::Always,
        )
    };
    register(
        registry,
        simple(ids::COLLAB_SHARE, "Share Workspace…"),
        |engine, _| {
            if !engine.settings().collaboration.enabled {
                return Err(refused(OFF));
            }
            if engine.in_session() {
                return Err(refused("a session is already running"));
            }
            if engine.roots().is_empty() {
                return Err(refused("open a folder to share it"));
            }
            step(CollaborationStep::Share)
        },
    );
    register(
        registry,
        simple(ids::COLLAB_JOIN, "Join Session…"),
        |engine, _| {
            if !engine.settings().collaboration.enabled {
                return Err(refused(OFF));
            }
            if engine.in_session() {
                return Err(refused("a session is already running"));
            }
            step(CollaborationStep::Join)
        },
    );
    register(
        registry,
        simple(ids::COLLAB_COPY_INVITATION, "Copy Invitation"),
        |engine, _| {
            if !engine.is_session_host() {
                return Err(refused("only a host invites"));
            }
            step(CollaborationStep::CopyInvitation)
        },
    );
    register(
        registry,
        simple(ids::COLLAB_STOP, "Stop Sharing"),
        |engine, _| {
            if !engine.is_session_host() {
                return Err(refused("nothing is being shared"));
            }
            engine.end_session();
            step(CollaborationStep::Ended)
        },
    );
    register(
        registry,
        simple(ids::COLLAB_LEAVE, "Leave Session"),
        |engine, _| {
            if !engine.in_session() || engine.is_session_host() {
                return Err(refused("not in someone else's session"));
            }
            engine.end_session();
            step(CollaborationStep::Ended)
        },
    );
    register(
        registry,
        panel(ids::COLLAB_ADMIT, "Admit Participant"),
        |engine, args| {
            engine
                .admit_peer(peer(ids::COLLAB_ADMIT, &args)?)
                .map(|()| CommandOutcome::Done)
        },
    );
    register(
        registry,
        panel(ids::COLLAB_DENY, "Turn Participant Away"),
        |engine, args| {
            let peer = peer(ids::COLLAB_DENY, &args)?;
            if !engine
                .session_knocks()
                .iter()
                .any(|(knock, _)| *knock == peer)
            {
                return Err(refused("nobody is waiting with that number"));
            }
            engine.remove_peer(peer);
            Ok(CommandOutcome::Done)
        },
    );
    register(
        registry,
        panel(ids::COLLAB_GRANT_EDIT, "Let Participant Edit"),
        |engine, args| {
            engine
                .set_peer_role(peer(ids::COLLAB_GRANT_EDIT, &args)?, Role::Editor)
                .map(|()| CommandOutcome::Done)
        },
    );
    register(
        registry,
        panel(ids::COLLAB_REVOKE_EDIT, "Make Participant a Viewer"),
        |engine, args| {
            engine
                .set_peer_role(peer(ids::COLLAB_REVOKE_EDIT, &args)?, Role::Viewer)
                .map(|()| CommandOutcome::Done)
        },
    );
    register(
        registry,
        panel(ids::COLLAB_REMOVE, "Remove Participant"),
        |engine, args| {
            let peer = peer(ids::COLLAB_REMOVE, &args)?;
            if !engine
                .participants()
                .iter()
                .any(|participant| participant.peer == peer)
            {
                return Err(refused("no such participant"));
            }
            engine.remove_peer(peer);
            Ok(CommandOutcome::Done)
        },
    );
    register(
        registry,
        panel(ids::COLLAB_OPEN_FILE, "Open Shared File"),
        |engine, args| {
            let file = number(ids::COLLAB_OPEN_FILE, &args, "file")?;
            if let Some(id) = engine.session_document(file) {
                engine.set_active(id);
                return Ok(CommandOutcome::Opened(id));
            }
            engine
                .open_session_file(file)
                .map(|()| CommandOutcome::Done)
        },
    );
}

/// Commands refused while this engine hosts a session (COLLAB 7/8, #694): each changes the
/// host's workspace beyond editing a page's text, across files or in its folders, which the
/// session cannot carry to its guests and an Editor's role does not cover. Stopping sharing
/// makes them available again.
pub const REFUSED_WHILE_HOSTING: &[crate::CommandId] = &[
    crate::commands::ids::ANCHOR_RENAME,
    crate::commands::ids::PAGE_RENAME,
    crate::commands::ids::PAGE_NEW,
    crate::commands::ids::PAGE_NEW_PARTIAL,
    crate::commands::ids::MODULE_NEW,
    crate::commands::ids::PROJECT_NEW,
    crate::commands::ids::FILE_NEW,
    crate::commands::ids::FILE_DELETE,
    crate::commands::ids::FILE_UNDO_DELETE,
    crate::commands::ids::FILE_RESTORE_DELETED,
    crate::commands::ids::NAV_ADD_PAGE,
    crate::commands::ids::WORKSPACE_REPLACE,
    crate::commands::ids::WORKSPACE_APPLY_PREVIEW,
    crate::commands::ids::EDIT_PLACE_IMAGES,
    crate::commands::ids::WORKSPACE_OPEN_FOLDER,
    crate::commands::ids::WORKSPACE_OPEN_RECENT,
];

/// Commands refused on a guest's shared page, which is the host's file: besides these, every
/// export (`export.*`), which would write the host's text to this computer.
pub const REFUSED_ON_GUEST_PAGES: &[crate::CommandId] = &[
    crate::commands::ids::PAGE_RENAME,
    crate::commands::ids::FILE_DELETE,
    crate::commands::ids::DOCUMENT_LOCAL_HISTORY,
    crate::commands::ids::DOCUMENT_RESTORE_VERSION,
];

impl Engine {
    /// Why `command` is refused in the session, if it is: a structural command while hosting,
    /// or a command on a guest's shared page that only the host's own file could answer.
    pub(crate) fn session_refuses(
        &self,
        command: crate::CommandId,
        args: &crate::CommandArgs,
    ) -> Option<String> {
        if self.is_session_host() && REFUSED_WHILE_HOSTING.contains(&command) {
            return Some(
                "not while sharing: it would change files the session cannot share. Stop \
                 sharing first"
                    .to_owned(),
            );
        }
        let target = match args {
            crate::CommandArgs::Document(id) => Some(*id),
            _ => self.active(),
        };
        let guest_page = target.is_some_and(|id| self.is_shared_guest(id));
        let refused =
            REFUSED_ON_GUEST_PAGES.contains(&command) || command.as_str().starts_with("export.");
        (guest_page && refused)
            .then(|| "a shared page is the host's: only the host can do that".to_owned())
    }
}

/// How the folder of a session's shared pages begins, in the system's temporary folder.
const SHARED_PREFIX: &str = "adocfoundry-shared-";

/// Whether `path` is a shared page's, under a session's virtual folder: never read from disk,
/// during a session or after it, whatever is there.
pub(crate) fn is_shared_path(path: &Path) -> bool {
    let temp = normalize_path(&std::env::temp_dir());
    path.strip_prefix(&temp).is_ok_and(|rest| {
        rest.components().next().is_some_and(|first| {
            first
                .as_os_str()
                .to_string_lossy()
                .starts_with(SHARED_PREFIX)
        })
    })
}

impl Guest {
    /// The path of a shared file in this session's virtual folder.
    fn path_of(&self, display: &str) -> Option<PathBuf> {
        let mut path = self.virtual_root.clone()?;
        for part in display.split('/').filter(|part| !part.is_empty()) {
            path.push(part);
        }
        Some(path)
    }

    /// Builds the shared workspace again from the manifest and the components: what each
    /// shared page's references resolve among.
    fn rebuild(&mut self) {
        let Some(root) = self.virtual_root.clone() else {
            return;
        };
        let mut files = BTreeMap::new();
        for entry in &self.manifest {
            if let Some(path) = self.path_of(&entry.path) {
                files.insert(path, String::new());
            }
        }
        for component in &self.components {
            let Some(dir) = self.path_of(&component.dir) else {
                continue;
            };
            // Names and versions are words (the protocol holds them to it): quoted, they are
            // read back as written.
            let descriptor = format!(
                "name: '{}'\nversion: {}\n",
                component.name,
                component
                    .version
                    .as_deref()
                    .map_or("~".to_owned(), |version| format!("'{version}'"))
            );
            files.insert(dir.join("antora.yml"), descriptor);
        }
        let ignore: [&str; 0] = [];
        let discovered = adoc_antora::discover_antora_files(&[root], &files, &ignore);
        *self.shared = adoc_lang::WorkspaceState {
            antora: discovered.catalog,
            descriptor_issues: discovered.issues,
            ..adoc_lang::WorkspaceState::default()
        };
        self.indexed.clear();
    }
}

impl Engine {
    /// The workspace a page resolves its references in: a guest's shared page in the session's
    /// shared workspace, everything else in this computer's own.
    pub(crate) fn workspace_for(&self, path: &Path) -> &adoc_lang::WorkspaceState {
        if let Some(Side::Guest(guest)) = self.collab_session.as_ref().map(|session| &session.side)
        {
            if guest
                .virtual_root
                .as_ref()
                .is_some_and(|root| path.starts_with(root))
            {
                return &guest.shared;
            }
        }
        &self.workspace
    }

    /// Whether this engine hosts a session.
    pub(crate) fn hosting(&self) -> bool {
        matches!(
            self.collab_session.as_ref().map(|session| &session.side),
            Some(Side::Host(_))
        )
    }

    /// Asks the host for the shared file at `path`, a guest's virtual path, to be shown when it
    /// arrives.
    pub(crate) fn open_shared_path(&mut self, path: &Path) -> Result<(), EngineError> {
        let Some(Side::Guest(guest)) = self.collab_session.as_ref().map(|session| &session.side)
        else {
            return Err(refused("that shared page is from a session that has ended"));
        };
        let file = guest
            .manifest
            .iter()
            .find(|entry| guest.path_of(&entry.path).as_deref() == Some(path))
            .map(|entry| entry.file)
            .ok_or_else(|| refused("the host does not share that page"))?;
        self.open_session_file(file)?;
        if let Some(Side::Guest(guest)) = self
            .collab_session
            .as_mut()
            .map(|session| &mut session.side)
        {
            if let Some(state) = guest.files.get_mut(&file) {
                state.show = true;
            }
        }
        Ok(())
    }

    /// The Antora component versions the offered files belong to, inside the shared folder,
    /// by name and version only.
    fn shared_components(
        &self,
        root: &Path,
        offered: &BTreeMap<u64, Offered>,
    ) -> Vec<SharedComponent> {
        let mut components = Vec::new();
        for descriptor in self.workspace.antora.components() {
            let Ok(relative) = descriptor.root.strip_prefix(root) else {
                continue;
            };
            if !offered
                .values()
                .any(|offered| offered.path.starts_with(&descriptor.root))
            {
                continue;
            }
            let dir: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            let component = SharedComponent {
                dir: dir.join("/"),
                name: descriptor.name.clone(),
                version: descriptor.version.clone(),
            };
            if components.contains(&component) {
                continue;
            }
            // The message is sent whole, so it is checked whole: a component it cannot carry
            // (a name that is not a word, or one too many for its frame) is left out, and its
            // pages resolve as plain files.
            components.push(component);
            let checked = Message::Components {
                components: components.clone(),
            };
            if checked.encode().is_err() {
                components.pop();
            }
        }
        components
    }
}
