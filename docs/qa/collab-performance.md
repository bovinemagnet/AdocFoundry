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
| Typing, alone, p50 / p95 | 0.8 µs / 0.9 µs | 0.7 µs / 0.7 µs |
| Typing, hosting, p50 / p95 | 4.5 µs / 5.7 µs | 3.6 µs / 4.9 µs |
| Remote update, loopback, p50 / p95 | 75.6 µs / 85.4 µs | 5,133 µs / 5,303 µs |
| Initial snapshot | 4,121 B; joined in 3 ms | 1,048,604 B; joined in 13 ms |
| Reconnection, one page | under 1 ms | 1 ms |

| Measure | |
| --- | --- |
| Update per character typed | 16.4 B |
| Update per 400-byte paragraph | 417 B |
| Snapshot after 20,000 edits (half deletions) on a 64 KiB page | 108,157 B to 289,126 B |
| Joining with ten 16 KiB pages open | 5 ms |

Typing is the time `edit.insert_text` takes on the engine's thread, with the host's update taken for sending. A remote update runs from the host's keystroke until the guest's text shows it: the host's pump, the transport both ways, and the guest applying it.

## Against the budgets

| Budget (spike) | Measured | |
| --- | --- | --- |
| Local typing on a shared 1 MiB page within the engine's `keystroke_insert` budget (100 µs) | 3.6 µs p50, 4.9 µs p95 | met |
| A remote keystroke applied in under 50 µs | 76 µs on a small page over loopback, transport included; **5.1 ms on a 1 MiB page** | missed on large pages |
| Update bytes per keystroke under 64 | 16.4 B | met |

**The 1 MiB remote update.** With the guest's shared index refresh switched off, 4.9 ms of it remains. That remainder grows with the page and not with the edit, and the code points to the replica's checks after each remote update (`Replica::apply_remote`). A sample of the release build was taken without symbols, so this is an inference from the code, not a resolved profile. The whole text is rebuilt from `yrs` and compared with the text the update should give. This is the defence against the defects fuzzing found in `yrs` (COLLAB 2/8). The spike measured the replica alone with no such check, and expected whole-text checks only in tests, after a resynchronisation and before a save. The cost grows with the page, about 5 ms per MiB, so it is under half a millisecond on pages of tens of kilobytes. Keeping or narrowing the check is an open release decision (`docs/qa/collab.md`).

Refreshing the guest's shared index from the page's own parse, rather than parsing it again, took the same measure from 7.4 ms to 5.1 ms (COLLAB 8/8).

**Growth.** Bytes per edit stay constant over a page's size and history: `collab_update_sizes_stay_small` holds them on every run (a character at most 32 bytes, a paragraph at most its length plus 32), so CI catches unbounded growth without timing anything. A long session's snapshot grows with its history of deletions, as the spike found (`a_long_session_grows_with_its_text` bounds it).
