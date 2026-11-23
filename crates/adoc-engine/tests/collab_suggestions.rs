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
            original: None,
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

/// A guest's box holds the range it was begun on while the guest's page changes: the text it
/// showed is what the host compares with its own, never the guest's text at the range by then.
#[test]
fn the_host_compares_the_text_a_guest_began_on() {
    let mut session = session();
    let ada = session.guest(1, true);
    let id = guest_doc(&mut session, ada);
    let start = PAGE.find(QUOTE).unwrap();
    let range = SourceRange::new(start, start + QUOTE.len());
    // The guest types before the range while its box is open.
    type_at(session.engine(ada), id, start - "macOS ".len(), "Apple ");
    session.settle();
    invoke(
        session.engine(ada),
        ids::COMMENTS_SUGGEST,
        CommentRequest::Suggest {
            id,
            range,
            original: Some(QUOTE.to_owned()),
            replacement: REPLACEMENT.to_owned(),
            text: String::new(),
        },
    )
    .unwrap();
    session.settle();
    assert_eq!(
        refusals(&session, ada),
        ["The text changed while you were suggesting; try again."]
    );
    let host = session.host_doc();
    assert!(session.host.comments(host).is_empty(), "nothing stored");
    assert!(session.engine(ada).comments(id).is_empty());
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

fn reopen(engine: &mut Engine, id: DocumentId, comment: CommentId) -> Result<(), EngineError> {
    invoke(
        engine,
        ids::COMMENTS_REOPEN,
        CommentRequest::Resolve {
            id,
            comment,
            resolved: false,
        },
    )
}

fn edit_suggestion(
    engine: &mut Engine,
    id: DocumentId,
    comment: CommentId,
    replacement: &str,
    note: &str,
) -> Result<(), EngineError> {
    invoke(
        engine,
        ids::COMMENTS_EDIT,
        CommentRequest::EditSuggestion {
            id,
            comment,
            replacement: replacement.to_owned(),
            text: note.to_owned(),
        },
    )
}

/// The page's metadata file once it holds `wanted`, waiting for the writes still to come.
fn sidecar_holding(session: &mut Session, wanted: &str) -> String {
    let sidecar = session.directory.path().join(".install.adoc.afd");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let written = fs::read_to_string(&sidecar).unwrap_or_default();
        if written.contains(wanted) || Instant::now() >= deadline {
            return written;
        }
        session.settle();
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The last thread `peer` was told of comment `comment`.
fn told(
    session: &Session,
    peer: PeerId,
    comment: CommentId,
) -> adoc_engine::protocol::SharedThread {
    session
        .sent_to(peer)
        .into_iter()
        .rev()
        .find_map(|message| match message {
            Message::CommentThread { thread, .. } if thread.id == comment.0 => Some(*thread),
            _ => None,
        })
        .expect("told of it")
}

#[test]
fn a_guest_rejects_and_reopens_a_suggestion() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    suggest(&mut session.host, host).unwrap();
    session.settle();
    let comment = the_comment(&session.host, host).id;

    let id = guest_doc(&mut session, ada);
    invoke(
        session.engine(ada),
        ids::COMMENTS_REJECT,
        CommentRequest::Reject { id, comment },
    )
    .unwrap();
    session.settle();
    assert_eq!(host_state(&session), SuggestionState::Rejected);
    assert_eq!(guest_state(&mut session, ada), SuggestionState::Rejected);
    assert_eq!(texts(&mut session, &[ada]), PAGE, "the page is untouched");
    let written = sidecar_holding(&mut session, "outcome = \"rejected\"");
    assert!(written.contains("outcome = \"rejected\""), "{written}");

    reopen(session.engine(ada), id, comment).unwrap();
    session.settle();
    assert_eq!(host_state(&session), SuggestionState::Pending);
    assert_eq!(guest_state(&mut session, ada), SuggestionState::Pending);
    assert!(refusals(&session, ada).is_empty());
}

#[test]
fn a_guest_edits_its_own_suggestion_with_a_blank_note() {
    let mut session = session();
    let ada = session.guest(1, true);
    let id = guest_doc(&mut session, ada);
    invoke(
        session.engine(ada),
        ids::COMMENTS_SUGGEST,
        CommentRequest::Suggest {
            id,
            range: {
                let start = PAGE.find(QUOTE).unwrap();
                SourceRange::new(start, start + QUOTE.len())
            },
            original: None,
            replacement: REPLACEMENT.to_owned(),
            text: "Clearer.".to_owned(),
        },
    )
    .unwrap();
    session.settle();
    let host = session.host_doc();
    let comment = the_comment(&session.host, host).id;

    edit_suggestion(
        session.engine(ada),
        id,
        comment,
        "macOS's Gatekeeper",
        "  \n",
    )
    .unwrap();
    session.settle();
    assert!(
        refusals(&session, ada).is_empty(),
        "{:?}",
        refusals(&session, ada)
    );
    let held = the_comment(&session.host, host);
    assert_eq!(held.text, "", "the blank note is empty");
    let suggestion = held.suggestion.unwrap();
    assert_eq!(suggestion.original, QUOTE);
    assert_eq!(suggestion.replacement, "macOS's Gatekeeper");
    assert_eq!(told(&session, ada, comment).text, "", "sent empty");
    let shown = the_comment(session.engine(ada), id);
    assert_eq!(shown.text, "");
    let shown = shown.suggestion.unwrap();
    assert_eq!(shown.replacement, "macOS's Gatekeeper");
    assert_eq!(shown.state, SuggestionState::Pending);
}

#[test]
fn a_guest_refuses_what_the_host_would_in_its_words() {
    let mut session = session();
    let ada = session.guest(1, true);
    let bea = session.guest(2, false);
    let host = session.host_doc();
    suggest(&mut session.host, host).unwrap();
    session.settle();
    let comment = the_comment(&session.host, host).id;

    // A Viewer rejecting the host's suggestion.
    let id = guest_doc(&mut session, bea);
    let refused = invoke(
        session.engine(bea),
        ids::COMMENTS_REJECT,
        CommentRequest::Reject { id, comment },
    );
    assert!(
        matches!(&refused, Err(EngineError::Refused(reason)) if reason == "You cannot reject someone else's suggestion."),
        "{refused:?}"
    );

    // An Editor's own suggestion, once decided, is reopened before its replacement is edited.
    let id = guest_doc(&mut session, ada);
    let start = PAGE.find("unsigned").unwrap();
    invoke(
        session.engine(ada),
        ids::COMMENTS_SUGGEST,
        CommentRequest::Suggest {
            id,
            range: SourceRange::new(start, start + "unsigned".len()),
            original: None,
            replacement: "an unnotarised".to_owned(),
            text: String::new(),
        },
    )
    .unwrap();
    session.settle();
    let own = session
        .host
        .comments(host)
        .into_iter()
        .find(|shown| shown.id != comment)
        .unwrap()
        .id;
    invoke(
        session.engine(ada),
        ids::COMMENTS_REJECT,
        CommentRequest::Reject { id, comment: own },
    )
    .unwrap();
    session.settle();
    let refused = edit_suggestion(session.engine(ada), id, own, "a notarised", "");
    assert!(
        matches!(&refused, Err(EngineError::Refused(reason)) if reason == "Reopen this suggestion first."),
        "{refused:?}"
    );
    assert!(
        session.net.lock().unwrap().to_host.is_empty(),
        "nothing sent"
    );
    assert!(refusals(&session, ada).is_empty() && refusals(&session, bea).is_empty());
}

/// A guest's edit of its suggestion carries the text it saw: the host refuses it when its own
/// text at the range has changed meanwhile, so a replacement is never paired with text its
/// author did not see.
#[test]
fn the_host_refuses_an_edit_of_a_suggestion_made_on_other_text() {
    let mut session = session();
    let ada = session.guest(1, true);
    let id = guest_doc(&mut session, ada);
    suggest(session.engine(ada), id).unwrap();
    session.settle();
    let host = session.host_doc();
    let comment = the_comment(&session.host, host).id;
    let before = told(&session, ada, comment);

    edit_suggestion(session.engine(ada), id, comment, "the signing check", "").unwrap();
    // The host types inside the suggestion's text before the request reaches it.
    let start = session.host_text().find(QUOTE).unwrap();
    type_at(&mut session.host, host, start + "the".len(), " new");
    session.settle();
    assert_eq!(
        refusals(&session, ada),
        ["The text changed while you were suggesting; try again."]
    );
    let held = the_comment(&session.host, host).suggestion.unwrap();
    assert_eq!(
        held.original, QUOTE,
        "still on the text it was suggested on"
    );
    assert_eq!(
        held.replacement, REPLACEMENT,
        "the replacement is unchanged"
    );
    assert_eq!(held.state, SuggestionState::Outdated);
    let after = told(&session, ada, comment);
    assert_eq!(
        after.suggestion.as_ref().map(|shown| shown.uncertain),
        before.suggestion.as_ref().map(|shown| shown.uncertain),
        "its certainty is unchanged"
    );
}

/// The host's accept, undone, stays in its redo history: once the suggestion is reopened and a
/// guest accepts it, the host's Redo must not apply the same suggestion a second time.
#[test]
fn a_redo_after_a_guest_accepts_again_applies_the_suggestion_once() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    suggest(&mut session.host, host).unwrap();
    let comment = the_comment(&session.host, host).id;
    accept(&mut session.host, host, comment).unwrap();
    session.settle();
    undo(&mut session.host, host);
    reopen(&mut session.host, host, comment).unwrap();
    session.settle();
    assert_eq!(texts(&mut session, &[ada]), PAGE);
    assert_eq!(guest_state(&mut session, ada), SuggestionState::Pending);

    let id = guest_doc(&mut session, ada);
    accept(session.engine(ada), id, comment).unwrap();
    session.settle();
    assert_eq!(texts(&mut session, &[ada]), accepted_page());

    // Nothing of the host's is left to redo: its accept was made again by the guest.
    let _ = session
        .host
        .invoke(ids::EDIT_REDO, CommandArgs::Document(host));
    session.settle();
    let text = texts(&mut session, &[ada]);
    assert_eq!(text.matches(REPLACEMENT).count(), 1, "{text}");
    assert_eq!(text, accepted_page());
    assert_eq!(host_state(&session), SuggestionState::Accepted);
}

/// Where `engine` shows the one comment on page `id`: detached, and the text it is on.
fn placed(engine: &Engine, id: DocumentId) -> (bool, String) {
    let comment = the_comment(engine, id);
    (comment.detached, comment.quote)
}

/// The host's Undo and Redo of its own accept move the suggestion's range over text made again
/// each time. A guest's copy of the deleted text is gone, so it is told the range again rather
/// than left to place it by text only the host still keeps.
#[test]
fn a_guest_follows_the_range_through_the_hosts_undo_and_redo_of_an_accept() {
    let mut session = session();
    let ada = session.guest(1, true);
    let host = session.host_doc();
    suggest(&mut session.host, host).unwrap();
    let comment = the_comment(&session.host, host).id;
    accept(&mut session.host, host, comment).unwrap();
    session.settle();
    let id = guest_doc(&mut session, ada);
    undo(&mut session.host, host);
    session.settle();
    assert_eq!(placed(session.engine(ada), id), (false, QUOTE.to_owned()));

    // Redone and undone again before the guest hears of the redo.
    session
        .host
        .invoke(ids::EDIT_REDO, CommandArgs::Document(host))
        .unwrap();
    undo(&mut session.host, host);
    session.settle();
    assert_eq!(texts(&mut session, &[ada]), PAGE);
    assert_eq!(placed(&session.host, host), (false, QUOTE.to_owned()));
    assert_eq!(placed(session.engine(ada), id), (false, QUOTE.to_owned()));
}
