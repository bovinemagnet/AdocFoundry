//! Following a participant (#767): on demand, one participant's active page follows another's
//! from shared file to shared file, until it edits, chooses another page itself, or stops.
//! Every participant tells the others which shared file it is in, never a path: a host on a
//! page it does not share is "elsewhere".

#![cfg(feature = "collab")]

mod collab_common;

use adoc_engine::{
    commands::ids,
    protocol::{Message, PeerId},
    CommandArgs, DocumentId, Engine,
};
use collab_common::*;

const PAGES: [(&str, &str); 3] = [
    ("one.adoc", "= One\n"),
    ("two.adoc", "= Two\n"),
    ("three.adoc", "= Three\n"),
];

/// The page of `file` in `peer`'s engine (the host's for `None`), opening it first if need
/// be: a guest asks the host for it, the host opens its own file.
fn page(session: &mut Session, peer: Option<PeerId>, file: u64) -> DocumentId {
    match peer {
        Some(peer) => {
            if session.engine(peer).session_document(file).is_none() {
                session.engine(peer).open_session_file(file).unwrap();
                session.settle();
            }
            session.engine(peer).session_document(file).unwrap()
        }
        None => match session.host.session_document(file) {
            Some(id) => id,
            None => {
                let path = session.directory.path().join(PAGES[file as usize - 1].0);
                session.host.open(&path).unwrap()
            }
        },
    }
}

/// Shows `file` in `peer`'s engine (the host's for `None`), as choosing its tab does.
fn go_to(session: &mut Session, peer: Option<PeerId>, file: u64) {
    let id = page(session, peer, file);
    match peer {
        Some(peer) => session.engine(peer).set_active(id),
        None => session.host.set_active(id),
    }
    session.settle();
}

/// The shared file `engine` shows, by its page's name.
fn shown(engine: &Engine) -> Option<u64> {
    let path = &engine.document(engine.active()?)?.path;
    (1..=3).find(|file| path.ends_with(PAGES[*file as usize - 1].0))
}

fn follow(engine: &mut Engine, peer: PeerId) {
    engine
        .invoke(
            ids::COLLAB_FOLLOW,
            CommandArgs::Custom(serde_json::json!({ "peer": peer.0 })),
        )
        .unwrap();
}

#[test]
fn the_host_follows_a_guest_from_page_to_page() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, Some(ada), 1);
    follow(&mut session.host, ada);
    session.settle();
    assert_eq!(session.host.following(), Some(ada));
    assert_eq!(shown(&session.host), Some(1));

    go_to(&mut session, Some(ada), 2);
    assert_eq!(
        shown(&session.host),
        Some(2),
        "the host followed to page two"
    );
    go_to(&mut session, Some(ada), 1);
    assert_eq!(shown(&session.host), Some(1), "and back");
    // A page opened for following is in the host's tabs, not its background.
    let two = session.host.session_document(2).unwrap();
    assert!(session.host.has_tab(two));
}

#[test]
fn a_guest_follows_another_to_a_page_it_had_not_opened() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    let bea = session.guest(2, true);
    go_to(&mut session, Some(ada), 1);
    follow(session.engine(bea), ada);
    session.settle();
    assert_eq!(session.engine(bea).session_document(3), None);

    go_to(&mut session, Some(ada), 3);
    assert_eq!(
        shown(session.engine(bea)),
        Some(3),
        "the host sent page three and it was shown"
    );
    assert_eq!(session.engine(bea).following(), Some(ada));
}

#[test]
fn a_guest_follows_the_host() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, None, 2);
    follow(session.engine(ada), PeerId::HOST);
    session.settle();
    assert_eq!(shown(session.engine(ada)), Some(2));
    go_to(&mut session, None, 3);
    assert_eq!(shown(session.engine(ada)), Some(3));
}

#[test]
fn an_edit_of_ones_own_stops_following_and_others_do_not() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, Some(ada), 1);
    follow(&mut session.host, ada);
    session.settle();

    // The leader typing moves nothing and stops nothing.
    guest_types(&mut session, ada, 0, "a");
    session.settle();
    assert_eq!(session.host.following(), Some(ada));

    let one = session.host.session_document(1).unwrap();
    type_at(&mut session.host, one, 0, "h");
    session.settle();
    assert_eq!(
        session.host.following(),
        None,
        "the follower's edit stopped it"
    );
    go_to(&mut session, Some(ada), 2);
    assert_eq!(shown(&session.host), Some(1), "and it no longer follows");
}

#[test]
fn choosing_another_page_stops_following() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, Some(ada), 1);
    follow(&mut session.host, ada);
    session.settle();
    go_to(&mut session, Some(ada), 2);
    assert_eq!(
        session.host.following(),
        Some(ada),
        "its own moves do not stop it"
    );

    go_to(&mut session, None, 3);
    assert_eq!(session.host.following(), None);
    go_to(&mut session, Some(ada), 1);
    assert_eq!(shown(&session.host), Some(3));
}

#[test]
fn stop_following_stops() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, Some(ada), 1);
    follow(&mut session.host, ada);
    session.settle();
    session
        .host
        .invoke(ids::COLLAB_STOP_FOLLOWING, CommandArgs::None)
        .unwrap();
    go_to(&mut session, Some(ada), 2);
    assert_eq!(session.host.following(), None);
    assert_eq!(shown(&session.host), Some(1));
}

#[test]
fn following_a_follower_leads_to_whom_it_follows() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    let bea = session.guest(2, true);
    go_to(&mut session, None, 2);
    follow(session.engine(ada), PeerId::HOST);
    follow(session.engine(bea), ada);
    session.settle();
    go_to(&mut session, None, 3);
    assert_eq!(shown(session.engine(ada)), Some(3));
    assert_eq!(shown(session.engine(bea)), Some(3));
}

/// Participants following each other in a ring must not chase each other for ever: nobody
/// leads the ring, so nobody moves.
#[test]
fn following_in_a_ring_settles() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    let bea = session.guest(2, true);
    go_to(&mut session, None, 1);
    go_to(&mut session, Some(ada), 2);
    go_to(&mut session, Some(bea), 3);
    follow(&mut session.host, ada);
    follow(session.engine(ada), bea);
    follow(session.engine(bea), PeerId::HOST);
    // `settle` fails if the session never comes to rest.
    session.settle();
    session.settle();
    let places = (
        shown(&session.host),
        shown(session.engine(ada)),
        shown(session.engine(bea)),
    );
    session.settle();
    assert_eq!(
        places,
        (
            shown(&session.host),
            shown(session.engine(ada)),
            shown(session.engine(bea))
        )
    );

    // Two following each other, the same.
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, None, 1);
    go_to(&mut session, Some(ada), 2);
    follow(&mut session.host, ada);
    follow(session.engine(ada), PeerId::HOST);
    session.settle();
    session.settle();
}

/// The host on a page it does not share is elsewhere: guests are told it is in no shared
/// file, and nothing sent names the page.
#[test]
fn a_host_on_a_page_it_does_not_share_is_elsewhere() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, None, 2);
    follow(session.engine(ada), PeerId::HOST);
    session.settle();
    let private = session.directory.path().join("private-notes.adoc");
    std::fs::write(&private, "= Private notes\n").unwrap();
    let private = session.host.open(&private).unwrap();
    session.host.set_active(private);
    session.settle();

    let host = session
        .engine(ada)
        .participants()
        .into_iter()
        .find(|participant| participant.peer == PeerId::HOST)
        .unwrap();
    assert_eq!(host.place, None);
    assert_eq!(shown(session.engine(ada)), Some(2), "the guest stays");
    let sent = session.sent_to(ada);
    assert!(sent
        .iter()
        .all(|message| !format!("{message:?}").contains("private")));
    assert!(sent.iter().any(|message| matches!(
        message,
        Message::Roster { members, .. } if members[0].file.is_none()
    )));
}

#[test]
fn a_guest_tells_where_it_is_again_after_resuming() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    go_to(&mut session, Some(ada), 2);
    let place = |session: &Session| {
        session
            .host
            .participants()
            .into_iter()
            .find(|participant| participant.peer == ada)
            .and_then(|participant| participant.place)
    };
    assert_eq!(place(&session), Some(2));
    session.drop_link(ada, 0);
    session.settle();
    let three = page(&mut session, Some(ada), 1);
    session.engine(ada).set_active(three);
    session.resume(ada);
    session.settle();
    assert_eq!(place(&session), Some(1));
}

#[test]
fn following_ends_when_the_leader_leaves() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    let bea = session.guest(2, true);
    go_to(&mut session, Some(ada), 1);
    follow(session.engine(bea), ada);
    session.settle();
    session.host.remove_peer(ada);
    session.settle();
    assert_eq!(session.engine(bea).following(), None);
}

#[test]
fn nobody_follows_themselves_or_a_stranger() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    for peer in [ada, PeerId(42)] {
        assert!(session
            .engine(ada)
            .invoke(
                ids::COLLAB_FOLLOW,
                CommandArgs::Custom(serde_json::json!({ "peer": peer.0 })),
            )
            .is_err());
    }
    assert!(session
        .host
        .invoke(
            ids::COLLAB_FOLLOW,
            CommandArgs::Custom(serde_json::json!({ "peer": 0 })),
        )
        .is_err());
}

/// A guest naming a file the host does not share as where it is breaks the protocol.
#[test]
fn a_focus_on_a_file_not_shared_closes_the_connection() {
    let mut session = Session::new(&PAGES);
    let ada = session.guest(1, true);
    session.forge(
        ada,
        Message::Focus {
            file: Some(99),
            following: None,
        },
    );
    session.settle();
    assert_eq!(
        session.removed(),
        [(ada, adoc_engine::protocol::ErrorCode::Protocol)]
    );
}
