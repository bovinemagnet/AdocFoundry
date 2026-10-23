# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

AdocFoundry is a native desktop editor for AsciiDoc documentation laid out the Antora way, written in Rust for macOS, Linux and Windows. It is built as a thin GPUI shell over a headless engine, with the language crates that began life in `bovinemagnet/zed-asciidoc` as its foundation.

The architecture and phase plan live in `docs/design/DESIGN-AdocFoundry_Architecture.md`. Each phase has an implementation specification under `docs/prd/`; the spec for the phase in progress is the source of truth for scope. `README.md` tracks what is and is not implemented and must be updated when setup or visible behaviour changes.

Phases, in order: (1) source editor with Antora support and link following; (2) native rendered preview; (3) WYSIWYG editing; (4) PDF export with themes. Do not start a later phase's features unless the current phase's spec says so or you are asked.

**Current state:** the repository holds only the design and Phase 1 spec (`docs/prd/PRD-AdocFoundry_Phase_1_Implementation_Specification.md`). There is no Cargo workspace yet, so the commands below apply once WP 1.2 (workspace bring-up and crate migration from `zed-asciidoc`) has landed. Work packages merge in this order: 1.1 → 1.2 → 1.5a → 1.3 → 1.5b → 1.7 → 1.4 → 1.6 → 1.8 → 1.9 (spec §16 lists each package's dependencies and normative test names).

## Commands

```sh
cargo check --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings   # CI treats warnings as errors
cargo test --workspace
cargo run -p adocfoundry                                 # launch the desktop app
cargo run -p adocfoundry-cli -- check tests/fixtures/antora-single-component
cargo run -p adoc-ls -- --version                        # smoke-test the language server
cargo deny check                                         # licences and advisories
```

`default-members = ["crates/adocfoundry"]`, so a bare `cargo test` or `cargo check` only covers the app crate. **Always pass `--workspace`** (or `-p <crate>`) when working on engine or language crates.

Targeted runs:

```sh
cargo test -p adoc-engine                          # one crate
cargo test -p adoc-antora resolves_cross_component # single test by name substring
cargo test -p adocfoundry-cli --test cli           # one integration test file
```

The app crate needs a GPU-capable environment; CI runs `cargo test --workspace --exclude adocfoundry` and then only builds the app crate (`cargo build -p adocfoundry`). Never add a test to `crates/adocfoundry` that needs a display when the same behaviour can be tested in `adoc-engine`.

## Architecture

The workspace is a layered crate ladder. A crate may depend only on crates in the layers below it. The dependency direction is enforced by `cargo xtask layers` in CI; do not add a dependency that violates it.

```
L4  adocfoundry (GPUI app)   adocfoundry-cli (headless)   adoc-ls (LSP, stdio)
L3  adoc-engine     sessions, documents, command registry, undo, watcher, settings, events
L2  adoc-render     adoc-diagrams     adoc-model     adoc-pdf          (adoc-render is migrated in Phase 1 but unused by the app; the rest arrive later)
L1  adoc-parser     adoc-dom     adoc-antora     adoc-index     adoc-lang
L0  adoc-core       domain types only; no internal deps; no editor, LSP or UI types
```

- **`adoc-core`** — `Document`, `LineIndex`, `SourceRange` (byte offsets), `Section`, `Anchor`, `Reference`, `IncludeDirective`, `ImageDirective`, `Diagnostic`/`DiagnosticCode`. Diagnostic codes are stable strings (`adoc.unresolved-xref-file`, `adoc.antora.unknown-module`). Never add dependencies here.
- **`adoc-parser`** — line-oriented semantic parser (`parse(uri, text) -> ParseResult`), deliberately *not* tree-sitter and *not* a full AST. It extracts titles, sections, attributes, anchors, xrefs, includes and images from half-typed input without panicking. Keep it fast; it runs on every keystroke-debounce.
- **`adoc-dom`** (Phase 2) — the full AST behind `trait DocumentParser`. External parser crates are only ever referenced inside this crate.
- **`adoc-antora`** — `antora.yml` parsing (`serde-saphyr`), `AntoraCatalog`, `parse_resource_id` for `version@component:module:family$path`, `AntoraResolver`. Resolution follows Antora 3.2 rules; the docs for those rules are linked from the design doc, not re-derived.
- **`adoc-index`** — `WorkspaceIndex` with replacement-based updates and direct map lookups. Never add a feature that scans every document. `normalize_path` is lexical and never canonicalises, so unsaved files work.
- **`adoc-lang`** — editor-agnostic language features operating on byte offsets: definition, hover, completion, diagnostics, document and workspace symbols, attribute resolution. This is the former `adoc-ls/src/handlers` and the non-LSP parts of its `state.rs`. Both `adoc-ls` and `adoc-engine` call it; neither re-implements it.
- **`adoc-engine`** — the only entry point front ends use. Owns the workspace (roots, catalog, index), open documents, the command registry, undo history, settings and the file watcher, and publishes `EngineEvent`s. Every user-visible action is a `Command` with a stable `CommandId`; the UI, CLI and future MCP server invoke commands, never engine internals.
- **`adoc-ls`** — the only crate that knows about LSP. `protocol.rs` maps `adoc-lang` results to `lsp-types`; nothing below it may import `lsp-types` or `lsp-server`.
- **`adocfoundry`** — the only crate that imports `gpui`, `gpui-component` or any other UI framework. Views are thin: they render engine state and dispatch commands. Logic that could live in `adoc-engine` must live there.
- **`adocfoundry-cli`** — headless binary over `adoc-engine`. Phase 1 subcommands: `check <root> [--format text|json] [--fail-on warning|error]`, `symbols <root> <query>`, `follow <file> <byte-offset>`; `render` and `pdf` come in later phases. Used in CI and for dogfooding.

Request flow: a keystroke edits the editor buffer → `Engine::apply_edit` updates the document, runs `adoc-parser`, replaces the index entry, publishes `EngineEvent::DocumentChanged` → views re-render from engine state. Nothing in the UI holds its own copy of document semantics.

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
- If the GPUI spike (WP 1.1) fails, the UI is rebuilt on egui/eframe. Keep that a one-crate swap: nothing below `adocfoundry` may know which framework is in use.
- Phase 2 hooks (`open_secondary_window`, the `Renderer` trait, the engine event stream) exist but stay empty in Phase 1. **Never** depend on Zed's `editor`, `text`, `rope`, `multi_buffer`, `language` or `project` crates: they are GPL-3.0. The permitted GPUI crates are `gpui`/`gpui-pre`, `gpui-component`, `gpui-base` and their platform crates (Apache-2.0).
- No Node.js, no JVM, no Ruby required at runtime. External tools (`asciidoctor`, `dot`, `plantuml`, `mmdc`) are optional and discovered, never assumed.
- Use `Path`/`PathBuf` for filesystem work and `adoc_index::normalize_path`; do not canonicalise paths that may not exist. Antora resource IDs always use `/`.
- Do not clone remote Antora repositories or execute project scripts.
- Settings are TOML, read through `adoc-engine::Settings`; the UI never reads the settings file itself.

## Testing

Unit tests live in `#[cfg(test)] mod tests` next to the code. Integration tests: `crates/adoc-engine/tests/` drives the engine headlessly through commands; `crates/adocfoundry-cli/tests/cli.rs` drives the binary via `CARGO_BIN_EXE_adocfoundry-cli`; `crates/adoc-ls/tests/stdio.rs` drives the language server through an initialise/shutdown lifecycle.

Shared fixtures are in `tests/fixtures/`, grouped by scenario (`simple/`, `includes/`, `xrefs/`, `antora-single-component/`, `antora-multi-component/`, `antora-versions/`), reached with `Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/...")` — never absolute developer-machine paths. Keep fixtures small and readable; add one alongside every resolver, parser, engine or command behaviour change.

Tests that spawn a real process must take the crate-private `spawn_guard()` mutex in `adoc-render` (the `ETXTBSY` workaround). Do not remove the guard or add unguarded spawning tests.

A pull request is done when: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check` and `cargo xtask layers` all pass, `README.md` reflects any visible change, and the phase spec's acceptance test for the work package has a test with that name.

## Working rules for Claude Code

- Work one work package at a time, in the order the phase spec gives; each package leaves the workspace compiling and green.
- Read the relevant section of the phase spec and the crate's existing code before writing. The spec gives trait signatures; implement them as written and raise a question in the PR description if a signature cannot work, rather than silently changing it.
- Prefer extending an existing crate over adding a new one; a new crate needs a line in the ladder above and in the layering check.
- When a dependency is needed, check `Cargo.lock` for one already in the tree before adding another, pin it exactly, and run `cargo deny check`.
- Do not refactor crates outside the package you are working on unless the spec says the package includes that refactor.
- In WP 1.2, change the migrated crates only as much as compiling in the new workspace requires; later packages do the rewrites.
- If the spike or a package shows the spec is wrong, update the spec in the same PR and say so in the description.
- Commit messages: short imperative (`Add cross-component resolution to adoc-antora`). One package per PR, titled `WP 1.x: <package name>`; the PR description lists the spec section, tests added and the commands run.
