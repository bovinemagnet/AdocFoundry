//! The session protocol, version 1 (design §8): the messages host and guests exchange, their
//! frames, and the limits each is held to. No network: the transport reads and writes frames,
//! and the engine reads and writes messages.
//!
//! A frame is a 4-byte big-endian length (at most [`MAX_FRAME`]), then a 1-byte kind, a 2-byte
//! big-endian length of the control part and the control part as JSON, then any binary payload
//! (an update, a state vector or a selection). Everything read is untrusted: [`decode`] refuses an
//! unknown kind, malformed or unexpected JSON, a field outside its limit and a payload where none
//! belongs, and the transport closes the connection on any of them.

use std::{fmt, time::Duration};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{EncodedUpdate, RelativeSelection, StateVectorBytes, MAX_ANCHOR, MAX_UPDATE};

/// The one protocol version this build speaks.
pub const PROTOCOL_VERSION: u16 = 2;
/// The largest control part: its length is two bytes.
pub const MAX_CONTROL: usize = u16::MAX as usize;
/// The largest frame after its length prefix: room for [`MAX_UPDATE`] and a control part.
pub const MAX_FRAME: usize = MAX_UPDATE + (64 << 10);
/// The most files one guest may have open.
pub const MAX_OPEN_FILES: usize = 64;
/// The most participants in a session, the host included. Version 1 admits one guest.
pub const MAX_PARTICIPANTS: usize = 8;
/// The most frames waiting to be sent to one peer.
pub const MAX_QUEUED_FRAMES: usize = 1024;
/// The most bytes waiting to be sent to one peer.
pub const MAX_QUEUED_BYTES: usize = 16 << 20;
/// The most component versions a session's files may belong to.
pub const MAX_COMPONENTS: usize = 256;
/// The most files a manifest lists.
pub const MAX_MANIFEST_FILES: usize = 100_000;
/// The most files one `Manifest` message lists, so its control part stays within
/// [`MAX_CONTROL`] whatever the paths, each escaped at worst to twice its length: a long
/// manifest is sent in parts.
pub const MANIFEST_PART: usize = 24;
/// The longest display path, in bytes.
pub const MAX_PATH: usize = 1024;
/// The longest display name, in bytes.
pub const MAX_NAME: usize = 64;
/// The longest free text: a reason, a message or the application's version.
pub const MAX_TEXT: usize = 512;
/// The longest text of a comment or reply, in bytes, as a page's metadata file holds it.
pub const MAX_COMMENT_TEXT: usize = 8 * 1024;
/// The most replies a comment holds.
pub const MAX_COMMENT_REPLIES: usize = 100;
/// The most comments one shared file holds.
pub const MAX_COMMENTS: usize = 500;
/// The longest quoted text a comment carries, in bytes.
pub const MAX_COMMENT_QUOTE: usize = 4 * 1024;
/// The longest e-mail address, in bytes.
pub const MAX_EMAIL: usize = 254;
/// How long an invitation stays valid.
pub const INVITE_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long a connection may take from accepting it to its `Authenticate`.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a quiet link sends a heartbeat.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// How long a link may stay silent before it is down.
pub const LINK_TIMEOUT: Duration = Duration::from_secs(45);

/// A random 256-bit value: an invitation's secret or a resume token. Its `Debug` hides it, so
/// it is never logged by accident.
#[derive(Clone, Eq, PartialEq)]
pub struct Secret(pub [u8; 32]);

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(..)")
    }
}

impl Secret {
    /// The secret as 64 lowercase hexadecimal digits.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    /// The secret written as 64 hexadecimal digits.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        unhex::<32>(text).map(Self)
    }
}

impl Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text).ok_or_else(|| serde::de::Error::custom("not a secret"))
    }
}

/// A session's identity: 128 random bits, per Share.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(pub [u8; 16]);

impl Serialize for SessionId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        unhex::<16>(&text)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("not a session id"))
    }
}

/// A participant in a session. The host is 0; the host numbers guests from 1 and never reuses
/// a number in a session.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PeerId(pub u32);

impl PeerId {
    pub const HOST: Self = Self(0);
}

/// What a participant may do.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The host.
    Owner,
    /// A guest who may edit and ask for saves.
    Editor,
    /// A guest who may only read. Every guest joins as one.
    Viewer,
}

/// What a shared file is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Page,
    Partial,
}

/// One file a manifest offers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    /// The host's opaque number for the file in this session.
    pub file: u64,
    /// The path from the shared root, with `/`; for showing only, never for finding a file.
    pub path: String,
    pub kind: FileKind,
    /// The text's length in bytes when the manifest was made.
    pub size: u64,
}

/// A participant as the host tells guests of them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub peer: PeerId,
    pub name: String,
    pub role: Role,
    /// Whether their link is up.
    pub connected: bool,
    /// The shared file they are in, when the page they are on is one; `None` elsewhere.
    pub file: Option<u64>,
    /// Whom they follow, if anyone.
    pub following: Option<PeerId>,
}

/// An Antora component version the shared files belong to, as much as a guest needs to resolve
/// resource ids among them: never its attributes, navigation or anything else in `antora.yml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedComponent {
    /// The folder holding its `antora.yml`, from the shared root with `/`; empty for the root.
    pub dir: String,
    pub name: String,
    /// `None` for a versionless component.
    pub version: Option<String>,
}

/// How a credential proves who is connecting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Credential {
    /// An invitation's secret, used once.
    Invitation(Secret),
    /// The resume token of an earlier connection in the same session.
    Resume(Secret),
}

/// What became of a guest's save request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SaveOutcome {
    Saved {
        revision: String,
    },
    Queued,
    Conflict,
    PermissionDenied,
    /// The request named an epoch the file has moved on from.
    Stale,
    Failed {
        reason: String,
    },
}

/// Why a message was refused or a connection closed. Stable: front ends show them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No protocol version in common.
    Incompatible,
    /// The credential was wrong, used or expired, or the host chose Deny.
    Denied,
    /// A message out of place, malformed or over a limit.
    Protocol,
    /// The peer did not keep up with what was sent to it.
    Slow,
    /// The peer's role does not allow the request.
    Unauthorised,
    /// The file's epoch has moved on: open it again.
    Stale,
    /// The host stopped sharing, or removed the peer.
    Closed,
    /// The link was silent for too long, or the handshake took too long.
    TimedOut,
}

/// A protocol message (design §8).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    /// The versions the sender speaks; the host's answer names the one chosen.
    Hello {
        min: u16,
        max: u16,
        app: String,
    },
    /// A guest's credential, display name and, if it gave one, the address its edits are
    /// credited by in the host's commits. The address reaches the host only.
    Authenticate {
        credential: Credential,
        name: String,
        email: Option<String>,
    },
    /// The host's welcome after Admit: who the guest is, its role, the session, and the token
    /// that resumes the connection if it drops.
    Join {
        peer: PeerId,
        role: Role,
        session: SessionId,
        resume: Secret,
    },
    /// Part of the files offered; `more` until the last part.
    Manifest {
        files: Vec<ManifestFile>,
        more: bool,
    },
    OpenFile {
        file: u64,
    },
    CloseFile {
        file: u64,
    },
    /// A file's whole state for its epoch, and the client id the receiver writes with in it.
    Snapshot {
        file: u64,
        epoch: u32,
        client: u64,
        update: EncodedUpdate,
    },
    /// A change to a file; `seq` grows by one per sender, file and epoch on a connection.
    /// From a guest, `base` is the `client` of the snapshot the change was made on, so the host
    /// can tell a change sent before its latest snapshot from one after it; from the host it is
    /// 0.
    Update {
        file: u64,
        epoch: u32,
        seq: u64,
        base: u64,
        update: EncodedUpdate,
    },
    /// The host applied the guest's updates to `file` up to `seq`.
    UpdateAck {
        file: u64,
        epoch: u32,
        seq: u64,
    },
    /// A participant's selection in a file. The host forwards a guest's with `peer` set to the
    /// guest, whatever the guest put there.
    Awareness {
        file: u64,
        epoch: u32,
        peer: PeerId,
        selection: RelativeSelection,
    },
    SaveRequest {
        file: u64,
        epoch: u32,
        request: u64,
    },
    SaveResult {
        request: u64,
        outcome: SaveOutcome,
    },
    /// The Antora component versions the manifest's files belong to; sent after the manifest.
    Components {
        components: Vec<SharedComponent>,
    },
    /// Where a guest is: the shared file of the page it is on (`None` when on a page of its own
    /// or none), and whom it follows; sent when either changes and after resuming. The host
    /// tells the others in its `Roster`.
    Focus {
        file: Option<u64>,
        following: Option<PeerId>,
    },
    /// A guest asks the host to change the comments of `file` (#762): the host decides, and
    /// answers with the comments as they then stand, or `CommentRefused` naming `request`. An
    /// `Add` carries the range commented on as `selection`; nothing else does.
    Comment {
        file: u64,
        epoch: u32,
        request: u64,
        change: CommentChange,
        selection: Option<RelativeSelection>,
    },
    /// One comment of `file` and its replies, as the host holds it, and where it is in the
    /// shared text (`None` while detached). Sent after the file's snapshot, and again whenever
    /// it changes.
    CommentThread {
        file: u64,
        epoch: u32,
        thread: Box<SharedThread>,
        selection: Option<RelativeSelection>,
    },
    /// A comment of `file` was deleted.
    CommentRemoved {
        file: u64,
        epoch: u32,
        comment: u64,
    },
    /// The host refused a guest's `Comment` request, and why.
    CommentRefused {
        file: u64,
        request: u64,
        reason: String,
    },
    /// Who is in the session, the host first, and the name of the folder shared; sent after
    /// `Join` and whenever it changes. Names are for showing only.
    Roster {
        workspace: String,
        members: Vec<Member>,
    },
    /// The host's file as it stands: the revision on disk if the shared text is that text
    /// (`None` while unsaved changes are in it), and whether a change made outside the session
    /// waits for the host to resolve it. A revision is the SHA-256 of the saved text, in
    /// hexadecimal, so a guest can tell whether its own text is the saved one.
    FileState {
        file: u64,
        epoch: u32,
        saved: Option<String>,
        conflict: bool,
    },
    RoleChanged {
        peer: PeerId,
        role: Role,
    },
    /// The sender's state vector for a file: answered with what it lacks.
    Resync {
        file: u64,
        epoch: u32,
        state: StateVectorBytes,
    },
    Leave {
        reason: String,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    Heartbeat,
}

/// Why bytes are not a frame of this protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// A frame, control part or field over its limit: the size given.
    TooLarge(usize),
    /// Too short for what it says it holds.
    Truncated,
    UnknownKind(u8),
    /// The control part is not the JSON its kind expects.
    Control(String),
    /// A field outside what the protocol allows.
    Invalid(&'static str),
    /// A payload where its kind has none, or none where it needs one.
    Payload,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(size) => write!(formatter, "{size} bytes is over the limit"),
            Self::Truncated => formatter.write_str("the frame is truncated"),
            Self::UnknownKind(kind) => write!(formatter, "unknown message kind {kind}"),
            Self::Control(reason) => write!(formatter, "malformed control part: {reason}"),
            Self::Invalid(field) => write!(formatter, "{field} is not allowed"),
            Self::Payload => formatter.write_str("the payload does not fit the message"),
        }
    }
}

impl std::error::Error for FrameError {}

/// The length a frame's 4-byte prefix announces, refused before anything is allocated when it
/// is over [`MAX_FRAME`] or too short for a kind and a control length.
pub fn frame_length(prefix: [u8; 4]) -> Result<usize, FrameError> {
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_FRAME {
        return Err(FrameError::TooLarge(length));
    }
    if length < 3 {
        return Err(FrameError::Truncated);
    }
    Ok(length)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloControl {
    min: u16,
    max: u16,
    app: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticateControl {
    credential: Credential,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinControl {
    peer: PeerId,
    role: Role,
    session: SessionId,
    resume: Secret,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestControl {
    files: Vec<ManifestFile>,
    more: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileControl {
    file: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotControl {
    file: u64,
    epoch: u32,
    client: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateControl {
    file: u64,
    epoch: u32,
    seq: u64,
    base: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SequenceControl {
    file: u64,
    epoch: u32,
    seq: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AwarenessControl {
    file: u64,
    epoch: u32,
    peer: PeerId,
    /// The length of the anchor's encoding; the head's follows it in the payload.
    anchor: u16,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveRequestControl {
    file: u64,
    epoch: u32,
    request: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveResultControl {
    request: u64,
    outcome: SaveOutcome,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentsControl {
    components: Vec<SharedComponent>,
}

/// What a guest asks of a file's comments; ids are the comments' and replies' own.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "kind")]
pub enum CommentChange {
    Add {
        text: String,
    },
    Reply {
        comment: u64,
        text: String,
    },
    Resolve {
        comment: u64,
        resolved: bool,
    },
    Edit {
        comment: u64,
        reply: Option<u64>,
        text: String,
    },
    Delete {
        comment: u64,
        reply: Option<u64>,
    },
}

/// A comment as the host sends it: times are seconds since 1970, UTC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedThread {
    pub id: u64,
    pub author: String,
    /// Whether the comment is the receiving guest's own, which the host decides by address;
    /// no address is ever sent.
    pub own: bool,
    pub created: u64,
    pub edited: Option<u64>,
    pub quote: String,
    pub text: String,
    pub resolved: Option<SharedResolution>,
    pub detached: bool,
    pub replies: Vec<SharedReply>,
}

/// Who resolved a comment, and when.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedResolution {
    pub by: String,
    pub at: u64,
}

/// A reply to a comment, as the host sends it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedReply {
    pub id: u64,
    pub author: String,
    /// Whether the reply is the receiving guest's own.
    pub own: bool,
    pub created: u64,
    pub edited: Option<u64>,
    pub text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentControl {
    file: u64,
    epoch: u32,
    request: u64,
    change: CommentChange,
    /// The length of the selection's anchor in the payload, its head following; none without
    /// a selection.
    anchor: Option<u16>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentThreadControl {
    file: u64,
    epoch: u32,
    /// The lengths of the selection's anchor and head at the payload's start, the thread's
    /// JSON following; none while detached.
    anchor: Option<(u16, u16)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentRemovedControl {
    file: u64,
    epoch: u32,
    comment: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentRefusedControl {
    file: u64,
    request: u64,
    reason: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FocusControl {
    file: Option<u64>,
    following: Option<PeerId>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterControl {
    workspace: String,
    members: Vec<Member>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStateControl {
    file: u64,
    epoch: u32,
    saved: Option<String>,
    conflict: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleControl {
    peer: PeerId,
    role: Role,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EpochControl {
    file: u64,
    epoch: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaveControl {
    reason: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorControl {
    code: ErrorCode,
    message: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyControl {}

mod kind {
    pub const HELLO: u8 = 1;
    pub const AUTHENTICATE: u8 = 2;
    pub const JOIN: u8 = 3;
    pub const MANIFEST: u8 = 4;
    pub const OPEN_FILE: u8 = 5;
    pub const CLOSE_FILE: u8 = 6;
    pub const SNAPSHOT: u8 = 7;
    pub const UPDATE: u8 = 8;
    pub const UPDATE_ACK: u8 = 9;
    pub const AWARENESS: u8 = 10;
    pub const SAVE_REQUEST: u8 = 11;
    pub const SAVE_RESULT: u8 = 12;
    pub const ROLE_CHANGED: u8 = 13;
    pub const RESYNC: u8 = 14;
    pub const LEAVE: u8 = 15;
    pub const ERROR: u8 = 16;
    pub const HEARTBEAT: u8 = 17;
    pub const FILE_STATE: u8 = 18;
    pub const ROSTER: u8 = 19;
    pub const COMPONENTS: u8 = 20;
    pub const FOCUS: u8 = 21;
    pub const COMMENT: u8 = 22;
    pub const COMMENT_THREAD: u8 = 23;
    pub const COMMENT_REMOVED: u8 = 24;
    pub const COMMENT_REFUSED: u8 = 25;
}

impl Message {
    /// The message as one frame, its length prefix first. Refuses what [`decode`] would refuse,
    /// so nothing this side sends is closed on at the other.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        self.check()?;
        let (kind, control, payload): (u8, Vec<u8>, Vec<u8>) = match self {
            Self::Hello { min, max, app } => (
                kind::HELLO,
                json(&HelloControl {
                    min: *min,
                    max: *max,
                    app: app.clone(),
                })?,
                Vec::new(),
            ),
            Self::Authenticate {
                credential,
                name,
                email,
            } => (
                kind::AUTHENTICATE,
                json(&AuthenticateControl {
                    credential: credential.clone(),
                    name: name.clone(),
                    email: email.clone(),
                })?,
                Vec::new(),
            ),
            Self::Join {
                peer,
                role,
                session,
                resume,
            } => (
                kind::JOIN,
                json(&JoinControl {
                    peer: *peer,
                    role: *role,
                    session: *session,
                    resume: resume.clone(),
                })?,
                Vec::new(),
            ),
            Self::Manifest { files, more } => (
                kind::MANIFEST,
                json(&ManifestControl {
                    files: files.clone(),
                    more: *more,
                })?,
                Vec::new(),
            ),
            Self::OpenFile { file } => (
                kind::OPEN_FILE,
                json(&FileControl { file: *file })?,
                Vec::new(),
            ),
            Self::CloseFile { file } => (
                kind::CLOSE_FILE,
                json(&FileControl { file: *file })?,
                Vec::new(),
            ),
            Self::Snapshot {
                file,
                epoch,
                client,
                update,
            } => (
                kind::SNAPSHOT,
                json(&SnapshotControl {
                    file: *file,
                    epoch: *epoch,
                    client: *client,
                })?,
                update.0.clone(),
            ),
            Self::Update {
                file,
                epoch,
                seq,
                base,
                update,
            } => (
                kind::UPDATE,
                json(&UpdateControl {
                    file: *file,
                    epoch: *epoch,
                    seq: *seq,
                    base: *base,
                })?,
                update.0.clone(),
            ),
            Self::UpdateAck { file, epoch, seq } => (
                kind::UPDATE_ACK,
                json(&SequenceControl {
                    file: *file,
                    epoch: *epoch,
                    seq: *seq,
                })?,
                Vec::new(),
            ),
            Self::Awareness {
                file,
                epoch,
                peer,
                selection,
            } => {
                let mut payload = selection.anchor.clone();
                payload.extend_from_slice(&selection.head);
                (
                    kind::AWARENESS,
                    json(&AwarenessControl {
                        file: *file,
                        epoch: *epoch,
                        peer: *peer,
                        anchor: selection.anchor.len() as u16,
                    })?,
                    payload,
                )
            }
            Self::SaveRequest {
                file,
                epoch,
                request,
            } => (
                kind::SAVE_REQUEST,
                json(&SaveRequestControl {
                    file: *file,
                    epoch: *epoch,
                    request: *request,
                })?,
                Vec::new(),
            ),
            Self::SaveResult { request, outcome } => (
                kind::SAVE_RESULT,
                json(&SaveResultControl {
                    request: *request,
                    outcome: outcome.clone(),
                })?,
                Vec::new(),
            ),
            Self::Components { components } => (
                kind::COMPONENTS,
                json(&ComponentsControl {
                    components: components.clone(),
                })?,
                Vec::new(),
            ),
            Self::Focus { file, following } => (
                kind::FOCUS,
                json(&FocusControl {
                    file: *file,
                    following: *following,
                })?,
                Vec::new(),
            ),
            Self::Comment {
                file,
                epoch,
                request,
                change,
                selection,
            } => {
                let payload = selection.as_ref().map_or_else(Vec::new, |selection| {
                    [selection.anchor.as_slice(), selection.head.as_slice()].concat()
                });
                (
                    kind::COMMENT,
                    json(&CommentControl {
                        file: *file,
                        epoch: *epoch,
                        request: *request,
                        change: change.clone(),
                        anchor: selection
                            .as_ref()
                            .map(|selection| selection.anchor.len() as u16),
                    })?,
                    payload,
                )
            }
            Self::CommentThread {
                file,
                epoch,
                thread,
                selection,
            } => {
                let mut payload = selection.as_ref().map_or_else(Vec::new, |selection| {
                    [selection.anchor.as_slice(), selection.head.as_slice()].concat()
                });
                payload.extend(json(thread.as_ref())?);
                (
                    kind::COMMENT_THREAD,
                    json(&CommentThreadControl {
                        file: *file,
                        epoch: *epoch,
                        anchor: selection.as_ref().map(|selection| {
                            (selection.anchor.len() as u16, selection.head.len() as u16)
                        }),
                    })?,
                    payload,
                )
            }
            Self::CommentRemoved {
                file,
                epoch,
                comment,
            } => (
                kind::COMMENT_REMOVED,
                json(&CommentRemovedControl {
                    file: *file,
                    epoch: *epoch,
                    comment: *comment,
                })?,
                Vec::new(),
            ),
            Self::CommentRefused {
                file,
                request,
                reason,
            } => (
                kind::COMMENT_REFUSED,
                json(&CommentRefusedControl {
                    file: *file,
                    request: *request,
                    reason: reason.clone(),
                })?,
                Vec::new(),
            ),
            Self::Roster { workspace, members } => (
                kind::ROSTER,
                json(&RosterControl {
                    workspace: workspace.clone(),
                    members: members.clone(),
                })?,
                Vec::new(),
            ),
            Self::FileState {
                file,
                epoch,
                saved,
                conflict,
            } => (
                kind::FILE_STATE,
                json(&FileStateControl {
                    file: *file,
                    epoch: *epoch,
                    saved: saved.clone(),
                    conflict: *conflict,
                })?,
                Vec::new(),
            ),
            Self::RoleChanged { peer, role } => (
                kind::ROLE_CHANGED,
                json(&RoleControl {
                    peer: *peer,
                    role: *role,
                })?,
                Vec::new(),
            ),
            Self::Resync { file, epoch, state } => (
                kind::RESYNC,
                json(&EpochControl {
                    file: *file,
                    epoch: *epoch,
                })?,
                state.0.clone(),
            ),
            Self::Leave { reason } => (
                kind::LEAVE,
                json(&LeaveControl {
                    reason: reason.clone(),
                })?,
                Vec::new(),
            ),
            Self::Error { code, message } => (
                kind::ERROR,
                json(&ErrorControl {
                    code: *code,
                    message: message.clone(),
                })?,
                Vec::new(),
            ),
            Self::Heartbeat => (kind::HEARTBEAT, json(&EmptyControl {})?, Vec::new()),
        };
        if control.len() > MAX_CONTROL {
            return Err(FrameError::TooLarge(control.len()));
        }
        let length = 3 + control.len() + payload.len();
        if length > MAX_FRAME {
            return Err(FrameError::TooLarge(length));
        }
        let mut frame = Vec::with_capacity(4 + length);
        frame.extend_from_slice(&(length as u32).to_be_bytes());
        frame.push(kind);
        frame.extend_from_slice(&(control.len() as u16).to_be_bytes());
        frame.extend_from_slice(&control);
        frame.extend_from_slice(&payload);
        Ok(frame)
    }

    /// Holds every field to its limit.
    fn check(&self) -> Result<(), FrameError> {
        match self {
            Self::Hello { min, max, app } => {
                if min > max || *min == 0 {
                    return Err(FrameError::Invalid("versions"));
                }
                text(app, MAX_TEXT, "app")
            }
            Self::Authenticate { name, email, .. } => {
                display_name(name)?;
                email.as_deref().map_or(Ok(()), commit_email)
            }
            Self::Join { peer, role, .. } => {
                if *peer == PeerId::HOST || *role == Role::Owner {
                    return Err(FrameError::Invalid("peer"));
                }
                Ok(())
            }
            Self::Manifest { files, .. } => {
                if files.len() > MANIFEST_PART {
                    return Err(FrameError::TooLarge(files.len()));
                }
                files.iter().try_for_each(|entry| display_path(&entry.path))
            }
            Self::Snapshot { client, update, .. } => {
                if crate::ReplicaId::new(*client).is_none() {
                    return Err(FrameError::Invalid("client"));
                }
                payload(&update.0, MAX_UPDATE)
            }
            Self::Update { update, .. } => payload(&update.0, MAX_UPDATE),
            Self::Awareness { selection, .. } => {
                if selection.anchor.len() > MAX_ANCHOR || selection.head.len() > MAX_ANCHOR {
                    return Err(FrameError::TooLarge(
                        selection.anchor.len().max(selection.head.len()),
                    ));
                }
                Ok(())
            }
            Self::SaveResult {
                outcome: SaveOutcome::Saved { revision: reason } | SaveOutcome::Failed { reason },
                ..
            } => text(reason, MAX_TEXT, "reason"),
            Self::Components { components } => {
                if components.len() > MAX_COMPONENTS {
                    return Err(FrameError::TooLarge(components.len()));
                }
                components.iter().try_for_each(|component| {
                    if !component.dir.is_empty() {
                        display_path(&component.dir)?;
                    }
                    component_word(&component.name)?;
                    component.version.as_deref().map_or(Ok(()), component_word)
                })
            }
            Self::Comment {
                change, selection, ..
            } => {
                let adding = matches!(change, CommentChange::Add { .. });
                if adding != selection.is_some() {
                    return Err(FrameError::Invalid("selection"));
                }
                if let Some(selection) = selection {
                    anchors(selection)?;
                }
                match change {
                    CommentChange::Add { text }
                    | CommentChange::Reply { text, .. }
                    | CommentChange::Edit { text, .. } => comment_text(text),
                    CommentChange::Resolve { .. } | CommentChange::Delete { .. } => Ok(()),
                }
            }
            Self::CommentThread {
                thread, selection, ..
            } => {
                if let Some(selection) = selection {
                    anchors(selection)?;
                }
                if thread.detached != selection.is_none() {
                    return Err(FrameError::Invalid("selection"));
                }
                check_thread(thread)
            }
            Self::CommentRefused { reason, .. } => text(reason, MAX_TEXT, "reason"),
            Self::Roster { workspace, members } => {
                text(workspace, MAX_TEXT, "workspace")?;
                if members.len() > MAX_PARTICIPANTS {
                    return Err(FrameError::TooLarge(members.len()));
                }
                members
                    .iter()
                    .try_for_each(|member| display_name(&member.name))
            }
            Self::FileState {
                saved: Some(revision),
                ..
            } => {
                if revision.len() == 64
                    && revision
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    Ok(())
                } else {
                    Err(FrameError::Invalid("revision"))
                }
            }
            Self::RoleChanged { role, .. } => {
                if *role == Role::Owner {
                    return Err(FrameError::Invalid("role"));
                }
                Ok(())
            }
            Self::Resync { state, .. } => payload(&state.0, MAX_UPDATE),
            Self::Leave { reason } => text(reason, MAX_TEXT, "reason"),
            Self::Error { message, .. } => text(message, MAX_TEXT, "message"),
            Self::OpenFile { .. }
            | Self::CloseFile { .. }
            | Self::UpdateAck { .. }
            | Self::SaveRequest { .. }
            | Self::SaveResult { .. }
            | Self::FileState { .. }
            | Self::Focus { .. }
            | Self::CommentRemoved { .. }
            | Self::Heartbeat => Ok(()),
        }
    }
}

/// The message in one frame's bytes, after its length prefix ([`frame_length`]).
pub fn decode(frame: &[u8]) -> Result<Message, FrameError> {
    if frame.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(frame.len()));
    }
    let [kind, high, low, rest @ ..] = frame else {
        return Err(FrameError::Truncated);
    };
    let control_len = u16::from_be_bytes([*high, *low]) as usize;
    if rest.len() < control_len {
        return Err(FrameError::Truncated);
    }
    let (control, payload) = rest.split_at(control_len);
    let no_payload = |message: Message| {
        if payload.is_empty() {
            Ok(message)
        } else {
            Err(FrameError::Payload)
        }
    };
    let message = match *kind {
        kind::HELLO => {
            let HelloControl { min, max, app } = read(control)?;
            no_payload(Message::Hello { min, max, app })?
        }
        kind::AUTHENTICATE => {
            let AuthenticateControl {
                credential,
                name,
                email,
            } = read(control)?;
            no_payload(Message::Authenticate {
                credential,
                name,
                email,
            })?
        }
        kind::JOIN => {
            let JoinControl {
                peer,
                role,
                session,
                resume,
            } = read(control)?;
            no_payload(Message::Join {
                peer,
                role,
                session,
                resume,
            })?
        }
        kind::MANIFEST => {
            let ManifestControl { files, more } = read(control)?;
            no_payload(Message::Manifest { files, more })?
        }
        kind::OPEN_FILE => {
            let FileControl { file } = read(control)?;
            no_payload(Message::OpenFile { file })?
        }
        kind::CLOSE_FILE => {
            let FileControl { file } = read(control)?;
            no_payload(Message::CloseFile { file })?
        }
        kind::SNAPSHOT => {
            let SnapshotControl {
                file,
                epoch,
                client,
            } = read(control)?;
            Message::Snapshot {
                file,
                epoch,
                client,
                update: EncodedUpdate(needed(payload)?),
            }
        }
        kind::UPDATE => {
            let UpdateControl {
                file,
                epoch,
                seq,
                base,
            } = read(control)?;
            Message::Update {
                file,
                epoch,
                seq,
                base,
                update: EncodedUpdate(needed(payload)?),
            }
        }
        kind::UPDATE_ACK => {
            let SequenceControl { file, epoch, seq } = read(control)?;
            no_payload(Message::UpdateAck { file, epoch, seq })?
        }
        kind::AWARENESS => {
            let AwarenessControl {
                file,
                epoch,
                peer,
                anchor,
            } = read(control)?;
            let anchor = anchor as usize;
            if anchor > payload.len() {
                return Err(FrameError::Payload);
            }
            let (anchor, head) = payload.split_at(anchor);
            Message::Awareness {
                file,
                epoch,
                peer,
                selection: RelativeSelection {
                    anchor: anchor.to_vec(),
                    head: head.to_vec(),
                },
            }
        }
        kind::SAVE_REQUEST => {
            let SaveRequestControl {
                file,
                epoch,
                request,
            } = read(control)?;
            no_payload(Message::SaveRequest {
                file,
                epoch,
                request,
            })?
        }
        kind::SAVE_RESULT => {
            let SaveResultControl { request, outcome } = read(control)?;
            no_payload(Message::SaveResult { request, outcome })?
        }
        kind::COMPONENTS => {
            let ComponentsControl { components } = read(control)?;
            no_payload(Message::Components { components })?
        }
        kind::FOCUS => {
            let FocusControl { file, following } = read(control)?;
            no_payload(Message::Focus { file, following })?
        }
        kind::COMMENT => {
            let CommentControl {
                file,
                epoch,
                request,
                change,
                anchor,
            } = read(control)?;
            let selection = match anchor {
                None if payload.is_empty() => None,
                None => return Err(FrameError::Payload),
                Some(anchor) => {
                    let anchor = anchor as usize;
                    if anchor > payload.len() {
                        return Err(FrameError::Payload);
                    }
                    let (anchor, head) = payload.split_at(anchor);
                    Some(RelativeSelection {
                        anchor: anchor.to_vec(),
                        head: head.to_vec(),
                    })
                }
            };
            Message::Comment {
                file,
                epoch,
                request,
                change,
                selection,
            }
        }
        kind::COMMENT_THREAD => {
            let CommentThreadControl {
                file,
                epoch,
                anchor,
            } = read(control)?;
            let (selection, rest) = match anchor {
                None => (None, payload),
                Some((anchor, head)) => {
                    let (anchor, head) = (anchor as usize, head as usize);
                    if anchor + head > payload.len() {
                        return Err(FrameError::Payload);
                    }
                    let (anchor, rest) = payload.split_at(anchor);
                    let (head, rest) = rest.split_at(head);
                    (
                        Some(RelativeSelection {
                            anchor: anchor.to_vec(),
                            head: head.to_vec(),
                        }),
                        rest,
                    )
                }
            };
            Message::CommentThread {
                file,
                epoch,
                thread: Box::new(read(rest)?),
                selection,
            }
        }
        kind::COMMENT_REMOVED => {
            let CommentRemovedControl {
                file,
                epoch,
                comment,
            } = read(control)?;
            no_payload(Message::CommentRemoved {
                file,
                epoch,
                comment,
            })?
        }
        kind::COMMENT_REFUSED => {
            let CommentRefusedControl {
                file,
                request,
                reason,
            } = read(control)?;
            no_payload(Message::CommentRefused {
                file,
                request,
                reason,
            })?
        }
        kind::ROSTER => {
            let RosterControl { workspace, members } = read(control)?;
            no_payload(Message::Roster { workspace, members })?
        }
        kind::FILE_STATE => {
            let FileStateControl {
                file,
                epoch,
                saved,
                conflict,
            } = read(control)?;
            no_payload(Message::FileState {
                file,
                epoch,
                saved,
                conflict,
            })?
        }
        kind::ROLE_CHANGED => {
            let RoleControl { peer, role } = read(control)?;
            no_payload(Message::RoleChanged { peer, role })?
        }
        kind::RESYNC => {
            let EpochControl { file, epoch } = read(control)?;
            Message::Resync {
                file,
                epoch,
                state: StateVectorBytes(needed(payload)?),
            }
        }
        kind::LEAVE => {
            let LeaveControl { reason } = read(control)?;
            no_payload(Message::Leave { reason })?
        }
        kind::ERROR => {
            let ErrorControl { code, message } = read(control)?;
            no_payload(Message::Error { code, message })?
        }
        kind::HEARTBEAT => {
            let EmptyControl {} = read(control)?;
            no_payload(Message::Heartbeat)?
        }
        other => return Err(FrameError::UnknownKind(other)),
    };
    message.check()?;
    Ok(message)
}

fn json<T: Serialize>(control: &T) -> Result<Vec<u8>, FrameError> {
    serde_json::to_vec(control).map_err(|error| FrameError::Control(error.to_string()))
}

fn read<T: DeserializeOwned>(control: &[u8]) -> Result<T, FrameError> {
    serde_json::from_slice(control).map_err(|error| FrameError::Control(error.to_string()))
}

fn needed(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.is_empty() {
        return Err(FrameError::Payload);
    }
    Ok(payload.to_vec())
}

fn payload(bytes: &[u8], limit: usize) -> Result<(), FrameError> {
    if bytes.is_empty() {
        return Err(FrameError::Payload);
    }
    if bytes.len() > limit {
        return Err(FrameError::TooLarge(bytes.len()));
    }
    Ok(())
}

fn text(value: &str, limit: usize, field: &'static str) -> Result<(), FrameError> {
    if value.len() > limit {
        return Err(FrameError::TooLarge(value.len()));
    }
    if value.chars().any(|c| c.is_control() && c != '\n') {
        return Err(FrameError::Invalid(field));
    }
    Ok(())
}

/// An Antora component's name or version as `antora.yml` may hold it: a word of letters,
/// digits, `.`, `_` and `-`, at most [`MAX_NAME`] bytes, so a guest can write it into a
/// descriptor of its own without quoting.
fn component_word(word: &str) -> Result<(), FrameError> {
    let ok = !word.is_empty()
        && word.len() <= MAX_NAME
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(FrameError::Invalid("component"))
    }
}

/// A display name: one line of printable text, not empty, at most [`MAX_NAME`] bytes.
fn anchors(selection: &RelativeSelection) -> Result<(), FrameError> {
    if selection.anchor.len() > MAX_ANCHOR || selection.head.len() > MAX_ANCHOR {
        return Err(FrameError::TooLarge(
            selection.anchor.len().max(selection.head.len()),
        ));
    }
    Ok(())
}

/// A comment's or reply's text: some, within [`MAX_COMMENT_TEXT`], with no control character
/// but line breaks and tabs.
fn comment_text(value: &str) -> Result<(), FrameError> {
    if value.len() > MAX_COMMENT_TEXT {
        return Err(FrameError::TooLarge(value.len()));
    }
    if value.trim().is_empty()
        || value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(FrameError::Invalid("text"));
    }
    Ok(())
}

/// An address a commit's `Co-authored-by:` trailer can carry: at most [`MAX_EMAIL`] bytes,
/// exactly one `@` with something on each side, and no white space, control character, `<`,
/// `>`, `,`, `"` or `\`.
pub fn commit_email(email: &str) -> Result<(), FrameError> {
    let refused =
        |c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | '"' | '\\');
    match email.split_once('@') {
        Some((local, domain))
            if email.len() <= MAX_EMAIL
                && !local.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && !email.chars().any(refused) =>
        {
            Ok(())
        }
        _ => Err(FrameError::Invalid("email")),
    }
}

fn check_thread(thread: &SharedThread) -> Result<(), FrameError> {
    display_name(&thread.author)?;
    comment_text(&thread.text)?;
    if thread.quote.len() > MAX_COMMENT_QUOTE {
        return Err(FrameError::TooLarge(thread.quote.len()));
    }
    if let Some(resolution) = &thread.resolved {
        display_name(&resolution.by)?;
    }
    if thread.replies.len() > MAX_COMMENT_REPLIES {
        return Err(FrameError::TooLarge(thread.replies.len()));
    }
    thread.replies.iter().try_for_each(|reply| {
        display_name(&reply.author)?;
        comment_text(&reply.text)
    })
}

fn display_name(name: &str) -> Result<(), FrameError> {
    if name.trim().is_empty() || name.len() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(FrameError::Invalid("name"));
    }
    Ok(())
}

/// A display path: relative, `/`-separated, no empty, `.` or `..` part, no backslash, colon or
/// control character, no part starting with a dot. A guest never finds a file by it; it is held
/// to this so it can only be shown as what it says.
pub fn display_path(path: &str) -> Result<(), FrameError> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(FrameError::Invalid("path"));
    }
    let bad = path
        .chars()
        .any(|c| c.is_control() || c == '\\' || c == ':');
    if bad
        || path
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.'))
    {
        return Err(FrameError::Invalid("path"));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != N * 2 {
        return None;
    }
    let value = |digit: u8| match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    };
    let mut bytes = [0; N];
    for (byte, pair) in bytes.iter_mut().zip(digits.as_chunks::<2>().0) {
        *byte = (value(pair[0])? << 4) | value(pair[1])?;
    }
    Some(bytes)
}
