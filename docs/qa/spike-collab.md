# Collaboration Spike (COLLAB 1/8, #688) — Record

| | |
|---|---|
| Author | Paul Snow |
| Date | 2026-11-20 |
| Candidates | `yrs` 0.28.2 (MIT), built and measured; `loro` 1.18.0 (MIT) and `automerge` 0.12.0 (MIT), licence and dependency tree only |
| Toolchain | Rust 1.99.1, as `rust-toolchain.toml` pins; release builds on an Apple Silicon Mac |
| Code | `spikes/collab/`, a Cargo workspace of its own that the main workspace excludes, so `yrs` reaches no shipped binary, lean build or notices file until COLLAB 2/8 adopts it |
| Decision | **Go with `yrs`**, with its `sync` feature and `OffsetKind::Bytes`, in a star topology through the host over ordered streams. Pending to the maintainer's acceptance of this record and of `docs/design/DESIGN-AdocFoundry_Collaboration.md` |

The PRD (`docs/prd/PRD-AdocFoundry_COLAB.md`) makes this package a gate. It proves that two independent replicas converge, that undo is per author, and that reconnecting is safe. It also settles the CRDT and the contracts COLLAB 2/8 to 8/8 build on. This record holds what the spike measured; the design document holds what was decided from it.

## Method

`spikes/collab/src/lib.rs` is a throwaway `Replica` over one `yrs` text, shaped like the one PRD §8 sketches:
- `apply_local` and `apply_remote` take and give byte edits in pre-edit coordinates, the engine's `TextEdit` shape.
- `undo` and `redo` cover the local author only.
- `state_vector`, `missing_since` and `snapshot` handle synchronisation; `anchor` and `resolve` handle carets.
- `authors` reads which clients an update creates items for, without applying it.

Replicas meet only through encoded bytes. In `tests/spike.rs`, each test peer keeps a plain `String` standing in for the engine's rope, changed only by the byte edits its replica reports. After every step the test checks that string against the replica's text. A fake transport drops, duplicates, reorders and delays updates, and tags each with a session epoch that the receiver checks before reading the bytes. Schedules are seeded, so every failure is reproducible.

Edits are drawn from text chosen to land on hard places:
- emoji with zero-width joiners, and a flag;
- combining marks, a precomposed `é` and CJK text;
- `\r\n` and `\n` line endings, and AsciiDoc markup.

```sh
cd spikes/collab
cargo test --release                                                  # the acceptance tests and probes
cargo test --release --test spike -- --ignored --nocapture            # finding 1's counts by text and delivery
cargo test --release --test measure -- --ignored --nocapture         # the measurements
/usr/bin/time -l target/release/deps/measure-<hash> --ignored --nocapture --exact measure_a_1_mib_session_yrs_alone
```

## Acceptance

| Test | Shows |
|---|---|
| `collab_spike_replicas_converge` | All of the following converge byte for byte, with every rope matching its replica after every step: concurrent insertions at one offset; an insertion inside a concurrent deletion; IME-style composition, replaced four times and then delivered in reverse; 2,000 seeded two-replica schedules reordered, duplicated and delayed; 2,000 seeded three-replica schedules in a star through the host; 300 three-replica schedules each sending straight to the others, with a resynchronisation where one is left pending (finding 1). A reconnect where both sides lost edits when the link dropped catches up both ways, exactly once, and the same catch-up again changes nothing. An update from an earlier epoch is refused unread. A snapshot starts a new replica with the same text |
| `collab_spike_undo_is_per_author` | The PRD's case: Alice types a word, Bob inserts text inside it, and Alice undoes. Alice's word goes and Bob's text stays. Redo puts her word back around his text, and a later undo of her first line leaves only Bob's text. Bob's history holds only his own insertion |
| `offsets_inside_a_character_are_refused_before_yrs_sees_them` | An offset inside a character, past the end, or edits out of order are refused, and the text is unchanged |
| `malformed_updates_change_nothing` | Malformed input is refused, applied as a valid update, or parked as pending; oversized input is refused before decoding (finding 4) |
| `a_guest_update_left_pending_makes_the_host_rebuild` | A one-byte mutation of a guest's update that `yrs` accepts and parks leaves the host's text unchanged but its replica waiting. The host's response (drop the guest, rebuild the file's replica from its own text under a new client id, snapshot to the rest) leaves nothing pending, and editing goes on (finding 4) |
| `anchors_follow_their_text` | A caret sent as an encoded `StickyIndex` stays before the same word on both replicas through concurrent edits, emoji included |
| `an_update_names_its_authors_before_it_is_applied` | `authors` gives the clients whose items an update creates. A deletion creates none, and malformed bytes are an error |
| `a_guest_undo_and_redo_name_only_the_guest` | A guest's undo of a deletion and its redo of an insertion create items under the guest's own client id, so the host's authorship rule accepts them; an undo of an insertion creates none |
| `probe_pending_needs_three_replicas_out_of_causal_order` (ignored) | Finding 1's counts, by text and delivery |
| `yrs_handles_are_send`, `the_replica_is_send` | With `sync`, a replica can live in an engine that moves between threads (finding 5) |

## Findings

1. **`yrs` can leave an update pending for ever when three or more replicas exchange updates out of causal order.**
   - **How often:** with three replicas each sending straight to the others, some sessions ended with every update delivered, but one replica still holding an update `yrs` had parked as pending (`ReadTxn::has_missing_updates`). The replicas' texts differed as a result. Out of 2,000 sessions each (`probe_pending_needs_three_replicas_out_of_causal_order`):

     | Delivery | Unicode text | ASCII text |
     |---|---|---|
     | Reordered and duplicated | 7 | 3 |
     | Reordered | 5 | 4 |
     | In the order sent | 0 | 0 |

   - **What it isn't:** it happens with ASCII text and without duplicates. It needs delivery out of the order sent.
   - **What repairs it:** an exchange of state vectors afterwards always repaired it.
   - **Where it never happened:** two replicas (2,000 schedules, reordered and duplicated), or three replicas in a star through the host where each link is ordered and the host forwards a guest's update only once it has applied it (2,000 schedules).
   - **Design consequence:** a star topology over ordered streams, which v1 needs anyway, since only the host holds the files. A guest left with something pending after the host's update asks the host for a resynchronisation. The host never asks a guest (finding 4).
   - Not yet reported upstream.
2. **`yrs` does not refuse an offset inside a character.** Inserting at byte 1 of `"é"` with `OffsetKind::Bytes` gives `"éx"`: the insertion moves silently to the next boundary. The bridge must check every offset against the rope before `yrs` sees it; the engine's `TextEdit::validate` already does.
3. **Deltas are in bytes.** With `OffsetKind::Bytes`, a text event's retains and deletions count UTF-8 bytes (`content_len(encoding)` in `types/text.rs`). Every rope in every schedule, rebuilt from those byte edits alone, matched its replica's text.
4. **Malformed input.**
   - 5,121 inputs were tried: every truncation of a real update, 5,000 random byte strings, every byte of a real update replaced by six other values, and length prefixes claiming far more than follows.
   - None panicked, and none put the text and the rope out of step. 5,070 were refused and 39 accepted with a change: those 39 are mutated updates that are still valid, which the decoder cannot tell from honest ones. 2 were accepted with no change.
   - **10 were accepted and parked as pending.** They change no text, but they leave the replica waiting for an item that will never come. A pre-check of the update's clocks and deletions against the receiver's state vector (`Replica::precheck`) caught none of them: what they lack is an item's origin, which can't be seen without applying the update.
   - **So a modified guest could stall a host that resynchronised whenever it found itself pending.** The host never resynchronises from a guest. A guest's update that leaves the host with anything pending is a protocol violation: the host drops the guest, rebuilds the file's replica from its own text in a new epoch under a new client id, and sends the others a new snapshot (`a_guest_update_left_pending_makes_the_host_rebuild`). Rebuilding costs about a millisecond for a 1 MiB page (seeding, below).
   - A size limit before decoding, a panic guard around it, and a check of authorship (finding 6) are still required: a sample is not a proof. COLLAB 2/8 adds a fuzz target.
5. **Threads.** Without the `sync` feature, `yrs`'s undo manager is not `Send`, so it could not live in an engine that moves between threads. With `sync` it is, at no measurable cost (below), and the dependency tree is unchanged at 34 crates.
6. **Authorship can be checked before applying.** `Update::insertions` names the clients whose items an update creates. The host assigns each guest its client ID at join and refuses a guest's update that creates items for any other. Deletions name no author: a guest with edit rights may delete the host's text, as an editor may. A guest's undo of a deletion and its redo of an insertion create new items under the guest's own id, so the rule never refuses a guest's own undo (`a_guest_undo_and_redo_name_only_the_guest`).
7. **Undo grouping can be the engine's.** With `capture_timeout_millis` at its maximum and `tracked_origins` holding only the local origin, `UndoManager` groups only where the replica calls `reset`. So the engine's `COALESCE_WINDOW` rules can decide groups, and tests don't depend on a clock. Remote transactions, made with another origin, never enter the local history.
8. **Whole-text checks are too slow for every keystroke.** Comparing the rope with the replica's whole text costs about 90 µs a keystroke on a 1 MiB page, ten times what `yrs` costs. The bridge compares each delta with the edits it asked for. It compares the whole text only in tests, after a resynchronisation, and before a save.

## Measurements

A 1,048,587-byte AsciiDoc page, release build, `sync` on, no whole-text check (`measure_a_1_mib_session_yrs_alone`):

| What | Result |
|---|---|
| Seeding the page as one transaction | 1.0 ms |
| Snapshot | 1,048,602 bytes; a new replica joins from it in 0.8 ms |
| 20,000 keystrokes, typed a word at a time with backspaces, each one transaction applied on a second replica | 17.9 update bytes a keystroke; local p50 9.3 µs, p95 24.5 µs; remote p50 8.5 µs, p95 15.4 µs |
| A 480-byte paste / deleting it | 496 bytes / 9 bytes |
| Reconnecting after missing 2,000 keystrokes | 5,500 bytes, encoded in 12 µs, applied in 73 µs |
| An undo | 22 µs; 9 bytes |
| State after the session | 1,098,896 bytes for 1,073,546 bytes of text |
| Peak resident memory of the process | 17 MB (`/usr/bin/time -l`) |

With the whole-text check on every keystroke, local p50 is 103 µs and peak memory 28 MB.

A small page (4,283 bytes, `measure_a_small_page`): a 4,297-byte snapshot; 2,000 keystrokes at 15.8 update bytes each; local p50 8.0 µs, p95 9.2 µs; remote p50 8.2 µs, p95 9.6 µs. So `yrs`'s cost is per transaction, not per byte of the page.

A clean release build of the spike's library, `yrs` and its tree included, takes 7 s.

These figures set COLLAB 8/8's budgets:
- local typing on a shared 1 MiB page within the engine's existing `keystroke_insert` budget (100 µs);
- a remote keystroke applied in under 50 µs;
- update bytes per keystroke under 64.

## The alternatives

`yrs` passed every test the design depends on, so the alternatives were not built.

| Crate | Licence | Crates in its tree (normal dependencies, distinct names) | Notes |
|---|---|---|---|
| `yrs` 0.28.2 (`sync`) | MIT | 34; only `smallstr` is new to the workspace | Yjs-compatible updates and state vectors, `OffsetKind::Bytes`, `UndoManager` with tracked origins, `StickyIndex`. 72 `unsafe` mentions in its source. Every build script in its tree belongs to a crate the workspace already builds |
| `automerge` 0.12.0 | MIT | 40 | Not built. Rust 1.90 or later |
| `loro` 1.18.0 | MIT | 95 | Not built |

`cargo deny check` (advisories, bans, licences) passes on the spike's tree against the repository's `deny.toml`. The architecture document's earlier note named `loro` 1.17 and `automerge` 0.12 "if collaboration ever becomes a goal". It now names `yrs`.

## Still to check

- Report finding 1 upstream with a reduced reproduction, if the maintainer wants it reported.
- The fuzz target for the update decoder (COLLAB 2/8), and the size and depth limits it holds.
- Everything about the transport, which this package designs but does not build (COLLAB 4/8).
