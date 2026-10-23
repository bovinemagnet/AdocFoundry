# Vendored stand-ins

## `evalexpr`

acdc-parser (the `adoc-dom` parser) depends on `evalexpr = "13"`, but `evalexpr` 12 and later are licensed AGPL-3.0-only, which AdocFoundry's permissive-only licence policy does not allow. acdc-parser uses exactly two functions from it, `eval_float` and `eval_int`, to evaluate arithmetic operands of `ifeval::[]` conditions.

`vendor/evalexpr` is AdocFoundry's own Apache-2.0 implementation of those two functions (numbers, `+ - * / %`, unary minus and parentheses), substituted for the crates.io package through `[patch.crates-io]` in the workspace `Cargo.toml`. It contains no code from the original crate. Remove the patch if acdc-parser drops the dependency or a permissively licensed release appears.

## `comemo`

Typst memoises its work through `comemo` 0.5.1, in process-wide caches. `adoc-pdf` empties them after each compile with `comemo::evict(0)`, but comemo keeps the memory those caches grew to: the call trees' slabs and maps, and the accelerator tables, which it clears but does not free. After exporting a 2,000-page book a long-running process (the app) kept about 15 GB allocated, reused by the next export but never returned.

`vendor/comemo` is comemo 0.5.1 (MIT OR Apache-2.0, its licences alongside) with two changes, substituted through `[patch.crates-io]`:

- `src/tree.rs`: `CallTree::retain` starts the tree afresh when no outputs are left, as nothing in it can be reached.
- `src/accelerate.rs`: `evict` drops the accelerators instead of clearing them. The eviction already moves the offset past every live ID, so none could be reached again.

comemo's own tests pass with both. After a 709-page book the memory still allocated fell from 4.5 GB to 60 MB, what parsing alone leaves. Remove the patch when a comemo release that Typst uses frees evicted memory, and update it with Typst's comemo version until then.

