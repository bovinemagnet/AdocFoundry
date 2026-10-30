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

- [ ] macOS: the DMG opens, AdocFoundry drags to Applications, and opens after Control-click, Open (signed ad hoc); `AdocFoundry.app/Contents/MacOS` holds `adocfoundry-cli`, `adoc-ls` and `adoc-mcp`, which run from a terminal. Checked locally with a DMG built by `packaging/macos/bundle.sh` from debug binaries (ad-hoc signature, the four binaries, the smoke test passing from the mounted image); the released universal DMG still needs this check. The released DMG (2026-10-24): universal (x86_64 and arm64), signed ad hoc, rejected by Gatekeeper as expected without a Developer ID, the four binaries in `Contents/MacOS`, and `packaging/smoke-test.py` passes from the mounted image, the app's launch included. Dragging to Applications and Control-click, Open still need a person.
- [ ] Windows x64 and ARM64: the MSI installs, the Start menu entry opens the app, and with the PATH feature the three tools run from a new terminal; the portable zip runs unpacked. SmartScreen's prompt is expected (unsigned).
- [ ] Linux x86_64 and aarch64: the AppImage runs; the deb and the rpm install `adocfoundry`, `adocfoundry-cli`, `adoc-ls` and `adoc-mcp` in `/usr/bin` with a desktop entry and icon. The released `adocfoundry_0.1.0-1_arm64.deb` installs with `apt` in a Debian 12 container, which resolves its dependencies, puts the four binaries in `/usr/bin` and the desktop entry in `/usr/share/applications`, and passes `packaging/smoke-test.py --no-app` (2026-10-24). The released `adocfoundry-0.1.0-1.aarch64.rpm` installs with `dnf` in Fedora 41 (unsigned, so without its OpenPGP check) with the four binaries, the desktop entry and the icon, and passes the smoke test too. The released aarch64 AppImage unpacks and its app reports 0.1.0, but it holds only `adocfoundry`: the tools were not passed to `linuxdeploy` (see below). On x86_64 (Debian 12 and Fedora 41 containers under emulation, 2026-10-25) the released `adocfoundry_0.1.0-1_amd64.deb` and `adocfoundry-0.1.0-1.x86_64.rpm` install the same files and pass `packaging/smoke-test.py --no-app`; the released x86_64 AppImage's app reports 0.1.0 and, like the aarch64 one, holds only `adocfoundry`. The app on a desktop is not yet run.
- [ ] The Flatpak manifest builds (`flatpak-builder`) and its app runs.
- [ ] Linux, from the deb or rpm, the Flatpak and the AppImage, on GNOME and KDE Plasma, each on Wayland and X11: the task bar, the window switcher (Alt+Tab) and the window menu show the AdocFoundry icon, not the desktop's placeholder, and the main window and a detached preview are grouped as one application (#153).
- [x] `packaging/smoke-test.py` on macOS against debug binaries: every `--version`, a PDF export, the language and MCP servers' initialise and shutdown, and the app staying up on a fixture workspace.

## STEM

- [ ] A page with block and inline AsciiMath and LaTeX (`:stem:` and `:stem: latexmath`) in the preview: block formulas typeset on a white card in light and dark themes, inline formulas as source with the typeset formula in a tooltip on hover.
- [ ] The same page exported to PDF: formulas typeset in STIX Two Math, aligned with the text; a screen reader reads each formula's source. With the released CLI: inline and display formulas in AsciiMath and LaTeX typeset in STIX Two Math (embedded), on the text's baseline and centred, in a PDF/UA-1 export (2026-10-24). The screen reader still needs a person.
- [ ] A formula that cannot be converted (`\unknowncommand`) shows as its source in the preview and the PDF, and the export's warning names its file and line. The PDF part, with the released CLI: `\unknowncommand{x}` is shown as its source in a code box and the export warns `maths.adoc:11: warning: STEM not typeset: error: unknown command: \unknowncommand` (2026-10-24). The preview part still needs the app.

## Language server

- [ ] Zed with the zed-asciidoc extension, unchanged: diagnostics, hover, completion, definition, and the code actions' preview commands work as before, with Asciidoctor on `PATH` and without it (the engine's preview, with diagrams).
- [ ] Find references on a page and on an anchor; rename an anchor; rename a page's file in the client and see its xrefs and nav entries rewritten.
- [ ] `adoc.exportPdf` and `adoc.checkWorkspace` from a client that can run commands, with the full build (`--features typst,git`).

## MCP server

- [x] An MCP client configured with `adoc-mcp <docs>` lists the nine read-only tools and the pages as resources, and reads, checks, searches and renders pages. Checked with the released macOS `adoc-mcp` driven by a JSON-RPC client over stdio, on a Git repository of `tests/fixtures/references` (2026-10-24).
- [x] With `--allow-write`, an edit through `edit_page` with `save` and a `rename_page` show in Git as the expected diff. After both, `git status` shows `R other.adoc -> guides/other.adoc` and `M index.adoc`, the diff being the title and the xref rewritten to `guides/other.adoc` (released macOS binary, 2026-10-24).

## Fixes from the tracker

- [ ] #13–#19 by hand in the app: the active tab stays visible with the preview docked; word movement and double-click in the visual editor; the status bar follows the visual caret; highlighted code blocks in the visual editor; the palette in source mode; long diagnostics in full on hover; no Indexing… while typing.

## Results

| Date | Build | Platform | Result |
|---|---|---|---|
| 2026-10-24 | `phase-5` branch, debug | macOS 26 (arm64) | `packaging/smoke-test.py target/debug` passes, the app included. A DMG built by `packaging/macos/bundle.sh` from the debug binaries is signed ad hoc, carries the four binaries, and passes the smoke test from the mounted image. Windows, Linux, the released artefacts and the items needing a person are not yet run. |
| 2026-10-24 | `phase-5` branch, debug and release | Linux arm64 (Debian 12 container, Rust 1.99.1) | `cargo test --workspace --exclude adocfoundry --no-fail-fast`: 580 passed, none failed (tests that need asciidoctor or asciidoctor-pdf skip themselves). The run found `stem_renders_to_svg_and_is_cached` assuming a page's two formulas finish rendering together; on Linux they finish apart, adding a preview update, so the test now accepts either. The release build of the four binaries (with `adoc-ls/typst,adoc-ls/git`) passes `packaging/smoke-test.py --no-app`. The app and the packages need a display and were not run. |
| 2026-10-24 | the v0.1.0 release | GitHub Actions: macOS 14, Windows Server (x64), Ubuntu 22.04 (x86_64) | The release workflow's `smoke` job on each platform's downloaded artefacts (the mounted DMG, the unpacked zip, the unpacked deb): every binary's `--version`, a PDF export, `adoc-ls`'s and `adoc-mcp`'s initialise and shutdown, and the app staying up on macOS and Windows; all pass. |
| 2026-10-24 | the v0.1.0 release | macOS 26 (arm64); Debian 12 container (arm64) | The released DMG and arm64 deb, as recorded against the items above: the smoke test, the MCP client checks with a Git diff, and STEM in a PDF/UA export with the released CLI. |
| 2026-10-24 | a source checkout of `main` | Windows (by hand) | `cargo test --workspace --exclude adocfoundry`: every test passes, `spans_are_the_same_with_crlf_line_endings`, `page_labels_match_the_printed_numbers` and the Windows path fixes among them; `fidelity` and `pdf_regression` skip themselves without Asciidoctor. The only warnings are `adoc-diagrams`' unused spawn guard, used by its Unix tests alone, now Unix-only. The release's MSI, zip and app are still to install and run. |

## Known limitations found

- The 0.1.0 AppImage holds only the app, not `adocfoundry-cli`, `adoc-ls` or `adoc-mcp`, which the other packages carry. The release workflow now passes them to `linuxdeploy` and smoke-tests the AppImage's tools, from the next release.
