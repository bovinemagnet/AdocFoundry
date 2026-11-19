# PRD: AdocFoundry — EPUB Export

| Field | Value |
| --- | --- |
| Status | Draft, 2026-11-16; not yet approved for implementation. The open decisions in §14 are settled in the work package they affect |
| Date | 2026-11-16 |
| Scope | A native publication backend that writes reflowable EPUB 3.3 books for a page, an Antora component version or selected navigation files, entirely in Rust, through the engine, CLI, desktop app and MCP server |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Standard | [EPUB 3.3](https://www.w3.org/TR/epub-33/), W3C Recommendation |
| Related work | Epic #614; work packages #638 to #646 (§12). #619 (the shared publication plan, from the DOCX epic #613). Separate exports, never prerequisites: #613 DOCX, #615 DocBook, #616 Kindle |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `AGENTS.md`; `docs/prd/PRD-AdocFoundry_DOCX_Export.md`; `docs/prd/PRD-AdocFoundry_Kindle_Export.md`; `docs/prd/PRD-AdocFoundry_Phase_4_Implementation_Specification.md` (PDF export); `cargo xtask layers` |
| Existing code | `crates/adoc-engine/src/export.rs`, `crates/adoc-engine/src/profiles.rs`; `crates/adoc-render/src/view.rs`, `crates/adoc-render/src/html.rs` |

## 1. Purpose and problem

AdocFoundry publishes a page or an Antora book as PDF. A PDF has fixed pages: on a phone or an e-reader the text cannot reflow, and the reader cannot choose a font or its size. EPUB is the open standard for reflowable books, read by e-readers, phones, tablets and desktop reading applications. Authors who want one today convert through other tools, outside AdocFoundry's Antora resolution and without its source-located diagnostics.

Add a native, headless Rust EPUB 3.3 exporter, comparable to the PDF exporter in content coverage and Antora-aware selection, that writes standards-compliant, reflowable `.epub` books from one AsciiDoc page, an Antora component version or selected navigation files. A real EPUB holds XHTML content documents, a navigation document, metadata, CSS and embedded media in an OCF container.

This is publication. It is not a live EPUB preview, EPUB import, a reading application, or Kindle, DocBook or Word output.

## 2. Goals

1. Export one page to an article and one component version or navigation selection to a book, with the same content selection, order and deduplication as the PDF exporter.
2. Write a valid EPUB 3.3 package: a correct OCF container, a complete OPF package document and exactly one navigation document.
3. Write semantic, well-formed XHTML that a reading system can reflow, restyle and read aloud, never pictures of pages.
4. Keep every link working across chapters: each in-book reference resolves to a file and fragment inside the package.
5. Make every book self-contained: images, diagrams, formulas, the cover and the stylesheet are embedded and listed in the manifest.
6. Give a sound accessibility baseline (language, heading hierarchy, landmarks, alternative text, table headers), without claiming a conformance that structural checks cannot prove.
7. Run every stage in process as Rust code (§7).
8. Make publication repeatable through `[epub]` settings and `[epub.profiles.<name>]`, independent of every other format's settings.
9. Offer the same export through the desktop app, the CLI and a write-gated MCP tool, all through `adoc-engine`.
10. Never lose content silently: an unsupported construct, a missing asset or an unexported link target gives a diagnostic with its source location.

## 3. Non-goals

- Kindle formats (KFX, AZW3, KPF: #616), DocBook XML (#615) and Word DOCX (#613). A Kindle tool may one day read an EPUB; this exporter depends on none of them and none depends on it.
- Legacy EPUB 2 and an NCX-only publication, DRM and encryption, digital signatures, media overlays (read-aloud), embedded JavaScript, fixed-layout publications, EPUB import, a reader or editor, store upload APIs and conversion over a network.
- The PDF's appearance. A reflowable book has no authoritative physical pages, so there are no running headers, footers or page-number fields. A page list is deferred unless the source carries explicit page labels.
- EPUB 3.4. It was not yet a Recommendation in November 2026; 3.3 is released first and no 3.4 conformance is claimed.
- A claim of EPUBCheck, WCAG or EPUB Accessibility conformance from structural tests alone.
- Embedded fonts in the first release. If added later, they must pass licence and reader-compatibility rules.
- Requiring Calibre, Pandoc, Asciidoctor, EPUBCheck or any other tool, to export or to run the tests.

## 4. User workflows

**Export a page.** With a page open, choose Export Page to EPUB…, pick a file, and receive a self-contained article. Diagnostics appear as they do for a PDF export.

**Export a book.** Choose Export Component to EPUB… for a component version; the book follows the effective navigation order, each page a chapter, with a navigation document and a cover.

**Publish a profile.** Define `[epub.profiles.user-guide]` once, then run `adocfoundry-cli epub --profile user-guide` locally or in CI, with `--fail-on warning` to gate on diagnostics.

**An agent exports.** With `--allow-write`, an MCP client calls `export_epub` for a page or a component, under the same root confinement as `export_pdf`.

**The reader reads.** The file opens in a reading application. The table of contents, links and footnotes work, and the text reflows at the reader's chosen font and size.

## 5. Functional requirements

### 5.1 Output and publication scope

- Export one AsciiDoc page as a self-contained article, or an Antora component version or selected navigation trees as a single book in effective navigation order.
- Match the PDF exporter's start-page inclusion, deduplication, component version scoping, part and chapter hierarchy, page-title fallback, attributes, resolved includes, source provenance and profile semantics.
- One selected page normally becomes one XHTML content document; a book is a sequence of stable chapter files. A large page is split only if necessary, and never in a way that breaks a link or the spine.
- Text is UTF-8, with language and direction where known. Content is **reflowable** and respects the reading system's font size and viewport. An explicit page break becomes a semantic separator, not a fixed page.
- Title, creators, language, version and modified date, a stable unique identifier, description, subject and rights come from the document's and component's metadata and from profile overrides. Missing metadata is never invented silently: a default is documented or the value is required and reported.

### 5.2 Container and package

- The OCF container is a ZIP archive written in process. Its **first entry is `mimetype`**, stored uncompressed, with the exact bytes `application/epub+zip`, no byte-order mark, no padding and no extra field. No entry is encrypted and no entry name is unsafe as a path.
- `META-INF/container.xml` names the package document (for example `EPUB/package.opf`). The package document is EPUB 3 (`version="3.0"`) with a stable unique identifier, the required Dublin Core metadata, `dcterms:modified`, a complete `manifest` with correct media types, exactly one item with `properties="nav"`, and an ordered `spine` naming every linear reading item.
- Every local dependency (content documents, the stylesheet, images, the cover, diagram assets) is embedded and listed in the manifest. No manifest item and no in-book link target is missing, and no relative reference leaves the package.
- Part order and names are deterministic, identifiers are stable, and XML text and attribute values are escaped. Archive timestamps and `dcterms:modified` are reproducible when a date is supplied (`SOURCE_DATE_EPOCH` or an explicit modified date); an input that makes the output irreproducible is reported.
- Invalid resource data is refused, not packed into a broken archive.

### 5.3 Content

- Titles, heading levels and section nesting; paragraphs; strong, emphasis, code and mark; superscript and subscript; keyboard, menu and button markup; line breaks; links; anchors; role and class semantics; block and inline quotes.
- Ordered, bulleted, nested, description, check and callout lists; source and literal blocks; admonitions, sidebars and examples; captions and horizontal rules.
- Tables as semantic HTML tables with header and footer groups, `colspan`, `rowspan`, alignment and captions, in a logical reading order.
- Footnote references lead to labelled entries with links back; a footnote named more than once is valid.
- `lang`, structural grouping and `epub:type` semantics where known. No invalid XHTML nesting.
- No JavaScript, no active HTML passthrough, no remote script and no iframe becomes active content.
- A node that cannot be rendered gives a warning with its source span and a readable fallback; it never disappears.

### 5.4 Navigation and links

- The book plan and the effective navigation trees become one navigation document with **exactly one** `epub:type="toc"` navigation, a valid nested list, the correct namespaces and chapter-relative paths. Landmarks (cover, body matter) are added where they apply.
- The required navigation document is distinct from an optional table of contents shown in the reading order. No NCX is written.
- Chapter and anchor identifiers are deterministic and scoped by component and version; file names are stable. A map from source pages and anchors to in-book fragments supports duplicate page references, aliases, moved resources and cross-chapter bookmarks without collision.
- The start page and navigation-only grouping and title entries are honoured. Navigation entries and the spine follow the plan's order.
- An xref to a page or anchor in the publication becomes a chapter-relative link. A target outside it uses its published URL when one is configured, otherwise readable text and a diagnostic. External URLs stay ordinary hyperlinks. Every dangling internal link is detected; no `file:` or `antora:` link is ever written.

### 5.5 Images, diagrams and STEM

- Local PNG, JPEG and GIF images, and SVG where it is safe, are embedded with correct media types. Sizing keeps the aspect ratio and avoids fixed pixel widths on small screens; alternative text and captions are kept. Identical image data is stored once where that is safe.
- Diagrams prepared by the engine's workers (`DiagramView::Svg`) are embedded. SVG is sanitised, or rasterised, **in Rust**, so that no script, external reference or entity reaches the book. A missing or unrenderable diagram gives a diagnostic.
- STEM uses standards-conformant MathML where it is reliably available; otherwise the already-rendered formula artwork with an accessible text fallback. It never needs a PDF intermediate, and a formula is never dropped: a build that cannot render one shows its source text with a diagnostic.
- An optional cover image carries the `cover-image` property, with a separate cover document and landmark when requested. The `front-cover-image` and `title-logo-image` attributes may supply defaults.
- Assets are resolved under the engine's file-access rules. Nothing is fetched from a network. An external URL, an absolute path, a path that leaves the roots or a wrong media type never becomes a dependency of the book.

### 5.6 Styling, metadata and accessibility

- A small, responsive default stylesheet for reflowable books: readable type, headings, nested lists, tables, wrapped code, figures and captions. It is specific to EPUB and independent of PDF theme YAML, DOCX styles and the application's theme. No fixed-layout viewport and no required custom font.
- An opt-in workspace or user CSS file, confined to the roots and validated: no remote `@import`, no external asset, no script, nothing that makes the book depend on a network. A stylesheet changes appearance, never content.
- The package metadata is built from page and component attributes and profile overrides: title, `dc:language`, creator, description, subject, rights, a stable `dc:identifier` and a UTC `dcterms:modified`. A missing required value is reported.
- Structure for accessibility: a declared language, a meaningful heading hierarchy, landmarks, meaningful link text, image alternative text, table headers, a navigable table of contents and readable footnotes. Missing alternative text, a broken heading hierarchy and an unknown language are warned about.
- `accessibilityFeature` and `accessibilitySummary` metadata are written only where they can be defended from the content. Coverage and limits are reported, not glossed.

### 5.7 Configuration

- `[epub]` holds defaults; `[epub.profiles.<name>]` holds a publication: exactly one of `page`, `component` or `nav`, an `output` (inside the workspace, `<name>.epub` by default), title, language, identifier, modified date, cover, CSS and AsciiDoc attributes. It reuses `ProfileContent` and the confinement and conflict checks of `crates/adoc-engine/src/profiles.rs`.
- Cover and CSS paths are resolved safely against the roots. Unknown keys, wrong types and invalid component or version references are refused or warned about.
- User and workspace settings follow the documented precedence. Nothing in `[epub]` changes another format's output, and no other format's profile changes an EPUB.

```toml
[epub]
style = "default"
# css = "resources/epub/reader.css"
# language = "en-GB"

[epub.profiles.user-guide]
component = "3.0@product"
output = "dist/user-guide.epub"
title = "Product User Guide"
# cover = "modules/ROOT/assets/images/cover.png"
# identifier = "urn:uuid:<stable-id>"
```

### 5.8 Front ends

- **CLI:** `adocfoundry-cli epub <file|component-dir> [--root <dir>] [--component [<version>@]<name>] --output <file.epub> [--fail-on none|warning|error]` and `epub --profile <name>`, with argument handling and exit codes that mirror `pdf`.
- **Desktop:** Export Page to EPUB…, Export Component to EPUB… and Export EPUB Profile… in the menu and command palette, with the save dialog, progress and result notifications of the other exports. No EPUB preview and no reading application is launched.
- **MCP:** `export_epub`, modelled on `export_pdf`: a typed output schema, listed and callable only with `--allow-write`, root confinement, link refusal, safe overwrites, no network and bounded results. A page or a component at first; a profile once the settings contract is stable.
- Every front end calls the same engine operation and receives the output path, the chapter count and the diagnostics. No front end reaches `adoc-epub` directly.

## 6. Existing code to reuse

| Code | What it gives | How EPUB uses it |
| --- | --- | --- |
| `crates/adoc-engine/src/export.rs` | `ExportTarget`, the article and book plan (`PlanItem`), start-page handling, navigation order, deduplication, parsing from open buffers or disk, resolved views, diagram workers, progress, `ExportDiagnostic` and output safety | Most of it is gated by `feature = "typst"` today. EPUB uses the format-neutral plan #619 extracts and never forks a second planner |
| `crates/adoc-engine/src/profiles.rs` | `ProfileContent` (`Page`, `Component`, `Navigation`), output confinement and conflict checks | The selection model and path rules; PDF settings and profiles are untouched |
| `crates/adoc-ebook` | `Book::build`: an article or the plan's pages mapped to one `Book` (chapters in reading order, blocks and inlines, a nested table of contents, generated anchors, in-book links, footnotes, metadata with a stable identifier, embedded pictures with SVG drawn as PNG) | EPUB writes the same book Kindle export writes (§15). A choice that suits one format only is an option of the mapping, never a fork |
| `crates/adoc-render/src/view.rs` | `ViewTree`, `ViewKind`, `ViewInline`, `ViewImage`, `DiagramView`, `TocEntry`, footnotes, attributes, resolved xrefs and source spans | The exporter's only input. AsciiDoc is not parsed a second time |
| `crates/adoc-render/src/html.rs` | The semantic HTML mapping and escaping of `to_html` | A reference. EPUB writes its own well-formed XHTML with EPUB navigation and chapter-relative links; it does not zip the standalone HTML, and `to_html`'s output does not change |
| The PDF entry points in the CLI, the app and the MCP server | Commands, dialogs, notifications and the write gate | EPUB entry points sit beside them |

## 7. Rust-only execution

- Every stage of the export runs in process as Rust code: planning, XHTML, packaging, compression, image handling and validation. It never calls Calibre, Pandoc, Asciidoctor, EPUBCheck, Java, Node.js, Ruby, a Kindle utility, a web service or any other program. The same holds for the automated tests and their setup.
- `adoc-epub`'s own source is held to a forbidden-paths list in `xtask/src/layers.rs` (no process, no network), as `adoc-audit` and `adoc-impact` are.
- ZIP and XML dependencies are added only after a review of maintenance, licence, memory use and the toolchain they need, pinned exactly, and they pass `cargo deny check`. A small in-process writer and safe escaping are preferred to a browser or DOM engine.
- Checking a book by hand in reading applications is welcome and recorded (§10), but no such program is ever needed to build, export or test.

## 8. Architecture

```text
L4   adocfoundry / adocfoundry-cli / adoc-mcp
                  | format-specific commands and presentation
L3   adoc-engine  | shared publication planning (#619); parse + views +
                  | diagram workers; output safety; EPUB options and profiles
L2   adoc-epub    | EPUB 3.3 XHTML / OPF / OCF / navigation / CSS / media writer
L2   adoc-ebook   | the book model shared with Kindle export: pages mapped to one Book
L2   adoc-render  | shared ViewTree, ViewKind, ViewInline and source spans
L1   existing parser, DOM, Antora and index crates
```

| Crate | Responsibility |
| --- | --- |
| `adoc-epub` (L2, new) | Turn a publication's view trees and typed options into an EPUB: content documents, the navigation document, the package document, the container, the stylesheet and media, with a structural validator and typed diagnostics carrying source locations. Headless; no engine, worker, UI or LSP types, no process and no network |
| `adoc-ebook` (L2) | The reflowable book model, moved out of `adoc-kindle` (§15): `Book::build` maps the view trees to chapters, blocks, inlines, a table of contents, anchors, links, notes, metadata and pictures. Read by `adoc-kindle` and `adoc-epub` |
| `adoc-render` (L2) | Unchanged in behaviour. A shared pure rendering helper may be factored out only if `to_html`'s output and tests are preserved |
| `adoc-engine` (L3) | The shared publication plan; the optional `epub` feature; EPUB commands, arguments and events; `[epub]` settings and profiles; the worker lifecycle; safe output |
| `adocfoundry`, `adocfoundry-cli`, `adoc-mcp` (L4) | The entry points of §5.8 |

- `adoc-epub` sits at **L2**. It may depend on `adoc-ebook` and `adoc-render` through new pairs in `SAME_LAYER_ALLOWED` in `xtask/src/layers.rs`, as `adoc-kindle` does, and on lower layers. It must not depend on `adoc-engine`, GPUI, Typst, `adoc-pdf`, `adoc-docx` or `adoc-kindle`.
- The crate needs an entry in `LAYERS`, an entry in `[workspace.dependencies]`, and a mention in `AGENTS.md`, `README.md`, the design document and the developer documentation's crate table and diagram (`architecture.adoc`, `crate-layers.mmd`), which `every_crate_is_documented` in `xtask` checks.
- The engine's `epub` feature is opt-in and independent of `typst`, `git` and every other format's feature. It is off in `[workspace.dependencies]`, so each front end names it: the released app and CLI do, and `adoc-ls` does not.

## 9. Headless contracts

Names are illustrative; EPUB 1/9 and 2/9 settle them and record the outcome in §15.

- `adoc-epub`: an `EpubDocument` built from the publication plan's chapters (a view tree each) and document facts; `EpubOptions` (metadata, cover, stylesheet, modified date); a result carrying the diagnostics; a memory-conscious archive writer. The modified date and the identifier are data given to the crate, so it reads no clock and no environment.
- `adoc-engine`: EPUB export arguments with no PDF-only field, export commands beside `export.pdf_page` and `export.pdf_component`, a blocking export for the CLI, timeouts and cancellation, progress, and completion as an `EngineEvent` consistent with `ExportFinished`, carrying the output path, the chapter count and the warnings. The engine supplies the modified date (the profile's, else `SOURCE_DATE_EPOCH`, else the current time, which it reports as irreproducible).
- Diagnostics follow the existing `ExportDiagnostic` model, with severities that allow CI gating.

## 10. Output safety, performance and reliability

- A finished book is written to a temporary sibling file and renamed into place only on success. A failure, a timeout or a cancellation leaves the source and any existing output intact, and no partial file behind.
- A source document, a link, an unrelated file and a path outside the roots are refused as outputs.
- Preparation and packaging run on workers and never block the UI. Memory does not grow without bound with the number of chapters.
- Hostile input is handled without a panic or an unsafe result: path traversal in archive names, malformed XML, a missing file, a wrong media type, malicious SVG or CSS and oversize assets.
- With a fixed date and identifier, the same input gives the same bytes.

## 11. Tests and acceptance criteria

Each test below exists under its name when its work package is done. All of them are Rust tests that reopen the archive in process and run no other program.

| # | Test | Package | What it shows |
| --- | --- | --- | --- |
| 1 | `epub_package_is_valid_ocf` | E1 | `mimetype` is first, stored, exact and without an extra field; `container.xml` names the package; the package has its namespaces, required metadata, one navigation item, and a spine whose every reference is in the manifest |
| 2 | `epub_text_is_escaped` | E1 | Unicode and XML metacharacters are escaped; invalid XML and malformed entries are refused |
| 3 | `epub_output_is_deterministic` | E1 | With a given date and identifier, two exports are the same bytes |
| 4 | `epub_and_pdf_select_the_same_pages` | E2 | For an article and a multi-page Antora fixture, EPUB and PDF take the same paths in the same chapter order, navigation duplicates and start pages included |
| 5 | `epub_builds_alone` | E2 | A build with `epub` and no other format feature compiles neither Typst nor another exporter |
| 6 | `epub_failed_export_keeps_existing_output` | E2 | A failed export leaves the existing output and the source as they were |
| 7 | `epub_content_is_wellformed_xhtml` | E3 | Every chapter parses as XML; chapter structure, nested lists, a complex table, Unicode, code blocks, links and admonitions are as expected |
| 8 | `epub_footnotes_link_both_ways` | E3 | Footnote identifiers and back links are valid, a repeated reference included |
| 9 | `epub_unsupported_nodes_warn` | E3 | An unsupported block gives a located warning and a readable fallback; passthrough never becomes active content |
| 10 | `epub_nav_matches_the_plan` | E4 | Exactly one `toc` navigation exists; its entries and the spine follow the plan's order |
| 11 | `epub_internal_links_resolve` | E4 | With nested navigation, an alias, a duplicate target, cross-module references, a missing anchor and an out-of-book link, every local `href` and fragment resolves and no `file:` or `antora:` link is written |
| 12 | `epub_media_is_embedded` | E5 | Two raster formats, a repeated image, a diagram, a cover and a formula are in the archive with matching bytes, media types, manifest items, alternative text and responsive dimensions |
| 13 | `epub_bad_assets_are_reported` | E5 | A missing file, wrong content for its type, a denied path and an SVG with a script give a diagnostic or a safe fallback; the book never depends on a local path |
| 14 | `epub_metadata_is_complete_or_reported` | E6 | Title, language, identifier and modified date are present, or their absence is a documented diagnostic |
| 15 | `epub_css_has_no_external_dependencies` | E6 | The default stylesheet and an accepted override name nothing outside the package; an override with a remote import is refused |
| 16 | `epub_accessibility_structure` | E6 | Language, heading hierarchy, landmarks, alternative text and table headers are present, and their absence is warned about |
| 17 | `epub_profiles_resolve_and_validate` | E7 | Valid and invalid settings in both layers; a profile gives the expected content, metadata and media; unsafe output, cover and CSS paths are refused |
| 18 | `epub_settings_leave_other_formats_unchanged` | E7 | An EPUB profile selects the same ordered content as the equivalent PDF profile, and neither format's settings change the other's output |
| 19 | `epub_cli_exports_and_gates` | E8 | The CLI writes an article, a book and a profile, with exit codes that follow `--fail-on` |
| 20 | `epub_mcp_export_is_write_gated` | E8 | `export_epub` is listed only with `--allow-write`; traversal, a link, a conflicting overwrite, a refused write and a timeout leave no partial output |
| 21 | `epub_app_exports_through_engine` | E8 | The desktop actions invoke the engine's commands and show the save and notification states; no preview is opened |
| 22 | `epub_is_in_process` | E9 | `adoc-epub` names no process and no network path |
| 23 | `epub_reference_book_is_complete` | E9 | A realistic Antora book (nested and duplicate navigation, several component versions, includes, cross-chapter anchors, footnotes, a complex table, code, accessibility metadata, a cover, PNG, JPEG and SVG diagrams, STEM) passes the in-process validator with no broken link and no dropped content |
| 24 | `epub_large_book_is_bounded` | E9 | A book of many chapters exports within a memory budget, off the UI thread |

The in-process validator of E9 checks the container, the package, the navigation, well-formedness, file existence, fragment links and media types. It does not certify EPUB 3.3 conformance, and the documentation says so. Observations from reading applications are recorded by hand in `docs/qa/epub.md`, as tested behaviour and not as a guarantee across readers.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check`.

## 12. Work packages

| WP | Deliverable | Issue |
| --- | --- | --- |
| E1 | New `adoc-epub` crate with the OCF container and OPF package writer | #638 (EPUB 1/9) |
| E2 | The shared Antora publication plan and the engine's export orchestration | #639 (EPUB 2/9) |
| E3 | Semantic AsciiDoc blocks and inlines as valid EPUB XHTML | #640 (EPUB 3/9) |
| E4 | Book navigation, the EPUB table of contents, anchors and cross-chapter links | #641 (EPUB 4/9) |
| E5 | Embedded raster images, SVG diagrams, covers and STEM | #642 (EPUB 5/9) |
| E6 | Reflowable CSS, metadata and the accessibility baseline | #643 (EPUB 6/9) |
| E7 | Settings, cover and style options and publication profiles | #644 (EPUB 7/9) |
| E8 | CLI, desktop and write-gated MCP entry points | #645 (EPUB 8/9) |
| E9 | Rust-only conformance checks, interoperability and release QA | #646 (EPUB 9/9) |

Order: E1 and #619 underpin the rest, and E2 joins them. E3, E4 and E5 proceed side by side once their interfaces are stable; E6 follows them; E7 and E8 expose the export to users; E9 is the completion gate.

## 13. Definition of done

1. A page and an Antora book export as standalone EPUB 3.3 files, with no converter, command-line publisher or network call anywhere in the export.
2. The EPUB 3 structure is valid without an NCX: the first, stored `mimetype`, `container.xml`, metadata, manifest, spine, exactly one navigation item, valid relative links and no missing asset.
3. A realistic fixture with multi-chapter navigation, heading levels, rich text, lists, footnotes, tables, internal and external links, a cover, PNG and JPEG images, a generated diagram and STEM has readable XHTML and embedded assets; an unsupported construct gives a source-located diagnostic.
4. The export works through the shared engine in the CLI, the app and the write-gated MCP tool, with independent profiles and failure-safe file handling. Existing PDF output is unchanged.
5. The Rust-only archive, XML, metadata, link and accessibility-structure checks and the adversarial tests pass. Observations from common reading applications are documented, and no external program is part of the implementation or of CI.
6. An engine built with `epub` alone compiles no Typst, PDF, DOCX or Kindle backend. `cargo xtask layers`, `cargo deny check`, the workspace's lints and tests and the release smoke tests pass.

## 14. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's pull request and in §15.

- **ZIP and XML writers (E1).** `quick-xml` and a deflate implementation are already in the dependency tree; a ZIP writer is not. Either a ZIP crate that gives exact control of the first entry (stored, no extra field) and of timestamps, or a small in-house writer.
- **Sharing with `to_html` (E3).** A separate XHTML writer in `adoc-epub`, or a shared pure helper in `adoc-render` that leaves `to_html`'s output byte for byte as it is.
- **Passthrough (E3).** `ViewKind::Passthrough` and `ViewInline::Raw` hold raw HTML that need not be well-formed XML and may be active. Decide between leaving it out with a diagnostic, showing it as text, and accepting it only after parsing and sanitising.
- **Splitting large pages (E4).** Whether the first release ever splits a page into several content documents, and by what rule.
- **MathML (E5).** The view tree carries a formula's source and its typeset SVG; nothing in the workspace produces MathML. The first release can embed the SVG with the source as its text alternative, and leave MathML to a converter added later to `adoc-math`. This PRD already fixes the case without Typst (source text and a diagnostic), which answers for EPUB a question the DOCX and Kindle PRDs leave open.
- **SVG (E5).** Embed sanitised SVG, rasterise it, or both by case; and whether the sanitiser is written here or the engine hands over prepared assets. The rasterising half is shared with DOCX and Kindle.
- **Diagrams drawn by external tools (E5).** Graphviz, PlantUML and D2 diagrams are rendered by optional tools found on `PATH`, for every export and the preview. Decide whether §7 covers the EPUB writing alone, so such diagrams are embedded as they are for PDF, or whether an EPUB takes in-process diagrams only. Shared with Kindle.
- **Default identifier (E6).** When none is configured: refuse, or derive a stable one from the publication's coordinates and say so.
- **Validating user CSS (E6).** A CSS parser as a dependency, or a conservative check that refuses any `@import` and any `url(` outside the package.
- **EPUBCheck (E9).** This PRD keeps it out of the tests and CI, and records manual checks only. The PDF exporter, by contrast, is validated by veraPDF in a CI job of its own. Decide whether an optional EPUBCheck job outside the test suite is wanted, or whether the rule stands as written.
- **Who extracts the shared plan (E2).** #619 belongs to the DOCX epic and is now a prerequisite of three. Whichever epic starts first does the extraction, once.
- **Profile sharing (E7).** Whether a publication's content selection is declared once and named by each format's profiles, or repeated in each; shared with DOCX and Kindle.
- **EPUB 3.4.** When it becomes a Recommendation, a follow-up decides what changes.

## 15. Implementation notes

Each work package records here where the implementation departs from this document and how the open decisions of §14 were settled.

### Before E1: the shared book model

The epic starts with a package of its own, before E1: Kindle export's book model moved out of `adoc-kindle` into a new L2 crate, `adoc-ebook`, so that EPUB and Kindle map pages the same way instead of each through its own mapper.

- `book.rs`, `map.rs`, `media.rs` and `metadata.rs` moved unchanged in behaviour, with neutral names: `Book`, `Document`, `Page`, `Plan`, `BookItem`, `Options`, `Diagnostic`, `Severity`.
- `adoc-kindle` re-exports them under its old names, and Kindle's bytes are unchanged (Kindle PRD §16).
- `adoc-ebook` is held to Kindle's forbidden paths: no process and no network.
- This changes two statements above:
  - §6 and §9 take the view trees as the exporter's only input. `adoc-epub` takes them through `adoc-ebook`'s `Book` instead.
  - §5.4 asks for chapter and anchor identifiers scoped by component and version. The shared model names them by position in the book (`c<n>` for a chapter, `c<n>-a<m>` for an id a page defines, `c<n>-n<k>` and `c<n>-r<k>` for a footnote and its first mark). These names are deterministic, unique in the book and never taken from a path, which is what the requirement protects; they are not stable when chapters are added before them.
- Where EPUB needs a distinction Kindle export does not make, such as admonition kinds, nested tables or a table's footer rows, the mapping gains an option whose default is Kindle's present behaviour. The packages that need one (E3, E5, E6) check Kindle's bytes again.


### E1 (#638): the container and the package

- **ZIP and XML writers (§14):** both in-house, in `adoc-epub`, as Word export's are:
  - `zip.rs` writes the archive in order into memory: `mimetype` first and stored with no extra field, every other file deflated through `flate2`, CRC-32 through `crc32fast`. There are no data descriptors and no 64-bit extensions.
  - An entry name must be a relative path of ASCII letters, digits, `.`, `-` and `_`. An unsafe or repeated name is refused.
  - `xml.rs` escapes every document string and replaces the characters XML 1.0 forbids.
  - Every XML file is parsed with `roxmltree`, already in the tree, before the bytes are returned. A file that is not well-formed fails the export (`EpubError::Invalid`) rather than being packed.
- **The modification time is input.** `EpubOptions` has no default: it needs a `Modified`, a UTC time to the second between 1980 and 2107, the years a ZIP archive can date. It gives `dcterms:modified` and every entry's date, so the crate reads no clock, and the same input and time give the same bytes. Choosing the time (a profile's, else `SOURCE_DATE_EPOCH`, else now with a diagnostic) is the engine's job, in E2.
- **The names in the package are fixed:** `META-INF/container.xml`, `EPUB/package.opf`, `EPUB/nav.xhtml` and `EPUB/text/c<n>.xhtml`, from the book's own chapter ids.
- **Chapters are minimal for now.** E1 writes headings as headings and each other block as a paragraph of its text; E3 writes semantic XHTML.
- `dc:date` is not written, since `revdate` is free text and EPUB needs a date. E6 decides.
- The acceptance tests are `epub_package_is_valid_ocf`, `epub_text_is_escaped` and `epub_output_is_deterministic` (`crates/adoc-epub/tests/package.rs`). They read the archive back with the crate's own reader (`testing`, feature `test-support`), so they never trust a ZIP library to forgive the bytes.


### E2 (#639): the engine's `epub` feature

- **The feature:** `adoc-engine`'s `epub` feature turns on the shared export pipeline, `adoc-epub` and `adoc-ebook`.
  - It is a default of the engine crate, and off in `[workspace.dependencies]`. No front end names it until E8, so the app's palette test lists the two new commands as exceptions, as Kindle's were.
  - `epub_builds_alone` (in `xtask`) and CI's lint job hold the engine built with `epub` alone to no Typst and no other exporter.
- **No new planner:** the shared plan (#619) was already extracted by the DOCX epic, so this package is shaped like Kindle's K2. Kindle and EPUB share the hand-over of the arranged pages to `adoc-ebook` (`to_book`) and the write to a temporary file renamed over the output through the job's commit (`write_book_file`).
- **The commands:** `export.epub_page` and `export.epub_component`, with `EpubExportArgs`, and `Engine::export_epub_blocking`. The output must be an `.epub` file (any letter case), not a folder or a link.
- **Read-only pages are refused:** an EPUB carries no editing restriction, so `ExportFormat::Epub` refuses a page asking for one before anything is read, as Kindle and HTML do.
- **The modification time:**
  - It is chosen in this order: the arguments' `modified` (a profile's, from E7), then `SOURCE_DATE_EPOCH`, then the time of the export.
  - The time of the export is reported as an information note, not a warning, since an interactive export is not wrong for being unrepeatable. So `--fail-on warning` does not fail on it.
  - A time outside 1980 to 2107 fails the export.
- **No settings yet:** E7 adds `[epub]` and its profiles, so this package takes metadata from the arguments only.
- **Tests:**
  - In `crates/adoc-engine/tests/epub_export.rs`: `epub_and_pdf_select_the_same_pages` and `epub_failed_export_keeps_existing_output`, with `epub_outputs_must_be_epub_files` and `epub_refuses_a_read_only_page` beside them.
  - `epub_builds_alone` in `xtask`.
  - A unit test of the time's order.


### E3 (#640): semantic XHTML

- **Sharing with `to_html` (§14):** a separate writer, `adoc-epub`'s `xhtml.rs`, from the shared `Book`. `to_html` is untouched.
- **Per-format choices in the shared mapping:** `adoc_ebook::Options::format` (`Format::Kindle`, the default, or `Epub`). For EPUB:
  - a table inside a table's cell stays a table, which Kindle turns into lines of text with a warning;
  - keys, buttons and menus are `Inline::Styled`, written as `kbd` and `span class="button"`/`"menu"`, which Kindle writes as code and strong text;
  - a diagnostic names the format it is about (`Format::book`).

  `write_epub` always makes the book for EPUB.
- **Distinctions every format keeps:** the model gained `AsideKind` on `Block::Aside` (each admonition kind, a sidebar or an example) and `footer` on table rows. Kindle's writers ignore both, and Kindle's AZW3 and KFX bytes were compared again and are unchanged.
- **The markup:**
  - Admonitions, sidebars and examples are a `div` with a class (`admonition note`, `sidebar`, `example`), not an `aside`, since some reading systems hide an `aside` from the text as a note.
  - Footnotes are `aside epub:type="footnote"` in a `section epub:type="footnotes"` at the chapter's end, with `noteref` marks.
  - A page break is `hr class="page-break"`.
  - No link is ever written inside a link.
- **Passthrough (§14):** shown as text, with a located warning, never as markup. This is the same choice as Kindle (K5) and Word (D3). An unsupported block is its source, with a located warning.
- **Not yet carried:**
  - roles, a block's `lang` and a table's column alignment, which the shared model does not hold;
  - a table caption, which is a `caption` paragraph before the table, not a `caption` element.

  A later package can add them to the model, an option at a time.
- **Links within and between chapters:** written here (`#anchor` in the chapter, `c<n>.xhtml#anchor` across chapters). E4 tests them over a book.
- **Pictures:** written as their description until E5 embeds them.


### E4 (#641): navigation and links

- **The navigation document** holds:
  - exactly one `toc` navigation, nested as the plan is: parts, then their pages and navigation headings, then each page's sections two levels deep;
  - hidden landmarks, `toc` and `bodymatter`, with the cover added by E5.

  No NCX is written.
- **Links:**
  - In a chapter, a reference is a fragment. Across chapters it is `c<n>.xhtml#…`, since the chapters share a folder. The navigation document's links are `text/c<n>.xhtml#…`.
  - A reference to a page outside the book uses the page's published address (`page_urls`, which the engine fills from the site's URL) when there is one. Otherwise it is text with a located warning, as is a reference to an id the book does not define.
  - Ids are the shared model's (§15, "Before E1"), so two pages defining one id keep their own. A page reached through an alias or from another module is linked like any other.
- **Splitting large pages (§14):** never, in this release. One page is one content document, so no link or spine entry depends on a rule for where a page breaks.
- **`testing::assert_links_resolve`** checks every link as a reading system follows it, and becomes part of E9's validator:
  - a link inside the book must lead to a document of the manifest and, with a fragment, to an element of that document with that id;
  - any other link must be an address of `testing::SCHEMES`;
  - nothing names `file:` or `antora:`.
- **Tests:**
  - `epub_nav_matches_the_plan` (in `adoc-epub`): parts, headings and pages at three depths, with a page's sections.
  - `epub_internal_links_resolve` (in the engine, over the Word and Kindle reference fixtures with a page alias and a missing id added): links across modules, through an alias, to an id two pages define, out of the book, and to a missing id.


### E5 (#642): pictures, diagrams, formulas and the cover

- **Pictures** come from the shared model's `media.rs`:
  - read through the export's `FileAccess`, recognised by their bytes, checked whole and within limits;
  - kept once however often shown;
  - a remote picture is never fetched.

  Each is written once as `EPUB/images/i<n>.<extension>`, with its media type in the manifest. In a chapter it is an `img` with its description as `alt`, in a `figure` for a block picture. No `width` or `height` is written, so the reading system sizes it to the screen.
- **SVG (§14):** rasterised by `resvg`, at twice its size on white, as Word and Kindle do. Nothing is loaded from outside the file, and no SVG is packed, so no script, external reference or entity can reach the book. Sanitised SVG is not offered: a rasterised drawing loses nothing a reader needs, and the PNG is safe by construction.
- **Diagrams drawn by external tools (§14):** §7 covers the EPUB writing alone, as Kindle's K6 settled. Diagrams are embedded as the export's diagram workers drew them, Graphviz, PlantUML and D2 from tools on `PATH` among them, as for PDF, Word and Kindle. One not drawn is its source, with a warning.
- **MathML (§14):** not in the first release. A formula is the drawing the STEM provider made, with its source as the description. With no drawing (a build without Typst) it is its source as code, with a warning.
- **The cover:**
  - It is the export's own, else the first page's `front-cover-image`.
  - It is listed with `properties="cover-image"`, shown alone in `EPUB/cover.xhtml`, first in the spine, with a `cover` landmark.
  - `title-logo-image` is not used, since EPUB has no title page.
- **Message wording:** the shared mapping's message for a file that is not a picture names the format's reader (`Format::reader`). Kindle's wording, "a Kindle", is unchanged, and Kindle's bytes were compared again.
- **`testing::assert_links_resolve`** also checks that every `img` is an image of the manifest and has an `alt`.
- **Tests:**
  - `epub_media_is_embedded`: a cover, a PNG shown twice and held once, a JPEG, a diagram, and display and inline formulas.
  - `epub_bad_assets_are_reported`: an oversized, a truncated, a missing, a remote and an outside picture, text named as a PNG, and an SVG with a script and a `file:` link.

## 16. References

- W3C, [EPUB 3.3](https://www.w3.org/TR/epub-33/) (Recommendation)
- W3C, [EPUB 3.4](https://www.w3.org/TR/epub-34/) (a later follow-up)
