# Vendored stand-ins

## `evalexpr`

acdc-parser (the `adoc-dom` parser) depends on `evalexpr = "13"`, but `evalexpr` 12 and later are licensed AGPL-3.0-only, which AdocFoundry's permissive-only licence policy does not allow. acdc-parser uses exactly two functions from it, `eval_float` and `eval_int`, to evaluate arithmetic operands of `ifeval::[]` conditions.

`vendor/evalexpr` is AdocFoundry's own Apache-2.0 implementation of those two functions (numbers, `+ - * / %`, unary minus and parentheses), substituted for the crates.io package through `[patch.crates-io]` in the workspace `Cargo.toml`. It contains no code from the original crate. Remove the patch if acdc-parser drops the dependency or a permissively licensed release appears.

## `comemo`

Typst memoises its work through `comemo` 0.5.1, in process-wide caches. `adoc-pdf` empties them after each export with `comemo::evict(0)` (the live PDF preview keeps what its last 10 compiles used, `evict(10)`), but comemo keeps the memory those caches grew to: the call trees' slabs and maps, and the accelerator tables, which it clears but does not free. After exporting a 2,000-page book a long-running process (the app) kept about 15 GB allocated, reused by the next export but never returned.

`vendor/comemo` is comemo 0.5.1 (MIT OR Apache-2.0, its licences alongside) with two changes, substituted through `[patch.crates-io]`:

- `src/tree.rs`: `CallTree::retain` starts the tree afresh when no outputs are left, as nothing in it can be reached.
- `src/accelerate.rs`: `evict` drops the accelerators instead of clearing them. The eviction already moves the offset past every live ID, so none could be reached again.

comemo's own tests pass with both. After a 709-page book the memory still allocated fell from 4.5 GB to 60 MB, what parsing alone leaves. Remove the patch when a comemo release that Typst uses frees evicted memory, and update it with Typst's comemo version until then.


## `notify`

The engine's file watcher is `notify` 8.2.1. On Windows its watcher could hang a process for ever when a watch was stopped, which is how CI's Windows tests hung in a different engine test each time (#391):

1. `stop_watch` cancelled the watch's pending `ReadDirectoryChangesW`, closed the directory handle at once, then waited for the read's completion routine.
2. A read that had already completed successfully (the folder had just changed) is not affected by the cancellation. Its completion routine ran during that wait and issued a new read on the handle value just closed.
3. Windows reuses handle values. When another thread had just opened a directory, say `remove_dir_all` deleting the workspace's folder, the new read was a synchronous change notification on that thread's handle. That thread's next call on its handle waited behind it, for a change only that thread would make.

Stacks of a hung test, captured with `cdb` on a Windows runner, show exactly this: the test thread in `remove_dir_all` → `ZwQueryDirectoryFile`, and notify's thread in `stop_watch` → `handle_event` → `start_read` → `NtNotifyChangeDirectoryFileEx`. notify 9.0.0-rc.9 fixes it upstream (`stop_watch_closes_handles_after_queued_completion`).

`vendor/notify` is notify 8.2.1 (CC0-1.0, its licence alongside) with that fix backported to `src/windows.rs`, substituted through `[patch.crates-io]`, every change marked `AdocFoundry`:

- `stop_watch` marks the watch as stopping before cancelling it, and closes its handles only after the completion has run. If the cancellation itself fails, it leaks them rather than close them under a read still in flight.
- `handle_event` releases the watch's semaphore and returns at once for a stopping watch, without issuing another read.

Two tests in `src/windows.rs` (`adocfoundry_tests`, run on Windows with `cargo test --lib --manifest-path vendor/notify/Cargo.toml adocfoundry_tests`) queue a read's successful completion before stopping its watch, as upstream's test does: one checks the handle is still open when the completion runs, the other that the completion starts no new read. Both fail on the unpatched code. The race itself is too narrow to reproduce on demand from the engine: 4,500 workspaces closed while their folders changed did not hang. Remove the patch once a stable notify release with the fix is out, and move to that release.
