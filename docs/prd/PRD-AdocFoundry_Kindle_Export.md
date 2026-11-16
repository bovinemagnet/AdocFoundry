# PRD: AdocFoundry — Kindle Export

| Field | Value |
| --- | --- |
| Status | Draft, 2026-11-16; not yet approved for implementation. K1 (§13) is a feasibility gate for everything after it, and the open decisions in §15 are settled in the work package they affect |
| Date | 2026-11-16 |
| Scope | A native publication backend that writes reflowable Kindle books, `.kfx` (KF10) and `.azw3` (KF8), for a page, an Antora component version or selected navigation files, entirely in Rust, through the engine, CLI, desktop app and MCP server |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Related work | Epic #616; work packages #627 to #635 (§13). #619 (the shared publication plan, from the DOCX epic #613). Separate exports, never prerequisites: #613 DOCX, #614 EPUB, #615 DocBook |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `AGENTS.md`; `docs/prd/PRD-AdocFoundry_DOCX_Export.md`; `docs/prd/PRD-AdocFoundry_Phase_4_Implementation_Specification.md` (PDF export); `cargo xtask layers` |
| Existing code | `crates/adoc-engine/src/export.rs`, `crates/adoc-engine/src/profiles.rs`; `crates/adoc-render/src/view.rs`; `crates/adoc-diagrams`; `crates/adoc-math` |

## 1. Purpose and problem

AdocFoundry publishes a page or an Antora book as PDF. A PDF reads badly on a Kindle: its pages are fixed, so the reader cannot change the font size, the screen orientation or the theme. Authors who want their documentation on a Kindle convert it with Calibre, KindleGen or Kindle Previewer, which means installing other tools, running them by hand, and losing the diagnostics that say where in the source a problem lies.

Add Kindle publishing as a first-class export, entirely in Rust: a single AsciiDoc page as an article, or an Antora component version or selected navigation tree as a book, written as real **`.kfx` (KF10)** and **`.azw3` (KF8)** files, never an EPUB with a Kindle label. A book has chapter navigation, metadata, a cover, embedded images and diagrams, readable STEM, hyperlinks, diagnostics with source locations and publication profiles.

This is publication. It is not a Kindle reader, a live Kindle preview, a device-management utility or an upload client for Kindle Direct Publishing (KDP).

## 2. Goals

1. Export one page to an article and one component version or navigation selection to a book, with the same content selection, order and deduplication as the PDF exporter.
2. Write genuine containers: a KFX file a Kindle reads as KFX and an AZW3 file it reads as KF8, each confirmed by reading it back in tests and on real readers.
3. Keep content as text a reader can reflow, select and search: headings, lists, code, tables, footnotes and links, never a page flattened to a picture.
4. Give every book working navigation: a reading order, a nested table of contents, a start location, chapter boundaries and in-book links that land.
5. Make every book self-contained: cover, images, diagrams and formulas are embedded bytes.
6. Run every stage in process as Rust code (§7), with no GPL dependency.
7. Make publication repeatable through `[kindle]` settings and `[kindle.profiles.<name>]`, independent of every other format's settings.
8. Offer the same export through the desktop app, the CLI and a write-gated MCP tool, all through `adoc-engine`.
9. Never lose content or change format silently: an unsupported construct or a missing asset gives a diagnostic with its source location, and a format that cannot be written correctly gives a typed error.
10. Describe KDP accurately: these files are for reading and sideloading; KPF, the format KDP takes, is researched separately (§4, K9).

## 3. Non-goals

- A Kindle reader, a Kindle live preview, sending a book to a device, or any KDP, cloud or service integration.
- `.mobi` (MOBI7, KF7) output or a dual-format file for legacy devices. KDP no longer accepts MOBI manuscripts.
- `.kpf` output in the first release. It is a research track (K9) and joins the format selector only after a writer passes its own acceptance criteria.
- Fixed-layout books, and any copy of the PDF's page design. A reflowable Kindle book has no running headers, footers or page numbers of its own; the reader supplies pagination and progress.
- Depending on another format's exporter. Kindle output never needs the DOCX exporter (#613), an EPUB exporter (#614) or a DocBook exporter (#615), and never writes an `.epub` file on the way.
- Requiring Calibre, KindleGen, Kindle Create, Kindle Previewer or any other tool, to export or to run the tests.

## 4. Formats

| Target | First release | Intended use | Requirement |
| --- | --- | --- | --- |
| `.kfx` (KF10) | **Required** | Reading on current Kindle devices and apps, where they support it | A genuine KFX container with navigation and assets, validated on real supported readers before release |
| `.azw3` (KF8) | **Required** | Compatibility with Kindle readers that support KF8 | A genuine KF8 container in a PalmDB/MOBI file; not a renamed EPUB and not legacy MOBI7 |
| `.kpf` (Kindle Package Format) | **Research only** (K9) | Manuscript submission to KDP | A native Rust writer is a separate feasibility decision, not a promise; no Kindle Create subprocess |
| `.mobi` (legacy) | **Out of scope** | Legacy Kindle devices | None |

A requested `.kfx` is never downgraded to `.azw3` or `.epub`. When a target cannot be written correctly the export fails with a typed unsupported or validation error.

KFX and AZW3 are files a Kindle reads. They are not KDP manuscript formats: Amazon recommends EPUB, DOCX and KPF for KDP. **KPF is a different format from KFX**, and neither a `.kfx` nor a relabelled AZW3 is a `.kpf`. The user documentation says so wherever it describes the export.

## 5. User workflows

**Export a page.** With a page open, choose Export Page to Kindle…, pick KFX or AZW3 and a file, and receive the article. Diagnostics appear as they do for a PDF export.

**Export a book.** Choose Export Component to Kindle… for a component version; the book follows the effective navigation order, each page a chapter, with a nested table of contents and a cover.

**Publish a profile.** Define `[kindle.profiles.handbook]` once, then run `adocfoundry-cli kindle --profile handbook` locally or in CI, with `--fail-on warning` to gate on diagnostics.

**An agent exports.** With `--allow-write`, an MCP client calls `export_kindle` for a page, component or profile, under the same root confinement as `export_pdf`.

**The reader reads.** The file is copied to a Kindle or opened in a Kindle app. It appears in the library with its cover and title; the table of contents, links and footnotes work; text reflows at any font size and orientation.

## 6. Functional requirements

### 6.1 Publication selection and book structure

- Select a page, a versioned or unversioned Antora component, or explicit navigation files. Follow the effective navigation order, deduplicate pages as the PDF exporter does, honour the start page and source attributes, and keep the part and chapter hierarchy.
- Write a valid reading order and an in-book table of contents with correct anchors, nested section links, a start location and chapter boundaries, built from the book plan: versioned components, `ROOT`, start pages, aliases, deduplicated pages and the heading depth after pages are merged into one book.
- Carry the metadata: title, subtitle, authors, language, identifier, publisher, version and date where available, description, and the cover shown in the library.
- Generated anchors and chapter identifiers are stable, do not depend on file paths, and are namespaced so that two pages' anchors never collide.

### 6.2 Content

- Real text for paragraphs, the heading hierarchy, bold, italic and code, literal and source blocks, ordered, bulleted, nested and description lists, admonitions, quotes, tables, captions, thematic and page breaks, image descriptions, and footnotes with links both ways.
- Internal cross-references between pages and to anchors are in-book links. A target outside the publication uses its published URL when one is configured, otherwise readable text and a diagnostic. External hyperlinks stay external.
- Where a construct or layout is not supported (complex tables, embedded media), the content stays readable and a warning names its source location. The documented fallback for each such case is part of K5.

### 6.3 Reflowable layout

- Content reflows for any screen size, font size, orientation and theme. The style model is small and reader-friendly: no fixed page dimensions, no forced body font, no positioning by pixel.
- The reader owns pagination, running content and progress. The PDF's repeated headers, footers and page numbers are not written into the book; the title, the chapter titles and the metadata carry that information instead.
- Images and tables are sized for reflow, without large fixed widths or fonts tied to a platform.

### 6.4 Assets and STEM

- Embed local PNG and JPEG images and rendered diagrams in the container, through the workspace's existing file-access rules. SVG is embedded as SVG only where the chosen backend supports it; any rasterising, resizing or transcoding uses Rust libraries in process.
- Keep each image's aspect ratio, caption and alternative text where the format supports them. The cover is named as the cover in the metadata, is present as embedded bytes, and is wired to the book's cover section where the format has one. How a sideloaded book's library thumbnail behaves is documented, with no device-side tool required.
- Inline and display STEM comes from the existing view and formula pipeline. Native, selectable maths is preferred where an implementation is proven; otherwise a legible image with a readable text alternative, and a warning for a case that cannot be shown. No external TeX or maths command.
- Embedded fonts are optional and must pass size and font-licence checks.
- No image or font is ever fetched over HTTP. A missing or corrupt asset gives a diagnostic with its source span, never an invisible omission.
- Image decoding, decompression and each asset's and the book's total bytes are limited.

### 6.5 Format validity

- **KFX:** a correct container and fragments, dictionaries and symbol references, reading order, navigation and storyline, content positions, image references and metadata. Not an arbitrary archive with a `.kfx` extension.
- **AZW3:** a real KF8 PalmDB/MOBI container with correct records, headers, EXTH metadata, navigation, fragment and record offsets and images. Not MOBI7 relabelled.
- Both: record bounds, encodings, content references, embedded-image signatures, generated identifiers, file-size limits and deterministic ordering are validated before the file is put in place. Integer overflow, unescaped markup and dangling references are refused.
- Reading a file back without error is not acceptance. Each format is also checked on supported Kindle hardware and apps before it is called ready (§12).

### 6.6 Configuration

- `[kindle]` holds defaults; `[kindle.profiles.<name>]` holds a publication: `format = "kfx" | "azw3"`, a `page`, `component` or `nav` selection, `output`, title, creator, language, identifier, cover and an optional formatting policy. It reuses `ProfileContent` and the output-path rules of `crates/adoc-engine/src/profiles.rs`.
- Nothing Kindle-specific goes in `[pdf]` or in any other format's table, and no PDF theme or standard is reinterpreted.
- Invalid profiles, an output extension that disagrees with the format, paths that leave the workspace and unknown settings are refused or warned about in the engine's established style.

### 6.7 Front ends

- **CLI:** `adocfoundry-cli kindle <file|component-dir> --format kfx|azw3 --output <file>`, `kindle --component [<version>@]<name>`, `kindle --profile <name>`, and `--fail-on none|warning|error` with deterministic exit codes.
- **Desktop:** Export Page to Kindle…, Export Component to Kindle… and Export Kindle Profile…, with a format selector, the save dialog and the progress, completion and diagnostics of the other exports. No Kindle preview.
- **MCP:** `export_kindle`, listed and callable only with `--allow-write`, with a typed schema, structured output, a check that the extension matches the format, root confinement, link refusal, protection against unsafe overwrites, bounded responses and timeouts.
- Every front end calls `adoc-engine` and produces the same content, diagnostics and errors; none reaches `adoc-kindle` directly. No front end transfers a book to a device or a service.

## 7. Rust-only execution and licensing

- Every stage of the Kindle export runs in process as Rust code: building the book, packaging, compression, transcoding artwork, validation and writing the file. It never calls Calibre, KindleGen, Kindle Create, Kindle Previewer, Java, Python, Node.js, Wine, a shell command, an HTTP service or any Amazon executable. The same holds for the automated tests.
- `adoc-kindle`'s own source is held to a forbidden-paths list in `xtask/src/layers.rs` (no process, no network), as `adoc-audit` and `adoc-impact` are.
- A third-party crate is accepted only after a review of its licence, the toolchain it needs (the workspace's `rust-version` is 1.99), safety, maintenance, features and build size, and it must pass `cargo deny check`. The licence check covers test dependencies too.
- No GPL or other copyleft dependency. [`boko`](https://github.com/zacharydenton/boko) reads and writes both formats but is GPL-3.0; it is not added to this repository, as a dependency or as a test dependency, without a separate, explicit licensing decision.
- [`ebook-rs`](https://github.com/SV-stark/ebook-rs) (MIT, a KFX writer) and [`epub3-kindle`](https://github.com/cbz-tools/Epub3-Kindle) (MIT, an AZW3 writer) are **candidates only**. Both are young projects with few users, and what a README claims is not evidence. K1 builds a constrained prototype with each and tests the result on real readers before either is chosen. An in-house writer remains an option where a dependency falls short.
- A dependency may take structured XHTML or EPUB-like parts in memory, as long as the conversion stays in Rust and those parts are internal data. No `.epub` file is written.
- The workspace forbids `unsafe` code, so an in-house writer is written without it.

## 8. Existing code to reuse

| Code | What it gives | How Kindle uses it |
| --- | --- | --- |
| `crates/adoc-render/src/view.rs` | `ViewTree`, `ViewKind`, `ViewInline`, `ViewImage`, `DiagramView`, `ResolvedLink`, `TocEntry`, `ViewFootnote`, source spans and document attributes | The exporter's only input. No second AsciiDoc parser or Antora resolver is introduced |
| `crates/adoc-engine/src/export.rs` | `ExportTarget`, the navigation-based article and book plan, parsing from open buffers or files, view and diagram preparation, the worker lifecycle, error reporting, file-access checks and temporary output handling | Through the format-neutral plan #619 extracts for PDF and DOCX. Kindle builds on that extraction and never copies the planner; it inherits no `typst` dependency |
| `crates/adoc-engine/src/profiles.rs` | `ProfileContent` (`Page`, `Component`, `Navigation`) and the output checks | The selection model and path rules; no PDF theme or standard option |
| `crates/adoc-diagrams`, `crates/adoc-math`, the engine's image resolution | Rendered diagrams, checked STEM markup and resolved images | The prepared assets, with their source provenance kept for diagnostics |
| The PDF entry points in the CLI, the app and the MCP server | Commands, dialogs, notifications and the write gate | Kindle entry points sit beside them as format-specific commands, not a post-processing pipeline |

## 9. Architecture

```text
L4  adocfoundry / adocfoundry-cli / adoc-mcp
                     |
L3  adoc-engine     shared publication plan (#619), parsing, views,
                    diagrams, worker lifecycle, safety, settings and profiles
                     |
L2  adoc-kindle     KindleBook IR, metadata and style policy,
                    KFX and AZW3 writers, structural and readback validation
L2  adoc-render     shared ViewTree / assets / source spans
L1  existing parsing / DOM / Antora / index crates
```

| Crate | Responsibility |
| --- | --- |
| `adoc-kindle` (L2, new) | Map view trees and document facts to a `KindleBook`; write it as KFX or AZW3; validate the bytes; report typed diagnostics with source locations. Headless; no engine, worker, UI or LSP types, no process and no network |
| `adoc-render` (L2) | Unchanged |
| `adoc-engine` (L3) | The shared publication plan; the optional `kindle` feature; Kindle commands, arguments and events; `[kindle]` settings and profiles; the worker lifecycle; safe output |
| `adocfoundry`, `adocfoundry-cli`, `adoc-mcp` (L4) | The entry points of §6.7 |

- `adoc-kindle` sits at **L2**. It may depend on `adoc-render` through a new pair in `SAME_LAYER_ALLOWED` in `xtask/src/layers.rs`, as `adoc-pdf` does, and on lower layers. It must not depend on `adoc-pdf`, `adoc-docx`, any EPUB or DocBook exporter, GPUI or `adoc-engine`.
- `KindleBook` is a private semantic intermediate representation: chapters, the heading tree, the reading order, anchors and links, notes, styles, metadata and assets. **Both writers consume the same `KindleBook`**, which is what makes the two formats agree. It is not a general EPUB intermediate and is never exposed to users.
- The crate needs an entry in `LAYERS`, an entry in `[workspace.dependencies]`, and a mention in `AGENTS.md`, `README.md`, the design document and the developer documentation's crate table and diagram (`architecture.adoc`, `crate-layers.mmd`), which `every_crate_is_documented` in `xtask` checks.
- The engine's `kindle` feature is opt-in and independent of `typst`, `docx` and any other format's feature. It is off in `[workspace.dependencies]`, so each front end names it; `adoc-ls` does not, and its default build compiles no Kindle dependency.
- All new dependencies are pinned exactly.

## 10. Headless contracts

Names are illustrative; K2 settles them and records the outcome in §16.

- `adoc-kindle`: a `KindleBook` built deterministically from one or more view trees and document facts; a format (`Kfx`, `Azw3`); options (metadata, cover, formatting policy); a writer per format behind one interface, with shared cancellation and error semantics; a result carrying the bytes' diagnostics. A typed error distinguishes an unsupported target from a failed validation.
- `adoc-engine`: Kindle export commands beside `export.pdf_page` and `export.pdf_component`, with no PDF-only field in their arguments; a blocking export for the CLI; completion reported as an `EngineEvent`, consistent with `ExportFinished`.
- Diagnostics follow the existing `ExportDiagnostic` model, with severities that allow CI gating.

## 11. Output safety, performance and reliability

- A finished book is written to a temporary sibling file, its completed bytes validated, and only then put in place under the existing safe export policy. A failure or a cancellation leaves the source and any existing output intact.
- Link targets, a wrong extension, a path that leaves the roots and other unexpected outputs are refused.
- Source size, image decoding, decompression and in-memory staging are limited. Export runs on the bounded worker pipeline and never blocks the UI; chapters are not copied more often than needed.
- The same input gives the same bytes: content order is fixed, and timestamps and generated identifiers are derived from the document or normalised.
- Corrupt input, text that is not UTF-8, very large input and Unicode in every field (CJK included) are handled without a panic.

## 12. Tests and acceptance criteria

Each test below exists under its name when its work package is done. All of them are Rust tests that run no process and need no reader, converter or validator installed.

| # | Test | Package | What it shows |
| --- | --- | --- | --- |
| 1 | `kindle_and_pdf_select_the_same_pages` | K2 | For an article and a versioned book over several components, Kindle and PDF take the same pages in the same order, unsaved buffers included, without calling `adoc-pdf` |
| 2 | `kindle_builds_alone` | K2 | A build with `kindle` and no other format feature compiles neither Typst nor another exporter; `cargo xtask layers` passes |
| 3 | `kindle_anchors_are_stable_and_unique` | K2 | Anchors and chapter identifiers are the same on every run, independent of paths, and never collide across pages |
| 4 | `azw3_is_a_kf8_container` | K3 | The file is detected as KF8: not an EPUB, a ZIP or MOBI7 under another name |
| 5 | `azw3_reads_back_complete` | K3 | An article and a multi-chapter book read back with their metadata, table of contents, anchors, text and an image |
| 6 | `kfx_is_a_kfx_container` | K4 | The file has a KFX signature and container, and every fragment reference resolves |
| 7 | `kfx_reads_back_complete` | K4 | Chapter order, table of contents, metadata, hyperlinks and embedded images read back |
| 8 | `kindle_never_substitutes_a_format` | K3, K4 | A target that cannot be written gives a typed error; no other format is written in its place |
| 9 | `kindle_formats_agree` | K5 | One fixture gives the same logical chapters, links and table of contents in both formats, with no broken in-book link |
| 10 | `kindle_content_stays_text` | K5 | Lists, code, tables and footnotes are text in both outputs, not images |
| 11 | `kindle_unsupported_content_warns` | K5 | A missing external target, unsupported media and a complex table each give a located diagnostic and a readable fallback |
| 12 | `kindle_media_is_embedded` | K6 | A cover, PNG, JPEG, a diagram and inline and display formulas are in both files' bytes, with valid signatures, references, counts and cover linkage |
| 13 | `kindle_bad_assets_are_reported` | K6 | Oversize, malformed, missing and remote assets fail or degrade with a located warning; nothing is fetched and nothing is dropped silently |
| 14 | `kindle_profiles_resolve_and_validate` | K7 | Page, component and navigation profiles resolve to the right pages, format and output; invalid ones are refused |
| 15 | `kindle_failed_export_keeps_existing_output` | K7 | A failed or cancelled export, a link, a wrong extension and a path outside the roots leave the existing file as it was |
| 16 | `kindle_cli_exports_and_gates` | K7 | The CLI writes an article, a book and a profile in both formats, with exit codes that follow `--fail-on` |
| 17 | `kindle_mcp_export_is_write_gated` | K7 | `export_kindle` is listed only with `--allow-write` and refuses unsafe outputs |
| 18 | `kindle_app_exports_through_engine` | K7 | The desktop actions invoke the engine's commands and report completion and diagnostics; no preview is opened |
| 19 | `kindle_is_in_process` | K8 | `adoc-kindle` names no process and no network path, and its dependency graph holds no denied licence |
| 20 | `kindle_reference_book_is_complete` | K8 | The fixture matrix (a page, a versioned book, `ROOT`, aliases, nested navigation, shared partials, footnotes, CJK, code, tables, formulas, a cover, images, a missing image, malformed input, long chapters, many pages, conflicting anchors) passes every structural check in both formats |
| 21 | `kindle_output_is_deterministic` | K8 | Two exports of one input are the same bytes |
| 22 | `kindle_large_book_is_bounded` | K8 | A large, image-heavy book exports within the memory and time budgets K1 sets, off the UI thread |

Where feasible, the writers' offset and fragment arithmetic is fuzzed in `fuzz/`, each crash becoming a regression test.

What tests cannot show is recorded by hand: K1's findings in `docs/qa/spike-kindle.md`, and K8's checks on supported devices and apps (cover, chapter and table-of-contents navigation, footnotes, links, images and readable STEM, for both formats) in `docs/qa/kindle.md`. Neither is needed to build or to run the tests.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check`.

## 13. Work packages

| WP | Deliverable | Issue |
| --- | --- | --- |
| K1 | Feasibility spike: a writer approach for each format, licences, prototypes read back and tried on readers, supported devices, resource budgets | #627 (Kindle 1/9) |
| K2 | New `adoc-kindle` crate, the `KindleBook` representation, the engine's `kindle` feature and the shared publication plan | #628 (Kindle 2/9) |
| K3 | AZW3 (KF8) writer with chapter navigation | #629 (Kindle 3/9) |
| K4 | KFX (KF10) writer and reflowable layout | #630 (Kindle 4/9) |
| K5 | AsciiDoc semantics, references and Kindle navigation | #631 (Kindle 5/9) |
| K6 | Embedded covers, images, diagrams, fonts and STEM | #632 (Kindle 6/9) |
| K7 | `[kindle]` settings and profiles; engine, CLI, desktop and MCP entry points | #633 (Kindle 7/9) |
| K8 | Conformance tests, device QA, performance and documentation | #634 (Kindle 8/9) |
| K9 | Research: a native Rust KPF writer for KDP submission | #635 (Kindle 9/9) |

Order: K1 first, since it decides whether and how each format is written. K2 follows, on K1 and on #619. K3 and K4 proceed independently of each other; K5 and K6 integrate with both; K7 once the engine's interface is stable (its scaffolding may start earlier); K8 verifies and documents the release. K9 runs at any time, may reuse K1's findings, and never blocks the KFX and AZW3 release.

K9 ends in a recorded decision (implement separately, defer or decline) that tells documented fact from speculation, and exposes no placeholder `.kpf` output. If it approves a writer, that work gets its own epic.

## 14. Definition of done

1. A single page and a versioned Antora book produce real `.kfx` and `.azw3` bytes with the correct reading order and metadata, using only Rust execution.
2. Titles and chapters, a nested table of contents, internal links, code, lists, tables, footnotes, embedded covers, figures and diagrams and legible maths are present, or have a diagnostic naming the source; no page is flattened into a picture.
3. Each format passes its structural and readback tests and the manual checks on supported Kindle readers, with its limits documented. No format is ever substituted for another.
4. The app, the CLI and the write-gated MCP tool export through one engine pipeline. The other exporters are unaffected and none of their code is forked.
5. Builds and tests run no conversion process and no external application; the licence, toolchain, feature, memory and failure-safe write checks pass in CI.
6. KDP is described accurately: KFX and AZW3 are files for Kindle readers, not KDP manuscript formats, and KPF stays separate research until a writer is verified.

## 15. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's pull request and in §16.

- **Writer per format (K1).** `epub3-kindle` or in-house for AZW3; `ebook-rs` or in-house for KFX. The decision rests on the prototypes, not on the candidates' descriptions.
- **If KFX proves blocked (K1).** KFX has no public specification, and §4 requires it. Should K1 find no acceptable route, decide between releasing AZW3 alone with KFX deferred, and holding the feature until both work.
- **Independent readback (K1, K3, K4).** The tests need a reader that shares no code with the writer under test. The best-known Rust reader of both formats is GPL and cannot be a test dependency, so the choice is between a permissively licensed reader and an in-house one written apart from the writer.
- **Supported readers (K1).** Which Kindle device generations and apps each format is promised on, and so tested on in K8.
- **Diagrams drawn by external tools (K6).** Mermaid is rendered in process; Graphviz, PlantUML and D2 diagrams are rendered by optional tools found on `PATH`, for every export and the preview. Decide whether §7 covers only the Kindle conversion, so such diagrams are embedded as they are for PDF, or whether a Kindle export takes in-process diagrams alone and reports the rest.
- **Formulas without Typst (K6).** The STEM provider is part of the engine's `typst` feature, so a build with `kindle` alone has no formula artwork. The same question is open for DOCX (§13 of its PRD) and wants one answer.
- **Where SVG is rasterised (K6).** The app's `image_decode::svg_to_png` is at L4 and out of reach of `adoc-kindle`. Either the crate rasterises (`resvg`, already in the tree) or the engine hands it prepared rasters; also shared with DOCX.
- **Tables (K5).** What a table too complex for a reader becomes: simplified text, or an image with a text alternative.
- **Who extracts the shared plan (K2).** #619 belongs to the DOCX epic. If Kindle starts first, K2 carries the extraction and #619 builds on it; either way it is done once.
- **Profile sharing (K7).** Whether a publication's content selection is declared once and named by each format's profiles, or repeated in each; shared with DOCX.
- **Which builds include it (K7).** Whether the released CLI and MCP server enable `kindle` as the app does.
- **KPF (K9).** Implement separately, defer or decline.

## 16. Implementation notes

None yet. Each work package records here where the implementation departs from this document and how the open decisions of §15 were settled.

## 17. References

- Amazon KDP, [supported eBook formats](https://kdp.amazon.com/en_US/help/topic/G200634390)
- Amazon KDP, [the end of MOBI submissions](https://kdp.amazon.com/en_US/help/topic/GULSQMHU5MNH4EZM)
- [`ebook-rs`](https://github.com/SV-stark/ebook-rs), MIT: the KFX writer candidate
- [`epub3-kindle`](https://github.com/cbz-tools/Epub3-Kindle), MIT: the AZW3 writer candidate
- [`boko`](https://github.com/zacharydenton/boko), GPL-3.0: research only, not a dependency
