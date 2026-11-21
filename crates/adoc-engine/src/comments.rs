//! Comments on a page's text (#762; `docs/prd/PRD-AdocFoundry_Comments.md`), kept in the page's
//! metadata file beside it ([`crate::page_meta`]).
//!
//! Each open AsciiDoc page holds its comments as byte ranges in its current text, mapped through
//! every edit `edit_document` applies: typing, undo and redo, a participant's update and a change
//! on disk applied as an edit. Text typed at a range's edges falls outside it, text typed inside
//! grows it, and deleting part of it shrinks it; a comment whose range empties is detached,
//! keeping the text it was on, and is searched for again near where it was once typing pauses,
//! so undoing the deletion brings it back.
//!
//! The file changes only when comments do: a comment added, replied to, resolved, edited,
//! deleted or detached, or one whose own text was edited, is written with the offsets, revision
//! and context of the page's text then; the others keep what they had, and are found again by
//! their quote when the page has changed since. So a page edited on two branches, with no
//! comment touched, never conflicts on its comments file. Comment changes are not in the page's
//! undo history and never mark it unsaved.
//!
//! Writes go through the engine's writer after [`COMMENT_SETTLE`], each checking first that the
//! file on disk is the one last read, by its SHA-256. One that changed (a `git pull`) is read
//! again and the changes made here applied on top, matched by id, so a comment made here is
//! never lost. A file this version cannot use is never written.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use adoc_core::SourceRange;

use crate::{
    commands::{ids, register, spec},
    page_meta::{
        self, context, cut, digest, revision_of, sidecar_path, MetaProblem, PageMeta, Resolution,
        StoredComment, StoredReply, MAX_AUTHOR, MAX_EMAIL, MAX_FILE, MAX_TEXT,
    },
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, EngineEvent, TextEdit,
};

/// How long after the last change to a page's comments they are written.
pub const COMMENT_SETTLE: Duration = Duration::from_millis(500);
/// How long after a failed write it is tried again.
const RETRY_FAILED_WRITE: Duration = Duration::from_secs(5);
/// The most occurrences of a quote weighed when finding a comment again.
const MAX_OCCURRENCES: usize = 1_000;

/// A comment's id, unique within its page's metadata file.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommentId(pub u64);

impl std::fmt::Display for CommentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

/// Who writes a comment: a name, and an e-mail address when they gave one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Author {
    pub name: String,
    pub email: Option<String>,
}

/// What the user may do to a comment or reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Allowed {
    pub resolve: bool,
    pub edit: bool,
    pub delete: bool,
}

impl Allowed {
    const ALL: Self = Self {
        resolve: true,
        edit: true,
        delete: true,
    };
}

/// A comment on a page, its range in the page's current text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comment {
    pub id: CommentId,
    pub author: Author,
    pub created: SystemTime,
    pub edited: Option<SystemTime>,
    /// Where it is in the current text; empty, at its last place, while detached.
    pub range: SourceRange,
    /// The text it is on.
    pub quote: String,
    pub text: String,
    pub resolved: Option<Resolution>,
    /// Its text is gone: [`Comment::quote`] is what it was on.
    pub detached: bool,
    pub replies: Vec<Reply>,
    pub can: Allowed,
}

/// A reply to a comment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reply {
    pub id: CommentId,
    pub author: Author,
    pub created: SystemTime,
    pub edited: Option<SystemTime>,
    pub text: String,
    pub can: Allowed,
}

/// Whether a page's comments can be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommentsState {
    Usable,
    /// Nothing can be shown or written: the reason, for the user.
    Unusable(String),
    /// Shown, but never written: a newer version's file.
    ReadOnly(String),
}

/// A change to a page's comments: what `comments.*` commands take.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommentRequest {
    Add {
        id: DocumentId,
        range: SourceRange,
        text: String,
    },
    Reply {
        id: DocumentId,
        comment: CommentId,
        text: String,
    },
    Resolve {
        id: DocumentId,
        comment: CommentId,
        resolved: bool,
    },
    /// The comment's own text, or one of its replies' with `reply`.
    Edit {
        id: DocumentId,
        comment: CommentId,
        reply: Option<CommentId>,
        text: String,
    },
    Delete {
        id: DocumentId,
        comment: CommentId,
        reply: Option<CommentId>,
    },
    /// From `offset`, for `comments.next` and `comments.previous`.
    Step { id: DocumentId, offset: usize },
}

/// A change to a page's comments, whoever asks for it: the user here, or a guest through the
/// host. Ids are the comments' and replies' own.
#[derive(Clone, Debug)]
pub(crate) enum Change {
    Add {
        range: SourceRange,
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

/// Who changes comments, and as what: alone, every change is theirs to make; in a session
/// (design §6) anyone adds and replies, the host, Editors and a comment's author resolve it, its
/// author edits it, and its author or the host deletes it.
#[derive(Clone, Debug)]
pub(crate) struct Actor {
    pub(crate) author: Author,
    pub(crate) kind: ActorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActorKind {
    Alone,
    Host,
    Editor,
    Viewer,
}

impl Actor {
    /// Whether a comment or reply by `name` (and `email`) is the actor's own: the same e-mail
    /// when both have one, else the same name. A courtesy, not a security boundary: the host
    /// admits each participant by name, and the file is anyone's with the repository.
    fn owns(&self, name: &str, email: Option<&str>) -> bool {
        match (self.author.email.as_deref(), email) {
            (Some(mine), Some(theirs)) => mine.eq_ignore_ascii_case(theirs),
            _ => self.author.name == name,
        }
    }

    /// What the actor may do to a comment or reply by `name` (and `email`).
    pub(crate) fn allowed(&self, name: &str, email: Option<&str>) -> Allowed {
        let own = self.owns(name, email);
        match self.kind {
            ActorKind::Alone => Allowed::ALL,
            ActorKind::Host => Allowed {
                resolve: true,
                edit: own,
                delete: true,
            },
            ActorKind::Editor => Allowed {
                resolve: true,
                edit: own,
                delete: own,
            },
            ActorKind::Viewer => Allowed {
                resolve: own,
                edit: own,
                delete: own,
            },
        }
    }
}

/// A comment held by an open page.
#[derive(Clone, Debug)]
struct Live {
    /// As it was last read or written, apart from what was changed here since.
    stored: StoredComment,
    /// Where it is in the current text.
    range: SourceRange,
}

/// One open page's comments.
#[derive(Debug)]
pub(crate) struct PageComments {
    /// The file as last read or written, for what this version does not know.
    meta: PageMeta,
    comments: Vec<Live>,
    state: CommentsState,
    /// Where they are kept; none for a page with no file, or one comments are not kept for.
    sidecar: Option<PathBuf>,
    /// The SHA-256 of the file as last read or written; none when there was no file.
    on_disk: Option<[u8; 32]>,
    /// The comments and replies added or changed here since the file was last read or written,
    /// by id, and those deleted here.
    changed: BTreeSet<u64>,
    deleted: BTreeSet<u64>,
    write_at: Option<Instant>,
    /// The write on the writer: its ticket, the text sent and what it carried of `changed` and
    /// `deleted`.
    writing: Option<Writing>,
    /// A write failed: why, for the notice, until one succeeds.
    failed: Option<String>,
    search_detached_at: Option<Instant>,
    /// Counts every change to the comments or their places, for whoever tells others of them.
    generation: u64,
}

#[derive(Debug)]
struct Writing {
    ticket: u64,
    text: Option<String>,
    changed: BTreeSet<u64>,
    deleted: BTreeSet<u64>,
}

impl PageComments {
    fn empty(sidecar: Option<PathBuf>) -> Self {
        Self {
            meta: PageMeta::default(),
            comments: Vec::new(),
            state: CommentsState::Usable,
            sidecar,
            on_disk: None,
            changed: BTreeSet::new(),
            deleted: BTreeSet::new(),
            write_at: None,
            writing: None,
            failed: None,
            search_detached_at: None,
            generation: 0,
        }
    }

    fn pending(&self) -> bool {
        !self.changed.is_empty() || !self.deleted.is_empty()
    }

    fn live(&self, comment: CommentId) -> Result<usize, EngineError> {
        self.comments
            .iter()
            .position(|live| live.stored.id == comment.0)
            .ok_or_else(|| EngineError::NotFound(format!("comment {comment}")))
    }
}

/// `range` mapped through `edits`, sorted and in the coordinates of the text before them: text
/// inserted at either edge falls outside, text inserted inside grows it, and text deleted from
/// it shrinks it, to empty when all of it goes.
#[must_use]
pub fn map_range(range: SourceRange, edits: &[TextEdit]) -> SourceRange {
    let start = map_point(range.start, edits, true);
    let end = map_point(range.end, edits, false).max(start);
    SourceRange::new(start, end)
}

/// Where `point` goes through `edits`. A start (`start`) stays before text inserted where it is
/// only when the edit replaces text after it; an end takes in a replacement that ends at it.
fn map_point(point: usize, edits: &[TextEdit], start: bool) -> usize {
    let mut shift: isize = 0;
    for edit in edits {
        let SourceRange {
            start: from,
            end: to,
        } = edit.range;
        let inserted = edit.replacement.len();
        if from == to && from == point {
            // Typed at the point: outside the range on either side.
            if start {
                shift += inserted as isize;
            }
            break;
        }
        if to <= point {
            shift += inserted as isize - (to - from) as isize;
        } else if from >= point {
            break;
        } else {
            // The point is inside text replaced.
            let at = from.saturating_add_signed(shift);
            return if start { at } else { at + inserted };
        }
    }
    point.saturating_add_signed(shift)
}

/// Whether `edits` change the text of `range`: anything replaced or deleted in it, or typed
/// inside it.
fn changes_inside(range: SourceRange, edits: &[TextEdit]) -> bool {
    edits.iter().any(|edit| {
        let SourceRange { start, end } = edit.range;
        if start == end {
            range.start < start && start < range.end
        } else {
            start < range.end && end > range.start
        }
    })
}

/// The range of the occurrence of `quote` in `text` that best matches the text `before` and
/// `after` it, the nearer to `near` among equals; none when `quote` is empty or absent.
#[must_use]
pub fn find_quote(
    text: &str,
    quote: &str,
    before: &str,
    after: &str,
    near: usize,
) -> Option<SourceRange> {
    if quote.is_empty() {
        return None;
    }
    text.match_indices(quote)
        .take(MAX_OCCURRENCES)
        .map(|(start, _)| {
            let preceding = &text[..start];
            let following = &text[start + quote.len()..];
            let score = common_suffix(before, preceding) + common_prefix(after, following);
            (score, start.abs_diff(near), start)
        })
        .max_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)))
        .map(|(_, _, start)| SourceRange::new(start, start + quote.len()))
}

fn common_prefix(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .take_while(|(left, right)| left == right)
        .count()
}

fn common_suffix(left: &str, right: &str) -> usize {
    left.bytes()
        .rev()
        .zip(right.bytes().rev())
        .take_while(|(left, right)| left == right)
        .count()
}

/// Where a stored comment is in `text`: at its offsets when `text` is the one they refer to,
/// else where its quote is found, else nowhere (detached, at its old start).
fn anchor(stored: &StoredComment, text: &str, revision: &str) -> (SourceRange, bool) {
    let at_offsets = stored.revision == revision
        && stored.end <= text.len()
        && text.is_char_boundary(stored.start)
        && text.is_char_boundary(stored.end);
    if at_offsets && !stored.detached {
        return (SourceRange::new(stored.start, stored.end), false);
    }
    match find_quote(
        text,
        &stored.quote,
        &stored.before,
        &stored.after,
        stored.start,
    ) {
        Some(range) => (range, false),
        None => {
            let place = stored.start.min(text.len());
            let place = (0..=place)
                .rev()
                .find(|offset| text.is_char_boundary(*offset))
                .unwrap_or(0);
            (SourceRange::new(place, place), true)
        }
    }
}

fn new_id(taken: &BTreeSet<u64>) -> u64 {
    loop {
        let mut bytes = [0; 8];
        if getrandom::fill(&mut bytes).is_err() {
            // No random source: the time, then counting up past what is taken.
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(1, |since| since.as_nanos() as u64);
            bytes = now.to_le_bytes();
        }
        let id = u64::from_le_bytes(bytes);
        if id != 0 && !taken.contains(&id) {
            return id;
        }
    }
}

/// Reads at most [`MAX_FILE`] and a byte of `path`: none when there is no file.
fn read_sidecar(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(metadata) if !metadata.is_file() => {
            return Err(std::io::Error::other("it is not a file"));
        }
        Ok(_) => {}
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

fn checked_text(text: &str) -> Result<String, EngineError> {
    let text = text.trim_end().to_owned();
    if text.trim().is_empty() {
        return Err(EngineError::Refused("a comment needs some text".to_owned()));
    }
    if text.len() > MAX_TEXT {
        return Err(EngineError::Refused(MetaProblem::TooLong.to_string()));
    }
    Ok(text)
}

/// The `comments.*` commands, each a function of its own since commands are plain functions.
macro_rules! comment_commands {
    ($($run:ident: $id:expr, $title:literal, $binding:expr;)*) => {
        $(
            fn $run(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
                let CommandArgs::Comment(request) = args else {
                    return Err(EngineError::InvalidArguments {
                        command: $id,
                        expected: "a comment request",
                    });
                };
                engine.comment_request($id, request)
            }
        )*

        pub(crate) fn register_comments(registry: &mut CommandRegistry) {
            $(
                register(
                    registry,
                    spec($id, $title, CommandCategory::Edit, $binding, CommandContext::ActiveDocument),
                    $run,
                );
            )*
        }
    };
}

comment_commands! {
    add_comment: ids::COMMENTS_ADD, "Add Comment", Some("secondary-alt-m");
    reply_to_comment: ids::COMMENTS_REPLY, "Reply to Comment", None;
    resolve_comment: ids::COMMENTS_RESOLVE, "Resolve Comment", None;
    reopen_comment: ids::COMMENTS_REOPEN, "Reopen Comment", None;
    edit_comment: ids::COMMENTS_EDIT, "Edit Comment", None;
    delete_comment: ids::COMMENTS_DELETE, "Delete Comment", None;
    next_comment: ids::COMMENTS_NEXT, "Next Comment", None;
    previous_comment: ids::COMMENTS_PREVIOUS, "Previous Comment", None;
}

impl Engine {
    /// Who the comments written here are by: `[comments] name` and `email`, else the global Git
    /// identity, else the operating system's user name.
    #[must_use]
    pub fn comment_author(&self) -> Author {
        let settings = &self.settings.comments;
        let (name, email) = match &settings.name {
            Some(name) => (Some(name.clone()), settings.email.clone()),
            None => {
                let git = self.git_identity();
                let email = settings
                    .email
                    .clone()
                    .or_else(|| git.as_ref().and_then(|(_, email)| email.clone()));
                (git.map(|(name, _)| name), email)
            }
        };
        let name = name
            .or_else(|| {
                ["USER", "USERNAME", "LOGNAME"]
                    .iter()
                    .find_map(|key| std::env::var(key).ok())
            })
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Unknown".to_owned());
        Author {
            name: cut(&name, MAX_AUTHOR).to_owned(),
            email: email.map(|email| cut(email.trim(), MAX_EMAIL).to_owned()),
        }
    }

    #[cfg(feature = "git")]
    fn git_identity(&self) -> Option<(String, Option<String>)> {
        self.comment_identity
            .get_or_init(adoc_git::global_identity)
            .clone()
    }

    #[cfg(not(feature = "git"))]
    fn git_identity(&self) -> Option<(String, Option<String>)> {
        None
    }

    /// The comments of document `id`, in the order of their text, detached ones last.
    #[must_use]
    pub fn comments(&self, id: DocumentId) -> Vec<Comment> {
        #[cfg(feature = "collab")]
        if let Some(mut comments) = self.guest_comments(id) {
            comments.sort_by_key(|comment| {
                (
                    comment.detached,
                    comment.range.start,
                    comment.range.end,
                    comment.id,
                )
            });
            return comments;
        }
        let Some(page) = self.page_comments.get(&id) else {
            return Vec::new();
        };
        let actor = self.local_actor(id);
        let mut comments: Vec<Comment> = page
            .comments
            .iter()
            .map(|live| {
                let stored = &live.stored;
                Comment {
                    id: CommentId(stored.id),
                    author: Author {
                        name: stored.author.clone(),
                        email: stored.email.clone(),
                    },
                    created: stored.created,
                    edited: stored.edited,
                    range: live.range,
                    quote: if stored.detached {
                        stored.quote.clone()
                    } else {
                        self.documents
                            .get(&id)
                            .map_or_else(String::new, |document| {
                                document
                                    .text
                                    .slice(live.range.start..live.range.end)
                                    .to_string()
                            })
                    },
                    text: stored.text.clone(),
                    resolved: stored.resolved.clone(),
                    detached: stored.detached,
                    replies: stored
                        .replies
                        .iter()
                        .map(|reply| Reply {
                            id: CommentId(reply.id),
                            author: Author {
                                name: reply.author.clone(),
                                email: reply.email.clone(),
                            },
                            created: reply.created,
                            edited: reply.edited,
                            text: reply.text.clone(),
                            can: actor.allowed(&reply.author, reply.email.as_deref()),
                        })
                        .collect(),
                    can: actor.allowed(&stored.author, stored.email.as_deref()),
                }
            })
            .collect();
        comments.sort_by_key(|comment| {
            (
                comment.detached,
                comment.range.start,
                comment.range.end,
                comment.id,
            )
        });
        comments
    }

    /// The innermost open comment whose range holds `offset` in document `id`.
    #[must_use]
    pub fn comment_at(&self, id: DocumentId, offset: usize) -> Option<CommentId> {
        self.comments(id)
            .into_iter()
            .filter(|comment| {
                !comment.detached
                    && comment.resolved.is_none()
                    && comment.range.start <= offset
                    && offset <= comment.range.end
            })
            .min_by_key(|comment| comment.range.len())
            .map(|comment| comment.id)
    }

    /// Counts every change to document `id`'s comments, for whoever tells others of them.
    pub(crate) fn comments_generation(&self, id: DocumentId) -> u64 {
        self.page_comments
            .get(&id)
            .map_or(0, |page| page.generation)
    }

    /// Whether document `id`'s comments can be used, or why not.
    #[must_use]
    pub fn comments_state(&self, id: DocumentId) -> CommentsState {
        match self.page_comments.get(&id) {
            Some(page) => match (&page.state, &page.failed) {
                (CommentsState::Usable, Some(failed)) => CommentsState::Unusable(failed.clone()),
                (state, _) => state.clone(),
            },
            None => CommentsState::Unusable("This page does not take comments.".to_owned()),
        }
    }

    /// Whether comments are kept for the page at `path`: an AsciiDoc file inside a root, reached
    /// through no link, in no hidden or ignored folder.
    fn keeps_comments(&self, path: &Path) -> bool {
        let Some(root) = self.roots.iter().find(|root| path.starts_with(root)) else {
            return false;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        let ignore = &self.settings.workspace.ignore;
        let mut folders = relative.components().collect::<Vec<_>>();
        folders.pop();
        let hidden = folders.iter().any(|part| {
            part.as_os_str()
                .to_str()
                .is_none_or(|name| name.starts_with('.') || ignore.iter().any(|ig| ig == name))
        });
        !hidden && self.listable(path).is_ok()
    }

    /// Reads the comments of document `id`, just opened.
    pub(crate) fn load_comments(&mut self, id: DocumentId) {
        self.load_comments_checked(id, true);
    }

    /// Reads the comments of a page the host offers, if they are not read already: it was
    /// confined when offered, so it keeps comments wherever it is.
    #[cfg(feature = "collab")]
    pub(crate) fn load_offered_comments(&mut self, id: DocumentId) {
        if !self
            .page_comments
            .get(&id)
            .is_some_and(|page| page.sidecar.is_some())
        {
            self.page_comments.remove(&id);
            self.load_comments_checked(id, false);
        }
    }

    fn load_comments_checked(&mut self, id: DocumentId, confine: bool) {
        let Some(document) = self.documents.get(&id) else {
            return;
        };
        let path = document.path.clone();
        if !document.asciidoc || document.untitled || (confine && !self.keeps_comments(&path)) {
            if document.untitled {
                self.page_comments.insert(id, PageComments::empty(None));
            }
            return;
        }
        // A page closed with a write still to make keeps what it had.
        if let Some(kept) = self.closing_comments.remove(&path) {
            self.page_comments.insert(id, kept);
            return;
        }
        let mut page = PageComments::empty(sidecar_path(&path));
        if let Some(sidecar) = page.sidecar.clone() {
            let text = document.text.to_string();
            match read_sidecar(&sidecar) {
                Ok(None) => {}
                Ok(Some(bytes)) => read_into(&mut page, &bytes, &text),
                Err(error) => {
                    page.state = CommentsState::Unusable(format!(
                        "The comments file cannot be read: {error}"
                    ));
                }
            }
        }
        self.page_comments.insert(id, page);
    }

    /// Moves the ranges of document `id`'s comments through `edits`, just applied.
    pub(crate) fn map_comments(&mut self, id: DocumentId, edits: &[TextEdit]) {
        let now = self.clock.now();
        let Some(page) = self.page_comments.get_mut(&id) else {
            return;
        };
        let mut changed = false;
        let mut detached = false;
        for live in &mut page.comments {
            if live.stored.detached {
                live.range = map_range(live.range, edits);
                live.range.end = live.range.start;
                detached = true;
                continue;
            }
            let inside = changes_inside(live.range, edits);
            live.range = map_range(live.range, edits);
            if live.range.is_empty() {
                live.stored.detached = true;
                detached = true;
            }
            if inside {
                page.changed.insert(live.stored.id);
                changed = true;
            }
        }
        if detached {
            page.search_detached_at = Some(now + crate::INDEX_SETTLE);
        }
        if changed {
            page.write_at = Some(now + COMMENT_SETTLE);
            self.emit(EngineEvent::CommentsChanged(id));
        }
    }

    /// The page at `from` moved to `to`, its metadata file with it: an open page's comments are
    /// kept there from now on.
    pub(crate) fn comments_moved(&mut self, from: &Path, to: &Path) {
        if let Some(id) = self.document_for_path(from) {
            if let Some(page) = self.page_comments.get_mut(&id) {
                if page.sidecar.is_some() {
                    page.sidecar = sidecar_path(to);
                }
            }
        }
        if let Some(mut page) = self.closing_comments.remove(from) {
            page.sidecar = sidecar_path(to);
            self.closing_comments.insert(to.to_path_buf(), page);
        }
    }

    /// Keeps document `id`'s comments, closed, until their last write is made.
    pub(crate) fn forget_comments(&mut self, id: DocumentId, path: &Path) {
        let Some(mut page) = self.page_comments.remove(&id) else {
            return;
        };
        if page.sidecar.is_some() && (page.pending() || page.writing.is_some()) {
            page.write_at = Some(self.clock.now());
            self.closing_comments.insert(path.to_path_buf(), page);
        }
    }

    fn comment_request(
        &mut self,
        command: crate::CommandId,
        request: CommentRequest,
    ) -> Result<CommandOutcome, EngineError> {
        let id = match &request {
            CommentRequest::Add { id, .. }
            | CommentRequest::Reply { id, .. }
            | CommentRequest::Resolve { id, .. }
            | CommentRequest::Edit { id, .. }
            | CommentRequest::Delete { id, .. }
            | CommentRequest::Step { id, .. } => *id,
        };
        let expected = match &request {
            CommentRequest::Add { .. } => ids::COMMENTS_ADD,
            CommentRequest::Reply { .. } => ids::COMMENTS_REPLY,
            CommentRequest::Resolve { resolved: true, .. } => ids::COMMENTS_RESOLVE,
            CommentRequest::Resolve {
                resolved: false, ..
            } => ids::COMMENTS_REOPEN,
            CommentRequest::Edit { .. } => ids::COMMENTS_EDIT,
            CommentRequest::Delete { .. } => ids::COMMENTS_DELETE,
            CommentRequest::Step { .. } if command == ids::COMMENTS_PREVIOUS => {
                ids::COMMENTS_PREVIOUS
            }
            CommentRequest::Step { .. } => ids::COMMENTS_NEXT,
        };
        if expected != command {
            return Err(EngineError::InvalidArguments {
                command,
                expected: "its own comment request",
            });
        }
        if let CommentRequest::Step { offset, .. } = request {
            return self
                .step_comment(id, offset, command == ids::COMMENTS_NEXT)
                .map(CommandOutcome::Selection);
        }
        let change = match request {
            CommentRequest::Add { range, text, .. } => Change::Add { range, text },
            CommentRequest::Reply { comment, text, .. } => Change::Reply {
                comment: comment.0,
                text,
            },
            CommentRequest::Resolve {
                comment, resolved, ..
            } => Change::Resolve {
                comment: comment.0,
                resolved,
            },
            CommentRequest::Edit {
                comment,
                reply,
                text,
                ..
            } => Change::Edit {
                comment: comment.0,
                reply: reply.map(|reply| reply.0),
                text,
            },
            CommentRequest::Delete { comment, reply, .. } => Change::Delete {
                comment: comment.0,
                reply: reply.map(|reply| reply.0),
            },
            CommentRequest::Step { .. } => unreachable!("answered above"),
        };
        #[cfg(feature = "collab")]
        if self.is_shared_guest(id) {
            return self
                .guest_comment(id, change)
                .map(|()| CommandOutcome::Done);
        }
        let actor = self.local_actor(id);
        self.change_comments(id, change, &actor)?;
        Ok(CommandOutcome::Done)
    }

    /// Who the person using this engine is to document `id`'s comments: alone, or the host or
    /// a participant of a session.
    pub(crate) fn local_actor(&self, id: DocumentId) -> Actor {
        #[cfg(feature = "collab")]
        if let Some(actor) = self.session_actor(id) {
            return actor;
        }
        let _ = id;
        Actor {
            author: self.comment_author(),
            kind: ActorKind::Alone,
        }
    }

    /// Makes `change` to document `id`'s comments for `actor`, if `actor` may.
    pub(crate) fn change_comments(
        &mut self,
        id: DocumentId,
        change: Change,
        actor: &Actor,
    ) -> Result<(), EngineError> {
        let now = self.clock.wall();
        let text = self
            .documents
            .get(&id)
            .ok_or(EngineError::UnknownDocument(id))?
            .text
            .to_string();
        let page = self
            .page_comments
            .get_mut(&id)
            .ok_or_else(|| EngineError::Refused("This page does not take comments.".to_owned()))?;
        match &page.state {
            CommentsState::Usable => {}
            CommentsState::Unusable(reason) | CommentsState::ReadOnly(reason) => {
                return Err(EngineError::Refused(reason.clone()));
            }
        }
        let taken: BTreeSet<u64> = page
            .comments
            .iter()
            .flat_map(|live| {
                std::iter::once(live.stored.id).chain(live.stored.replies.iter().map(|r| r.id))
            })
            .collect();
        let author = actor.author.clone();
        let not_allowed = |what: &str| EngineError::Refused(format!("You cannot {what}."));
        let touched = match change {
            Change::Add { range, text: body } => {
                let body = checked_text(&body)?;
                if range.is_empty()
                    || range.end > text.len()
                    || !text.is_char_boundary(range.start)
                    || !text.is_char_boundary(range.end)
                {
                    return Err(EngineError::InvalidRequest(
                        "a comment is on a range of the page's text".to_owned(),
                    ));
                }
                if page.comments.len() >= page_meta::MAX_COMMENTS {
                    return Err(EngineError::Refused(MetaProblem::TooMany.to_string()));
                }
                let (quote, before, after) = context(&text, range.start, range.end);
                let new = new_id(&taken);
                page.comments.push(Live {
                    stored: StoredComment {
                        id: new,
                        author: author.name,
                        email: author.email,
                        created: now,
                        edited: None,
                        start: range.start,
                        end: range.end,
                        revision: revision_of(&text),
                        quote,
                        before,
                        after,
                        text: body,
                        resolved: None,
                        detached: false,
                        replies: Vec::new(),
                    },
                    range,
                });
                new
            }
            Change::Reply {
                comment,
                text: body,
            } => {
                let body = checked_text(&body)?;
                let at = page.live(CommentId(comment))?;
                let replies = &mut page.comments[at].stored.replies;
                if replies.len() >= page_meta::MAX_REPLIES {
                    return Err(EngineError::Refused(MetaProblem::TooMany.to_string()));
                }
                let new = new_id(&taken);
                replies.push(StoredReply {
                    id: new,
                    author: author.name,
                    email: author.email,
                    created: now,
                    edited: None,
                    text: body,
                });
                new
            }
            Change::Resolve { comment, resolved } => {
                let at = page.live(CommentId(comment))?;
                let stored = &mut page.comments[at].stored;
                if !actor
                    .allowed(&stored.author, stored.email.as_deref())
                    .resolve
                {
                    return Err(not_allowed("resolve or reopen this comment"));
                }
                stored.resolved = resolved.then_some(Resolution {
                    by: author.name,
                    at: now,
                });
                comment
            }
            Change::Edit {
                comment,
                reply,
                text: body,
            } => {
                let body = checked_text(&body)?;
                let at = page.live(CommentId(comment))?;
                let stored = &mut page.comments[at].stored;
                match reply {
                    None => {
                        if !actor.allowed(&stored.author, stored.email.as_deref()).edit {
                            return Err(not_allowed("edit someone else's comment"));
                        }
                        stored.text = body;
                        stored.edited = Some(now);
                        comment
                    }
                    Some(reply) => {
                        let reply = stored
                            .replies
                            .iter_mut()
                            .find(|stored| stored.id == reply)
                            .ok_or_else(|| {
                                EngineError::NotFound(format!("reply {}", CommentId(reply)))
                            })?;
                        if !actor.allowed(&reply.author, reply.email.as_deref()).edit {
                            return Err(not_allowed("edit someone else's reply"));
                        }
                        reply.text = body;
                        reply.edited = Some(now);
                        reply.id
                    }
                }
            }
            Change::Delete { comment, reply } => {
                let at = page.live(CommentId(comment))?;
                match reply {
                    None => {
                        let stored = &page.comments[at].stored;
                        if !actor
                            .allowed(&stored.author, stored.email.as_deref())
                            .delete
                        {
                            return Err(not_allowed("delete someone else's comment"));
                        }
                        let removed = page.comments.remove(at);
                        page.deleted.insert(removed.stored.id);
                        page.deleted
                            .extend(removed.stored.replies.iter().map(|reply| reply.id));
                        page.changed.remove(&removed.stored.id);
                        0
                    }
                    Some(reply) => {
                        let replies = &mut page.comments[at].stored.replies;
                        let found = replies
                            .iter()
                            .find(|stored| stored.id == reply)
                            .ok_or_else(|| {
                                EngineError::NotFound(format!("reply {}", CommentId(reply)))
                            })?;
                        if !actor.allowed(&found.author, found.email.as_deref()).delete {
                            return Err(not_allowed("delete someone else's reply"));
                        }
                        replies.retain(|stored| stored.id != reply);
                        page.deleted.insert(reply);
                        0
                    }
                }
            }
        };
        if touched != 0 {
            page.changed.insert(touched);
            if let Some(comment) = page
                .comments
                .iter()
                .find(|live| live.stored.replies.iter().any(|reply| reply.id == touched))
            {
                // A reply's comment is written with it.
                page.changed.insert(comment.stored.id);
            }
        }
        page.generation += 1;
        page.write_at = Some(self.clock.now() + COMMENT_SETTLE);
        self.emit(EngineEvent::CommentsChanged(id));
        Ok(())
    }

    /// The range of the open comment after (or before) `offset`, wrapping round the page.
    fn step_comment(
        &self,
        id: DocumentId,
        offset: usize,
        forward: bool,
    ) -> Result<SourceRange, EngineError> {
        let mut ranges: Vec<SourceRange> = self
            .comments(id)
            .into_iter()
            .filter(|comment| !comment.detached && comment.resolved.is_none())
            .map(|comment| comment.range)
            .collect();
        ranges.sort_by_key(|range| (range.start, range.end));
        let found = if forward {
            ranges
                .iter()
                .find(|range| range.start > offset)
                .or(ranges.first())
        } else {
            ranges
                .iter()
                .rev()
                .find(|range| range.start < offset)
                .or(ranges.last())
        };
        found
            .copied()
            .ok_or_else(|| EngineError::NotFound("a comment on this page".to_owned()))
    }

    /// When a page's comments next need the pump: a write's settle, or a search for detached
    /// comments.
    pub(crate) fn comments_due(&self) -> Option<Instant> {
        self.page_comments
            .values()
            .chain(self.closing_comments.values())
            .flat_map(|page| {
                let write = page.write_at.filter(|_| page.writing.is_none());
                [write, page.search_detached_at]
            })
            .flatten()
            .min()
    }

    /// Searches for detached comments once typing pauses, and writes comments whose settle has
    /// passed.
    pub(crate) fn update_due_comments(&mut self) -> usize {
        let now = self.clock.now();
        let mut done = 0;
        let searching: Vec<DocumentId> = self
            .page_comments
            .iter()
            .filter(|(_, page)| page.search_detached_at.is_some_and(|at| at <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in searching {
            done += 1;
            self.search_detached(id);
        }
        let open: Vec<DocumentId> = self
            .page_comments
            .iter()
            .filter(|(_, page)| page.writing.is_none() && page.write_at.is_some_and(|at| at <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in open {
            done += 1;
            self.write_comments(Some(id), None);
        }
        let closed: Vec<PathBuf> = self
            .closing_comments
            .iter()
            .filter(|(_, page)| page.writing.is_none() && page.write_at.is_some_and(|at| at <= now))
            .map(|(path, _)| path.clone())
            .collect();
        for path in closed {
            done += 1;
            self.write_comments(None, Some(path));
        }
        done
    }

    fn search_detached(&mut self, id: DocumentId) {
        let Some(text) = self
            .documents
            .get(&id)
            .map(|document| document.text.to_string())
        else {
            return;
        };
        let Some(page) = self.page_comments.get_mut(&id) else {
            return;
        };
        page.search_detached_at = None;
        let mut found = false;
        for live in page.comments.iter_mut().filter(|live| live.stored.detached) {
            let stored = &live.stored;
            if let Some(range) = find_quote(
                &text,
                &stored.quote,
                &stored.before,
                &stored.after,
                live.range.start,
            ) {
                live.range = range;
                live.stored.detached = false;
                page.changed.insert(live.stored.id);
                found = true;
            }
        }
        if found {
            page.generation += 1;
            page.write_at = Some(self.clock.now() + COMMENT_SETTLE);
            self.emit(EngineEvent::CommentsChanged(id));
        }
    }

    /// Sends the comments of open document `id`, or of the closed page at `closed`, to the
    /// writer, rendered as they stand.
    fn write_comments(&mut self, id: Option<DocumentId>, closed: Option<PathBuf>) {
        let text = id.and_then(|id| {
            self.documents
                .get(&id)
                .map(|document| document.text.to_string())
        });
        let page = match (&id, &closed) {
            (Some(id), _) => self.page_comments.get_mut(id),
            (None, Some(path)) => self.closing_comments.get_mut(path),
            (None, None) => None,
        };
        let Some(page) = page else {
            return;
        };
        page.write_at = None;
        let Some(sidecar) = page.sidecar.clone() else {
            return;
        };
        if page.state != CommentsState::Usable || (!page.pending() && page.failed.is_none()) {
            return;
        }
        if let Some(text) = &text {
            refresh_changed(page, text);
        }
        let comments: Vec<StoredComment> = page
            .comments
            .iter()
            .map(|live| live.stored.clone())
            .collect();
        let rendered = match page.meta.render(&comments) {
            Ok(rendered) => rendered,
            Err(problem) => {
                page.failed = Some(problem.to_string());
                return;
            }
        };
        let ticket = self.next_comment_ticket;
        self.next_comment_ticket += 1;
        page.writing = Some(Writing {
            ticket,
            text: rendered.clone(),
            changed: page.changed.clone(),
            deleted: page.deleted.clone(),
        });
        let expected = page.on_disk;
        let target = match id {
            Some(id) => crate::writer::SidecarTarget::Open(id),
            None => crate::writer::SidecarTarget::Closed(closed.unwrap_or_default()),
        };
        self.write_sidecar(crate::writer::SidecarJob {
            ticket,
            target,
            path: sidecar,
            expected,
            contents: rendered,
        });
    }

    /// Applies the writer's answer to a sidecar write.
    pub(crate) fn sidecar_written(
        &mut self,
        target: crate::writer::SidecarTarget,
        ticket: u64,
        result: Result<Option<[u8; 32]>, crate::writer::SaveFailure>,
    ) {
        let now = self.clock.now();
        let (page, id) = match &target {
            crate::writer::SidecarTarget::Open(id) => (self.page_comments.get_mut(id), Some(*id)),
            crate::writer::SidecarTarget::Closed(path) => {
                (self.closing_comments.get_mut(path), None)
            }
        };
        let Some(page) = page else {
            return;
        };
        let Some(writing) = page
            .writing
            .take()
            .filter(|writing| writing.ticket == ticket)
        else {
            return;
        };
        match result {
            Ok(on_disk) => {
                page.on_disk = on_disk;
                page.failed = None;
                page.meta = writing
                    .text
                    .as_deref()
                    .and_then(|text| page_meta::parse(text.as_bytes()).ok())
                    .unwrap_or_default();
                page.changed.retain(|id| !writing.changed.contains(id));
                page.deleted.retain(|id| !writing.deleted.contains(id));
                if page.pending() {
                    page.write_at = Some(now + COMMENT_SETTLE);
                }
            }
            Err(crate::writer::SaveFailure::Conflict) => {
                // Changed on disk since it was read: read it again and write what was made here
                // on top.
                let text = id.and_then(|id| {
                    self.documents
                        .get(&id)
                        .map(|document| document.text.to_string())
                });
                let page = match &target {
                    crate::writer::SidecarTarget::Open(id) => self.page_comments.get_mut(id),
                    crate::writer::SidecarTarget::Closed(path) => {
                        self.closing_comments.get_mut(path)
                    }
                };
                if let Some(page) = page {
                    reread(page, text.as_deref());
                    page.generation += 1;
                    page.write_at = Some(now);
                }
            }
            Err(crate::writer::SaveFailure::Io(error)) => {
                page.failed = Some(format!("The comments file could not be written: {error}"));
                page.write_at = Some(now + RETRY_FAILED_WRITE);
            }
        }
        if let crate::writer::SidecarTarget::Closed(path) = &target {
            if self
                .closing_comments
                .get(path)
                .is_some_and(|page| !page.pending() && page.writing.is_none())
            {
                self.closing_comments.remove(path);
            }
        }
        if let Some(id) = id {
            self.emit(EngineEvent::CommentsChanged(id));
        }
    }

    /// The watcher reports `path` changed: if it is an open page's comments file, and not as
    /// last read or written here, it is read again and what was made here kept on top.
    pub(crate) fn sidecar_changed_on_disk(&mut self, path: &Path) -> bool {
        let Some(page_path) = page_meta::page_of_sidecar(path) else {
            return false;
        };
        let Some(id) = self.document_for_path(&page_path) else {
            return true;
        };
        let text = self
            .documents
            .get(&id)
            .map(|document| document.text.to_string());
        let Some(page) = self.page_comments.get_mut(&id) else {
            return true;
        };
        if page.writing.is_some() {
            // The write checks for itself, and reads it again if it changed.
            return true;
        }
        let now_on_disk = read_sidecar(path).ok().flatten();
        if now_on_disk.as_deref().map(digest) == page.on_disk {
            return true;
        }
        reread(page, text.as_deref());
        page.generation += 1;
        if page.pending() {
            page.write_at = Some(self.clock.now() + COMMENT_SETTLE);
        }
        self.emit(EngineEvent::CommentsChanged(id));
        true
    }

    /// Writes every page's comments still to be written and waits for them: for an engine
    /// about to go.
    pub(crate) fn write_comments_now(&mut self) {
        for _ in 0..3 {
            let open: Vec<DocumentId> = self
                .page_comments
                .iter()
                .filter(|(_, page)| page.pending() && page.writing.is_none())
                .map(|(id, _)| *id)
                .collect();
            let closed: Vec<PathBuf> = self
                .closing_comments
                .iter()
                .filter(|(_, page)| page.pending() && page.writing.is_none())
                .map(|(path, _)| path.clone())
                .collect();
            if open.is_empty() && closed.is_empty() {
                return;
            }
            for id in open {
                self.write_comments(Some(id), None);
            }
            for path in closed {
                self.write_comments(None, Some(path));
            }
            while self
                .page_comments
                .values()
                .chain(self.closing_comments.values())
                .any(|page| page.writing.is_some())
            {
                if !self.apply_next_worker_message() {
                    return;
                }
            }
        }
    }
}

/// Gives each comment changed here the offsets, revision and context of `text`, the page's
/// text now; the others keep what they were read with.
fn refresh_changed(page: &mut PageComments, text: &str) {
    let revision = revision_of(text);
    for live in &mut page.comments {
        if !page.changed.contains(&live.stored.id) {
            continue;
        }
        let stored = &mut live.stored;
        stored.start = live.range.start;
        stored.end = live.range.end;
        stored.revision.clone_from(&revision);
        if !stored.detached {
            let (quote, before, after) = context(text, live.range.start, live.range.end);
            stored.quote = quote;
            stored.before = before;
            stored.after = after;
        }
    }
}

/// Reads a page's comments from `bytes` into `page`, placing each in `text`.
fn read_into(page: &mut PageComments, bytes: &[u8], text: &str) {
    page.on_disk = Some(digest(bytes));
    match page_meta::parse(bytes) {
        Ok(meta) => {
            let revision = revision_of(text);
            page.comments = meta
                .comments
                .iter()
                .map(|stored| {
                    let (range, detached) = anchor(stored, text, &revision);
                    let mut stored = stored.clone();
                    stored.detached = detached;
                    Live { stored, range }
                })
                .collect();
            page.state = match meta.newer {
                Some(format) => {
                    CommentsState::ReadOnly(MetaProblem::NewerFormat(format).to_string())
                }
                None => CommentsState::Usable,
            };
            page.meta = meta;
        }
        Err(problem) => {
            page.state = CommentsState::Unusable(problem.to_string());
        }
    }
}

/// Reads `page`'s file again, keeping on top what was changed or deleted here since it was last
/// read, matched by id: a comment or reply made or edited here is kept, one deleted here stays
/// deleted, and the rest is the file's.
fn reread(page: &mut PageComments, text: Option<&str>) {
    let Some(sidecar) = page.sidecar.clone() else {
        return;
    };
    let ours: BTreeMap<u64, Live> = page
        .comments
        .drain(..)
        .map(|live| (live.stored.id, live))
        .collect();
    let bytes = match read_sidecar(&sidecar) {
        Ok(bytes) => bytes,
        Err(error) => {
            page.comments = ours.into_values().collect();
            page.state =
                CommentsState::Unusable(format!("The comments file cannot be read: {error}"));
            return;
        }
    };
    let text = text.unwrap_or_default();
    match &bytes {
        None => {
            page.on_disk = None;
            page.meta = PageMeta::default();
            page.state = CommentsState::Usable;
        }
        Some(bytes) => read_into(page, bytes, text),
    }
    if page.state != CommentsState::Usable && bytes.is_some() {
        // Unusable now: what is held here stays, and nothing is written until it is usable.
        page.comments = ours.into_values().collect();
        return;
    }
    let theirs: Vec<Live> = std::mem::take(&mut page.comments);
    let mut merged: Vec<Live> = Vec::new();
    for mut live in theirs {
        let id = live.stored.id;
        if page.deleted.contains(&id) {
            continue;
        }
        if let Some(mine) = ours.get(&id) {
            if page.changed.contains(&id) {
                let replies = merge_replies(&live.stored.replies, &mine.stored.replies, page);
                live = mine.clone();
                live.stored.replies = replies;
            } else {
                let replies = merge_replies(&live.stored.replies, &mine.stored.replies, page);
                live.stored.replies = replies;
                live.range = mine.range;
                live.stored.detached = mine.stored.detached;
            }
        }
        live.stored
            .replies
            .retain(|reply| !page.deleted.contains(&reply.id));
        merged.push(live);
    }
    for (id, mine) in ours {
        let ours_changed = page.changed.contains(&id)
            || mine
                .stored
                .replies
                .iter()
                .any(|reply| page.changed.contains(&reply.id));
        if ours_changed && !merged.iter().any(|live| live.stored.id == id) {
            merged.push(mine);
        }
    }
    page.comments = merged;
}

/// The replies of a comment read again: the file's, with those changed here in their place or
/// added, and those deleted here gone.
fn merge_replies(
    theirs: &[StoredReply],
    mine: &[StoredReply],
    page: &PageComments,
) -> Vec<StoredReply> {
    let mut replies: Vec<StoredReply> = theirs
        .iter()
        .filter(|reply| !page.deleted.contains(&reply.id))
        .map(|reply| {
            mine.iter()
                .find(|mine| mine.id == reply.id && page.changed.contains(&mine.id))
                .unwrap_or(reply)
                .clone()
        })
        .collect();
    for reply in mine {
        if page.changed.contains(&reply.id) && !replies.iter().any(|kept| kept.id == reply.id) {
            replies.push(reply.clone());
        }
    }
    replies.sort_by_key(|reply| reply.created);
    replies
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(at: usize, text: &str) -> TextEdit {
        TextEdit::insert(at, text)
    }

    fn replace(start: usize, end: usize, text: &str) -> TextEdit {
        TextEdit::new(SourceRange::new(start, end), text)
    }

    #[test]
    fn a_range_moves_with_edits() {
        let range = SourceRange::new(10, 20);
        let mapped = |edits: &[TextEdit]| {
            let mapped = map_range(range, edits);
            (mapped.start, mapped.end)
        };
        assert_eq!(mapped(&[insert(0, "abc")]), (13, 23), "before it");
        assert_eq!(mapped(&[insert(30, "abc")]), (10, 20), "after it");
        assert_eq!(mapped(&[insert(15, "abc")]), (10, 23), "inside grows it");
        assert_eq!(
            mapped(&[insert(10, "abc")]),
            (13, 23),
            "at its start, outside"
        );
        assert_eq!(
            mapped(&[insert(20, "abc")]),
            (10, 20),
            "at its end, outside"
        );
        assert_eq!(
            mapped(&[replace(12, 15, "")]),
            (10, 17),
            "part deleted shrinks it"
        );
        assert_eq!(mapped(&[replace(5, 15, "")]), (5, 10), "its start deleted");
        assert_eq!(mapped(&[replace(15, 25, "")]), (10, 15), "its end deleted");
        assert_eq!(mapped(&[replace(5, 25, "")]), (5, 5), "all of it deleted");
        assert_eq!(
            mapped(&[replace(15, 20, "words")]),
            (10, 20),
            "its last word typed again stays in it"
        );
        assert_eq!(
            mapped(&[replace(10, 13, "The")]),
            (10, 20),
            "its first word typed again stays in it"
        );
        assert_eq!(
            mapped(&[insert(0, "ab"), insert(15, "cd"), insert(30, "ef")]),
            (12, 24),
            "several at once"
        );
    }

    #[test]
    fn edits_inside_a_range_are_told_apart() {
        let range = SourceRange::new(10, 20);
        assert!(changes_inside(range, &[insert(15, "a")]));
        assert!(changes_inside(range, &[replace(5, 11, "")]));
        assert!(!changes_inside(range, &[insert(10, "a")]));
        assert!(!changes_inside(range, &[insert(20, "a")]));
        assert!(!changes_inside(range, &[replace(20, 25, "")]));
    }

    #[test]
    fn the_nearer_of_two_quotes_wins() {
        let text = "alpha beta gamma. alpha beta gamma.";
        assert_eq!(
            find_quote(text, "beta", "alpha ", " gamma", 24),
            Some(SourceRange::new(24, 28))
        );
        assert_eq!(
            find_quote(text, "beta", "alpha ", " gamma", 0),
            Some(SourceRange::new(6, 10))
        );
        let text = "one beta two. alpha beta gamma.";
        assert_eq!(
            find_quote(text, "beta", "alpha ", " gamma", 0),
            Some(SourceRange::new(20, 24)),
            "better context wins over nearness"
        );
        assert_eq!(find_quote(text, "delta", "", "", 0), None);
        assert_eq!(find_quote(text, "", "", "", 0), None);
    }
}
