# PRD: AdocFoundry — Read-Only Exports from a Page Header

| Field | Value |
| --- | --- |
| Status | Proposed, 2026-11-17 (epic #685). The open decisions in §13 are settled in the work package they affect, and recorded in §14 |
| Date | 2026-11-17 |
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | An `:export-read-only:` attribute in a page's own header that makes its DOCX and PDF exports carry a real, format-supported editing restriction, refused clearly wherever a format cannot provide one |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Related work | Epic #685; work packages #697 to #701 (§11). DOCX export epic #613 and `docs/prd/PRD-AdocFoundry_DOCX_Export.md`; Inspect Attribute #99 and `docs/prd/PRD-AdocFoundry_Effective_Context_Inspector.md` |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `AGENTS.md`; `docs/prd/PRD-AdocFoundry_Phase_4_Implementation_Specification.md` (PDF export); `cargo xtask layers` |
| Existing code | `crates/adoc-engine/src/export.rs`, `profiles.rs`, `docx_settings.rs`, `html_export.rs`; `crates/adoc-lang/src/effective.rs`; `crates/adoc-core/src/document.rs` (`AttributeDeclaration`); `crates/adoc-dom/src/model.rs`; `crates/adoc-docx/src/parts.rs`, `body.rs`; `crates/adoc-pdf/src/export.rs`, `document.rs` |

## 1. Purpose and problem

Authors publish pages as Word and PDF documents that recipients should read but not edit: approved procedures, signed-off specifications, published manuals. Today every export is fully editable, and the only way to restrict one is to open it in Word or Acrobat afterwards, by hand, every time.

Let a page ask for it in its own header:

```asciidoc
= Architecture Guide
:export-read-only: true

This source remains editable in AdocFoundry.
```

Its DOCX and PDF exports then carry a genuine editing restriction that the format defines (Word's enforced document protection, the PDF Standard Security Handler's permissions), or the export fails with a clear, source-located reason. It is never a file name, a label, a file permission bit, an advisory banner or a "read-only recommended" flag.

This is a **publication policy, not a source-editing policy**. The `.adoc` source, the source and visual editors, undo, saving, Git and MCP edits are untouched; it has nothing to do with the visual editor's read-only nodes (`adoc-model`'s `ReadOnly`) or the source editor's `readonly` flag. And it is an editing deterrent, not DRM: Word protection without a password can be removed, and PDF permissions rely on the reader honouring them.

## 2. Goals

1. Read `:export-read-only:` from the exporting page's **own main header** only, never from the body, an included file, `antora.yml`, the playbook or a profile's attributes, with the source line of the declaration kept for diagnostics.
2. Decide from the **same text snapshot** the export renders, unsaved buffer included.
3. Write enforced Word protection (`w:documentProtection w:edit="readOnly" w:enforcement="1"`) for a protected article or an all-protected book.
4. For a Word book mixing protected and editable pages, leave exactly the editable pages' content editable through permission ranges, or fail before writing.
5. Restrict modification of an ordinary PDF through an in-process Rust implementation of the PDF Standard Security Handler, once a feasibility gate proves it; until then, protected PDF exports fail clearly.
6. Refuse protection for PDF/A, for a PDF book mixing protected and editable pages, and for every format with no restriction mechanism (HTML today; EPUB, Kindle and DocBook when they exist), before any output is replaced.
7. Give the same decision and the same diagnostics through the desktop app, the CLI, the MCP server and publication profiles.
8. Leave exports of pages without the attribute, or with it `false`, byte-for-byte unchanged.

## 3. Non-goals

- Making the source read-only, locking the editor, or changing file permissions, Git or MCP write access.
- DRM, digital signatures, certification, irreversible protection, or password-to-open encryption.
- Per-page PDF permissions, encrypted PDF/A, or claiming protection for EPUB, Kindle, DocBook or HTML through metadata.
- A `--read-only` flag, settings key, profile key or UI toggle that sets or overrides the policy: the page header is its only source.
- A weaker "mark as final" mode as a fallback; if wanted, it is a separate feature.
- Running Word, LibreOffice, Acrobat, qpdf, Ghostscript, Java, Python, any converter or any cloud service during export.
- A new crate.

## 4. User workflows

**Protect a page.** Add `:export-read-only: true` under the page title and export to Word or PDF as usual. The Word document opens read-only in Word and LibreOffice; the PDF opens normally but the reader refuses edits.

**Protect a book.** Every page of a component version carries the attribute; Export Component to Word… gives a book protected throughout. If only some pages carry it, Word leaves the others editable (or the export says why it cannot), and PDF refuses with the list of protected and editable pages, suggesting separate exports.

**Publish in CI.** `adocfoundry-cli docx --profile manual` exits non-zero with the page and line when the header's value is invalid or the format cannot honour it; existing output is left as it was.

**An agent exports.** `export_docx` and `export_pdf` stay write-gated; a refusal comes back as a structured error with a stable code and source locations, never as a success with an unprotected file.

## 5. Functional requirements

### 5.1 The attribute

| Page header | Policy |
| --- | --- |
| Not present | Unspecified: today's behaviour |
| `:export-read-only: true` | Enabled |
| `:export-read-only:` | Enabled |
| `:export-read-only: false` | Disabled |
| `:export-read-only!:` | Disabled |
| Any other value | An export error naming the page and line |

- A typed value, such as `ExportReadOnly::{Unspecified, Disabled, Enabled}`, with the declaration's source location; Unspecified and Disabled both mean editable but are told apart in tests and diagnostics.
- Repeated declarations in one header: the last wins, as attribute entries do. Whether values are case-sensitive is settled in READONLY 1/5 and tested.
- A declaration in the body, in an included file (its header or body), in `antora.yml`, in the playbook or in a profile's attributes is not the page's policy. A partial included by a protected page is protected content of that page, not a page of its own.
- The decision is made before the output is written, from the snapshot the export parses.

### 5.2 Pages, books and formats

The policy belongs to a source page; the export plan's final, deduplicated page list (start page and navigation order included) decides the outcome.

| Selected pages | DOCX | Ordinary PDF | PDF/A | HTML, EPUB, Kindle, DocBook |
| --- | --- | --- | --- | --- |
| None enabled | Unchanged | Unchanged | Unchanged | Unchanged |
| All enabled (or an enabled article) | Whole-document Word protection | Whole-file permissions once READONLY 4/5 passes its gate; refused until then | Refused | Refused |
| Some enabled | Editable ranges for the other pages, once verified; refused until then | Refused: permissions apply to the whole file | Refused | Refused |
| Invalid value | Source-located error | Source-located error | Source-located error | Source-located error |

- In a mixed Word book, covers, the title page, the table of contents, part and chapter headings the book adds, and anything no page owns stay protected. The unit is the page's logical content, never Word's reflowed physical pages.
- A refusal names the conflicting pages, not only "cannot export".
- Every failure (invalid value, refusal, missing credential, writer failure, timeout) leaves the source and any existing output intact, through the export's existing `Commit`.

### 5.3 DOCX

- `word/settings.xml` gets `w:documentProtection` with `w:edit="readOnly"` and `w:enforcement="1"`, in schema order, beside the existing `w:evenAndOddHeaders`, `w:updateFields` and footnote properties. Never `w:writeProtection`, `w:readOnlyRecommended`, or a ZIP attribute in its place.
- Mixed books: `w:permStart`/`w:permEnd` pairs with unique ids (for example `w:edGrp="everyone"`) around the editable pages' content, never crossing or nesting illegally, never split by a section break in a way Word cannot read. Footnotes, headers and footers live in other parts; if their editability cannot be made right, the affected mixed books are refused.
- No password or hash is written in the first release; the limitation is documented.
- Unprotected output keeps its current bytes.

### 5.4 PDF

- `typst-pdf` 0.15.1 (through `krilla` 0.8.2) exposes no permissions in the options `adoc-pdf` builds today. READONLY 4/5 begins with a feasibility gate: a permissions option in the in-process writer, or a constrained in-memory rewrite of the finished PDF that adds a conformant Standard Security Handler (`/Encrypt` with `/Filter`, `/V`, `/R`, `/Length`, `/O`, `/U`, `/P` and crypt filters) without leaving unencrypted copies behind. No ad hoc byte substitution. If neither passes, protected PDF stays refused.
- Modification is disallowed; reading, and the permissions assistive technology needs, are kept deliberately. Links, fonts, images, bookmarks and tags survive.
- Owner credentials, if the chosen handler needs one to mean anything, arrive only through a transient secure path: never the source, a profile, settings, command-line arguments, logs or event JSON. A front end that cannot provide one securely gets a "credentials required" refusal.
- PDF/A (`A1b`, `A1a`, `A2a`), from the export or a profile, is refused: PDF/A forbids encryption. PDF/UA (`Ua1`) is allowed only once shown conformant with the chosen permissions.
- Protected output may differ run to run (encryption needs randomness); unprotected output stays deterministic.

### 5.5 Front ends

- The app, the CLI (`pdf`, `docx`, with `--profile`) and the MCP tools (`export_pdf`, `export_docx`, write-gated and root-confined) all take the engine's decision; none reads the header itself. HTML export (`export.html_page`) refuses a protected page like any other unsupported format.
- The app says, before exporting, when the pages selected ask for protection, and when the chosen format cannot provide it, naming the pages of a mixed book. The editors stay editable.
- The CLI exits non-zero on every refusal, keeping `--fail-on` as it is. The MCP server answers refusals with stable `ErrorCode`s (such as `export_protection_unsupported`, `export_protection_mixed`, `export_protection_pdfa_conflict`, `export_protection_invalid_header`, `export_protection_credentials_required`) and source locations; no tool schema gains a plaintext password.
- Profiles select pages and formats as now; a profile's `export-read-only` attribute is content, never the policy.

## 6. Existing code to reuse

| Code | What it gives | How read-only exports use it |
| --- | --- | --- |
| `crates/adoc-core/src/document.rs`, `crates/adoc-parser` | `AttributeDeclaration { name, value, range }` for every attribute entry of a page's own text, from the line parse. The unset forms keep their `!` in `name`; `value` is `None` both for the empty flag and for unset. The line parse has no notion of where the header ends | A candidate source for the page's declaration and its location, once bounded to the main header (§13) |
| `crates/adoc-lang/src/effective.rs` (#99) | `effective_attribute`, its `Step`s with `Origin` (`Playbook`, `Component`, `Antora`, `Declaration`) and, for a declaration from an included file, `via` (the page's `include::`); locations are `Location { path, range }` | Shows which declaration is the page's own (`Origin::Declaration` with no `via`); no second resolver |
| `crates/adoc-dom/src/model.rs` | `Dom { attributes, .. }`, a flat effective map; `SourceSpan { path, range, line, include_chain }` | The flat map cannot prove origin; `SourceSpan` is the location type diagnostics reuse |
| `crates/adoc-engine/src/export.rs` | `ExportTarget`, `Plan` and `paths()`, `Request::{Pdf, Docx}`, `Stage`, `Written { sources, arranged, unread }`, `export_views`, `export_ready`, the engine's `write_pdf` and `write_docx`, `Commit::replace`, `theme_source` (which already reads the page's `pdf-theme`) | The policy rides on each planned page; the preflight runs in `export_ready` before a writer starts; no second planner |
| `crates/adoc-engine/src/profiles.rs`, `docx_settings.rs` | Profiles and their attributes (`profile_attributes`) | Unchanged; their attributes never become the policy |
| `crates/adoc-engine/src/html_export.rs` | `export.html_page` | Refuses a protected page |
| `crates/adoc-docx/src/lib.rs`, `parts.rs`, `body.rs`, `package.rs`, `testing` | `DocxOptions`, `DocxDocument::{Article, Book}`, `write_docx`; `parts::settings(footnotes, odd_even, update_fields)`; `body::Writer`; the package checks (`assert_valid_package`) | A typed protection option; settings in schema order; page boundaries for permission ranges; structural tests |
| `crates/adoc-pdf/src/document.rs`, `export.rs` | `PdfOptions`, `PdfStandard::{A1b, A1a, A2a, Ua1}`; the private `write_pdf` building `typst_pdf::PdfOptions` | A typed protection option distinct from standards; the gate's starting point |
| `crates/adoc-mcp/src/responses.rs` | `ErrorCode`, `Failure` | New stable refusal codes |

## 7. Architecture

```text
L4  adocfoundry / adocfoundry-cli / adoc-mcp   show the decision and its diagnostics
L3  adoc-engine   per-page policy on the shared plan; preflight against the format's
                  capability before any writer starts; typed protection to the writers
L2  adoc-docx     enforced documentProtection; permission ranges at page boundaries
L2  adoc-pdf      Standard Security Handler permissions, behind the feasibility gate
L1  adoc-lang / adoc-parser / adoc-dom   the page's own header declaration, with its location
```

- No new crate and no new dependency direction. Any dependency the PDF gate adds is pinned exactly in `[workspace.dependencies]`, passes `cargo deny check`, and reaches no network or executable (`cargo xtask layers`); `aes` and `sha2` are already in the tree.
- The engine makes the one decision. Writers receive a typed protection request and either apply it or return an error; a requested boolean is never taken as proof of protection.
- The engine's `typst` and `docx` features stay independent; a Word-only build still compiles no Typst.

## 8. Headless contracts

Names are illustrative; READONLY 1/5 and 2/5 settle them and record the outcome in §14.

```rust
pub enum ExportReadOnly { Unspecified, Disabled, Enabled }

pub struct HeaderExportProtection {
    pub policy: ExportReadOnly,
    pub source: Option<SourceSpan>,      // or a path and byte range: see §13
}

pub enum HeaderProtectionError {
    InvalidValue { value: String, source: SourceSpan },
}

enum ResolvedProtection {
    Off,
    WholeDocument { requested_by: Vec<SourceSpan> },
    WordMixed { editable_pages: BTreeSet<PageKey> },
}

enum ProtectionError {
    UnsupportedFormat { format: ExportFormat, sources: Vec<SourceSpan> },
    MixedDocument { format: ExportFormat, enabled: Vec<PathBuf>, disabled: Vec<PathBuf> },
    IncompatibleStandard { standard: PdfStandard, sources: Vec<SourceSpan> },
    MissingCredentials { format: ExportFormat },
}

pub enum WordProtection {                 // adoc-docx, on DocxOptions
    None,
    ReadOnlyWholeDocument,
    ReadOnlyWithExceptions { editable_pages: BTreeSet<PageKey> },
}

pub enum PdfProtection {                  // adoc-pdf, on PdfOptions; fields set by the gate
    None,
    RestrictModification { /* secret material redacted from Debug and Display */ },
}
```

Page keys come from the shared book arrangement, never from matching headings or paths after formatting. `parts::settings` takes `&WordProtection` rather than another positional boolean.

## 9. Safety and reliability

Release gates:

- A DOCX reported as protected has enforced protection; one reported as successful for a protected page is never unprotected.
- A protected PDF is refused until the Rust security writer passes validation.
- A mixed policy never broadens or weakens what was asked: nothing editable is silently locked, nothing protected silently left editable.
- Every refusal and failure leaves existing output intact; protected exports go through the same atomic `Commit` and timeout handling as any other.
- No password or secret in sources, profiles, settings, arguments, logs, event JSON, `Debug` output or the exported file's metadata.
- Unprotected exports are unchanged, byte for byte where they are deterministic today.

DOCX can ship on its own after READONLY 3/5, with protected PDF refused clearly until READONLY 4/5's gate passes or the scope is changed openly.

## 10. Tests and acceptance criteria

Each test below exists under its name when its work package is done. Names may be refined by the package that writes them, and the change recorded in §14.

| # | Test | Package | What it shows |
| --- | --- | --- | --- |
| 1 | `readonly_header_values` | R1 | Omitted, `true`, the empty flag, `false`, `!`, invalid values, whitespace and CRLF give the right policy or a located error |
| 2 | `readonly_header_duplicate_and_unset` | R1 | Repeated and unset declarations in one header resolve as attribute entries do |
| 3 | `readonly_ignores_body_and_includes` | R1 | A body declaration, an included file's declaration, `antora.yml`, the playbook and profile attributes never set the policy; a protected page stays protected whatever a partial says |
| 4 | `readonly_uses_unsaved_snapshot` | R1 | An unsaved `true` is honoured over a `false` on disk |
| 5 | `protection_article_enabled_disabled` | R2 | Single-page decisions are right |
| 6 | `protection_book_all_enabled` | R2 | Only an all-enabled book becomes a whole-document request |
| 7 | `protection_pdf_book_mixed_fails` | R2 | A mixed PDF book is refused, naming the pages, and replaces nothing |
| 8 | `protection_docx_mixed_fails_until_supported` | R2 | A mixed Word book is refused, naming the pages, until R3's ranges are verified |
| 9 | `protection_unsupported_format_fails` | R2 | HTML export of a protected page is refused and replaces nothing |
| 10 | `protection_profile_attributes_cannot_override` | R2 | A profile's `export-read-only` neither locks nor unlocks a page |
| 11 | `protection_pdfa_is_rejected` | R2 | Every PDF/A standard, from the export or a profile, refuses protection before writing |
| 12 | `protection_failed_export_preserves_existing_file` | R2 | Existing output bytes survive a refusal or a writer failure |
| 13 | `unprotected_export_unchanged` | R2 | Absent and `false` give today's PDF and DOCX bytes |
| 14 | `docx_readonly_settings_enforced` | R3 | One `w:documentProtection` with `readOnly` and enforcement, in schema order |
| 15 | `docx_readonly_preserves_relationships` | R3 | Media, styles, footnotes, headers, footers and links stay valid (`assert_valid_package`) |
| 16 | `docx_mixed_range_ids_paired` | R3 | Permission ranges pair one to one, around the editable pages only |
| 17 | `docx_mixed_does_not_unlock_front_matter` | R3 | Covers, title page, contents and book headings lie outside every range |
| 18 | `docx_mixed_complex_tables_and_notes` | R3 | Tables and footnotes in a mixed book behave as intended, or the book is refused with a reason |
| 19 | `pdf_protected_has_encrypt_dictionary` | R4 | `/Encrypt` and its entries for the chosen revision, with modification denied in `/P` |
| 20 | `pdf_protected_preserves_links_images_fonts` | R4 | A protected reference PDF keeps its catalog, links, images and tags, and reads back |
| 21 | `pdfua_with_protection_checked` | R4 | PDF/UA with protection is shown conformant, or refused |
| 22 | `protection_cli_exit_codes` | R5 | The CLI's exit status, diagnostics and preserved output for each row of §5.2 |
| 23 | `protection_mcp_errors` | R5 | The MCP tools' error codes, locations and write gate; no downgrade through a profile or extension |
| 24 | `protection_app_keeps_editors_editable` | R5 | The app reports the policy and refusals through the engine, and no editor turns read-only |

Manual checks that tests cannot make (Word on Windows and macOS, LibreOffice, Acrobat and the platforms' own PDF readers: opening without repair, edits refused where protected and allowed where not, links, accessibility tools) are recorded in `docs/qa/readonly.md` by READONLY 5/5.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check`.

## 11. Work packages

| WP | Deliverable | Depends on | Issue |
| --- | --- | --- | --- |
| R1 | The page's own header declaration of `:export-read-only:`, typed, with its location, from the export's snapshot | — | #697 (READONLY 1/5) |
| R2 | Per-page policy on the shared export plan; preflight against each format; refusals before any write | R1 | #698 (READONLY 2/5) |
| R3 | Enforced Word protection; permission ranges for mixed books | R1, R2 | #699 (READONLY 3/5) |
| R4 | PDF feasibility gate, then Standard Security Handler permissions; PDF/A refusal | R1, R2 | #700 (READONLY 4/5) |
| R5 | App, CLI, MCP and profiles; documentation and manual QA | R2; R3 and R4 for the full release | #701 (READONLY 5/5) |

Order: R1, then R2; R3 and R4 in parallel; R5 integrates DOCX as soon as R3 lands, with protected PDF refused until R4 passes its gate.

## 12. Definition of done

1. A protected page, and an all-protected book, export to DOCX with enforced protection that Word and LibreOffice honour without a repair prompt.
2. A mixed Word book leaves exactly its editable pages editable, or is refused before writing.
3. Protected PDFs carry verified native Rust permissions, or the gate's conclusion that they cannot is recorded and protected PDF stays refused with a clear message.
4. PDF/A, mixed PDF books, unsupported formats, invalid values and missing credentials each fail with their own tested message, leaving output intact.
5. The app, the CLI, the MCP server and profiles agree for the same pages; the source and editors are untouched; unprotected exports are unchanged.
6. The user documentation in `src/docs` explains the attribute, what each format does, and that this is a deterrent, not DRM; the manual checks are recorded in `docs/qa/readonly.md`.
7. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check` pass.

## 13. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's pull request and in §14.

- **Where the header declaration is read (R1).** Either:
  - the line parse's `AttributeDeclaration`s of the page's own text, with `adoc-lang`'s `effective.rs` for provenance (a page's own declaration is `Origin::Declaration` with no `via`). This reuses what #99 built and needs no parity between `AcdcParser` and the comparison-only `AsciidocParser`, but needs a main-header boundary the line parse does not have today, and locates by path and byte range (`Location`, converted to a line with `LineIndex`), since `adoc-lang` does not depend on `adoc-dom`; or
  - header provenance added to `adoc-dom`'s model at the parser boundary, located by `SourceSpan`, which knows the header from `AcdcParser`'s AST but must keep the unset forms the flat `Dom.attributes` map loses.

  Whichever is chosen reads the export's own snapshot; §8's location type follows the choice.
- **Case of values (R1).** Whether `TRUE` or `True` are accepted, following how Asciidoctor treats boolean attribute values.
- **Mixed Word books in the first release (R3).** Ship permission ranges, or refuse mixed books until they are proven across tables, footnotes, headers, footers and section breaks.
- **PDF route (R4).** A permissions option in the Typst writer or `krilla`, or an in-memory rewrite through a permissively licensed Rust PDF library; or none, keeping protected PDF refused. The Security Handler revision is chosen against the PDF version the writer emits (and the standards it may claim), avoiding RC4 where that version allows AES.
- **PDF owner credentials (R4, R5).** Whether a meaningful restriction needs an owner password; if so, how the app, the CLI and the MCP server take one transiently, and what a front end without a secure path does.
- **HTML export (R2).** Refuse a protected page, as the table in §5.2 says, or treat HTML as a rendering that carries no protection by design. Refusing is the default. `adocfoundry-cli render --output` is a development and fidelity tool, not a publication export, and is left as it is.

## 14. Implementation notes

Each work package records here where the implementation departs from this document and how the open decisions of §13 were settled.

### READONLY 1/5 (#697): the page's own header

- **Where the declaration is read (§13).** A scan of the page's own text, `adoc_parser::export_read_only`, reusing the line parse's `parse_attribute` for each entry and adding the one thing it lacked, the main header's end (`main_header_end`). Reading only the page's own text makes provenance true by construction: an included file, `antora.yml`, the playbook and a profile's attributes never reach it, so `adoc-lang`'s `effective.rs` is not needed. The export's parser (acdc) is the oracle for the boundary: `the_header_boundary_agrees_with_the_export_parser` checks the scan against the DOM's header attributes over sixteen header shapes. The header is read as acdc reads it: leading blank lines and comments, then the title (`= Title` or Markdown's `# Title`) with the entries, comments and preprocessor lines after it, the first other line as the author line (whatever it holds, `== Section` included) and the next as the revision line only when it reads as one (`v1.0`, `1.0, 2026-11-17: Draft`); with no title, the leading entries. `every_short_header_agrees_with_the_export_parser` checks every page top of up to three lines of thirteen kinds (title, Markdown title, underlined title, entries, blank, comment, comment block, author-like text, attribute list, anchor, block title, section and prose), with the declaration at every position and with and without a byte-order mark, both ways: whenever acdc reads the declaration as the header's, the scan finds it or refuses the page, and the scan never finds one acdc does not. Run once to four lines (about 28,000 shapes), it agreed too.
- **Types.** `ExportReadOnly::{Unspecified, Disabled, Enabled}` and `HeaderReadOnly { policy, range }` in `adoc-parser`; in the engine, `PageReadOnly { policy, source }` with `ReadOnlySource { path, line, range }` (a path, a one-based line and the declaration's byte range, since the scan has no `SourceSpan`), and `ReadOnlyHeaderError`, whose `Display` is `<path>:<line>: <reason>`.
- **Values (§13).** Asciidoctor gives attribute values no boolean meaning, so the attribute's values are its own: exactly `true` and `false`, or empty for true. `TRUE`, `True`, `yes`, `1` and `on` are errors, so a value that looks meant is never taken either way; this can be loosened later without breaking a page. Names compare ignoring case, as Asciidoctor's do. Both unset forms, `:export-read-only!:` and `:!export-read-only:`, are Disabled, and the last declaration in the header wins.
- **Failing closed.** Where a declaration's place cannot be read for certain it is an error at its line, never a guess, so a page that asks is never exported editable: inside `ifdef::`, `ifndef::` or `ifeval::` (block or one-line); after an `include::` in the header; between the header's end and the first content (`NotInHeader`, the common case being a blank line between the title and the entries); in a header with anything but comments above its title, or after an anchor or attribute list with no title (where acdc's reading varies); and in a page beginning with a byte-order mark (`ByteOrderMark`), which hides the header from acdc. An earlier invalid declaration is not excused by a later valid one.
- **The snapshot.** `Engine::export_read_only(path)` reads the text an export of the page renders, `parse_input_for_path` (the open buffer, unsaved edits included, else the file), behind the engine's `export` feature. READONLY 2/5 carries the same scan on each page the export parses.
- **Tests.** `readonly_header_values`, `readonly_header_duplicate_and_unset`, `readonly_ignores_body_and_includes` (the playbook's and the component's attributes, a body declaration, a partial's in the body and in the header, and a protected page including an editable partial; a profile's attributes are READONLY 2/5's test 10) and `readonly_uses_unsaved_snapshot`, in `crates/adoc-engine/tests/read_only.rs`; unit tests in `adoc-parser`'s `header.rs`, half-written headers included.

### READONLY 2/5 (#698): one decision per export, before anything is written

- **Where it is decided.** In `start_export_job`, from the very `OwnedInput` texts the parse worker then receives, before the worker starts: a refusal returns `EngineError::ExportProtection(ProtectionRefusal)` from the command (or the blocking export), so no worker runs, nothing is written and the existing output, untouched, needs no `Commit` to protect it. The engine's `protection.rs` holds the decision: `request` (each planned page's policy, in the plan's order, to `ProtectionRequest::{Off, WholeDocument, Mixed}`; the first page whose header gives no policy refuses) and `resolve` (against the format: PDF/A first, then a mixed document, then whether the format can carry a whole-document restriction), giving `ExportProtection::{Off, WholeDocument}`.
- **Typed refusals.** `ProtectionRefusal::{InvalidHeader, UnsupportedFormat, MixedDocument, IncompatibleStandard}`, each naming the pages (`ReadOnlySource`, path and line, for the pages that ask; paths for the editable ones), with a message that says what to do. `EngineError::ExportProtection` carries it, so front ends never parse a message; the MCP server maps it to `invalid_argument` (an invalid header) or `conflict` until READONLY 5/5 gives it its own codes.
- **No format protects yet.** `ExportFormat::protects_whole_document` is false for every format: Word's arrives with READONLY 3/5, the PDF's with READONLY 4/5's gate. Until then every export a page asks to protect is refused, and a mixed Word book is refused until READONLY 3/5's ranges are verified.
- **Formats.** `ExportFormat::{Pdf, PdfA, Docx, Html}`. A PDF claiming PDF/A-1b, 1a or 2a, from `[pdf] standards`, the export or a profile, is `PdfA` and refused with `IncompatibleStandard`; PDF/UA is decided with the PDF's protection in READONLY 4/5. The HTML export (`export.html_page`) decides from the open page's buffer, which it renders, and refuses a page that asks.
- **Entry points.** Every PDF and Word export goes through `start_export_job`: the page and component commands, the themed Word commands, profiles, the blocking exports the CLI and the MCP server use, and `export.pdf_pages`, which lists a protected page among its failures with the reason and writes the others. `pdf --typst` writes its Typst source only beside a PDF it writes, so a refused PDF writes none. The live PDF preview and `adocfoundry-cli render` are not exports and are unchanged.
- **Attributes that are not the page's.** A profile's `attributes` and an export's own `attributes` go to the parser as caller attributes, never into the text scanned, so they neither lock nor unlock a page (`protection_profile_attributes_cannot_override`).
- **Before exporting.** `Engine::export_protection(target, format)` makes the same decision from the pages the export would select as they stand, for READONLY 5/5's app to say before exporting.
- **Reporting.** `ExportSummary::protected`, false for every export until a format carries a restriction.
- **Tests.** `crates/adoc-engine/tests/protection.rs`: `protection_article_enabled_disabled`, `protection_book_all_enabled`, `protection_pdf_book_mixed_fails`, `protection_docx_mixed_fails_until_supported`, `protection_unsupported_format_fails`, `protection_profile_attributes_cannot_override`, `protection_pdfa_is_rejected`, `protection_failed_export_preserves_existing_file` (the blocking exports, the command and a batch of pages) and `unprotected_export_unchanged` (absent, `false` and unset give the same Word and PDF bytes; compared with `main`'s bytes once by hand, as the pull request says).

### READONLY 3/5 (#699): Word's enforced protection

- **Mixed Word books (§13), settled with the user: refused in the first release.** Permission ranges need Word itself to verify, across tables, footnotes, headers, footers and section breaks, and only the user can run Word. A book mixing pages that ask with pages that do not is refused before writing with `MixedDocument`, naming both, as READONLY 2/5 already does; the ranges can follow once tested in Word. Tests 16 to 18 (`docx_mixed_range_ids_paired`, `docx_mixed_does_not_unlock_front_matter`, `docx_mixed_complex_tables_and_notes`) are deferred with them; `protection_docx_mixed_fails_until_supported` holds the refusal.
- **`adoc-docx`.** `WordProtection::{None, ReadOnly}` on `DocxOptions::protection`, set only by the engine from its decision (`docx_options` writes `None`). `parts::settings` takes it as a typed argument and writes `<w:documentProtection w:edit="readOnly" w:enforcement="1"/>` after `w:embedTrueTypeFonts` and before `w:evenAndOddHeaders`, as `CT_Settings` orders them: never `w:writeProtection` or `w:readOnlyRecommended`, and no password, hash or salt. `DocxReport::protected` says the writer wrote it.
- **The engine.** Word now `protects_whole_document`. The export job keeps the decision; `write_docx` asks for `ReadOnly` for a whole-document decision and fails the export if the report does not say the document is protected, so a request is never taken as proof; `ExportSummary::protected` comes from the report. A PDF job that somehow held a decision would be failed before writing.
- **Checks.** `assert_valid_package` checks the settings' order, that protection is set at most once, read-only and enforced, and that neither look-alike is used; `the_package_checks_catch_damage` gains the three cases.
- **Fields.** A protected document still sets `w:updateFields` when it has a table of contents. Whether Word fills in the contents' page numbers on opening a protected document is one of the checks only Word can make (`docs/qa/readonly.md`, READONLY 5/5).
- **LibreOffice.** A protected page exported and opened in LibreOffice 26.8 (headless) converts to PDF, and saved back to Word keeps `w:documentProtection` read-only and enforced, which shows LibreOffice read it.
- **Tests.** `docx_readonly_settings_enforced` (`crates/adoc-docx/tests/protection.rs`: one enforced read-only element, no password, every other part byte for byte the editable document's) and `docx_readonly_preserves_relationships` (`crates/adoc-engine/tests/docx_export.rs`: the reference book with every page protected passes every structural check and differs from the editable book only in its settings). The READONLY 2/5 tests now expect Word to protect an enabled page or an all-enabled book.

### READONLY 5/5 (#701): the front ends, documentation and QA

Ahead of READONLY 4/5 in the stack, so the Word protection reaches every front end while the PDF's feasibility gate is decided; protected PDF stays refused until then.

- **App.** Export Page and Export Component to PDF, Word or HTML ask `Engine::export_protection` before the save dialog and, on a refusal, say why (naming the pages) and ask nothing more; the engine's decision when the export runs is the same one. A Word export's notification ends ", read-only" when `ExportSummary::protected` is set. The source and visual editors are untouched. `Engine::export_protection` reads `[pdf] standards` for a PDF, as the export does, so its PDF/A refusal reads the same.
- **CLI.** A refusal fails `docx` and `pdf` (with or without `--profile`) with exit status 1 and the reason, whatever `--fail-on` says, as every engine error already did; a protected export prints `protected: read-only` after its output line.
- **MCP.** Four stable codes in `ErrorCode`: `export_protection_unsupported`, `export_protection_mixed`, `export_protection_pdfa_conflict` and `export_protection_invalid_header`, from `responses::protection_code`. The export tools' `export_failure` adds root-relative details: `requested_by` (`{path, line}` of each page that asks), `editable` for a mixed book, `source` for an invalid header. `export_docx` and `export_pdf` answer `protected`, in their output schemas. `export_protection_credentials_required` is not needed: no front end is ever asked for a credential (READONLY 4/5 decides the PDF's owner key without one).
- **Documentation.** `src/docs/modules/ROOT/pages/read-only-exports.adoc`, with pointers from the Word and PDF export pages, the command line and the MCP server pages; the manual checks in `docs/qa/readonly.md`.
- **Tests.** `protection_cli_exit_codes` (`crates/adocfoundry-cli/tests/cli.rs`), `protection_mcp_errors` (`crates/adoc-mcp/tests/protection.rs`) and `protection_app_keeps_editors_editable` (`crates/adocfoundry/src/ui_tests.rs`).

## 15. References

- Epic #685.
- WordprocessingML document protection: <https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.wordprocessing.documentprotection>
- PDF/A and encryption: <https://pdfa.org/pdfa-faq/>
- ISO 32000-2, §7.6 (encryption and the Standard Security Handler).
