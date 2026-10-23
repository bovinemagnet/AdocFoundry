# GPUI Spike (WP 1.1) — Record

| | |
|---|---|
| Author | Paul Snow |
| Date | 2026-10-23 |
| Snapshot | `gpui-pre` 0.3.9 (zed@93d7456), `gpui-pre-platform` 0.3.9, `gpui-component` 0.8.0, `gpui-kit-assets` 0.8.0 |
| Verified on | Linux x86_64, KDE Plasma 6 on Wayland, and the X11 backend through XWayland |
| Decision | **Go**: continue on GPUI. Not yet verified on macOS and Windows (built by the CI matrix) |

The spike was folded into the application crate rather than built as a throwaway: `crates/adocfoundry` is the spike, and the items below point at the code and tests that demonstrate each criterion.

## Criteria

### 1. Editor with the AsciiDoc grammars, smooth on a 5,000-line file — *partially verified*

- gpui-component's code editor shows `.adoc` files with the block grammar and the inline grammar injected into paragraphs, titles, table cells and macro targets, plus source-block injection (`crates/adocfoundry/src/editor/language.rs`). Every query compiles against the pinned grammars (`queries_compile_against_the_grammars`).
- The engine's work per keystroke — rope edit, re-parse, index replacement and diagnostics for open documents — was measured on a 5,008-line synthetic document with `cargo test --release -p adoc-engine --test perf -- --ignored --nocapture`: **median 0.76 ms, worst 3.8 ms** over 200 keystrokes, inside the 5 ms budget. Workspace diagnostics for 501 files take 2.3 ms.
- **Not yet done:** a frame-time capture of a 10-second typing burst. The keystroke path leaves more than 12 ms of each 16 ms frame to the widget, but the 60 fps claim needs the frame counter on each platform (see `docs/qa/phase1.md`).

### 2. `definition_at_offset` wired in-process to a click — *verified*

The editor's definition provider calls the engine, which calls `adoc_lang::definition_at_offset`; a modifier-click or F12 runs `navigate.follow_link`, which opens and activates the target and records history (`crates/adocfoundry/src/editor/providers.rs`). Tested headlessly by `following_a_link_opens_a_tab_and_records_history`.

### 3. A second window sharing state — *verified*

`app::open_secondary_window(WindowKind::Mirror)` opens a read-only view of the active document that follows its edits. Tested headlessly by `a_second_window_mirrors_the_active_document`. Phase 2 adds `WindowKind::Preview` beside it.

### 4. IME composition — *not verified*

Needs manual testing with Japanese input on macOS and Windows and ibus on Linux; listed in `docs/qa/phase1.md`. gpui-component's editor implements GPUI's `EntityInputHandler`, which carries IME composition, but that is not evidence it works here.

### 5. One snapshot upgrade during the spike — *not possible yet*

0.3.9 is the latest `gpui-pre` on crates.io as of this record, and gpui-component 0.8.0 pins `gpui-pre =0.3.9`. Repeat this item when the next snapshot and a matching gpui-component appear, and record the changes it needs here.

## Findings that shaped the design

- **Provider traits are LSP-typed.** gpui-component's definition, hover and completion hooks take and return `lsp-types` values. Rewriting those widgets would cost far more than one translation module, so `adocfoundry` depends on `lsp-types` as a single, listed exception in `xtask/src/layers.rs`; `lsp-server` stays confined to `adoc-ls`.
- **The widget reports changes, not edits.** `InputEvent::Change` carries no range, so the editor recovers each edit as the replacement between the common prefix and suffix of widget and engine text (`editor/sync.rs`). Typing still reaches the engine one character at a time, so undo coalescing works (`typing_reaches_the_engine_and_undo_goes_through_it`).
- **Undo must bypass the widget.** The app binds undo, redo and definition in the editor's key context after gpui-component's own bindings, so they take precedence and go through the engine.
- **Unfocused X11 windows repaint lazily.** A screenshot of an unfocused window under XWayland showed a stale frame until the window received input. This affects screenshot tooling, not users.
- **Folding comes from the syntax tree.** gpui-component folds multi-line nodes itself and does not read `folds.scm`.

## Real-world check

The release build was run on a real five-module Antora component (`pg-console/docs`). Its navigation tree matched the `nav.adoc` files entry for entry, every target resolved, the outline nested by section, and highlighting covered the header, attributes, inline formatting, tables and xrefs. `adocfoundry-cli check` over 25 local Antora components found no false positives once two parser rules were corrected to match Asciidoctor (includes only as whole-line directives; block images only at the start of a line or table cell); the remaining warnings were genuinely missing images and anchors.

## Platform status

| Platform | Builds | Runs | Notes |
|---|---|---|---|
| Linux x86_64, Wayland | yes | yes | Verified by hand: navigation tree, editor, highlighting, diagnostics, outline |
| Linux x86_64, X11 | yes | yes | Through XWayland |
| macOS (Apple Silicon, Intel) | CI | not yet | The engine, CLI and language server cross-check for `aarch64-apple-darwin` from Linux; the app's C build scripts (tree-sitter, the grammars) need the Apple SDK, so CI builds it |
| Windows x64 | CI | not yet | |
