//! Selections that follow their text across replicas, and the presence throttle.

mod common;

use std::time::{Duration, Instant};

use adoc_collab::{Affinity, AwarenessThrottle, ByteEdit, CollabError, RelativeSelection};
use common::Peer;

#[test]
fn a_selection_follows_its_text_on_every_replica() {
    let mut alice = Peer::host("one two three\n", 1);
    let mut bob = Peer::join(&alice, 2);
    // Bob selects "two"; Alice resolves his selection where her text has it.
    let selection = bob.replica.selection(4, 7, Affinity::After).unwrap();
    assert_eq!(alice.replica.resolve(&selection), Some((4, 7)));
    // Edits before it, an emoji among them, move it on both replicas.
    bob.receive(&alice.edit(&[ByteEdit::new(0, 0, "zero 👋 ")]));
    alice.receive(&bob.edit(&[ByteEdit::new(3, 3, ",")]));
    for peer in [&alice, &bob] {
        let (start, end) = peer.replica.resolve(&selection).unwrap();
        assert_eq!(&peer.rope[start..end], "two");
    }
}

#[test]
fn a_caret_inside_a_replaced_paragraph_lands_at_its_edge() {
    let mut alice = Peer::host("Before.\n\nOld paragraph.\n\nAfter.\n", 1);
    let mut bob = Peer::join(&alice, 2);
    let caret = bob.replica.selection(13, 13, Affinity::After).unwrap();
    // Alice replaces the whole paragraph Bob's caret was in.
    bob.receive(&alice.edit(&[ByteEdit::new(9, 23, "New text.")]));
    let (start, end) = bob.replica.resolve(&caret).unwrap();
    assert_eq!(start, end);
    assert!(bob.rope.is_char_boundary(start));
    assert!((9..=18).contains(&start), "{start}");
}

#[test]
fn affinity_decides_which_side_of_an_insertion_an_anchor_keeps() {
    let mut alice = Peer::host("ab", 1);
    let bob = Peer::join(&alice, 2);
    let before = bob.replica.selection(1, 1, Affinity::Before).unwrap();
    let after = bob.replica.selection(1, 1, Affinity::After).unwrap();
    alice.edit(&[ByteEdit::new(1, 1, "XYZ")]);
    assert_eq!(alice.replica.resolve(&before), Some((1, 1)));
    assert_eq!(alice.replica.resolve(&after), Some((4, 4)));
}

#[test]
fn selections_from_the_wire_are_checked() {
    let alice = Peer::host("text", 1);
    let selection = alice.replica.selection(0, 4, Affinity::After).unwrap();
    let back =
        RelativeSelection::from_wire(selection.anchor.clone(), selection.head.clone()).unwrap();
    assert_eq!(back, selection);
    assert!(matches!(
        RelativeSelection::from_wire(vec![0xff; 3], selection.head),
        Err(CollabError::MalformedUpdate(_))
    ));
    assert_eq!(
        alice.replica.selection(0, 5, Affinity::After).unwrap_err(),
        CollabError::InvalidBoundary(5)
    );
}

#[test]
fn presence_is_paced() {
    let start = Instant::now();
    let mut throttle = AwarenessThrottle::new(Duration::from_millis(50));
    assert!(throttle.offer(start), "the first goes at once");
    assert!(!throttle.offer(start + Duration::from_millis(10)));
    assert!(
        !throttle.offer(start + Duration::from_millis(20)),
        "the latest waits"
    );
    assert_eq!(throttle.due(), Some(start + Duration::from_millis(50)));
    assert!(!throttle.take_due(start + Duration::from_millis(49)));
    assert!(throttle.take_due(start + Duration::from_millis(50)));
    assert_eq!(throttle.due(), None, "one message for the two that waited");
    assert!(throttle.offer(start + Duration::from_millis(120)));
}

#[test]
fn a_selection_naming_a_participant_wider_than_53_bits_is_refused() {
    // Found by the `collab_update` fuzz target: decoding such an end trips a `debug_assert!` in
    // `yrs`. Read inside the guard, it is refused in a build with debug assertions; without
    // them `yrs` masks the id, and a selection it reads still never resolves past the text.
    let anchor = [
        0x00, 0x9e, 0xdc, 0xdc, 0x86, 0xf8, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3d,
    ];
    let head = [
        0x7e, 0x9e, 0xff, 0xff, 0xff, 0xff, 0xff, 0xdc, 0xdc, 0x86, 0xf8, 0x24,
    ];
    let peer = Peer::host("= Page\n", 1);
    match RelativeSelection::from_wire(anchor.to_vec(), head.to_vec()) {
        Err(error) => assert!(matches!(error, CollabError::MalformedUpdate(_)), "{error}"),
        Ok(selection) => {
            if let Some((start, end)) = peer.replica.resolve(&selection) {
                assert!(start <= peer.rope.len() && end <= peer.rope.len());
            }
        }
    }
    if cfg!(debug_assertions) {
        assert!(RelativeSelection::from_wire(anchor.to_vec(), head.to_vec()).is_err());
    }
}
