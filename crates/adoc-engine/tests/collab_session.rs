//! A collaboration session through the engine (COLLAB 4/8, #691; design §§2, 5, 7, 8, 11):
//! a host engine and guest engines meet over an in-memory link that encodes and decodes every
//! message as a frame, as the transport does, and can lose what is in flight. Guests are held to
//! what the host's session allows, whatever they send.

#![cfg(feature = "collab")]

mod collab_common;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids,
    protocol::{ErrorCode, Message, PeerId, Role, SaveOutcome},
    CommandArgs, EncodedUpdate, ReplicaId, SESSION_EVENTS_PER_PUMP,
};
use collab_common::*;

#[test]
fn collab_engines_meet_through_a_session() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    assert_eq!(session.guest_text(ada), "= Page\n");

    let id = session.host_doc();
    type_at(&mut session.host, id, 7, "Host.\n");
    session.settle();
    guest_types(&mut session, ada, 0, "// ");
    session.settle();
    assert_eq!(session.host_text(), "// = Page\nHost.\n");
    assert_eq!(session.guest_text(ada), session.host_text());
    assert!(session.removed().is_empty());
}

#[test]
fn two_guests_converge_through_the_host() {
    let mut session = Session::new(&[("page.adoc", "abc\n")]);
    let ada = session.guest(1, true);
    let bob = session.guest(2, true);
    for round in 0..20 {
        guest_types(&mut session, ada, 0, "a");
        guest_types(&mut session, bob, 1, "b");
        let id = session.host_doc();
        let end = session.host_text().len();
        type_at(&mut session.host, id, end, &round.to_string());
        if round % 3 == 0 {
            session.settle();
        }
    }
    session.settle();
    let host = session.host_text();
    assert_eq!(session.guest_text(ada), host);
    assert_eq!(session.guest_text(bob), host);
    assert_eq!(host.matches('a').count(), 21);
    assert_eq!(host.matches('b').count(), 21);
    assert!(session.removed().is_empty());
}

#[test]
fn collab_nothing_is_sent_before_admit() {
    let mut session = Session::new(&[("page.adoc", "= Secret\n")]);
    let peer = session.knock(1);
    assert_eq!(
        session.host.session_knocks(),
        [(peer, "guest 1".to_owned())]
    );
    assert!(session.sent_to(peer).is_empty());
    assert!(session.net.lock().unwrap().admitted.is_empty());
    // A message from a connection not yet admitted is not read.
    session.forge(peer, Message::OpenFile { file: 1 });
    session.settle();
    assert!(session.sent_to(peer).is_empty());

    session.host.remove_peer(peer);
    session.settle();
    assert!(session.sent_to(peer).is_empty());
    assert_eq!(session.removed(), [(peer, ErrorCode::Denied)]);
    assert!(session.host.participants().is_empty());
}

#[test]
fn collab_unauthorised_requests_are_refused() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let viewer = session.guest(1, false);
    let before = session.host_text();

    // A Viewer's guest engine refuses to edit; a modified one sends an update anyway.
    let engine = session.engine(viewer);
    let id = engine.session_document(1).unwrap();
    assert!(engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .is_err());
    let Some(Message::Snapshot { client, update, .. }) = session
        .sent_to(viewer)
        .into_iter()
        .find(|message| matches!(message, Message::Snapshot { .. }))
    else {
        panic!("no snapshot");
    };
    let mut forged =
        adoc_collab::Replica::from_snapshot(&update, ReplicaId::new(client).unwrap()).unwrap();
    let change = forged
        .apply_local(&[adoc_collab::ByteEdit::new(0, 0, "PWNED ")])
        .unwrap();
    session.forge(
        viewer,
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 1,
            base: client,
            update: change.outbound.unwrap(),
        },
    );
    session.settle();
    assert_eq!(session.host_text(), before);
    assert!(session.sent_to(viewer).iter().any(|message| matches!(
        message,
        Message::Error {
            code: ErrorCode::Unauthorised,
            ..
        }
    )));
    // The guest started again from the host's text.
    assert_eq!(session.guest_text(viewer), before);

    // A Viewer may not save.
    session.forge(
        viewer,
        Message::SaveRequest {
            file: 1,
            epoch: 1,
            request: 9,
        },
    );
    session.settle();
    assert!(session.sent_to(viewer).contains(&Message::SaveResult {
        request: 9,
        outcome: SaveOutcome::PermissionDenied,
    }));
    assert!(session.removed().is_empty());

    // A file id the manifest never offered closes the connection.
    session.forge(viewer, Message::OpenFile { file: 99 });
    session.settle();
    assert_eq!(session.removed(), [(viewer, ErrorCode::Protocol)]);
    assert!(session.host.participants().is_empty());
}

#[test]
fn messages_only_the_host_sends_close_the_connection() {
    for message in [
        Message::Join {
            peer: PeerId(5),
            role: Role::Editor,
            session: adoc_engine::protocol::SessionId([0; 16]),
            resume: adoc_engine::protocol::Secret([0; 32]),
        },
        Message::RoleChanged {
            peer: PeerId(1),
            role: Role::Editor,
        },
        Message::UpdateAck {
            file: 1,
            epoch: 1,
            seq: 1,
        },
        Message::Update {
            file: 2,
            epoch: 1,
            seq: 1,
            base: 0,
            update: EncodedUpdate(vec![0]),
        },
    ] {
        let mut session = Session::new(&[("page.adoc", "= Page\n")]);
        let peer = session.guest(1, true);
        session.forge(peer, message.clone());
        session.settle();
        assert_eq!(
            session.removed(),
            [(peer, ErrorCode::Protocol)],
            "{message:?}"
        );
    }
}

#[test]
fn collab_forged_author_is_refused() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let before = session.host_text();
    let Some(Message::Snapshot { update, .. }) = session
        .sent_to(ada)
        .into_iter()
        .find(|message| matches!(message, Message::Snapshot { .. }))
    else {
        panic!("no snapshot");
    };
    // Items written under a client id the host never gave this guest.
    let mut forged =
        adoc_collab::Replica::from_snapshot(&update, ReplicaId::new(424_242).unwrap()).unwrap();
    let change = forged
        .apply_local(&[adoc_collab::ByteEdit::new(0, 0, "forged ")])
        .unwrap();
    let base = session.base(ada, 1);
    session.forge(
        ada,
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 1,
            base,
            update: change.outbound.unwrap(),
        },
    );
    session.settle();
    assert_eq!(session.host_text(), before);
    assert_eq!(session.removed(), [(ada, ErrorCode::Protocol)]);
}

#[test]
fn updates_out_of_order_close_the_connection() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 0, "a");
    session.settle();
    let before = session.host_text();
    // The next update must be number 2.
    let base = session.base(ada, 1);
    session.forge(
        ada,
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 7,
            base,
            update: EncodedUpdate(vec![0]),
        },
    );
    session.settle();
    assert_eq!(session.host_text(), before);
    assert_eq!(session.removed(), [(ada, ErrorCode::Protocol)]);
}

#[test]
fn a_role_taken_away_while_edits_are_in_flight() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let before = session.host_text();
    // Typed and sent, but not yet read by the host when it takes editing away.
    guest_types(&mut session, ada, 0, "a");
    guest_types(&mut session, ada, 1, "b");
    session.engine(ada).pump();
    session.host.set_peer_role(ada, Role::Viewer).unwrap();
    session.settle();
    assert_eq!(session.host_text(), before);
    assert_eq!(session.guest_text(ada), before);
    assert!(session.removed().is_empty());

    // Given editing back, the guest's edits count again.
    session.host.set_peer_role(ada, Role::Editor).unwrap();
    session.settle();
    guest_types(&mut session, ada, 0, "c");
    session.settle();
    assert_eq!(session.host_text(), format!("c{before}"));
    assert_eq!(session.guest_text(ada), session.host_text());
}

#[test]
fn a_reconnected_guest_delivers_each_edit_once() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 7, "one ");
    session.settle();
    // Three more edits are sent; the link drops with the last two still in flight, and the
    // host's own edit never reaches the guest.
    guest_types(&mut session, ada, 11, "two ");
    guest_types(&mut session, ada, 15, "three ");
    guest_types(&mut session, ada, 21, "four ");
    session.engine(ada).pump();
    assert_eq!(session.net.lock().unwrap().to_host.len(), 3);
    session.net.lock().unwrap().hold_host = true;
    let id = session.host_doc();
    type_at(&mut session.host, id, 0, "// ");
    session.host.pump();
    session.drop_link(ada, 2);
    session.net.lock().unwrap().hold_host = false;
    session.settle();
    assert!(!session.engine(ada).session_connected());
    // The host has "two" only; the guest never had the host's "// ".
    assert_eq!(session.host_text(), "// = Page\none two ");
    assert_eq!(session.guest_text(ada), "= Page\none two three four ");
    // A disconnected guest cannot edit.
    let engine = session.engine(ada);
    let doc = engine.session_document(1).unwrap();
    assert!(engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: doc,
                range: SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .is_err());

    session.resume(ada);
    session.settle();
    let host = session.host_text();
    assert_eq!(host, "// = Page\none two three four ");
    assert_eq!(session.guest_text(ada), host);
    assert!(session.engine(ada).session_connected());
    assert!(session.removed().is_empty());

    // Editing works again after the resynchronisation.
    guest_types(&mut session, ada, 0, "!");
    session.settle();
    assert_eq!(session.host_text(), format!("!{host}"));
}

#[test]
fn a_page_a_guest_opens_stays_in_the_hosts_background() {
    let mut session = Session::new(&[("page.adoc", "= Page\n"), ("other.adoc", "= Other\n")]);
    // The host is working on another page when Ada opens this one.
    let path = session.directory.path().join("other.adoc");
    let other = session.host.open(&path).unwrap();
    let ada = session.guest(1, true);
    let id = session.host_doc();
    assert!(
        !session.host.has_tab(id),
        "opened for the guest, not the host"
    );
    assert_eq!(session.host.active(), Some(other));

    // The host opens it, then closes its tab: it stays shared for Ada, in the background.
    session
        .host
        .open(&session.directory.path().join("page.adoc"))
        .unwrap();
    assert!(session.host.has_tab(id));
    session.host.close(id).unwrap();
    assert!(!session.host.has_tab(id));
    guest_types(&mut session, ada, 0, "x");
    session.settle();
    assert_eq!(session.host_text(), "x= Page\n");
    assert_eq!(session.host.shared_file(id).unwrap().1, 1, "no new epoch");
    assert!(session.removed().is_empty());

    // When sharing stops, a page with changes not saved comes forward as a tab; a saved one
    // closes.
    session.host.end_session();
    assert!(session.host.has_tab(id));
    assert!(session.host.document(id).unwrap().dirty);
}

#[test]
fn a_saved_page_in_the_background_closes_when_sharing_stops() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let _ada = session.guest(1, true);
    let id = session.host_doc();
    assert!(!session.host.has_tab(id));
    session.host.end_session();
    assert!(session.host.document(id).is_none());
}

#[test]
fn collab_manifest_offers_only_what_may_be_shared() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    let outside = directory.path().join("outside.adoc");
    fs::write(&outside, "= Outside\n").unwrap();
    for (name, text) in [
        ("modules/ROOT/pages/index.adoc", "= Index\n"),
        ("modules/ROOT/partials/part.adoc", "Part.\n"),
        (".git/config.adoc", "x"),
        ("modules/ROOT/pages/.draft.adoc", "x"),
        ("node_modules/pkg/readme.adoc", "x"),
        ("modules/ROOT/pages/notes.txt", "x"),
    ] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("modules/ROOT/pages/link.adoc")).unwrap();

    let offer = |files: &[&str]| {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        let net = Arc::new(Mutex::new(Net::default()));
        engine().host_session(Box::new(HostLink(net)), &root, &files, "Host")
    };
    let manifest = offer(&[
        "modules/ROOT/pages/index.adoc",
        "modules/ROOT/partials/part.adoc",
    ])
    .unwrap();
    let shown: Vec<(&str, adoc_engine::protocol::FileKind)> = manifest
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect();
    assert_eq!(
        shown,
        [
            (
                "modules/ROOT/pages/index.adoc",
                adoc_engine::protocol::FileKind::Page
            ),
            (
                "modules/ROOT/partials/part.adoc",
                adoc_engine::protocol::FileKind::Partial
            ),
        ]
    );
    let mut refused = vec![
        ".git/config.adoc",
        "modules/ROOT/pages/.draft.adoc",
        "node_modules/pkg/readme.adoc",
        "modules/ROOT/pages/notes.txt",
        "modules/ROOT/pages/missing.adoc",
        "../outside.adoc",
        "modules/../../outside.adoc",
    ];
    let absolute = outside.display().to_string();
    refused.push(&absolute);
    #[cfg(unix)]
    refused.push("modules/ROOT/pages/link.adoc");
    for file in refused {
        assert!(offer(&[file]).is_err(), "{file} was offered");
    }
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_link_after_sharing_is_not_opened() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let outside = session
        .directory
        .path()
        .parent()
        .unwrap()
        .join(format!("outside-{}.adoc", std::process::id()));
    fs::write(&outside, "= Secret\n").unwrap();
    let page = session.directory.path().join("page.adoc");
    fs::remove_file(&page).unwrap();
    std::os::unix::fs::symlink(&outside, &page).unwrap();
    let peer = session.knock(1);
    session.host.admit_peer(peer).unwrap();
    session.settle();
    session.engine(peer).open_session_file(1).unwrap();
    session.settle();
    fs::remove_file(&outside).unwrap();
    assert!(!session
        .sent_to(peer)
        .iter()
        .any(|message| matches!(message, Message::Snapshot { .. })));
    assert!(session.host.session_document(1).is_none());
    let _ = Path::new(&page);
}

#[test]
fn a_session_drains_a_bounded_number_of_events_a_pump() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    for _ in 0..(SESSION_EVENTS_PER_PUMP + 10) {
        guest_types(&mut session, ada, 0, "a");
        session.engine(ada).pump();
    }
    let waiting = session.net.lock().unwrap().to_host.len();
    assert!(waiting > SESSION_EVENTS_PER_PUMP);
    assert_eq!(session.host.pump(), SESSION_EVENTS_PER_PUMP);
    // More wait: the next pump is due at once.
    assert!(session
        .host
        .next_due()
        .is_some_and(|due| due <= std::time::Instant::now()));
    session.settle();
    assert_eq!(session.guest_text(ada), session.host_text());
    assert!(session
        .host_text()
        .starts_with(&"a".repeat(SESSION_EVENTS_PER_PUMP + 10)));
}

#[test]
fn guests_are_told_who_is_in_the_session() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let bob = session.guest(2, false);
    session.settle();
    let names = |engine: &mut adoc_engine::Engine| {
        engine
            .participants()
            .into_iter()
            .map(|participant| (participant.name, participant.role, participant.connected))
            .collect::<Vec<_>>()
    };
    let expected = vec![
        ("Host".to_owned(), Role::Owner, true),
        ("guest 1".to_owned(), Role::Editor, true),
        ("guest 2".to_owned(), Role::Viewer, true),
    ];
    assert_eq!(names(session.engine(ada)), expected);
    assert_eq!(names(session.engine(bob)), expected);
    assert_eq!(
        session.engine(ada).session_workspace(),
        session.host.session_workspace()
    );
    assert_eq!(session.engine(bob).session_me(), Some((bob, Role::Viewer)));

    // A change of role and a drop reach everyone, once each.
    session.host.set_peer_role(bob, Role::Editor).unwrap();
    session.drop_link(ada, 0);
    session.settle();
    let told = session
        .sent_to(bob)
        .iter()
        .filter(|message| matches!(message, Message::Roster { .. }))
        .count();
    assert_eq!(
        names(session.engine(bob)),
        vec![
            ("Host".to_owned(), Role::Owner, true),
            ("guest 1".to_owned(), Role::Editor, false),
            ("guest 2".to_owned(), Role::Editor, true),
        ]
    );
    session.settle();
    assert_eq!(
        session
            .sent_to(bob)
            .iter()
            .filter(|message| matches!(message, Message::Roster { .. }))
            .count(),
        told
    );
}

#[test]
fn a_remote_tab_closed_can_be_opened_again() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let id = session.engine(ada).session_document(1).unwrap();
    session.engine(ada).close(id).unwrap();
    session.settle();
    assert!(session.net.lock().unwrap().to_host.is_empty());
    assert_eq!(session.engine(ada).session_document(1), None);
    // The host no longer sends it edits.
    let host_doc = session.host_doc();
    let before = session.sent_to(ada).len();
    type_at(&mut session.host, host_doc, 0, "x");
    session.settle();
    assert!(!session.sent_to(ada)[before..]
        .iter()
        .any(|message| matches!(message, Message::Update { .. })));
    session.engine(ada).open_session_file(1).unwrap();
    session.settle();
    assert_eq!(session.guest_text(ada), "x= Page\n");
}

#[test]
fn a_guests_document_is_never_unsaved_on_its_side() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 0, "x");
    session.settle();
    let engine = session.engine(ada);
    let id = engine.session_document(1).unwrap();
    assert!(!engine.document(id).unwrap().dirty);
    assert!(session.host.document(session.host_doc()).unwrap().dirty);
}

#[test]
fn selections_of_an_earlier_epoch_are_forgotten() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    let ada = session.guest(1, true);
    let bob = session.guest(2, true);
    // Ada shows a selection; the host and Bob see it.
    let (anchor, head) = {
        let engine = session.engine(ada);
        let id = engine.session_document(1).unwrap();
        let _ = engine.document(id).unwrap();
        (0, 2)
    };
    let selection = {
        let snapshot = session
            .sent_to(ada)
            .into_iter()
            .find_map(|message| match message {
                Message::Snapshot { update, client, .. } => Some((update, client)),
                _ => None,
            })
            .unwrap();
        let replica = adoc_collab::Replica::from_snapshot(
            &snapshot.0,
            adoc_collab::ReplicaId::new(snapshot.1).unwrap(),
        )
        .unwrap();
        replica
            .selection(anchor, head, adoc_collab::Affinity::After)
            .unwrap()
    };
    let events = session.engine(bob).subscribe();
    session.forge(
        ada,
        Message::Awareness {
            file: 1,
            epoch: 1,
            peer: ada,
            selection,
        },
    );
    session.settle();
    let host_doc = session.host_doc();
    assert_eq!(session.host.peer_selections(host_doc).len(), 1);
    let bob_doc = session.engine(bob).session_document(1).unwrap();
    assert_eq!(session.engine(bob).peer_selections(bob_doc).len(), 1);
    assert!(events.try_iter().any(
        |event| matches!(event, adoc_engine::EngineEvent::PresenceChanged(id) if id == bob_doc)
    ));

    // The file starts again in a new epoch: everyone's selections of the old text go.
    fs::write(session.directory.path().join("page.adoc"), "= Outside\n").unwrap();
    type_at(&mut session.host, host_doc, 0, "y");
    assert!(session.host.save(host_doc).is_err());
    session
        .host
        .resolve_disk_conflict(host_doc, adoc_engine::DiskResolution::ReloadDisk)
        .unwrap();
    session.settle();
    assert!(session.host.peer_selections(host_doc).is_empty());
    assert!(session.engine(bob).peer_selections(bob_doc).is_empty());
}

fn peer_args(peer: PeerId) -> adoc_engine::CommandArgs {
    adoc_engine::CommandArgs::Custom(serde_json::json!({ "peer": peer.0 }))
}

#[test]
fn collaboration_commands_act_on_the_session() {
    use adoc_engine::{CollaborationStep, CommandOutcome};

    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    // Listening, connecting and inviting are the front end's: their commands say so.
    assert!(session
        .host
        .invoke(ids::COLLAB_SHARE, CommandArgs::None)
        .is_err());
    assert_eq!(
        session
            .host
            .invoke(ids::COLLAB_COPY_INVITATION, CommandArgs::None)
            .unwrap(),
        CommandOutcome::Collaboration(CollaborationStep::CopyInvitation)
    );
    let ada = session.knock(1);
    session
        .host
        .invoke(ids::COLLAB_ADMIT, peer_args(ada))
        .unwrap();
    session.settle();
    session
        .host
        .invoke(ids::COLLAB_GRANT_EDIT, peer_args(ada))
        .unwrap();
    session.settle();
    assert_eq!(session.engine(ada).session_me(), Some((ada, Role::Editor)));
    session
        .engine(ada)
        .invoke(
            ids::COLLAB_OPEN_FILE,
            CommandArgs::Custom(serde_json::json!({ "file": 1 })),
        )
        .unwrap();
    session.settle();
    // A guest's Save asks the host.
    guest_types(&mut session, ada, 0, "x");
    session.settle();
    let id = session.engine(ada).session_document(1).unwrap();
    session
        .engine(ada)
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        .unwrap();
    session.settle();
    assert_eq!(
        fs::read_to_string(session.directory.path().join("page.adoc")).unwrap(),
        "x= Page\n"
    );
    session
        .host
        .invoke(ids::COLLAB_REVOKE_EDIT, peer_args(ada))
        .unwrap();
    session.settle();
    assert_eq!(session.engine(ada).session_me(), Some((ada, Role::Viewer)));
    // Stopping ends the session and hands the front end its step.
    assert_eq!(
        session
            .host
            .invoke(ids::COLLAB_STOP, CommandArgs::None)
            .unwrap(),
        CommandOutcome::Collaboration(CollaborationStep::Ended)
    );
    assert!(!session.host.in_session());
    assert_eq!(session.removed(), [(ada, ErrorCode::Closed)]);
    assert!(session
        .host
        .invoke(ids::COLLAB_STOP, CommandArgs::None)
        .is_err());
}

#[test]
fn the_share_plan_offers_what_hosting_would() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    for name in [
        "modules/ROOT/pages/index.adoc",
        "modules/ROOT/partials/part.adoc",
        "modules/ROOT/pages/.draft.adoc",
        "node_modules/pkg/readme.adoc",
    ] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "= Page\n").unwrap();
    }
    let mut engine = engine();
    assert!(matches!(
        engine.invoke(ids::COLLAB_SHARE, CommandArgs::None),
        Err(adoc_engine::EngineError::Refused(_))
    ));
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let plan = engine.share_plan(&root);
    let root = adoc_engine::normalize_path(&root);
    assert_eq!(
        plan.files,
        [
            root.join("modules/ROOT/pages/index.adoc"),
            root.join("modules/ROOT/partials/part.adoc"),
        ]
    );
    // Whatever the plan offers, hosting offers as it is.
    let net = Arc::new(Mutex::new(Net::default()));
    let manifest = engine
        .host_session(Box::new(HostLink(net)), &root, &plan.files, "Host")
        .unwrap();
    assert_eq!(manifest.len(), 2);
}

#[test]
fn collaboration_is_offered_only_once_turned_on() {
    use adoc_engine::Settings;

    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    assert!(!settings.collaboration.enabled, "off by default");
    let directory = tempfile::tempdir().unwrap();
    let mut engine = adoc_engine::Engine::new(settings.clone());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    for id in [ids::COLLAB_SHARE, ids::COLLAB_JOIN, ids::COLLAB_STOP] {
        assert!(!engine.command_available(id), "{id:?} is offered while off");
    }
    assert!(matches!(
        engine.invoke(ids::COLLAB_SHARE, CommandArgs::None),
        Err(adoc_engine::EngineError::Refused(reason)) if reason.contains("[collaboration] enabled")
    ));
    assert!(engine.invoke(ids::COLLAB_JOIN, CommandArgs::None).is_err());

    // As the app reads it: from the user settings file, which opening a folder reads again.
    let user = directory.path().join("settings.toml");
    fs::write(
        &user,
        "[collaboration]\nenabled = true\n[git]\nenabled = false\n",
    )
    .unwrap();
    let _ = settings;
    let mut engine = adoc_engine::Engine::new(Settings::load(Some(&user), None).unwrap());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.command_available(ids::COLLAB_SHARE));
    assert_eq!(
        engine.invoke(ids::COLLAB_JOIN, CommandArgs::None).unwrap(),
        adoc_engine::CommandOutcome::Collaboration(adoc_engine::CollaborationStep::Join)
    );
}

#[test]
fn a_running_session_can_be_ended_whatever_the_setting() {
    let mut session = Session::new(&[("page.adoc", "= Page\n")]);
    assert!(!session.host.settings().collaboration.enabled);
    assert!(session.host.command_available(ids::COLLAB_STOP));
    session
        .host
        .invoke(ids::COLLAB_STOP, CommandArgs::None)
        .unwrap();
    assert!(!session.host.command_available(ids::COLLAB_STOP));
}

/// Each participant's caret and selection reach the others and follow the text through
/// everyone's edits (PRD §10 `collab_remote_carets_follow_edits`).
#[test]
fn collab_remote_carets_follow_edits() {
    let mut session = Session::new(&[("page.adoc", "= Page\nSome text.\n")]);
    let ada = session.guest(1, true);
    let host_doc = session.host_doc();
    // The host's caret after "Some", and Ada's selection of "text".
    session.host.share_selection(host_doc, 11, 11);
    let guest_doc = session.engine(ada).session_document(1).unwrap();
    session.engine(ada).share_selection(guest_doc, 12, 16);
    session.settle();
    let carets = session.engine(ada).peer_carets(guest_doc);
    assert_eq!(carets.len(), 1);
    assert_eq!(
        (
            carets[0].peer,
            carets[0].name.as_str(),
            carets[0].anchor,
            carets[0].head
        ),
        (PeerId::HOST, "Host", 11, 11)
    );
    let carets = session.host.peer_carets(host_doc);
    assert_eq!(
        (
            carets[0].peer,
            carets[0].name.as_str(),
            carets[0].anchor,
            carets[0].head
        ),
        (ada, "guest 1", 12, 16)
    );

    // Text typed before them moves both. Ada types at her own caret, the end of her selection,
    // in her text before the host's edit reaches her: the end moves past it, as a caret does.
    type_at(&mut session.host, host_doc, 0, "// ");
    guest_types(&mut session, ada, 16, "!");
    session.settle();
    let carets = session.engine(ada).peer_carets(guest_doc);
    assert_eq!((carets[0].anchor, carets[0].head), (14, 14));
    let carets = session.host.peer_carets(host_doc);
    assert_eq!((carets[0].anchor, carets[0].head), (15, 20));
    assert_eq!(session.host_text(), "// = Page\nSome text!.\n");

    // A participant who drops out shows no caret.
    session.drop_link(ada, 0);
    session.settle();
    assert!(session.host.peer_carets(host_doc).is_empty());
}

#[test]
fn presence_is_paced_and_the_latest_wins() {
    let mut session = Session::new(&[("page.adoc", "= Page\nSome text.\n")]);
    let ada = session.guest(1, true);
    let host_doc = session.host_doc();
    let awareness = |session: &Session| {
        session
            .sent_to(ada)
            .iter()
            .filter(|message| matches!(message, Message::Awareness { .. }))
            .count()
    };
    let before = awareness(&session);
    for offset in 0..10 {
        session.host.share_selection(host_doc, offset, offset);
    }
    session.settle();
    // One at once; the rest wait for the interval.
    assert_eq!(awareness(&session), before + 1);
    std::thread::sleep(
        adoc_engine::protocol::HEARTBEAT_INTERVAL.min(std::time::Duration::from_millis(80)),
    );
    assert!(session.host.next_due().is_some());
    session.settle();
    assert_eq!(awareness(&session), before + 2);
    let guest_doc = session.engine(ada).session_document(1).unwrap();
    let carets = session.engine(ada).peer_carets(guest_doc);
    assert_eq!(
        (carets[0].anchor, carets[0].head),
        (9, 9),
        "the latest selection"
    );
}

/// Every command the engine has, decided for a session: refused while hosting
/// (`REFUSED_WHILE_HOSTING`) or reviewed and allowed here. A new command fails this until it
/// is put in one or the other.
const ALLOWED_WHILE_HOSTING: &[&str] = &[
    "app.about",
    "app.check_for_updates",
    "app.diagnostics",
    "app.documentation",
    "app.quit",
    "appearance.save",
    "appearance.save_font_set",
    "collaboration.admit",
    "collaboration.copy_invitation",
    "collaboration.deny",
    "collaboration.follow",
    "collaboration.grant_edit",
    "collaboration.join",
    "collaboration.leave",
    "collaboration.open_file",
    "collaboration.remove",
    "collaboration.revoke_edit",
    "collaboration.share_workspace",
    "collaboration.stop_following",
    "collaboration.stop_sharing",
    "document.browse",
    "document.close",
    "document.close_all",
    "document.close_clean",
    "document.close_left",
    "document.close_others",
    "document.close_right",
    "document.discard_recovery",
    "document.keep_open",
    "document.local_history",
    "document.move_tab",
    "document.open",
    "document.overwrite",
    "document.pin_tab",
    "document.recover",
    "document.recover_unsaved",
    "document.reload",
    "document.restore_version",
    "document.save",
    "document.save_all",
    "document.unpin_tab",
    "edit.delete_table_column",
    "edit.delete_table_row",
    "edit.edit_image",
    "edit.format_table",
    "edit.indent_list_item",
    "edit.insert_admonition",
    "edit.insert_code_block",
    "edit.insert_image",
    "edit.insert_link",
    "edit.insert_table",
    "edit.insert_table_column",
    "edit.insert_table_row",
    "edit.insert_text",
    "edit.outdent_list_item",
    "edit.redo",
    "edit.remove_link",
    "edit.replace_range",
    "edit.set_block_type",
    "edit.toggle_bullet_list",
    "edit.toggle_checked",
    "edit.toggle_checklist",
    "edit.toggle_code",
    "edit.toggle_emphasis",
    "edit.toggle_highlight",
    "edit.toggle_line_comment",
    "edit.toggle_numbered_list",
    "edit.toggle_strong",
    "edit.undo",
    "export.docbook_component",
    "export.docbook_page",
    "export.docbook_profile",
    "export.docx_component",
    "export.docx_component_themed",
    "export.docx_page",
    "export.docx_page_themed",
    "export.docx_profile",
    "export.epub_component",
    "export.epub_page",
    "export.epub_profile",
    "export.html_page",
    "export.kindle_component",
    "export.kindle_page",
    "export.kindle_profile",
    "export.pdf_component",
    "export.pdf_page",
    "export.pdf_pages",
    "export.pdf_profile",
    "git.commit",
    "git.next_change",
    "git.previous_change",
    "git.push",
    "git.refresh",
    "git.revert_hunk",
    "git.save_and_commit",
    "git.show_blame",
    "git.show_changes",
    "git.stage",
    "git.stage_all_adoc",
    "git.unstage",
    "graph.export",
    "graph.reset_layout",
    "image.reload",
    "impact.analyse_changes",
    "impact.compare_revisions",
    "impact.inspect",
    "navigate.back",
    "navigate.copy_remote_link",
    "navigate.definition",
    "navigate.find_references",
    "navigate.follow_link",
    "navigate.forward",
    "navigate.go_to",
    "navigate.go_to_symbol",
    "navigate.go_to_symbol_in_page",
    "navigate.inspect_attribute",
    "navigate.open_in_version",
    "navigate.open_on_remote",
    "navigate.peek_include",
    "navigate.reveal",
    "preview.pin",
    "preview.retry_diagrams",
    "project.build_site",
    "quality.run_audit",
    "settings.open",
    "settings.reload",
    "sidebar.collapse_all",
    "sidebar.collapse_one_level",
    "sidebar.expand_all",
    "sidebar.reset_expansion",
    "spelling.add_to_user",
    "spelling.add_to_workspace",
    "view.detach_preview",
    "view.graph",
    "view.local_graph",
    "view.show_source",
    "view.show_split",
    "view.show_visual",
    "view.toggle_pdf_preview",
    "view.toggle_pdf_window",
    "view.toggle_preview",
    "view.toggle_scroll_sync",
    "visual.delete_backward",
    "visual.delete_forward",
    "visual.insert_block",
    "visual.insert_text",
    "visual.split_block",
    "workspace.quick_open",
    "workspace.reindex",
    "workspace.search",
];

#[test]
fn every_command_is_decided_for_a_session() {
    let engine = engine();
    let undecided: Vec<&str> = engine
        .commands()
        .all()
        .map(|spec| spec.id)
        .filter(|id| {
            !adoc_engine::REFUSED_WHILE_HOSTING.contains(id)
                && !ALLOWED_WHILE_HOSTING.contains(&id.as_str())
        })
        .map(adoc_engine::CommandId::as_str)
        .collect();
    assert!(
        undecided.is_empty(),
        "decide these for a session: {undecided:?}"
    );
}

/// Commands that would change the host's files beyond a page's text are refused while it
/// shares, before they do anything (PRD §10 `collab_structural_commands_refused`).
#[test]
fn collab_structural_commands_refused() {
    let mut session = Session::new(&[("page.adoc", "= Page\n"), ("other.adoc", "= Other\n")]);
    let ada = session.guest(1, true);
    let listing = |session: &Session| {
        let mut names: Vec<String> = fs::read_dir(session.directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let before = listing(&session);
    let page = session.host_doc();
    for &id in adoc_engine::REFUSED_WHILE_HOSTING {
        for args in [CommandArgs::None, CommandArgs::Document(page)] {
            let refused = session.host.invoke(id, args);
            assert!(
                matches!(&refused, Err(adoc_engine::EngineError::Refused(reason)) if reason.contains("while sharing")),
                "{id:?}: {refused:?}"
            );
        }
    }
    assert_eq!(listing(&session), before);
    assert_eq!(session.host_text(), "= Page\n");

    // A guest's shared page is the host's: it is not exported, renamed or deleted here.
    let guest_page = session.engine(ada).session_document(1).unwrap();
    for id in [
        ids::EXPORT_HTML_PAGE,
        ids::PAGE_RENAME,
        ids::FILE_DELETE,
        ids::DOCUMENT_LOCAL_HISTORY,
    ] {
        let refused = session
            .engine(ada)
            .invoke(id, CommandArgs::Document(guest_page));
        assert!(
            matches!(&refused, Err(adoc_engine::EngineError::Refused(reason)) if reason.contains("host")),
            "{id:?}: {refused:?}"
        );
    }

    // Stopping sharing makes them available again.
    session.host.end_session();
    let after = session.host.invoke(ids::FILE_NEW, CommandArgs::None);
    assert!(
        !matches!(&after, Err(adoc_engine::EngineError::Refused(reason)) if reason.contains("while sharing")),
        "{after:?}"
    );
}

/// A guest's shared page is parsed for its diagnostics once typing pauses, not on every update
/// the host sends: a 1 MiB page would otherwise cost a whole parse per keystroke.
#[test]
fn a_guests_page_is_parsed_once_typing_pauses() {
    use adoc_core::probe::{self, Probe};
    let mut session = Session::new(&[("page.adoc", "= Page\n\nText.\n")]);
    let clock = adoc_engine::ManualClock::new();
    let mut settings = adoc_engine::Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    let guest = adoc_engine::Engine::with_clock(settings, std::sync::Arc::new(clock.clone()));
    let ada = session.knock_with(1, guest);
    session.host.admit_peer(ada).unwrap();
    session.settle();
    session.engine(ada).open_session_file(1).unwrap();
    session.settle();
    let id = session.engine(ada).session_document(1).unwrap();

    probe::reset();
    let host = session.host_doc();
    for n in 0..5 {
        type_at(&mut session.host, host, 0, &n.to_string());
        session.settle();
    }
    assert_eq!(session.guest_text(ada), session.host_text());
    assert_eq!(probe::count(Probe::Parse), 0, "parsed while the host typed");

    clock.advance(adoc_engine::INDEX_SETTLE);
    session.settle();
    assert_eq!(probe::count(Probe::Parse), 1, "parsed once typing paused");
    assert!(session
        .engine(ada)
        .document(id)
        .unwrap()
        .text_string()
        .starts_with("43210"));
}
