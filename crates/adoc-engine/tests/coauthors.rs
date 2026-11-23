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
    assert_eq!(
        session.host.coauthors_of(std::slice::from_ref(&b)),
        vec![guest1()]
    );
    assert!(session
        .host
        .coauthors_of(std::slice::from_ref(&a))
        .is_empty());
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
    assert_eq!(
        session.host.coauthors_of(std::slice::from_ref(&a)),
        vec![guest1()]
    );

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
    assert_eq!(
        session.host.coauthors_of(std::slice::from_ref(&b)),
        vec![guest1()]
    );
    assert_eq!(session.host.coauthors_of(&[a, b]), vec![guest1()]);
}

/// The host sees the address a knocking guest gives before admitting it, so a wrong one can be
/// spotted; only while the guest waits, and never on a guest's engine.
#[test]
fn the_host_sees_a_knocking_guests_address() {
    let mut session = session();
    let ada = session.knock_with_email(1, collab_common::engine(), Some("ada@example.com"));
    let bea = session.knock_with_email(2, collab_common::engine(), None);
    let cy = session.knock_with_email(3, collab_common::engine(), Some("cy@example.com"));
    assert_eq!(
        session.host.session_knock_email(ada),
        Some("ada@example.com")
    );
    assert_eq!(session.host.session_knock_email(bea), None);
    assert_eq!(session.host.session_knock_email(cy), Some("cy@example.com"));
    assert_eq!(session.engine(ada).session_knock_email(ada), None);

    session.host.admit_peer(ada).unwrap();
    session.host.remove_peer(cy);
    session.settle();
    assert_eq!(session.host.session_knock_email(ada), None);
    assert_eq!(session.host.session_knock_email(cy), None);
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

/// Committing: the commit dialog's co-authors, the trailers, and what a commit forgets.
#[cfg(feature = "git")]
mod committing {
    use std::{path::PathBuf, time::Duration};

    use adoc_core::SourceRange;
    use adoc_engine::{commands::ids, CoAuthor, CommandArgs, CommandOutcome, Engine};
    use adoc_git::testing::TestRepo;

    use super::{guest1, guest_types_in, open_b, A, B};
    use crate::collab_common::*;

    const C: &str = "= C\n\nThird page.\n";

    fn session(linked: bool) -> Session {
        Session::in_repo(&[("a.adoc", A), ("b.adoc", B), ("c.adoc", C)], linked)
    }

    fn page(session: &Session, name: &str) -> PathBuf {
        session.root().join(name)
    }

    fn repo(session: &Session) -> TestRepo {
        TestRepo::open(&session.directory.path().join("tree"))
    }

    fn save_all(engine: &mut Engine) {
        let ids: Vec<_> = engine.documents().map(|document| document.id).collect();
        for id in ids {
            if engine.document(id).is_some_and(|document| document.dirty) {
                engine.save(id).unwrap();
            }
        }
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
    }

    fn commit(
        engine: &mut Engine,
        message: &str,
        paths: &[PathBuf],
        coauthors: &[CoAuthor],
    ) -> Result<CommandOutcome, adoc_engine::EngineError> {
        let coauthors: Vec<_> = coauthors
            .iter()
            .map(|author| serde_json::json!({ "name": author.name, "email": author.email }))
            .collect();
        let paths: Vec<_> = paths.iter().map(|path| path.to_str().unwrap()).collect();
        engine.invoke(
            ids::GIT_COMMIT,
            CommandArgs::Custom(serde_json::json!({
                "message": message,
                "paths": paths,
                "coauthors": coauthors,
            })),
        )
    }

    /// The host has every file open with the guest: a, b and c are files 1, 2 and 3.
    fn editing_everywhere(session: &mut Session) -> adoc_engine::protocol::PeerId {
        let ada = session.guest(1, true);
        open_b(session, ada);
        session.engine(ada).open_session_file(3).unwrap();
        session.settle();
        ada
    }

    #[test]
    fn a_commit_credits_only_those_who_changed_its_files() {
        let mut session = session(false);
        let ada = editing_everywhere(&mut session);
        for file in [1, 3] {
            let id = session.host.session_document(file).unwrap();
            type_at(&mut session.host, id, 0, "Host. ");
        }
        session.settle();
        guest_types_in(&mut session, ada, 2, "Guest. ");
        guest_types_in(&mut session, ada, 3, "Guest. ");
        save_all(&mut session.host);
        let (a, b, c) = (
            page(&session, "a.adoc"),
            page(&session, "b.adoc"),
            page(&session, "c.adoc"),
        );
        assert!(session.host.coauthors_for_commit(&[a]).is_empty());
        assert_eq!(
            session.host.coauthors_for_commit(std::slice::from_ref(&b)),
            vec![guest1()]
        );
        assert_eq!(session.host.coauthors_for_commit(&[c]), vec![guest1()]);
        let offered = session.host.coauthors_for_commit(std::slice::from_ref(&b));
        commit(&mut session.host, "docs: b", &[b], &offered).unwrap();
        assert_eq!(
            repo(&session).head_message(),
            "docs: b\n\nCo-authored-by: guest 1 <guest1@example.org>\n"
        );
    }

    #[test]
    fn committed_files_start_again() {
        let mut session = session(false);
        let ada = editing_everywhere(&mut session);
        guest_types_in(&mut session, ada, 2, "Guest. ");
        guest_types_in(&mut session, ada, 3, "Guest. ");
        save_all(&mut session.host);
        let (b, c) = (page(&session, "b.adoc"), page(&session, "c.adoc"));
        // A commit refused forgets nothing.
        assert!(commit(
            &mut session.host,
            "  ",
            std::slice::from_ref(&b),
            &[guest1()]
        )
        .is_err());
        assert_eq!(
            session.host.coauthors_for_commit(std::slice::from_ref(&b)),
            vec![guest1()]
        );
        commit(
            &mut session.host,
            "docs: b",
            std::slice::from_ref(&b),
            &[guest1()],
        )
        .unwrap();
        assert!(session.host.coauthors_for_commit(&[b]).is_empty());
        assert_eq!(session.host.coauthors_for_commit(&[c]), vec![guest1()]);
    }

    #[test]
    fn an_edit_after_staging_is_credited_next_time() {
        let mut session = session(false);
        let ada = editing_everywhere(&mut session);
        guest_types_in(&mut session, ada, 2, "Before staging. ");
        save_all(&mut session.host);
        let b = page(&session, "b.adoc");
        session
            .host
            .invoke(ids::GIT_STAGE, CommandArgs::Path(b.clone()))
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        guest_types_in(&mut session, ada, 2, "After staging. ");
        save_all(&mut session.host);
        // The index is committed: what was staged, without the later edit.
        assert_eq!(session.host.coauthors_for_commit(&[]), vec![guest1()]);
        commit(&mut session.host, "docs: b", &[], &[guest1()]).unwrap();
        assert!(repo(&session).head_message().contains("guest1@example.org"));
        assert_eq!(session.host.coauthors_for_commit(&[b]), vec![guest1()]);
    }

    /// What the commit dialog offered, as it passes it to `git.commit`.
    fn commit_offering(
        engine: &mut Engine,
        message: &str,
        paths: &[PathBuf],
        offered: &[CoAuthor],
    ) -> Result<CommandOutcome, adoc_engine::EngineError> {
        let listed: Vec<_> = offered
            .iter()
            .map(|author| serde_json::json!({ "name": author.name, "email": author.email }))
            .collect();
        let paths: Vec<_> = paths.iter().map(|path| path.to_str().unwrap()).collect();
        engine.invoke(
            ids::GIT_COMMIT,
            CommandArgs::Custom(serde_json::json!({
                "message": message,
                "paths": paths,
                "coauthors": listed,
                "offered": listed,
            })),
        )
    }

    #[test]
    fn a_guest_who_first_edits_while_the_dialog_is_open_is_kept() {
        let mut session = session(false);
        let ada = editing_everywhere(&mut session);
        let bea = session.guest(2, true);
        open_b(&mut session, bea);
        guest_types_in(&mut session, ada, 2, "Ada. ");
        let b = page(&session, "b.adoc");
        let offered = session.host.coauthors_for_commit(std::slice::from_ref(&b));
        assert_eq!(offered, vec![guest1()]);
        // While the host writes the message, someone else edits for the first time; the
        // commit saves the page again, taking their edit too, but they were never offered.
        guest_types_in(&mut session, bea, 2, "Bea. ");
        commit_offering(
            &mut session.host,
            "docs: b",
            std::slice::from_ref(&b),
            &offered,
        )
        .unwrap();
        assert_eq!(
            session.host.coauthors_for_commit(&[b]),
            vec![CoAuthor {
                name: "guest 2".to_owned(),
                email: "guest2@example.org".to_owned(),
            }]
        );
    }

    #[test]
    fn staging_takes_what_was_saved_not_what_is_unsaved() {
        let mut session = session(false);
        let ada = editing_everywhere(&mut session);
        guest_types_in(&mut session, ada, 2, "Saved. ");
        save_all(&mut session.host);
        std::thread::sleep(Duration::from_millis(5));
        // An edit the host has not saved: staging takes the file, without it.
        guest_types_in(&mut session, ada, 2, "Unsaved. ");
        let b = page(&session, "b.adoc");
        let host_b = session.host.session_document(2).unwrap();
        assert!(session.host.document(host_b).unwrap().dirty);
        std::thread::sleep(Duration::from_millis(5));
        session
            .host
            .invoke(ids::GIT_STAGE, CommandArgs::Path(b.clone()))
            .unwrap();
        assert!(session.host.wait_for_workers(Duration::from_secs(10)));
        let offered = session.host.coauthors_for_commit(&[]);
        assert_eq!(offered, vec![guest1()]);
        commit_offering(&mut session.host, "docs: b", &[], &offered).unwrap();
        assert!(!repo(&session).head_message().is_empty());
        assert_eq!(session.host.coauthors_for_commit(&[b]), vec![guest1()]);
    }

    #[test]
    fn the_committer_is_never_a_co_author() {
        let mut session = session(false);
        let host = session.guest_with_email(1, true, Some("HOST@Example.org"));
        guest_types_in(&mut session, host, 1, "Typed. ");
        save_all(&mut session.host);
        let a = page(&session, "a.adoc");
        assert_eq!(session.host.coauthors_of(std::slice::from_ref(&a)).len(), 1);
        assert!(session.host.coauthors_for_commit(&[a]).is_empty());
    }

    #[test]
    fn a_renamed_file_keeps_its_credit() {
        let mut session = session(false);
        let ada = editing_everywhere(&mut session);
        guest_types_in(&mut session, ada, 2, "Guest. ");
        save_all(&mut session.host);
        session.host.end_session();
        let (b, d) = (page(&session, "b.adoc"), page(&session, "d.adoc"));
        let id = session.host.open(&b).unwrap();
        let CommandOutcome::Preview(preview) = session
            .host
            .invoke(
                ids::PAGE_RENAME,
                CommandArgs::Rename {
                    id,
                    new_path: d.clone(),
                },
            )
            .unwrap()
        else {
            panic!("a rename previews");
        };
        session
            .host
            .invoke(
                ids::WORKSPACE_APPLY_PREVIEW,
                CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
            )
            .unwrap();
        assert!(d.exists());
        assert_eq!(session.host.coauthors_for_commit(&[d]), vec![guest1()]);
        assert!(session.host.coauthors_of(&[b]).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn credit_follows_a_page_shared_through_a_link() {
        let mut session = session(true);
        let ada = session.guest(1, true);
        guest_types_in(&mut session, ada, 1, "Typed. ");
        save_all(&mut session.host);
        // Staged through the link, so the status is read again (a watcher may report the
        // file by its real path, which is no root's).
        session
            .host
            .invoke(ids::GIT_STAGE, CommandArgs::Path(page(&session, "a.adoc")))
            .unwrap();
        assert!(session.host.wait_for_workers(Duration::from_secs(10)));
        let changed: Vec<PathBuf> = session
            .host
            .changed_writer_files()
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert_eq!(changed.len(), 1, "{changed:?}");
        assert_eq!(session.host.coauthors_for_commit(&changed), vec![guest1()]);
        let real = std::fs::canonicalize(page(&session, "a.adoc")).unwrap();
        assert_eq!(session.host.coauthors_for_commit(&[real]), vec![guest1()]);
    }

    #[test]
    fn a_file_back_to_its_committed_text_is_pruned() {
        let mut session = session(false);
        let ada = editing_everywhere(&mut session);
        guest_types_in(&mut session, ada, 2, "Guest. ");
        save_all(&mut session.host);
        let b = page(&session, "b.adoc");
        let id = session.host.session_document(2).unwrap();
        let length = session.host.document(id).unwrap().text_string().len();
        session
            .host
            .invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(0, length),
                    text: B.to_owned(),
                },
            )
            .unwrap();
        save_all(&mut session.host);
        session.settle();
        session.host.prune_coauthors();
        assert!(session
            .host
            .coauthors_of(std::slice::from_ref(&b))
            .is_empty());
        assert!(session.host.coauthors_for_commit(&[b]).is_empty());
    }
}
