# Phase 5 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.1.0 |
| Scope | Phase 5 acceptance (spec §11 and §14) that automated tests cannot cover, and the release |

Run each item on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ from the release artefacts before the release is published. Every item of [`phase1.md`](phase1.md) to [`phase4.md`](phase4.md) is run again on all three platforms and recorded in their Results tables; that closes #30. Record the build, the platform and the result in the table at the end.

Automated coverage for reference: STEM conversion and its safety check (`crates/adoc-math/tests/convert.rs`), typeset STEM in the PDF and its alternative text (`crates/adoc-pdf/tests/stem.rs`, `stem_alt_text_is_the_source`), formulas rendered and cached by the engine (`stem_renders_to_svg_and_is_cached`, `unconvertible_stem_is_reported`), the preview's block formula (`preview_draws_block_stem_as_an_image`) and the HTML backend (`html_stem_is_an_image_with_alt_text`); the language server over stdio (`crates/adoc-ls/tests/engine.rs`, `tests/stdio.rs`, `tests/parity.rs`) and its previews in process (`src/protocol.rs`); the MCP server over stdio (`crates/adoc-mcp/tests/stdio.rs`); the lean `adoc-ls` build (`cargo xtask layers`); and the release artefacts on each platform (`packaging/smoke-test.py`, the release workflow's `smoke` job).

## Setup

1. The release artefacts for the platform, from the GitHub release (or the release workflow's run).
2. `git clone --recurse-submodules https://github.com/bovinemagnet/AdocFoundry.git` for the fixtures.
3. A PDF viewer, a screen reader (VoiceOver, NVDA or Orca), and an LSP client (Zed with the zed-asciidoc extension) and an MCP client.

## Packages

- [ ] macOS: the DMG opens, AdocFoundry drags to Applications, and opens after Control-click, Open (signed ad hoc); `AdocFoundry.app/Contents/MacOS` holds `adocfoundry-cli`, `adoc-ls` and `adoc-mcp`, which run from a terminal. Checked locally with a DMG built by `packaging/macos/bundle.sh` from debug binaries (ad-hoc signature, the four binaries, the smoke test passing from the mounted image); the released universal DMG still needs this check.
- [ ] Windows x64 and ARM64: the MSI installs, the Start menu entry opens the app, and with the PATH feature the three tools run from a new terminal; the portable zip runs unpacked. SmartScreen's prompt is expected (unsigned).
- [ ] Linux x86_64 and aarch64: the AppImage runs; the deb and the rpm install `adocfoundry`, `adocfoundry-cli`, `adoc-ls` and `adoc-mcp` in `/usr/bin` with a desktop entry and icon.
- [ ] The Flatpak manifest builds (`flatpak-builder`) and its app runs.
- [x] `packaging/smoke-test.py` on macOS against debug binaries: every `--version`, a PDF export, the language and MCP servers' initialise and shutdown, and the app staying up on a fixture workspace.

## STEM

- [ ] A page with block and inline AsciiMath and LaTeX (`:stem:` and `:stem: latexmath`) in the preview: block formulas typeset on a white card in light and dark themes, inline formulas as source with the typeset formula in a tooltip on hover.
- [ ] The same page exported to PDF: formulas typeset in STIX Two Math, aligned with the text; a screen reader reads each formula's source.
- [ ] A formula that cannot be converted (`\unknowncommand`) shows as its source in the preview and the PDF, and the export's warning names its file and line.

## Language server

- [ ] Zed with the zed-asciidoc extension, unchanged: diagnostics, hover, completion, definition, and the code actions' preview commands work as before, with Asciidoctor on `PATH` and without it (the engine's preview, with diagrams).
- [ ] Find references on a page and on an anchor; rename an anchor; rename a page's file in the client and see its xrefs and nav entries rewritten.
- [ ] `adoc.exportPdf` and `adoc.checkWorkspace` from a client that can run commands, with the full build (`--features typst,git`).

## MCP server

- [ ] An MCP client configured with `adoc-mcp <docs>` lists the nine read-only tools and the pages as resources, and reads, checks, searches and renders pages.
- [ ] With `--allow-write`, an edit through `edit_page` with `save` and a `rename_page` show in Git as the expected diff.

## Fixes from the tracker

- [ ] #13–#19 by hand in the app: the active tab stays visible with the preview docked; word movement and double-click in the visual editor; the status bar follows the visual caret; highlighted code blocks in the visual editor; the palette in source mode; long diagnostics in full on hover; no Indexing… while typing.

## Results

| Date | Build | Platform | Result |
|---|---|---|---|
| 2026-10-24 | `phase-5` branch, debug | macOS 26 (arm64) | `packaging/smoke-test.py target/debug` passes, the app included. A DMG built by `packaging/macos/bundle.sh` from the debug binaries is signed ad hoc, carries the four binaries, and passes the smoke test from the mounted image. Windows, Linux, the released artefacts and the items needing a person are not yet run. |
