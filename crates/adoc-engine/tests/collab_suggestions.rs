//! Suggested changes in a collaboration session (#763, SUGGEST 2/4; spec §8): the host holds
//! every suggestion, checks every request against its own text, and applies every accept once,
//! a guest's in nobody's undo history.

#![cfg(feature = "collab")]

mod collab_common;

use std::{
    fs,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids,
    protocol::{CommentChange, Message, PeerId},
    CoAuthor, CommandArgs, Comment, CommentId, CommentRequest, DocumentId, Engine, EngineError,
    SuggestionState,
};
use collab_common::*;

const PAGE: &str = "= Install\n\nOn macOS the installer refuses an unsigned package.\n";
const QUOTE: &str = "the installer";
const REPLACEMENT: &str = "Gatekeeper";
const AWAY: &str = "The host cannot be reached; try again once reconnected.";

fn session() -> Session {
    Session::new(&[("install.adoc", PAGE)])
}

fn accepted_page() -> String {
    PAGE.replace(QUOTE, REPLACEMENT)
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

/// Suggests [`REPLACEMENT`] for [`QUOTE`] on `engine`'s page `id`, with no note.
fn suggest(engine: &mut Engine, id: DocumentId) -> Result<(), EngineError> {
    suggest_instead(engine, id, REPLACEMENT)
}

/// Suggests `replacement` for [`QUOTE`] on `engine`'s page `id`, with no note.
fn suggest_instead(
    engine: &mut Engine,
    id: DocumentId,
    replacement: &str,
) -> Result<(), EngineError> {
    let start = engine
        .document(id)
        .unwrap()
        .text
        .to_string()
        .find(QUOTE)
        .unwrap();
    invoke(
        engine,
        ids::COMMENTS_SUGGEST,
        CommentRequest::Suggest {
            id,
            range: SourceRange::new(start, start + QUOTE.len()),
            replacement: replacement.to_owned(),
            text: String::new(),
        },
    )
}

fn accept(engine: &mut Engine, id: DocumentId, comment: CommentId) -> Result<(), EngineError> {
    invoke(
        engine,
        ids::COMMENTS_ACCEPT,
        CommentRequest::Accept { id, comment },
    )
}

fn undo(engine: &mut Engine, id: DocumentId) {
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
}

/// The one comment on page `id`.
fn the_comment(engine: &Engine, id: DocumentId) -> Comment {
    let comments = engine.comments(id);
    assert_eq!(comments.len(), 1, "{comments:?}");
    comments.into_iter().next().unwrap()
}

fn state(engine: &Engine, id: DocumentId) -> SuggestionState {
    the_comment(engine, id)
        .suggestion
        .expect("a suggestion")
        .state
}

fn host_state(session: &Session) -> SuggestionState {
    state(&session.host, session.host_doc())
}

fn guest_state(session: &mut Session, peer: PeerId) -> SuggestionState {
    let id = guest_doc(session, peer);
    state(session.engine(peer), id)
}

/// The reasons the host gave `peer` for refusing its requests.
fn refusals(session: &Session, peer: PeerId) -> Vec<String> {
    session
        .sent_to(peer)
        .into_iter()
        .filter_map(|message| match message {
            Message::CommentRefused { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// Every participant's text, which must agree.
fn texts(session: &mut Session, peers: &[PeerId]) -> String {
    let host = session.host_text();
    for peer in peers {
        assert_eq!(session.guest_text(*peer), host, "{peer:?}");
    }
    host
}

fn sidecar(session: &mut Session) -> String {
    let sidecar = session.directory.path().join(".install.adoc.afd");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sidecar.exists() && Instant::now() < deadline {
        session.settle();
        std::thread::sleep(Duration::from_millis(50));
    }
    fs::read_to_string(&sidecar).unwrap()
}

#[test]
fn a_viewers_suggestion_changes_nothing_until_accepted() {
    let mut session = session();
    let bea = session.guest(2, false);
    let id = guest_doc(&mut session, bea);
    suggest(session.engine(bea), id).unwrap();
    session.settle();
    assert_eq!(texts(&mut session, &[bea]), PAGE, "nothing changed yet");

    let host = session.host_doc();
    let held = the_comment(&session.host, host);
    assert_eq!(held.author.name, "guest 2");
    let suggestion = held.suggestion.unwrap();
    assert_eq!(suggestion.original, QUOTE);
    assert_eq!(suggestion.replacement, REPLACEMENT);
    assert_eq!(suggestion.state, SuggestionState::Pending);
    let written = sidecar(&mut session);
    assert!(written.contains("author = \"guest 2\""), "{written}");
    assert!(written.contains(REPLACEMENT), "{written}");

    // The Viewer sees it pending, its own to withdraw but not to accept.
    let shown = the_comment(session.engine(bea), id);
    assert_eq!(shown.suggestion.unwrap().state, SuggestionState::Pending);
    assert!(!shown.can.accept && shown.can.reject, "{:?}", shown.can);
    assert_eq!(shown.text, "", "an empty note stays empty");

    accept(&mut session.host, host, held.id).unwrap();
    session.settle();
    let text = texts(&mut session, &[bea]);
    assert_eq!(text, accepted_page());
    assert_eq!(text.matches(REPLACEMENT).count(), 1);
    assert_eq!(guest_state(&mut session, bea), SuggestionState::Accepted);
}

#[test]
fn accepting_applies_the_suggested_edit_once_for_everyone() {
    let mut session = session();
    let ada = session.guest(1, true);
    let cy = session.guest(3, true);
    let host = session.host_doc();
    // Something of the host's and of the guest's own to undo.
    type_at(&mut session.host, host, 0, "// Draft\n");
    session.settle();
    let end = session.guest_text(ada).len();
    guest_types(&mut session, ada, end, "Ada.\n");
    session.settle();
    suggest(&mut session.host, host).unwrap();
    session.settle();

    let id = guest_doc(&mut session, ada);
    let comment = the_comment(session.engine(ada), id);
    assert!(comment.can.accept, "{:?}", comment.can);
    accept(session.engine(ada), id, comment.id).unwrap();
    assert!(
        !session.guest_text(ada).contains(REPLACEMENT),
        "never applied on the guest's side first"
    );
    session.settle();
    let text = texts(&mut session, &[ada, cy]);
    assert_eq!(text.matches(REPLACEMENT).count(), 1, "{text}");
    assert!(!text.contains(QUOTE));
    let held = the_comment(&session.host, host);
    assert_eq!(
        held.suggestion.unwrap().state,
        SuggestionState::Accepted,
        "accepted at once"
    );
    assert_eq!(held.resolved.unwrap().by, "guest 1");

    // The host's undo takes back its own typing, never the guest's accept.
    undo(&mut session.host, host);
    session.settle();
    let text = texts(&mut session, &[ada, cy]);
    assert!(!text.contains("// Draft"), "{text}");
    assert_eq!(text.matches(REPLACEMENT).count(), 1, "{text}");

    // Nor does the guest's: only its own typing goes.
    undo(session.engine(ada), id);
    session.settle();
    let text = texts(&mut session, &[ada, cy]);
    assert!(!text.contains("Ada."), "{text}");
    assert_eq!(text, accepted_page());
}

#[test]
fn two_accepts_apply_once() {
    let mut session = session();
    let ada = session.guest(1, true);
    let cy = session.guest(3, true);
    let host = session.host_doc();
    suggest(&mut session.host, host).unwrap();
    session.settle();
    let comment = the_comment(&session.host, host).id;

    // Both ask in the same pump.
    for peer in [ada, cy] {
        let id = guest_doc(&mut session, peer);
        accept(session.engine(peer), id, comment).unwrap();
    }
    session.settle();
    let text = texts(&mut session, &[ada, cy]);
    assert_eq!(text, accepted_page());
    assert_eq!(text.matches(REPLACEMENT).count(), 1);
    let refused: Vec<String> = [ada, cy]
        .iter()
        .flat_map(|peer| refusals(&session, *peer))
        .collect();
    assert_eq!(refused, ["This suggestion is decided already."]);
    assert!(
        session.removed().is_empty(),
        "a refusal keeps the connection"
    );
}

#[test]
fn a_viewer_cannot_accept() {
    let mut session = session();
    let bea = session.guest(2, false);
    let host = session.host_doc();
    suggest(&mut session.host, host).unwrap();
    session.settle();
    let comment = the_comment(&session.host, host).id;

    // Refused on its own side, nothing sent.
    let id = guest_doc(&mut session, bea);
    assert!(!the_comment(session.engine(bea), id).can.accept);
    let refused = accept(session.engine(bea), id, comment);
    assert!(
        matches!(&refused, Err(EngineError::Refused(reason)) if reason == "You cannot accept a suggestion."),
        "{refused:?}"
    );
    assert!(session.net.lock().unwrap().to_host.is_empty());

    // And by the host, when a modified guest asks anyway.
    session.forge(
        bea,
        Message::Comment {
            file: 1,
            epoch: 1,
            request: 88,
            change: CommentChange::Accept { comment: comment.0 },
            selection: None,
        },
    );
    session.settle();
    assert_eq!(refusals(&session, bea), ["You cannot accept a suggestion."]);
    assert_eq!(texts(&mut session, &[bea]), PAGE);
    assert_eq!(host_state(&session), SuggestionState::Pending);
    assert!(session.removed().is_empty());
}

#[test]
fn the_host_refuses_a_suggestion_made_on_other_text() {
    let mut session = session();
    let ada = session.guest(1, true);
    let id = guest_doc(&mut session, ada);
    suggest(session.engine(ada), id).unwrap();
    // The host types inside the selected text before the request reaches it.
    let host = session.host_doc();
    let start = session.host_text().find(QUOTE).unwrap();
    type_at(&mut session.host, host, start + "the".len(), " new");
    session.settle();
    assert_eq!(
        refusals(&session, ada),
        ["The text changed while you were suggesting; try again."]
    );
    assert!(session.host.comments(host).is_empty(), "nothing stored");
    assert!(session.engine(ada).comments(id).is_empty());
    assert!(session.removed().is_empty());
}

#[test]
fn a_guest_cannot_accept_while_away() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    suggest(&mut session.host, host).unwrap();
    session.settle();
    let comment = the_comment(&session.host, host).id;
    session.drop_link(ada, 0);
    session.settle();

    let id = guest_doc(&mut session, ada);
    let refused = accept(session.engine(ada), id, comment);
    assert!(
        matches!(&refused, Err(EngineError::Refused(reason)) if reason == AWAY),
        "{refused:?}"
    );
    session.resume(ada);
    session.settle();
    assert_eq!(texts(&mut session, &[ada]), PAGE, "nothing applied after");
    assert_eq!(host_state(&session), SuggestionState::Pending);
}

#[test]
fn accepting_a_guests_suggestion_credits_the_guest() {
    let mut session = session();
    let ada = session.guest(1, false);
    let id = guest_doc(&mut session, ada);
    suggest(session.engine(ada), id).unwrap();
    session.settle();
    let host = session.host_doc();
    let comment = the_comment(&session.host, host).id;
    let page = session.directory.path().join("install.adoc");
    assert!(session
        .host
        .coauthors_of(std::slice::from_ref(&page))
        .is_empty());
    accept(&mut session.host, host, comment).unwrap();
    session.settle();
    assert_eq!(
        session.host.coauthors_of(&[page]),
        [CoAuthor {
            name: "guest 1".to_owned(),
            email: "guest1@example.org".to_owned(),
        }]
    );
}

/// The host's own accept on a shared page is its own edit: Undo takes it back for everyone, the
/// suggestion stays accepted, and Reopen makes it pending again, so it can be accepted again.
/// The suggestion deletes at its range's start, so only the accept's reversal, matched against
/// the replica's undo, puts the range back over its original: the edge rule alone would leave
/// the restored text outside it.
#[test]
fn the_hosts_own_accept_on_a_shared_page_is_undone_and_accepted_again() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    suggest_instead(&mut session.host, host, "installer").unwrap();
    let comment = the_comment(&session.host, host).id;
    accept(&mut session.host, host, comment).unwrap();
    session.settle();
    let accepted = PAGE.replace(QUOTE, "installer");
    assert_eq!(texts(&mut session, &[ada]), accepted);

    undo(&mut session.host, host);
    session.settle();
    assert_eq!(texts(&mut session, &[ada]), PAGE);
    assert_eq!(host_state(&session), SuggestionState::Accepted);
    assert_eq!(the_comment(&session.host, host).quote, QUOTE);

    invoke(
        &mut session.host,
        ids::COMMENTS_REOPEN,
        CommentRequest::Resolve {
            id: host,
            comment,
            resolved: false,
        },
    )
    .unwrap();
    session.settle();
    assert_eq!(host_state(&session), SuggestionState::Pending);
    assert_eq!(guest_state(&mut session, ada), SuggestionState::Pending);
    accept(&mut session.host, host, comment).unwrap();
    session.settle();
    assert_eq!(texts(&mut session, &[ada]), accepted);
}
