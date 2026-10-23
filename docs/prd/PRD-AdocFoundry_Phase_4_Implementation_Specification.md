# PRD: AdocFoundry — Phase 4 Implementation Specification

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Date | 2026-10-24 |
| Status | Implemented |
| Scope | Phase 4: PDF export with themes |
| Depends on | `docs/design/DESIGN-AdocFoundry_Architecture.md` (Phase 4 section), Phases 1 to 3 and the Git work as merged |

## 1. Document Purpose

Phase 4 turns pages and whole components into paginated PDF. Typst is the typesetter, compiled in process; AdocFoundry generates the Typst from the same view tree the preview and the HTML backend draw, and styles it from asciidoctor-pdf's YAML theme language, so existing themes carry over.

Phase 4 must provide:

- `adoc-pdf`: a theme loader for asciidoctor-pdf themes (inheritance, variables, arithmetic, units) producing a typed `Theme`; a Typst emitter for the view tree's block and inline set; a Typst `World` with bundled fonts; in-process compilation to PDF with errors mapped back to AsciiDoc source lines; running content, a title page, a table of contents and recto/verso parity; book assembly; an index; PDF/A-2a and PDF/UA-1.
- An engine export service: a page, or a component version in nav order, exported on a worker, with every page parsed, resolved and its diagrams rendered first.
- `adocfoundry-cli pdf`, and export commands in the app.
- A visual regression harness that renders reference pages with asciidoctor-pdf and with AdocFoundry and compares the rasterised pages.

## 2. Decisions

| Question | Decision | Consequence |
|---|---|---|
| Typesetter | **Typst 0.15.1** as a library: `typst`, `typst-layout`, `typst-pdf`, pinned exactly; no `typst-as-lib`, no `typst-kit` | The design's recommendation; the world is written in-house (about 100 lines), which keeps the API surface to what Typst itself guarantees. |
| Emitter input | the **view tree** (`adoc_render::ViewTree`), not the raw DOM | References, images, diagrams, numbering, captions and footnotes are already resolved, exactly as the preview and the HTML backend show them; one resolution path for every output. `adoc-pdf` depends on `adoc-render`, a same-layer pair added to `SAME_LAYER_ALLOWED`. |
| What the world can reach | the generated source, the theme's fonts and the files the document uses, each at a fixed virtual path; **no packages, no network, no file system** | A document cannot make Typst read anything else. Typst interns file ids for the life of the process, so virtual paths are derived from the real path to stay stable between exports. |
| Typst bundled fonts | not used (`typst-assets` fonts stay off) | Output depends only on the theme's fonts. |
| Bundled fonts | the subset Noto Serif, Noto Sans and M+ 1mn that asciidoctor-pdf ships (Apache-2.0; M+ under its own permissive licence), embedded in `adoc-pdf` | The bundled themes render with the same fonts as asciidoctor-pdf, everywhere. About 1.9 MB. |
| Bundled themes | asciidoctor-pdf 2.3's `base`, `default`, `default-sans` and `default-for-print` YAML files, unmodified (MIT) | `extends: default` and the other bundled names resolve as they do in asciidoctor-pdf. |
| Theme semantics | asciidoctor-pdf 2.3's theme loader, reproduced: key flattening, `-` to `_`, `extends` (bundled names, `./` relative paths, lists, `!important`), `$variable` references, the arithmetic it evaluates, measurement units, colour normalisation, `font.catalog` with `merge`, `font.fallbacks`, `GEM_FONTS_DIR` | A theme file written for asciidoctor-pdf loads without edits. Keys the converter here does not use are reported as warnings, not errors. |
| Source highlighting | Typst's own `raw` highlighting, given the block's language | The tree-sitter highlighter lives in the app crate (it comes with gpui-component), which a library crate cannot use. The design's "same tree-sitter grammars" is deferred; see §11. |
| Index | in-house: index terms from the DOM become Typst `metadata`, and the index is generated with `query` and page numbers | No Typst packages are reachable, by decision. |
| Output | PDF 1.7 by default; PDF/A-2a and PDF/UA-1 on request, together or apart; tagged PDF always | As the design; Typst 0.15 enforces both standards and reports what a document lacks. |
| Book | a component version's navigation becomes a book: parts from nav files (or titled nav lists) when there is more than one, chapters from top-level entries, sections from nested entries, pages shifted down by their nav depth | As the design's document assembly, with the start page first as a preface when the nav does not list it. |
| Cross references in a book | to a page in the book: an internal link; to a page outside it: a link to the configured site URL, or plain text without one | As the design. |
| Where export runs | the engine, on a worker; compilation and writing the file happen off the engine thread | The app stays responsive; the CLI waits for the same job. |

## 3. Architecture

```text
L4  adocfoundry (export commands)   adocfoundry-cli (pdf)   adoc-ls
L3  adoc-engine   export service: pages → parse → views → diagrams → adoc-pdf → file
L2  adoc-pdf (theme, emitter, world, compile, book, index) → adoc-render   adoc-diagrams   adoc-model   adoc-git
L1  adoc-dom (index terms)   adoc-parser   adoc-antora   adoc-index   adoc-lang
L0  adoc-core
```

`adoc-pdf` depends on `adoc-core`, `adoc-render`, `typst`, `typst-layout`, `typst-pdf`, `serde`, `serde-saphyr` and `regex`. It knows nothing of the engine, the workspace or a UI framework: it is given view trees, a theme and options, and returns PDF bytes and diagnostics.

Data flow of an export: a front end invokes `export.pdf` with a target (the active page, or a component version) and an output path → the engine lists the pages (one, or the nav order) and parses each on a worker from its open buffer or the file → the views are built on the engine thread and their diagrams rendered → once no diagram is pending, the views and the theme go to a worker, which emits Typst, compiles it, writes the PDF and reports → `EngineEvent::ExportFinished` carries the path, the page count and any diagnostics, or the error.

## 4. Themes (WP 4.1)

```rust
pub enum ThemeSource { Bundled(&'static str), File(PathBuf) }
pub fn load_theme(source: &ThemeSource) -> Result<LoadedTheme, ThemeError>;

pub struct LoadedTheme {
    pub keys: ThemeKeys,             // the flattened, evaluated key map, in definition order
    pub theme: Theme,                // the typed view of the keys the converter uses
    pub fonts: FontCatalog,          // family → style → font file (bundled or a path)
    pub warnings: Vec<ThemeWarning>, // unknown keys, unknown variables, unreadable fonts
}
pub enum ThemeValue { Number(f64), Text(String), Color(Color), Length(Length), List(Vec<ThemeValue>), Null }
pub enum Length { Pt(f64), Em(f64), Rem(f64), Percent(f64), Vw(f64), Vh(f64) }
pub enum Color { Rgb([u8; 3]), Cmyk([f64; 4]), Transparent }
```

Loading follows asciidoctor-pdf 2.3's `ThemeLoader`:

- A theme starts from `base_font_size: 12`; `base` is never implied, only `extends: base` loads it.
- `extends` takes a name, a path or a list, applied in order before the file's own keys; bundled names resolve to the bundled files, `./x.yml` to the extending file's directory, other paths to the theme directory; ` !important` forces a reload.
- Keys flatten with `_` (`heading: h2: font-size` → `heading_h2_font_size`), `-` becomes `_` except under `role`, deprecated categories (`blockquote`, `key`, `literal`, `outline_list`) and keys (`*_align` → `*_text_align`) are renamed with a warning.
- Values: `$name` references to keys already defined; measurements (`0.5in`, `15mm`, `2cm`, `12px`, `1pc`) converted to points inside expressions; `*`, `/`, `^`, `+`, `-` evaluated left to right with the precedence asciidoctor-pdf gives them; `round()`, `floor()`, `ceil()`; `em`, `rem`, `%`, `vw` and `vh` kept as relative lengths for the converter.
- Colours: hex strings with or without `#`, three-digit shorthand, numbers coerced to six digits, `[r, g, b]`, `[c, m, y, k]` (percentages or fractions), `transparent`, `~` for none. Hex values in themes outside the bundled directory are quoted before YAML parsing, as asciidoctor-pdf does, so `000000` stays a colour.
- `font.catalog` maps family names to `normal`, `bold`, `italic`, `bold_italic` (or `regular`, or one file for all four); `merge: true` keeps the inherited catalogue; `GEM_FONTS_DIR` is the bundled fonts; relative paths resolve against the theme directory; `font.fallbacks` lists families tried for missing glyphs.

The typed `Theme` holds the keys the converter reads (§6) with asciidoctor-pdf's fallbacks applied; everything it does not hold is listed in `warnings` as unknown.

Acceptance tests: `loads_every_bundled_theme`, `default_theme_values_match_asciidoctor_pdf` (a table of keys and values computed by asciidoctor-pdf's loader for `default`, `default-sans` and `default-for-print`), `extends_applies_in_order`, `variables_and_arithmetic_evaluate`, `units_normalise_to_points`, `colours_normalise`, `font_catalogue_merges_and_resolves`, `unknown_keys_warn_and_load`, `custom_theme_loads_without_edits` (a fixture theme written for asciidoctor-pdf, with relative fonts and images), `malformed_theme_reports_an_error`.

## 5. Emitter (WP 4.2)

```rust
pub struct EmitOptions<'a> { pub theme: &'a Theme, pub book: Option<&'a BookPlan>, pub site_url: Option<&'a str> }
pub struct Emitted {
    pub source: String,                   // the generated Typst
    pub files: Vec<(String, PathBuf)>,    // virtual path → image or diagram file
    pub map: SourceMap,                   // generated byte range → the AsciiDoc span it came from
}
pub fn emit(pages: &[PageView], options: &EmitOptions) -> Emitted;
```

Every view block and inline has a Typst form; the generated source uses only Typst markup and calls to functions the template defines (`adoc-admonition`, `adoc-code`, `adoc-sidebar`, …), so styling lives in one preamble generated from the theme.

- **Escaping**: text is emitted in markup mode with every character Typst gives meaning (`\ * _ ` # $ @ < > [ ] ~ = - + / ' "` at line starts and in context) escaped, or as a string literal where a function takes text. Nothing from a document is ever emitted as Typst code: passthrough content is shown as text.
- **Labels**: anchors become labels namespaced by page (`<p3:install>`), so two pages with the same section id do not collide; cross references become `link(<label>)` or `link("url")`.
- **Blocks**: sections and discrete headings, paragraphs (with roles), lists (ordered with their numbering style, unordered, checklists, nested), description lists (including `horizontal` and `qanda`), callout lists and code callouts, admonitions, listing and source blocks, literal blocks, quotes and verses with attribution, sidebars, examples, open blocks, tables (column widths, alignment, header and footer rows, spans, cell styles), block images and diagrams (SVG), STEM (shown as source in a code style; see §11), thematic and page breaks, the `toc::[]` macro, and unsupported blocks shown as their source.
- **Inlines**: strong, emphasis, monospace, mark, superscript, subscript, roles (`underline`, `line-through`, `big`, `small` and theme roles), links, cross references, anchors, inline images, icons as text, footnotes (Typst footnotes), line breaks, `kbd`, `btn`, `menu`, inline STEM as source, index terms.

Acceptance tests: `emits_every_block_kind`, `emits_every_inline_kind`, `escapes_typst_markup` (a corpus of strings full of Typst syntax compiles, and the PDF's text equals the input), `text_round_trips_through_compilation` (every render fixture's plain text survives emit and compile), `labels_are_namespaced_per_page`, `passthrough_is_text`.

## 6. World, compilation and errors (WP 4.3)

```rust
pub struct PdfOptions { pub standards: Vec<PdfStandard>, pub title: Option<String>, pub today: Option<(i32, u8, u8)> }
pub enum PdfStandard { A2a, Ua1 }
pub struct PdfOutput { pub bytes: Vec<u8>, pub pages: usize, pub diagnostics: Vec<PdfDiagnostic> }
pub struct PdfDiagnostic { pub severity: Severity, pub message: String, pub hints: Vec<String>, pub source: Option<SourceSpan> }
pub fn export(pages: &[PageView], theme: &LoadedTheme, options: &PdfOptions) -> Result<PdfOutput, PdfError>;
pub fn to_typst(pages: &[PageView], theme: &LoadedTheme, options: &PdfOptions) -> Emitted;   // for inspection
```

The world serves the main source, the theme's fonts (bundled fonts parsed once per process) and the emitted files. Typst's errors and warnings carry spans in the generated source; `SourceMap` maps each to the innermost AsciiDoc span that produced it, so a diagnostic reads `install.adoc:42: unknown image format`. An error with no mapping is reported against the page.

Acceptance tests: `compiles_with_bundled_fonts_only`, `world_reaches_only_listed_files`, `errors_map_to_asciidoc_lines`, `missing_image_is_a_diagnostic_not_a_failure`, `compile_is_deterministic` (same input, same bytes, given a fixed date).

## 7. Running content, title page, TOC and parity (WP 4.4)

The preamble is generated from the theme:

| Theme keys | Typst |
|---|---|
| `page_size`, `page_layout`, `page_margin`, `page_margin_inner`, `page_margin_outer`, `page_background_color` | `set page(paper or width/height, flipped, margin: (top, bottom, inside, outside), fill)`; inner and outer only for `media=prepress` |
| `base_*` | `set text(font, size, fill, weight, style)`, `set par(leading, justify, spacing)` from `base_line_height` and `prose_margin_bottom` |
| `heading_*`, `heading_h<n>_*` | `show heading.where(level: n)` with font, size, colour, style, alignment, `margin_top`, `margin_bottom` |
| `title_page_*` | a title page before the body: title, subtitle, authors and revision, each with its `top` and margins, a logo image; shown for the book doctype or when `title-page` is set |
| `toc_*` | `outline()` with indentation per level, `toc_dot_leader_*`, `toc_line_height`, levels from `toclevels`; its own page in a book |
| `header_*`, `footer_*`, `running_content_start_at` | `set page(header: context …, footer: context …)`: a three-column grid per side, recto or verso from the page number's parity, `{page-number}`, `{page-count}`, `{document-title}`, `{section-title}`, `{chapter-title}`, `{part-title}`, `{section-or-chapter-title}` and document attributes substituted; `height`, `padding`, `border_*`, `vertical_align`, `font_*` |
| `admonition_*`, `code_*`, `codespan_*`, `table_*`, `list_*`, `description_list_*`, `quote_*`, `verse_*`, `sidebar_*`, `example_*`, `caption_*`, `image_*`, `thematic_break_*`, `link_*`, `mark_*`, `kbd_*`, `button_*`, `menu_*`, `footnotes_*`, `role_*`, `block_*`, `conum_*` | the template functions the emitter calls |

Article doctype: the document title is an `h1` on the first page, the body follows; running content starts on the first page (or `running_content_start_at`). Book doctype: title page, TOC, then parts and chapters, each chapter on a new recto page in `media=prepress` and a new page otherwise; running content starts after the title page.

Acceptance tests: `header_and_footer_follow_parity`, `running_content_starts_where_the_theme_says`, `title_page_shows_title_authors_and_revision`, `toc_lists_sections_to_toclevels`, `page_size_and_margins_follow_the_theme`, `attribute_references_in_running_content_resolve`.

## 8. Book assembly (WP 4.5)

```rust
pub struct PageView { pub key: String, pub view: ViewTree, pub depth: u8 }   // depth 0: an article
pub struct BookPlan { pub title: String, pub subtitle: Option<String>, pub version: Option<String>, pub parts: Vec<BookPart> }
pub struct BookPart { pub title: Option<String>, pub entries: Vec<BookEntry> }
pub enum BookEntry { Page { key: String, path: PathBuf, children: Vec<BookEntry> }, Heading { title: String, children: Vec<BookEntry> } }
```

The engine builds the plan from `Engine::nav_trees`: one part per nav file when there are several (titled by the nav list's title, or none), entries in nav order, an entry without a page becoming a heading at its depth, each page once (the first time the nav lists it). A page at nav depth `d` becomes a section at level `d` whose title is the page title; its own sections move down by `d`. The book's title is the component version's `title` (or name) and its version, from `antora.yml`; `pdf-title` in the playbook's or component's attributes overrides it.

Engine:

```rust
pub enum ExportTarget { Page(DocumentId), Component { name: String, version: Option<String> } }
CommandArgs::Export(ExportArgs { target: ExportTarget, output: PathBuf, theme: Option<PathBuf>, standards: Vec<PdfStandard> })
EngineEvent::ExportFinished { output: PathBuf, result: Result<ExportSummary, String> }
pub struct ExportSummary { pub pages: usize, pub diagnostics: Vec<Diagnostic> }
impl Engine {
    pub fn export_blocking(&mut self, args: ExportArgs, timeout: Duration) -> Result<ExportSummary, EngineError>;
    pub fn is_exporting(&self) -> bool;
}
```

Commands: `export.pdf_page` (Export Page to PDF…), `export.pdf_component` (Export Component to PDF…). Settings: `[pdf] theme = "default"` (a bundled name or a path, relative to the workspace root), `site_url` for cross references out of the book, `standards = []`.

Acceptance tests (engine): `exports_one_page`, `exports_a_component_in_nav_order`, `nav_depth_shifts_section_levels`, `xrefs_between_book_pages_are_internal`, `xrefs_out_of_the_book_use_the_site_url`, `diagrams_render_before_compiling`, `export_runs_off_the_engine_thread`, `unsaved_buffers_are_exported`, `theme_setting_selects_the_theme`.

CLI: `adocfoundry-cli pdf <file|component-dir> [--root <dir>] [--component [<version>@]<name>] [--theme <name|file.yml>] [--fonts-dir <dir>] [--output <file.pdf>] [--standard pdf-a-2a|pdf-ua-1] [--typst <file.typ>]`. A file exports that page; `--component` exports the book; `--typst` also writes the generated Typst for inspection. Diagnostics print as `path:line: message`; the exit code is 1 when the export fails. Tests in `tests/cli.rs`: `pdf_exports_a_page`, `pdf_exports_a_component`, `pdf_reports_theme_errors`.

App: both commands in the palette and the File menu, a save dialog for the output path (suggested from the page or component name), the status bar showing "Exporting PDF…" while the job runs, and a notification with the result and an "Open" action. App test: `export_command_writes_a_pdf`.

## 9. Index, PDF/A and PDF/UA (WP 4.6)

- `adoc-dom` gains `Inline::IndexTerm { terms: Vec<String>, visible: Vec<Inline> }` (visible content empty for concealed terms) from acdc's `IndexTerm`, and `adoc-render` `ViewInline::IndexTerm`. The preview, the visual editor's check and the HTML backend show the visible content as before.
- The emitter writes each term as `metadata` with a label, and a section with the `index` style (`[index]` `== Index`) becomes the index: terms grouped by first letter, then primary, secondary and tertiary, each with its page numbers as links, in `index_column_count` columns with `index_column_gap`.
- `PdfStandard::A2a` and `Ua1` pass to `typst-pdf`; the document's title comes from the doctitle (or the book title), its language from `lang`, its author and keywords from the attributes. A document Typst cannot make conformant fails with Typst's reasons, mapped to source lines.

Acceptance tests: `index_terms_reach_the_dom`, `index_lists_terms_with_pages`, `pdf_a_2a_export_is_conformant` (Typst's validator passes and the output declares PDF/A-2a), `pdf_ua_1_requires_alt_text`, `metadata_comes_from_attributes`.

## 10. Visual regression (WP 4.7)

`tests/fixtures/pdf/` holds reference documents: an article exercising every block kind, a page of running content and parity, and a small component for the book. `crates/adocfoundry-cli/tests/pdf_regression.rs` renders each with `asciidoctor-pdf` (skipped, as the fidelity harness is, when it is not on `PATH`) and with `adocfoundry-cli pdf`, rasterises both at 40 dpi with `hayro`, and compares them:

- the page counts are equal (books: within one page);
- per page, after blurring both rasters, the mean absolute grey difference is below a threshold, and the bounding boxes of the ink on each page agree to within a few points (the text block lands in the same place).

Failing pages are written as PNG pairs under `target/pdf-regression/` for inspection. Thresholds are set from the first passing run and recorded in the test. The manual check (side by side at reading distance) is in `docs/qa/phase4.md`.

## 11. Out of Scope in Phase 4, and Implementation Notes

Not in Phase 4: STEM typeset as mathematics (shown as its source in the code style; Typst math from LaTeX is a separate converter), source highlighting through tree-sitter (Typst's highlighter is used), a live Typst preview pane, PDF/A-1 and PDF/UA-2, cover images from `front-cover-image` beyond a full-page image, and asciidoctor-pdf's extended converter API (Ruby).

Deviations found while implementing are recorded here, as in Phase 3 §11.

| Area | Specification | Implementation | Why |
|---|---|---|---|
| Layout of assets | `themes/` and `fonts/` at the repository root (design) | `crates/adoc-pdf/themes` and `crates/adoc-pdf/fonts`, embedded with `include_str!`/`include_bytes!` | The crate stays self-contained when packaged; nothing reads them from disk. |
| Bundled themes | `base`, `default`, `default-sans`, `default-for-print` | as specified; asciidoctor-pdf's `*-with-font-fallbacks` names load as the theme they extend, with a warning that the fallback fonts are not bundled | The M+ 1p fallback and Noto Emoji add 1.9 MB; a theme that extends them still loads without edits. |
| Typed `Theme` | a struct of the keys the converter reads | the flattened keys are passed to the template as a Typst dictionary under asciidoctor-pdf's own names, with values typed (lengths in points, colours as colours); fallbacks are resolved in the template with `pick` | One name for each setting from the YAML to the template; a key asciidoctor-pdf reads is a key the template can read. |
| Unknown keys | a warning per unknown key | a warning per unknown top-level category (`brand`, `mystery`), the keys kept for variables | Themes commonly keep their own variables in categories of their own; a warning per key would be noise. |
| Spacing | Typst's block spacing | Prawn's model, reproduced: block spacing is zero and the body writes each block's bottom margin after it as `#v(...)`; heading tops are weak spacing (dropped at the top of a page); every paragraph gets half its leading above and below; line heights add the font's `hhea.lineGap`; superscripts and subscripts are zero-height boxes; Typst's own heading size, weight, spacing and stickiness are switched off | Typst collapses adjacent spacing to the larger value and spaces lines differently; with this model the reference article lands within 1.7 pt of asciidoctor-pdf on every line, and negative theme margins work. |
| Keeping blocks together | — | admonitions, code, quotes, sidebars, examples and captioned blocks are unbreakable when they fit on a page, measured at the page's content width; `heading_min_height_after` moves a heading that has too little room below it | asciidoctor-pdf's `arrange_block` and `arrange_heading`, which decide where pages break. `layout()` would measure the exact width, but links laid out inside it lose their place in a tagged PDF (a typst-pdf internal error). |
| Paragraph half-leading | — | spacing around the paragraph in its show rule, not a block | Wrapping a paragraph in a block from its own show rule also cut its links off from the tag tree. |
| Line breaking | — | greedy (`linebreaks: "simple"`) and spaces only stretch (`justification-limits`) | Prawn fills lines first-fit and never shrinks spaces. |
| Flowing text | — | runs of spaces collapse to one | Prawn normalises them; Typst string literals keep them. |
| Lead paragraph | — | the first paragraph of an article's preamble gets the `lead` role, styled only when the theme sets `role_lead_font_size` | As asciidoctor-pdf's `convert_preamble`. |
| Footnotes | Typst footnotes | collected and placed at the bottom of the last page of the article or chapter, `[n]` references | Where asciidoctor-pdf puts them. |
| Running content | `{page-number}` from Typst's page counter | counted from the page holding the body's start marker | A counter update at the start of a page comes after that page's background is laid out. |
| Running content markup | substitutions as Asciidoctor applies them | attribute references (unknown ones drop the line), constrained `*`, `_`, `` ` ``, character references and a lone `image:` macro; widths are parsed numbers | The forms themes use; nothing from a theme or document is written as Typst code (a security review found an image width written as raw text, fixed). |
| Titles | — | the article's title and the title page's title are level 1 headings in the PDF's structure, styled by their callers | PDF/UA-1 requires the first heading to be level 1. |
| Image alt text | — | an image without alt text gets its file name, as Asciidoctor does | Every image carries a text alternative for PDF/UA. |
| Source highlighting | tree-sitter | Typst's highlighter, only when `source-highlighter` is set | As asciidoctor-pdf, which highlights only then; the tree-sitter highlighter lives in the app crate. |
| PDF/A | — | a document date is written when a standard is requested: the given one, else today's (UTC) | PDF/A requires it; an ordinary export stays undated so the same input gives the same bytes. |
| `lang` | — | split into Typst's language and region (`en-GB` → `en`, `GB`) | Typst rejects a region in `lang`. |
| Book plan | parts from nav files or titled lists when there are several | as specified; an untargeted nav entry is a heading at its depth; each page appears once; the start page leads as a preface unless the nav lists it; a list title that links to a page (`.xref:index.adoc[Architecture]`) titles the part and its page leads it; a page with no title takes its entry's text | Every nav file of a real project titles its list with a link to the module's index page. |
| Unreadable pages | — | a page the parser rejects is shown as its source, and the export warns with the parser's message | Without the warning the page's source appeared in the book unexplained. |
| Typst's cache | — | emptied after each compile | Typst memoises layout in a process-wide cache that nothing else empties; after a 2,000-page book it held about 11 GB, which the app would keep. comemo 0.5 keeps the capacity of the tables it empties (call-tree slabs, edge maps, per-call constraint tables), so a long-running process keeps its peak, 6 to 8 MB a PDF page, which the next export reuses; freeing it needs typesetting in a child process or a change to comemo. |
| Lists | — | Typst's `enum`, each item's marker given as its number in a box as wide as the indent | A `grid` is tagged `Div`; the enum is tagged `L`, `LI`, `Lbl` and `LBody`, so a screen reader announces a list, with no line moved. Description lists are still blocks, tagged `Div`. |
| CLI | `--component <name>[@<version>]` | `--component [<version>@]<name>`, as Antora writes coordinates; also `--fonts-dir`, the counterpart of `-a pdf-fontsdir`; for a component directory the workspace is its parent, so its sibling components resolve | Matches resource ids; real projects (the user's documentation among them) pass a fonts directory. |
| App | the palette and the File menu | the palette | The app has no menu bar. |
| Advisories | — | `deny.toml` ignores RUSTSEC-2026-0194 and -0195 (quick-xml 0.38 through typst-library → hayagriva → citationberg) | Reached only by Typst's `bibliography()`, which generated Typst never calls; citationberg 0.7.0, the latest, still needs quick-xml 0.38. |
| Visual regression | books within one page | books are not compared | asciidoctor-pdf cannot assemble an Antora component; the book is covered by the engine's structural tests. |
| Visual regression thresholds | set from the first passing run | mean grey difference at most 0.02 (the first run's worst page: 0.0085) and ink edges within 6 pt (first run: 1.8 pt, one pixel at 40 dpi) | — |
| Index | `index_*` keys | letters as category headings, terms and subterms with their pages, in `index_column_count` columns | The index is built with `query` over `metadata` markers, as no Typst package is reachable. |

## 12. Implementation Sequence

| # | Package | Depends on |
|---|---|---|
| 4.1 | Theme loader with inheritance, variables, arithmetic, units; tests against the bundled asciidoctor-pdf themes | Phase 2 |
| 4.2 | View tree → Typst emitter for the block set, with escaping and round-trip tests | 2.1 |
| 4.3 | Typst `World` with bundled fonts, in-process compile, error mapping back to AsciiDoc lines | 4.2 |
| 4.4 | Running content, title page, TOC, parity | 4.1, 4.3 |
| 4.5 | Multi-page book assembly from nav; engine export service, CLI and app commands | 4.4 |
| 4.6 | Index, PDF/A, PDF/UA | 4.5 |
| 4.7 | Visual regression: render reference pages with asciidoctor-pdf and compare rasterised pages | 4.4 |

## 13. Definition of Done for Phase 4

- The `default` theme produces a PDF of the reference article that a reader cannot tell apart from asciidoctor-pdf's at reading distance (the regression harness passes, and the side-by-side check in `docs/qa/phase4.md` is recorded).
- A custom theme written for asciidoctor-pdf loads without edits (`custom_theme_loads_without_edits`).
- A page and a whole component export from the CLI and from the app, with a TOC, running content, an index when the document has one, and PDF/A-2a and PDF/UA-1 on request.
- README, the documentation site and `docs/qa/phase4.md` describe PDF export, themes and what is not supported.
