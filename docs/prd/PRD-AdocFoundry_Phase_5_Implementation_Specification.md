# PRD: AdocFoundry — Phase 5 Implementation Specification

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Date | 2026-10-24 |
| Status | Draft |
| Scope | Phase 5: release readiness, fixes, typeset STEM, the language server on the engine, and an MCP server |
| Depends on | `docs/design/DESIGN-AdocFoundry_Architecture.md`, Phases 1 to 4 as merged, the open issues on GitHub |

## 1. Document Purpose

Phases 1 to 4 built an editor, a preview, a visual editor and PDF export, but none has shipped. The CI and release workflows are parked in `packaging/ci/`. Windows has never been run, and each phase's manual QA record still has open items. Phase 5 makes AdocFoundry releasable. It also adds three things the design left open: typeset STEM, the language server on the engine, and an MCP server.

Phase 5 must provide:

- **CI that runs:** on Linux, macOS and Windows, with the asciidoctor-pdf comparison and PDF validation. A confinement rule keeps network clients out of everything below the app.
- **Fixes:** the open issues in the tracker, and the fidelity gaps that exporting a real documentation project found.
- **Typeset STEM:** AsciiMath and LaTeX converted to Typst maths, typeset natively in PDFs and as SVG in the preview and the HTML backend.
- **`adoc-ls` on `adoc-engine`:** Zed and other LSP clients get the app's diagnostics, page rename with link updates, references, typeset previews and PDF export.
- **`adoc-mcp`:** a stdio MCP server over the engine, so assistants and tools can read, check, search, render and (when allowed) edit an Antora workspace through the engine's commands.
- **A first release:** packages for the three platforms, and the manual QA of Phases 1 to 5 recorded. Code signing waits for Phase 6.

## 2. Decisions

| Question | Decision | Consequence |
|---|---|---|
| Order | CI first, then fixes, then features, then the release | Every later package is checked on three platforms as it lands; the release is cut from a green tree. |
| STEM notation conversion | a new L1 crate, `adoc-math`, turns AsciiMath and LaTeX into Typst maths markup: `asciimath-parser` 0.5.0 (MIT) for AsciiMath and `mitex` 0.2.4 (Apache-2.0) for LaTeX, both pinned exactly; an in-house AsciiMath converter if `asciimath-parser`'s tree cannot express Asciidoctor's dialect | One conversion serves the PDF, the preview and the HTML backend. Licences checked with `cargo info`; `cargo deny` decides. |
| STEM safety | the converter's output is parsed with `typst-syntax` in maths mode and refused if it holds embedded code (`#`) or calls any function outside an allow-list of Typst's `math` functions | The emitter's rule holds: a document can never inject Typst code. STEM is the one place generated markup reaches `eval`, and only after this check. |
| STEM in PDF | native Typst equations, `math.equation(block:, alt: <source>)`, from `adoc-pdf` | Vector maths that selects and searches as text; the source is the equation's alternative text, so PDF/UA-1 still passes. |
| STEM in the preview and HTML | SVG rendered by `adoc-pdf` through `typst-svg` (already in the tree), cached by checksum in the diagram cache, through an engine-provided `DiagramProvider` | One Typst world, one font setup and one cache for every output. Block STEM is typeset in the preview; see §8 for inline. |
| Maths font | STIX Two Math (OFL-1.1), bundled in `adoc-pdf` | Typst needs an OpenType MATH font. STIX suits the serif default theme. New Computer Modern Math is under the GUST licence, which the policy does not list. |
| The language server | `adoc-ls` depends on `adoc-engine` (L4 on L3) and drops its own `ServerState` and index | One implementation of documents, diagnostics and resolution for the app, the CLI and every LSP client. `adoc-ls` stays the only crate that knows LSP. |
| Compatibility for existing `adoc-ls` users | the binary name, `--stdio` and the three existing command ids stay as they are; the preview keeps rendering through Asciidoctor when it is configured or on `PATH`; `cargo install` builds a lean server by default | The Zed extension `zed-asciidoc` starts `adoc-ls` from `PATH` and relies on exactly these. Anyone who reinstalls keeps a working setup and a similar install. |
| Optional engine features | `adoc-engine` gains the features `typst` (PDF export and typeset STEM, through `adoc-pdf`) and `git` (`adoc-git`). The app and the CLI enable both; `adoc-ls` enables neither by default and offers both as features of its own | A default `adoc-ls` build stays close to today's 100 crates and 4 MB instead of the engine's 482 crates and about 85 MB. The release archives are built with both features. |
| Rename over LSP | `workspace/willRenameFiles`, answered from the engine's pending change (the same one `workspace.apply_preview` applies), as a `WorkspaceEdit` | The client moves the file and applies the edits; the server never writes files behind the client's back. |
| MCP | a new L4 crate and binary, `adoc-mcp`, on `rmcp` 3.7.0 (Apache-2.0) with only its server and stdio transport features, on a current-thread tokio runtime (tokio is already in the tree) | No HTTP transport is compiled in; the network confinement rule checks it. |
| MCP writes | read-only by default; `--allow-write` enables tools that edit, rename or export, each going through engine commands | An assistant cannot change a workspace unless the user started the server to allow it, and every change is an ordinary command. |
| Network | a confinement rule in `cargo xtask layers`: HTTP clients (`reqwest` and its forks, `ureq`, `hyper`, `isahc`, `curl`) may appear only in `adocfoundry`'s tree, where `gpui-kit-assets` brings one | Closes #3: diagrams, STEM and everything below the app render locally. |
| Release signing | none in Phase 5: macOS is ad-hoc signed (as `bundle.sh` does without an identity), Windows is unsigned; the README documents the Gatekeeper and SmartScreen steps. Developer ID signing and notarisation, and a Windows certificate, are Phase 6 | No credentials are needed to release. Signing can be added later without changing the packages' contents. |
| First version | tagged from the workspace version, `0.1.0`, unless decided otherwise before WP 5.8 | `xtask package` and `release.yml` already read the workspace version. |

## 3. Architecture

```
L4  adocfoundry   adocfoundry-cli   adoc-ls (now on adoc-engine)   adoc-mcp (new)
L3  adoc-engine   gains a StemProvider for the diagram cache, and pending changes as data for adoc-ls
L2  adoc-render   adoc-diagrams   adoc-model   adoc-git   adoc-pdf (maths font, equations, render_math_svg)
L1  adoc-parser   adoc-dom   adoc-antora   adoc-index (reverse references)   adoc-lang   adoc-math (new)
L0  adoc-core
```

- `adoc-math` depends on no internal crate. `adoc-pdf` depends on it as a lower layer.
- `adoc-mcp` depends on `adoc-engine` and on nothing else internal except `adoc-core` types.
- `LAYERS` in `xtask/src/layers.rs` gains `adoc-math` at L1 and `adoc-mcp` at L4. `NETWORK_ALLOWED` lists only `adocfoundry`.
- `LSP_ALLOWED` is unchanged: `adoc-mcp` must not import `lsp-types`.
- The ladder in `CLAUDE.md` and the architecture page are updated in the same PR as each new crate.

## 4. CI and release groundwork (WP 5.1)

- **Activate the workflows.** Move `packaging/ci/ci.yml` and `release.yml` into `.github/workflows/`, and update `packaging/ci/README.md` to say they run.
- **`ci.yml` gains:**
  - asciidoctor and asciidoctor-pdf 2.3 on the Ubuntu job, so `fidelity` and `pdf_regression` run instead of skipping. Ruby is a CI dependency only, never a runtime one. This settles the open item in #27.
  - a veraPDF job that exports the reference article and the book fixture with `--standard pdf-a-2a` and `--standard pdf-ua-1` and fails on any failed check.
  - `cargo test -p adocfoundry` on the macOS runner, where GPUI's test platform needs no display.
  - the first Windows run of `cargo test --workspace --exclude adocfoundry`. Whatever fails on Windows is fixed in this package: paths, line endings, process spawning, file watching.
- **Network confinement.** `cargo xtask layers` checks that no crate other than `adocfoundry` reaches an HTTP client, through `cargo metadata`'s resolved graph for normal dependencies. The user docs say that diagrams and STEM render locally, which closes #3.

Acceptance tests: `network_clients_are_confined_to_the_app` (xtask). CI is green on all three platforms, with `pdf_regression` and `fidelity` reported as run, not skipped.

## 5. Fixes from the issue tracker (WP 5.2)

| Issue | Fix | Where | Acceptance test |
|---|---|---|---|
| #19 "Indexing…" on every keystroke | the indexing state covers workspace (re)indexing only, not the reparse after an edit | `adoc-engine` | `typing_does_not_report_indexing` |
| #17 palette lists visual-only commands in Source mode | a command spec declares the modes it applies to; the palette and the key bindings ask the engine | `adoc-engine`, `adocfoundry` | `visual_commands_are_unavailable_in_source_mode` (engine), `palette_hides_visual_commands_in_source_mode` (app) |
| #13 tab strip hides the active tab while the preview is docked | the strip scrolls the active tab into view when its width changes | `adocfoundry` | `active_tab_stays_visible_with_the_preview_docked` |
| #15 Ln/Col does not follow the visual caret | the status bar reads the caret from whichever editor is focused, through `LineIndex` | `adocfoundry` | `status_bar_follows_the_visual_caret` |
| #18 Problems panel clips long messages | messages wrap, and the full text is in a tooltip | `adocfoundry` | `long_diagnostics_wrap` |
| #14 word-wise caret movement and double-click selection | word boundaries (UAX #29) in the model; option/ctrl-arrow and double-click use them | `adoc-model`, `adocfoundry` | `word_boundaries_follow_uax29`, `double_click_selects_a_word` |
| #16 highlighting in editable code blocks | the visual editor's code blocks use the source editor's tree-sitter highlighter | `adocfoundry` | `visual_code_blocks_are_highlighted` |

The existing tests, the round-trip properties and the GPUI-test-platform rule all stay as they are. Behaviour that can be tested in the engine is tested there.

## 6. Fidelity (WP 5.3)

- **#31, description lists tagged as `Div`:** draw description lists with Typst's `terms`, styled so no line moves against asciidoctor-pdf (term on its own line, description indented below, `description_list_term_*` keys honoured). Test: extend `the_tag_tree_reads_in_order` so description lists are `L` with `Lbl` and `LBody`; `pdf_regression` passes unchanged.
- **#32, input acdc rejects:** report both cases upstream. Until releases fix them, `adoc-dom` rewrites the text before acdc reads it, keeping its length as the existing workarounds do:
  - a tab between a section title's `=` and its text becomes a space, outside verbatim blocks;
  - attributes acdc does not know on an include (`subs=`, anything else Asciidoctor ignores) move into the target's placeholder padding.

  Tests: `tabbed_section_titles_parse`, `unknown_include_attributes_are_ignored`.
- **#33, tag directives in tagged includes and callout items:** report the include case upstream. In `adoc-dom`, drop tag directive lines from verbatim content whose lines come from a file included with `tag` or `tags`, and from callout item text. Tests: `tagged_includes_drop_other_tag_directives`, `callout_items_drop_tag_directives`.
- **#35, the footnote 3.6 pt low:** match asciidoctor-pdf's footnote block: its top margin, the separator and `footnotes_item_spacing`. Test: `footnotes_sit_where_asciidoctor_pdf_puts_them` compares the footnote line's y from `typeset_pages` with the y asciidoctor-pdf 2.3.15 gives, recorded in a fixture, to within 1 pt.
- **#34, peak memory while a book is typeset:** measure the template's share first:
  - `keep()`'s `context` and `measure` for every unbreakable block;
  - running content querying the start marker on every page;
  - Typst's layout iterations.

  Reduce what is ours. Target: at least 30% below the 9 MB a PDF page measured on the 1,838-page book. If the target cannot be reached, record why in §12 and document a practical book size in the user docs. Probe: `book_memory_probe` (ignored, like `perf`).

## 7. STEM notation conversion (WP 5.4)

```rust
// adoc-math
pub enum Notation { AsciiMath, Latex }
pub struct TypstMath(String);                    // Typst maths markup that passed the safety check
pub struct MathError { pub message: String, pub offset: Option<usize> }   // byte offset into the source
pub fn to_typst(notation: Notation, source: &str) -> Result<TypstMath, MathError>;
impl TypstMath { pub fn as_str(&self) -> &str; }
```

- **Notation follows Asciidoctor:**
  - `:stem:` with no value, or `asciimath`, means AsciiMath; `latexmath` means LaTeX;
  - `stem:[…]` and `[stem]` blocks use the document's notation;
  - `asciimath:[…]`, `latexmath:[…]`, `[asciimath]` and `[latexmath]` name theirs.

  `adoc-dom` already carries `latex: bool` on `BlockKind::Stem` and `Inline::Stem`. WP 5.4 checks that it follows `:stem:`, and fixes it if not.
- **AsciiMath:** the full symbol tables of Asciidoctor's AsciiMath (fractions, roots, sub- and superscripts, matrices, `text()`, brackets, accents, the font commands). `asciimath-parser` provides the tree, and `adoc-math` maps it to Typst maths.
- **LaTeX:** `mitex`'s conversion, then the same safety check.
- **Safety:** the output is parsed with `typst_syntax` in maths mode. It is refused if any node is code (`#…`), or a call or identifier outside the allow-list of Typst's `math` module functions and symbols. A refused or unconvertible formula is a `MathError`, never a panic, and the output keeps showing its source.

Fixtures: `tests/fixtures/stem/` holds AsciiMath and LaTeX samples, taken from Asciidoctor's documentation and real pages, each with the expected Typst. Half-typed input is included, because incomplete AsciiDoc is normal input.

Acceptance tests: `asciimath_converts_to_typst`, `latex_converts_to_typst`, `stem_notation_follows_the_stem_attribute` (adoc-dom), `converted_maths_holds_no_code`, `half_typed_maths_does_not_panic`.

## 8. Typeset STEM in PDF, preview and HTML (WP 5.5)

- **PDF:**
  - `ViewKind::Stem` and `ViewInline::Stem` are emitted as `#adoc-stem(block: true|false, "<checked Typst maths>", alt: "<source>")`, both string literals. The template typesets them with `math.equation(block:, alt:)` over `eval(…, mode: "math")`.
  - STIX Two Math is registered with the bundled fonts and used for equations.
  - A formula that does not convert is shown as its source, as today, and the export warns with its file and line.
- **Rendering for the preview and HTML:**
  - `adoc-pdf` gains `render_math_svg(&TypstMath, block: bool, size: f64) -> Result<String, PdfError>` through `typst-svg`.
  - The engine registers a `StemProvider` (a `DiagramProvider` for the kinds `stem-asciimath` and `stem-latex`) with its diagram cache, so formulas render on a worker and are cached by checksum like diagrams.
  - The view tree's `Stem` gains a rendered state like `DiagramView`: pending, rendered (an SVG path) or failed.
- **Preview:**
  - Block STEM is drawn as its SVG.
  - Inline STEM stays as styled source in the text flow, with the typeset formula in a tooltip. The preview lays text out as `StyledText` runs, which cannot hold an image. Typesetting inline maths in the text flow needs inline boxes in the preview's layout, recorded as later in §12.
- **HTML backend:** block and inline STEM become `<img>` elements with the source as `alt`.
- **Visual editor:** STEM stays read-only (Phase 3). Its blocks are drawn by the preview's block renderer, so they are typeset too.

Acceptance tests:

- `stem_typesets_in_the_pdf` (adoc-pdf): the text survives, and the PDF/UA-1 export with equations passes Typst's checks.
- `stem_alt_text_is_the_source` (adoc-pdf).
- `unconvertible_stem_is_reported` (engine).
- `stem_renders_to_svg_and_is_cached` (engine).
- `preview_draws_block_stem_as_an_image` (app).
- `html_stem_is_an_image_with_alt_text` (adoc-render).

The reference article gains a STEM section, and `pdf_regression` compares it. asciidoctor-pdf has no native STEM, so that page is compared for layout, not for the formula, and the threshold for that region is recorded in §12.

## 9. The language server on the engine (WP 5.6)

- **State:** `adoc-ls` keeps an `Engine` instead of its `ServerState` and index. `initialize`'s workspace folders become `Engine::open_workspace`. Git stays off unless `[git] enabled` is set, and settings load from the workspace's `.adocfoundry.toml` as in the app.
- **Synchronisation:** incremental (`TextDocumentSyncKind::INCREMENTAL`). Each change maps through `LineIndex` in the negotiated position encoding to `edit.replace_range`. `didClose` closes the engine document; unsaved text is the engine's buffer.
- **Diagnostics:** published from the engine, the same codes and severities as the app's Problems panel, after the engine's settle and for every affected document (a renamed page's referrers included).
- **Hover, definition, completion and symbols:** unchanged behaviour, now called through the engine.
- **New: references:** `textDocument/references` on a page (the pages that link or include it) or an anchor (the references to it). `adoc-index` keeps a reverse map from targets to references, updated on replacement, so no request scans every document. `adoc-lang` gains `references`.
- **New: rename:** `workspace/willRenameFiles` for `.adoc` pages returns the engine's pending change for `page.rename` as a `WorkspaceEdit`. That covers xrefs, nav entries and includes across the workspace; the client performs the move. `textDocument/rename` on an anchor id renames the anchor and its references.
- **Commands** (`workspace/executeCommand`):
  - the existing preview commands now render through the engine's view tree, with diagrams and typeset STEM;
  - `adoc.exportPdf { uri, component?: bool, output, theme?, standards? }`, reported with `window/showMessage`;
  - `adoc.checkWorkspace`.

  Every command a code action carries stays in `executeCommandProvider.commands`.
- **Compatibility:**
  - The binary is still `adoc-ls` and still serves over `--stdio`.
  - The three command ids clients already hold in code actions stay: `adoc.renderPreview`, `adoc.renderLivePreview` and `adoc.stopLivePreview`.
  - The server's capabilities are a superset of today's, and `crates/adoc-ls/tests/stdio.rs` keeps its lifecycle test.
  - The preview renders through Asciidoctor whenever `tools.asciidoctor` is set or `asciidoctor` is on `PATH`, as it does today. Otherwise it renders through the engine's view tree, with diagrams, and with typeset STEM in builds with the `typst` feature. A preview that fails today for want of Ruby now works, and nobody else's changes.
  - Settings come from the same files the app reads: the user settings file and the workspace's `.adocfoundry.toml`. The README's language-server section says so.
- **Features and size:**
  - `adoc-engine`'s `typst` feature gates PDF export and the STEM provider: their commands are not registered without it, and STEM shows as styled source.
  - Its `git` feature gates `adoc-git`: without it the engine behaves as with `[git] enabled = false`.
  - `adoc-ls` forwards both as its own `typst` and `git` features, default off. `adoc.exportPdf` is advertised only when the server is built with `typst`.
  - CI builds `adoc-ls` with no features and with all features, and the default build's size and crate count are recorded in §12.
  - The release archives (WP 5.8) are built with both features.
- **Installing:**
  - The README keeps `cargo install --git https://github.com/bovinemagnet/AdocFoundry adoc-ls` for the lean server, and adds `--features typst,git` and the release archives for the full one.
  - Changing `zed-asciidoc` to download the release archive is a follow-up in that repository (§12), not part of this phase.

Acceptance tests (stdio):

- `command_ids_and_stdio_are_unchanged`: the three existing command ids are advertised, and `--stdio` is accepted.
- `preview_uses_asciidoctor_when_available`, and `preview_falls_back_to_the_engine` when it is not (both take `spawn_guard()`).
- `default_build_has_no_typst_or_git` (xtask, from `cargo metadata`).
- `diagnostics_come_from_the_engine`
- `incremental_changes_apply`
- `references_list_linking_pages`
- `will_rename_files_updates_xrefs_and_nav`
- `anchor_rename_updates_references`
- `export_pdf_command_writes_a_file`
- `preview_renders_typeset_stem`

## 10. The MCP server (WP 5.7)

`adoc-mcp <root>… [--allow-write] [--version]` speaks MCP over stdio through `rmcp`, with only its server and stdio transport features.

Tools, read-only:

| Tool | Arguments | Result |
|---|---|---|
| `list_components` | — | each component version: name, version, title, start page, nav files |
| `get_nav` | `component`, `version?` | the nav trees, with each entry's page path and resource id |
| `read_page` | `path` or `resource_id` (`version@component:module:family$path`), `from?` | the page's text (the open buffer if unsaved), its title, attributes and include list |
| `resolve` | `target`, `from` | what an xref, include or image target resolves to, or why it does not |
| `check` | `scope?`, `fail_on?` | diagnostics, as `adocfoundry-cli check --format json` gives them |
| `symbols` | `query` | workspace symbols |
| `search` | `query`, `regex?`, `case_sensitive?`, `whole_word?`, `scope?` | matches with lines, through `workspace.search` |
| `render_page` | `path`, `format: "text" \| "html"` | the page as plain text from the view tree, or HTML from `to_html` |
| `git_status` | — | per-root status, as `adocfoundry-cli git-status --format json` |

Tools with `--allow-write` (otherwise not listed, and refused if called):

| Tool | Arguments | Result |
|---|---|---|
| `edit_page` | `path`, `edits: [{ old_text, new_text } \| { range, new_text }]`, `save?` | applies each edit through `edit.replace_range`; an `old_text` that is not found exactly once is an error |
| `rename_page` | `path`, `new_name` | the pending change from `page.rename`, applied through `workspace.apply_preview` |
| `export_pdf` | `path` or `component`, `output`, `theme?`, `standards?` | the export summary, through `export_blocking` |

- **Resources:** every page, as a `file://` URI with the `text/asciidoc` media type. A resource template `antora:{resource_id}` resolves Antora ids.
- **Limits:** every path argument resolves inside a root (`normalize_path`, no escape through `..` or symlinks), or the call is refused. There is no network, and project scripts are never executed. Large results are truncated, with the total stated.
- **Errors:** MCP tool errors with the engine's message.

Acceptance tests (`crates/adoc-mcp/tests/stdio.rs`, driving the binary):

- `lists_read_only_tools_by_default`
- `reads_a_page_by_resource_id`
- `check_matches_the_cli`
- `search_finds_matches`
- `render_page_as_text`
- `write_tools_need_allow_write`
- `edit_page_applies_through_commands`
- `rename_page_updates_xrefs`
- `paths_outside_the_roots_are_refused`

Documentation: a user page on running `adoc-mcp` with MCP clients, in the client-neutral `mcpServers` JSON form.

## 11. Release (WP 5.8)

- **Packages from `release.yml` on a `v*` tag:**
  - macOS: a universal DMG, ad-hoc signed (Developer ID signing and notarisation are Phase 6);
  - Windows: an MSI and a zip, x64 and ARM64;
  - Linux: an AppImage, a deb and an rpm, x64 and arm64.

  Each carries the app and the `adocfoundry-cli`, `adoc-ls` and `adoc-mcp` binaries. `adoc-ls` is built with the `typst` and `git` features and is also published as its own archive per platform, as `release.yml` already does. The Flatpak manifest is checked against the release.
- **Smoke test** on each platform from the downloaded artefacts: every binary's `--version`, the app launching on a fixture workspace, a PDF export from the CLI, and `adoc-mcp`'s and `adoc-ls`'s initialise and shutdown.
- **Manual QA:** every item in `docs/qa/phase1.md` to `phase4.md`, and a new `docs/qa/phase5.md`, run on macOS, Windows and Linux and recorded in their Results tables. That includes:
  - IME and frame timing;
  - an hour of visual editing and its Git diff;
  - the app's PDF export dialog and Open;
  - a screen reader over a PDF/UA export.

  This closes #30.
- **Release documents:** `CHANGELOG.md`, the README's install section per platform, release notes on the GitHub release, and the documentation site's version.

Acceptance: a tagged release whose artefacts pass the smoke test on all three platforms, with every QA record complete for them.

## 12. Out of Scope in Phase 5, Open Questions and Implementation Notes

Out of scope:
- Typesetting inline STEM in the preview's text flow, which needs inline boxes in the preview's layout.
- Editing STEM visually.
- An HTTP transport for MCP.
- Code signing: macOS Developer ID signing and notarisation, and a Windows certificate (Phase 6).
- Auto-update.
- Publishing to package managers (Homebrew, winget, Flathub).

Open questions, to settle before the package that needs them:

- [ ] **WP 5.8:** the first version number. `0.1.0` unless decided otherwise.
- [ ] **WP 5.7:** whether `export_pdf` counts as a write, as decided here, or may run read-only when `output` is outside the roots.
- [ ] **WP 5.3:** whether to keep the `adoc-dom` workarounds for #32 and #33 after acdc-parser releases fixes, or drop them on upgrade.
- [ ] **After WP 5.8, in `zed-asciidoc`:** download the `adoc-ls` release archive for the platform (Zed extensions can fetch GitHub release assets), and fall back to `PATH`. That repository's change, not this one's.

Implementation notes, as each package lands:

| Area | Specification | Implementation | Why |
|---|---|---|---|
| Windows in WP 5.1 | the first Windows run, with whatever fails fixed in the package | CI runs Windows; no Windows-specific fix was needed to type-check on macOS, and a local cross-check stopped at a C build script (`stacker`) that needs the Windows SDK | Windows is checked by the first CI run on GitHub; anything it finds is fixed then. |
| Command availability (#17) | a command spec declares the modes it applies to | the existing `CommandContext::Selection`, which only the visual editor's commands use, now means "the active document is shown visually or split"; `Engine::command_available` evaluates every context, and the palette lists only available commands | No new field: the context already named the visual editor's commands. The palette also stops listing document commands with no document open. |
| Problems rows (#18) | messages wrap, with the full text in a tooltip | every sidebar row is one line, cut off with an ellipsis, with the full text in a tooltip; test `long_diagnostics_show_in_full_on_hover` | The sidebar trees are virtual lists with one fixed row height, where a wrapped row would overlap the next. |
| Revealing the active tab (#13) | the strip scrolls the active tab into view | the reveal is requested on two consecutive frames | GPUI places a tracked item from the previous frame's layout, which a change of tabs or widths has just made stale. |
| Word movement (#14) | word boundaries in the model | `adoc-model`'s `next_word_end`, `previous_word_start` and `word_at` over the block's shown text (UAX #29 through `unicode-segmentation` 1.13.3, already in the tree); also `caret_moves_by_word` | A word move at the start of a marked word stops before the mark's delimiter, as character movement does. |
| Description lists (#31) | `terms`, styled to keep asciidoctor-pdf's layout | Typst's `terms` with a hanging indent of `description_list_description_indent`, the term spacing above each description and `strong` neutralised in terms; the emitter no longer ends a term with a paragraph break | A break inside a term split its tag, which PDF/UA rejects. No line of the reference article moved. Horizontal description lists stay a grid, tagged `Div`. |
| Tag directives (#33) | drop them from tagged includes and callout items | callout items drop their comment lines as paragraphs do; tagged includes needed nothing: acdc already drops every directive of a tag-filtered region | The corpus's remaining directive lines are in literal paragraphs of pages rendered whole, where Asciidoctor shows them too. Tests: `tagged_includes_drop_other_tag_directives`, `callout_items_drop_tag_directives`. |
| Footnotes (#35) | match asciidoctor-pdf's footnote block | the last footnote keeps `footnotes_item_spacing` below it, the margin that meets the page's bottom margin | As asciidoctor-pdf's `ink_footnotes`; the reference article's footnote is now 0.56 pt from asciidoctor-pdf's, from 3.6 pt. |
| Book memory (#34) | measure the template's share, then reduce ours | the emitter marks blocks its text shows fit on a page (`fits: true`, at most 20 estimated lines) so `keep()` measures only the rest; running titles look up one memoised list of headings with a binary search instead of filtering every heading on every page | The 1,833-page corpus book: peak 17.1 GB to 9.3 GB, 55 s to 12.6 s; the 617-page one: 8.5 GB to 5.9 GB. `book_memory_probe` exports a synthetic 300-page component. No line or page changed. |
| LaTeX helpers (WP 5.4) | `mitex`'s conversion, then the safety check | `mitex` writes calls to its own Typst package's helpers (`pmatrix`, `mitexmathbf`, …) and `\\text` as `#textmath[…]`; `adoc-math` turns `\\text`, `\\sqrt[n]` and `\\operatorname` into plain maths and lists the helpers it allows (`HELPERS`), which the evaluator defines; a LaTeX command outside them fails the check and shows as source | No Typst package is reachable, and the helpers are the part of mitex's package documentation commonly uses. |
| mitex's advisory (WP 5.4) | `cargo deny` decides | RUSTSEC-2026-0235 (`rkyv` 0.7 through `mitex-spec`) ignored with its reason | rkyv reads only the specification compiled into mitex, never document data. |
| STEM notation (WP 5.4) | check that `latex` follows `:stem:` | acdc gave `[stem]` blocks LaTeX whatever `:stem:` said, and read `[latexmath]` passthrough blocks as passthrough; `adoc-dom` now sets the notation from the style and `:stem:` | As Asciidoctor. |
| Code highlighting (#16) | the source editor's highlighter | the preview's tree-sitter highlighter, laid over the editable block's runs; the visual editor keeps its cache between frames, emptied past 256 entries | The cache was rebuilt every frame, so read-only blocks were highlighted again on every render too. |
| The emitted call (WP 5.5) | `#adoc-stem(block: …, "<maths>", alt: "<source>")` | `#adoc-stem("<maths>", "<source>", block: …)`, the source a positional argument | Both are string literals either way; `math.typ` holds the function and the helper scope, ahead of the template. |
| STEM fonts and sizes (WP 5.5) | STIX Two Math for equations | STIX Two Math 2.13 (OFL-1.1) bundled and set for equations only, the text around them unchanged; the preview's formulas render at 15 pt (block) and 13 pt (inline) | The size of the preview's body text, so a formula reads at the size of the text beside it. |
| Formulas in the preview (WP 5.5) | block STEM drawn as its SVG; inline in a tooltip | the SVG is drawn on a white card, black on white in either theme; an inline formula's source is styled as code, with the typeset formula in a tooltip | Typst draws maths in black, which a dark theme would hide. |
| Formulas in export (WP 5.5) | — | the export's view tree asks for no formula images, since the PDF typesets them itself | Rendering each formula to SVG for a PDF that never uses it doubled the work. |
| STEM in `pdf_regression` (WP 5.5) | the formulas compared for layout, with a threshold for that region | the reference article ends with a Formulas section; its page is held to the same mean difference (0.02) and edge limits, with the right edge of the ink left out | asciidoctor-pdf shows the formula's source in a full-width code box where ours is typeset, so only the right edge differs; it was within 0.0113 mean and 0 pt on the other edges in every theme. |
| Reverse map (WP 5.6) | `adoc-index` keeps a reverse map from targets to references | the index maps the file name an xref or include names (`setup.adoc` for `admin:setup.adoc`) and each fragment to the references, and `adoc-lang` resolves those candidates, the target page's `page-aliases` included | Resolving a target needs the Antora catalog, which sits beside `adoc-index` on L1, not below it. A request visits only the references that name the file. |
| Renames (WP 5.6) | `page.rename`'s edits cover xrefs, nav entries and includes | `rename_preview` now also rewrites includes: an Antora include keeps its family and coordinates (`partial$notes/token.adoc`), a relative one stays relative, and one written through an attribute is left as written; `textDocument/rename` on anything but an anchor is refused | The app's page rename gains the include edits too. A client renames a file through `willRenameFiles`. |
| Git in `adoc-ls` (WP 5.6) | Git stays off unless `[git] enabled` is set | `Settings::with_git_opt_in`, kept across reloads | `open_workspace` reloads settings, which would otherwise turn Git back on. |
| Diagnostics timing (WP 5.6) | published after the engine's settle | each change is an `edit.replace_range`, which indexes at once, so a notification's diagnostics are published as it is handled; between messages the server pumps the engine every 50 ms and publishes what workers and the watcher changed; `initialize` is answered once the workspace is indexed (at most 120 s) | The first requests already see every page, as the old server's synchronous indexing did. |
| Features (WP 5.6) | `adoc.exportPdf` advertised only in builds with `typst` | the server advertises it when the engine registered its export commands, and `Engine::export_blocking` exists without Typst and fails | A workspace build unifies the engine's features, so the engine, not `adoc-ls`'s own flag, says what is there. `adoc-ls`'s features only forward. |
| Default build (WP 5.6) | `default_build_has_no_typst_or_git` from `cargo metadata` | from `cargo tree -p adoc-ls`, also run by `cargo xtask layers` | `cargo metadata` resolves features for the whole workspace at once. |
| Size (WP 5.6) | record the default build's size and crate count | release, macOS arm64: 32.4 MB and 237 crates by default; 88.0 MB and 535 crates with `typst,git` | The lean build still carries acdc, merman and the diagram renderers the engine's preview uses. |
| Preview tests (WP 5.6) | stdio tests taking `spawn_guard()` | the preview tests drive the protocol in process with a sink that never opens a browser, under a guard of their own (`adoc-render`'s is crate-private); the others drive the binary (`tests/engine.rs`) | Over stdio the preview opens the system browser. |
| Shared reports (WP 5.7) | `check` and `git_status` as the CLI's JSON | the CLI's diagnostics and Git status reports moved into the engine (`report.rs`: `check_report`, `git_status_report`), and the CLI prints them; `check` answers `{ diagnostics, failed }`, `diagnostics` being the CLI's array | One implementation, so the two cannot drift; the CLI's snapshots did not change. |
| Engine queries (WP 5.7) | — | `inspect.rs`: `resolve_target` (a macro as written or a bare xref target, read with the page's attributes), `resource_id` and `resolve_resource_id`; `adoc-render` gains `to_text` for `render_page` | `adoc-mcp` may depend only on the engine and `adoc-core`, so what it needs from the catalog comes through the engine. |
| Write tools (WP 5.7) | not listed, and refused if called | removed from the tool router without `--allow-write`, so a call is the protocol's unknown-tool error; `edit_page` is all or nothing (edits already made in a failing call are undone); `rename_page` saves every file it changed | `workspace.apply_preview` leaves its edits in buffers, which a server with no editor would never save. |
| Limits (WP 5.7) | large results truncated, with the total stated | lists at 500 items and texts at 200,000 bytes, with `truncated: { shown, total }`; paths are absolute or relative to the first root, and links are followed only through the part of a path that exists | A path to a file not yet written, such as a rename's target or an export's output, is still checked. |
| Runtime (WP 5.7) | a current-thread tokio runtime | tokio with `rt`, `macros`, `io-std` and `time`; each tool call locks the engine for its duration | rmcp's service needs timers. One call at a time matches the engine's single thread. |

## 13. Implementation Sequence

| # | Package | Depends on |
|---|---|---|
| 5.1 | CI on three platforms, asciidoctor-pdf and veraPDF in CI, app tests on macOS, network confinement (#3, #27) | Phase 4 |
| 5.2 | Fixes from the tracker (#13–#19) | 5.1 |
| 5.3 | Fidelity: description-list tags, acdc gaps, footnote position, book memory (#31–#35) | 5.1 |
| 5.4 | `adoc-math`: AsciiMath and LaTeX to checked Typst maths | 5.1 |
| 5.5 | Typeset STEM in PDF, preview and HTML | 5.4 |
| 5.6 | `adoc-ls` on the engine: diagnostics, references, rename, commands, with today's command ids, Asciidoctor preview and a lean default build kept | 5.5 |
| 5.7 | `adoc-mcp` | 5.5 |
| 5.8 | Release: packages (unsigned; signing is Phase 6), smoke tests, the QA backlog (#30), changelog | 5.2, 5.3, 5.6, 5.7 |

## 14. Definition of Done for Phase 5

- CI runs on Linux, macOS and Windows on every push, with the asciidoctor-pdf comparison and veraPDF validation, and is green.
- The issues listed in §5 and §6 are closed, each with its acceptance test.
- A STEM formula in AsciiMath or LaTeX typesets in the PDF, in the preview (blocks) and in the HTML backend, and a formula that cannot be converted is shown as its source with a warning naming its file and line.
- `adoc-ls` serves the engine's diagnostics, references, rename and PDF export to an LSP client, existing clients (the Zed extension among them) keep working unchanged, the default build carries neither Typst nor Git, and its stdio tests pass.
- `adoc-mcp` serves the read-only tools by default and the write tools only with `--allow-write`, and its stdio tests pass.
- A release is tagged, its packages pass the smoke test on all three platforms, and every manual QA record is complete for them.
- README, the documentation site, `CHANGELOG.md` and `docs/qa/phase5.md` describe what shipped.
