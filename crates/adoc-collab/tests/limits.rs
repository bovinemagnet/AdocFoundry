//! The design's `collab_limits_hold_at_their_bounds`, and a long session's growth.

mod common;

use adoc_collab::{
    ByteEdit, CollabError, EncodedUpdate, RelativeSelection, Replica, ReplicaId, Sender,
    MAX_ANCHOR, MAX_SHARED_FILE, MAX_UPDATE,
};
use common::{id, Peer};

// `Peer` is for the long session below.

#[test]
fn collab_limits_hold_at_their_bounds() {
    // Replica ids: 1 to 2^53 - 1.
    assert!(ReplicaId::new(0).is_none());
    assert!(ReplicaId::new(1).is_some());
    assert!(ReplicaId::new(ReplicaId::MAX).is_some());
    assert!(ReplicaId::new(ReplicaId::MAX + 1).is_none());
    assert!(ReplicaId::random().get() <= ReplicaId::MAX);

    // A shared text: at most MAX_SHARED_FILE bytes, from the start or by an edit.
    let full = "a".repeat(MAX_SHARED_FILE);
    let mut replica = Replica::new(&full, id(1)).unwrap();
    assert_eq!(replica.len(), MAX_SHARED_FILE);
    assert_eq!(
        replica
            .apply_local(&[ByteEdit::new(0, 0, "b")])
            .unwrap_err(),
        CollabError::TooLarge(MAX_SHARED_FILE + 1)
    );
    replica.apply_local(&[ByteEdit::new(0, 1, "b")]).unwrap();
    assert!(!replica.is_poisoned());
    let over = "a".repeat(MAX_SHARED_FILE + 1);
    assert_eq!(
        Replica::new(&over, id(1)).unwrap_err(),
        CollabError::TooLarge(MAX_SHARED_FILE + 1)
    );

    // An update: refused unread one byte over MAX_UPDATE; at the limit it is read.
    let mut guest = Replica::new("", id(2)).unwrap();
    assert_eq!(
        guest
            .apply_remote(&EncodedUpdate(vec![0; MAX_UPDATE + 1]), Sender::Host)
            .unwrap_err(),
        CollabError::TooLarge(MAX_UPDATE + 1)
    );
    assert!(matches!(
        guest.apply_remote(&EncodedUpdate(vec![0; MAX_UPDATE]), Sender::Host),
        Err(CollabError::MalformedUpdate(_)) | Ok(_)
    ));
    assert!(!guest.is_poisoned());

    // A whole file's snapshot fits in one update, so a guest can join any file the host may
    // share.
    let host = Replica::new(&"a".repeat(MAX_SHARED_FILE - 10), id(3)).unwrap();
    let snapshot = host.snapshot().unwrap();
    assert!(snapshot.0.len() <= MAX_UPDATE, "{}", snapshot.0.len());
    let mut first = Replica::from_snapshot(&snapshot, id(4)).unwrap();
    let mut second = Replica::from_snapshot(&snapshot, id(5)).unwrap();

    // Two edits each within the limit, together over it: the replica receiving the other's
    // is poisoned, since there is no undoing a CRDT transaction, and must be rebuilt.
    let mine = first.apply_local(&[ByteEdit::new(0, 0, "123456")]).unwrap();
    let theirs = second
        .apply_local(&[ByteEdit::new(0, 0, "abcdef")])
        .unwrap();
    assert_eq!(
        first
            .apply_remote(&theirs.outbound.unwrap(), Sender::Host)
            .unwrap_err(),
        CollabError::TooLarge(MAX_SHARED_FILE + 2)
    );
    assert!(first.is_poisoned());
    assert_eq!(
        first
            .apply_remote(&mine.outbound.unwrap(), Sender::Host)
            .unwrap_err(),
        CollabError::Poisoned
    );
    assert_eq!(first.snapshot().unwrap_err(), CollabError::Poisoned);

    // A selection's encoded end: at most MAX_ANCHOR bytes.
    assert!(matches!(
        RelativeSelection::from_wire(vec![0; MAX_ANCHOR + 1], vec![]),
        Err(CollabError::TooLarge(_))
    ));
}

#[test]
fn a_long_session_grows_with_its_text() {
    // 20,000 keystrokes typed a word at a time with backspaces, as the spike measured. What
    // the state holds beyond the text grows with the edits made, about a byte a keystroke (the
    // spike's 1 MiB session: 25 KB over its text after 20,000), never with the text again.
    let mut host = Peer::host(&"= Page\n\nA paragraph.\n".repeat(200), 1);
    let mut guest = Peer::join(&host, 2);
    let mut caret = host.rope.len() / 2;
    while !host.rope.is_char_boundary(caret) {
        caret += 1;
    }
    let mut bytes = 0usize;
    for keystroke in 0..20_000usize {
        let edit = if keystroke % 23 == 22 {
            let mut start = caret - 1;
            while !host.rope.is_char_boundary(start) {
                start -= 1;
            }
            ByteEdit::new(start, caret, "")
        } else {
            let key = ["a", "é", " ", "日"][keystroke % 4];
            ByteEdit::new(caret, caret, key)
        };
        caret = edit.range.start + edit.text.len();
        let update = host.edit(std::slice::from_ref(&edit));
        if edit.text == " " {
            host.replica.end_group();
        }
        bytes += update.0.len();
        guest.receive(&update);
    }
    assert_eq!(guest.rope, host.rope);
    assert!(
        bytes / 20_000 < 64,
        "{} update bytes a keystroke",
        bytes / 20_000
    );
    let state = host.replica.snapshot().unwrap().0.len();
    assert!(
        state < host.rope.len() + 2 * 20_000 + 4096,
        "{state} bytes of state for {} of text",
        host.rope.len()
    );
}
