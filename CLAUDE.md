# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

AdocFoundry is a native desktop editor for AsciiDoc documentation laid out the Antora way, written in Rust for macOS, Linux and Windows. It is built as a thin GPUI shell over a headless engine, with the language crates that began life in `bovinemagnet/zed-asciidoc` as its foundation.

The architecture and phase plan live in `docs/design/DESIGN-AdocFoundry_Architecture.md`. Each phase has an implementation specification under `docs/prd/`; the spec for the phase in progress is the source of truth for scope. `README.md` tracks what is and is not implemented and must be updated when setup or visible behaviour changes.

Phases, in order: (1) source editor with Antora support and link following; (2) native rendered preview; (3) WYSIWYG editing; (4) PDF export with themes; (5) release readiness, fixes, typeset STEM, `adoc-ls` on the engine and an MCP server. Do not start a later phase's features unless the current phase's spec says so or you are asked.

**Current state:** Phases 1 to 4 are implemented (work packages 1.1–1.9, 2.1–2.8, 3.1–3.8 and 4.1–4.7). Phase 5 is specified in `docs/prd/PRD-AdocFoundry_Phase_5_Implementation_Specification.md` and not started. Where the implementation departs from a spec, its "Implementation Notes" section records it (§20 of the Phase 1 spec, §12 of the Phase 2 spec, §11 of the Phase 3 and Phase 4 specs); read them before changing those areas. The manual checks that automated tests cannot cover are listed in `docs/qa/phase1.md` (IME, frame timing, macOS and Windows runs, packages), `docs/qa/phase2.md` (the preview in the app), `docs/qa/phase3.md` (an hour of visual editing and its Git diff) and `docs/qa/phase4.md` (PDFs side by side with asciidoctor-pdf, validators, the app's export), and the GPUI spike's findings are in `docs/qa/spike-gpui.md`.

## Commands

```sh
cargo check --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings   # CI treats warnings as errors
cargo test --workspace
cargo run -p adocfoundry                                 # launch the desktop app
cargo run -p adocfoundry-cli -- check tests/fixtures/antora-single-component
cargo run -p adocfoundry-cli -- pdf tests/fixtures/pdf/reference/article.adoc --output /tmp/article.pdf
cargo run -p adoc-ls -- --version                        # smoke-test the language server
cargo deny check                                         # licences and advisories
cargo xtask layers                                       # the crate ladder and confinement rules
cargo xtask package                                      # release build and archive in dist/
git submodule update --init                              # the grammars, needed to build the app
```

`default-members = ["crates/adocfoundry"]`, so a bare `cargo test` or `cargo check` only covers the app crate. **Always pass `--workspace`** (or `-p <crate>`) when working on engine or language crates.

Targeted runs:

```sh
cargo test -p adoc-engine                          # one crate
cargo test -p adoc-antora resolves_cross_component # single test by name substring
cargo test -p adocfoundry-cli --test cli           # one integration test file
cargo test -p adoc-model --test roundtrip          # the visual editor's round-trip properties
cargo test -p adocfoundry-cli --test fidelity      # render vs Asciidoctor; skips itself without asciidoctor
cargo test --release -p adoc-engine --test perf -- --ignored --nocapture   # timing probe (ignored by default)
cargo test -p adoc-pdf --test themes               # theme loader against asciidoctor-pdf's key maps
cargo test -p adocfoundry-cli --test pdf_regression   # PDFs vs asciidoctor-pdf, rasterised; skips without it
ADOC_PDF_OUT=/tmp cargo test -p adoc-pdf --test reference -- --ignored   # write the reference PDFs and Typst
```

The CLI and engine integration tests use `insta` snapshots (`tests/snapshots/`); review a changed snapshot rather than overwriting it blindly.

The user and developer documentation is an Antora site in `src/docs` (modules `ROOT` and `developer`). Gradle exists only to build it: `gradle25w antora` builds the site to `build/site` and reports errors. Nothing in Gradle is needed to build or run the app.

The CI and release workflows are written but kept in `packaging/ci/`, not `.github/workflows/`, so they do not run yet; macOS and Windows are verified by hand first. The app crate needs a GPU-capable environment; CI is set up to run `cargo test --workspace --exclude adocfoundry` and then only builds the app crate (`cargo build -p adocfoundry`). Never add a test to `crates/adocfoundry` that needs a display when the same behaviour can be tested in `adoc-engine`.

## Architecture

The workspace is a layered crate ladder. A crate may depend only on crates in the layers below it, apart from the same-layer pairs listed in `SAME_LAYER_ALLOWED` in `xtask/src/layers.rs` (`adoc-index` and `adoc-lang` on `adoc-parser`; `adoc-lang` on `adoc-index` and `adoc-antora`; `adoc-pdf` on `adoc-render`). The dependency direction is enforced by `cargo xtask layers` in CI; do not add a dependency that violates it. A new crate needs an entry in `LAYERS` there.

```
L4  adocfoundry (GPUI app)   adocfoundry-cli (headless)   adoc-ls (LSP, stdio)
L3  adoc-engine     sessions, documents, command registry, undo, watcher, settings, events
L2  adoc-render     adoc-diagrams     adoc-model     adoc-git     adoc-pdf
L1  adoc-parser     adoc-dom     adoc-antora     adoc-index     adoc-lang
L0  adoc-core       domain types only; no internal deps; no editor, LSP or UI types
```

- **`adoc-core`** — `Document`, `LineIndex`, `SourceRange` (byte offsets), `Section`, `Anchor`, `Reference`, `IncludeDirective`, `ImageDirective`, `Diagnostic`/`DiagnosticCode`. Diagnostic codes are stable strings (`adoc.unresolved-xref-file`, `adoc.antora.unknown-module`). Never add dependencies here.
- **`adoc-parser`** — line-oriented semantic parser (`parse(uri, text) -> ParseResult`), deliberately *not* tree-sitter and *not* a full AST. It extracts titles, sections, attributes, anchors, xrefs, includes and images from half-typed input without panicking. Keep it fast; it runs on every keystroke-debounce.
- **`adoc-dom`** — the full AST behind `trait DocumentParser`, with spans that map through includes. `AcdcParser` (acdc-parser) is the one the engine uses; `AsciidocParser` (asciidoc-parser) exists for comparison. External parser crates are only ever referenced inside this crate.
- **`adoc-render`** — the framework-free view tree (`build_view`, references, images and diagrams resolved through a `ViewResolver`), `block_at` for scroll sync, and the HTML backend (`to_html`). The GPUI preview, the CLI and the fidelity harness all render from the same tree.
- **`adoc-diagrams`** — Mermaid in process (merman), Graphviz, PlantUML and D2 through optional tools on `PATH`, and a checksum cache in `<workspace>/.cache/adocfoundry/diagrams`.
- **`adoc-model`** — the visual editor's model, built from `adoc-dom`'s tree and the page text. Its scanner reads each text block losslessly into pieces (text, mark delimiters, atoms) and the build checks that reading against the DOM, making any block where they disagree read-only. Steps (`Model::apply`) return the smallest source edits, the selection after, and the updated model for steps that change no structure (structural steps leave the model to be rebuilt). Inline edits are verified by rescanning: typed markup is escaped only where it would form a construct, marks keep their delimiters, and a step that cannot keep the meaning of the text around it is refused. The round-trip property tests in `tests/roundtrip.rs` are the phase's Definition of Done in test form; keep them passing.
- **`adoc-git`** — Git over `gix` (pure Rust, no network transports): `Repository::discover`, head, status, line diffs against `HEAD` of a buffer's text, file content at `HEAD`, remote URLs. Never runs the `git` executable. Its `test-support` feature provides `testing::TestRepo`, temporary repositories built with `gix`, for any crate's tests; never test against a developer's checkout.
- **`adoc-pdf`** — PDF export: `theme/` loads asciidoctor-pdf 2.3 YAML themes exactly as its `ThemeLoader` does (checked key by key against golden maps in `tests/fixtures/pdf/themes/expected/`); `emit.rs` writes the view tree as Typst with every document string a Typst string literal, so a document can never inject Typst code (keep it that way: only numbers and enum values are ever spliced); `adoc.typ` is the template, reading the theme's keys as `t` and document facts as `doc`; `world.rs` is the Typst `World` (only listed files, fonts registered under the theme's family names, no packages). The template reproduces Prawn's spacing model (block spacing zero, explicit bottom margins, half-leading per paragraph, line gaps, `keep()` for blocks asciidoctor-pdf keeps together); read spec §11 before changing spacing and run `cargo test -p adocfoundry-cli --test pdf_regression` afterwards. Typst is pinned at 0.15.1.
- **`adoc-antora`** — `antora.yml` parsing (`serde-saphyr`), `AntoraCatalog`, `parse_resource_id` for `version@component:module:family$path`, `AntoraResolver`. Resolution follows Antora 3.2 rules; the docs for those rules are linked from the design doc, not re-derived.
- **`adoc-index`** — `WorkspaceIndex` with replacement-based updates and direct map lookups. Never add a feature that scans every document. `normalize_path` is lexical and never canonicalises, so unsaved files work.
- **`adoc-lang`** — editor-agnostic language features operating on byte offsets: definition, hover, completion, diagnostics, document and workspace symbols, attribute resolution. This is the former `adoc-ls/src/handlers` and the non-LSP parts of its `state.rs`. Both `adoc-ls` and `adoc-engine` call it; neither re-implements it.
- **`adoc-engine`** — the only entry point front ends use. Owns the workspace (roots, catalog, index), open documents, the command registry, undo history, settings and the file watcher, and publishes `EngineEvent`s. Every user-visible action is a `Command` with a stable `CommandId`; the UI, CLI and future MCP server invoke commands, never engine internals. Each document has an `EditorMode`; while it is shown visually the engine keeps a `VisualDocument` (model and view tree from one parse, `src/visual.rs`). Visual commands take `CommandArgs::Visual` (a selection in source offsets and the document version it belongs to) and apply model steps as ordinary edits in the document's one undo history. A change from elsewhere makes the model stale: source typing rebuilds it after `MODEL_SETTLE`, any other command before `invoke` returns. `Engine::git()` holds each root's repository, file status and the hunks of open documents (`src/git.rs`); discovery, status and diffs run on workers, hunks recompute `GIT_SETTLE` after typing, and the watcher also watches `.git/HEAD`, `index`, `packed-refs` and `refs/`. Commands that act on changes (`git.revert_hunk`, `git.next_change`) use `Engine::hunks_now`, computed from the current text, never the debounced hunks. `git.push` (and a commit when `commit.gpgsign` is set) is the one place that runs the `git` executable (`[tools] git` or `PATH`), so credential helpers and signing work; everything else stays in `gix`. `[git] enabled = false`, or no repository, means no Git state and no Git events. PDF export is `export.pdf_page`/`export.pdf_component` with `CommandArgs::Export` (`src/export.rs`): pages are parsed on a worker, views built and diagrams awaited on the engine thread, then the document is typeset and written on a worker and reported by `EngineEvent::ExportFinished`; `export_blocking` serves the CLI. The theme is the export's own, else the page's `pdf-theme`, else `[pdf] theme`.
- **`adoc-ls`** — the only crate that knows about LSP. `protocol.rs` maps `adoc-lang` results to `lsp-types`; nothing below it may import `lsp-types` or `lsp-server`. The one listed exception (`LSP_ALLOWED` in `xtask/src/layers.rs`) is `adocfoundry` using `lsp-types` in `editor/providers.rs`, because gpui-component's editor provider traits are typed with it; keep it to that module.
- **`adocfoundry`** — the only crate that imports `gpui`, `gpui-component` or any other UI framework. Views are thin: they render engine state and dispatch commands. Logic that could live in `adoc-engine` must live there. The editor widget reports only that its text changed, so `editor/sync.rs` recovers each edit as one replacement and sends it as `edit.insert_text`/`edit.replace_range`; engine-side changes are applied back the same way. Undo, redo and definition are bound in the editor key context after gpui-component's bindings so they reach the engine. Tabs follow the engine's active document (`EngineEvent::NavigationChanged`). The visual editor (`src/visual/`) draws the engine's `VisualDocument`: text blocks are laid out with their layouts kept for hit testing, the selection is a pair of source offsets mapped through edits made elsewhere, typing and IME arrive through the platform input handler, and read-only nodes are drawn by the preview's block renderer. A block begun with Enter at the end of another exists only in the view until typed into. The preview (`src/preview/`) draws the engine's view tree natively, docked or in a `WindowKind::Preview` window; pin, scroll sync and detach are engine state (`PreviewLayout`), and clicks go back through `navigate.go_to` and `navigate.reveal`. App tests in `src/ui_tests.rs` run on GPUI's test platform and need no display.
- **`adocfoundry-cli`** — headless binary over `adoc-engine`. Subcommands: `check <root> [--format text|json] [--fail-on warning|error]`, `symbols <root> <query>`, `follow <file> <byte-offset>`, `render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]`, `pdf <file|component-dir> [--component [<version>@]<name>] [--theme …] [--fonts-dir …] [--output …] [--standard …] [--typst …]`, `git-status <root> [--format text|json]`. Only `git-status` turns Git on. Used in CI and for dogfooding.

Request flow: a keystroke edits the editor buffer → `Engine::apply_edit` updates the document, runs `adoc-parser`, replaces the index entry, publishes `EngineEvent::DocumentChanged` → views re-render from engine state. Nothing in the UI holds its own copy of document semantics.

Preview flow: while the preview is visible the engine requests its target (the pinned document, else the active one) → `PREVIEW_SETTLE` (100 ms) after the last change to the page or any file it includes, a worker parses a snapshot through `adoc-dom` → `pump` builds the view tree on the engine thread (resolution needs the catalog and index) → diagrams come from the cache or render on a worker → `EngineEvent::PreviewReady` → views read `rendered_preview(id)`. A result is dropped when a newer job started or an input changed after it began.

### Grammars and assets

`grammars/tree-sitter-asciidoc` is a git submodule pinned to the `zed` branch of `bovinemagnet/tree-sitter-asciidoc`. tree-sitter is for highlighting, folding, outline hints and injections only — never for attribute evaluation, include expansion, Antora coordinates, cross-file references or rendering. The `.scm` queries live in `crates/adocfoundry/languages/`.

## Conventions that must be preserved

- Layering is absolute: no upward dependencies, no UI types below `adocfoundry`, no LSP types below `adoc-ls`, no external parser crates outside `adoc-dom`.
- Byte offsets everywhere (`SourceRange`). Line/column conversion happens at the edge (`adoc-ls` for LSP, the editor view for the caret) using `LineIndex`.
- Diagnostics stay conservative: prefer no diagnostic over a false positive, warnings over errors for unresolved references, and no diagnostics at all for dynamic constructs (`include::{attr}/x.adoc[]`).
- Incomplete AsciiDoc is normal input. Parsers, resolvers and handlers must not panic on half-typed syntax; add a regression test for every panic fixed.
- Every mutation of a document goes through a `Command` and is recorded in undo history. No view writes to a buffer directly.
- `unsafe_code = "forbid"` workspace-wide; avoid global mutable state; `anyhow` only in binaries, typed errors in libraries.
- Edition 2021 (matching the migrated crates); the toolchain is pinned in `rust-toolchain.toml`. Crates take dependency versions from `[workspace.dependencies]`, never their own.
- All external dependencies are pinned exactly (`=1.0.229` style) and new ones must pass `cargo deny` (MIT, Apache-2.0, BSD-2/3, MPL-2.0, Unlicense, Zlib, OFL-1.1 for fonts; GPL and LGPL are denied).
- `vendor/evalexpr` is a home-grown Apache-2.0 stand-in, patched in through `[patch.crates-io]`, for the AGPL `evalexpr` that acdc-parser pulls in (see `vendor/README.md`). Do not remove the patch while acdc-parser still depends on it.
- `vendor/comemo` is comemo 0.5.1 with two changes that free the memory of evicted caches (gigabytes after a book export, which the app would otherwise keep), patched in the same way. Keep it at the comemo version Typst uses, and drop it once a release frees evicted memory (see `vendor/README.md`).
- Keep the UI framework a one-crate swap (egui/eframe was the fallback had the GPUI spike failed): nothing below `adocfoundry` may know which framework is in use.
- **Never** depend on Zed's `editor`, `text`, `rope`, `multi_buffer`, `language` or `project` crates: they are GPL-3.0. The permitted GPUI crates are `gpui`/`gpui-pre`, `gpui-component`, `gpui-base` and their platform crates (Apache-2.0).
- No Node.js, no JVM, no Ruby required at runtime. External tools (`asciidoctor`, `dot`, `plantuml`, `mmdc`) are optional and discovered, never assumed.
- Use `Path`/`PathBuf` for filesystem work and `adoc_index::normalize_path`; do not canonicalise paths that may not exist. Antora resource IDs always use `/`.
- Do not clone remote Antora repositories or execute project scripts.
- Settings are TOML, read through `adoc-engine::Settings`; the UI never reads the settings file itself.

## Testing

Unit tests live in `#[cfg(test)] mod tests` next to the code. Integration tests: `crates/adoc-engine/tests/` drives the engine headlessly through commands; `crates/adocfoundry-cli/tests/cli.rs` drives the binary via `CARGO_BIN_EXE_adocfoundry-cli`; `crates/adoc-ls/tests/stdio.rs` drives the language server through an initialise/shutdown lifecycle.

Shared fixtures are in `tests/fixtures/`, grouped by scenario (`simple/`, `includes/`, `xrefs/`, `antora-single-component/`, `antora-multi-component/`, `antora-versions/`, and `render/` for the fidelity harness), reached with `Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/...")` — never absolute developer-machine paths. Keep fixtures small and readable; add one alongside every resolver, parser, engine or command behaviour change.

Tests that spawn a real process must take the crate-private `spawn_guard()` mutex in `adoc-render` (the `ETXTBSY` workaround). Do not remove the guard or add unguarded spawning tests.

A pull request is done when: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check` and `cargo xtask layers` all pass, `README.md` reflects any visible change, and the phase spec's acceptance test for the work package has a test with that name.

## Working rules for Claude Code

- Work one work package at a time, in the order the phase spec gives; each package leaves the workspace compiling and green.
- Read the relevant section of the phase spec and the crate's existing code before writing. The spec gives trait signatures; implement them as written and raise a question in the PR description if a signature cannot work, rather than silently changing it.
- Prefer extending an existing crate over adding a new one; a new crate needs a line in the ladder above and in the layering check.
- When a dependency is needed, check `Cargo.lock` for one already in the tree before adding another, pin it exactly, and run `cargo deny check`.
- Do not refactor crates outside the package you are working on unless the spec says the package includes that refactor.
- If the spike or a package shows the spec is wrong, update the spec in the same PR and say so in the description.
- Commit messages: short imperative (`Add cross-component resolution to adoc-antora`). One package per PR, titled `WP <phase>.<n>: <package name>`; the PR description lists the spec section, tests added and the commands run.
