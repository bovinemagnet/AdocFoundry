# PRD: AdocFoundry — DocBook Export

| Field | Value |
| --- | --- |
| Status | Implemented in DocBook 1/9 to 9/9 (#649–#657), 2026-11-19; the open decisions of §14 are settled in §15. The checks with DocBook consumers in `docs/qa/docbook.md` are open |
| Date | 2026-11-16 |
| Scope | A native publication backend that writes semantic DocBook 5.2.1 XML, with its referenced assets, for a page, an Antora component version or selected navigation files, entirely in Rust, through the engine, CLI, desktop app and MCP server |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Standard | [DocBook 5.2.1 schemas](https://docbook.org/schemas/docbook/) (RELAX NG and Schematron); [DocBook: The Definitive Guide, 5.2.1](https://tdg.docbook.org/tdg/5.2.1/index.html) |
| Related work | Epic #615; work packages #649 to #657 (§12). #619 (the shared publication plan, from the DOCX epic #613). Separate exports, never prerequisites: #613 DOCX, #614 EPUB, #616 Kindle |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `AGENTS.md`; the DOCX, EPUB and Kindle export PRDs in `docs/prd/`; `docs/prd/PRD-AdocFoundry_Phase_4_Implementation_Specification.md` (PDF export); `cargo xtask layers` |
| Existing code | `crates/adoc-engine/src/export.rs`, `crates/adoc-engine/src/profiles.rs`; `crates/adoc-render/src/view.rs` |

## 1. Purpose and problem

AdocFoundry publishes a page or an Antora book as PDF. Some publishing pipelines do not want a finished rendering at all: they want structured XML to feed their own toolchain, where a house stylesheet, a translation system or a content management system takes over. DocBook is the long-standing XML vocabulary for technical books, and many such pipelines read it.

Add a native Rust DocBook exporter: a single AsciiDoc page as a DocBook `article`, or an Antora component version or explicit navigation selection as a structured `book`, in **DocBook 5.2.1 XML**. It preserves chapters, sections, titles, metadata, lists, technical tables, source code, admonitions, footnotes, cross-references, figures, diagrams and STEM, writes the media the document refers to beside it, and reports source-located warnings, straight from AdocFoundry's resolved view data.

**DocBook is semantic XML, not a rendered publication.** Page headers, footers and page numbers are outside the exporter by design: the downstream DocBook toolchain controls pagination and appearance.

This is publication. It is not DocBook import, a DocBook preview or a DocBook processor.

## 2. Goals

1. Export one page as an `article` and one component version or navigation selection as a `book`, with the same content selection, order and deduplication as the PDF exporter.
2. Write well-formed, deterministic XML in the DocBook namespace with the correct version, a valid article or book hierarchy and `info` metadata.
3. Keep meaning as DocBook elements: lists, CALS tables, program listings, admonitions, footnotes and figures, never flattened text and never converted HTML.
4. Keep references sound when many pages become one book: unique `xml:id` values and no dangling `linkend`.
5. Make the output portable: every local `fileref` resolves to a file written beside the XML, by a relative path.
6. Run every stage in process as Rust code (§7).
7. State the validation coverage honestly: full schema validation only if it is really performed, otherwise partial structural validation described as such.
8. Make publication repeatable through `[docbook]` settings and `[docbook.profiles.<name>]`, independent of every other format's settings.
9. Offer the same export through the desktop app, the CLI and a write-gated MCP tool, all through `adoc-engine`.
10. Never lose content silently: an unsupported construct, a missing asset or an unexported link target gives a diagnostic with its source location.

## 3. Non-goals

- DocBook input or import, a DocBook live preview, and any transformation *from* DocBook to PDF, HTML or DOCX.
- XSLT themes, page styling and anything else that belongs to the downstream processor. No page headers, footers or page numbers are written.
- Fetching remote images, DocBook extensions and customisation layers.
- Output split into several XML files joined by XInclude. The first release writes one XML document and one assets folder.
- Depending on another format's exporter. DocBook shares the engine's selection and preparation with DOCX (#613), EPUB (#614) and Kindle (#616), never their serialisers or style rules.
- Requiring `xmllint`, `jing`, an XSLT processor, Java, Python or any other tool, to export or to run the tests, and downloading a schema.

## 4. User workflows

**Export a page.** With a page open, choose Export Page as DocBook…, pick a file, and receive `guide.xml` with `guide_assets/` beside it when the page has images. Diagnostics appear as they do for a PDF export.

**Export a book.** Choose Export Component as DocBook… for a component version; the book follows the effective navigation order, with parts, chapters and sections.

**Publish a profile.** Define `[docbook.profiles.manual]` once, then run `adocfoundry-cli docbook --profile manual` locally or in CI, with `--fail-on warning` to gate on diagnostics.

**An agent exports.** With `--allow-write`, an MCP client calls `export_docbook` for a page, component or profile, under the same root confinement as `export_pdf`.

**The pipeline takes over.** The XML and its assets folder are handed to a DocBook toolchain, which renders, translates or stores them. Nothing in them points back at the author's checkout.

## 5. Functional requirements

### 5.1 Output and publication structure

- A selected page becomes an `article`; a component version or navigation selection becomes a `book`. Each has `info` with title, subtitle, authors, revision, keywords and description where available.
- The start page or preface, navigation list parts, page chapters, navigation-only headings, nested sections and the depth of document-level headings map to a valid DocBook hierarchy. Where the schema fixes the placement of `title`, `info`, `part` and `chapter`, the output follows it exactly.
- Selection matches the shared plan: `ExportTarget::{Page, File, Component, Navigation}`, the start page, navigation order, page deduplication, navigation-only grouping, component and version scoping and effective attributes.
- Document attributes and profile overrides are kept. Metadata is never fabricated; a page without a title gets a documented fallback and a diagnostic.
- The XML is UTF-8, in the DocBook namespace, with a `version` attribute on the root that the schema accepts. Text and attribute values are escaped, characters XML 1.0 forbids are refused or normalised, and the order of everything is predictable. No `DOCTYPE` and no entity declaration is written.

### 5.2 Content

- Paragraphs; emphasis and strong; inline code; hard breaks; superscript and subscript; keyboard, menu and button markup; quotations.
- Itemised, ordered, variable and nested lists; callout groups; a `procedure` only where the source indicates one.
- Source and literal blocks as `programlisting` and `screen`, with whitespace, language and callouts kept; sidebars, examples and open blocks; note, tip, warning, caution and important admonitions; captions and thematic breaks.
- Footnotes as `footnote`, and a footnote referenced again as `footnoteref`, with stable identifiers.
- Tables as **CALS** tables with header and footer rows, alignments, column widths, `colspan` and `rowspan` (as column and row spans), captions and blocks inside cells. Every combination is held to the DocBook table model, not to HTML's.
- Raw passthrough is never written as XML: it is escaped and warned about. Nothing is converted from HTML.
- An unsupported node keeps its text where possible and gives a warning with its source span; content never disappears.

### 5.3 Identifiers and links

- `xml:id` values for publication roots, chapters, sections, anchors, figures and tables are deterministic, valid XML names, unique across the publication and derived from Antora resource identities and anchors, not from checkout paths. Collisions, anchors repeated on different pages and Unicode identifiers are handled.
- `ViewInline::Xref` and `ResolvedLink` are reused; nothing is resolved a second time. A reference to a target in the publication becomes a `linkend`. A target outside it uses its published URL as an `xlink:href` where one is configured and the content model allows, otherwise readable text and a diagnostic. External links are `xlink:href` links.
- The author's link text is kept. A dangling `linkend` is never written.
- Cross-chapter links, local anchors, repeated footnote references, figure and table identifiers, and references to content in a partial (under the page that includes it) are covered.
- After writing, a reference pass checks that identifiers are unique and that every reference and asset resolves, with source ranges in its diagnostics.

### 5.4 Images, diagrams and STEM

- Images become `figure` or `informalfigure` with `mediaobject`, `imageobject` and `imagedata fileref`, a caption and a text alternative from `ViewImage`. Each `fileref` is a stable relative path.
- The default output is `<name>.xml` with a companion `<name>_assets/` folder. The engine resolves and copies existing PNG, JPEG and supported SVG files into a confined staging folder, storing identical content once. No absolute path and no path into the checkout appears in the XML.
- A `reference` asset policy, which leaves `fileref` pointing at the original files and copies nothing, is offered beside the default `copy`, and is documented as not portable.
- Diagrams the view pipeline rendered in process are used. The export starts no Graphviz, D2, PlantUML, image converter or downloader. A diagram that is not available keeps its source in the document and gives a diagnostic.
- STEM becomes valid MathML where Rust code can produce it; otherwise the formula's source is kept as a `mathphrase` and a warning notes the loss of fidelity. Both inline and display formulas are covered.
- Assets are resolved under the engine's file-access rules. Nothing is fetched from a network.

### 5.5 Configuration

- `[docbook]` holds defaults: the asset policy, the assets folder's suffix, structural preferences, the diagnostic threshold, an optional self-check after writing and the fallbacks.
- `[docbook.profiles.<name>]` holds a publication: a `page`, `component` or `nav` selection, an `output`, `attributes` and DocBook options. It reuses `ProfileContent` and the output-path validation of `crates/adoc-engine/src/profiles.rs`, and no PDF theme data.
- User and workspace settings follow the documented precedence. Unknown and invalid values are refused or warned about, and no setting gets round a path boundary.
- A profile fails explicitly for a missing component, an unknown version, an output that cannot be written, an unexpected extension and an asset collision.
- The PDF profile interfaces, `pdf-theme`, defaults and output paths are untouched.

### 5.6 Front ends

- **CLI:** `adocfoundry-cli docbook <file|component-dir> [--root <dir>] [--component [<version>@]<name>] --output <file.xml>` and `docbook --profile <name>`, with `--assets copy|reference` and `--fail-on none|warning|error`.
- **Desktop:** Export Page as DocBook…, Export Component as DocBook… and Export DocBook Profile… in the menu and command palette, with the dialogs, progress, completion and diagnostics of the other exports. No DocBook preview.
- **MCP:** `export_docbook`, typed, listed and callable only with `--allow-write`, with root confinement covering the XML and its assets folder, link refusal and the safe overwrite policy.
- Every front end calls `adoc-engine`, selects the same sources and produces the same XML and the same list of assets; none reaches `adoc-docbook` directly. The output and asset paths are reported, and never a path outside the workspace.

## 6. Existing code to reuse

| Code | What it gives | How DocBook uses it |
| --- | --- | --- |
| `crates/adoc-engine/src/export.rs` | `ExportTarget`, the article and book plan, parsing of unsaved buffers, view construction, the diagram lifecycle, worker completion, cancellation, diagnostics and source spans | Through the format-neutral plan #619 extracts. No second page selection and no copied book planner |
| `crates/adoc-engine/src/profiles.rs` | `ProfileContent` and output-path validation | The selection model and path rules; no PDF option |
| `crates/adoc-render/src/view.rs` | `ViewTree`, `ViewKind`, `ViewInline` (with `Xref`, `Link`, `FootnoteRef`), `ResolvedLink`, `ViewImage`, `DiagramView`, footnotes, attributes and source spans | The exporter's only input. AsciiDoc is not parsed a second time, and HTML is not an intermediate |
| The PDF entry points in the CLI, the app and the MCP server | Commands, dialogs, notifications and the write gate | DocBook entry points sit beside them |

## 7. Rust-only execution

- Every stage of the export runs in process as Rust code. It never calls `xmllint`, `jing`, an XSLT processor, Java, Python, a converter or a publishing service, and downloads no schema. The same holds for the automated tests.
- `adoc-docbook`'s own source is held to a forbidden-paths list in `xtask/src/layers.rs` (no process, no network), as `adoc-audit` and `adoc-impact` are.
- New dependencies are pinned exactly, reviewed for licence, maintenance and the toolchain they need, and pass `cargo deny check`.
- **Validation is described as what it is.** Well-formed XML is not schema conformance. If full RELAX NG and Schematron validation against the 5.2.1 schemas can be done by a permissively licensed Rust library in process, it is; if not, the self-check is called partial and the documentation lists exactly which constraints it covers.
- Evidence from DocBook consumers, checked by hand, is welcome and recorded apart (§11), and is never needed to build, export or test.

## 8. Architecture

```text
L4   adocfoundry / adocfoundry-cli / adoc-mcp
                    | format-specific commands and presentation
L3   adoc-engine    | shared publication plan (#619); parse + views + diagrams;
                    | asset staging; safe output; DocBook settings and profiles
L2   adoc-docbook   | DocBook 5.2.1 XML writer, identifiers and references,
                    | asset references, structural validation
L2   adoc-render    | shared ViewTree / ViewKind / ViewInline / source spans
L1   existing parser, DOM, Antora and index crates
```

| Crate | Responsibility |
| --- | --- |
| `adoc-docbook` (L2, new) | Turn a publication's view trees and typed options into DocBook XML and a list of the assets it refers to, check its identifiers and references, and report typed diagnostics with source locations. Headless; no engine, worker, UI or LSP types, no process and no network |
| `adoc-render` (L2) | Unchanged |
| `adoc-engine` (L3) | The shared publication plan; the optional `docbook` feature; DocBook commands, arguments and events; `[docbook]` settings and profiles; staging and copying assets; the worker lifecycle; safe output |
| `adocfoundry`, `adocfoundry-cli`, `adoc-mcp` (L4) | The entry points of §5.6 |

- `adoc-docbook` sits at **L2**. It may depend on `adoc-render` through a new pair in `SAME_LAYER_ALLOWED` in `xtask/src/layers.rs`, as `adoc-pdf` does, and on lower layers. It must not depend on `adoc-engine`, GPUI, Typst, `adoc-pdf` or another format's exporter.
- The crate needs an entry in `LAYERS`, an entry in `[workspace.dependencies]`, and a mention in `AGENTS.md`, `README.md`, the design document and the developer documentation's crate table and diagram (`architecture.adoc`, `crate-layers.mmd`), which `every_crate_is_documented` in `xtask` checks.
- The engine's `docbook` feature is opt-in and independent of `typst`, `git` and every other format's feature. It is off in `[workspace.dependencies]`, so each front end names it, and `adoc-ls` does not.

## 9. Headless contracts

Names are illustrative; DocBook 1/9 and 2/9 settle them and record the outcome in §15.

- `adoc-docbook`: a `DocBookDocument` that is an `Article` or a `Book`, built from the plan's view trees and document facts; options; a result carrying the XML, the assets it names (each a source and its relative `fileref`) and the diagnostics. The crate writes no file: it names the assets and the engine copies them.
- `adoc-engine`: DocBook export arguments with no PDF-only field, commands beside `export.pdf_page` and `export.pdf_component`, a blocking export for the CLI, cancellation and timeouts, progress, and completion as an `EngineEvent` consistent with `ExportFinished`. Publishing never modifies a source.
- Diagnostics follow the existing `ExportDiagnostic` model, with severities that allow CI gating.

## 10. Output safety, performance and reliability

- The XML and the assets are staged beside their destination and put in place only when the whole export has succeeded. A failure, a worker panic or a cancellation leaves the source, the previous XML and the previous assets as they were.
- A link as the output file or assets folder, a path that leaves the roots, an unexpected extension and a collision with unrelated files are refused.
- Export runs on workers and never blocks the UI. Assembling a book is not superlinear in its pages, and views are not cloned needlessly.
- Hostile input is handled without a panic or an unsafe result: markup in text, entities and `DOCTYPE` tricks, Unicode, truncated and invalid input, malicious paths and links that escape the roots.
- The same input gives the same XML on every platform and from any checkout path.

## 11. Tests and acceptance criteria

Each test below exists under its name when its work package is done. All of them are Rust tests that run no other program.

| # | Test | Package | What it shows |
| --- | --- | --- | --- |
| 1 | `docbook_root_is_wellformed_and_versioned` | DB1 | A minimal article and book are well formed, in the DocBook namespace, with the accepted `version` and `info` in its place |
| 2 | `docbook_text_is_escaped` | DB1 | Escaping, Unicode and forbidden characters; no `DOCTYPE`, entity or raw passthrough; malformed metadata, empty output and inconsistent structure give errors or diagnostics |
| 3 | `docbook_output_is_deterministic` | DB1 | Two exports of one input are the same bytes |
| 4 | `docbook_and_pdf_select_the_same_pages` | DB2 | Article, component and navigation exports take the same pages in the same order as PDF, over several components and versions and with unsaved buffers |
| 5 | `docbook_builds_alone` | DB2 | A build with `docbook` and no other format feature compiles neither Typst nor another exporter |
| 6 | `docbook_failed_export_keeps_existing_output` | DB2 | A failed export, a worker panic, a link as the output and an invalid target leave the previous output and the source untouched |
| 7 | `docbook_book_hierarchy_follows_the_plan` | DB3 | A version-aware fixture over several components gives an ordered book with the expected `part`, `chapter` and `section` nesting, unique headings, and the plan's deduplication and start page |
| 8 | `docbook_article_has_metadata` | DB3 | A single page gives an `article` with its metadata; a missing title gives the fallback and a diagnostic |
| 9 | `docbook_rich_content_uses_docbook_elements` | DB4 | Nested lists, code with callouts, admonitions, quotations and repeated footnotes are the proper elements |
| 10 | `docbook_tables_are_cals` | DB4 | Header and footer rows, alignments, widths, merged cells and blocks in cells are valid CALS |
| 11 | `docbook_unsupported_nodes_warn` | DB4 | An unsupported block gives a located warning and keeps its text; passthrough is escaped |
| 12 | `docbook_assets_are_copied_and_relative` | DB5 | Every local `fileref` resolves to a file in the assets folder by a relative path; identical content is stored once; no absolute path appears |
| 13 | `docbook_missing_media_is_reported` | DB5 | Missing media and an unavailable diagram give a located diagnostic; the caption and the text alternative remain |
| 14 | `docbook_formulas_have_a_fallback` | DB5 | Inline and display formulas are MathML or a `mathphrase` with a warning, never absent |
| 15 | `docbook_ids_are_unique_and_stable` | DB6 | Identifiers are valid, unique and the same across platforms, checkout paths and runs, with repeated anchors and Unicode |
| 16 | `docbook_references_resolve` | DB6 | No missing or dangling `linkend`; links reach the intended chapters and sections; a target outside the publication is text or a URL |
| 17 | `docbook_profiles_resolve_and_validate` | DB7 | Page, component and navigation profiles export the right documents; invalid settings, outputs and asset paths fail explicitly; precedence holds |
| 18 | `docbook_settings_leave_pdf_unchanged` | DB7 | One book published as PDF and as DocBook with independent settings; the PDF profile tests are as before |
| 19 | `docbook_cli_exports_and_gates` | DB8 | The CLI writes an article, a component book and a profile, with exit codes that follow `--fail-on` |
| 20 | `docbook_mcp_export_is_write_gated` | DB8 | `export_docbook` is listed only with `--allow-write`, and confines both the XML and the assets folder |
| 21 | `docbook_app_exports_through_engine` | DB8 | The desktop actions invoke the engine's commands and report completion and diagnostics |
| 22 | `docbook_is_in_process` | DB9 | `adoc-docbook` names no process and no network path |
| 23 | `docbook_reference_book_is_complete` | DB9 | The fixture book (several versions, the whole content vocabulary, merged CALS cells, code, repeated footnotes, links, figures, a diagram, a formula) exports with no unreported loss and no invalid link, against snapshots |
| 24 | `docbook_hostile_input_is_safe` | DB9 | Markup in text, entity and `DOCTYPE` tricks, malicious paths, escaping links and a failure midway give no unsafe output and leave nothing behind |
| 25 | `docbook_large_book_is_bounded` | DB9 | Books of about 100 and 1,000 pages export within an agreed time and memory budget, without superlinear growth |

The XML snapshots use `insta`, as the CLI's and the engine's do. Evidence from DocBook consumers, inspected by hand, is recorded in `docs/qa/docbook.md`. The user guide carries a matrix of supported features, the known limits and the validation coverage of §7.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check`.

## 12. Work packages

| WP | Deliverable | Issue |
| --- | --- | --- |
| DB1 | New `adoc-docbook` crate, XML emission and the 5.2.1 schema strategy | #649 (DocBook 1/9) |
| DB2 | The shared Antora publication plan and the engine's export orchestration | #650 (DocBook 2/9) |
| DB3 | Book and article hierarchy, navigation chapters and metadata | #651 (DocBook 3/9) |
| DB4 | Rich AsciiDoc blocks and inline semantics as DocBook | #652 (DocBook 4/9) |
| DB5 | Portable images, figures, diagrams, STEM and output assets | #653 (DocBook 5/9) |
| DB6 | Publication-wide identifiers, cross-references and link integrity | #654 (DocBook 6/9) |
| DB7 | DocBook settings, named publication profiles and safety | #655 (DocBook 7/9) |
| DB8 | CLI, desktop and MCP entry points | #656 (DocBook 8/9) |
| DB9 | Rust-only conformance, compatibility, regression and performance QA | #657 (DocBook 9/9) |

Order: DB1 (the XML and validation decisions) and DB2 (the shared plan, with #619) first. DB3 follows; DB4, DB5 and DB6 then proceed independently on stable contracts. DB7 and DB8 come once the engine's interface is stable. DB9's schema-validation spike starts with DB1, and its release QA comes last.

## 13. Definition of done

1. An article and a version-aware Antora book produce deterministic, well-formed DocBook 5.2.1 XML with the correct publication hierarchy and metadata.
2. The complex fixture keeps nested lists, CALS tables with column and row spans, code, admonitions, figures and their assets, diagrams, footnotes and valid internal cross-references, with explicit warnings where a fallback is used.
3. Every link and asset reference in the XML is sound; path confinement, link refusal and the preservation of previous output on failure are tested.
4. The app, the CLI and the MCP tool publish through the shared engine; the PDF, DOCX, EPUB and Kindle exporters stay independent.
5. The Rust tests document the real validation coverage (RELAX NG and Schematron where feasible, otherwise structural validation marked as partial), and no external processor is ever invoked.

## 14. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's pull request and in §15.

- **The root's `version` value (DB1).** The schema release is 5.2.1; the value the root element carries is taken from the schema and its published examples, not guessed from the release's name.
- **Schema validation (DB1, DB9).** Full RELAX NG and Schematron validation in Rust, or a partial structural self-check. Schematron in particular needs XPath evaluation. If the official schema files are kept in the repository as test fixtures, their licence and notice are recorded; they are never downloaded.
- **XML writer (DB1).** `quick-xml`, already in the dependency tree, or a small in-house writer. `roxmltree`, also in the tree, can read the output back in tests.
- **Replacing a file and a folder (DB2, DB5).** The other exporters rename one file into place. An XML document and an assets folder cannot be replaced in one step, so the staging, the order of replacement and what is left after a failure between the two need a rule. That rule also covers assets left over from a previous export.
- **Book structure (DB3).** What becomes a `part`, a `chapter` and a `preface`: in particular navigation-only headings and the start page.
- **Procedures (DB4).** What "semantically indicated" means for `procedure`: an explicit role on an ordered list, or never in the first release.
- **Diagrams drawn by external tools (DB5).** This PRD starts no tool during a DocBook export, where a PDF export renders Graphviz, PlantUML and D2 diagrams through tools on `PATH`. Decide whether a diagram already in the cache from an earlier render may be used, and whether the four export PRDs should share one rule.
- **Formulas (DB5).** Nothing in the workspace produces MathML; the view tree carries a formula's source and, with the `typst` feature, its typeset SVG. The first release can write a `mathphrase` with the source, add the SVG as an image where it exists, and leave MathML to a converter added later to `adoc-math`. The EPUB PRD has the same question.
- **Asset names and the `reference` policy (DB5, DB7).** Whether copied assets keep their original names or take names from their content; and whether `reference`, which is not portable, is offered through MCP at all.
- **Identifier scheme (DB6).** The exact derivation of `xml:id` from a resource identity and an anchor, and how a collision is resolved.
- **Who extracts the shared plan (DB2).** #619 belongs to the DOCX epic and is now a prerequisite of four. Whichever epic starts first does the extraction, once.
- **Profile sharing (DB7).** Whether a publication's content selection is declared once and named by each format's profiles, or repeated in each; shared with the other three export PRDs.

## 15. Implementation notes

Each work package records here where the implementation departs from this document and how the open decisions of §14 were settled.

**DocBook 1/9 (#649).**

- **The root's `version`.** `5.2`, the crate's `VERSION`. The 5.2.1 schema declares `attribute version { text }`, so it accepts any value, and its Schematron requires only that the root has one; DocBook 5.2.1's own release notes, a DocBook document, carry `version='5.2'`. So the value is the convention of DocBook's own documents, not something the schema fixes.
- **Schema validation.** Full RELAX NG validation against the official 5.2.1 schema is done in process, in the tests: `crates/adoc-docbook/schema/docbook.rng` is the schema unchanged, its copyright notice and permission at its head, never downloaded, compiled by `oxixml-relaxng` 0.1.2 (Apache-2.0, pure Rust, no `unsafe`, no network, no dependency outside its own family; it passes James Clark's RELAX NG test suite). Before it was adopted it accepted a valid article, book, CALS table and figure and refused nine invalid documents (misplaced `info`, a section without a title, an unknown element, a `tgroup` without `cols`, an empty `tbody`, a chapter in an article, a paragraph in a book, the wrong namespace, an empty list). It is a dependency of the crate's `test-support` feature only, so every document a test writes is validated (`adoc_docbook::testing::assert_valid`) and no shipped binary carries the schema. The schema's embedded Schematron rules are **not** run: no Rust Schematron engine is used. An export runs the crate's own structural check, `adoc_docbook::check`, which covers what applies to what the crate writes (well-formed XML with no `DOCTYPE` or processing instruction, the root, namespace and version, unique and valid `xml:id`s, every `linkend` resolving and a `footnoteref` naming a `footnote`, which is one of the Schematron rules, and relative `fileref`s below the document); it is partial, and is described so.
- **XML writer.** In-house (`xml.rs`, as in the EPUB and DOCX exporters), not `quick-xml`; `roxmltree`, already in the tree, reads documents back in `check` and the tests.
- **An empty container.** DocBook does not allow an `article`, `chapter` or `section` with no block, which an empty page or a heading with nothing under it would give; one gets an empty `para`, which shows nothing.
- **Forbidden paths.** The crate is held to the audit's list (no files, processes, network or threads), stricter than §7's "no process, no network": the engine writes what it hands back.

**DocBook 2/9 (#650).**

- **Who extracts the shared plan.** No extraction was needed: by the time DocBook started, PDF, Word, Kindle and EPUB export already shared one plan in `crates/adoc-engine/src/export.rs` (`Plan`, `PlanItem`, `arrange`), so DocBook is one more `Request` over it (`Request::DocBook`, `DocBookExportArgs`, `to_docbook`), with no page selection of its own. `docbook_and_pdf_select_the_same_pages` holds it to PDF's.
- **The engine's interface.** `DocBookExportArgs { target, output, attributes }` (later packages add their options), `export.docbook_page`/`export.docbook_component` titled Export Page as DocBook… and Export Component as DocBook…, `Engine::export_docbook_blocking`, completion as the other formats' `EngineEvent::ExportFinished`, and `ExportFormat::DocBook`, refused for a read-only page. The `docbook` feature is default for the engine crate, off in `[workspace.dependencies]`; no front end names it until DocBook 8/9.
- **Replacing a file and a folder.** The output is `<name>.xml` and its pictures `<name>_assets/` beside it (`docbook_assets_folder`). Both are written first to a staging folder beside the output (`.adocfoundry-docbook-*`). Then, inside the job's one commit (so a timed-out export never replaces either): the old assets folder is moved into the staging folder, the new one (when there are pictures) moved into its place, and the XML renamed over the output last. If the XML's rename fails, the new folder is removed and the old one put back, so the old document still finds its pictures; otherwise the old folder goes with the staging folder. An assets folder left by an earlier export is so replaced or, when the new document has no pictures, removed. Since that removes a folder, it is done only to a folder that is provably the export's own: not a link, and holding only regular files named as the export names pictures (`i<n>.png`, `.jpg`, `.gif`, `.svg`); any other folder, a file or a link there is refused as a collision before anything is read, and checked again inside the commit.
- **Diagrams drawn by programs.** The shared pipeline starts Graphviz, PlantUML and D2 for every other format. A DocBook job does not (`Request::starts_programs`): its views take such a diagram only from the cache the preview fills, and one not there keeps its source, with a warning saying so. Mermaid, drawn in process, is drawn as for the other formats. `adoc_diagrams::Diagrams::runs_a_program` tells them apart. The other export PRDs keep starting the tools as before.

## 16. References

- [DocBook schemas](https://docbook.org/schemas/docbook/) and the [5.2.1 release](https://cdn.docbook.org/schema/5.2.1/)
- [DocBook: The Definitive Guide, 5.2.1](https://tdg.docbook.org/tdg/5.2.1/index.html)

**DocBook 3/9 (#651).**

- **Book structure.** A book follows the plan's items (`adoc_docbook::BookItem`): the start page the navigation does not list leads as the `preface` (the engine marks it, `PlanItem::Page::preface`, which the other formats ignore; only a book's first item is read so); each navigation list's title is a `part`; a top-level heading of the navigation with no page of its own is a `part` too, outside such a part, and a `chapter` holding its pages inside one, since parts do not nest; a top-level page is a `chapter`; and a page or heading the navigation nests under another is a `section` of it, written after the page's own sections, at any depth. A page's own sections nest as they do in the page. A part or heading with no page under it is left out with one warning (DocBook allows neither empty). A page with no blocks of its own but pages under it needs no empty paragraph.
- **Metadata.** `info` holds only what the attributes give, as Asciidoctor's DocBook converter writes it, never made up: the title and subtitle (a page's title of plain text is split at its last `": "`, as Asciidoctor does; one with markup is kept whole), `author`s from the header's author line (`firstname`, `othername` from `middlename`, `surname`, `email`, numbered for each author) or else an `authors` list, `date` from `revdate`, a `revhistory` of one `revision` when there is a date and a `revnumber` or `revremark`, `releaseinfo` for a `revnumber` with no date (a `revision` must have a date), a `keywordset` from `keywords` and an `abstract` from `description`. A book's attributes are the component's (`revnumber` is its version), and its title the component's.
- **Missing titles.** A page with no title, after the navigation's text for it, takes its file's name, with a warning located at the page; a book with no title is "Untitled", with a warning.

**DocBook 4/9 (#652).**

- **Procedures.** None in the first release: an ordered list is an `orderedlist` whatever its role, so nothing is guessed to be a `procedure`. A role naming one could map to it later.
- **Blocks.** Paragraphs are `para`, or `formalpara` with a title; discrete headings `bridgehead` (`renderas="sect<n>"`); lists `itemizedlist`, `orderedlist` (`numeration` from the list's style, `startingnumber` from `start`), `variablelist` (`role="horizontal"` for that style) and, for the `qanda` style, `qandaset`; a checklist is an `itemizedlist` with `role="checklist"`, each item's `override` `checkedbox` or `checkbox`, the marks DocBook's stylesheets draw as ticked and empty boxes (the schema takes a name there, not a character). Source blocks are `programlisting` with `language` and, with `linenums`, `linenumbering="numbered"`, each callout a `co` at the end of its line, the callout list after it `calloutlist` with each `callout`'s `arearefs` naming its marks (a callout list with a number the listing does not mark is an `orderedlist`, with a warning); literal blocks are `screen`; a titled listing or literal block is in a `formalpara`, as Asciidoctor writes it, since neither has a title of its own. Admonitions are `note`, `tip`, `important`, `caution` and `warning`; quotations and verses `blockquote` with an `attribution` (and `citetitle`), a verse's lines a `literallayout`; sidebars `sidebar`; examples `example` with a title and `informalexample` without; an open block only groups, its title, if any, a `para` with `role="title"` before its content. A block's roles are its element's `role`. A thematic break and a page break write nothing, as a table of contents macro does: presentation is the processor's.
- **Tables.** CALS: `table` (with its title) or `informaltable`, `frame` from `frame` (`ends` as `topbot`), `rowsep`/`colsep` from `grid`, one `tgroup` with a `colspec` per column (`colname="col_<n>"`, `colwidth` proportional, `align`), then `thead`, `tfoot` and `tbody` in CALS's order; each `entry` has its `align` and `valign`, `namest`/`nameend` for a column span and `morerows` for a row span, placed on the grid as the table lays out. Blocks in a cell are written as they are, but for a table: the RELAX NG schema allows an `informaltable` in an `entry`, its Schematron does not, so (as corrected in DocBook 9/9) a table that is all its cell holds is the cell's `entrytbl`, and one beside other content is its rows of text, with a warning. A table of only a header row has it as its body, since CALS needs one; a table with no rows (the parser drops a row whose span does not fit) is left out with a warning. A span past the table's edge is cut back, with a warning, and a row every cell of which a span covers is folded into the spans, since CALS cannot hold a row with no entry.
- **Inlines.** `emphasis` (with `role="strong"` and `role="marked"`), `literal`, `superscript`, `subscript`, `phrase` with the roles, `link xlink:href` (a link in a link keeps only its text), `keycap` and `keycombo`, `guibutton`, `guimenu` or `menuchoice` with `guisubmenu` and `guimenuitem`, `indexterm` with `primary`, `secondary` and `tertiary`, `phrase role="icon"`. A footnote is a `footnote` (`xml:id` `<page key>-fn<n>`) where first referenced and a `footnoteref` after. A hard line break is Asciidoctor's `<?asciidoc-br?>` instruction followed by a line feed, since DocBook has no element for one; it is the one processing instruction written, and `check` refuses any other.
- **Passthrough.** A passthrough block is a `literallayout` with `role="passthrough"`, its text escaped, with a warning. An inline passthrough reaches the view as text, so it is escaped as text is, and cannot be reported apart from it; a raw inline the view does mark is escaped with a warning.

**DocBook 5/9 (#653).**

- **Asset names.** From the order a picture is first used, `i<n>.<ext>` (`png`, `jpg`, `gif`, `svg`), as in EPUB export; the export decides them, so no name of the author's reaches the output. Identical content is stored once, compared byte for byte (a hash narrows the comparison; no new dependency). The `fileref` is `<name>_assets/i<n>.<ext>`.
- **What is copied.** A local picture is read through the engine's `FileAccess` (so never from outside the roots when the engine is confined), recognised by its bytes (PNG, JPEG, GIF, and SVG whose root element is `svg`), at most 16 MB a picture and 256 MB a document, and copied as it is (an SVG is not drawn as a PNG: DocBook processors read SVG). A picture that is missing, unreadable, not one of those or too large keeps its caption and its description (`textobject`), with a warning at its block. A remote picture is never fetched: its address is the `fileref`, with a warning, and `check` accepts an `http`/`https` address there.
- **The `reference` policy.** `adoc_docbook::AssetPolicy::Reference` (the engine's `DocBookAssets::Reference`) copies nothing and writes each `fileref` as the path from the document's folder to the picture, which may climb out of it (`check_document(…, true)`; `testing::assert_valid_referenced`). It is not portable, and says so. Offered through settings and the command line (DocBook 7/9 and 8/9), not through MCP.
- **Figures.** A block picture is a `figure` with its title, else an `informalfigure`, holding a `mediaobject` with an `imageobject` (`imagedata` with `contentwidth`/`contentdepth` from `width`/`height` when they are lengths) and a `textobject` with the description; an inline picture an `inlinemediaobject`.
- **Diagrams.** One the view drew (in process, or from the cache: DocBook 2/9) is a figure of its SVG, its description the block's `alt`, else "A diagram". One not drawn keeps its source as a `programlisting` with `role="diagram"`; the export reports why, as it does for every format.
- **Formulas.** The schema lets an equation hold pictures or `mathphrase`s, never both, so the agreed "source and picture" is written as: with a picture drawn (with the `typst` feature), the equation's `alt` is the source, then a `mediaobject` (or `inlinemediaobject`) of the picture whose text alternative is the source as a `phrase` with `role="math"` (a `mathphrase` may not be inside a `phrase`); without one, a `mathphrase`. A display formula is an `equation` with its title, else an `informalequation`; an inline one an `inlineequation`. No MathML is made; one warning a page says how many formulas were written so. The EPUB PRD keeps the same question open.
- **Audio and video.** Never copied or fetched: each target at a web address is a `videodata` or `audiodata` `fileref` in a `figure`'s `mediaobject`, with the targets as its text; a local file keeps only its name, with a warning.

**DocBook 6/9 (#654).**

- **Identifier scheme.** Every `xml:id` is decided before anything is written (`ids.rs`), so a reference may lead forward. A page's id is its key, which the engine derives from its Antora resource, `component-version-module-path` with the path's folders joined by `-` and its extension dropped (`manual-2.0-ROOT-install`; a file outside Antora by its name), never from a checkout path, so the same pages give the same ids from any checkout and in any navigation order. An anchor a page defines (a section's or block's id, an inline anchor) is `<page id>.<anchor>` in a book, where pages meet, and the anchor itself in an article. Each is made a valid XML name (what no name may hold becomes `_`; a name that may not start as it does starts with `_`) and unique, a later one that would repeat an earlier getting `-2`, `-3`, …; an anchor defined twice on a page is written once. Footnotes (`<page id>.fn<n>`) and callout marks take ids from the same allocator as they are written. Unicode anchors are kept as they are where XML allows them. This departs from the plan's "page and anchor, as Kindle and EPUB do": those exporters' ids are positional (`c<n>-a<m>`), which a DocBook pipeline storing or translating documents should not have to follow.
- **References.** `ViewInline::Xref` and its `ResolvedLink` are reused; nothing is resolved again. A reference into the publication is a `link` with `linkend` and the text the view gives it (the author's, else the target's title); where the view has only its `[target]` placeholder, an `xref`, so the processor writes the target's label. The target page is the one whose file the link names or, for an anchor, the one holding an anchor written in that file, so a reference to an anchor in a partial leads to it in the page that includes the partial. A reference to a page outside the publication is a `link` with `xlink:href` to its published address when the site's address is known (`[pdf] site_url` or the playbook's, as every exporter takes it), with the anchor as a fragment; otherwise, like an unresolved reference or one naming an anchor the publication does not have, it is its text, with a warning located at it. A `linkend` is never written to an id the publication lacks, and `check` refuses one anyway.

**DocBook 7/9 (#655).**

- **Settings.** `[docbook]` holds one key, `assets` (`copy`, the default, or `reference`); a profile's `assets` overrides it, and an export's own (the command line's) overrides both. The other defaults §5.5 lists are not settings: the assets folder is always `<name>_assets`, which the rule for replacing it depends on (DocBook 2/9); the self-check always runs, since nothing unchecked is handed out; `--fail-on` gives the diagnostic threshold where it matters, on the command line; and the structure and fallbacks are the publication plan's and the PRD's, not choices. An unknown key, or an `assets` that is neither, is reported as the settings are read, as every table's is.
- **Profile sharing.** A profile repeats its content (`page`, `component` or `nav`), as the Word, Kindle and EPUB profiles do, through the PDF profiles' shared `profile_content`, `profile_attributes`, `profile_target` and `target_problems`; no PDF option is read. `[docbook.profiles.<name>]` takes `page`, `component` or `nav`, `output` (an `.xml` file inside the first root, `<name>.xml` there by default), `attributes` and `assets`; anything else, an output that is not `.xml` and an unknown policy are refused as the settings are read. `Engine::docbook_profile_export_args` resolves one (an output outside the workspace refused), `docbook_profile_problems` finds without exporting a missing component or an assets folder that is not the export's own, `docbook_profiles` lists them, and `export.docbook_profile` (Export DocBook Profile…) exports one. `DocBookSettings` and `DocBookProfile` are built without the feature too, so a settings file reads the same in every build.
- **Precedence.** As for every table: the workspace's settings over the user's, key by key, a profile wholly replacing one of the same name; no setting reaches outside the first root.

**DocBook 8/9 (#656).**

- **The command line.** `adocfoundry-cli docbook <file|component-dir> [--root <dir>] [--component [<version>@]<name>] [--output <file.xml>] [--assets copy|reference] [--fail-on none|warning|error]` and `docbook --profile <name>`, through `export_docbook_blocking` and the shared target selection and reporting; an output that is not `.xml` or an unknown `--assets` exits 2 before anything is read; after the output and its page count it prints the assets folder written and how many pictures it holds.
- **The app.** Export Page as DocBook…, Export Component as DocBook… and Export DocBook Profile… in the File menu and the palette, through the same save dialog and profile picker as the other formats. Like an EPUB, a DocBook document gets no Open button after it is written (`opens_after_export`): it is for a toolchain, not a viewer.
- **The MCP server.** `export_docbook`, a write tool listed and callable only with `--allow-write`: one of `path`, `component` or `profile`, and `output`. The output is confined to the roots, never a link, a page or anything but an `.xml` file, and its assets folder is confined too (the engine refuses one that is a link or holds others' files). The tool always copies the pictures (`DocBookAssets::Copy`), whatever `[docbook] assets` or a profile says, since referring to files in place is not portable; that is for the command line. It answers `output`, `assets`, `pages`, `sources`, `warnings` and `protected`.
- **Manual checks.** `docs/qa/docbook.md` lists the checks with DocBook consumers (Jing with the Schematron rules, the XSLT stylesheets, an editor), recorded apart and never needed to build or test.

**DocBook 9/9 (#657).**

- **Validation coverage, stated as it is.** Full RELAX NG validation against the official DocBook 5.2.1 schema, in process, of every document the tests write (the crate's own tests, the engine's, the CLI's and the MCP server's), including the reference book and every AsciiDoc page of the repository's fixtures and documentation (`tests/corpus.rs`, 255 pages). Of the schema's Schematron rules, the export's own `check`, run on every export, enforces the root's `version`, a `footnoteref` naming a `footnote`, and every rule that keeps an element out of another's descendants (`EXCLUSIONS`: no admonition in an admonition or a note, no sidebar in a sidebar, no example, figure, table or equation in an example, figure or equation, no example, figure or equation in a table, no table in a table's cell, no block in a footnote); the rest (about links of other kinds, `coref`, glossary and synopsis rules, XLink typing, transclusion) are not run, so Schematron conformance is partial, and the export writes none of the elements they concern.
- **Nestings the Schematron forbids.** RELAX NG alone accepted them, so the first packages wrote some. The writer now tracks the containers open around what it writes and lowers what one forbids: an admonition inside an admonition is a `para` with its label and `role` and then its content, with a warning; a sidebar in a sidebar is its title and content; a titled example, figure, table or equation where its formal element may not be is its informal one with its title as a `para` before it; a table that is all its cell holds is the cell's CALS `entrytbl` (unless the cell spans rows, which an `entrytbl` cannot), and one beside other content in a cell is its rows as paragraphs of text, with a warning.
- **What a code span holds.** A `literal` may hold text, emphasis, links, phrases, footnotes, sub- and superscripts, index terms and inline pictures, nothing else: in a code span a nested code span, a formula, a key, a button or a menu is its text, and an anchor is written after the span. The corpus found this; the hand-written tests had not. Every export runs `check` (well-formed, no `DOCTYPE` or processing instruction but the line break, the root, namespace and version, unique valid ids, every `linkend` resolving, relative picture paths). No external validator, processor or schema download is involved anywhere; consumers checked by hand are recorded apart in `docs/qa/docbook.md`.
- **Links a reader should not follow.** An `xlink:href` is written only for a web, mail, file transfer or telephone address, or one with no scheme; any other (`javascript:`, `data:`, `file:`) is its text, with a warning, as the e-book exporters do.
- **Tests.** `docbook_is_in_process` (`xtask`: the crate's sources against the audit's forbidden paths, and its licences); `docbook_reference_book_is_complete` (the reference book valid, every page and construct present, every `fileref` resolving, only the expected warnings, byte-for-byte deterministic with its pictures, against an `insta` snapshot); `docbook_hostile_input_is_safe` (markup, entity and `DOCTYPE` tricks as text, pictures outside the roots or reached by `..` never read, unsafe addresses never links, nothing left staged; a failure midway is DocBook 2/9's `docbook_failed_export_keeps_existing_output`); `docbook_large_book_is_bounded` (`tests/docbook_large.rs`, a binary of its own: 100 and 1,000 pages, each with sections, lists, notes, code with callouts, tables, footnotes and cross references and 25 pictures, the command and every pump under 3 s, 1,000 pages under 180 s and under 25 times the 100 pages' time plus 5 s, peak memory under 2 GB where measured; on the developer's machine, a debug build took 0.24 s and 2.24 s, the longest pump 0.12 s); and the fuzz target `docbook_write` (`fuzz/`), which writes any page and runs `check`.
