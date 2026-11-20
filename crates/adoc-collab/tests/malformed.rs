//! PRD §10 test 6, `collab_malformed_update_changes_nothing`, and the design's
//! `collab_pending_update_asks_for_resync` and `collab_authors_are_read_before_applying`:
//! untrusted bytes never change accepted text, never panic, and are seen for what they are.

mod common;

use adoc_collab::{
    authors, ByteEdit, CollabError, EncodedUpdate, Replica, Sender, StateVectorBytes,
};
use common::{id, Peer, Rng};

/// Every truncation of `update`, random byte strings, and every byte of `update` replaced by a
/// few others: what a modified peer, or a broken link, could send.
fn hostile_inputs(update: &EncodedUpdate) -> Vec<EncodedUpdate> {
    let bytes = &update.0;
    let mut inputs: Vec<Vec<u8>> = (0..bytes.len()).map(|cut| bytes[..cut].to_vec()).collect();
    let mut rng = Rng::seeded(42);
    for _ in 0..2000 {
        let length = rng.below(64);
        inputs.push((0..length).map(|_| rng.next() as u8).collect());
    }
    for at in 0..bytes.len() {
        for value in [0x00, 0x01, 0x7f, 0x80, 0xff, bytes[at] ^ 0x40] {
            let mut mutated = bytes.clone();
            mutated[at] = value;
            inputs.push(mutated);
        }
    }
    // Length prefixes claiming far more than follows.
    inputs.push(vec![0x01, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x01]);
    inputs.push(
        [
            vec![0x01, 0x01, 0x02, 0x00, 0x04],
            vec![0xff; 9],
            vec![0x7f],
        ]
        .concat(),
    );
    inputs.into_iter().map(EncodedUpdate).collect()
}

#[test]
fn collab_malformed_update_changes_nothing() {
    let mut host = Peer::host("= Page\n\nText.\n", 1);
    let guest = Peer::join(&host, 2);
    let honest = host.edit(&[ByteEdit::new(7, 7, "More. ")]);
    let accepted = guest.replica.text();
    let snapshot = guest.replica.snapshot().unwrap();

    // Offered as the host's, to a guest, and as a guest's, to the host.
    let (mut refused, mut parked, mut applied) = (0, 0, 0);
    for sender in [Sender::Host, Sender::Guest(id(1))] {
        for input in hostile_inputs(&honest) {
            let mut probe = Replica::from_snapshot(&snapshot, id(9)).unwrap();
            let state = probe.state_vector();
            match probe.apply_remote(&input, sender) {
                // Refused: nothing changed, and the replica goes on.
                Err(CollabError::MalformedUpdate(_) | CollabError::NotAdmitted(_))
                    if !probe.is_poisoned() =>
                {
                    refused += 1;
                    assert_eq!(probe.text(), accepted);
                    assert_eq!(probe.state_vector(), state);
                }
                // Accepted but parked: no text changed, and the caller is told.
                Ok(change) if change.pending => {
                    parked += 1;
                    assert!(change.edits.is_empty());
                    assert_eq!(probe.text(), accepted);
                }
                // A valid update a modified peer made: its edits rebuild the text exactly.
                Ok(change) => {
                    applied += 1;
                    let mut rope = accepted.clone();
                    adoc_collab::apply_edits(&mut rope, &change.edits);
                    assert_eq!(rope, probe.text());
                }
                // Whatever else, the replica refuses everything until rebuilt.
                Err(_) => {
                    assert!(probe.is_poisoned());
                    assert_eq!(
                        probe.apply_remote(&honest, sender).unwrap_err(),
                        CollabError::Poisoned
                    );
                }
            }
        }
    }
    assert!(refused > 4000, "{refused}");
    eprintln!("{refused} refused, {parked} parked, {applied} applied");

    // Oversized bytes are refused before decoding.
    let mut guest = guest;
    let huge = EncodedUpdate(vec![0; adoc_collab::MAX_UPDATE + 1]);
    assert_eq!(
        guest.replica.apply_remote(&huge, Sender::Host).unwrap_err(),
        CollabError::TooLarge(adoc_collab::MAX_UPDATE + 1)
    );
    // And the honest update still applies.
    guest.receive(&honest);
    assert_eq!(guest.rope, host.rope);
    // A malformed state vector is refused too.
    assert!(matches!(
        host.replica
            .missing_since(&StateVectorBytes(vec![0xff, 0xff])),
        Err(CollabError::MalformedUpdate(_))
    ));
}

#[test]
fn collab_pending_update_asks_for_resync() {
    // Admission checks each participant's run where it starts; an item's dependency on another
    // participant's item can only be seen by applying it. A host forwarding in order never sends
    // such an update, but a guest given one is told it waits and asks for a resynchronisation.
    let mut host = Peer::host("", 1);
    let mut x = Peer::join(&host, 5);
    let mut y = Peer::join(&host, 6);
    x.receive(&y.edit(&[ByteEdit::new(0, 0, "a")]));
    let depends = x.edit(&[ByteEdit::new(1, 1, "b")]);
    // The guest gets X's item without Y's, which it follows.
    let mut guest = Peer::join(&host, 2);
    let change = guest.receive(&depends);
    assert!(change.pending, "the guest is told it waits");
    assert!(change.edits.is_empty());
    // Nothing is encoded from a replica that waits: `yrs` could panic or write clocks never
    // used (y-crdt/y-crdt#720, #744).
    assert_eq!(guest.replica.snapshot().unwrap_err(), CollabError::Pending);
    // A guest resynchronises from a replica holding everything, and nothing waits any more.
    host.host = false;
    host.receive(
        &x.replica
            .missing_since(&host.replica.state_vector())
            .unwrap(),
    );
    guest.resync_from(&host);
    assert!(!guest.replica.has_pending());
    assert_eq!(guest.rope, "ab");
}

#[test]
fn a_guest_update_left_pending_makes_the_host_rebuild() {
    let mut host = Peer::host("= Page\n\nText.\n", 1);
    let mut guest = Peer::join(&host, 2);
    let honest = guest.edit(&[ByteEdit::new(7, 7, "More. ")]);
    // A modified guest's update that `yrs` accepts and parks.
    let hostile = hostile_inputs(&honest)
        .into_iter()
        .find(|input| {
            let mut probe =
                Replica::from_snapshot(&host.replica.snapshot().unwrap(), id(9)).unwrap();
            matches!(probe.apply_remote(input, Sender::Guest(id(2))), Ok(change) if change.pending)
        })
        .expect("a mutation that is parked");
    let change = host
        .replica
        .apply_remote(&hostile, Sender::Guest(id(2)))
        .unwrap();
    assert!(change.pending && change.edits.is_empty());

    // The host never asks a guest: it rebuilds the file's replica from its own text, in a new
    // epoch under a new id, and the guests start again from its snapshot.
    let mut rebuilt = Peer::host(&host.rope, 3);
    assert!(!rebuilt.replica.has_pending());
    let mut rejoined = Peer::join(&rebuilt, 4);
    rejoined.receive(&rebuilt.edit(&[ByteEdit::new(0, 0, "// Still editing.\n")]));
    assert_eq!(rejoined.rope, rebuilt.rope);
}

#[test]
fn collab_authors_are_read_before_applying() {
    let mut host = Peer::host("= Page\n", 1);
    let mut guest = Peer::join(&host, 2);
    let own = guest.edit(&[ByteEdit::new(7, 7, "Text.\n")]);
    assert_eq!(authors(&own).unwrap(), vec![id(2)]);
    let deletion = guest.edit(&[ByteEdit::new(0, 2, "")]);
    assert!(authors(&deletion).unwrap().is_empty());
    // The host's own update, passed off as the guest's, names the host.
    let from_host = host.edit(&[ByteEdit::new(0, 0, "x")]);
    assert_eq!(authors(&from_host).unwrap(), vec![id(1)]);
    assert!(authors(&EncodedUpdate(vec![0xff, 0xff])).is_err());
    // Reading the authors changes no replica.
    assert_eq!(host.rope, host.replica.text());
}

#[test]
fn a_client_id_wider_than_53_bits_is_refused_or_harmless() {
    // Found by the `collab_update` fuzz target: with debug assertions, `yrs` panics decoding a
    // client id wider than 53 bits; without them it masks the id. Either way the replica's
    // guard leaves it unpoisoned and its text unchanged.
    let host = Peer::host("= Page\n", 1);
    let mut guest = Peer::join(&host, 2);
    let before = guest.replica.text();
    match guest
        .replica
        .apply_remote(&EncodedUpdate(vec![0xb0, 0x25, 0x04, 0x0a]), Sender::Host)
    {
        Ok(change) => assert!(change.edits.is_empty()),
        Err(error) => assert!(matches!(error, CollabError::MalformedUpdate(_)), "{error}"),
    }
    assert!(!guest.replica.is_poisoned());
    assert_eq!(guest.replica.text(), before);
    let _ = authors(&EncodedUpdate(vec![0xb0, 0x25, 0x04, 0x0a]));
}

#[test]
fn counts_claiming_more_than_follows_are_refused() {
    // Found by the `collab_update` fuzz target: `yrs` reserves room for as many entries as an
    // update's counts claim before reading them, with `try_reserve`, so a few bytes can ask for
    // tens or hundreds of gigabytes. The request either fails, as an error, or reserves address
    // space that is never touched; then decoding runs out of input. Neither aborts the process
    // nor touches the replica (`docs/design/DESIGN-AdocFoundry_Collaboration.md`, §6).
    let host = Peer::host("= Page\n", 1);
    let mut guest = Peer::join(&host, 2);
    let before = guest.replica.text();
    for bytes in [
        vec![0xff, 0xdd, 0xf4, 0x56, 0xea, 0x02],
        vec![0xc0, 0xc0, 0xc0, 0xc0, 0x01],
        vec![0xfb, 0xa8, 0xa8, 0xa8, 0xa8, 0x7e],
    ] {
        let update = EncodedUpdate(bytes);
        assert!(authors(&update).is_err());
        assert!(matches!(
            guest.replica.apply_remote(&update, Sender::Host),
            Err(CollabError::MalformedUpdate(_))
        ));
        assert!(!guest.replica.is_poisoned());
        assert_eq!(guest.replica.text(), before);
    }
}

#[test]
fn an_update_splitting_a_character_poisons_the_replica() {
    // Found by the `collab_update` fuzz target: one byte changed in an honest update makes `yrs`
    // integrate an item at a position inside the 👋 already in the text (it counts in UTF-16
    // units), splitting it into replacement characters the reported edits do not show. The
    // replica compares the text whole and refuses to go on with a text its rope cannot match.
    let host = Peer::host("= Page\n\nUn café 👋\r\nMore.\n", 1);
    let snapshot = host.replica.snapshot().unwrap();
    let mut author = Replica::from_snapshot(&snapshot, id(2)).unwrap();
    let mut honest = author
        .apply_local(&[ByteEdit::new(8, 17, "Thé 🇬🇧")])
        .unwrap()
        .outbound
        .unwrap();
    // The fuzzer's input: byte 0x8f % len set to 0x00, then 0x06 % len set to 0x10.
    let len = honest.0.len();
    honest.0[0x8f % len] = 0x00;
    honest.0[0x06 % len] = 0x10;
    let mut guest = Peer::join(&host, 3);
    assert_eq!(
        guest
            .replica
            .apply_remote(&honest, Sender::Guest(id(2)))
            .unwrap_err(),
        CollabError::ProjectionMismatch
    );
    assert!(guest.replica.is_poisoned());
    // The host's own text, which its engine holds, is what the file's replica is rebuilt from.
    let rebuilt = Peer::host(&guest.rope, 4);
    assert_eq!(rebuilt.rope, "= Page\n\nUn café 👋\r\nMore.\n");
}

#[test]
fn a_snapshot_that_would_rebuild_another_text_is_refused() {
    // Found by the `collab_update` fuzz target: an honest update with one byte changed passes
    // the grammar, admission and the whole-text check, and is applied consistently, but the
    // receiver's own snapshot then rebuilds a different text. Whoever joined from it would
    // diverge, so a snapshot is checked by rebuilding it, and refused when it differs; the host
    // then rebuilds its replica from its own text.
    let host = Peer::host("= Page\n\nUn café 👋\r\nMore.\n", 1);
    let snapshot = host.replica.snapshot().unwrap();
    let mut author = Replica::from_snapshot(&snapshot, id(2)).unwrap();
    let mut update = author
        .apply_local(&[ByteEdit::new(8, 17, "Thé 🇬🇧")])
        .unwrap()
        .outbound
        .unwrap();
    let len = update.0.len();
    update.0[8 % len] = 0x01;
    for sender in [Sender::Guest(id(2)), Sender::Host] {
        let mut receiver = Replica::from_snapshot(&snapshot, id(3)).unwrap();
        if receiver.apply_remote(&update, sender).is_err() {
            continue;
        }
        match receiver.snapshot() {
            Ok(snapshot) => {
                let copy = Replica::from_snapshot(&snapshot, id(4)).unwrap();
                assert_eq!(copy.text(), receiver.text(), "{sender:?}");
            }
            Err(error) => assert_eq!(error, CollabError::ProjectionMismatch, "{sender:?}"),
        }
    }
}
