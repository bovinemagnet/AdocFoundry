//! Comments in a collaboration session (#762, COMMENTS 2/4): the host holds a shared page's
//! comments and decides who may do what; every guest with the page open is told them, placed in
//! the shared text, and asks the host for its own changes.

#![cfg(feature = "collab")]

mod collab_common;

use std::{
    fs,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids,
    protocol::{CommentChange, ErrorCode, Message, PeerId},
    CommandArgs, Comment, CommentId, CommentRequest, DocumentId, Engine, EngineError,
};
use collab_common::*;

const PAGE: &str = "= Install\n\nOn macOS the installer refuses an unsigned package.\n";
const QUOTE: &str = "the installer refuses";

fn session() -> Session {
    Session::new(&[("install.adoc", PAGE)])
}

fn guest_doc(session: &mut Session, peer: PeerId) -> DocumentId {
    session.engine(peer).session_document(1).unwrap()
}

fn invoke(
    engine: &mut Engine,
    command: adoc_engine::CommandId,
    request: CommentRequest,
) -> Result<(), EngineError> {
    engine
        .invoke(command, CommandArgs::Comment(request))
        .map(|_| ())
}

fn add(engine: &mut Engine, id: DocumentId, text: &str) -> Result<(), EngineError> {
    let start = engine
        .document(id)
        .unwrap()
        .text
        .to_string()
        .find(QUOTE)
        .unwrap();
    invoke(
        engine,
        ids::COMMENTS_ADD,
        CommentRequest::Add {
            id,
            range: SourceRange::new(start, start + QUOTE.len()),
            text: text.to_owned(),
        },
    )
}

/// What every participant must agree on: each comment's id, quoted text, text, resolution and
/// replies.
fn shown(comments: &[Comment]) -> Vec<(CommentId, String, String, bool, Vec<String>)> {
    comments
        .iter()
        .map(|comment| {
            (
                comment.id,
                comment.quote.clone(),
                comment.text.clone(),
                comment.resolved.is_some(),
                comment
                    .replies
                    .iter()
                    .map(|reply| reply.text.clone())
                    .collect(),
            )
        })
        .collect()
}

fn agree(session: &mut Session, peer: PeerId) -> Vec<Comment> {
    let host = session.host.comments(session.host_doc());
    let id = guest_doc(session, peer);
    let guest = session.engine(peer).comments(id);
    assert_eq!(shown(&guest), shown(&host));
    host
}

#[test]
fn a_hosts_comments_reach_a_guest_placed_on_the_same_text() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    add(&mut session.host, host, "Still true?").unwrap();
    session.settle();
    let comments = agree(&mut session, ada);
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].quote, QUOTE);
    assert_eq!(comments[0].author.name, session.host.comment_author().name);

    // Replies, resolving and deleting follow.
    let comment = comments[0].id;
    invoke(
        &mut session.host,
        ids::COMMENTS_REPLY,
        CommentRequest::Reply {
            id: host,
            comment,
            text: "Yes.".to_owned(),
        },
    )
    .unwrap();
    invoke(
        &mut session.host,
        ids::COMMENTS_RESOLVE,
        CommentRequest::Resolve {
            id: host,
            comment,
            resolved: true,
        },
    )
    .unwrap();
    session.settle();
    agree(&mut session, ada);
    invoke(
        &mut session.host,
        ids::COMMENTS_DELETE,
        CommentRequest::Delete {
            id: host,
            comment,
            reply: None,
        },
    )
    .unwrap();
    session.settle();
    assert!(agree(&mut session, ada).is_empty());
}

#[test]
fn a_guests_comment_is_the_hosts_to_keep() {
    let mut session = session();
    let ada = session.guest(1, true);
    let id = guest_doc(&mut session, ada);
    add(session.engine(ada), id, "Why?").unwrap();
    session.settle();
    let comments = agree(&mut session, ada);
    assert_eq!(comments.len(), 1);
    assert_eq!(
        comments[0].author.name, "guest 1",
        "named by the host's roster"
    );

    // Written beside the host's page, never anywhere on the guest's side.
    let sidecar = session.directory.path().join(".install.adoc.afd");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sidecar.exists() && Instant::now() < deadline {
        session.settle();
        std::thread::sleep(Duration::from_millis(50));
    }
    let written = fs::read_to_string(&sidecar).unwrap();
    assert!(written.contains("author = \"guest 1\""), "{written}");
    assert!(written.contains("Why?"), "{written}");
    session.assert_no_host_paths();
}

#[test]
fn a_viewer_can_comment_but_not_edit() {
    let mut session = session();
    let bea = session.guest(2, false);
    let id = guest_doc(&mut session, bea);
    assert!(session
        .engine(bea)
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .is_err());
    add(session.engine(bea), id, "A Viewer's question.").unwrap();
    session.settle();
    let comments = agree(&mut session, bea);
    assert_eq!(comments.len(), 1);
    let mine = session.engine(bea).comments(id);
    assert!(
        mine[0].can.edit && mine[0].can.delete && mine[0].can.resolve,
        "its own: {:?}",
        mine[0]
    );
    assert_eq!(session.host_text(), PAGE, "the text is as it was");
}

#[test]
fn a_comment_follows_its_text_through_concurrent_edits() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    add(&mut session.host, host, "Here.").unwrap();
    session.settle();
    let start = session.host_text().find(QUOTE).unwrap();

    // The host types before the comment while the guest types inside it, both at once.
    type_at(&mut session.host, host, 0, "// Draft\n");
    guest_types(&mut session, ada, start + "the installer".len(), " always");
    session.settle();
    let comments = agree(&mut session, ada);
    assert_eq!(comments[0].quote, "the installer always refuses");

    // And the other way about: the guest before it, the host inside it.
    let start = session.host_text().find("the installer always").unwrap();
    guest_types(&mut session, ada, 0, "// Note\n");
    type_at(&mut session.host, host, start + "the".len(), " new");
    session.settle();
    let comments = agree(&mut session, ada);
    assert_eq!(comments[0].quote, "the new installer always refuses");
    assert_eq!(session.host_text(), session.guest_text(ada));
}

#[test]
fn what_a_participant_may_do_is_the_hosts_to_decide() {
    let mut session = session();
    let ada = session.guest(1, true);
    let bea = session.guest(2, false);
    let host = session.host_doc();
    add(&mut session.host, host, "The host's.").unwrap();
    session.settle();
    let comment = session.host.comments(host)[0].id;

    // A Viewer cannot resolve, edit or delete the host's comment: refused on its own side.
    let id = guest_doc(&mut session, bea);
    let shown = session.engine(bea).comments(id);
    assert!(!shown[0].can.resolve && !shown[0].can.edit && !shown[0].can.delete);
    for (command, request) in [
        (
            ids::COMMENTS_RESOLVE,
            CommentRequest::Resolve {
                id,
                comment,
                resolved: true,
            },
        ),
        (
            ids::COMMENTS_EDIT,
            CommentRequest::Edit {
                id,
                comment,
                reply: None,
                text: "Mine now.".to_owned(),
            },
        ),
        (
            ids::COMMENTS_DELETE,
            CommentRequest::Delete {
                id,
                comment,
                reply: None,
            },
        ),
    ] {
        assert!(matches!(
            invoke(session.engine(bea), command, request),
            Err(EngineError::Refused(_))
        ));
    }
    // And on the host's, when a modified guest asks anyway.
    session.forge(
        bea,
        Message::Comment {
            file: 1,
            epoch: 1,
            request: 77,
            change: CommentChange::Delete {
                comment: comment.0,
                reply: None,
            },
            selection: None,
        },
    );
    session.settle();
    assert!(session
        .sent_to(bea)
        .iter()
        .any(|message| matches!(message, Message::CommentRefused { request: 77, .. })));
    assert_eq!(session.host.comments(host).len(), 1);
    assert!(
        session.removed().is_empty(),
        "a refusal keeps the connection"
    );

    // An Editor resolves it, but edits only its own.
    let id = guest_doc(&mut session, ada);
    invoke(
        session.engine(ada),
        ids::COMMENTS_RESOLVE,
        CommentRequest::Resolve {
            id,
            comment,
            resolved: true,
        },
    )
    .unwrap();
    session.settle();
    assert!(session.host.comments(host)[0].resolved.is_some());
    assert!(!session.engine(ada).comments(id)[0].can.edit);
}

#[test]
fn a_request_from_an_earlier_epoch_is_refused() {
    let mut session = session();
    let ada = session.guest(1, true);
    session.forge(
        ada,
        Message::Comment {
            file: 1,
            epoch: 0,
            request: 5,
            change: CommentChange::Reply {
                comment: 1,
                text: "Late.".to_owned(),
            },
            selection: None,
        },
    );
    session.settle();
    assert!(session
        .sent_to(ada)
        .iter()
        .any(|message| matches!(message, Message::CommentRefused { request: 5, .. })));
    assert!(session.removed().is_empty());
}

#[test]
fn a_comment_on_a_file_not_opened_closes_the_connection() {
    let mut session = session();
    let ada = session.guest(1, true);
    session.forge(
        ada,
        Message::Comment {
            file: 9,
            epoch: 1,
            request: 1,
            change: CommentChange::Reply {
                comment: 1,
                text: "Elsewhere.".to_owned(),
            },
            selection: None,
        },
    );
    session.settle();
    assert_eq!(session.removed(), [(ada, ErrorCode::Protocol)]);
}

#[test]
fn comments_are_told_again_after_a_reconnection() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    add(&mut session.host, host, "Before the drop.").unwrap();
    session.settle();
    session.drop_link(ada, 0);
    session.settle();
    let comment = session.host.comments(host)[0].id;
    invoke(
        &mut session.host,
        ids::COMMENTS_REPLY,
        CommentRequest::Reply {
            id: host,
            comment,
            text: "While away.".to_owned(),
        },
    )
    .unwrap();
    session.settle();
    session.resume(ada);
    session.settle();
    let comments = agree(&mut session, ada);
    assert_eq!(comments[0].replies.len(), 1);
}

#[test]
fn a_guest_cannot_comment_while_the_host_is_away() {
    let mut session = session();
    let ada = session.guest(1, true);
    session.drop_link(ada, 0);
    session.settle();
    let id = guest_doc(&mut session, ada);
    assert!(matches!(
        add(session.engine(ada), id, "Unsent."),
        Err(EngineError::Refused(_))
    ));
}

#[test]
fn guests_are_never_told_an_email_address() {
    let mut settings = adoc_engine::Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    settings.comments.name = Some("Hester".to_owned());
    settings.comments.email = Some("hester@example.org".to_owned());
    let mut session = Session::with_host(&[("install.adoc", PAGE)], Engine::new(settings));
    let ada = session.guest(1, true);
    let host = session.host_doc();
    add(&mut session.host, host, "With an address.").unwrap();
    assert_eq!(
        session.host.comments(host)[0].author.email.as_deref(),
        Some("hester@example.org")
    );
    session.settle();
    let id = guest_doc(&mut session, ada);
    assert!(session.engine(ada).comments(id)[0].author.email.is_none());
    assert!(session.sent_to(ada).iter().all(|message| !matches!(
        message,
        Message::CommentThread { thread, .. } if thread.email.is_some()
            || thread.replies.iter().any(|reply| reply.email.is_some())
    )));
}
