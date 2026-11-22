//! Co-authorship of shared edits (#761, `docs/prd/PRD-AdocFoundry_CoAuthors.md`): the host
//! records which guests changed each shared file, keeps it until the file is committed, and
//! never tells anyone a guest's address.

#![cfg(feature = "collab")]

mod collab_common;

use std::{fs, path::PathBuf};

use adoc_collab::{ByteEdit, Replica, ReplicaId};
use adoc_engine::{protocol::Message, CoAuthor, DiskResolution, Engine, Settings};
use collab_common::*;

const A: &str = "= A\n\nFirst page.\n";
const B: &str = "= B\n\nSecond page.\n";

fn session() -> Session {
    Session::new(&[("a.adoc", A), ("b.adoc", B)])
}

fn path(session: &Session, name: &str) -> PathBuf {
    session.directory.path().join(name)
}

fn guest1() -> CoAuthor {
    CoAuthor {
        name: "guest 1".to_owned(),
        email: "guest1@example.org".to_owned(),
    }
}

/// The guest opens file 2 (`b.adoc`) as well.
fn open_b(session: &mut Session, peer: adoc_engine::protocol::PeerId) {
    session.engine(peer).open_session_file(2).unwrap();
    session.settle();
}

fn guest_types_in(
    session: &mut Session,
    peer: adoc_engine::protocol::PeerId,
    file: u64,
    typed: &str,
) {
    let engine = session.engine(peer);
    let id = engine.session_document(file).unwrap();
    type_at(engine, id, 0, typed);
    session.settle();
}

#[test]
fn a_guests_change_is_recorded() {
    let mut session = session();
    let ada = session.guest(1, true);
    open_b(&mut session, ada);
    guest_types_in(&mut session, ada, 2, "Typed. ");
    let (a, b) = (path(&session, "a.adoc"), path(&session, "b.adoc"));
    assert_eq!(session.host.coauthors_of(&[b.clone()]), vec![guest1()]);
    assert!(session.host.coauthors_of(&[a.clone()]).is_empty());
    let host_a = session.host.session_document(1).unwrap();
    type_at(&mut session.host, host_a, 0, "Host. ");
    session.settle();
    assert!(session.host.coauthors_of(&[a]).is_empty());
}

#[test]
fn a_participant_who_only_read_is_never_credited() {
    let mut session = session();
    let ada = session.guest(1, true);
    let bea = session.guest(2, false);
    for peer in [ada, bea] {
        open_b(&mut session, peer);
        let engine = session.engine(peer);
        let id = engine.session_document(2).unwrap();
        engine.share_selection(id, 2, 4);
    }
    session.settle();
    // An update holding an insertion and its deletion: it changes nothing.
    let (epoch, client, snapshot) = session
        .sent_to(ada)
        .into_iter()
        .rev()
        .find_map(|message| match message {
            Message::Snapshot {
                file: 2,
                epoch,
                client,
                update,
            } => Some((epoch, client, update)),
            _ => None,
        })
        .unwrap();
    let mut replica = Replica::from_snapshot(&snapshot, ReplicaId::new(client).unwrap()).unwrap();
    let before = replica.state_vector();
    replica.apply_local(&[ByteEdit::new(0, 0, "x")]).unwrap();
    replica.apply_local(&[ByteEdit::new(0, 1, "")]).unwrap();
    let update = replica.missing_since(&before).unwrap();
    session.forge(
        ada,
        Message::Update {
            file: 2,
            epoch,
            seq: 1,
            base: client,
            update,
        },
    );
    session.settle();
    assert!(session.removed().is_empty(), "{:?}", session.removed());
    let paths = [path(&session, "a.adoc"), path(&session, "b.adoc")];
    assert!(session.host.coauthors_of(&paths).is_empty());

    // The same way, a change that stays is credited.
    let before = replica.state_vector();
    replica.apply_local(&[ByteEdit::new(0, 0, "y")]).unwrap();
    let update = replica.missing_since(&before).unwrap();
    session.forge(
        ada,
        Message::Update {
            file: 2,
            epoch,
            seq: 2,
            base: client,
            update,
        },
    );
    session.settle();
    assert_eq!(session.host.coauthors_of(&paths), vec![guest1()]);
}

#[test]
fn a_guest_without_an_address_is_not_recorded() {
    let mut session = session();
    let ada = session.guest_with_email(1, true, None);
    guest_types_in(&mut session, ada, 1, "Typed. ");
    assert!(session
        .host
        .coauthors_of(&[path(&session, "a.adoc")])
        .is_empty());
}

#[test]
fn credit_survives_a_reconnection_and_a_new_epoch() {
    let mut session = session();
    let ada = session.guest(1, true);
    open_b(&mut session, ada);
    session.drop_link(ada, 0);
    session.settle();
    session.resume(ada);
    session.settle();
    guest_types_in(&mut session, ada, 1, "After a reconnection. ");
    let (a, b) = (path(&session, "a.adoc"), path(&session, "b.adoc"));
    assert_eq!(session.host.coauthors_of(&[a.clone()]), vec![guest1()]);

    // The host starts b again from the disk, in a new epoch.
    fs::write(&b, "= B on disk\n").unwrap();
    let host_b = session.host.session_document(2).unwrap();
    assert!(session.host.save(host_b).is_err());
    session
        .host
        .resolve_disk_conflict(host_b, DiskResolution::ReloadDisk)
        .unwrap();
    session.settle();
    assert_eq!(session.host.shared_file(host_b).unwrap().1, 2);
    guest_types_in(&mut session, ada, 2, "In a new epoch. ");
    assert_eq!(session.host.coauthors_of(&[b.clone()]), vec![guest1()]);
    assert_eq!(session.host.coauthors_of(&[a, b]), vec![guest1()]);
}

#[test]
fn a_guests_email_reaches_only_the_host() {
    let mut session = session();
    let ada = session.guest(1, true);
    let bea = session.guest(2, true);
    let id = session.engine(ada).session_document(1).unwrap();
    session
        .engine(ada)
        .invoke(
            adoc_engine::commands::ids::COMMENTS_ADD,
            adoc_engine::CommandArgs::Comment(adoc_engine::CommentRequest::Add {
                id,
                range: adoc_core::SourceRange::new(2, 3),
                text: "Why?".to_owned(),
            }),
        )
        .unwrap();
    guest_types_in(&mut session, ada, 1, "Typed. ");
    guest_types_in(&mut session, bea, 1, "Also. ");
    assert_eq!(session.host.comments(session.host_doc()).len(), 1);
    for peer in [ada, bea] {
        for message in session.sent_to(peer) {
            let frame = String::from_utf8_lossy(&message.encode().unwrap()).into_owned();
            for email in ["guest1@example.org", "guest2@example.org"] {
                assert!(
                    !frame.contains(email),
                    "{peer:?} was told {email}: {message:?}"
                );
            }
        }
    }
}

#[test]
fn credit_outlives_the_session_and_the_engine() {
    let state = tempfile::tempdir().unwrap();
    let settings = || {
        let mut settings = Settings::load(None, None).unwrap();
        settings.git.enabled = false;
        settings.with_state_dir(Some(state.path().to_path_buf()))
    };
    let mut session = Session::with_host(&[("a.adoc", A), ("b.adoc", B)], Engine::new(settings()));
    let ada = session.guest(1, true);
    guest_types_in(&mut session, ada, 1, "Typed. ");
    session.host.end_session();
    // Dropping the engine waits for its writes.
    drop(std::mem::replace(
        &mut session.host,
        Engine::new(settings()),
    ));
    let a = path(&session, "a.adoc");
    assert_eq!(session.host.coauthors_of(&[a]), vec![guest1()]);
}
