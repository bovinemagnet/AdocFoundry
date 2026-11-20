//! The acceptance tests of COLLAB 1/8 (#688, PRD §10 tests 1 and 2) and the probes behind
//! `docs/qa/spike-collab.md`. Replicas meet only through encoded bytes on a fake transport that
//! drops, duplicates, reorders and delays them; each peer keeps a plain `String` as the engine
//! keeps its rope, changed only by the byte edits its replica reports.

use adocfoundry_collab_spike::{apply_edits, authors, Edit, Replica, SpikeError};
use yrs::Assoc;

/// The largest update a replica accepts in these tests.
const LIMIT: usize = 4 << 20;

/// A small deterministic generator, so every schedule is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A participant: its replica and the text its engine would hold.
struct Peer {
    replica: Replica,
    rope: String,
}

impl Peer {
    fn new(client: u64) -> Self {
        Self {
            replica: Replica::new(client),
            rope: String::new(),
        }
    }

    fn edit(&mut self, edits: &[Edit]) -> Vec<u8> {
        let update = self.replica.apply_local(edits).unwrap();
        apply_edits(&mut self.rope, edits);
        self.check();
        update
    }

    fn receive(&mut self, update: &[u8]) -> Vec<Edit> {
        let edits = self.replica.apply_remote(update, LIMIT).unwrap();
        apply_edits(&mut self.rope, &edits);
        self.check();
        edits
    }

    fn undo(&mut self) -> Vec<u8> {
        let (edits, update) = self.replica.undo().expect("something to undo");
        apply_edits(&mut self.rope, &edits);
        self.check();
        update
    }

    fn redo(&mut self) -> Vec<u8> {
        let (edits, update) = self.replica.redo().expect("something to redo");
        apply_edits(&mut self.rope, &edits);
        self.check();
        update
    }

    /// The rope, rebuilt from byte edits alone, is the replica's text.
    fn check(&self) {
        assert_eq!(self.rope, self.replica.text());
    }
}

/// A message in flight: who it is for, the session epoch it was sent in, and the update.
struct Message {
    to: usize,
    epoch: u32,
    update: Vec<u8>,
}

/// The fake transport: everything sent waits here until delivered, in any order, some twice,
/// unless `in_order` or `no_duplicates` says otherwise.
#[derive(Default)]
struct Net {
    queue: Vec<Message>,
    in_order: bool,
    no_duplicates: bool,
}

impl Net {
    fn send(&mut self, from: usize, peers: usize, epoch: u32, update: &[u8]) {
        if update.is_empty() {
            return;
        }
        for to in (0..peers).filter(|to| *to != from) {
            self.queue.push(Message {
                to,
                epoch,
                update: update.to_vec(),
            });
        }
    }

    /// Delivers one message chosen at random, sometimes leaving a copy behind to arrive again.
    /// A message from another epoch is refused before its bytes are read.
    fn deliver_one(&mut self, rng: &mut Rng, peers: &mut [Peer], epoch: u32) {
        let index = if self.in_order {
            0
        } else {
            rng.below(self.queue.len())
        };
        let message = if !self.no_duplicates && rng.below(5) == 0 {
            let message = &self.queue[index];
            Message {
                to: message.to,
                epoch: message.epoch,
                update: message.update.clone(),
            }
        } else if self.in_order {
            self.queue.remove(index)
        } else {
            self.queue.swap_remove(index)
        };
        if message.epoch == epoch {
            peers[message.to].receive(&message.update);
        }
    }

    fn drain(&mut self, rng: &mut Rng, peers: &mut [Peer], epoch: u32) {
        while !self.queue.is_empty() {
            self.deliver_one(rng, peers, epoch);
        }
    }
}

/// Text chosen to land on hard places: emoji sequences, combining marks, wide characters and
/// line endings of both kinds.
const PIECES: &[&str] = &[
    "a",
    "word ",
    "é",
    "e\u{301}",
    "👩\u{200d}👩\u{200d}👧",
    "🇬🇧",
    "日本語",
    "\r\n",
    "\n",
    "== Title\n",
    "*bold*",
];

/// ASCII alone, to show a finding is not about Unicode.
const ASCII: &[&str] = &["a", "word ", "\n", "== Title\n", "*bold*"];

/// A random edit on character boundaries of `text`, inserting one of `pieces`.
fn random_edit(rng: &mut Rng, text: &str, pieces: &[&str]) -> Edit {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    let start = boundaries[rng.below(boundaries.len())];
    let after: Vec<usize> = boundaries
        .iter()
        .copied()
        .filter(|at| *at >= start)
        .collect();
    let end = if rng.below(3) == 0 {
        after[rng.below(after.len().min(6))]
    } else {
        start
    };
    let text = if end > start && rng.below(2) == 0 {
        ""
    } else {
        pieces[rng.below(pieces.len())]
    };
    Edit::new(start..end, text)
}

/// Edits at random by random peers, `steps` times, delivering something in between at random.
fn random_session(rng: &mut Rng, peers: &mut [Peer], net: &mut Net, steps: usize, pieces: &[&str]) {
    for _ in 0..steps {
        if !net.queue.is_empty() && rng.below(2) == 0 {
            net.deliver_one(rng, peers, 1);
            continue;
        }
        let who = rng.below(peers.len());
        let edit = random_edit(rng, &peers[who].rope, pieces);
        let update = peers[who].edit(&[edit]);
        net.send(who, peers.len(), 1, &update);
    }
}

/// Every peer holds the same text and nothing pending.
fn assert_converged(peers: &[Peer], seed: u64) {
    for peer in peers {
        assert_eq!(peer.rope, peers[0].rope, "seed {seed}");
        assert!(!peer.replica.has_pending(), "seed {seed}");
    }
}

/// A host (peer 0) and guests, each guest's link an ordered queue each way.
struct Star {
    peers: Vec<Peer>,
    down: Vec<std::collections::VecDeque<Vec<u8>>>,
    up: Vec<std::collections::VecDeque<Vec<u8>>>,
}

impl Star {
    fn new(peers: usize) -> Self {
        Self {
            peers: (0..peers as u64)
                .map(|client| Peer::new(client + 10))
                .collect(),
            down: vec![Default::default(); peers],
            up: vec![Default::default(); peers],
        }
    }

    fn edit(&mut self, who: usize, edit: Edit) {
        let update = self.peers[who].edit(&[edit]);
        if who == 0 {
            for guest in 1..self.peers.len() {
                self.down[guest].push_back(update.clone());
            }
        } else {
            self.up[who].push_back(update);
        }
    }

    /// Delivers the next message on one busy link chosen at random; false when none is busy.
    fn step(&mut self, rng: &mut Rng) -> bool {
        let busy: Vec<(bool, usize)> = (1..self.peers.len())
            .flat_map(|guest| [(true, guest), (false, guest)])
            .filter(|(down, guest)| {
                let links = if *down { &self.down } else { &self.up };
                !links[*guest].is_empty()
            })
            .collect();
        if busy.is_empty() {
            return false;
        }
        let (down, guest) = busy[rng.below(busy.len())];
        if down {
            let update = self.down[guest].pop_front().unwrap();
            self.peers[guest].receive(&update);
        } else {
            let update = self.up[guest].pop_front().unwrap();
            self.peers[0].receive(&update);
            for other in (1..self.peers.len()).filter(|other| *other != guest) {
                self.down[other].push_back(update.clone());
            }
        }
        true
    }
}

#[test]
fn collab_spike_replicas_converge() {
    // Concurrent insertions at one offset, and an insertion inside a concurrent deletion.
    let mut peers = [Peer::new(1), Peer::new(2)];
    let seed = peers[0].edit(&[Edit::new(0..0, "== Intro\r\nSome text.\n")]);
    peers[1].receive(&seed);
    let a = peers[0].edit(&[Edit::new(9..9, "Hello ")]);
    let b = peers[1].edit(&[Edit::new(9..9, "Bonjour ")]);
    let c = peers[1].edit(&[Edit::new(10..15, "")]);
    let d = peers[0].edit(&[Edit::new(17..17, "🇬🇧")]);
    for update in [&c, &b] {
        peers[0].receive(update);
    }
    for update in [&d, &a] {
        peers[1].receive(update);
    }
    assert_eq!(peers[0].rope, peers[1].rope);

    // IME-style composition: the marked text replaced again and again, then committed.
    let at = peers[0].rope.len();
    let mut composing = 0..0;
    let mut updates = Vec::new();
    for marked in ["n", "ni", "にほ", "日本"] {
        let edit = Edit::new(at + composing.start..at + composing.end, marked);
        updates.push(peers[0].edit(&[edit]));
        composing = 0..marked.len();
    }
    for update in updates.iter().rev() {
        peers[1].receive(update);
    }
    assert_eq!(peers[0].rope, peers[1].rope);
    assert!(peers[1].rope.ends_with("日本"));

    // Seeded schedules, every rope checked after every step. Two replicas, their updates
    // reordered, duplicated and delayed, converge as soon as everything has arrived.
    for seed in 1..=2000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut peers = vec![Peer::new(10), Peer::new(11)];
        let mut net = Net::default();
        random_session(&mut rng, &mut peers, &mut net, 40, PIECES);
        net.drain(&mut rng, &mut peers, 1);
        assert_converged(&peers, seed);
    }

    // Three replicas in a star, as a session runs: each guest's link to the host is ordered
    // both ways, and the host passes a guest's update on only once it has applied it.
    for seed in 1..=2000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut star = Star::new(3);
        for _ in 0..60 {
            if rng.below(2) == 0 && star.step(&mut rng) {
                continue;
            }
            let who = rng.below(3);
            let edit = random_edit(&mut rng, &star.peers[who].rope, PIECES);
            star.edit(who, edit);
        }
        while star.step(&mut rng) {}
        assert_converged(&star.peers, seed);
    }

    // Three replicas each sending straight to the others, out of causal order: `yrs` 0.28.2 can
    // leave an update pending whose dependencies have all arrived (the spike record's finding 1).
    // A replica left with something pending resynchronises by state vector, and all converge.
    let mut resynchronised = 0;
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut peers: Vec<Peer> = (0..3).map(|client| Peer::new(client + 10)).collect();
        let mut net = Net::default();
        random_session(&mut rng, &mut peers, &mut net, 40, PIECES);
        net.drain(&mut rng, &mut peers, 1);
        for stuck in 0..peers.len() {
            if peers[stuck].replica.has_pending() {
                resynchronised += 1;
                for other in (0..peers.len()).filter(|other| *other != stuck) {
                    let missing = peers[other]
                        .replica
                        .missing_since(&peers[stuck].replica.state_vector())
                        .unwrap();
                    peers[stuck].receive(&missing);
                }
            }
        }
        assert_converged(&peers, seed);
    }
    eprintln!("PROBE three-way mesh: {resynchronised} of 300 sessions needed a resync");

    // Reconnect within the epoch: both sides lost updates when the link dropped, then each sends
    // what the other's state vector lacks, and both catch up exactly once; the same catch-up
    // again changes nothing.
    let mut host = Peer::new(1);
    let mut guest = Peer::new(2);
    guest.receive(&host.edit(&[Edit::new(0..0, "= Page\n\nFirst.\n")]));
    let _lost = host.edit(&[Edit::new(15..15, "Second.\n")]);
    let _lost = host.edit(&[Edit::new(0..0, "// note\n")]);
    let _lost = guest.edit(&[Edit::new(8..8, "Typed before the drop. ")]);
    let for_guest = host
        .replica
        .missing_since(&guest.replica.state_vector())
        .unwrap();
    let for_host = guest
        .replica
        .missing_since(&host.replica.state_vector())
        .unwrap();
    assert!(!guest.receive(&for_guest).is_empty());
    assert!(!host.receive(&for_host).is_empty());
    assert_eq!(guest.rope, host.rope);
    assert!(guest.rope.contains("Typed before the drop.") && guest.rope.contains("Second."));
    assert!(guest.receive(&for_guest).is_empty(), "applied twice");
    assert!(host.receive(&for_host).is_empty(), "applied twice");
    let again = host
        .replica
        .missing_since(&guest.replica.state_vector())
        .unwrap();
    assert!(guest.receive(&again).is_empty());

    // A message from an earlier epoch is refused unread: after the host restarts sharing, the
    // guest's old update changes nothing.
    let mut net = Net::default();
    let mut peers = vec![host, guest];
    let stale = peers[1]
        .replica
        .apply_local(&[Edit::new(0..0, "stale ")])
        .unwrap();
    net.queue.push(Message {
        to: 0,
        epoch: 1,
        update: stale,
    });
    let before = peers[0].rope.clone();
    net.drain(&mut Rng(7), &mut peers, 2);
    assert_eq!(peers[0].rope, before);

    // A snapshot starts a new replica with the same text.
    let mut joined = Peer::new(3);
    joined.receive(&peers[0].replica.snapshot());
    assert_eq!(joined.rope, peers[0].rope);
}

#[test]
fn collab_spike_undo_is_per_author() {
    let mut alice = Peer::new(1);
    let mut bob = Peer::new(2);
    bob.receive(&alice.edit(&[Edit::new(0..0, "Para.\n")]));
    alice.replica.end_group();

    // Alice types a word; Bob inserts text inside it; Alice undoes her word.
    bob.receive(&alice.edit(&[Edit::new(4..4, " wonderful")]));
    alice.replica.end_group();
    alice.receive(&bob.edit(&[Edit::new(8..8, "XX")]));
    assert_eq!(alice.rope, "Para wonXXderful.\n");

    bob.receive(&alice.undo());
    assert_eq!(alice.rope, "ParaXX.\n", "Bob's text stays where it was");
    assert_eq!(bob.rope, alice.rope);

    // Redo puts Alice's word back around Bob's text, never deleting it.
    bob.receive(&alice.redo());
    assert_eq!(alice.rope, "Para wonXXderful.\n");
    assert_eq!(bob.rope, alice.rope);

    // Alice's next undo takes her word again, then her first line; Bob's text is never hers.
    bob.receive(&alice.undo());
    bob.receive(&alice.undo());
    assert_eq!(alice.rope, "XX");
    assert!(alice.replica.undo().is_none());
    assert_eq!(bob.rope, alice.rope);

    // Bob's own history holds only his insertion.
    alice.receive(&bob.undo());
    assert_eq!(bob.rope, "");
    assert!(bob.replica.undo().is_none());
    assert_eq!(alice.rope, bob.rope);
}

#[test]
fn offsets_inside_a_character_are_refused_before_yrs_sees_them() {
    let mut peer = Peer::new(1);
    peer.edit(&[Edit::new(0..0, "é👩\u{200d}👧\r\n")]);
    let before = peer.replica.text();
    for edit in [
        Edit::new(1..1, "x"),
        Edit::new(0..1, ""),
        Edit::new(3..3, "x"),
        Edit::new(0..100, ""),
    ] {
        assert!(matches!(
            peer.replica.apply_local(&[edit]),
            Err(SpikeError::InvalidBoundary(_))
        ));
    }
    assert!(matches!(
        peer.replica
            .apply_local(&[Edit::new(2..2, "a"), Edit::new(0..0, "b")]),
        Err(SpikeError::Unordered)
    ));
    assert_eq!(
        peer.replica.text(),
        before,
        "a refused batch changes nothing"
    );
}

/// What `yrs` itself does with an offset inside a character, which the replica never gives it.
#[test]
fn probe_yrs_with_an_offset_inside_a_character() {
    use yrs::{Doc, GetString, OffsetKind, Options, Text, Transact};
    let outcome = std::panic::catch_unwind(|| {
        let mut options = Options::with_client_id(yrs::ClientID::new(1));
        options.offset_kind = OffsetKind::Bytes;
        let doc = Doc::with_options(options);
        let text = doc.get_or_insert_text("text");
        text.insert(&mut doc.transact_mut(), 0, "é");
        text.insert(&mut doc.transact_mut(), 1, "x");
        let string = text.get_string(&doc.transact());
        (string.clone(), string.len())
    });
    eprintln!("PROBE yrs insert at byte 1 of \"é\": {outcome:?}");
}

/// Whether the precheck refuses `input` against `replica`'s state.
fn flagged(replica: &Replica, input: &[u8]) -> bool {
    !matches!(replica.precheck(input), Ok(true))
}

#[test]
fn malformed_updates_change_nothing() {
    let mut host = Peer::new(1);
    let mut guest = Peer::new(2);
    guest.receive(&host.edit(&[Edit::new(0..0, "= Page\n\nText.\n")]));
    let update = host.edit(&[Edit::new(7..7, "More. ")]);
    let before = guest.replica.text();

    // Every truncation of a real update, and random bytes.
    let mut outcomes = std::collections::BTreeMap::<&str, usize>::new();
    let mut inputs: Vec<Vec<u8>> = (0..update.len())
        .map(|cut| update[..cut].to_vec())
        .collect();
    let mut rng = Rng(42);
    for _ in 0..5_000 {
        let length = rng.below(64);
        inputs.push((0..length).map(|_| rng.next() as u8).collect());
    }
    // Every byte of a real update replaced by a few others.
    for at in 0..update.len() {
        for value in [0x00, 0x01, 0x7f, 0x80, 0xff, update[at] ^ 0x40] {
            let mut mutated = update.clone();
            mutated[at] = value;
            inputs.push(mutated);
        }
    }
    // A varint length prefix claiming far more than follows.
    inputs.push(vec![0x01, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x01]);
    inputs.push(
        [
            vec![0x01, 0x01, 0x02, 0x00, 0x04],
            vec![0xff; 9],
            vec![0x7f],
        ]
        .concat(),
    );
    for input in &inputs {
        let mut probe = Replica::new(9);
        probe
            .apply_remote(&guest.replica.snapshot(), LIMIT)
            .unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            probe.apply_remote(input, LIMIT)
        }));
        let kind = match outcome {
            Ok(Ok(_)) if probe.has_pending() && !flagged(&guest.replica, input) => {
                "accepted, left pending, not caught by the precheck"
            }
            Ok(Ok(_)) if probe.has_pending() => "accepted, left pending, caught by the precheck",
            Ok(Ok(edits)) if edits.is_empty() => "accepted, no change",
            Ok(Ok(_)) => "accepted, changed",
            Ok(Err(SpikeError::MalformedUpdate(_))) => "refused",
            Ok(Err(_)) => "refused otherwise",
            Err(_) => "panicked",
        };
        *outcomes.entry(kind).or_default() += 1;
    }
    eprintln!(
        "PROBE malformed updates ({} inputs): {outcomes:?}",
        inputs.len()
    );

    // An oversized update is refused before it is decoded.
    assert_eq!(
        guest.replica.apply_remote(&[0; 64], 32),
        Err(SpikeError::TooLarge(64))
    );
    assert_eq!(guest.replica.text(), before);
    guest.receive(&update);
    assert_eq!(guest.rope, host.rope);
}

#[test]
fn anchors_follow_their_text() {
    let mut alice = Peer::new(1);
    let mut bob = Peer::new(2);
    bob.receive(&alice.edit(&[Edit::new(0..0, "one two three")]));
    // Bob's caret before "three", sent to Alice as an anchor.
    let caret = bob.replica.anchor(8, Assoc::After);
    assert_eq!(alice.replica.resolve(&caret), Some(8));
    bob.receive(&alice.edit(&[Edit::new(0..0, "zero 👋 ")]));
    alice.receive(&bob.edit(&[Edit::new(4..4, ",")]));
    assert_eq!(
        &alice.rope[alice.replica.resolve(&caret).unwrap()..],
        "three"
    );
    assert_eq!(&bob.rope[bob.replica.resolve(&caret).unwrap()..], "three");
}

#[test]
fn an_update_names_its_authors_before_it_is_applied() {
    let mut host = Peer::new(1);
    let mut guest = Peer::new(2);
    let seed = host.edit(&[Edit::new(0..0, "= Page\n")]);
    guest.receive(&seed);
    let own = guest.edit(&[Edit::new(7..7, "Text.\n")]);
    assert_eq!(authors(&own).unwrap(), vec![2]);
    // A deletion of the host's text creates nothing, so names no author.
    let deletion = guest.edit(&[Edit::new(0..2, "")]);
    assert_eq!(authors(&deletion).unwrap(), Vec::<u64>::new());
    // The host's own update passed off as the guest's names the host.
    assert_eq!(authors(&seed).unwrap(), vec![1]);
    assert!(authors(&[0xff, 0xff]).is_err());
}

/// The record's finding 1, reproducible: how many of 2,000 three-replica sessions, each replica
/// sending straight to the others, end with an update `yrs` left pending, by text and delivery.
/// Ignored, since it asserts nothing: `cargo test --release -- --ignored --nocapture probe_pending`.
#[test]
#[ignore = "a probe: run with --ignored --nocapture"]
fn probe_pending_needs_three_replicas_out_of_causal_order() {
    const SESSIONS: u64 = 2000;
    for (name, pieces) in [("Unicode", PIECES), ("ASCII", ASCII)] {
        for (delivery, in_order, no_duplicates) in [
            ("reordered and duplicated", false, false),
            ("reordered", false, true),
            ("in the order sent", true, true),
        ] {
            let mut pending = 0;
            for seed in 1..=SESSIONS {
                let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
                let mut peers: Vec<Peer> = (0..3).map(|client| Peer::new(client + 10)).collect();
                let mut net = Net {
                    in_order,
                    no_duplicates,
                    ..Net::default()
                };
                random_session(&mut rng, &mut peers, &mut net, 40, pieces);
                net.drain(&mut rng, &mut peers, 1);
                if peers.iter().any(|peer| peer.replica.has_pending()) {
                    pending += 1;
                }
            }
            eprintln!(
                "PROBE three-way mesh, {name}, {delivery}: {pending} of {SESSIONS} left pending"
            );
        }
    }
}

#[test]
fn a_guest_undo_and_redo_name_only_the_guest() {
    let mut host = Peer::new(1);
    let mut guest = Peer::new(2);
    guest.receive(&host.edit(&[Edit::new(0..0, "The host's line.\n")]));
    guest.replica.end_group();
    // The guest deletes the host's word, then undoes it: the text comes back as the guest's.
    let deletion = guest.edit(&[Edit::new(4..10, "")]);
    assert_eq!(authors(&deletion).unwrap(), Vec::<u64>::new());
    host.receive(&deletion);
    guest.replica.end_group();
    let undo = guest.undo();
    assert_eq!(authors(&undo).unwrap(), vec![2]);
    host.receive(&undo);
    assert_eq!(host.rope, "The host's line.\n");
    // The guest types, undoes and redoes: the redo creates the guest's items again.
    let typed = guest.edit(&[Edit::new(0..0, "Guest: ")]);
    host.receive(&typed);
    guest.replica.end_group();
    let undo = guest.undo();
    assert_eq!(authors(&undo).unwrap(), Vec::<u64>::new());
    host.receive(&undo);
    let redo = guest.redo();
    assert_eq!(authors(&redo).unwrap(), vec![2]);
    host.receive(&redo);
    assert_eq!(host.rope, guest.rope);
}

#[test]
fn a_guest_update_left_pending_makes_the_host_rebuild() {
    let mut host = Peer::new(1);
    let mut guest = Peer::new(2);
    guest.receive(&host.edit(&[Edit::new(0..0, "= Page\n\nText.\n")]));
    let honest = guest.edit(&[Edit::new(7..7, "More. ")]);
    // A modified guest's update, changed by one byte, that `yrs` accepts and parks: the first
    // such mutation of the honest update.
    let hostile = (0..honest.len())
        .flat_map(|at| {
            [0x00u8, 0x01, 0x7f, 0x80, 0xff].map(|value| {
                let mut mutated = honest.clone();
                mutated[at] = value;
                mutated
            })
        })
        .find(|mutated| {
            let mut probe = Replica::new(9);
            probe.apply_remote(&host.replica.snapshot(), LIMIT).unwrap();
            probe.apply_remote(mutated, LIMIT).is_ok() && probe.has_pending()
        })
        .expect("a mutation that is left pending");
    let before = host.rope.clone();
    assert_eq!(
        host.receive(&hostile),
        Vec::new(),
        "nothing visible changed"
    );
    assert!(
        host.replica.has_pending(),
        "and yet the host's replica waits"
    );

    // The host never asks a guest to resynchronise it: it drops the guest and rebuilds the file's
    // replica from its own text, in a new epoch, under a new client id.
    let mut rebuilt = Peer::new(3);
    rebuilt.edit(&[Edit::new(0..0, &before)]);
    assert!(!rebuilt.replica.has_pending());
    let snapshot = rebuilt.replica.snapshot();
    let mut rejoined = Peer::new(4);
    rejoined.receive(&snapshot);
    rejoined.receive(&rebuilt.edit(&[Edit::new(0..0, "// Host, still editing.\n")]));
    assert_eq!(rejoined.rope, rebuilt.rope);
    assert!(rebuilt.rope.ends_with("= Page\n\nText.\n"));
}
