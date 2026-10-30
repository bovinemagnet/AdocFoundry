# AdocFoundry — Architecture & Technology Design

Oct 23, 2026 · @Paul Snow

## Summary

AdocFoundry ([bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry)) is a native Rust desktop editor for Antora-structured AsciiDoc on macOS, Linux and Windows, built as a GPUI shell over a headless engine, with the five `adoc-*` crates from zed-asciidoc as its language layer. Four phases each ship a usable tool: editor with link following, native preview with a detachable follow-along window, WYSIWYG editing that round-trips cleanly, and themed PDF export.

**Headline decisions**

| Layer | Decision | Fallback |
| --- | --- | --- |
| GUI | GPUI via gpui-kit / gpui-component 0.8 (Apache-2.0), gated by a two-week spike | egui 0.37, the stack WordCraft, PdfCraft and Lockbook ship on |
| Language layer | Reuse `adoc-core`, `adoc-parser`, `adoc-antora`, `adoc-index`, `adoc-render` and the tree-sitter grammar fork; move them into the AdocFoundry workspace | — |
| Full AST | acdc-parser 0.11 behind an `adoc-dom` trait (owned tree, positions mapped through includes, book doctype) | asciidoc-parser 0.31 as second implementation, same fixture corpus |
| Preview | Render the AST to native elements; HTML kept for export and fidelity diffs | wry webview behind a feature flag |
| Diagrams | merman 0.9 (the Rust Mermaid port Zed adopted) plus a provider chain for dot/plantuml/Kroki, Asciidoctor Diagram semantics and cache | — |
| WYSIWYG model | ProseMirror-style steps on a typed tree with verbatim nodes; source stays the truth | prosemirror-rs if its schema fits |
| PDF | Generate Typst from the AST, compile in process with `typst` 0.15; asciidoctor-pdf YAML themes mapped onto a Typst template | krilla direct with an in-house layout engine |
| Architecture | Layered crate ladder with one engine and three front ends (app, CLI, LSP), the ArtCraft pattern; only the app crate imports the UI framework | — |

The two things that do not exist anywhere yet are the GPUI shell and `adoc-dom`; everything else has a crate to start from. The largest risks are single-maintainer dependencies (every Rust AsciiDoc parser, the GPUI snapshot train) and WYSIWYG round-trip safety, both addressed by traits, a fidelity corpus and verbatim-by-default serialisation.

## Goals and scope

AdocFoundry is a native desktop editor for AsciiDoc documentation sites laid out the Antora way, delivered in phases that each ship a usable tool on macOS, Linux and Windows. Phases 1 to 4 build the editor; Phase 5 makes it releasable and opens it to other tools; Phase 6 finishes what Phase 5 left for later.

| Phase | Deliverable | Must have | Explicitly later |
| --- | --- | --- | --- |
| 1 | AsciiDoc source editor with Antora awareness | Open a repo or component folder; syntax highlighting; outline; resolve `xref:`, `include::` and `image::` to files; click-to-follow links and includes; diagnostics for broken links; find/replace across the component | Preview, WYSIWYG, PDF |
| 2 | Rendered preview | Side-by-side pane; detachable preview window; follow-active-page mode; Mermaid and other diagrams rendered; scroll sync | Editing in the preview |
| 3 | WYSIWYG editing | Edit the rendered view and write clean AsciiDoc back; source and WYSIWYG modes on the same document; lossless round-trip of what the editor does not understand | Full fidelity for every AsciiDoc construct |
| 4 | PDF export | Paginated PDF from one page or a whole component/book; YAML themes in the spirit of asciidoctor-pdf; TOC, running headers and footers | Index generation, PDF/A |
| 5 | Release, STEM and integrations | CI on three platforms; packages; fixes from the tracker; typeset STEM in PDF, preview and HTML; `adoc-ls` on the engine; an MCP server | Inline STEM in the preview's text flow, an HTTP transport for MCP, code signing, auto-update |
| 6 | Signed releases and updates | Inline STEM typeset in the text flow; MCP over Streamable HTTP on loopback with a token; Developer ID signing and notarisation, Authenticode through Azure Trusted Signing; an update check, and updating in place on macOS and Windows | Delta updates, update channels, package-manager publishing |

Non-goals for now: a web or mobile build, collaborative editing, hosting or publishing the site (Antora itself does that), and bit-for-bit output parity with Asciidoctor. The editor targets the Asciidoctor dialect of AsciiDoc that Antora consumes, so Antora-built sites and AdocFoundry agree on what a document means.

Cross-cutting requirements: one binary per platform with no runtime dependency on Ruby, Node or Java; sub-16 ms keystroke-to-paint in the editor; parse and re-render a page under 50 ms on save for the preview; no telemetry.

## Reference projects

The two ArtCraft apps are the closer model for AdocFoundry's shape; Zed/GPUI is the model for the editor surface itself.

| Project | What it is | Stack | What to borrow | What not to copy |
| --- | --- | --- | --- | --- |
| [GPUI / Zed](https://gpui.rs/) | Zed's own UI framework: hybrid immediate/retained, GPU-accelerated, Rust | `gpui` crate in the Zed repo (`crates/gpui`); the site says it stays tied to Zed for the near future | Entity/context model, the element builder API, `cx.open_window` for a detached preview, Zed's rope and multi-buffer editor design | Depending on Zed internals (`editor`, `project` crates) that churn with each Zed release |
| [PdfCraft](https://github.com/storytold/pdfcraft/) | Open-source PDF reader/editor, pure Rust, alpha (self-assessed \~57% of tracked features, Oct 2026) | Cargo workspace; egui UI (`pdfcraft-ui-egui`) on desktop and WASM; pages drawn by `hayro`; dual MIT/Apache-2.0; Windows x64/x86/ARM64, macOS universal, Linux (AppImage, Flatpak, deb, rpm), FreeBSD | Strict layering with the core never depending on the UI; a `-engine` facade owning sessions, undo and a tool catalogue; CLI and MCP server sharing the engine; output verified against reference tools in CI | Its own PDF object layer is far more than a document editor needs |
| [WordCraft](https://github.com/storytold/wordcraft) | Clean-room Word reimplementation, pure Rust, alpha (\~60% readiness by its own estimate) | egui on GPU; `wordcraft-layout` does line breaking, pagination, tables and hit testing; `wordcraft-render` on `vello_cpu`; `wordcraft-pdf` on craft-pdf and Krilla; `wordcraft-fonts` for shaping; enforced layering checks | The L0 to L6 crate ladder (geom, doc model, fonts, layout, render, pdf, engine, mcp, ui); every action as an ID'd command shared by ribbon, shortcuts, CLI and MCP; cached paragraph layout (188-page relayout \~1.4 ms after a keystroke); Krilla for PDF | The ribbon UI, OOXML, and a bespoke page-layout engine before Phase 3 needs one |

The common thread in both ArtCraft apps is a headless engine with a swappable front end. AdocFoundry adopts the same split: `adoc-*` library crates that never import a UI crate, an `engine` facade that owns documents, undo and commands, and a thin GPUI shell on top. That split is what makes Phase 2's detachable preview window and a later CLI (`adocfoundry check`, `adocfoundry pdf`) cheap.

### Starting point: zed-asciidoc

[bovinemagnet/zed-asciidoc](https://github.com/bovinemagnet/zed-asciidoc) (Apache-2.0, v0.1.0, last push 1 Oct 2026, \~10.6k lines of Rust) already contains most of what Phase 1 needs below the UI, and AdocFoundry should consume those crates rather than start again.

| Crate | What it already does | Role in AdocFoundry |
| --- | --- | --- |
| `adoc-core` | Editor-independent types: `Document`, `LineIndex`, `SourceRange` (byte offsets), `Section`, `Anchor`, `Reference`, `IncludeDirective`, `Diagnostic` with stable codes | Unchanged; the shared vocabulary of every layer |
| `adoc-parser` | Line-oriented semantic parser (titles, sections, attributes, anchors, xrefs, includes, images) that tolerates half-typed input; `completion_context` | Phase 1 outline, link following and completion. Replaced for rendering by a full AST parser in Phase 2 but kept for its speed and tolerance |
| `adoc-antora` | `antora.yml` descriptor parsing, `AntoraCatalog`, `parse_resource_id`, `AntoraResolver` with typed errors; model already carries component and version | The Antora catalog described above. Gaps to close: cross-component and cross-version resolution, descriptor re-read on change, `page-aliases` |
| `adoc-index` | `WorkspaceIndex` with replacement updates and direct lookups (never full scans) | The workspace index behind the sidebar, diagnostics and go-to-definition |
| `adoc-render` | `Renderer` trait; `SystemAsciidoctor` spawning the Ruby CLI; `MockRenderer` | Phase 2 adds a `NativeRenderer` behind the same trait, exactly as the repo's preview design note (DESIGN-Preview\_Pipeline\_and\_Native\_Rendering, Aug 2026) already proposes |
| `adoc-ls` | LSP server over stdio: symbols, diagnostics, definition, completion, hover, workspace symbols, preview code actions | Stays as a sibling product for Zed, VS Code and others. The desktop app calls the handler layer in-process, not over LSP |
| tree-sitter grammars (`bovinemagnet/tree-sitter-asciidoc`, block + inline, pinned rev `5098fa6`, release v0.11.0) | Highlighting, outline, folding, source-block injections; two fork-only changes carried on the fork's `master` branch | Phase 1 syntax highlighting and folding in the GPUI editor |

The repository's own conventions carry over: no LSP types below `adoc-ls`, byte offsets everywhere, conservative diagnostics, pinned dependencies, `unsafe_code = "forbid"`. One convention has to be relaxed: "no custom HTML rendering engine" was right for a language server and is exactly what Phases 2 and 3 add, in a separate crate ladder so the language server never depends on it.

**The grammar fork.** [bovinemagnet/tree-sitter-asciidoc](https://github.com/bovinemagnet/tree-sitter-asciidoc) (fork of cathaysia's, which was archived in October 2026; Apache-2.0, published to crates.io as `tree-sitter-asciidoc` 0.9.0 and `tree-sitter-asciidoc-inline`) is a two-grammar design: a block grammar whose external C scanner keeps a serialised stack of open delimited blocks, and an inline grammar injected into paragraph lines, table cells and macro targets. The fork's `master` branch adds `table_header_row` and allows block content in table cells, which upstream never took. It also has the fixes that went upstream before the archive: comments no longer close a section, attribute values accept globs and empty values, and constrained spans keep word-internal delimiters. Its own LIMITATIONS.md is candid about what it is not: lists do not nest, paragraphs parse line by line, sections only nest at document level, `ifdef::`/`endif::` are not scoped, and attribute-gated macros are not gated. Those are fine for highlighting and folding, which is what tree-sitter is for in this design; they rule it out as the document model for preview, WYSIWYG or PDF. That job goes to a full AST parser (next section), and the two trees stay independent: tree-sitter for the source pane, the AST for everything rendered.

## Antora content model

Antora support means the editor resolves every reference the way Antora 3.2 does, from a component version descriptor rather than from the file system alone. The editor should ship a `antora-model` crate that builds the same in-memory catalog Antora builds, minus Git.

**Directory layout the editor must recognise**

```
docs/                          <- content source root (any folder holding antora.yml)
  antora.yml                   <- component version descriptor: name, version, title, nav, start_page, asciidoc.attributes
  modules/
    ROOT/                      <- default module; pages publish at the component root
      nav.adoc                 <- navigation list; pages referenced by xref
      pages/                   <- family: page$   (.adoc, publishable)
      partials/                <- family: partial$ (included, never published)
      examples/                <- family: example$ (included as source, often extensionless)
      images/                  <- family: image$
      attachments/             <- family: attachment$
    <module>/                  <- same five families
antora-playbook.yml            <- site-level (may live elsewhere): content sources, UI bundle, global attributes
```

Version comes from `version:` in antora.yml (or `~` for versionless). A single repo may hold several components, and one component's versions normally live on different Git branches, so the editor treats each `antora.yml` it finds as one component version and lets the user open several roots.

**Resource IDs** take the form `version@component:module:family$file`. Rules the resolver implements:

- Coordinates omitted are inferred from the current page: same component version, same module. A component given without a version resolves to the latest version the editor has loaded.
- `file` is relative to the family directory, never to the current page, and keeps its extension (except extensionless examples and partials). `./` abbreviates a path within the current page's subdirectory.
- Family defaults depend on the macro: `xref:` defaults to `page$`; `include::` of a page needs no family but a partial or example needs `partial$`/`example$`; `image::`/`image:` imply `image$`; linking to an image or attachment via `xref:` requires the family explicitly.
- `xref:page.adoc#fragment[]` targets an anchor inside the page; the editor validates the fragment against the parsed target.
- `page-aliases` (a page attribute listing old resource IDs) makes a page reachable under its former IDs; the resolver treats aliases as secondary keys.
- Attribute references in targets (`xref:{page-component-name}:...`) are resolved after attribute substitution using antora.yml, playbook and page attributes, plus Antora's built-in `page-*` attributes.

**Navigation.** `nav:` in antora.yml lists one or more `nav.adoc` files per module. Each is a nested unordered list of `xref`s; the editor renders that list as the page tree in the sidebar and uses it to order pages for Phase 4's book export.

**What follows from this for the editor**

1. Link following is a resolver over the catalog, not a file-path join. Phase 1 builds the catalog on open and keeps it current with a file watcher.
2. Includes are resolved through the same catalog, so the preview in Phase 2 shows partials and examples inline, with tag/line filtering (`tag=`, `tags=`, `lines=`, `leveloffset=`, `indent=`).
3. Antora's `page-*`, `site-*` and `playbook` attributes must be available during parsing so conditionals (`ifdef::`) and attribute-driven xrefs behave as they will on the published site.
4. Round-tripping in Phase 3 must preserve resource IDs byte-for-byte; the WYSIWYG model keeps the original target string on every link and image node.

Source: [Antora 3.2 resource ID coordinates](https://docs.antora.org/antora/3.2/page/resource-id-coordinates/).

## Technology survey

Every layer has at least one credible pure-Rust option in late 2026, but the AsciiDoc parsers are all single-maintainer and pre-1.0, so the design isolates each choice behind a trait the way `adoc-render` already isolates `Renderer`.

### AsciiDoc parsing (the document model)

Three pure-Rust parsers are live and releasing monthly. All three expose source positions, which is the non-negotiable requirement for an editor that must map a click in the preview back to a byte in the source.

|  | [asciidoc-parser](https://github.com/asciidoc-rs/asciidoc-parser) (asciidoc-rs, Eric Scouten) | [acdc-parser](https://github.com/nlopes/acdc) (Norberto Lopes) | [asciidork](https://github.com/jaredh159/asciidork) (Jared Henderson) |
| --- | --- | --- | --- |
| Latest release | 0.31.2, 13 Sep 2026 (75 releases) | 0.11.0, 10 Oct 2026 (15 releases) | parser 0.38.2 Jul 2026; CLI/backend 0.39.0, 8 Oct 2026 |
| Licence | MIT OR Apache-2.0 | MIT OR Apache-2.0 | MIT OR Apache-2.0 |
| Technique | hand-written, tests cite asciidoc-lang docs and the Asciidoctor Ruby suite | `rust-peg`, two-pass inline, follows spec SDR-005 substitution order | hand-written, arena-allocated (`bumpalo`) |
| Source positions | `Span` with line, col, byte offset on every node; borrows from source (`'src`) | `Location` byte range + `Position` line/col on every node; maps back through nested includes to file + line + col | byte offsets + include depth only; compute line/col from a line index |
| Owned, long-lived tree | No | Yes (owns source) | No (arena lifetime) |
| Includes and conditionals | `IncludeFileHandler` trait; no URL includes | Preprocessor with `ifdef`/`ifndef`/`ifeval`, tags, lines, safe modes, custom `IncludeSourceProvider` | Yes |
| Book doctype | **No** (article only) | Yes | Yes, incl. multi-part books |
| Backends | HTML5 similar to Asciidoctor; `InlineSubstitutionRenderer` trait | HTML, semantic HTML5, ANSI, manpage, Markdown; ASG JSON | `dr-html` targeting Asciidoctor's exact HTML; `html5s`; WASM |
| Spec TCK / ASG | None | Passes current (thin) TCK; emits draft ASG | TCK adapter crate |
| Stated maturity | Author: feature-complete since 0.25 (Jul 2026), 1.0 expected within a year; 99.5% coverage | Author: compatibility references, not conformance; 600+ fixtures | Author: vast majority of documented behaviour; STEM and highlighting unchecked |
| Gaps that matter here | no book doctype, no setext, no extensions before 1.0, HTML unsanitised | stricter limits than Asciidoctor by design; PDF backend listed but unverified | thin docs; byte offsets only |

**Recommendation: acdc-parser as the Phase 2 document model, behind an internal `adoc_dom` crate.** It is the only one whose tree can outlive the source buffer (required for WYSIWYG in Phase 3), the only one that maps positions through includes (required for click-to-source in a page assembled from partials), and it already supports the book doctype Phase 4 needs. asciidoc-parser is the stronger bet for Asciidoctor HTML fidelity and release cadence; keep it as the second implementation of the same trait and run both over the same fixture corpus in CI so the choice can be reversed. The existing `adoc-parser` stays for what it is good at: instant, panic-free outline and reference extraction on half-typed input.

**Compatibility fallback.** Asciidoctor.js 4.1.1 (5 Oct 2026, MIT) is now a native-JavaScript rewrite with no Opal layer, \~700 kB browser bundle, async API. Embedding it through [rquickjs](https://docs.rs/crate/rquickjs/latest) (QuickJS-NG, 0.14.0, \~1 MiB added to the binary, sub-millisecond startup) gives an "render exactly as Asciidoctor would" mode for diffing the native renderer, at interpreter speed (roughly 10 to 50x slower than V8, still well under a second for a typical page). V8 via `deno_core` adds 40+ MiB and is only worth it if the app later wants a plugin runtime. Treat this as a Phase 2 stretch, not a dependency: it needs polyfills (`TextDecoder`, `URL`) that nobody has verified for the 4.x bundle.

**The spec.** The Eclipse AsciiDoc Language project is still incubating with no spec release since 0.3.0 (Feb 2024) and a TCK of about 13 tests. Asciidoctor's own behaviour remains the conformance target; the draft ASG JSON (with `location` boundaries) is nevertheless a sensible interchange shape for the editor's document model, and both candidate parsers can already emit it.

### GUI framework

GPUI is viable outside Zed, but only through third-party snapshot crates, and Zed's editor crates are GPL, so the editor surface has to be written or taken from gpui-component. egui is the proven low-risk alternative: both ArtCraft apps and Lockbook's WYSIWYG Markdown editor ship on it.

| Framework | Version, licence | Rendering and text | Multi-window | Accessibility, IME | Shipped editors | Pain points |
| --- | --- | --- | --- | --- | --- | --- |
| [GPUI](https://www.gpui.rs/) | Official `gpui` 0.2.2 (Oct 2025, Apache-2.0); current API via Longbridge's weekly `gpui-pre` 0.3.9 (12 Oct 2026); Zed 1.0 shipped 29 Apr 2026 | Metal + Core Text (macOS), DirectX + DirectWrite (Windows), wgpu + cosmic-text 0.19 (Linux); taffy layout | `App::open_window`; Zed is multi-window | AccessKit on all three OSes (examples marked untested); `PlatformInputHandler` IME | Zed. Zed's `rope`, `text`, `editor`, `multi_buffer` crates are **GPL-3.0** and `publish = false` | Weekly breaking changes; snapshots maintained by one person; macOS needs Xcode |
| [gpui-kit / gpui-component](https://github.com/longbridge/gpui-kit) | 0.8.0 (19 Oct 2026, Apache-2.0) | On GPUI; 60+ components, a tree-sitter + LSP code editor claimed stable at 200k lines, native Markdown/HTML rendering, docks, `gpui-webview` (wry), `gpui-shell` JS host (rquickjs) | Yes | Not addressed in README | Longbridge Pro (commercial) | Pins exact `gpui-pre` snapshots; a11y unknown |
| [egui / eframe](https://github.com/emilk/egui) | 0.37.0 (15 Oct 2026, MIT OR Apache-2.0) | egui-wgpu or glow; own rasteriser, 0.37 adds system-font provider (fontique) | Viewports map to real windows on native | AccessKit on by default; 0.37 was a major a11y release; IME native | WordCraft, PdfCraft, Lockbook (WYSIWYG Markdown on egui + comrak) | Immediate mode: first-frame layout jitter, costly huge scroll areas; no native look |
| [iced](https://github.com/iced-rs/iced) | 0.14.0 (Dec 2025, MIT) | wgpu or tiny-skia; cosmic-text 0.15; IME added in 0.14 | `daemon` + `window::open` | **No AccessKit in mainline** (only the System76 fork) | COSMIC Text Editor | Elm architecture verbose for a large editor; a11y gap |
| Xilem / Masonry | 0.4.0 (Oct 2025, Apache-2.0) | winit + Vello 0.11 + Parley 0.12 + AccessKit | Added in 0.4 | AccessKit yes; IME not called out | None | Self-described experimental |
| Floem | 0.2.0 (Nov 2024 on crates.io, MIT; main active) | wgpu/vello/skia; parley | Yes | **No AccessKit** | Lapce 0.4.6 | Release cadence; a11y |
| Slint | 1.18.1 (Sep 2026; GPL-3 OR royalty-free OR commercial) | FemtoVG/Skia/software/Vello | Unconfirmed | AccessKit via winit | None | `.slint` DSL; no rich-text editing widget; attribution clause |
| Dioxus desktop / Tauri 2 | Dioxus 0.7.10; Tauri 2.12.5 (Oct 2026) | System webview (WebView2, WKWebView, WebKitGTK) | Yes | Browser a11y | Many | A browser, not Rust; WebKitGTK + NVIDIA DMABUF issues on Linux; Verso (Servo) archived Oct 2025 |

**Recommendation: GPUI through gpui-kit, gated by a two-week spike; egui as the named fallback.** The case for GPUI is Zed-grade text editing DNA, a ready-made tree-sitter + LSP code editor that can drive `adoc-ls` in-process, docks for the sidebar and preview, and `open_window` for the detachable preview. The case against is real: the API moves weekly, the supply chain is one maintainer's snapshot train, and accessibility is unproven. The spike (Phase 1 week 1 to 2) opens a 5,000-line `.adoc` in gpui-component's editor with the AsciiDoc grammar, attaches `adoc-ls` handlers in-process, and opens a second window. If it fails on any OS or the pinned snapshot cannot be upgraded cleanly, switch to egui, where the same engine crates plug into `eframe` the way `wordcraft-ui-egui` sits on `wordcraft-engine`. Either way the UI crate is the only crate allowed to import the framework.

### Editor core

| Need | Choice | Why |
| --- | --- | --- |
| Source buffer | [ropey](https://docs.rs/ropey/2.0.0-beta.1/ropey/) 2.0.0-beta.1 (byte-indexed; gpui-kit already pins it). [crop](https://github.com/nomad/crop) 0.4.3 is 3 to 4x faster on edit traces but LF/CRLF only | Byte offsets match `adoc-core`'s `SourceRange`; Zed's rope is GPL |
| Incremental syntax | tree-sitter 0.27.1 with the fork's block + inline grammars | Highlighting, folding, outline at keystroke speed |
| Shaping | Whatever the framework uses (cosmic-text 0.19 on GPUI/Linux, Core Text/DirectWrite elsewhere) | Do not add a second shaping stack |
| Structured document model (Phase 3) | Own `adoc-model` crate: typed node tree + ProseMirror-style steps and position mapping. Evaluate [prosemirror-rs](https://docs.rs/crate/prosemirror/0.5.0) 0.5 (Jun 2026, MIT) first | Transactions with inverse steps give undo, selection mapping and collaborative-ready history. CRDTs (loro 1.17, automerge 0.12) only if collaboration ever becomes a goal |

### Preview rendering

Three routes, and the choice decides how much of Phase 2 survives into Phase 3.

| Route | What it is | Fidelity | Reuse in WYSIWYG | Risk |
| --- | --- | --- | --- | --- |
| A. AST to native elements | `adoc-view` walks the document tree and emits framework elements (GPUI `div`/text/svg, or egui widgets), like Zed's `markdown` crate and gpui-component's native Markdown renderer | Controlled by us; no CSS engine | **Full**: the same element tree becomes the editable surface in Phase 3 | Every block type is hand-built; tables and code highlighting are the long tail |
| B. HTML to [Blitz](https://github.com/DioxusLabs/blitz) | 0.3.0-beta.4 (16 Oct 2026, MIT/Apache; Stylo is MPL-2.0): Stylo + Taffy + Parley + Vello, renders into a wgpu texture; block/inline/flex/grid, tables emulated via grid; no JS, no `@page`, IME planned | High for HTML we author; CSS gaps tolerable | Low: a DOM is not an editing model | Beta quality; pulls a large dependency tree |
| C. HTML in a webview | wry 0.57 via `gpui-webview` or Tauri | Exact Asciidoctor HTML + CSS | None | Platform webview bugs; not Rust |

**Recommendation: Route A for the product, Route C behind a feature flag for fidelity checks.** The HTML backend of the parser is still produced and used for export and for diffing against Asciidoctor, but the preview pane renders from the AST. This is more work in Phase 2 and the reason Phase 3 is tractable.

### Diagrams

Mermaid needs a DOM and font metrics in JavaScript, so embedding a JS engine is a research project, not a plan. [merman](https://docs.rs/merman/) 0.9.2 (21 Oct 2026, MIT OR Apache-2.0) is a headless Rust port targeting Mermaid 11.15 fixtures with 37 diagram types, Dagre/Cytoscape/optional ELK layouts and SVG/PNG/PDF output; its README says Zed adopted it as its Mermaid backend. Alternatives are weaker: mermaid-rs-renderer 0.3.1 (23 types, admits mismatches), ariel-rs 0.4.0 (small). Graphviz has no complete pure-Rust layout engine (`layout-rs` is limited; `graphviz-rust` shells out to `dot`); PlantUML needs Java; D2 is Go.

**Recommendation: an `adoc-diagrams` crate with merman built in and a provider chain for everything else** (user-configured `dot`, `plantuml`, `mmdc`, `d2` binaries, then a Kroki URL), emulating Asciidoctor Diagram's semantics: `[mermaid, target, format]` block form and `mermaid::file[]` macro, output name from `target` or a checksum of the source, `diagram-*`/`mermaid-*` attributes, and a checksum-keyed cache under the component's images output directory. SVG goes to the preview as a vector element and to PDF as embedded SVG.

### PDF

| Option | Version | Gives you | Missing |
| --- | --- | --- | --- |
| [Typst](https://typst.app/docs/changelog/0.15.0/) as a library (`typst`, `typst-pdf`, optionally [typst-as-lib](https://github.com/Relacibo/typst-as-lib) 0.17.0) | 0.15.1 (Jul 2026, Apache-2.0) | Pagination, running headers/footers with recto/verso, outline/TOC, footnotes, index via packages, tagged PDF/UA-1, PDF/A-2a, variable fonts; `typst-pdf` is itself built on krilla | A second markup language in the pipeline (generated, never hand-edited); `typst-as-lib` API unstable |
| [krilla](https://github.com/LaurenzV/krilla) | 0.8.2 (Jun 2026, MIT/Apache) | Font subsetting, colour fonts, tagged PDF, PDF/A and UA, outlines, links; `krilla-svg` | **No layout or pagination**; you write the engine (this is WordCraft's route, and WordCraft had to write `wordcraft-layout`) |
| printpdf | 0.12.8 | HTML-to-PDF via azul-layout | Marked evolving; no gradients |
| genpdf | 0.2.0 (2021) | Nothing current | Abandoned |
| Webview print | wry `print()` | Dialog-driven only | Not headless; WebView2 `PrintToPdfAsync` not wrapped |

**Recommendation: generate Typst from the AST and compile in-process.** It is the only route that delivers book-style output (chapters, running content, TOC, index, PDF/UA) without building a layout engine, and Typst's own crate split proves it works as a library. The asciidoctor-pdf YAML theme model maps cleanly: themes flatten to a key map with `extends:` inheritance, `$variable` references and arithmetic (`round($base_font_size * 1.25)`) evaluated at load; `page` keys become `page()` arguments, `base`/`heading` keys become `set text`/`show heading` rules, `header`/`footer` recto/verso columns become a three-cell grid in `page.header`/`page.footer`, `title-page`/`toc`/`index` keys become template functions, `font.catalog` becomes font files handed to the Typst `World`. Bundle Noto Serif, Noto Sans and M+ 1mn as asciidoctor-pdf does so the `default` theme renders identically out of the box. Krilla direct is the fallback if Typst's layout ever proves too opinionated for a theme.

## Proposed architecture

AdocFoundry is one Cargo workspace at [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) that adopts the five `adoc-*` crates from zed-asciidoc, adds a document-model layer above them, and keeps every front end (GPUI app, CLI, LSP) as a thin consumer of one engine.

```text
Front ends        adocfoundry (GPUI app)   adocfoundry-cli   adoc-ls (LSP)
                          |                      |               |
                          v                      v               v
Engine            adoc-engine: sessions, documents, command registry, undo, watcher, settings
                          |
                          v
Document services adoc-model (P3)   adoc-render (P2)   adoc-diagrams (P2)   adoc-pdf (P4)
                          |
                          v
Language layer    adoc-parser   adoc-dom (new)   adoc-antora   adoc-index   (adoc-lang, see Phase 1 spec)
                          |
                          v
Core              adoc-core: Document, LineIndex, SourceRange, Section, Anchor, Reference, Diagnostic
```

*Crate ladder: 5 layers, 3 front ends. Arrows are the only permitted dependency direction. The rendered diagram is in the Claude Doc version.*

Arrows show the only permitted dependency direction; a crate never imports a crate above it, and only `adocfoundry` imports GPUI. The accented boxes are the two pieces that do not exist yet in any form: the GPUI shell and `adoc-dom`.

**Workspace layout** (Typst/Zed pattern: `crates/*`, shared `[workspace.package]`, `default-members` on the app binary):

```
AdocFoundry/
  Cargo.toml                 workspace; default-members = ["crates/adocfoundry"]
  crates/
    adoc-core                L0  (from zed-asciidoc)
    adoc-parser              L1  (from zed-asciidoc)
    adoc-antora              L1  (from zed-asciidoc)
    adoc-index               L1  (from zed-asciidoc)
    adoc-dom                 L1  new: full AST behind `trait DocumentParser`
    adoc-model               L2  Phase 3: editable tree + steps
    adoc-render              L2  (from zed-asciidoc) + NativeRenderer
    adoc-diagrams            L2  Phase 2
    adoc-pdf                 L2  Phase 4
    adoc-engine              L3  sessions, commands, undo, watcher, settings
    adoc-ls                  L4  (from zed-asciidoc) LSP binary
    adocfoundry-cli          L4  headless binary
    adocfoundry              L4  GPUI app
  grammars/                  git submodule or vendored tree-sitter-asciidoc (master branch)
  themes/                    bundled PDF themes (base, default, default-sans)
  fonts/                     Noto Serif, Noto Sans, M+ 1mn (OFL)
  tests/fixtures/            Antora components, parity corpus vs Asciidoctor
```

Whether the five existing crates move into this workspace or stay in zed-asciidoc and are consumed as git dependencies is an open question (see Risks). Moving them and leaving zed-asciidoc as a thin extension that depends on AdocFoundry's `adoc-ls` is the cleaner end state.

**Data flow on a keystroke (Phase 2 onward)**

1. The source pane edits a `ropey` buffer; tree-sitter re-parses incrementally for highlighting in the same frame.
2. `adoc-engine` debounces (about 100 ms idle), runs `adoc-parser` for the outline and diagnostics immediately, and `adoc-dom` on a worker thread for the full AST with includes resolved through the Antora catalog.
3. `adoc-render` turns the AST into a view tree (`adoc-view` inside the app crate) and HTML (for export and fidelity diffing); `adoc-diagrams` fills diagram blocks from its cache or renders them asynchronously, replacing a placeholder when done.
4. The preview window receives the view tree over an in-process channel; scroll sync and click-to-source use the byte ranges every node carries.

**Cross-platform concerns**

| Concern | Approach |
| --- | --- |
| Rendering | GPUI: Metal on macOS, DirectX on Windows, wgpu on Linux (Vulkan, GL fallback). Test on an Intel iGPU Linux box and Windows ARM64 early |
| Fonts | App UI uses system fonts through the framework; PDF export ships its own fonts so output is identical everywhere |
| Paths | `adoc-index::normalize_path` already stays lexical; Antora resource IDs always use `/`; Windows needs long-path handling and case-insensitive matching flagged, never assumed |
| File watching | `notify` crate with debouncing; Antora descriptors re-read on change (gap in adoc-ls today) |
| External tools | `dot`, `plantuml`, `mmdc` found via PATH and a settings override; never required |
| Packaging | macOS universal DMG (signed, notarised), Windows MSI x64 + ARM64, Linux AppImage + Flatpak + deb/rpm, the PdfCraft matrix; `cargo-dist` or `cargo-packager` |
| Binary size budget | Under 40 MB without the optional QuickJS fallback; Typst and its fonts are the largest contributors |

## Phase 1 — AsciiDoc editor with Antora support and link following

Phase 1 is mostly a shell around code that exists: the deliverable is a GPUI application that opens an Antora component, edits pages with highlighting and an outline, and follows every `xref`, `include` and `image` the catalog can resolve.

**Scope**

- Workspace: open a folder; discover every `antora.yml` beneath it; build the catalog and index; watch for changes (`notify`), re-read descriptors on change.
- Sidebar: page tree from `nav.adoc` (falls back to the `pages/` directory tree), module and family browser, diagnostics list.
- Editor: gpui-component's code editor with the AsciiDoc block + inline grammars; folding, outline (sections and Antora nav), bracket matching, source-block injection highlighting for the common languages.
- Language features, in-process through the `adoc-ls` handler layer (not over stdio): go-to-definition on xrefs, includes, images and attributes; hover; completion for targets, anchors and `{attributes}`; diagnostics for unresolved references; workspace symbol search.
- Link following: Ctrl/Cmd-click opens the target page, partial, example or image in a tab; back/forward navigation stack; peek for partials.
- Find and replace across the component with regex; rename of a page updates every xref to it (first write-side refactor; conservative, previews the edit list).
- Settings file (TOML) and per-workspace overrides; light and dark themes.

**Work packages**

| # | Package | Depends on | Notes |
| --- | --- | --- | --- |
| 1.1 | GPUI spike: editor + grammar + second window on all three OSes | — | Gate for the framework decision |
| 1.2 | Workspace bring-up: move or depend on the five `adoc-*` crates, `adoc-engine` skeleton with command registry and undo | 1.1 | Decide repo strategy here |
| 1.3 | Catalog and index in the engine, file watcher, descriptor reload | 1.2 | Closes two of adoc-ls's known gaps |
| 1.4 | Sidebar: nav tree, families, diagnostics | 1.3 |  |
| 1.5 | In-process language features via `adoc-ls` handlers | 1.3 | Handlers already take byte offsets |
| 1.6 | Link following and navigation stack | 1.5 |  |
| 1.7 | Cross-component and cross-version resolution in `adoc-antora` | 1.3 | Model already carries both; implement selection rules and `page-aliases` |
| 1.8 | Find/replace and page rename | 1.5 | First mutating command; exercises undo |
| 1.9 | Packaging and CI matrix (macOS universal, Windows x64/ARM64, Linux x86\_64/aarch64) | 1.1 | Early, so platform bugs surface before Phase 2 |

**Done when** a user can open the Antora docs repo of a real project, click through every link in its nav without a broken target, and the LSP handlers report the same diagnostics the Zed extension reports for the same files.

## Phase 2 — Rendered preview

Phase 2 adds the full AST (`adoc-dom`), renders it natively beside the source, and makes the preview a first-class window that can be torn off and that follows whichever page is active in the editor.

**Pipeline.** Source buffer → (debounce \~100 ms) → `adoc-dom` parse with includes resolved through the catalog and Antora attributes applied → view tree → preview. The parse runs on a worker thread; the view tree is immutable and cheap to clone, so the detached window and the docked pane can show the same document without a second parse. Every view node keeps the byte range of the source that produced it and, for included content, the include chain.

**Preview behaviours**

| Behaviour | Design |
| --- | --- |
| Side by side | A dock pane in the main window; splitter ratio persisted per workspace |
| Detachable window | `open_window` with the same view model; closing the window re-docks. Window position and monitor remembered. Works as a second-monitor reading pane |
| Follow active page | Preview subscribes to the engine's active-document event; switching editor tabs switches the preview, with the scroll position of each page remembered. A pin control freezes the preview on one page |
| Scroll sync | Source line → nearest view node by byte range, and back; both directions, with a toggle |
| Click to source | Click a rendered block to place the caret in the source, including inside an included partial (opens the partial in a tab, caret at the right line) |
| Navigation | xrefs in the preview navigate the preview (and, when following, the editor); external links open the browser; images and attachments open natively |
| Diagnostics overlay | Unresolved xrefs render as marked links; missing images as placeholders with the resource ID |

**Rendering coverage, in order.** Sections and TOC; paragraphs and inline formatting (constrained and unconstrained, roles, attributes, replacements); lists (nested, description, callout, checklist); admonitions; source and listing blocks with syntax highlighting via the same tree-sitter grammars the editor uses; tables (column specs, spans, header/footer, AsciiDoc cells); images with `imagesdir` and Antora `image$` resolution; block quotes, verses, sidebars, examples, open blocks; footnotes; STEM (render with `typst` math or an SVG from a library, decide in the spike); index terms ignored in preview.

**Diagrams.** `adoc-diagrams` renders Mermaid through merman synchronously for small diagrams and on a worker for large ones, writes SVG to the checksum-named cache under the module's images directory (or a workspace cache if the user prefers not to commit generated images), and the view shows an SVG element. Other diagram kinds go to configured external tools or Kroki; while rendering, the block shows a placeholder with the diagram source.

**Fidelity harness.** The HTML backend of `adoc-dom` is compared against `asciidoctor` output for a fixture corpus in CI (normalised DOM diff). This is also where the optional QuickJS + Asciidoctor.js renderer earns its place: it lets the fidelity diff run on machines without Ruby.

**Work packages**

| # | Package | Depends on |
| --- | --- | --- |
| 2.1 | `adoc-dom` crate: `DocumentParser` trait, acdc-parser implementation, fixture corpus, position mapping through includes | Phase 1 |
| 2.2 | View tree model and `adoc-view` renderer for the block types above, in coverage order | 2.1 |
| 2.3 | Preview pane, worker-thread parse, debounce, follow-active-page | 2.2 |
| 2.4 | Detachable window | 2.3 |
| 2.5 | Scroll sync and click-to-source | 2.3 |
| 2.6 | `adoc-diagrams` with merman, provider chain, cache | 2.2 |
| 2.7 | Fidelity harness vs Asciidoctor; optional Asciidoctor.js-in-QuickJS renderer | 2.1 |
| 2.8 | asciidoc-parser as the second `DocumentParser` implementation, run over the same corpus | 2.1 |

**Done when** the Antora documentation for a real project renders with no block type falling back to raw source, Mermaid diagrams appear without a browser, and a page assembled from partials can be clicked through to the partial's source.

## Phase 3 — WYSIWYG editing

WYSIWYG is the hard phase, and the design choice that makes it possible is that the source text stays the source of truth: the editable view is a projection, every edit is a transaction on `adoc-model`, and the transaction is serialised back to AsciiDoc so that text the editor does not understand survives untouched.

**The model.** `adoc-model` is a typed tree (document, section, paragraph, list, table, block, inline run with marks) built from `adoc-dom`'s AST. Each node carries three things: its semantic content, the exact source slice it came from, and a `verbatim` flag for constructs the model does not understand (a macro from an unknown extension, an odd passthrough, a conditional). Edits are ProseMirror-style steps (replace range, add or remove mark, wrap, lift, split, join) with inverses, so undo, redo and selection mapping come from the step log rather than from snapshots. [prosemirror-rs](https://docs.rs/crate/prosemirror/0.5.0) is evaluated first as the step and position-mapping engine; if its schema model fits AsciiDoc's block set the crate is adopted, otherwise its design is followed in-house.

**Round-tripping rules**

1. A node that was not touched by a transaction serialises as its original source slice, byte for byte, including attribute lists, block titles, comments and blank-line spacing.
2. A touched node re-serialises from its semantic content using the style it had before (constrained vs unconstrained formatting, `*` vs `-` bullets, delimiter lengths, table cell separators) so a one-word edit does not reformat a block.
3. Resource IDs on xrefs, includes and images are kept as the original strings; the editor never rewrites `xref:` targets unless the user changes the link.
4. Includes render inline but are not editable in place in the first release; the included region is read-only with an "edit partial" affordance that opens the source. Editing inside includes is a follow-on once the model can address a node by include chain.
5. Conditionals (`ifdef::`) show the branch that applies under current attributes; the inactive branch is preserved verbatim and shown collapsed.

**Editing surface.** The Phase 2 view tree gains a caret, selection and hit testing (every node already knows its layout box in GPUI). Keyboard editing goes through commands (`insert_text`, `toggle_strong`, `set_block_type`, `indent_list_item`, `insert_table_row`), the same registry the source pane uses, so shortcuts and a future command palette are shared. A formatting toolbar and a block-type picker are the only new chrome. Source and WYSIWYG are two views on one engine document and can be open side by side; a change in one updates the other through the same transaction stream, which is also how the preview already updates.

**What stays source-only in Phase 3.** Tables with AsciiDoc-formatted cells (`a` style) beyond paragraph content, STEM editing, complex attribute entries, extension macros, and anything the model flags verbatim: these render read-only in the WYSIWYG view with a one-click jump to the source location.

**Work packages**

| # | Package | Depends on |
| --- | --- | --- |
| 3.1 | `adoc-model`: tree, steps, inverse steps, position mapping; prosemirror-rs evaluation | Phase 2 |
| 3.2 | Serialiser with the round-trip rules; property test: parse → serialise is identity on the fixture corpus | 3.1 |
| 3.3 | Caret, selection, hit testing on the view tree; IME composition | 3.1 |
| 3.4 | Inline editing: text, marks, links | 3.2, 3.3 |
| 3.5 | Block editing: paragraphs, headings, lists, admonitions, code blocks | 3.4 |
| 3.6 | Tables and images | 3.5 |
| 3.7 | Source ↔ WYSIWYG live sync and shared undo | 3.4 |
| 3.8 | Toolbar, block picker, command palette | 3.5 |

**Done when** a documentation page edited for an hour in WYSIWYG mode produces a Git diff that touches only the lines the writer changed.

## Phase 4 — PDF export with template support

PDF export compiles Typst generated from the AST, in process, with a theme loader that reads asciidoctor-pdf's YAML format so existing themes carry over.

**Theme model.** `adoc-pdf` loads a YAML theme, applies `extends:` (bundled `base`, `default`, `default-sans`, `default-for-print`), flattens keys the way asciidoctor-pdf does (`heading_h2_font_size`), resolves `$variable` references and evaluates arithmetic (`round($base_font_size * 1.25)`), and normalises units (pt default; in, mm, cm, %, em) into a typed `Theme` struct. Unknown keys produce a warning listing the key, not a failure. The mapping to Typst:

| Theme keys | Typst |
| --- | --- |
| `page.size`, `page.layout`, `page.margin` (incl. inner/outer) | `set page(paper:, flipped:, margin: (inside:, outside:, …))` |
| `base.font-*`, `base.line-height`, `base.text-align` | `set text(font:, size:, …)`, `set par(leading:, justify:)` |
| `heading.h1..h6.*`, `heading.font-*` | `show heading.where(level: n): set text(...)` + spacing rules |
| `title-page.*`, `cover` | A title-page template function emitting a page before the body |
| `toc.*` | `outline()` with a show rule for the dot leaders and levels |
| `header.*`, `footer.*`, `running-content.start-at`, recto/verso columns, `{page-number}`, `{section-title}`, `{document-title}` | `set page(header: context [...])` with a 3-cell `grid`, parity from `here().page()`; attribute references substituted from the document |
| `admonition.*`, `code.*`, `table.*`, `list.*`, `quote.*`, `sidebar.*`, `example.*`, `caption.*` | Block template functions, one per AsciiDoc block type |
| `font.catalog`, `font.fallbacks` | Font files passed to the Typst `World`; fallback list honoured |
| `index.*` | Index terms collected into a package-based index (in-dexter or an in-house equivalent) |

**Document assembly.** One page, or a whole component in nav order (book doctype: parts and chapters from nav nesting, Antora start page as the preface). Xrefs between pages become internal links; xrefs to other components become external URLs if a site URL is configured. Images come from the catalog; diagrams come from `adoc-diagrams` as SVG. Source blocks keep their highlighting through the same tree-sitter grammars, emitted as Typst `raw` with a custom theme.

**Output options.** PDF 1.7 default, PDF/A-2a and PDF/UA-1 switches (Typst 0.15 supports both at once), outline bookmarks from sections, page labels, metadata from document attributes. Export runs from the app (with a live Typst preview pane later) and from `adocfoundry-cli pdf --theme my-theme.yml`.

**Work packages**

| # | Package | Depends on |
| --- | --- | --- |
| 4.1 | Theme loader with inheritance, variables, arithmetic, units; tests against the bundled asciidoctor-pdf themes | Phase 2 |
| 4.2 | AST → Typst emitter for the block set, with escaping and round-trip tests | 2.1 |
| 4.3 | Typst `World` with bundled fonts, in-process compile, error mapping back to AsciiDoc lines | 4.2 |
| 4.4 | Running content, title page, TOC, parity | 4.1, 4.3 |
| 4.5 | Multi-page book assembly from nav | 4.4 |
| 4.6 | Index, PDF/A, PDF/UA | 4.5 |
| 4.7 | Visual regression: render reference pages with asciidoctor-pdf and compare rasterised pages | 4.4 |

**Done when** the `default` theme produces a PDF of a reference document that a reader cannot tell apart from asciidoctor-pdf's at reading distance, and a custom theme file used with asciidoctor-pdf loads without edits.

## Phase 5 — Release, STEM, the language server and MCP

Phase 5 turns the four phases into a release and opens the engine to other tools. It is specified in [`docs/prd/PRD-AdocFoundry_Phase_5_Implementation_Specification.md`](../prd/PRD-AdocFoundry_Phase_5_Implementation_Specification.md).

- **Release:** CI runs on Linux, macOS and Windows, with the asciidoctor-pdf comparison and PDF validation. Packages are signed where credentials allow, and each phase's manual QA is recorded on each platform.
- **STEM:** AsciiMath and LaTeX are converted to Typst maths by a new `adoc-math` crate and checked to hold no code. They are typeset natively in PDFs, and as SVG in the preview and the HTML backend, through the diagram cache.
- **`adoc-ls`:** it moves onto `adoc-engine`, so LSP clients get the app's diagnostics, references, page rename with link updates and PDF export.
- **`adoc-mcp`:** a stdio MCP server over the engine's commands, read-only unless started with `--allow-write`.

## Risks, open questions and spikes

The biggest risks are supply-chain ones: every pure-Rust AsciiDoc parser and the GPUI snapshot crates are single-maintainer, pre-1.0 projects. The design answers that with traits and a fixture corpus rather than with a bet on any one of them.

| Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- |
| GPUI snapshot crates break or stall (one maintainer, weekly API churn) | High | High | Pin exact versions; UI confined to one crate; egui fallback kept credible by never leaking framework types into the engine; the Phase 1 spike is the go/no-go |
| GPL contamination from Zed's editor crates | Medium | High | Depend only on `gpui`, `gpui-pre`, `gpui-component` (Apache-2.0); never vendor `editor`, `text`, `rope`, `multi_buffer` |
| Chosen AST parser (acdc) diverges from Asciidoctor or stops | Medium | High | `DocumentParser` trait with asciidoc-parser as a second implementation; fidelity corpus diffed against Asciidoctor in CI; the fork-and-maintain option is realistic for a 1-person parser |
| Native preview long tail (tables, STEM, highlighting) delays Phase 2 | High | Medium | Coverage order with raw-source fallback blocks; optional webview route for comparison only |
| Round-trip losses in WYSIWYG damage a user's source | Medium | Very high | Verbatim nodes by default; property test parse → serialise identity; diff shown before save in early releases |
| merman coverage gaps vs real Mermaid | Medium | Low | Provider chain to `mmdc`/Kroki; cache keyed by source so a gap is visible, not silent |
| Typst layout too opinionated for a theme (e.g. exact asciidoctor-pdf spacing) | Medium | Medium | Visual regression corpus; accept reading-distance parity, not pixel parity; krilla direct route as last resort |
| Linux GPU variance (wgpu on Intel/NVIDIA/Wayland) | Medium | Medium | GL fallback; CI on a software-rendered Linux runner; early AppImage testing |
| Scope: four phases for one engineer | High | High | Each phase ships something usable; Phase 1 reuses \~10k existing lines; CLI in Phase 1 makes CI and dogfooding possible from the start |

**Open questions**

- [x] Repository strategy: move the five `adoc-*` crates into AdocFoundry and make zed-asciidoc depend on them, or keep them in zed-asciidoc and consume by git dependency? **Decided: moved.** The crates and fixtures were imported with their history in Phase 1, `adoc-lang` was extracted from `adoc-ls`, and zed-asciidoc is now the extension alone, installing `adoc-ls` from AdocFoundry.
- [x] Licence for AdocFoundry: Apache-2.0 like zed-asciidoc, or MIT OR Apache-2.0 like the ArtCraft apps and most of the dependency graph? **Decided: Apache-2.0**, matching zed-asciidoc and tree-sitter-asciidoc; `deny.toml` enforces a permissive-only dependency licence list.
- [ ] Where generated diagram images live: committed under `images/` (Antora-friendly) or in a workspace cache?
- [x] Should `adoc-ls` gain the engine's command layer so Zed users get rename and preview commands for free, or stay minimal? **Decided: it moves onto `adoc-engine` in Phase 5** (WP 5.6).
- [x] STEM rendering route for preview and PDF (Typst math in both is the simplest consistent answer). **Decided: Typst maths in both**, native in PDFs and as SVG in the preview and HTML (Phase 5, WP 5.4–5.5).
- [ ] Whether to keep the Asciidoctor.js-in-QuickJS renderer at all once the fidelity corpus is stable.

**Spikes before committing, two weeks each**

1. GPUI: gpui-component editor with the fork's grammars, `adoc-ls` handlers in-process, a second window, on macOS, Windows and Linux. Pass: all three build from one pinned snapshot and the editor is smooth on a 5,000-line file.
2. Parser: acdc-parser and asciidoc-parser over an Antora component with partials, conditionals and tables; compare position fidelity and HTML against Asciidoctor. Pass: positions map through includes and the HTML diff is explainable.
3. Preview: render 200 lines of mixed AsciiDoc as GPUI elements from the AST, with one merman diagram. Pass: under 16 ms per frame while scrolling, under 50 ms to rebuild after an edit.
4. PDF: load `default-theme.yml`, emit Typst for a ten-page document with running headers and a TOC, compile in process. Pass: output side by side with asciidoctor-pdf is recognisably the same theme.

## Sources

Versions, dates and licences were read from crates.io, GitHub and project docs on 23 Oct 2026. Your own repositories were cloned and read directly.

**Your repositories:** [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) · [bovinemagnet/zed-asciidoc](https://github.com/bovinemagnet/zed-asciidoc) (incl. `docs/prd/DESIGN-Preview_Pipeline_and_Native_Rendering.md`) · [bovinemagnet/tree-sitter-asciidoc](https://github.com/bovinemagnet/tree-sitter-asciidoc) (docs/ARCHITECTURE.md, LIMITATIONS.md, ROADMAP.md)

**Reference projects:** [gpui.rs](https://www.gpui.rs/) · [gpui-pre docs](https://docs.rs/gpui-pre/=0.3.9/) · [Zed 1.0](https://zed.dev/blog/zed-1-0) · [gpui-kit](https://github.com/longbridge/gpui-kit) · [PdfCraft](https://github.com/storytold/pdfcraft/) · [WordCraft](https://github.com/storytold/wordcraft) · [Lockbook](https://github.com/lockbook/lockbook)

**Antora:** [Resource ID coordinates (3.2)](https://docs.antora.org/antora/3.2/page/resource-id-coordinates/)

**AsciiDoc parsers and spec:** [asciidoc-rs/asciidoc-parser](https://github.com/asciidoc-rs/asciidoc-parser) · [nlopes/acdc](https://github.com/nlopes/acdc) · [Building acdc](https://nlopes.dev/writing/building-acdc) · [jaredh159/asciidork](https://github.com/jaredh159/asciidork) · [delfanbaum/asciidocr](https://github.com/delfanbaum/asciidocr) · [cathaysia/tree-sitter-asciidoc](https://github.com/cathaysia/tree-sitter-asciidoc) · [Eclipse asciidoc-lang](https://projects.eclipse.org/projects/asciidoc.asciidoc-lang) · [asciidoc-tck](https://gitlab.eclipse.org/eclipse/asciidoc-lang/asciidoc-tck) · [Asciidoctor.js migration guide](https://docs.asciidoctor.org/asciidoctor.js/latest/setup/migration-guide/) · [rquickjs](https://docs.rs/crate/rquickjs/latest) · [deno\_core](https://docs.rs/crate/deno_core/latest)

**GUI and editor core:** [egui 0.37.0](https://newreleases.io/project/github/emilk/egui/release/0.37.0) · [iced 0.14.0](https://github.com/iced-rs/iced/releases/tag/0.14.0) · [iced accessibility issue](https://github.com/iced-rs/iced/issues/552) · [Xilem](https://github.com/linebender/xilem) · [Slint 1.18](https://slint.dev/blog/slint-1.18-released) · [Dioxus 0.7](https://dioxuslabs.com/blog/release-070) · [Floem](https://github.com/lapce/floem) · [Tauri Linux graphics](https://v2.tauri.app/develop/debug/linux-graphics/) · [ropey 2.0 beta](https://docs.rs/ropey/2.0.0-beta.1/ropey/) · [crop](https://github.com/nomad/crop) · [cosmic-text](https://github.com/pop-os/cosmic-text) · [parley](https://github.com/linebender/parley) · [tree-sitter releases](https://github.com/tree-sitter/tree-sitter/releases) · [prosemirror-rs](https://docs.rs/crate/prosemirror/0.5.0) · [Loro](https://github.com/loro-dev/loro)

**Preview, diagrams, PDF:** [Blitz](https://github.com/DioxusLabs/blitz) · [Blitz CSS status](https://blitz.is/status/css) · [Servo 0.1.0](https://servo.org/blog/2026/04/13/servo-0.1.0-release/) · [merman](https://docs.rs/merman/) · [mermaid-rs-renderer](https://github.com/1jehuang/mermaid-rs-renderer) · [Asciidoctor Diagram](https://docs.asciidoctor.org/diagram-extension/latest/) · [Kroki](https://docs.kroki.io/kroki/) · [Typst 0.15.0 changelog](https://typst.app/docs/changelog/0.15.0/) · [typst-as-lib](https://github.com/Relacibo/typst-as-lib) · [krilla](https://github.com/LaurenzV/krilla) · [printpdf](https://github.com/fschutt/printpdf) · [Asciidoctor PDF theming](https://docs.asciidoctor.org/pdf-converter/latest/theme/) · [Theme language](https://docs.asciidoctor.org/pdf-converter/latest/theme/language/) · [Running content](https://docs.asciidoctor.org/pdf-converter/latest/theme/running-content/) · [Font support](https://docs.asciidoctor.org/pdf-converter/latest/theme/font-support/)
