# PRD: AdocFoundry — DOCX Export

| Field | Value |
| --- | --- |
| Status | Approved for implementation, 2026-11-16 (epic #613). The open decisions in §13 are settled in the work package they affect, and recorded in §14 |
| Date | 2026-11-16 |
| Scope | A native publication backend that writes editable Microsoft Word `.docx` files for a page, an Antora component version or selected navigation files, through the engine, CLI, desktop app and MCP server |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Related work | Epic #613; work packages #618 to #625 (§11) |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `AGENTS.md`; `docs/prd/PRD-AdocFoundry_Phase_4_Implementation_Specification.md` (PDF export); `cargo xtask layers` |
| Existing code | `crates/adoc-engine/src/export.rs`, `crates/adoc-engine/src/profiles.rs`; `crates/adoc-render/src/view.rs`; `crates/adoc-pdf` (reference behaviour only) |

## 1. Purpose and problem

AdocFoundry publishes a page or an Antora book as PDF. A PDF is a finished artefact: a reviewer, translator or customer who works in Microsoft Word cannot edit it, track changes in it or lift its tables. Authors asked for a Word document today convert through HTML or PDF, which loses heading styles, lists, tables, footnotes and running content.

Add a native DOCX publication backend, comparable in coverage to the PDF exporter, that produces **editable** `.docx` files: a single AsciiDoc page as an article, or an Antora component version or selected navigation files as a book, with embedded images and diagrams, real Word heading and paragraph styles, page layout, covers, running headers and footers, page numbers and a table of contents.

This is publication. It is not a live preview, DOCX import or bidirectional editing. The exporter writes WordprocessingML from the same engine-built view data the PDF exporter uses; it never converts through PDF, Typst or HTML pasted into Word.

## 2. Goals

1. Export one page to an article and one component version or navigation selection to a single book, with the same content selection, order and deduplication as the PDF exporter.
2. Produce content Word can edit: paragraphs, runs, numbering, tables, headings, footnotes and fields, never page-sized pictures.
3. Make every document self-contained: images, diagrams, formulas, cover artwork and logos are embedded in the package.
4. Provide genuine Word page design: `styles.xml`, sections, header and footer parts, live `PAGE` and `NUMPAGES` fields, a title page and a table of contents built from heading styles.
5. Make publication repeatable through `[docx]` settings and `[docx.profiles.<name>]`, independent of `[pdf]`.
6. Offer the same export through the desktop app, the CLI and a write-gated MCP tool, all through `adoc-engine`.
7. Keep DOCX independent of Typst, so a build with DOCX and without PDF compiles neither `adoc-pdf` nor Typst, and `adoc-ls` stays as light as it is.
8. Never lose content silently: an unsupported node, a missing image or an unexported cross-reference target gives a diagnostic with its source location.

## 3. Non-goals

- The legacy `.doc` binary format, `.docx` import, Word editor integration, a DOCX live preview, tracked changes, macros or VBA, SharePoint or other cloud integration, and automated publishing to Microsoft 365.
- Page layout that matches the PDF. Word reflows with the fonts, platform and printer settings of whoever opens the document, so page breaks, page count and typography differ. Parity means feature intent, not pixel identity.
- Arbitrary `.dotx` templates and automatic translation of asciidoctor-pdf theme YAML. Both are deferred to a separate design if needed.
- Editable Office Math (OMML) equations, sophisticated cross-document field updates, a full indexed bibliography and an accessibility certification equivalent to PDF/UA. Sensible alt text and heading semantics are kept from the first release.
- Requiring Microsoft Word, LibreOffice or any other tool to be installed to export.

## 4. User workflows

**Export a page.** With a page open, choose Export Page to Word…, pick a file, and receive `<name>.docx`. Diagnostics appear as they do for a PDF export.

**Export a book.** Choose Export Component to Word… for a component version; the book follows the effective navigation order, with parts and chapters as Word headings and a table of contents.

**Publish a profile.** Define `[docx.profiles.manual]` once, then run `adocfoundry-cli docx --profile manual` locally or in CI, with `--fail-on warning` to gate on diagnostics.

**An agent exports.** With `--allow-write`, an MCP client calls `export_docx` for a page, component or profile, under the same root confinement as `export_pdf`.

**The recipient edits.** The document opens in Word or LibreOffice Writer without a repair prompt. Text, tables and lists are editable; images persist offline; updating fields refreshes the table of contents and page numbers.

## 5. Functional requirements

### 5.1 Output and content selection

- Export **one page** to `<name>.docx` and **one Antora component version or selected navigation tree** to a single book in effective navigation order. Match the PDF exporter's deduplication of pages, treatment of the root and start page, chapter and part headings, component and version scope and authoring attributes. Partials arrive through the existing parse and render pipeline.
- Preserve the page title, document title, authors, revision and description where available; set the matching Word core properties and an optional title or cover page. Respect document-level section breaks and explicit page breaks.
- Use paragraphs, runs, numbering, tables, headings and fields. Only diagrams, equations and unavoidable special graphics may be rasterised.

### 5.2 Rich Word content

- Headings with real Word heading styles; paragraphs with bold, italic, underline where available, monospace, inline code, marks, superscript and subscript, hard line breaks, keyboard, menu and button markup, links, anchors as bookmarks, internal xrefs and external hyperlinks.
- Bullet, numbered, nested, check, description and callout lists; quote, sidebar, example, open and admonition blocks; source and literal blocks with whitespace kept and optional token colouring; callouts, captions, horizontal rules and explicit page breaks.
- Real Word tables with header rows, column widths, cell alignment, colspan and rowspan; block content in cells and nested tables handled conservatively.
- Word footnotes for `ViewFootnote` and `ViewInline::FootnoteRef`, with correct behaviour for a footnote referenced more than once, or a documented limitation with a visible fallback and a diagnostic.
- Labels and cross-references to pages and anchors inside the publication are preserved as bookmarks and internal hyperlinks. A target outside the publication uses its published URL when one is configured, otherwise readable text and a diagnostic; a broken internal relationship is never written.
- An unsupported node gives a warning with its `SourceSpan` rather than disappearing.

### 5.3 Images, diagrams and mathematics

- **Embed** PNG, JPEG and other supported raster images in `word/media/` with correct OPC content types and part relationships; no dependency on a file path remains after export. Preserve aspect ratio, sizing (in EMUs), alignment, captions, alt text and link targets where Word supports them.
- Embed generated diagrams (`DiagramView::Svg`) as Word-compatible images, with a tested raster fallback at a sensible resolution. Interoperability between Word and LibreOffice matters more than assuming every reader renders SVG.
- Inline and block STEM appears as the already-rendered formula artwork, with a sensible baseline and size and a readable description. A formula is never dropped silently; failed or pending artwork gives a warning with its source span.
- Identify images by file signature, not extension. Cap raster dimensions and memory, and deduplicate identical media where useful.
- Use the engine's existing image and font access rules. Report missing, unreadable, oversize and unsupported images with source locations. No image is ever downloaded implicitly.
- The document body, each header and each footer have their own relationship parts; cover and logo images are embedded through the part that shows them.

### 5.4 Page design, headers and footers

- A consistent default publication style in `styles.xml` (Normal, Title, Heading 1–N, code, table, caption, admonition and the rest), paper size (A4, Letter), orientation, margins, header and footer offsets and typography. Controlled overrides come from `[docx]` and DOCX profiles; a PDF theme's YAML is not expected to transfer.
- Genuine Word header and footer parts (`word/headerN.xml`, `word/footerN.xml`) referenced from section properties, with configurable first-page and odd and even variants, left, centre and right content, text, optional logos, the document title and chapter title where feasible, and **dynamic `PAGE` and `NUMPAGES` fields**, never static page-number text.
- A title page or front-cover image and, when configured, a back cover; no header or footer on a title page unless configured.
- A Word table of contents built from heading styles and anchors. A field-based table of contents may need Word to refresh its page numbers on opening; that behaviour is documented and tested, and accurate cached page numbers are not promised before the update.

### 5.5 Configuration

- `[docx]` holds defaults: page size, orientation and margins, the built-in style and its overrides (named paragraph and run styles, colours, fonts, spacing), the title page, the table of contents and running content with its first, odd and even variants.
- `[docx.profiles.<name>]` holds a publication: `page`, `component` or `nav` selection, `output`, `attributes` and Word-specific options. It reuses `ProfileContent` and the target and output-path resolution of `crates/adoc-engine/src/profiles.rs`.
- Nothing Word-specific goes in `[pdf]`, and `pdf-theme` and the PDF standards (PDF/A, PDF/UA) are never reinterpreted for DOCX. Existing `[pdf.profiles.<name>]` tables keep their meaning.
- Invalid profiles, paths that leave the workspace, conflicting output extensions and unknown settings are refused or warned about in the engine's established style.
- The user guide carries an example that publishes one Antora book as both PDF and DOCX, with independent output paths and style settings.

### 5.6 Front ends

- **CLI:** `adocfoundry-cli docx <file|component-dir> [--root <dir>] [--component [<version>@]<name>] --output <file.docx>`, `docx --profile <name> [<root>]`, and `--fail-on none|warning|error`. Target selection, diagnostics and exit codes mirror `pdf`, with no PDF-only flag.
- **Desktop:** Export Page to Word…, Export Component to Word… and Export DOCX Profile… in the menu and command palette, reusing the save dialogs, progress and completion notifications and diagnostics of the PDF exports. No DOCX preview pane.
- **MCP:** `export_docx`, taking a page, a component or a profile, listed and callable only with `--allow-write`, with the root confinement, link refusal, existing-output checks and `.docx` extension check that `export_pdf` applies.
- Every front end calls `adoc-engine` and produces the same content and warnings; none reaches `adoc-docx` directly. No export runs an external tool or opens Word.

## 6. Existing code to reuse

| Code | What it gives | How DOCX uses it |
| --- | --- | --- |
| `crates/adoc-engine/src/export.rs` | `ExportTarget`, the article and book plan (`PlanItem`), navigation-ordered book assembly, parsing from unsaved buffers or disk, diagram preparation, worker orchestration, `ExportSummary`, `ExportDiagnostic` and the file-access rules | Most of the file is gated by `feature = "typst"` today. Extract only the format-neutral planning and preparation, so DOCX does not need Typst; PDF semantics and tests are preserved |
| `crates/adoc-render/src/view.rs` | `ViewTree`, blocks, inlines, `ViewImage`, `DiagramView`, footnotes, attributes, resolved xrefs and source spans | The exporter's only input. No second AsciiDoc or Antora parser or resolver is introduced |
| `crates/adoc-pdf` (`document.rs`, `emit.rs`, `running.rs`) | Title and book structure, images, running content, document facts, bibliography, footnotes, links and page sizing | Reference behaviour only. `adoc-docx` neither depends on `adoc-pdf` nor emits Typst |
| `crates/adoc-engine/src/profiles.rs` | `[pdf.profiles.<name>]` selection (`ProfileContent`: `page`, `component`, `nav`) and output-path safety | The selection model and path rules are shared; PDF-specific options stay with PDF |
| `crates/adocfoundry-cli/src/main.rs`, `crates/adocfoundry/src/workspace_view.rs`, `crates/adoc-mcp/src/server.rs` | The PDF entry points | DOCX entry points sit beside them |

## 7. Architecture

```text
L4   adocfoundry / adocfoundry-cli / adoc-mcp
                  |  format-specific commands and presentation
L3   adoc-engine  |  shared publication plan; parse + view + diagrams;
                  |  worker lifecycle; safe output; settings and profiles
L2   adoc-docx    |  Word styles, WordprocessingML, OPC/ZIP package,
                  |  media, relationships, sections, headers and footers
L2   adoc-render  |  shared ViewTree / ViewImage / ViewInline / ViewKind
L1   existing parser, DOM, Antora, index and language crates
```

| Crate | Responsibility |
| --- | --- |
| `adoc-docx` (L2, new) | Turn a `ViewTree` and typed options into a DOCX package: styles, document body, numbering, footnotes, sections, header and footer parts, media, relationships, content types and core properties, with typed diagnostics carrying source locations. Headless; no engine, worker, UI or LSP types |
| `adoc-render` (L2) | Unchanged. The view tree stays the one description of a rendered document |
| `adoc-engine` (L3) | The format-neutral publication plan and preparation extracted from `export.rs`; the optional `docx` feature; DOCX commands, arguments and events; `[docx]` settings and profiles; worker lifecycle; safe output |
| `adocfoundry`, `adocfoundry-cli`, `adoc-mcp` (L4) | The entry points of §5.6 |

- `adoc-docx` sits at **L2**. It may depend on `adoc-render` through a new pair in `SAME_LAYER_ALLOWED` in `xtask/src/layers.rs`, as `adoc-pdf` does, and on lower layers. It must not depend on `adoc-pdf`, Typst, GPUI or `adoc-engine`.
- The crate needs an entry in `LAYERS`, an entry in `[workspace.dependencies]`, and a mention in `AGENTS.md`, `README.md`, the design document and the developer documentation's crate table and diagram (`architecture.adoc`, `crate-layers.mmd`), which `every_crate_is_documented` in `xtask` checks.
- The engine's `docx` feature is opt-in and **independent of `typst`**. It is off in `[workspace.dependencies]`, so each front end names it; `adoc-ls` does not, and `cargo xtask layers` keeps checking that its default build compiles neither. Release builds of the app, CLI and MCP server include both PDF and DOCX.
- The engine remains the only entry point front ends use. The publication plan is extracted from the PDF-gated code, never copied into a second planner.
- All new dependencies are pinned exactly and pass `cargo deny check`.

## 8. Headless contracts

Names are illustrative; DOCX 1/8 and 2/8 settle them and record the outcome in §14.

- `adoc-docx`: a `DocxDocument` built from one or more view trees and document facts, `DocxOptions` (style, layout, running content, table of contents, covers), and a result carrying the diagnostics, written to an output sink suited to bounded packaging.
- `adoc-engine`: DOCX export commands beside `export.pdf_page` and `export.pdf_component`, with arguments that hold no PDF-only field (either format-specific arguments, or a common request typed by format); a blocking export for the CLI as `export_blocking` is for PDF; completion reported as an `EngineEvent`, consistent with `ExportFinished`.
- Diagnostics follow the existing `ExportDiagnostic` model, with severities that allow CI gating.

## 9. Output safety, performance and reliability

- A finished export is written to a temporary sibling file and renamed into place only after packaging succeeds. A failure leaves the source and any existing output intact.
- Link targets and other unexpected output types are refused. MCP and profile outputs are confined to the workspace roots, as for PDF.
- Export runs on workers with the existing lifecycle, cancellation and timeouts; nothing renders on the UI thread. A large book is processed with bounded memory.
- The same input gives the same package bytes where practicable (stable part order, no timestamps beyond the document's own facts), so outputs can be compared in tests.
- Every document string is escaped as XML text or an attribute value; control characters that XML 1.0 forbids are normalised. A document can never inject markup into a part.
- No network access, no external tool and no Word runtime, at export or in tests.

## 10. Tests and acceptance criteria

Each test below exists under its name when its work package is done.

| # | Test | Package | What it shows |
| --- | --- | --- | --- |
| 1 | `docx_package_is_valid_opc` | D1 | `[Content_Types].xml`, `_rels/.rels`, `word/document.xml`, its relationships, `styles.xml` and core properties are present, well formed and consistent; every relationship target exists |
| 2 | `docx_text_is_escaped` | D1 | Unicode, control characters and XML metacharacters are escaped or normalised; no input panics |
| 3 | `docx_builds_without_typst` | D1, D2 | A build with `docx` and without `typst` compiles neither `adoc-pdf` nor Typst; `cargo xtask layers` passes |
| 4 | `docx_and_pdf_select_the_same_pages` | D2 | For an article, a component version and a navigation selection, both formats take the same pages in the same order, unsaved buffers included |
| 5 | `docx_failed_export_keeps_existing_output` | D2 | An invalid target, a link as the output, a refused write and a failed packaging leave the source and existing file intact, with the usual diagnostics |
| 6 | `docx_rich_content_uses_word_structures` | D3 | Headings, nested lists, tables with merged cells, footnotes, code and special characters are real Word styles, numbering and table elements, not flat text |
| 7 | `docx_internal_links_reach_bookmarks` | D3 | An xref inside the publication is an internal hyperlink to an existing bookmark; one outside is a URL or text with a diagnostic |
| 8 | `docx_unsupported_nodes_warn` | D3 | An unsupported construct gives a warning with its source span and a visible fallback |
| 9 | `docx_media_is_embedded` | D4 | PNG, JPEG, an SVG diagram and inline and block formulas are in `word/media/` with content types, alt text and no dangling `r:embed`; removing the source files changes nothing |
| 10 | `docx_bad_images_are_reported` | D4 | Missing, unreadable, oversize, mislabelled and remote images give located diagnostics; nothing is fetched |
| 11 | `docx_running_content_has_live_fields` | D5 | Header and footer parts, their own relationships and media, first, odd and even variants, and `PAGE`, `NUMPAGES` and `TOC` fields are present and referenced from the section properties |
| 12 | `docx_covers_and_sections` | D5 | Title page, front and back cover images, page size, orientation and margins are written to the right sections; the title page has no running content unless configured |
| 13 | `docx_profiles_resolve_and_validate` | D6 | Page, component and navigation profiles resolve to the right pages and output; invalid profiles, escaping paths and unknown settings are refused |
| 14 | `docx_settings_leave_pdf_unchanged` | D6 | PDF profile tests, default file names and settings precedence are as before |
| 15 | `docx_cli_exports_and_gates` | D7 | The CLI writes an article, a book and a profile, with exit codes that follow `--fail-on` |
| 16 | `docx_mcp_export_is_write_gated` | D7 | `export_docx` is listed only with `--allow-write`, and refuses paths outside the roots, link outputs and unsafe overwrites |
| 17 | `docx_app_exports_through_engine` | D7 | The desktop actions invoke the engine's commands and report completion and diagnostics; no preview is opened |
| 18 | `docx_reference_book_is_complete` | D8 | A fixture book with several components and versions, nested partials and every construct above passes every structural check, deterministically |
| 19 | `docx_large_book_is_bounded` | D8 | A large book exports within a memory and time budget, off the UI thread |

Manual interoperability checks that tests cannot make (opening, editing and saving in Microsoft Word and LibreOffice Writer, no repair prompt, field refresh) are recorded in `docs/qa/docx.md` by D8.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check`.

## 11. Work packages

| WP | Deliverable | Issue |
| --- | --- | --- |
| D1 | New `adoc-docx` crate, the OOXML library spike and a valid minimal WordprocessingML package | #618 (DOCX 1/8) |
| D2 | Format-neutral publication plan extracted from the PDF-gated export code; the engine's `docx` feature and export orchestration | #619 (DOCX 2/8) |
| D3 | Rich content as editable Word styles and structures | #620 (DOCX 3/8) |
| D4 | Embedded images, diagrams and STEM with valid relationships | #621 (DOCX 4/8) |
| D5 | Headers, footers, covers, page sections and the table of contents | #622 (DOCX 5/8) |
| D6 | Document styles, `[docx]` settings and named publication profiles | #623 (DOCX 6/8) |
| D7 | CLI, desktop and MCP entry points | #624 (DOCX 7/8) |
| D8 | Interoperability, performance, accessibility and documentation QA | #625 (DOCX 8/8) |

Order: D1 and D2 first (D2's extraction may begin beside D1); D3 to D6 on their contracts, D4 coordinating with D5 for header, footer and cover artwork and D6 with D5 for configuration; D7 once an export works end to end; D8 verifies and documents the release.

## 12. Definition of done

1. A single-page article and a version-aware Antora book over several components export to valid, **editable** DOCX with no Word runtime and no PDF or Typst intermediate.
2. A realistic fixture holding embedded PNG and JPEG images, a generated SVG diagram, formulas, tables, lists, source blocks, footnotes, hyperlinks, covers and a table of contents opens in Microsoft Word and LibreOffice without a repair prompt, and its content stays readable and editable.
3. Genuine Word headers and footers hold the configured text and logos and live page-number fields, with correct section, first, odd and even behaviour; how the table of contents refreshes is documented.
4. The same article or book can be exported through the app, the CLI and the write-gated MCP tool, over shared engine logic. Existing PDF exports are unchanged.
5. The automated structure, escaping, image-relationship, cross-reference, output-safety, path-confinement, determinism and large-book tests pass in CI, and the manual interoperability QA is recorded.

## 13. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's pull request and in §14.

- **OOXML library (D1).** `docx-rs` is a candidate, not a mandate. The spike checks complex tables, footnotes, the table of contents, sections, field codes, image relationships in headers and footers, build size and licences. The alternative is narrowly scoped in-house WordprocessingML; `quick-xml` is already in the dependency tree, a ZIP writer is not. *Settled in D1: in-house (§14).*
- **Request shape (D2).** Format-specific DOCX arguments, command ids and events, or one common request typed by format with no PDF-only field reachable from DOCX.
- **Where SVG is rasterised (D4).** The app's `image_decode::svg_to_png` is at L4 and cannot be called from `adoc-docx`. Either `adoc-docx` rasterises (`resvg` at the version already in the tree), or the engine hands it prepared rasters.
- **Formulas without Typst (D4).** The STEM provider is part of the engine's `typst` feature. A build with `docx` and without `typst` has no formula artwork; decide between the formula's source as text with a diagnostic, and a narrower feature that provides the artwork alone.
- **Repeated footnote references (D3).** A real Word footnote per reference, or one footnote and cross-references to it.
- **Table of contents (D5).** A `TOC` field alone, refreshed on opening, or a field with entries written in advance so the document reads correctly before any refresh.
- **Chapter titles in running content (D5).** `STYLEREF` fields, or static text per section.
- **Profile sharing (D6).** Whether a publication's content selection is declared once and named by both `[pdf.profiles]` and `[docx.profiles]`, or repeated in each.

## 14. Implementation notes

Each work package records here where the implementation departs from this document and how the open decisions of §13 were settled.

### D1 (#618): the crate and its package

- **OOXML library: in-house.** The spike built a document with `docx-rs` 0.4.22 (MIT, July 2026; its only new crate would have been `zip` 8) holding a heading, a footnote, a header with a `PAGE` field, a table of contents and merged cells. It writes all of them, but:
  - It numbers paragraphs, footnotes, bookmarks, headers, footers and pictures from process-wide counters (`static PIC_ID: AtomicUsize` and nine more). Two exports of the same document in one process gave different bytes, so in the app an export's bytes would depend on what was exported before, against §9, and the counters are global mutable state.
  - It wrote U+0001 and U+FFFE straight into `word/document.xml`, which is then not well-formed XML (§9); every string would have to be cleaned before it reached the library.

  `adoc-docx` therefore writes the package itself, with no new dependency: `flate2` and `crc32fast` were already in the tree. Every document string goes through one writer, `xml.rs`. It escapes `&`, `<` and `>`, and `"` in attributes. Carriage returns, and tabs and line feeds in attributes, are written as character references. The characters XML 1.0 forbids are replaced by U+FFFD. `zip.rs` writes the ZIP in order, with no seeking, no data descriptors and every entry dated 1 January 1980, and refuses a package past 65,535 entries or 4 GiB (no ZIP64; `DocxError::TooLarge`). `package.rs` writes `[Content_Types].xml` (a `Default` for `rels` and `xml`, an `Override` per part) and each part's relationships in a fixed order. `quick-xml` is not used: writing needs none of it, and the tests read the XML back with `roxmltree`, already a workspace dependency.
- **Contracts.**
  - `write_docx(&DocxDocument, &DocxOptions, impl Write) -> Result<DocxReport, DocxError>`.
  - `DocxDocument` is `Article(DocxPage)` or `Book(DocxBook)`, the shape of `adoc-pdf`'s `Document`, `PageView` and `Book`, without depending on it.
  - `DocxOptions` holds only the paper for now (A4 or Letter).
  - `DocxDiagnostic` has a severity, a message and an optional `SourceSpan`.
  - The sink takes the package a part at a time. Whoever owns the file writes to a temporary sibling and renames it (D2).
- **Body.**
  - Sections and discrete headings take Word's built-in `heading 1`–`heading 9` styles, with outline levels; the document title takes Title, and a book's subtitle Subtitle.
  - In a book, a page's title is a heading at its depth and its sections move down by it. Where the book has parts, a part is Heading 1 on a new page and everything else moves down one.
  - Paragraphs are text with tabs and line breaks; thematic and page breaks are written.
  - Every other block is, until D3 to D5, written as plain paragraphs of its text with a warning at its source span. So are the footnotes, at the end of their page, while a table of contents block only warns.
  - The core properties take the title and the `author` (or `authors`), `description` and `keywords` attributes, and no date.
- **Checked by hand.** LibreOffice 26 opened the samples `ADOC_DOCX_OUT=<dir> cargo test -p adoc-docx --test package -- --ignored` writes, and read their headings as headings. Microsoft Word is checked in D8.

## 15. References

- Microsoft, [Structure of a WordprocessingML document](https://learn.microsoft.com/en-us/office/open-xml/word/structure-of-a-wordprocessingml-document)
- [`docx-rs`](https://docs.rs/docx-rs/latest/docx_rs/), the library candidate of §13
