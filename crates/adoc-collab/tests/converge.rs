//! PRD §10 test 3, `collab_replicas_converge`, and the regressions #689 asks for: replicas fed
//! seeded adversarial schedules converge once everything is delivered, in any order, with
//! duplicates, every rope matching its replica after every step.

mod common;

use adoc_collab::ByteEdit;
use adoc_collab::{CollabError, Sender};
use common::{assert_converged, random_edit, Peer, Rng, Star};

#[test]
fn collab_replicas_converge() {
    // A host and a guest, then a host and two guests: the star a session is, each link ordered
    // both ways, the host forwarding only what it has applied, every step chosen at random.
    for (guests, steps) in [(1, 40), (2, 60)] {
        for seed in 1..=1000u64 {
            let mut rng = Rng::seeded(seed);
            let mut star = Star::new("= Shared\n\n", guests);
            for _ in 0..steps {
                if rng.below(2) == 0 && star.step(&mut rng) {
                    continue;
                }
                let who = rng.below(guests + 1);
                let edit = random_edit(&mut rng, &star.peers[who].rope);
                star.edit(who, edit);
            }
            while star.step(&mut rng) {}
            assert_converged(&star.peers, seed);
        }
    }
}

#[test]
fn duplicates_and_reordering_are_refused_unread() {
    // Links are ordered and deliver once; an update arriving twice or ahead of one before it
    // is a protocol violation, refused before `yrs` reads it, and the replica is unchanged.
    let mut host = Peer::host("= Page\n", 1);
    let mut guest = Peer::join(&host, 2);
    let first = host.edit(&[ByteEdit::new(7, 7, "One.\n")]);
    let second = host.edit(&[ByteEdit::new(12, 12, "Two.\n")]);
    assert!(matches!(
        guest.replica.apply_remote(&second, Sender::Host),
        Err(CollabError::NotAdmitted(_))
    ));
    guest.receive(&first);
    guest.receive(&second);
    let before = guest.replica.text();
    assert!(matches!(
        guest.replica.apply_remote(&first, Sender::Host),
        Err(CollabError::NotAdmitted(_))
    ));
    assert_eq!(guest.replica.text(), before);
    assert!(!guest.replica.is_poisoned());
    assert_eq!(guest.rope, host.rope);
}

#[test]
fn emoji_and_composition_converge() {
    let mut host = Peer::host("Wave: \n", 1);
    let mut guest = Peer::join(&host, 2);
    // Both insert at one place: a family emoji and a flag.
    let a = host.edit(&[ByteEdit::new(6, 6, "👩\u{200d}👩\u{200d}👧")]);
    let b = guest.edit(&[ByteEdit::new(6, 6, "🇬🇧")]);
    host.receive(&b);
    guest.receive(&a);
    assert_eq!(host.rope, guest.rope);

    // IME composition: the marked text replaced again and again.
    let at = host.rope.len() - 1;
    let mut marked_len = 0;
    let mut updates = Vec::new();
    for marked in ["n", "ni", "にほ", "日本"] {
        updates.push(host.edit(&[ByteEdit::new(at, at + marked_len, marked)]));
        marked_len = marked.len();
    }
    for update in &updates {
        guest.receive(update);
    }
    assert_eq!(guest.rope, host.rope);
    assert!(guest.rope.ends_with("日本\n"));
}

#[test]
fn a_deletion_straddling_concurrent_insertions_converges() {
    let mut host = Peer::host("alpha beta gamma\n", 1);
    let mut guest = Peer::join(&host, 2);
    // The guest inserts inside "beta" and after "gamma"; the host deletes "beta gamm".
    let inserted = guest.edit(&[ByteEdit::new(8, 8, "XX"), ByteEdit::new(16, 16, "!")]);
    let deleted = host.edit(&[ByteEdit::new(6, 15, "")]);
    host.receive(&inserted);
    guest.receive(&deleted);
    assert_eq!(host.rope, guest.rope);
    assert_eq!(host.rope, "alpha XXa!\n");
}

#[test]
fn repeated_disconnects_replay_once() {
    let mut host = Peer::host("= Page\n\n", 1);
    let mut guest = Peer::join(&host, 2);
    let mut rng = Rng::seeded(7);
    for round in 0..20 {
        // Both edit while the link is down; nothing arrives.
        for _ in 0..3 {
            let edit = random_edit(&mut rng, &host.rope);
            host.edit(&[edit]);
            let edit = random_edit(&mut rng, &guest.rope);
            guest.edit(&[edit]);
        }
        // On reconnecting, each sends what the other's state vector lacks; the same again is a
        // duplicate, refused unread.
        let for_guest = host
            .replica
            .missing_since(&guest.replica.state_vector())
            .unwrap();
        let for_host = guest
            .replica
            .missing_since(&host.replica.state_vector())
            .unwrap();
        guest.receive(&for_guest);
        host.receive(&for_host);
        for (peer, again, sender) in [
            (&mut guest, &for_guest, Sender::Host),
            (&mut host, &for_host, Sender::Guest(common::id(2))),
        ] {
            // Refused if it creates anything; a deletion already made changes nothing.
            match peer.replica.apply_remote(again, sender) {
                Err(CollabError::NotAdmitted(_)) => {}
                Ok(change) => assert!(change.edits.is_empty()),
                Err(error) => panic!("{error}"),
            }
        }
        assert_eq!(host.rope, guest.rope, "round {round}");
        assert!(!host.replica.has_pending() && !guest.replica.has_pending());
    }
}

#[test]
fn a_guest_never_bootstraps_twice() {
    let host = Peer::host("= Page\r\n\r\nText.\r\n", 1);
    let mut guest = Peer::join(&host, 2);
    // The snapshot arriving again is a duplicate, refused unread.
    assert!(matches!(
        guest
            .replica
            .apply_remote(&host.replica.snapshot().unwrap(), Sender::Host),
        Err(CollabError::NotAdmitted(_))
    ));
    assert_eq!(guest.replica.text(), host.rope);
}

#[test]
fn a_guest_joins_after_a_long_session_with_deletions_and_undo() {
    // A host and two guests edit, delete, undo and redo for a while; a third guest then joins
    // from the host's snapshot, and from a guest's, through the same grammar and admission as
    // any update, and goes on receiving.
    for seed in 1..=100u64 {
        let mut rng = Rng::seeded(seed);
        let mut star = Star::new("= Shared\n\nText to start with.\n", 2);
        for _ in 0..80 {
            if rng.below(3) == 0 && star.step(&mut rng) {
                continue;
            }
            let who = rng.below(3);
            match rng.below(5) {
                0 | 1 => {
                    let peer = &mut star.peers[who];
                    let change = if rng.below(2) == 0 {
                        peer.replica.undo_local().unwrap()
                    } else {
                        peer.replica.redo_local().unwrap()
                    };
                    if let Some(change) = change {
                        peer.apply(&change);
                        star.send(who, change.outbound.unwrap());
                    }
                }
                _ => {
                    star.peers[who].replica.end_group();
                    let edit = random_edit(&mut rng, &star.peers[who].rope);
                    star.edit(who, edit);
                }
            }
        }
        while star.step(&mut rng) {}
        assert_converged(&star.peers, seed);
        let mut joined = Peer::join(&star.peers[0], 50);
        let from_guest = Peer::join(&star.peers[1], 51);
        assert_eq!(from_guest.rope, star.peers[0].rope, "seed {seed}");
        let more = star.peers[0].edit(&[ByteEdit::new(0, 0, "// later\n")]);
        joined.receive(&more);
        assert_eq!(joined.rope, star.peers[0].rope, "seed {seed}");
    }
}
