//! Collaborative editing through the engine (COLLAB 3/8, #690; PRD §10 tests 7–10): headless
//! engines meet only through the updates they hand out, over ordered in-memory queues, and each
//! engine's rope and replica agree after every step.

#![cfg(feature = "collab")]

use std::{collections::VecDeque, fs, sync::mpsc::Receiver};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, EditOrigin, EncodedUpdate, Engine, EngineError,
    EngineEvent, ReplicaId, Sender, Settings, SharedFileId, SharedReceipt,
};

fn engine() -> Engine {
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    Engine::new(settings)
}

fn id(value: u64) -> ReplicaId {
    ReplicaId::new(value).unwrap()
}

/// A host engine sharing a page and a guest engine that joined it.
struct Pair {
    _directory: tempfile::TempDir,
    host: Engine,
    guest: Engine,
    host_doc: DocumentId,
    guest_doc: DocumentId,
    /// Updates in flight, in order: host to guest, and guest to host.
    down: VecDeque<EncodedUpdate>,
    up: VecDeque<EncodedUpdate>,
}

impl Pair {
    fn new(text: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page.adoc");
        fs::write(&path, text).unwrap();
        let mut host = engine();
        let host_doc = host.open(&path).unwrap();
        host.share_document(host_doc, SharedFileId(1), id(1))
            .unwrap();
        let (snapshot, epoch) = host.shared_snapshot(host_doc).unwrap();
        let mut guest = engine();
        let guest_doc = guest
            .open_shared("page.adoc", SharedFileId(1), epoch, &snapshot, id(2), true)
            .unwrap();
        let pair = Self {
            _directory: directory,
            host,
            guest,
            host_doc,
            guest_doc,
            down: VecDeque::new(),
            up: VecDeque::new(),
        };
        pair.check();
        pair
    }

    fn engine(&mut self, host: bool) -> (&mut Engine, DocumentId) {
        if host {
            (&mut self.host, self.host_doc)
        } else {
            (&mut self.guest, self.guest_doc)
        }
    }

    /// Collects what each side made since last asked, in order.
    fn collect(&mut self) {
        self.down
            .extend(self.host.take_shared_updates(self.host_doc));
        self.up
            .extend(self.guest.take_shared_updates(self.guest_doc));
    }

    /// Delivers the next update on one direction's queue; false when it was empty.
    fn deliver(&mut self, down: bool) -> bool {
        self.collect();
        if down {
            let Some(update) = self.down.pop_front() else {
                return false;
            };
            let receipt = self
                .guest
                .apply_shared_update(self.guest_doc, 1, &update, Sender::Host)
                .unwrap();
            assert_eq!(receipt, SharedReceipt::Applied);
        } else {
            let Some(update) = self.up.pop_front() else {
                return false;
            };
            let receipt = self
                .host
                .apply_shared_update(self.host_doc, 1, &update, Sender::Guest(id(2)))
                .unwrap();
            assert_eq!(receipt, SharedReceipt::Applied);
        }
        self.check();
        true
    }

    fn drain(&mut self) {
        while self.deliver(true) || self.deliver(false) {}
    }

    /// Each engine's rope is its replica's text.
    fn check(&self) {
        for (engine, doc) in [(&self.host, self.host_doc), (&self.guest, self.guest_doc)] {
            assert_eq!(
                Some(text(engine, doc)),
                engine.shared_text(doc),
                "the rope and the replica differ"
            );
        }
    }

    fn texts(&self) -> (String, String) {
        (
            text(&self.host, self.host_doc),
            text(&self.guest, self.guest_doc),
        )
    }
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text_string()
}

fn replace(engine: &mut Engine, id: DocumentId, start: usize, end: usize, with: &str) {
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(start, end),
                text: with.to_owned(),
            },
        )
        .unwrap();
}

fn type_at(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(at, at),
                text: typed.to_owned(),
            },
        )
        .unwrap();
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

const PIECES: &[&str] = &["a", "word ", "é", "👩\u{200d}👧", "日本", "\r\n", "== T\n"];

#[test]
fn collab_engines_converge() {
    for seed in 1..=60u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut pair = Pair::new("= Page\n\nText.\n");
        for _ in 0..30 {
            match rng.below(4) {
                0 => {
                    pair.deliver(true);
                }
                1 => {
                    pair.deliver(false);
                }
                _ => {
                    let host = rng.below(2) == 0;
                    let (engine, doc) = pair.engine(host);
                    let current = text(engine, doc);
                    let bounds: Vec<usize> = current
                        .char_indices()
                        .map(|(at, _)| at)
                        .chain([current.len()])
                        .collect();
                    let start = bounds[rng.below(bounds.len())];
                    let after: Vec<usize> =
                        bounds.iter().copied().filter(|at| *at >= start).collect();
                    let end = after[rng.below(after.len().min(4))];
                    let piece = PIECES[rng.below(PIECES.len())];
                    if end == start {
                        type_at(engine, doc, start, piece);
                    } else {
                        replace(engine, doc, start, end, piece);
                    }
                    pair.check();
                }
            }
        }
        pair.drain();
        let (host, guest) = pair.texts();
        assert_eq!(host, guest, "seed {seed}");
    }
}

fn changes(
    events: &Receiver<EngineEvent>,
    id: DocumentId,
) -> Vec<(Vec<adoc_engine::TextEdit>, EditOrigin)> {
    events
        .try_iter()
        .filter_map(|event| match event {
            EngineEvent::DocumentChanged {
                id: changed,
                edits,
                origin,
                ..
            } if changed == id => Some((edits, origin)),
            _ => None,
        })
        .collect()
}

#[test]
fn collab_remote_edit_is_not_echoed() {
    let mut pair = Pair::new("= Page\n\nA long paragraph of text.\n");
    let events = pair.guest.subscribe();
    type_at(&mut pair.host, pair.host_doc, 9, "x");
    pair.deliver(true);
    // One small patch at its place, from elsewhere: never the whole text.
    let seen = changes(&events, pair.guest_doc);
    assert_eq!(seen.len(), 1);
    let (edits, origin) = &seen[0];
    assert_eq!(*origin, EditOrigin::Remote);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].range, SourceRange::new(9, 9));
    assert_eq!(edits[0].replacement, "x");
    // Nothing goes back, and nothing is the guest's to undo.
    assert!(pair.guest.take_shared_updates(pair.guest_doc).is_empty());
    assert!(!pair.guest.undo(pair.guest_doc));
    assert_eq!(pair.texts().0, pair.texts().1);
}

#[test]
fn collab_visual_edits_share_the_bridge() {
    let mut pair = Pair::new("= Page\n\nFirst.\n");
    pair.host
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(pair.host_doc))
        .unwrap();
    let version = pair.host.document(pair.host_doc).unwrap().version;
    pair.host
        .invoke(
            ids::VISUAL_INSERT_TEXT,
            CommandArgs::Visual(adoc_engine::VisualArgs {
                id: pair.host_doc,
                version,
                selection: SourceRange::new(14, 14),
                marks: adoc_engine::MarkSet::EMPTY,
                value: adoc_engine::VisualValue::Text(" Second".to_owned()),
            }),
        )
        .unwrap();
    assert!(text(&pair.host, pair.host_doc).contains("First. Second"));
    pair.drain();
    assert_eq!(pair.texts().0, pair.texts().1);
}

#[test]
fn collab_off_changes_nothing() {
    // Compiled in, but nothing shared: editing, undo and events are as they always were.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    fs::write(&path, "= Page\n").unwrap();
    let mut engine = engine();
    let doc = engine.open(&path).unwrap();
    let events = engine.subscribe();
    type_at(&mut engine, doc, 7, "Text.");
    assert_eq!(changes(&events, doc)[0].1, EditOrigin::Local);
    assert!(engine.shared_file(doc).is_none());
    assert!(engine.take_shared_updates(doc).is_empty());
    assert!(engine.undo(doc));
    assert_eq!(text(&engine, doc), "= Page\n");
    assert_eq!(changes(&events, doc)[0].1, EditOrigin::Undo);
    assert!(engine.redo(doc));
    assert_eq!(text(&engine, doc), "= Page\nText.");
}

#[test]
fn local_undo_keeps_remote_edits_and_forgets_the_time_before_sharing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    fs::write(&path, "Para.\n").unwrap();
    let mut host = engine();
    let doc = host.open(&path).unwrap();
    // Typed before sharing: its byte ranges are not replayed once the text is shared.
    type_at(&mut host, doc, 0, "Old ");
    host.share_document(doc, SharedFileId(1), id(1)).unwrap();
    assert!(!host.undo(doc), "the history before sharing was dropped");
    let (snapshot, epoch) = host.shared_snapshot(doc).unwrap();
    let mut guest = engine();
    let gdoc = guest
        .open_shared("page.adoc", SharedFileId(1), epoch, &snapshot, id(2), true)
        .unwrap();

    // The host types a word; the guest inserts inside it; the host undoes its word.
    replace(&mut host, doc, 8, 8, " wonderful");
    for update in host.take_shared_updates(doc) {
        guest
            .apply_shared_update(gdoc, 1, &update, Sender::Host)
            .unwrap();
    }
    type_at(&mut guest, gdoc, 12, "XX");
    for update in guest.take_shared_updates(gdoc) {
        host.apply_shared_update(doc, 1, &update, Sender::Guest(id(2)))
            .unwrap();
    }
    assert_eq!(text(&host, doc), "Old Para wonXXderful.\n");
    assert!(host.undo(doc));
    assert_eq!(text(&host, doc), "Old ParaXX.\n", "the guest's text stays");
    assert!(host.redo(doc));
    assert_eq!(text(&host, doc), "Old Para wonXXderful.\n");
    for update in host.take_shared_updates(doc) {
        guest
            .apply_shared_update(gdoc, 1, &update, Sender::Host)
            .unwrap();
    }
    assert_eq!(text(&guest, gdoc), text(&host, doc));
}

#[test]
fn refused_updates_and_edits_change_nothing() {
    let mut pair = Pair::new("= Page\n\nText.\n");
    let version = pair.guest.document(pair.guest_doc).unwrap().version;
    let before = pair.texts();
    let guest_doc = pair.guest_doc;
    // Malformed bytes, an earlier epoch, the wrong side.
    for (epoch, update, from) in [
        (1, EncodedUpdate(vec![0xff, 0xff]), Sender::Host),
        (0, EncodedUpdate(vec![0x00, 0x00]), Sender::Host),
        (1, EncodedUpdate(vec![0x00, 0x00]), Sender::Guest(id(3))),
    ] {
        assert!(pair
            .guest
            .apply_shared_update(guest_doc, epoch, &update, from)
            .is_err());
    }
    assert_eq!(pair.guest.document(guest_doc).unwrap().version, version);
    assert_eq!(pair.texts(), before);
    pair.check();

    // A guest who may not edit cannot, by typing or undo; nor can a guest save.
    pair.guest.set_shared_editing(guest_doc, false);
    let refused = pair.guest.invoke(
        ids::EDIT_INSERT_TEXT,
        CommandArgs::Text {
            id: guest_doc,
            range: SourceRange::new(0, 0),
            text: "x".to_owned(),
        },
    );
    assert!(
        matches!(refused, Err(EngineError::Refused(_))),
        "{refused:?}"
    );
    assert_eq!(pair.texts(), before);
    assert!(matches!(
        pair.guest.save(guest_doc),
        Err(EngineError::Refused(_))
    ));
    // The host does not reload a shared document from disk.
    assert!(matches!(
        pair.host.reload(pair.host_doc),
        Err(EngineError::Refused(_))
    ));
}

#[test]
fn a_hostile_update_makes_the_host_rebuild_and_the_guest_start_again() {
    let mut pair = Pair::new("= Page\n\nUn café 👋\r\nMore.\n");
    // A guest's honest update with one byte changed, which `yrs` integrates inside the 👋:
    // the host's replica can no longer be trusted with its rope (`adoc-collab`'s
    // `an_update_splitting_a_character_poisons_the_replica`).
    replace(&mut pair.guest, pair.guest_doc, 8, 17, "Thé 🇬🇧");
    let mut honest = pair.guest.take_shared_updates(pair.guest_doc).remove(0);
    let len = honest.0.len();
    honest.0[0x8f % len] = 0x00;
    honest.0[0x06 % len] = 0x10;
    let before = text(&pair.host, pair.host_doc);
    let receipt = pair
        .host
        .apply_shared_update(pair.host_doc, 1, &honest, Sender::Guest(id(2)))
        .unwrap();
    assert_eq!(receipt, SharedReceipt::Rebuilt { epoch: 2 });
    assert_eq!(
        text(&pair.host, pair.host_doc),
        before,
        "the host's text is untouched"
    );
    assert_eq!(pair.host.shared_text(pair.host_doc), Some(before.clone()));

    // The guest starts again from the host's new snapshot.
    let (snapshot, epoch) = pair.host.shared_snapshot(pair.host_doc).unwrap();
    pair.guest
        .reset_shared(pair.guest_doc, epoch, &snapshot, id(3))
        .unwrap();
    assert_eq!(text(&pair.guest, pair.guest_doc), before);
    assert_eq!(pair.guest.shared_file(pair.guest_doc).unwrap().1, 2);
    // The old epoch's updates are refused.
    assert!(pair
        .host
        .apply_shared_update(
            pair.host_doc,
            1,
            &EncodedUpdate(vec![0, 0]),
            Sender::Guest(id(3))
        )
        .is_err());
}

#[test]
fn a_snapshot_that_would_rebuild_another_text_makes_the_host_rebuild() {
    // `adoc-collab`'s `a_snapshot_that_would_rebuild_another_text_is_refused`, through the
    // engine: a guest's altered update applies consistently, but the host's next snapshot would
    // give a later joiner another text, so the host rebuilds from its own text in a new epoch.
    let mut pair = Pair::new("= Page\n\nUn café 👋\r\nMore.\n");
    replace(&mut pair.guest, pair.guest_doc, 8, 17, "Thé 🇬🇧");
    let mut update = pair.guest.take_shared_updates(pair.guest_doc).remove(0);
    let len = update.0.len();
    update.0[8 % len] = 0x01;
    let receipt = pair
        .host
        .apply_shared_update(pair.host_doc, 1, &update, Sender::Guest(id(2)))
        .unwrap();
    assert_eq!(receipt, SharedReceipt::Applied);
    let text_now = text(&pair.host, pair.host_doc);
    let (snapshot, epoch) = pair.host.shared_snapshot(pair.host_doc).unwrap();
    assert_eq!(epoch, 2, "rebuilt before the snapshot was given");
    let mut joiner = engine();
    let doc = joiner
        .open_shared("page.adoc", SharedFileId(1), epoch, &snapshot, id(5), true)
        .unwrap();
    assert_eq!(
        text(&joiner, doc),
        text_now,
        "the joiner sees the host's text"
    );
}
