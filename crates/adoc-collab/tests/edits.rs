//! PRD §10 test 4, `collab_edits_are_byte_exact`, and the design's
//! `collab_offsets_inside_a_character_are_refused`: byte edits in and out are ordered,
//! non-overlapping, on character boundaries and minimal, and the text is held exactly.

mod common;

use adoc_collab::{ByteEdit, ChangeOrigin, CollabError, Replica};
use common::{id, Peer};

#[test]
fn collab_edits_are_byte_exact() {
    let mut host = Peer::host("= Page\r\n\r\nUn café, déjà vu.\r\n", 1);
    let mut guest = Peer::join(&host, 2);

    // A keystroke is one edit at its place, locally and remotely: never the whole text.
    let at = host.rope.find("vu").unwrap();
    let change = host
        .replica
        .apply_local(&[ByteEdit::new(at, at, "x")])
        .unwrap();
    assert_eq!(change.origin, ChangeOrigin::Local);
    assert_eq!(change.edits, vec![ByteEdit::new(at, at, "x")]);
    host.apply(&change);
    let received = guest.receive(&change.outbound.unwrap());
    assert_eq!(received.origin, ChangeOrigin::Remote);
    assert_eq!(received.edits, vec![ByteEdit::new(at, at, "x")]);
    assert!(
        received.outbound.is_none(),
        "a remote change is never sent on"
    );

    // A batch in pre-edit coordinates: a replacement of a multibyte word and an insertion after
    // it, received as the same batch.
    let cafe = host.rope.find("café").unwrap();
    let end = host.rope.len() - 2;
    let batch = [
        ByteEdit::new(cafe, cafe + "café".len(), "thé"),
        ByteEdit::new(end, end, " 👋"),
    ];
    let update = host.edit(&batch);
    let received = guest.receive(&update);
    assert_eq!(received.edits, batch.to_vec());

    // Deleting across a line ending, and CRLF kept as it is.
    let update = host.edit(&[ByteEdit::new(6, 10, "\n")]);
    let received = guest.receive(&update);
    assert_eq!(received.edits, vec![ByteEdit::new(6, 10, "\n")]);
    assert_eq!(guest.rope, host.rope);
    assert!(guest.rope.ends_with("\r\n"));

    // Edits that touch are reported as one; no-ops are dropped.
    let change = host
        .replica
        .apply_local(&[
            ByteEdit::new(0, 1, "#"),
            ByteEdit::new(1, 1, "#"),
            ByteEdit::new(4, 4, ""),
        ])
        .unwrap();
    assert_eq!(change.edits, vec![ByteEdit::new(0, 1, "##")]);
    host.apply(&change);
    guest.receive(&change.outbound.unwrap());
    assert_eq!(guest.rope, host.rope);

    // A batch of nothing changes nothing and sends nothing.
    let change = host
        .replica
        .apply_local(&[ByteEdit::new(3, 3, "")])
        .unwrap();
    assert!(change.edits.is_empty() && change.outbound.is_none());
}

#[test]
fn the_text_is_held_exactly() {
    // A byte order mark, decomposed characters, CRLF and a lone CR are never rewritten.
    let text = "\u{feff}= Pa\u{301}ge\r\n\rA line\n";
    let host = Peer::host(text, 1);
    let guest = Peer::join(&host, 2);
    assert_eq!(guest.replica.text(), text);
    assert_eq!(guest.replica.len(), text.len());
}

#[test]
fn collab_offsets_inside_a_character_are_refused() {
    let mut replica = Replica::new("é👋\r\n", id(1)).unwrap();
    // Order, direction and bounds are this crate's to check; character boundaries the caller's
    // (the engine checks against its rope), and `check_boundaries` does both.
    for (edits, error) in [
        (
            vec![ByteEdit::new(0, 9, "")],
            CollabError::InvalidBoundary(9),
        ),
        (vec![ByteEdit::new(3, 2, "")], CollabError::Unordered),
        (
            vec![ByteEdit::new(4, 4, "a"), ByteEdit::new(0, 0, "b")],
            CollabError::Unordered,
        ),
    ] {
        assert_eq!(replica.apply_local(&edits).unwrap_err(), error);
    }
    for (edits, offset) in [
        (vec![ByteEdit::new(1, 1, "x")], 1),
        (vec![ByteEdit::new(0, 1, "")], 1),
        (vec![ByteEdit::new(3, 3, "x")], 3),
    ] {
        assert_eq!(
            adoc_collab::check_boundaries(&replica.text(), &edits),
            Err(CollabError::InvalidBoundary(offset))
        );
    }
    assert_eq!(replica.text(), "é👋\r\n", "a refused batch changes nothing");
    assert!(!replica.is_poisoned());
}

#[test]
fn an_unchecked_split_character_fails_closed() {
    // A caller that skips the boundary check: `yrs` moves the insertion to the next boundary,
    // the replica sees that what changed is not what was asked, and refuses everything until it
    // is rebuilt, rather than letting the rope and the shared text drift apart.
    let mut replica = Replica::new("é", id(1)).unwrap();
    assert_eq!(
        replica
            .apply_local(&[ByteEdit::new(1, 1, "x")])
            .unwrap_err(),
        CollabError::ProjectionMismatch
    );
    assert!(replica.is_poisoned());
    assert_eq!(
        replica
            .apply_local(&[ByteEdit::new(0, 0, "x")])
            .unwrap_err(),
        CollabError::Poisoned
    );
}
