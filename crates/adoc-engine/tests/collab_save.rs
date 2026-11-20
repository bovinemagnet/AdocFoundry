//! Saving shared files (COLLAB 5/8, #692; design §10, PRD §§5.4, 9): only the host writes, a
//! save names the exact text it wrote, a change made on disk outside the session is a conflict
//! the host resolves, and nothing a guest holds is ever written on its side.

#![cfg(feature = "collab")]

mod collab_common;

use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use adoc_engine::{
    protocol::{ErrorCode, Message, PeerId, SaveOutcome},
    revision_of, AutoSave, DiskResolution, DocumentId, EncodedUpdate, Engine, EngineError,
    EngineEvent, ManualClock, Settings, SharedSaveState, AUTO_SAVE_DELAY,
};
use collab_common::*;

fn page(session: &Session) -> PathBuf {
    session.directory.path().join("page.adoc")
}

fn on_disk(session: &Session) -> String {
    fs::read_to_string(page(session)).unwrap()
}

fn guest_doc(session: &mut Session, peer: PeerId) -> DocumentId {
    session.engine(peer).session_document(1).unwrap()
}

fn host_state(session: &Session) -> Option<SharedSaveState> {
    session.host.shared_save_state(session.host_doc())
}

fn guest_state(session: &mut Session, peer: PeerId) -> Option<SharedSaveState> {
    let id = guest_doc(session, peer);
    session.engine(peer).shared_save_state(id)
}

/// Asks the host to save from `peer`, and returns the answer.
fn request_save(session: &mut Session, peer: PeerId) -> SaveOutcome {
    let id = guest_doc(session, peer);
    let request = session.engine(peer).request_shared_save(id).unwrap();
    session.settle();
    let (answered, outcome) = session.engine(peer).shared_save_result(id).unwrap();
    assert_eq!(answered, request);
    outcome
}

fn saved(text: &str) -> SharedSaveState {
    SharedSaveState::Saved {
        revision: revision_of(text),
    }
}

fn saves(events: &std::sync::mpsc::Receiver<EngineEvent>) -> usize {
    events
        .try_iter()
        .filter(|event| matches!(event, EngineEvent::DocumentSaved(_)))
        .count()
}

/// A revision is the saved text's hash, not the replica's state vector: a deletion leaves the
/// state vector as it was, so two texts would share a revision.
#[test]
fn a_deletion_leaves_the_state_vector_as_it_was() {
    let mut replica =
        adoc_collab::Replica::new("abc", adoc_collab::ReplicaId::new(1).unwrap()).unwrap();
    replica
        .apply_local(&[adoc_collab::ByteEdit::new(3, 3, "d")])
        .unwrap();
    let before = replica.state_vector();
    replica
        .apply_local(&[adoc_collab::ByteEdit::new(0, 1, "")])
        .unwrap();
    assert_eq!(replica.text(), "bcd");
    assert_eq!(replica.state_vector(), before);
    assert_ne!(revision_of("abcd"), revision_of("bcd"));
}

#[test]
fn collab_save_writes_converged_text() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    assert_eq!(host_state(&session), Some(saved("= Page\n")));
    assert_eq!(guest_state(&mut session, ada), Some(saved("= Page\n")));

    // Interleaved edits, then a guest asks the host to save.
    guest_types(&mut session, ada, 7, "Guest.\n");
    let id = session.host_doc();
    type_at(&mut session.host, id, 0, "// ");
    session.settle();
    assert_eq!(host_state(&session), Some(SharedSaveState::Unsaved));
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Unsaved)
    );
    let events = session.host.subscribe();
    let text = session.host_text();
    assert_eq!(
        request_save(&mut session, ada),
        SaveOutcome::Saved {
            revision: revision_of(&text)
        }
    );
    assert_eq!(on_disk(&session), text);
    assert_eq!(session.guest_text(ada), text);
    assert_eq!(saves(&events), 1);
    assert_eq!(host_state(&session), Some(saved(&text)));
    assert_eq!(guest_state(&mut session, ada), Some(saved(&text)));

    // Asking again writes nothing and names the same revision.
    assert_eq!(
        request_save(&mut session, ada),
        SaveOutcome::Saved {
            revision: revision_of(&text)
        }
    );
    assert_eq!(saves(&events), 0);

    // An edit since makes it unsaved for both, and the guest never wrote anything itself.
    guest_types(&mut session, ada, 0, "x");
    session.settle();
    assert_eq!(host_state(&session), Some(SharedSaveState::Unsaved));
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Unsaved)
    );
    let guest = guest_doc(&mut session, ada);
    assert!(matches!(
        session.engine(ada).save(guest),
        Err(EngineError::Refused(_))
    ));
    assert_eq!(on_disk(&session), text);
}

#[test]
fn an_edit_during_a_save_is_not_claimed_saved() {
    let clock = ManualClock::new();
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    settings.editor.auto_save = AutoSave::AfterDelay;
    let host = Engine::with_clock(settings, Arc::new(clock.clone()));
    let mut session = Session::with_host(&[("page.adoc", "= Page\n")], host);
    let ada = session.guest(1, true);
    let id = session.host_doc();
    type_at(&mut session.host, id, 7, "Host.\n");
    let snapshot = session.host_text();

    // The auto-save takes its snapshot; the write is held while a guest's edit arrives.
    let hold = session.host.hold_writer();
    clock.advance(AUTO_SAVE_DELAY);
    session.host.pump();
    guest_types(&mut session, ada, 0, "Late. ");
    for _ in 0..20 {
        session.engine(ada).pump();
        session.host.pump();
    }
    assert!(session.host_text().starts_with("Late. "));
    drop(hold);
    let deadline = Instant::now() + Duration::from_secs(10);
    while on_disk(&session) != snapshot {
        assert!(Instant::now() < deadline, "the held save was never written");
        session.host.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    session.settle();
    // The file holds the snapshot; the edit that came during the write is not claimed saved.
    assert_eq!(on_disk(&session), snapshot);
    assert_eq!(host_state(&session), Some(SharedSaveState::Unsaved));
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Unsaved)
    );
    // Saved again, it is.
    let text = session.host_text();
    assert_eq!(
        request_save(&mut session, ada),
        SaveOutcome::Saved {
            revision: revision_of(&text)
        }
    );
    assert_eq!(on_disk(&session), text);
}

#[test]
fn collab_external_change_is_a_conflict() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 7, "Shared.\n");
    session.settle();
    fs::write(page(&session), "= Outside\n").unwrap();

    // The host's save finds the file changed: nothing is written, and the file is in conflict.
    let id = session.host_doc();
    assert!(matches!(
        session.host.save(id),
        Err(EngineError::SaveConflict { .. })
    ));
    session.settle();
    assert_eq!(on_disk(&session), "= Outside\n");
    assert_eq!(host_state(&session), Some(SharedSaveState::Conflict));
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Conflict)
    );
    // Neither a guest's request nor the host's own save writes over it.
    assert_eq!(request_save(&mut session, ada), SaveOutcome::Conflict);
    assert!(session.host.save(id).is_err());
    assert_eq!(on_disk(&session), "= Outside\n");
    // The shared text never took the file's.
    assert_eq!(session.host_text(), "= Page\nShared.\n");
    assert_eq!(session.guest_text(ada), "= Page\nShared.\n");

    // Keeping the shared text: the next save writes it.
    session
        .host
        .resolve_disk_conflict(id, DiskResolution::KeepShared)
        .unwrap();
    session.settle();
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Unsaved)
    );
    let text = session.host_text();
    assert_eq!(
        request_save(&mut session, ada),
        SaveOutcome::Saved {
            revision: revision_of(&text)
        }
    );
    assert_eq!(on_disk(&session), "= Page\nShared.\n");

    // A second change on disk is a conflict again.
    fs::write(page(&session), "= Again\n").unwrap();
    guest_types(&mut session, ada, 0, "x");
    session.settle();
    assert!(session.host.save(id).is_err());
    assert_eq!(on_disk(&session), "= Again\n");
}

#[test]
fn reloading_from_disk_starts_everyone_again_in_a_new_epoch() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 7, "Shared.\n");
    session.settle();
    fs::write(page(&session), "= Outside\n").unwrap();
    let id = session.host_doc();
    assert!(session.host.save(id).is_err());
    session
        .host
        .resolve_disk_conflict(id, DiskResolution::ReloadDisk)
        .unwrap();
    session.settle();
    assert_eq!(session.host_text(), "= Outside\n");
    assert_eq!(session.guest_text(ada), "= Outside\n");
    assert_eq!(session.host.shared_file(id).unwrap().1, 2);
    let guest = guest_doc(&mut session, ada);
    assert_eq!(session.engine(ada).shared_file(guest).unwrap().1, 2);
    assert_eq!(host_state(&session), Some(saved("= Outside\n")));
    assert_eq!(guest_state(&mut session, ada), Some(saved("= Outside\n")));
    assert!(session.removed().is_empty());

    guest_types(&mut session, ada, 0, "// ");
    session.settle();
    assert_eq!(session.host_text(), "// = Outside\n");
}

#[test]
fn collab_stale_epoch_is_refused() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let id = session.host_doc();
    fs::write(page(&session), "= Outside\n").unwrap();
    type_at(&mut session.host, id, 0, "x");
    assert!(session.host.save(id).is_err());
    session
        .host
        .resolve_disk_conflict(id, DiskResolution::ReloadDisk)
        .unwrap();
    session.settle();
    let before = session.host_text();

    // A modified guest still writing to epoch 1.
    session.forge(
        ada,
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 1,
            update: EncodedUpdate(vec![0, 0]),
        },
    );
    session.forge(
        ada,
        Message::SaveRequest {
            file: 1,
            epoch: 1,
            request: 41,
        },
    );
    session.settle();
    assert_eq!(session.host_text(), before);
    assert_eq!(on_disk(&session), "= Outside\n");
    assert!(session.sent_to(ada).contains(&Message::SaveResult {
        request: 41,
        outcome: SaveOutcome::Stale,
    }));
    assert!(session.removed().is_empty());

    // Once the session ends, nothing from it is read at all: the forged request stays in
    // flight, unanswered.
    session.host.end_session();
    session.forge(
        ada,
        Message::SaveRequest {
            file: 1,
            epoch: 2,
            request: 42,
        },
    );
    for _ in 0..20 {
        session.host.pump();
    }
    assert_eq!(on_disk(&session), "= Outside\n");
    assert!(!session
        .sent_to(ada)
        .iter()
        .any(|message| matches!(message, Message::SaveResult { request: 42, .. })));
}

#[cfg(unix)]
#[test]
fn a_failed_save_keeps_the_file_and_the_shared_text() {
    use std::os::unix::fs::PermissionsExt;

    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 7, "Shared.\n");
    session.settle();
    let folder = session.directory.path().to_path_buf();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
    let outcome = request_save(&mut session, ada);
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    // Anyone who can write a read-only folder (a superuser) cannot see this fail.
    if matches!(outcome, SaveOutcome::Saved { .. }) {
        return;
    }
    assert!(matches!(outcome, SaveOutcome::Failed { .. }), "{outcome:?}");
    assert_eq!(on_disk(&session), "= Page\n");
    assert_eq!(session.host_text(), "= Page\nShared.\n");
    assert_eq!(host_state(&session), Some(SharedSaveState::Unsaved));
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Unsaved)
    );
}

#[test]
fn a_guest_out_of_touch_does_not_know_what_is_saved() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    assert_eq!(guest_state(&mut session, ada), Some(saved("= Page\n")));
    session.drop_link(ada, 0);
    session.settle();
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Unknown)
    );
    let id = guest_doc(&mut session, ada);
    assert!(session.engine(ada).request_shared_save(id).is_err());
    session.resume(ada);
    session.settle();
    assert_eq!(guest_state(&mut session, ada), Some(saved("= Page\n")));
}

#[test]
fn a_guest_cannot_announce_file_states() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    session.forge(
        ada,
        Message::FileState {
            file: 1,
            epoch: 1,
            saved: None,
            conflict: true,
        },
    );
    session.settle();
    assert_eq!(session.removed(), [(ada, ErrorCode::Protocol)]);
    assert_eq!(host_state(&session), Some(saved("= Page\n")));
}

#[test]
fn an_editor_is_not_dropped_when_the_host_closed_a_file_that_is_gone() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let id = session.host_doc();
    session.host.close(id).unwrap();
    fs::remove_file(page(&session)).unwrap();
    guest_types(&mut session, ada, 0, "x");
    session.settle();
    assert!(session.removed().is_empty());
    assert!(session.sent_to(ada).iter().any(|message| matches!(
        message,
        Message::Error {
            code: ErrorCode::Stale,
            ..
        }
    )));
    assert!(!page(&session).exists());
}

#[test]
fn a_guests_text_is_never_kept_for_recovery_but_the_hosts_is() {
    let state = tempfile::tempdir().unwrap();
    let recovering = |folder: &std::path::Path| {
        let mut settings = Settings::load(None, None)
            .unwrap()
            .with_state_dir(Some(folder.to_path_buf()));
        settings.git.enabled = false;
        let mut engine = Engine::new(settings);
        engine.enable_recovery();
        engine
    };
    let host = recovering(&state.path().join("host"));
    let mut session = Session::with_host(&[("page.adoc", "= Page\n")], host);
    let guest_state_dir = state.path().join("guest");
    let ada = session.knock_with(1, recovering(&guest_state_dir));
    session.host.admit_peer(ada).unwrap();
    session.settle();
    session
        .host
        .set_peer_role(ada, adoc_engine::protocol::Role::Editor)
        .unwrap();
    session.engine(ada).open_session_file(1).unwrap();
    session.settle();
    guest_types(&mut session, ada, 7, "Guest.\n");
    session.settle();
    session.engine(ada).write_recovery_now();
    session.host.write_recovery_now();
    let records = |folder: PathBuf| {
        walk(&folder)
            .into_iter()
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .count()
    };
    assert_eq!(records(guest_state_dir), 0);
    assert_eq!(records(state.path().join("host")), 1);
}

fn walk(folder: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Pumps the host until `done`, for at most fifteen seconds: the file watcher reports changes
/// on disk after its own debounce.
fn until_host(session: &mut Session, mut done: impl FnMut(&mut Session) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done(session) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the watcher"
        );
        session.settle();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_change_the_watcher_reports_is_a_conflict_never_an_edit() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let root = session.directory.path().to_path_buf();
    session.host.open_workspace(vec![root]).unwrap();
    assert!(session.host.wait_for_workers(Duration::from_secs(10)));
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 7, "Shared.\n");
    session.settle();

    fs::write(page(&session), "= Outside\n").unwrap();
    until_host(&mut session, |session| {
        host_state(session) == Some(SharedSaveState::Conflict)
    });
    assert_eq!(session.host_text(), "= Page\nShared.\n");
    assert_eq!(session.guest_text(ada), "= Page\nShared.\n");
    assert_eq!(
        guest_state(&mut session, ada),
        Some(SharedSaveState::Conflict)
    );

    // Put back as it was saved: nothing to resolve.
    fs::write(page(&session), "= Page\n").unwrap();
    until_host(&mut session, |session| {
        host_state(session) == Some(SharedSaveState::Unsaved)
    });

    // Removed: a conflict too, and the text stays.
    fs::remove_file(page(&session)).unwrap();
    until_host(&mut session, |session| {
        host_state(session) == Some(SharedSaveState::Conflict)
    });
    assert_eq!(session.host_text(), "= Page\nShared.\n");
    let id = session.host_doc();
    session
        .host
        .resolve_disk_conflict(id, DiskResolution::KeepShared)
        .unwrap();
    session.host.save(id).unwrap();
    assert_eq!(on_disk(&session), "= Page\nShared.\n");
}
