//! PRD §10 test 5, `collab_local_undo_keeps_remote_edits`: undo and redo touch only the local
//! author's work, however remote edits interleave, and groups end where the engine says.

mod common;

use adoc_collab::{authors, ByteEdit, ChangeOrigin};
use common::{id, Peer};

#[test]
fn collab_local_undo_keeps_remote_edits() {
    let mut alice = Peer::host("Para.\n", 1);
    let mut bob = Peer::join(&alice, 2);
    assert!(
        !alice.replica.can_undo(),
        "the initial text is nobody's edit"
    );
    assert!(!bob.replica.can_undo(), "nor is the snapshot");

    // Alice types a word; Bob inserts text inside it; Alice undoes her word.
    bob.receive(&alice.edit(&[ByteEdit::new(4, 4, " wonderful")]));
    alice.replica.end_group();
    alice.receive(&bob.edit(&[ByteEdit::new(8, 8, "XX")]));
    assert_eq!(alice.rope, "Para wonXXderful.\n");

    let change = alice.replica.undo_local().unwrap().unwrap();
    assert_eq!(change.origin, ChangeOrigin::Undo);
    alice.apply(&change);
    bob.receive(&change.outbound.unwrap());
    assert_eq!(alice.rope, "ParaXX.\n", "Bob's text stays where it was");
    assert_eq!(bob.rope, alice.rope);

    // Redo puts Alice's word back around Bob's text, never deleting it.
    bob.receive(&alice.redo());
    assert_eq!(alice.rope, "Para wonXXderful.\n");
    assert_eq!(bob.rope, alice.rope);

    // Alice's history is her word alone: undoing it again leaves the rest.
    bob.receive(&alice.undo());
    assert_eq!(alice.rope, "ParaXX.\n");
    assert!(alice.replica.undo_local().unwrap().is_none());

    // Bob's history holds only his insertion.
    alice.receive(&bob.undo());
    assert_eq!(bob.rope, "Para.\n");
    assert!(bob.replica.undo_local().unwrap().is_none());
    assert_eq!(alice.rope, bob.rope);
}

#[test]
fn groups_end_where_the_engine_says() {
    let mut peer = Peer::host("", 1);
    // Three keystrokes in one group, then two in another.
    for (at, key) in [(0, "a"), (1, "b"), (2, "c")] {
        peer.edit(&[ByteEdit::new(at, at, key)]);
    }
    peer.replica.end_group();
    for (at, key) in [(3, " "), (4, "d")] {
        peer.edit(&[ByteEdit::new(at, at, key)]);
    }
    peer.undo();
    assert_eq!(peer.rope, "abc");
    peer.undo();
    assert_eq!(peer.rope, "");
    peer.redo();
    assert_eq!(peer.rope, "abc");
}

#[test]
fn remote_edits_never_enter_the_local_history() {
    let mut host = Peer::host("", 1);
    let mut guest = Peer::join(&host, 2);
    for at in 0..5 {
        guest.receive(&host.edit(&[ByteEdit::new(at, at, "h")]));
    }
    assert!(!guest.replica.can_undo());
    assert!(guest.replica.undo_local().unwrap().is_none());
    assert_eq!(guest.rope, "hhhhh");
}

#[test]
fn a_guest_undo_and_redo_name_only_the_guest() {
    let mut host = Peer::host("The host's line.\n", 1);
    let mut guest = Peer::join(&host, 2);
    // The guest deletes the host's word and undoes it: the text comes back as the guest's.
    let deletion = guest.edit(&[ByteEdit::new(4, 10, "")]);
    assert!(authors(&deletion).unwrap().is_empty());
    host.receive(&deletion);
    guest.replica.end_group();
    let undo = guest.undo();
    assert_eq!(authors(&undo).unwrap(), vec![id(2)]);
    host.receive(&undo);
    assert_eq!(host.rope, "The host's line.\n");
    // The guest types, undoes and redoes: the redo creates the guest's items again.
    host.receive(&guest.edit(&[ByteEdit::new(0, 0, "Guest: ")]));
    guest.replica.end_group();
    let undo = guest.undo();
    assert!(authors(&undo).unwrap().is_empty());
    host.receive(&undo);
    let redo = guest.redo();
    assert_eq!(authors(&redo).unwrap(), vec![id(2)]);
    host.receive(&redo);
    assert_eq!(host.rope, guest.rope);
}
