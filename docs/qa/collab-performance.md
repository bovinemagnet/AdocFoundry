# Collaborative Editing — Performance

What collaboration costs (COLLAB 8/8, #695; PRD §9), measured against the budgets the spike set (`docs/qa/spike-collab.md`). Every figure comes from `collab_performance_probe` in `crates/adoc-share/tests/performance.rs`:

```sh
cargo test --release -p adoc-share --test performance -- --ignored --nocapture
```

`ADOC_PERF_ROUNDS` sets how many remote updates are timed (200 by default).

**Environment:** Apple M4 Pro, macOS 26, rustc 1.99.1, release build, 2026-11-21. The host and guest are engines in one process, meeting over loopback TLS 1.3. Loopback is not a LAN: these are the engines' and the transport's own costs, with no network's added.

## Results

| Measure | Small page (4 KiB) | 1 MiB document |
| --- | --- | --- |
| Typing, alone, p50 / p95 | 0.8 µs / 0.8 µs | 0.7 µs / 0.8 µs |
| Typing in the middle, hosting, p50 / p95 | 4.6 µs / 5.8 µs | 3.5 µs / 4.8 µs |
| Typing at the end, hosting, p50 / p95 | 3.2 µs / 4.3 µs | 3.0 µs / 4.0 µs |
| Remote update, loopback, p50 / p95 | 36.3 µs / 47.7 µs | 119.2 µs / 122.6 µs |
| Initial snapshot | 4,121 B; joined in 3 ms | 1,048,604 B; joined in 13 ms |
| Reconnection, one page | under 1 ms | 1 ms |

| Measure | |
| --- | --- |
| Update per character typed | 29.9 B |
| Update per 400-byte paragraph | 423 B |
| Snapshot after 20,000 edits (half deletions) on a 64 KiB page | 110,904 B to 406,626 B |
| Joining with ten 16 KiB pages open | 5 ms |

Typing is the time `edit.insert_text` takes on the engine's thread, with the host's update taken for sending. A remote update runs from the host's keystroke until the guest's rope has it: the host's pump, the transport both ways, and the guest applying it. The probe waits on the rope's length, so no copy of the text is timed.

## Against the budgets

| Budget (spike) | Measured | |
| --- | --- | --- |
| Local typing on a shared 1 MiB page within the engine's `keystroke_insert` budget (100 µs) | 3.0–4.8 µs p95, middle or end | met |
| A remote keystroke applied in under 50 µs | 36 µs on a small page, transport included; 119 µs on a 1 MiB page, transport included, of which the guest's apply is about 85 µs | met on small pages; about 1.7 times over on 1 MiB |
| Update bytes per keystroke under 64 | 29.9 B | met |

## What the measurement found

The first measurements were far off two budgets. Both causes were in the engine, not in the defences, and both are fixed:

- **A guest's page was parsed whole on every update.**
  - A guest's shared page is an untitled document, and the engine refreshed an untitled document's diagnostics by parsing it whole after every edit.
  - On a 1 MiB page that was 7.8 ms for each update from the host, and for each keystroke the guest made.
  - The page is now parsed once typing pauses (`INDEX_SETTLE`), as an indexed page is indexed, and the guest's shared index with it (`a_guests_page_is_parsed_once_typing_pauses`).
- **Typing at the end of a long page was proportional to the page.**
  - The host's replica held the whole text under the host's own `yrs` client, and `yrs` merges a client's consecutive items.
  - Each character typed at the end was merged into the whole text, which was split again for the next: 1.3 ms a keystroke at the end of 1 MiB, against 12 µs in the middle.
  - `Replica::new` now writes the text under a client of its own, derived from the host's, which never types.
  - The cost: each typed item names that client as its neighbour, so an update per character grew from 16 to 30 bytes, and a long session's snapshot grows faster. Both are within the budgets.

Timed alone on a 1 MiB page, the replica's remote apply, whole-text check included, takes about 130 µs, and the engine's whole remote apply about 85 µs. The whole-text check stays as it is.

**Growth.** Bytes per edit stay constant over a page's size and history. `collab_update_sizes_stay_small` holds them on every run (a character at most 32 bytes, a paragraph at most its length plus 32), so CI catches unbounded growth without timing anything. A long session's snapshot grows with its history of deletions, as the spike found (`a_long_session_grows_with_its_text` bounds it).
