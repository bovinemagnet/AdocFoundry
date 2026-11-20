//! The harness the tests share: seeded randomness, peers that keep a plain `String` as the
//! engine keeps its rope (changed only by the byte edits their replica reports), and fake
//! transports that meet only through encoded bytes.

#![allow(dead_code)]

use std::collections::VecDeque;

use adoc_collab::{
    apply_edits, authors, check_boundaries, AppliedChange, ByteEdit, EncodedUpdate, Replica,
    ReplicaId, Sender,
};

/// A small deterministic generator, so every schedule is reproducible from its seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn seeded(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

pub fn id(value: u64) -> ReplicaId {
    ReplicaId::new(value).unwrap()
}

/// A participant: its replica, the text its engine would hold, and whether it is the host.
pub struct Peer {
    pub replica: Replica,
    pub rope: String,
    pub host: bool,
}

impl Peer {
    /// The host of a file whose text is `text`.
    pub fn host(text: &str, client: u64) -> Self {
        Self {
            replica: Replica::new(text, id(client)).unwrap(),
            rope: text.to_owned(),
            host: true,
        }
    }

    /// A guest joining from `host`'s snapshot.
    pub fn join(host: &Peer, client: u64) -> Self {
        let replica =
            Replica::from_snapshot(&host.replica.snapshot().unwrap(), id(client)).unwrap();
        let peer = Self {
            rope: replica.text(),
            replica,
            host: false,
        };
        assert_eq!(peer.rope, host.rope);
        peer
    }

    /// The local author's edits, checked against the rope first as the engine checks them.
    pub fn edit(&mut self, edits: &[ByteEdit]) -> EncodedUpdate {
        check_boundaries(&self.rope, edits).unwrap();
        let change = self.replica.apply_local(edits).unwrap();
        self.apply(&change);
        change.outbound.unwrap_or_default()
    }

    /// An update from the other side: a guest's, to the host, as from the guest who made it
    /// (the one whose items it holds; a deletion holds none, and any guest may delete); the
    /// host's, to a guest.
    pub fn receive(&mut self, update: &EncodedUpdate) -> AppliedChange {
        let sender = if self.host {
            let author = authors(update).unwrap().first().copied();
            Sender::Guest(author.unwrap_or_else(|| id(ReplicaId::MAX)))
        } else {
            Sender::Host
        };
        let change = self.replica.apply_remote(update, sender).unwrap();
        self.apply(&change);
        change
    }

    pub fn undo(&mut self) -> EncodedUpdate {
        let change = self
            .replica
            .undo_local()
            .unwrap()
            .expect("something to undo");
        self.apply(&change);
        change.outbound.unwrap()
    }

    pub fn redo(&mut self) -> EncodedUpdate {
        let change = self
            .replica
            .redo_local()
            .unwrap()
            .expect("something to redo");
        self.apply(&change);
        change.outbound.unwrap()
    }

    /// Applies a change's byte edits to the rope, which must then be the replica's text.
    pub fn apply(&mut self, change: &AppliedChange) {
        check_boundaries(&self.rope, &change.edits).unwrap();
        apply_edits(&mut self.rope, &change.edits);
        self.check();
    }

    pub fn check(&self) {
        assert_eq!(self.rope, self.replica.text());
        assert_eq!(self.rope.len(), self.replica.len());
    }

    /// Catches up from `other` by state vector, as a guest's resynchronisation does.
    pub fn resync_from(&mut self, other: &Peer) {
        let missing = other
            .replica
            .missing_since(&self.replica.state_vector())
            .unwrap();
        self.receive(&missing);
    }
}

/// Text chosen to land on hard places: emoji sequences, combining marks, wide characters and
/// line endings of both kinds.
pub const PIECES: &[&str] = &[
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

/// A random edit on character boundaries of `text`, inserting one of [`PIECES`].
pub fn random_edit(rng: &mut Rng, text: &str) -> ByteEdit {
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
    let insert = if end > start && rng.below(2) == 0 {
        ""
    } else {
        PIECES[rng.below(PIECES.len())]
    };
    ByteEdit::new(start, end, insert)
}

/// Every peer holds the same text and nothing pending.
pub fn assert_converged(peers: &[Peer], seed: u64) {
    for peer in peers {
        assert_eq!(peer.rope, peers[0].rope, "seed {seed}");
        assert!(!peer.replica.has_pending(), "seed {seed}");
    }
}

/// A host (peer 0) and guests, each guest's link an ordered queue each way; the host passes a
/// guest's update on only once it has applied it.
pub struct Star {
    pub peers: Vec<Peer>,
    down: Vec<VecDeque<EncodedUpdate>>,
    up: Vec<VecDeque<EncodedUpdate>>,
}

impl Star {
    pub fn new(text: &str, guests: usize) -> Self {
        let host = Peer::host(text, 10);
        let mut peers = vec![];
        for guest in 0..guests {
            peers.push(Peer::join(&host, 11 + guest as u64));
        }
        peers.insert(0, host);
        Self {
            down: vec![VecDeque::new(); peers.len()],
            up: vec![VecDeque::new(); peers.len()],
            peers,
        }
    }

    pub fn edit(&mut self, who: usize, edit: ByteEdit) {
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
    pub fn step(&mut self, rng: &mut Rng) -> bool {
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
