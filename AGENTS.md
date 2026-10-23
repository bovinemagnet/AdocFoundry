# Repository Guidelines

## Project Structure

AdocFoundry is a Rust workspace: a GPUI desktop editor for AsciiDoc and Antora documentation over a headless engine.

- `crates/` holds the crate ladder. A crate depends only on crates in lower layers; `cargo xtask layers` enforces it.
  - L0 `adoc-core`: domain types, no dependencies.
  - L1 `adoc-parser`, `adoc-antora`, `adoc-index`, `adoc-lang`: parsing, the Antora model and resolver, the workspace index, and editor-agnostic language features.
  - L2 `adoc-render`: the external Asciidoctor renderer, used by `adoc-ls` preview.
  - L3 `adoc-engine`: documents, commands, undo, workspace, file watcher, settings, search and rename. The only entry point front ends use.
  - L4 `adocfoundry` (the GPUI app, the only crate importing GPUI), `adocfoundry-cli`, `adoc-ls` (the only crate speaking LSP).
- `grammars/tree-sitter-asciidoc` is a git submodule (the `zed` branch of `bovinemagnet/tree-sitter-asciidoc`); run `git submodule update --init` after cloning.
- `crates/adocfoundry/languages/` holds the tree-sitter queries. tree-sitter is for highlighting, folding and injections only.
- `tests/fixtures/` holds small AsciiDoc and Antora workspaces, grouped by scenario.
- `docs/design/` is the architecture, `docs/prd/` the phase specifications (the source of truth for scope), `docs/qa/` the manual QA records.
- `packaging/` holds icons, the desktop entry, the macOS bundle script and the Flatpak manifest; `crates/adocfoundry/wix/` the Windows installer source.

## Build, Test and Development Commands

```sh
cargo check --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo xtask layers
cargo run -p adocfoundry -- tests/fixtures/antora-multi-component
cargo run -p adocfoundry-cli -- check tests/fixtures/antora-single-component
```

`default-members` is the app crate, so a bare `cargo test` covers only the app: pass `--workspace` or `-p <crate>`.

## Coding Style and Conventions

- Follow `rustfmt` output; British spelling in prose and identifiers' documentation.
- Byte offsets (`SourceRange`) everywhere; line and column conversion happens only at the edges.
- Every user-visible action is an engine command with a stable id; every document mutation goes through a command and undo history.
- Parsers, resolvers and handlers must not panic on half-typed input; add a regression test for every panic fixed.
- Diagnostics are conservative: no diagnostic rather than a false positive, warnings rather than errors.
- Dependencies are pinned exactly in `[workspace.dependencies]` and must pass `cargo deny check`. Never depend on Zed's GPL crates (`editor`, `text`, `rope`, `multi_buffer`, `language`, `project`).
- `unsafe_code` is forbidden; `anyhow` only in binaries, typed errors in libraries.
- No Node.js, JVM or Ruby at run time; external tools are optional and discovered.

## Testing Guidelines

Add tests with every parser, resolver, engine, command or language-feature change, with a fixture where behaviour depends on files. Unit tests sit next to the code. Engine behaviour is tested headlessly in `crates/adoc-engine/tests/`; the CLI through its binary in `crates/adocfoundry-cli/tests/cli.rs` with `insta` snapshots; the language server through its binary in `crates/adoc-ls/tests/`. App tests run on GPUI's test platform and must not need a display. Tests that spawn a process take the `spawn_guard()` mutex in `adoc-render`.

## Commit and Pull Request Guidelines

Short imperative commit messages (`Add cross-component resolution to adoc-antora`). One work package per pull request, titled `WP 1.x: <package name>`, describing the spec section, the tests added and the commands run. When the work shows the spec is wrong, update the spec in the same pull request and say so.
