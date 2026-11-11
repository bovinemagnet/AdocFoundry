# Repository Guidelines

## Project Structure

AdocFoundry is a Rust workspace: a GPUI desktop editor for AsciiDoc and Antora documentation over a headless engine, with a CLI, a language server and an MCP server over the same engine.

- `crates/` holds the crate ladder. A crate depends only on crates in lower layers; `cargo xtask layers` enforces it from `LAYERS` in `xtask/src/layers.rs`, and a new crate needs an entry there.
  - L0 `adoc-core`: domain types (`Document`, `SourceRange`, `Diagnostic` and the rest), section ids, `normalize_path`, the default ignore list and its rule, `FileAccess` (reads confined to roots) and the test `probe` counters. No internal dependencies.
  - L1 `adoc-parser` (the line-oriented semantic parser), `adoc-dom` (the full AST behind `DocumentParser`), `adoc-antora` (`antora.yml`, the playbook, the catalog and resolver), `adoc-index` (the workspace index), `adoc-lang` (editor-agnostic language features), `adoc-math` (AsciiMath and LaTeX to checked Typst maths), `adoc-spell` (spell checking) and `adoc-graph` (the link graph, its layout and SVG, with no internal dependencies).
  - L2 `adoc-render` (the view tree and HTML backend), `adoc-diagrams` (Mermaid in process, other tools on `PATH`, a cache), `adoc-model` (the visual editor's model), `adoc-git` (Git over `gix`), `adoc-pdf` (themes, the Typst emitter and `World`), `adoc-audit` (the documentation quality audit) and `adoc-impact` (documentation impact analysis).
  - L3 `adoc-engine`: documents, commands, undo, workspace, watcher, settings, preview, visual model and graph. The only entry point front ends use.
  - L4 `adocfoundry` (the GPUI app, the only crate importing a UI framework), `adocfoundry-cli` (headless), `adoc-ls` (the only crate speaking LSP) and `adoc-mcp` (the MCP server, over stdio or Streamable HTTP).
  - Same-layer dependencies allowed (`SAME_LAYER_ALLOWED`): `adoc-index` and `adoc-lang` on `adoc-parser`; `adoc-lang` on `adoc-index` and `adoc-antora`; `adoc-pdf` on `adoc-render`.
  - Front ends reach `adoc-audit` and `adoc-impact` only through `adoc-engine` (`ENGINE_ONLY`), which re-exports them.
- `grammars/tree-sitter-asciidoc` is a git submodule (the `master` branch of `bovinemagnet/tree-sitter-asciidoc`); run `git submodule update --init` after cloning.
- `crates/adocfoundry/languages/` holds the tree-sitter queries. tree-sitter is for highlighting, folding, outline hints and injections only.
- `vendor/` holds `evalexpr` (an Apache-2.0 stand-in for the AGPL crate acdc-parser pulls in) and a patched `comemo`, both applied through `[patch.crates-io]` in `Cargo.toml`; see `vendor/README.md` before touching either.
- `tests/fixtures/` holds small AsciiDoc and Antora workspaces, grouped by scenario.
- `src/docs/` is the user and developer documentation, an Antora site (`./gradlew antora` builds it to `build/site`; Gradle is used for nothing else).
- `docs/design/` is the architecture, `docs/prd/` the phase and feature specifications (the source of truth for scope), `docs/qa/` the manual QA records.
- `packaging/` holds icons, the desktop entry, the macOS bundle script, the Flatpak manifest and the release smoke tests; `crates/adocfoundry/wix/` the Windows installer source; `xtask/` the `layers`, `package`, `run`, `notices` and `bump` tasks.

## Build, Test and Development Commands

```sh
cargo check --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo xtask layers
cargo xtask run -- tests/fixtures/antora-multi-component
cargo run -p adocfoundry-cli -- check tests/fixtures/antora-single-component
```

`default-members` is the app crate, so a bare `cargo test` covers only the app: pass `--workspace` or `-p <crate>`.

`adoc-engine`'s features `typst` (PDF export and typeset STEM) and `git` (`adoc-git`) are on by default for the crate but off in `[workspace.dependencies]`, so each front end names the ones it wants; `adoc-ls` builds neither by default, which `cargo xtask layers` checks.

## Coding Style and Conventions

- Follow `rustfmt` output; British spelling in prose and documentation comments.
- Byte offsets (`SourceRange`) everywhere; line and column conversion happens only at the edges.
- Every user-visible action is an engine command with a stable id; every document mutation goes through a command and undo history. Views render engine state and dispatch commands.
- Parsers, resolvers and handlers must not panic on half-typed input; add a regression test for every panic fixed.
- Diagnostics are conservative: no diagnostic rather than a false positive, warnings rather than errors, nothing for dynamic constructs.
- Confinement, checked by `cargo xtask layers` over the resolved dependency graph:
  - UI framework crates only in `adocfoundry`; `lsp-types` and `lsp-server` only in `adoc-ls` (and `lsp-types` in the app's `editor/providers.rs`);
  - parser crates (`acdc-parser`, `asciidoc-parser`) only in `adoc-dom`; `spellbook` only in `adoc-spell`;
  - no HTTP client reachable from any crate but `adocfoundry`, which runs the system's `curl`; `adoc-mcp` may use `hyper` and `hyper-util` for their server side only.
- Dependencies are pinned exactly (`=1.2.3`) in `[workspace.dependencies]` and must pass `cargo deny check` (permissive licences only; GPL and LGPL are denied). Never depend on Zed's GPL crates (`editor`, `text`, `rope`, `multi_buffer`, `language`, `project`).
- `unsafe_code` is forbidden; `anyhow` only in binaries, typed errors in libraries.
- No Node.js, JVM or Ruby at run time; external tools are optional and discovered.
- Start engine workers with `worker::spawn` in `adoc-engine`, never a bare `thread::spawn`.

## Testing Guidelines

Add tests with every parser, resolver, engine, command or language-feature change, with a fixture where behaviour depends on files; reach fixtures through `CARGO_MANIFEST_DIR`, never an absolute path. Unit tests sit next to the code. Engine behaviour is tested headlessly in `crates/adoc-engine/tests/`; a test that opens a fixture in place turns Git off with `Settings::with_git_opt_in()`. The CLI is tested through its binary in `crates/adocfoundry-cli/tests/cli.rs` with `insta` snapshots: review a changed snapshot rather than overwriting it. The language server and the MCP server are tested through their binaries in `crates/adoc-ls/tests/` and `crates/adoc-mcp/tests/`. App tests run on GPUI's test platform and must not need a display; test behaviour in `adoc-engine` when it can be tested there. Tests that spawn a process take the `spawn_guard()` mutex in `adoc-render`.

## Commit and Pull Request Guidelines

Short imperative commit messages (`Add cross-component resolution to adoc-antora`). A phase work package's pull request is titled `WP <phase>.<n>: <package name>`; any other pull request has a short imperative title and names the issue it fixes. Describe the spec section or issue, the tests added and the commands run, and update `README.md` and `CHANGELOG.md` when visible behaviour changes. When the work shows the spec is wrong, update the spec in the same pull request and say so.
