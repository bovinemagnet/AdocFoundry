//! The multi-replica simulation (COLLAB 8/8, #695; PRD §§9 and 10, `collab_simulation_converges`):
//! a host and two guests, each a whole engine, edit two shared pages over the in-memory session
//! by seeded schedules of insertions, deletions, undo and redo in multi-byte text, while links
//! drop and resume, deliveries are held, roles change, an update is duplicated and the host
//! reloads a page from disk in a new epoch. After every step each engine's rope and replica
//! agree, a refused edit changes nothing, an undo never takes away another participant's text
//! and a saved revision is the text on disk; whenever everyone connected settles, and once the
//! faults are lifted, every participant converges byte for byte.
//!
//! A schedule is generated from its seed before it runs, its positions fractions of the text,
//! so any part of it replays. A failing schedule is shrunk to the fewest operations that still
//! fail, which the failure prints with each replica's state vector; no secret is printed. The
//! checked-in seeds run in seconds; `collab_simulation_soak` runs many more and is ignored by
//! default.

#![cfg(feature = "collab")]

mod collab_common;

use std::fs;

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids,
    protocol::{Message, PeerId, Role},
    revision_of, CommandArgs, DiskResolution, DocumentId, Engine, LinkEvent, SharedSaveState,
};
use collab_common::*;

/// The seeds run on every test run. A seed that once failed is added here once fixed.
const SEEDS: [u64; 8] = [
    1,
    2,
    3,
    0x5eed,
    0xc011_ab01,
    0xdead_beef,
    0x0123_4567_89ab_cdef,
    0xfeed_face_cafe_f00d,
];
const STEPS: usize = 300;

/// SplitMix64: small, seeded and the same everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Who acts: the host, or a guest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Actor {
    Host,
    Guest(PeerId),
}

const GUESTS: [PeerId; 2] = [PeerId(1), PeerId(2)];

fn actor(n: u8) -> Actor {
    match n {
        0 => Actor::Host,
        n => Actor::Guest(GUESTS[usize::from(n - 1)]),
    }
}

/// One step of a schedule. Positions are fractions of the text (of 65,536), so a step means
/// the same whatever ran before it.
#[derive(Clone, Copy, Debug)]
enum Op {
    /// Inserts one of the actor's pieces at `at`, or deletes up to `len` characters from it.
    Edit {
        actor: u8,
        file: u64,
        at: u16,
        len: u8,
        piece: u8,
        delete: bool,
    },
    Undo {
        actor: u8,
        file: u64,
        redo: bool,
    },
    /// One engine reads what has reached it, out of step with the others.
    Pump {
        actor: u8,
    },
    /// Everyone pumps until nothing is in flight (only while deliveries flow).
    Settle,
    /// A guest's link drops, losing the last `lost` messages it sent.
    Drop {
        guest: u8,
        lost: u8,
    },
    /// The guest whose link dropped last resumes.
    Resume,
    /// The host stops, or starts again, reading what reaches it.
    Hold,
    /// A guest is made a Viewer, or an Editor again.
    Role {
        guest: u8,
    },
    Save {
        file: u64,
    },
    /// The page changes on disk and the host takes the file's text in a new epoch.
    Reload {
        file: u64,
    },
    /// One update a guest sent reaches the host a second time.
    Duplicate {
        guest: u8,
    },
}

fn schedule(seed: u64, steps: usize) -> Vec<Op> {
    let mut rng = Rng(seed);
    (0..steps)
        .map(|_| {
            let actor = rng.below(3) as u8;
            let guest = 1 + rng.below(2) as u8;
            let file = 1 + rng.below(2);
            match rng.below(100) {
                0..=44 => Op::Edit {
                    actor,
                    file,
                    at: rng.below(65_536) as u16,
                    len: 1 + rng.below(4) as u8,
                    piece: rng.below(5) as u8,
                    delete: rng.below(100) < 35,
                },
                45..=54 => Op::Undo {
                    actor,
                    file,
                    redo: rng.below(100) < 30,
                },
                55..=69 => Op::Pump { actor },
                70..=79 => Op::Settle,
                80..=83 => Op::Drop {
                    guest,
                    lost: rng.below(3) as u8,
                },
                84..=86 => Op::Resume,
                87..=89 => Op::Hold,
                90..=92 => Op::Role { guest },
                93..=95 => Op::Save { file },
                96..=97 => Op::Reload { file },
                _ => Op::Duplicate { guest },
            }
        })
        .collect()
}

/// What each actor types, its letters its own so that whose text is whose can be counted: ASCII,
/// two-byte, three-byte and four-byte characters, a combining mark and line endings.
fn pieces(actor: Actor) -> &'static [&'static str; 5] {
    match actor {
        Actor::Host => &["h", "hh", "ĥ", "ħ\n", "h\r\n"],
        Actor::Guest(PeerId(1)) => &["g", "ğ", "😀", "g\n", "ǵ\r\n"],
        Actor::Guest(_) => &["k", "κ", "k\u{301}", "ķ\n", "€"],
    }
}

/// The characters only `actor` types.
fn letters(actor: Actor) -> &'static [char] {
    match actor {
        Actor::Host => &['h', 'ĥ', 'ħ'],
        Actor::Guest(PeerId(1)) => &['g', 'ğ', '😀', 'ǵ'],
        Actor::Guest(_) => &['k', 'κ', '\u{301}', 'ķ', '€'],
    }
}

fn count(text: &str, of: &[char]) -> usize {
    text.chars().filter(|c| of.contains(c)).count()
}

/// The offsets in `text` a caret can be at.
fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect()
}

struct Simulation {
    session: Session,
    /// Guests whose link is down, the last dropped last.
    offline: Vec<PeerId>,
    /// The guest one of whose updates was duplicated: the host removes it, or discards the copy
    /// as sent before its latest snapshot.
    duplicated: Option<PeerId>,
    /// Guests made Viewers.
    viewers: Vec<PeerId>,
    /// Reloads so far, which name the text each reload writes.
    reloads: usize,
}

type Checked = Result<(), String>;

impl Simulation {
    fn new() -> Self {
        let mut session = Session::new(&[("one.adoc", "= One\n\nText.\n"), ("two.adoc", "")]);
        for peer in [1, 2] {
            let peer = session.guest(peer, true);
            session.engine(peer).open_session_file(2).unwrap();
            session.settle();
        }
        Self {
            session,
            offline: Vec::new(),
            duplicated: None,
            viewers: Vec::new(),
            reloads: 0,
        }
    }

    fn actors(&self) -> Vec<Actor> {
        [Actor::Host]
            .into_iter()
            .chain(GUESTS.iter().map(|peer| Actor::Guest(*peer)))
            .collect()
    }

    fn engine(&mut self, actor: Actor) -> &mut Engine {
        match actor {
            Actor::Host => &mut self.session.host,
            Actor::Guest(peer) => self.session.engine(peer),
        }
    }

    fn document(&mut self, actor: Actor, file: u64) -> Option<DocumentId> {
        self.engine(actor).session_document(file)
    }

    fn text(&mut self, actor: Actor, id: DocumentId) -> String {
        self.engine(actor).document(id).unwrap().text_string()
    }

    /// Whether the host removed `peer`, which then takes no further part.
    fn removed(&self, peer: PeerId) -> bool {
        self.session.removed().iter().any(|(gone, _)| *gone == peer)
    }

    fn held(&self) -> bool {
        self.session.net.lock().unwrap().hold_host
    }

    /// Each replica's state, for a failure: no secret is in it.
    fn state(&mut self) -> String {
        let mut lines = Vec::new();
        for actor in self.actors() {
            for file in [1, 2] {
                let Some(id) = self.document(actor, file) else {
                    continue;
                };
                let engine = self.engine(actor);
                let vector = engine
                    .shared_state_vector(id)
                    .map(|vector| format!("{:02x?}", vector.0))
                    .unwrap_or_else(|error| format!("({error})"));
                let epoch = engine.shared_file(id).map(|(_, epoch, _)| epoch);
                let connected = engine.session_connected();
                let text = engine.document(id).unwrap().text_string();
                lines.push(format!(
                    "{actor:?} file {file}: epoch {epoch:?}, connected {connected}, {text:?} \
                     {vector}"
                ));
            }
        }
        let net = self.session.net.lock().unwrap();
        for ((peer, code), reason) in net.removed.iter().zip(&net.reasons) {
            lines.push(format!("the host removed {peer:?} ({code:?}): {reason}"));
        }
        lines.join("\n  ")
    }

    /// Every engine's rope and replica of each shared page hold the same text.
    fn check_replicas(&mut self) -> Checked {
        for actor in self.actors() {
            for file in [1, 2] {
                let Some(id) = self.document(actor, file) else {
                    continue;
                };
                let engine = self.engine(actor);
                let rope = engine.document(id).unwrap().text_string();
                if let Some(replica) = engine.shared_text(id) {
                    if replica != rope {
                        return Err(format!(
                            "{actor:?}'s replica of file {file} is not its text: {replica:?}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Every guest connected and not removed holds each page's text as the host does.
    fn check_converged(&mut self) -> Checked {
        for file in [1, 2] {
            let host = self
                .document(Actor::Host, file)
                .ok_or("the host lost a file")?;
            let text = self.text(Actor::Host, host);
            for peer in GUESTS {
                if self.removed(peer) || self.offline.contains(&peer) {
                    continue;
                }
                let id = self
                    .document(Actor::Guest(peer), file)
                    .ok_or_else(|| format!("{peer:?} lost file {file}"))?;
                let theirs = self.text(Actor::Guest(peer), id);
                if theirs != text {
                    return Err(format!(
                        "{peer:?} did not converge on file {file}: {theirs:?} against {text:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn present(&self, actor: Actor) -> bool {
        !matches!(actor, Actor::Guest(peer) if self.removed(peer))
    }

    fn apply(&mut self, op: Op) -> Checked {
        match op {
            Op::Edit {
                actor: n,
                file,
                at,
                len,
                piece,
                delete,
            } => {
                let actor = actor(n);
                if !self.present(actor) {
                    return Ok(());
                }
                let Some(id) = self.document(actor, file) else {
                    return Ok(());
                };
                let before = self.text(actor, id);
                let places = boundaries(&before);
                let start = usize::from(at) * places.len() / 65_536;
                let (range, typed) = if delete && places.len() > 1 {
                    let start = start.min(places.len() - 2);
                    let end = (start + usize::from(len)).min(places.len() - 1);
                    (SourceRange::new(places[start], places[end]), String::new())
                } else {
                    let at = places[start];
                    let typed = pieces(actor)[usize::from(piece)];
                    (SourceRange::new(at, at), typed.to_owned())
                };
                let result = self.engine(actor).invoke(
                    ids::EDIT_REPLACE_RANGE,
                    CommandArgs::Text {
                        id,
                        range,
                        text: typed,
                    },
                );
                if result.is_err() && self.text(actor, id) != before {
                    return Err("a refused edit changed the text".to_owned());
                }
            }
            Op::Undo {
                actor: n,
                file,
                redo,
            } => {
                let actor = actor(n);
                if !self.present(actor) {
                    return Ok(());
                }
                let Some(id) = self.document(actor, file) else {
                    return Ok(());
                };
                let others: Vec<char> = self
                    .actors()
                    .into_iter()
                    .filter(|other| *other != actor)
                    .flat_map(|other| letters(other).iter().copied())
                    .collect();
                let command = if redo { ids::EDIT_REDO } else { ids::EDIT_UNDO };
                let before = self.text(actor, id);
                let _ = self
                    .engine(actor)
                    .invoke(command, CommandArgs::Document(id));
                let after = self.text(actor, id);
                // An undo takes away only its author's insertions and brings back what its
                // author deleted. A redo may delete others' text again, as its author did.
                if !redo && count(&after, &others) < count(&before, &others) {
                    return Err(format!(
                        "{actor:?}'s {} took away another participant's text: {before:?} -> \
                         {after:?}",
                        command.as_str()
                    ));
                }
            }
            Op::Pump { actor: n } => {
                let actor = actor(n);
                if self.present(actor) {
                    self.engine(actor).pump();
                }
            }
            Op::Settle => {
                if !self.held() {
                    self.session.settle();
                    self.check_converged()?;
                }
            }
            Op::Drop { guest, lost } => {
                let peer = GUESTS[usize::from(guest - 1)];
                if !self.offline.contains(&peer) && !self.removed(peer) {
                    self.session.drop_link(peer, usize::from(lost));
                    self.offline.push(peer);
                }
            }
            Op::Resume => {
                if let Some(peer) = self.offline.pop() {
                    self.session.resume(peer);
                }
            }
            Op::Hold => {
                let mut net = self.session.net.lock().unwrap();
                net.hold_host = !net.hold_host;
            }
            Op::Role { guest } => {
                let peer = GUESTS[usize::from(guest - 1)];
                if self.removed(peer) {
                    return Ok(());
                }
                if self.viewers.contains(&peer) {
                    self.viewers.retain(|other| *other != peer);
                    let _ = self.session.host.set_peer_role(peer, Role::Editor);
                } else {
                    self.viewers.push(peer);
                    let _ = self.session.host.set_peer_role(peer, Role::Viewer);
                }
            }
            Op::Save { file } => {
                let Some(id) = self.document(Actor::Host, file) else {
                    return Ok(());
                };
                if self.session.host.save(id).is_err() {
                    return Ok(());
                }
                let path = self.session.host.document(id).unwrap().path.clone();
                if let Some(SharedSaveState::Saved { revision }) =
                    self.session.host.shared_save_state(id)
                {
                    if revision != revision_of(&fs::read_to_string(&path).unwrap()) {
                        return Err("a saved revision is not the text on disk".to_owned());
                    }
                }
            }
            Op::Reload { file } => {
                let Some(id) = self.document(Actor::Host, file) else {
                    return Ok(());
                };
                self.reloads += 1;
                let path = self.session.host.document(id).unwrap().path.clone();
                fs::write(&path, format!("= Reloaded {}\n", self.reloads)).unwrap();
                // The host finds the change when it next saves, as the watcher would sooner.
                let _ = self.session.host.save(id);
                let _ = self
                    .session
                    .host
                    .resolve_disk_conflict(id, DiskResolution::ReloadDisk);
            }
            Op::Duplicate { guest } => {
                let peer = GUESTS[usize::from(guest - 1)];
                if self.duplicated.is_some() {
                    return Ok(());
                }
                let mut net = self.session.net.lock().unwrap();
                let update = net.to_host.iter().find_map(|event| match event {
                    LinkEvent::Message {
                        peer: from,
                        message: Message::Update { .. },
                    } if *from == peer => Some(event.clone()),
                    _ => None,
                });
                if let Some(update) = update {
                    net.to_host.push_back(update);
                    drop(net);
                    self.duplicated = Some(peer);
                }
            }
        }
        self.check_replicas()
    }

    /// Lifts every fault: deliveries flow, links resume, Viewers edit again.
    fn heal(&mut self) {
        self.session.net.lock().unwrap().hold_host = false;
        while let Some(peer) = self.offline.pop() {
            if !self.removed(peer) {
                self.session.resume(peer);
            }
        }
        for peer in std::mem::take(&mut self.viewers) {
            let _ = self.session.host.set_peer_role(peer, Role::Editor);
        }
        self.session.settle();
    }

    fn finish(&mut self) -> Checked {
        self.heal();
        self.check_replicas()?;
        self.check_converged()?;
        for (peer, _) in self.session.removed() {
            if self.duplicated != Some(peer) {
                return Err(format!("the host removed {peer:?}, who did nothing wrong"));
            }
        }
        self.session.assert_no_host_paths();
        Ok(())
    }
}

/// Runs `ops`, giving the first broken invariant and the state then.
fn run(ops: &[Op]) -> Result<(), (usize, String, String)> {
    let mut simulation = Simulation::new();
    for (step, op) in ops.iter().enumerate() {
        if let Err(what) = simulation.apply(*op) {
            return Err((step, what, simulation.state()));
        }
    }
    simulation
        .finish()
        .map_err(|what| (ops.len(), what, simulation.state()))
}

/// The fewest operations of `ops` that still fail (delta debugging, bounded).
fn shrink(mut ops: Vec<Op>) -> Vec<Op> {
    let mut chunk = ops.len() / 2;
    let mut runs = 0;
    while chunk >= 1 && runs < 3000 {
        let mut start = 0;
        let mut removed_any = false;
        while start < ops.len() && runs < 3000 {
            let mut fewer = ops.clone();
            fewer.drain(start..(start + chunk).min(ops.len()));
            runs += 1;
            if run(&fewer).is_err() {
                ops = fewer;
                removed_any = true;
            } else {
                start += chunk;
            }
        }
        if !removed_any {
            chunk /= 2;
        }
    }
    ops
}

fn simulate(seed: u64, steps: usize) {
    let ops = schedule(seed, steps);
    if run(&ops).is_ok() {
        return;
    }
    let fewest = shrink(ops);
    let (step, what, state) = run(&fewest).expect_err("the shrunk schedule fails");
    let listed: Vec<String> = fewest
        .iter()
        .enumerate()
        .map(|(n, op)| format!("{n:3}: {op:?}"))
        .collect();
    panic!(
        "seed {seed:#x} fails at step {step} of its shrunk schedule: {what}\n\
         schedule:\n  {}\nstate:\n  {state}",
        listed.join("\n  ")
    );
}

#[test]
fn collab_simulation_converges() {
    for seed in SEEDS {
        simulate(seed, STEPS);
    }
}

/// Many more seeds and steps, for a change to the session or the replica:
/// `cargo test --release -p adoc-engine --test collab_simulation -- --ignored`, from
/// `ADOC_COLLAB_SEED` (1000 by default).
#[test]
#[ignore = "a soak run, minutes long"]
fn collab_simulation_soak() {
    let start: u64 = std::env::var("ADOC_COLLAB_SEED")
        .ok()
        .and_then(|seed| seed.parse().ok())
        .unwrap_or(1000);
    for seed in start..start + 200 {
        simulate(seed, 1500);
    }
}

/// A guest away while the host reloads a page from disk resumes into the new epoch (found by
/// the simulation: the host had recorded the new epoch for a guest it could not send it to).
#[test]
fn a_guest_away_through_a_new_epoch_catches_up() {
    let mut session = Session::new(&[("one.adoc", "= One\n")]);
    let ada = session.guest(1, true);
    guest_types(&mut session, ada, 0, "g");
    session.settle();
    session.drop_link(ada, 0);
    session.settle();
    let id = session.host_doc();
    let path = session.host.document(id).unwrap().path.clone();
    fs::write(&path, "= Reloaded\n").unwrap();
    let _ = session.host.save(id);
    session
        .host
        .resolve_disk_conflict(id, DiskResolution::ReloadDisk)
        .unwrap();
    session.settle();
    type_at(&mut session.host, id, 0, "h");
    session.settle();
    session.resume(ada);
    session.settle();
    assert_eq!(session.guest_text(ada), "h= Reloaded\n");
    assert_eq!(session.guest_text(ada), session.host_text());
}

/// Each schedule the simulation once failed on, shrunk, now converging.
fn converges(ops: &[Op]) {
    if let Err((step, what, state)) = run(ops) {
        panic!("fails at step {step}: {what}\n  {state}");
    }
}

/// An edit a guest made before the host reloaded the page reaches the host in the old epoch:
/// it is spent, not a breach that removes the guest.
#[test]
fn an_edit_crossing_a_reload_is_spent() {
    converges(&[
        Op::Reload { file: 2 },
        Op::Edit {
            actor: 1,
            file: 2,
            at: 1245,
            len: 1,
            piece: 0,
            delete: true,
        },
        Op::Pump { actor: 1 },
    ]);
}

/// A guest's edit not yet sent when its link drops is sent on resuming before it asks to be
/// resynchronised, so the snapshot answering holds it.
#[test]
fn an_edit_waiting_when_the_link_drops_is_kept() {
    converges(&[
        Op::Edit {
            actor: 2,
            file: 1,
            at: 36503,
            len: 4,
            piece: 1,
            delete: true,
        },
        Op::Drop { guest: 2, lost: 0 },
    ]);
}

/// The host answers an update with a snapshot before it knows the link came back; its
/// acknowledgement on resuming then waits to be asked, which the guest does.
#[test]
fn a_snapshot_racing_a_resume_is_followed_by_a_resync() {
    converges(&[
        Op::Hold,
        Op::Edit {
            actor: 1,
            file: 2,
            at: 4212,
            len: 3,
            piece: 3,
            delete: false,
        },
        Op::Role { guest: 1 },
        Op::Pump { actor: 1 },
        Op::Drop { guest: 1, lost: 0 },
        Op::Hold,
        Op::Resume,
        Op::Pump { actor: 0 },
        Op::Edit {
            actor: 2,
            file: 2,
            at: 17048,
            len: 1,
            piece: 3,
            delete: false,
        },
    ]);
}

/// A copy of an update the host answered with a snapshot names the snapshot before: it is
/// discarded, and the guest stays.
#[test]
fn a_duplicate_after_a_snapshot_is_discarded() {
    converges(&[
        Op::Edit {
            actor: 1,
            file: 2,
            at: 4212,
            len: 3,
            piece: 3,
            delete: false,
        },
        Op::Role { guest: 1 },
        Op::Pump { actor: 1 },
        Op::Duplicate { guest: 1 },
    ]);
}

/// Updates the host reads after the guest's link came back are acknowledged over the new
/// link; the guest, resuming, sends again those after the first acknowledgement, and the host
/// discards those it applied already rather than the guest.
#[test]
fn an_update_sent_again_after_a_reconnection_is_discarded() {
    converges(&[
        Op::Edit {
            actor: 2,
            file: 2,
            at: 33943,
            len: 4,
            piece: 3,
            delete: false,
        },
        Op::Edit {
            actor: 2,
            file: 2,
            at: 5736,
            len: 1,
            piece: 2,
            delete: false,
        },
        Op::Undo {
            actor: 2,
            file: 2,
            redo: false,
        },
        Op::Pump { actor: 2 },
        Op::Drop { guest: 2, lost: 1 },
    ]);
}
