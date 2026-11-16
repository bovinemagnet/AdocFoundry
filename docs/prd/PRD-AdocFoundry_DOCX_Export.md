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
- **Request shape (D2).** Format-specific DOCX arguments, command ids and events, or one common request typed by format with no PDF-only field reachable from DOCX. *Settled in D2: format-specific arguments and command ids, one shared event (§14).*
- **Where SVG is rasterised (D4).** The app's `image_decode::svg_to_png` is at L4 and cannot be called from `adoc-docx`. Either `adoc-docx` rasterises (`resvg` at the version already in the tree), or the engine hands it prepared rasters. *Settled in D4: `adoc-docx` rasterises with `resvg` (§14).*
- **Formulas without Typst (D4).** The STEM provider is part of the engine's `typst` feature. A build with `docx` and without `typst` has no formula artwork; decide between the formula's source as text with a diagnostic, and a narrower feature that provides the artwork alone. *Settled in D4: the source as text, with a warning (§14).*
- **Repeated footnote references (D3).** A real Word footnote per reference, or one footnote and cross-references to it. *Settled in D3: one footnote, and `NOTEREF` fields to it (§14).*
- **Table of contents (D5).** A `TOC` field alone, refreshed on opening, or a field with entries written in advance so the document reads correctly before any refresh. *Settled in D5: both, a field with its entries written, and Word asked to update fields on opening (§14).*
- **Chapter titles in running content (D5).** `STYLEREF` fields, or static text per section. *Settled in D5: `STYLEREF` (§14).*
- **Profile sharing (D6).** Whether a publication's content selection is declared once and named by both `[pdf.profiles]` and `[docx.profiles]`, or repeated in each. *Settled in D6: repeated in each (§14).*

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

### D2 (#619): one pipeline for both formats

- **Request shape: format-specific arguments, one event.**
  - DOCX has its own `DocxExportArgs` (target, output, attributes; no theme, standard, fonts folder or Typst), its own `CommandArgs::ExportDocx` and its own commands, `export.docx_page` and `export.docx_component` ("Export Page to Word…", "Export Component to Word…"), registered only with the engine's `docx` feature.
  - Both formats report with the existing `EngineEvent::ExportFinished`. An event names the output written, whatever its format, and front ends already handle it.
  - `ExportSummary` gains `sources`, the AsciiDoc pages exported in order. For DOCX, `pages` is the number of those pages, since Word lays out its own pages when it opens the document.
- **The shared pipeline.**
  - `export.rs`'s plan (`Plan`: the article, or the book from the component version's or the given navigation files, the start page as a preface, each page once), the parsing of open buffers or files on a worker, the views and the wait for their diagrams now run for either format.
  - A job holds a `Request`, PDF or DOCX. `arrange` puts the views in the plan's order once (`Arranged`), and only then does `to_pdf` or `to_docx` make the format's document. The diagnostics for unread pages and failed diagrams are worked out from the arranged views for both.
  - The pipeline is the engine's `export` feature, which `typst` and `docx` each turn on. Front ends never name it, and turning it on alone is a compile error. Theme, typesetting, the PDF preview and `export.pdf_pages` stay under `typst`.
  - `pdf-title`, which the book plan read, is now applied for PDF only. DOCX reads no `pdf-` attribute; its book takes the component version's title.
- **Without Typst.** `adoc-engine --no-default-features --features docx` builds `adoc-docx` and neither `adoc-pdf` nor Typst. `cargo xtask layers` checks the dependency tree (`docx_builds_without_typst`), and CI's lint job compiles that build. Formulas have no artwork without Typst; D4 decides what DOCX shows instead.
- **Safe output.**
  - A DOCX output must be a `.docx` file name. A folder or a link there is refused before anything is read, and checked again when writing.
  - The package is written to a temporary file beside the output (created, with its folders, as for PDF) and renamed over it only once complete. A failure leaves the source and whatever was at the output, and no temporary file.
  - PDF's own writing is unchanged.
- **Options.** The DOCX export writes with `DocxOptions::default()` (A4) until D6 adds `[docx]`.
- **Tests.** `crates/adoc-engine/tests/docx_export.rs`:
  - `docx_and_pdf_select_the_same_pages`: an open page with unsaved text, the component version, a file and two navigation files in the order given, each exported to both formats; the `sources` agree, and the DOCX holds the unsaved text and the pages in that order.
  - `docx_failed_export_keeps_existing_output`: an unknown component, a non-`.docx` output, a folder, a link and a folder that refuses the write, each leaving what was there.
  - An article, a book and the command's `ExportFinished`.

### D3 (#620): rich content as Word structures

The body is now written by `body.rs` (blocks), `inline.rs` (runs, links, footnotes), `table.rs`, `numbering.rs` and `labels.rs`, with the parts `numbering.xml`, `footnotes.xml` (its separators first) and `settings.xml` (compatibility mode 15, the footnotes' separators).

- **Repeated footnotes: one footnote, and fields to it.** A footnote's first reference is a Word footnote, with a bookmark (`_f<n>`) around its mark. A later reference to it (`ViewInline::FootnoteRef { repeat: true }`) is a `NOTEREF _f<n> \f \h` field, which Word shows as the same number in the footnote reference style, linked to the footnote. Its cached result is that number, so it reads correctly before any field update. A reference to a footnote the page does not have shows its number and is reported.
- **Bookmarks and links.**
  - Every place a link can lead is named before anything is written (`labels.rs`), so a link may lead forward: a page's start, every section and block id, and every inline anchor, as the PDF exporter's labels. Names are `_a1`, `_a2` and so on in document order: valid whatever the id, at most 40 characters, and hidden from Word's bookmark list by the underscore.
  - A cross reference is decided as the PDF exporter decides it. It leads to a bookmark of the document, else to the target page's published address (`DocxOptions::page_urls`, which the engine fills from `[pdf] site_url` or the playbook, as for PDF), else it stays text with a warning at its source.
  - An external link is a hyperlink relationship (`TargetMode="External"`) of the part it is in, so a link in a footnote is a relationship of `footnotes.xml`. Only `http`, `https`, `mailto`, `ftp`, `ftps`, `tel` and relative addresses are links. Anything else stays text with a warning, though the parser reads neither `javascript:` nor `file:` as a link anyway.
- **Inlines.**
  - Strong, emphasis, mark (yellow highlight), superscript and subscript are run properties.
  - The `underline` and `line-through` roles are run properties too.
  - Code is the character style `CodeChar` and keys `Keyboard`; buttons and menus are bold.
  - Hard line breaks are `w:br`, tabs `w:tab`.
- **Lists.**
  - Bullets and each ordered style (`arabic`, `loweralpha`, `upperalpha`, `lowerroman`, `upperroman`) have an abstract numbering of nine levels. Each list is an instance of its own, ordered ones overriding their level's start, so a list starts again at its `start`.
  - The level is the list's nesting, up to Word's ninth. An item's other blocks are indented under it.
  - In a checklist only the items written with a box show one (☐ or ☑, then a tab), as in Asciidoctor; the others are bullets.
  - `reversed` is numbered upwards, with a warning.
- **Other lists and blocks.**
  - A description list is its term in `DescriptionTerm` (bold) with the description indented below it. A callout list is `(n)` and a tab, the listing showing ` (n)` at its lines' ends.
  - Source and literal blocks are one paragraph each (`SourceCode`, `LiteralBlock`: Consolas, shaded), whitespace kept, lines as breaks. There is no token colouring yet.
  - Quotes are `Quote` paragraphs with a `QuoteAttribution`; verses keep their lines.
  - Block titles are `Caption` paragraphs above their block, with the numbered caption.
- **Tables and containers.**
  - An AsciiDoc table is a Word table (`AdocTable`, full width), its grid sized from `cols` and its header rows repeated (`tblHeader`).
  - A column span is `gridSpan`. A row span is `vMerge`, its continuations placed on the grid below it, so every row fills the grid. Cells take their alignment and their style (header and strong bold, emphasis italic, monospace and literal `CodeChar`); a short row is filled with empty cells, and cells past the grid are left out with a warning.
  - Blocks in cells are written as anywhere else, nested tables included. Every cell ends with a paragraph, and two tables are kept apart by a `TableSpacer` paragraph, since Word would join them.
  - `frame`, `grid` and `stripes` are not applied.
  - Admonitions (`AdocAdmonition`: a bar at the left, the label in a cell of its own, from `note-caption` and the like), sidebars (`AdocSidebar`, shaded) and examples (`AdocExample`, boxed) are tables of one row, so their blocks stay editable in place.
- **Breaks.** In a book, parts and chapters (pages and headings at depth 1) start a page, except right after a part.
- **Still text, reported.** Images, diagrams and formulas (D4), media, passthrough blocks and unsupported blocks are written as text: alt text, source or address. Each is reported with its source span. A table of contents block only warns (D5).
- **Checked by hand.** LibreOffice 26 converted the rich test page (`ADOC_DOCX_OUT=<dir> cargo test -p adoc-docx --test content -- --ignored`) to HTML. It kept the table's column span and row span, the nested bullet and numbered lists, the footnote and the containers.

### D4 (#621): pictures embedded

`media.rs` reads, checks and keeps the pictures; `drawing.rs` writes them as inline DrawingML (`wp:inline`, `pic:pic`, `a:blip r:embed`), each a relationship of the part it is in (the document or the footnotes) to its part in `word/media/`. Document and footnote parts declare the DrawingML namespaces.

- **SVG: drawn by `adoc-docx` with `resvg`.**
  - `adoc-docx` depends on `resvg` at the workspace's version (0.46.0, the app's, with text and system fonts), already in the tree; the engine's DOCX-only build still compiles no Typst.
  - An SVG, an SVG image file, a rendered diagram or a typeset formula is drawn as a PNG at twice its own size (at most 10,000 pixels on a side and 40 million in all), with transparency kept. The system's fonts are read once per export, on the first SVG, and nothing is loaded from outside the file, as the app's image viewer does.
  - Word since 2016 can show SVG with a PNG fallback (`asvg:svgBlip`). Writing only the PNG keeps every Word and LibreOffice alike, and needs no second copy.
- **Formulas without Typst: the source, and a warning.**
  - With the `typst` feature, block formulas arrive as rendered SVGs and inline ones as `FormulaImage`s with their measures in points. Both are embedded, and an inline formula's run is lowered (`w:position`) by its depth, so it sits on the line's baseline.
  - Without Typst, or where typesetting failed, the formula's source is written in `CodeChar` (inline) or `LiteralBlock` (block) with a warning at its source. A narrower feature for the artwork alone was not worth its cost.
  - Editable OMML equations remain out of scope (§3).
- **Reading and checking.**
  - Pictures are read through `DocxOptions::access`, the engine's `FileAccess`, so an MCP export reads only inside its roots. They are recognised by their bytes, never their extension: PNG, JPEG, GIF, or an SVG root element past any declaration, comments and doctype.
  - Each is checked whole: a PNG's header chunk and end chunk, a JPEG's frame and end marker, a GIF's trailer. The size is read from those headers without decoding.
  - Limits: files of at most 64 MB, and at most 10,000 pixels on a side and 40 million in all.
  - Anything else is written as its alternative text with a warning saying why:
    - missing;
    - refused by the file access;
    - unreadable;
    - not PNG, JPEG, GIF or SVG (WebP, HEIC, BMP and TIFF included; Word may refuse them, and converting them would need a decoder);
    - cut short or damaged;
    - too large.
  - A remote image is never fetched: it is written as a link to its address (when the address is a safe one) with a warning.
- **Media.**
  - Each picture is one part, `word/media/image<n>.<png|jpeg|gif>`, with its content type, kept once however often it is shown (identical bytes are one part).
  - The JPEG, PNG and GIF bytes are the files' own. Nothing in the package names the source files, so removing them changes nothing.
- **Sizes.**
  - In EMUs: `width` and `height` in pixels (`120`, `120px`) or as a share of the text width (`50%`), the other kept in proportion when only one is given, else the picture's own size at 96 pixels to the inch.
  - Never wider than the text: inside a table cell, the cell's width.
- **Figures.**
  - A block image, diagram or formula is a paragraph aligned by the block's `align` (formulas centred by default), its title the caption below it, aligned with it, as Asciidoctor places a figure's title.
  - A block image's `link` makes the picture a hyperlink (`a:hlinkClick`).
  - Each picture's `docPr` carries its alternative text as `descr`: the image's alt, a diagram's `alt` attribute or `diagram`, or a formula's source. Its id is unique in the document.
- **Diagrams.** A diagram still rendering is written as its source with a warning. One that failed is written as its source, the engine reporting the failure as it does for PDF.
- **Not yet.** Cover and logo images, and header and footer pictures, come with D5, through the same per-part relationships.
- **Checked by hand.** LibreOffice 26 rendered the sample (`ADOC_DOCX_OUT=<dir> cargo test -p adoc-docx --test media -- --ignored`), offline. It showed a PNG at its asked width, a centred JPEG with its caption, a GIF in a line, an SVG with its transparency and an inline formula on its line.

### D5 (#622): pages, running content and the table of contents

`pages.rs` writes the sections, covers, title page, header and footer parts and the table of contents; the writer's parts are now numbered (`Part`), each header and footer a part with relationships of its own.

- **Sections.**
  - A book is a front cover, a title page, the body and a back cover, each a section. The body starts with the table of contents, and its page numbers start at 1 (`pgNumType`).
  - An article is one section, with a title page before it when it sets `title-page`, and covers when it names them.
  - Every section has the page size (`DocxOptions::paper`: A4 or Letter; `landscape` turns it and sets `orient`) and the margins (`DocxOptions::margins`: the four sides, and the header and footer offsets).
  - A cover's section has no margins.
  - A section ends with a paragraph holding its properties; the last section's end the body.
- **Covers and the title page.**
  - `front-cover-image` and `back-cover-image` come from the first page's view, as the PDF exporter takes them, and fill their page, proportions kept.
  - The title page is centred: `title-logo-image`, the title (a book's, or an article's), a book's subtitle, then `author` (or `authors`), `Version {revnumber}` and `revdate`, each when set.
  - Covers and the title page show no running content. The first sections name no header or footer. The back cover names empty ones for every type, since in Word a section without its own takes the previous section's.
- **Running content.**
  - `DocxOptions::running` (`Running`) gives the header's and footer's lines (`Slots`: `odd`, optional `even` and `first`). Each line has left, centre and right slots (`Line`), placed by a centre and a right tab stop in the `Header` and `Footer` styles.
  - Slots hold text and the placeholders:
    - `{page-number}` and `{page-count}`: live `PAGE` and `NUMPAGES` fields. `NUMPAGES` counts every page, covers included, as Word does.
    - `{document-title}`.
    - `{chapter-title}`: a `STYLEREF` field on the chapter's heading style.
    - `{logo}`: `Running::logo`, drawn 32 pixels high, a relationship of its own header or footer part.

    An unknown placeholder stays as written.
  - With `odd_even`, `w:evenAndOddHeaders` is set. An even page's line is its own, or the odd page's mirrored, left and right swapped.
  - A `first` line in either the header or the footer gives the body's first page headers and footers of its own (`titlePg`), empty where none is given.
  - A header or footer with no line is not written.
  - The default follows asciidoctor-pdf's default theme: no header, and the page number in the footer, at the right of odd pages and the left of even ones.
- **Chapter titles: `STYLEREF`.** `{chapter-title}` is `STYLEREF "heading 1"`, or `"heading 2"` where parts take Heading 1. Word and LibreOffice show the latest chapter heading on each page, with no section per chapter.
- **Table of contents: a field with its entries written, updated on opening.**
  - Each listed heading (parts, book headings, page titles and sections; not discrete headings or the title) gets a bookmark `_Toc<n>` as it is written, and the contents is built after the body, from the headings actually written, then inserted where it was asked for. In a book that is the body's start (books have `toc`). In an article it is after the title when the page asks for a table of contents (`ViewTree::toc`), or at `toc::[]` with `toc: macro`.
  - It is a `TOC \o "1-<n>" \h \z \u` field, its result already written: a `TOCHeading` paragraph (`toc-title`, else `Table of Contents`), then a `TOC<level>` paragraph per heading, each a link to the heading's bookmark with a `PAGEREF` field for its page number, blank until updated.
  - `n` is `toclevels` (default 2) counted from the chapters, as Asciidoctor counts it in a book, plus one where there are parts.
  - `w:updateFields` is set when there is a table of contents, so Word offers to update the document's fields on opening. The prompt says the document contains fields that may refer to other files; updating fills in the page numbers.
  - LibreOffice does not update on opening. Its contents reads correctly, without page numbers, until Tools › Update › Update All. Accurate page numbers before an update are not promised.
- **Styles.** Added: `Header`, `Footer` (both with the tab stops), `TOCHeading`, `TOC1`–`TOC9` (indented, a dotted right tab for the page number), `CoverImage` and `TitlePageDetail`. `styles.xml` takes the text width for the tab stops.
- **Checked by hand.** LibreOffice 26 laid out the book sample (`ADOC_DOCX_OUT=<dir> cargo test -p adoc-docx --test pages -- --ignored`), Letter landscape, as six pages:
  1. the front cover, filling the page;
  2. the title page, logo, title, subtitle, author, version and date, with no running content;
  3. the contents, its first-page footer a centred `1`;
  4. an even page, its header mirrored (logo left, title right) and its footer number at the left;
  5. an odd page, title left, logo right, the chapter's title left in the footer and `3 of 6` at the right;
  6. the back cover, with no running content.

### D6 (#623): `[docx]` settings and profiles

`crates/adoc-engine/src/docx_settings.rs` reads `[docx]` (its module documentation is the schema); the engine turns it into `DocxOptions` for each export (`Engine::docx_options`).

- **`[docx]`, either layer.**
  - Keys:
    - `paper` (`a4`, `letter`) and `orientation` (`portrait`, `landscape`);
    - `logo` (relative to the first root);
    - `odd_even`;
    - `[docx.margins]`: the four sides and the header and footer offsets, as `25mm`, `2.5cm`, `1in`, `72pt` or a number of points, at most 10 inches;
    - `[docx.header]` and `[docx.footer]`: `left`, `centre` (or `center`) and `right`, with `[… .even]` and `[… .first]` tables;
    - `[docx.styles]`: `body_font`, `heading_font` and `code_font` (1 to 31 characters), `body_size` (6 to 72 points), and `heading_colour` and `link_colour` (six hexadecimal digits, `#` optional; `color` spellings accepted).
  - A workspace key replaces the user's. A header or footer table replaces that one's lines whole, so a workspace that sets `[docx.footer]` does not keep the user's even line.
  - Unknown keys and values that cannot be used are warned about in the settings' usual form (`<file>: `docx.<key>`: <problem>`) and leave the default.
  - The defaults are those of D5: A4, portrait, an inch of margin, the page number in the footer.
- **Styles.** `DocxOptions::styles` (`StyleOverrides`) change `styles.xml`:
  - the body font and size are the document defaults;
  - the heading font and colour go on the title, subtitle, headings and the table of contents' heading;
  - the code font goes on `CodeChar`, `Keyboard`, `SourceCode` and `LiteralBlock`;
  - the link colour goes on `Hyperlink`.

  `.dotx` templates remain out of scope (§3).
- **Profiles: repeated in each format.**
  - `[docx.profiles.<name>]` declares its own content (`page`, `component` or `nav`), `output`, `attributes`, and Word's own `paper` and `orientation`.
  - The content and attribute parsing, and the content's resolution against the first root, are the PDF profiles' own (`profile_content`, `profile_attributes`, `profile_target`, `target_problems` in `profiles.rs`, refactored out of `parse_profile` without changing it). Nothing is shared at the level of a profile: a PDF profile and a DOCX profile of the same name are independent, and a book published in both repeats its one line of content.
  - The cost is a repeated line; the gain is that neither format's profile can change the other's, and `[pdf.profiles]` keeps its meaning exactly.
  - An `output` that is not a `.docx` file name, an unknown key (a PDF one such as `theme` included), more or fewer than one content key, and a bad `paper` or `orientation` leave the profile out with a warning.
  - `Engine::docx_profile_export_args` resolves a profile as `profile_export_args` does: `<name>.docx` by default, refused when the output is outside the workspace. `docx_profile_problems` says what would stop it.
  - `export.docx_profile` ("Export DOCX Profile…") exports one with `CommandArgs::Profile`.
  - `DocxExportArgs` gains `paper` and `landscape`, a profile's.
- **PDF unchanged.** `[pdf]`, its profiles, their default `<name>.pdf` and their precedence are as before. `docx_settings_leave_pdf_unchanged` loads the same `[pdf]` with and without `[docx]` and compares, and `a_pdf_profile_of_the_same_name_is_its_own` checks the engine side.
- **Tests.**
  - `docx_settings.rs`: every setting read, the layers, every unusable value warned about, the profiles.
  - `tests/docx_export.rs`: `docx_profiles_resolve_and_validate`, covering:
    - page, component and navigation profiles;
    - the default output;
    - an output outside the workspace, refused;
    - a missing component, reported;
    - a non-`.docx` output, warned about;
    - an export through `export.docx_profile`, its document Letter landscape with the settings' margin, header and heading colour and the profile's author.
  - `adoc-docx`: `style_overrides_change_the_document_s_styles`.

### D7 (#624): CLI, desktop and MCP entry points

Each front end builds the engine's `docx` feature and calls only the engine; none names `adoc-docx`.

- **Command line.**
  - `adocfoundry-cli docx <file|component-dir> [--root] [--component [<version>@]<name>] [--output <file.docx>] [--fail-on none|warning|error]`, and `docx --profile <name> [<root>] [--output]`.
  - The target selection, default outputs and reporting are `pdf`'s, now shared (`open_target`, `component_file_name`, `fail_on`, `profile_root`, `report`), so the two cannot drift.
  - There are no PDF flags; an unknown option is refused as for any subcommand.
  - The line printed counts AsciiDoc pages (`(8 AsciiDoc pages)`), since Word lays out its own pages and the count is not comparable with a PDF's.
  - A diagnostic at or above `--fail-on` exits 1 with the document written, as `pdf` does.
- **Desktop.**
  - Export Page to Word…, Export Component to Word… and Export DOCX Profile… are actions in the File menu, after the PDF ones, and in the palette. None has a default key.
  - They reuse the PDF commands' save dialog and component choice (`export_page`, `export_component` and `export_to`, which now take a function from the chosen path to `CommandArgs`), the profile picker (`export_profile`) and the completion notification. The notification counts AsciiDoc pages for a `.docx` and says "Word export failed" on an error.
  - There is no preview pane; Open hands the file to the system only when clicked.
- **MCP.**
  - `export_docx` is a write tool, taking exactly one of `path`, `component` (`name` or `version@name`) or `profile`, plus `output` (required unless a profile gives one).
  - The output is confined to the roots, refused when it is a link (inside or outside) and refused unless it is a `.docx` file no page is. A `.docx` there already is replaced, as `export_pdf` replaces a PDF.
  - The answer is `{ output, pages, sources, warnings }`, with `sources` the pages in the document's order.
- **Formulas.** Building the release CLI showed that the shared pipeline built every export's views with formulas left for a typesetter, which a PDF has and a Word document does not, so each formula was exported as its source with a warning. A Word job's views now ask for formulas as the images the STEM provider renders (`Request::draws_formulas`), awaited like diagrams; each is a picture with its source as alternative text. `formulas_are_typeset_as_pictures` covers it.
- **Packaging.** The release smoke test (`packaging/smoke-test.py`) also exports a page with the downloaded `adocfoundry-cli docx`, so a build without Word export fails before it is published.
- **Tests.**
  - `docx_cli_exports_and_gates` (`crates/adocfoundry-cli/tests/cli.rs`): an article, a book and a profile (the same bytes as the engine's command), the `--fail-on` codes, PDF flags refused and a non-`.docx` output refused.
  - `docx_mcp_export_is_write_gated` (`crates/adoc-mcp/tests/docx.rs`): unlisted and `permission_denied` without `--allow-write`; page, component and profile exports (the profile's bytes equal to the engine's); every refusal with its code and nothing written; links refused.
  - `docx_app_exports_through_engine` and `export_docx_profile_offers_the_word_profiles` (`crates/adocfoundry/src/ui_tests.rs`): through the save dialog, the book the engine writes, byte for byte, in navigation order, with no tab opened.

### D8 (#625): interoperability, performance, accessibility and QA

- **The validator is shared.**
  - The package reader and `assert_valid_package` moved from `adoc-docx`'s test helpers into `adoc_docx::testing`, behind a `test-support` feature (as `adoc-git`'s `testing` is), so the engine's tests run every structural check on what the engine writes.
  - The checks gained:
    - styles by kind in headers and footers, not only the body and footnotes;
    - fields that are whole (begin, at most one separate, end, in order, none left open);
    - bookmarks that end once each;
    - sections' header and footer references that lead to header and footer parts.
  - `the_package_checks_catch_damage` (`tests/validator.rs`) breaks a valid package nine ways (a missing picture part, malformed XML, an id with no relationship, an undefined style, a field left open, an unended bookmark, a link to no bookmark, a footnote not written, a part with no content type), plus a changed byte and a cut package. It requires each to be caught, so a check that passed everything would fail.
- **The reference book** (`tests/fixtures/docx/reference`).
  - Contents:
    - the `manual` component at 2.0 (two navigation files) and 1.0, and an `api` component;
    - a partial including another;
    - every construct D3–D5 write;
    - PNG, JPEG and SVG images, a Mermaid diagram, and inline and block formulas;
    - covers and a title logo;
    - `[docx]` settings with odd, even and first-page headers and footers.
  - `docx_reference_book_is_complete` checks:
    - the package whole;
    - the pages and their order, and nothing from 1.0 or `api`;
    - each construct's Word structure;
    - one warning, at its line;
    - seven distinct pictures, each once whatever uses it, nothing referring outside the package;
    - the four sections, A4 and the settings' margins;
    - the running content and its fields, and the core properties;
    - the same bytes from a second export and from a copy of the workspace elsewhere.
  - A book's author comes from the component's, playbook's or profile's attributes, as the PDF's does, not from the start page's header; the fixture sets it in `antora.yml`.
- **Failures.**
  - `docx_problems_are_located`: a missing image in a page and in a partial (placed in the partial), a diagram nothing renders, a video and source cut short are each a warning at their file and line, with a stand-in in the document.
  - **Found and fixed: a timed-out export replaced the output afterwards.** `export_blocking`/`export_docx_blocking` dropped a job that ran out of time, but a writer already packaging went on to rename its file over the output, after the caller had been told the export failed. Each job now has a `Commit`, one lock the writers take to replace the output and the timeout takes to abandon the job. A timed-out job never replaces the output, and one whose file is already in place is waited for and reported as finished. PDF exports share the guard.
  - `a_timed_out_export_leaves_the_output` sweeps timeouts upward from 2 ms until one succeeds, checking the old file and that no temporary file is left after each failure. It failed on every run before the fix.
- **Size and time** (`docx_large_book_is_bounded`, `tests/docx_large.rs`, a test binary of its own so the process's peak is the export's).
  - The book is 600 pages of ten sections each (lists, notes, code, tables, footnotes, links between pages), exported through `export.docx_component` as the app does, each `pump` timed.
  - On a 2024 Mac:

    | Build | Whole export | Command | Longest pump | Peak memory |
    | --- | --- | --- | --- | --- |
    | Debug | 4.4 s | 0.32 s | 0.15 s | 0.38 GB (process, index included) |
    | Release | 0.41 s | 0.04 s | 0.05 s | 0.36 GB |

  - Time and memory grow linearly: 1,200 pages took 8.8 s and 0.75 GB in debug.
  - The budgets, for CI's debug builds: 3 s for the command, 3 s for any pump, 90 s in all, and 2 GB peak where Linux reports it (`VmHWM`). Elsewhere memory is not measured, since the engine forbids the `unsafe` a counting allocator would need and `rustix` offers no `getrusage`.
  - The command reads the pages' text and the views are built in one pump. Both belong to the pipeline PDF uses too and are left as they are; at these sizes in release they stay under a frame or three.
- **Build impact** (`cargo build -p adoc-engine --no-default-features`, debug, an empty target folder each):

  | Features | Cold build | Units built | Typst crates |
  | --- | --- | --- | --- |
  | `docx` | 32 s | 264 | none |
  | `typst` | 44 s | 478 | 25 |
  | `typst docx` | 40 s | 485 | 25 |

  Word export adds 7 crates to a build with PDF, within the noise of a cold build. Without PDF, Word export compiles no Typst at all. Rebuilding the engine after touching `adoc-docx` takes 1 s.
- **Interoperability** (`docs/qa/docx.md`, which also lists the manual checks for Word and LibreOffice on each platform). LibreOffice 26.8 on macOS:
  - converts the reference book without error, and every construct shows as intended;
  - shows the contents without page numbers until fields are updated, as expected without Word;
  - when it saves the book back to DOCX, keeps the text unchanged, all 11 drawings with valid links and all 11 fields, merging the two pictures whose pixels are identical.

  Word itself has not been run here; its checks are in the record's results table.
- **Docs.** The user guide gains *Exporting to Word*. It covers what it makes, the three entry points, settings and profiles, warnings, safety, what Word does differently (reflow, fields, page counts) and what is deferred from this version. The developer pages cover the tests and the commit lock.

## 15. References

- Microsoft, [Structure of a WordprocessingML document](https://learn.microsoft.com/en-us/office/open-xml/word/structure-of-a-wordprocessingml-document)
- [`docx-rs`](https://docs.rs/docx-rs/latest/docx_rs/), the library candidate of §13
