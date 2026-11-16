# PRD: AdocFoundry — Word Export Looks: PDF Themes, Word Templates or None

| Field | Value |
| --- | --- |
| Status | Approved for implementation, 2026-11-16, with the builder check's questions settled in §12 |
| Date | 2026-11-16 |
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Choosing how a Word export looks: like the PDF (its asciidoctor-pdf theme mapped onto Word), from a named Word template, or with no theme; and embedding the fonts the look names |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Related work | Epic #668; work packages #669 to #675 (§10). DOCX export epic #613 (done) and its spec `docs/prd/PRD-AdocFoundry_DOCX_Export.md`, whose §3 and §13 decisions this one revises (§3 below); #667 (pictures fit the page) |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `AGENTS.md`; `cargo xtask layers` |
| Existing code | `crates/adoc-pdf/src/theme/` (the theme loader), `crates/adoc-pdf/src/running.rs` (running content), `crates/adoc-pdf/src/fonts.rs` and `fonts/` (bundled fonts), `crates/adoc-pdf/themes/` (bundled themes); `crates/adoc-docx` (the writer, `StyleOverrides`, `Running`, `Margins`); `crates/adoc-engine/src/export.rs` (`theme_source`, `docx_options`), `src/docx_settings.rs`, `src/profiles.rs` |

## 1. Purpose and problem

A Word export looks nothing like the PDF of the same book. A team that publishes PDFs styled by an asciidoctor-pdf theme (its fonts, colours, heading sizes, title page with a logo, headers and footers with logos and page numbers, page size and margins) gets a Word document in Word's own look, styled only by the handful of `[docx]` settings. Teams that already have a corporate Word template cannot use it either.

Give every Word export a **look**, chosen per export or by default:

1. **The PDF theme.** The theme the PDF export would use, mapped onto Word: styles, page setup, title page, and headers and footers, with the theme's fonts embedded.
2. **A Word template.** A named `.dotx` (or `.docx`) template's styles, page setup, and headers and footers, with the export's own styles added where the template has none.
3. **No theme.** Neither: the export's built-in look, as today, with the `[docx]` settings.

`[docx]` settings apply over the PDF theme or template: whatever a settings file says wins.

## 2. Goals

1. One export, three looks, chosen the same way in the app, the CLI, the MCP server and profiles.
2. With the PDF theme, a reader recognises the PDF in the Word document: the same fonts, sizes, colours, page, title page and running content, as far as Word can express them, every style still editable.
3. A Word template is used as Word users expect of one: its styles, numbering, page setup, and headers and footers, so a corporate document looks corporate.
4. Fonts the look names travel with the document where their licence allows.
5. A build with Word export and without PDF export still compiles no Typst.
6. Nothing a template or theme holds can inject content, run code or reach outside the workspace.

## 3. Non-goals, and decisions revised

- **Revised:** the DOCX spec's goal 5 ("independent of `[pdf]`") and its §3 exclusion of `.dotx` templates. `[pdf]`'s theme settings are now read for Word when the look is the PDF theme, and Word templates are supported as §5.3 sets out. `[pdf.profiles]` and `[docx.profiles]` stay independent.
- No live preview of a look, no editing of themes or templates in the app.
- A template's cover pages, building blocks, content controls, macros (`.dotm`), custom XML and document body content are not used.
- Theme keys Word cannot express (Prawn-specific spacing, `keep` rules, background images behind body text, gradient fills) are left out, listed in a table as the PDF theme keys page lists the PDF's.
- Fonts are embedded whole; no subsetting in this version.

## 4. User workflows

- **Default look.** With no choice made, an export follows `[docx] look` (§5.1), which defaults to the PDF theme **when one is named**: an existing workspace with `[pdf] theme = "corporate"` gets corporate Word documents with no new settings. A workspace that names no theme anywhere gets today's Word output, not the bundled `default` theme.
- **App.** Export Page to Word… and Export Component to Word… use the default look. New Export Page to Word with Theme… and Export Component to Word with Theme… show one picker, as Export Page to PDF with Theme… does, listing:
  - the PDF themes (bundled and the workspace's), marked as PDF themes;
  - the Word templates found;
  - No theme.
  Choosing one asks where to save, then exports.
- **CLI.** `adocfoundry-cli docx … [--theme <name|file.yml> | --template <name|file.dotx> | --no-theme]`, at most one; none means the default look. Profiles: `--profile` with any of them overrides the profile's.
- **MCP.** `export_docx` takes at most one of `theme`, `template` or `no_theme: true`, each confined to the roots like every path.
- **Profiles.** `[docx.profiles.<name>]` may set `theme`, `template` or `look = "none"`.
- **Settings.** `[docx] look = "pdf-theme" | "template" | "none"` and `[docx] template = "<name|path>"`.

## 5. Functional requirements

### 5.1 Choosing the look

- **Precedence.** An export's look is the first given of:
  1. the export's own choice;
  2. the profile's;
  3. `[docx] look`, with `[docx] template` naming the template when the look is `template`.
- **PDF theme.** Which PDF theme is resolved exactly as for PDF: the export's own `theme`, else the first page's `pdf-theme` (with `pdf-themesdir`), else `[pdf] theme` (with `[pdf] themes_dir`). Use the engine's existing `theme_source`, so the two formats can never disagree about which theme applies.
- **Only a named theme.** Unlike PDF, Word does not fall back to the bundled `default` theme. When the look is the PDF theme and no theme is named, the look is no theme. Named means:
  - the export's own;
  - a profile's;
  - the page's `pdf-theme`;
  - `[pdf] theme` written in a settings file.

  `Settings` records whether `[pdf] theme` was written, since its value defaults to `default` either way. An export choosing `--theme default` gets the bundled default.
- **Named Word templates** are looked for beside the PDF themes:
  - in `pdf-themesdir`, else `[pdf] themes_dir`;
  - named `<name>.dotx`, else `<name>.docx`;
  - a value ending in `.dotx` or `.docx` is a path, relative to that folder, as `ThemeSource::named` treats `.yml`.
  The picker lists every `.dotx` and `.docx` file in that folder.
- **Failure.** A theme or template that cannot be read fails the export with a diagnostic naming the file and why. It never falls back silently to another look.
- **`[docx]` applies over the look.** Any `[docx]` setting written in a settings file applies over the look, key by key: paper, orientation, margins, header and footer lines, logo, `odd_even` and `[docx.styles]`. Unset `[docx]` keys take the look's values, and with no theme the built-in defaults. A profile's `paper` and `orientation` apply over both.

### 5.2 The PDF theme mapped onto Word

The writer receives a resolved, format-neutral `WordLook` (§7), built from the theme's keys after asciidoctor-pdf's own loading (`extends`, variables, arithmetic, units, colours). The mapping:

| Theme keys | Word |
| --- | --- |
| `page_size`, `page_layout`, `page_margin` (and `page_margin_inner`/`outer` for print themes), and the page's `pdf-page-size`, `pdf-page-layout` and `pdf-page-margin`, which win over the theme as for PDF | Section page size, orientation and margins; mirrored margins where inner and outer differ |
| `base_font_family`, `_size`, `_color`, `_style`, `base_line_height`, `base_text_align` | The document defaults and the Normal style; line spacing as a multiple |
| `heading_font_*`, `heading_h<n>_font_*`, `heading_margin_top`/`_bottom`, `heading_text_transform` | Heading 1–6 (and Title for `h1` in a book), spacing before and after, caps |
| `link_font_color`, `_text_decoration` | Hyperlink |
| `code_*`, `codespan_*` | Source Code (paragraph: font, size, colour, background shading, border) and Code Char |
| `admonition_*`, `admonition_label_*`, `admonition_icon_*` colours | The admonition tables: border, background, label font and colour |
| `sidebar_*`, `example_*`, `quote_*`, `abstract_*` | Their tables and styles: border, background, font |
| `table_*` (head and body backgrounds, stripes, border colour and width, cell padding, font) | The table style and the header row's shading |
| `caption_*` | Caption |
| `title_page_*` (title, subtitle, authors, revision: font, size, colour, margins; `title_page_logo_image`, `_align`, `_top`; `title_page_align`; `title_page_background_color`) | The title page: its logo, the paragraphs' styles, alignment and spacing, and the page's background colour |
| `cover_front_image`, `cover_back_image` (theme keys; the page's `front-cover-image` and `back-cover-image` attributes win, as for PDF) | The covers |
| `header_*`, `footer_*` (`height`, `border_color`, `_width`, `font_*`, `image_vertical_align`, `<recto|verso>_<left|center|right>_content`, `columns`) | The header and footer parts: odd (recto) and even (verso) lines, their font, a bottom or top border, images placed by the content's `image:` macros; `{page-number}`, `{page-count}`, `{document-title}`, `{chapter-title}`, `{section-title}` and attribute references as fields or text |
| `font_catalog`, `font_fallbacks` | The families named in the styles, and the fonts embedded (§5.4) |

- Running content is parsed by the same code the PDF uses (`running.rs`'s content parsing, moved with the loader), so a header reads the same in both formats.
- A key in the table that the theme sets is applied; one Word cannot express is listed, in the documentation, as ignored or approximated, with a test keeping the list in step, as the PDF theme keys page does.

### 5.3 Word templates

A template is read, never trusted:

- **Accepted.** A ZIP of at most 32 MB holding a WordprocessingML package, `.dotx` or `.docx`. A macro-enabled `.dotm`, or any package with a `vbaProject` part, is refused.
- **Used:**
  - `styles.xml`;
  - `numbering.xml`;
  - the theme part (`theme1.xml`) and `fontTable.xml`, with any fonts the template embeds;
  - the last section's page size, orientation and margins;
  - the headers and footers that section references, with their own relationships to images.
- **Not used:** the body, other parts, custom XML, altChunks, OLE objects, external relationships, and anything an allowed part refers to outside the package.
- **Copied safely.** Allowed parts are copied byte for byte after being parsed as well-formed XML. Their relationships are rewritten into the export's numbering of parts. An external relationship in a header or footer (a linked picture) is dropped with a warning.
- **Missing styles.** Every style the export uses (Heading 1–6, Title, Subtitle, Source Code, Code Char, Caption, Quote, the Adoc table styles, TOC 1–9 and the rest) is used as the template defines it. Where the template has none by that id, the export's own definition is added, based on the template's Normal, so code blocks and admonitions look right in any template. The export never redefines a style the template has.
- **Numbering.** The export's list numbering is added with ids after the template's.
- **Fields.** The template's headers and footers are used as they are, fields included. `[docx] header`/`footer` lines written in settings replace the template's.

### 5.4 Fonts

- **What is embedded.** The fonts the look's styles name: the theme's `font_catalog` files, the bundled fonts a bundled theme names, and a template's own embedded fonts, which are kept. They are embedded as Word does: obfuscated with a per-font key (ECMA-376 Part 1 §17.8.1), in `word/fonts/`, referenced from `fontTable.xml` as `w:embedRegular`, `w:embedBold`, `w:embedItalic` and `w:embedBoldItalic`.
- **Licence.** A font is embedded only when its OpenType `fsType` allows it:
  - installable (0) or editable (8): embedded;
  - preview and print (4): embedded, with a warning that Word may open the document read-only;
  - restricted (2), or the bitmap-only flag: not embedded, with a warning naming the font. The no-subsetting flag allows embedding, since fonts are embedded whole.
  A font file that cannot be read gives a warning and is named only.
- **Setting.** `[docx] embed_fonts = true` by default. `false` names the families without embedding them.
- **Limits.** Embedded font files count towards the export's size limits. A family is embedded once, whatever uses it.

### 5.5 Front ends

- **App.** The two new commands (`export.docx_page_themed`, `export.docx_component_themed`) are in the File menu and the palette. Their picker reads `Engine::docx_looks()`, the bundled and workspace PDF themes, the Word templates and No theme, each with its kind and path. The completion notification names the look used.
- **CLI.** `--theme`, `--template` and `--no-theme` on `docx`, mutually exclusive, and refused as unknown options by `pdf`, except `--theme`, which `pdf` already has.
- **MCP.** `theme`, `template` and `no_theme` on `export_docx`, at most one, each confined to the roots. Its answer gains `look: { kind, name, path }`.
- **Profiles.** `theme`, `template` and `look = "none"` (at most one) in `[docx.profiles.<name>]`, validated by `docx_profile_problems`. The D6 rule refusing `theme` as a PDF key is lifted.
- **Summary.** `ExportSummary` gains the look used, which every front end can show.

## 6. Existing code to reuse

- The theme loader (`adoc-pdf/src/theme/`): `ThemeSource`, `ThemeKeys`, `LoadedTheme`, `KeptTheme`, the font catalogue. It is free of Typst already (it uses `adoc-core`, `regex`, `serde`, `serde-saphyr`).
- Running content parsing (`adoc-pdf/src/running.rs`), and `theme_source` and `export_theme` in the engine's `export.rs`.
- The bundled themes (`adoc-pdf/themes/`) and fonts (`adoc-pdf/fonts/`), with their notices.
- The writer's `StyleOverrides`, `Running`/`Slots`/`Line`, `Margins`, `Paper`, and `parts::styles`, which gains a full style set from a `WordLook`.
- The package reader in `adoc_docx::testing`, the basis of the template reader (§5.3), which becomes production code with limits.

## 7. Architecture

- **New L1 crate `adoc-theme`.** The asciidoctor-pdf theme loader, the bundled themes and fonts, the font catalogue's resolution to files, and running content's parsing, moved from `adoc-pdf`. It depends on `adoc-core`, `regex`, `serde` and `serde-saphyr`, and never on Typst. `adoc-pdf` depends on it and re-exports what it used to define, so its callers do not change.
  - The move changes no behaviour. The theme golden tests (`cargo test -p adoc-pdf --test themes`) and the PDF regression run pass unchanged.
  - It needs a `LAYERS` entry and the documentation `every_crate_is_documented` checks.
- **`adoc-docx`** depends on `adoc-theme`, and gains:
  - `look.rs`: `WordLook`, the resolved look (page, styles, title page, running content, fonts), with `WordLook::from_theme(&LoadedTheme, …)` and `WordLook::from_template(Template)`;
  - `template.rs`: reading and checking a template;
  - `fonts.rs`: embedding and obfuscation, with `fsType` read by a small table reader of its own (only the `OS/2` table is needed), or by `ttf-parser` if it is already in the tree.
  `DocxOptions` gains `look: WordLook`. `StyleOverrides` and the other `[docx]` options stay, and apply over it (§5.1).
- **`adoc-engine`** resolves the look (`src/docx_look.rs`): precedence, `theme_source` shared with PDF, template lookup beside the themes, `Engine::docx_looks()` for pickers, and `[docx] look`, `template` and `embed_fonts` in `docx_settings.rs`. `DocxExportArgs` gains `look: Option<LookChoice>` (`PdfTheme(String)`, `Template(String)`, `None`).
- **Layers.** A build with `docx` and without `typst` compiles `adoc-theme` and not `adoc-pdf` or Typst; `docx_builds_without_typst` is extended to check that `adoc-theme` holds no Typst.

## 8. Output safety

- A template is untrusted input. It is limited in size, refused if macro-enabled, and copied only part by part from the allowed list. External relationships are dropped, and every XML part is checked well formed. The package validator (`adoc_docx::testing`) runs on every exported document in the tests, template ones included.
- Theme and template files are read only through the engine's `FileAccess`, so the MCP server's confinement covers them.
- Embedded fonts are read the same way, and only the files the look names.

## 9. Tests and acceptance criteria

| # | Test | Package | What it shows |
| --- | --- | --- | --- |
| 1 | `theme_crate_builds_without_typst` | T1 | `adoc-theme` compiles with no Typst; `adoc-pdf`'s theme golden tests and the PDF regression pass unchanged |
| 2 | `docx_look_follows_precedence` | T2 | Export choice over profile over `[docx] look`; the PDF theme resolved as the PDF's is, page attribute included; a missing theme or template fails with its path |
| 3 | `docx_settings_apply_over_the_look` | T2 | Each `[docx]` key written wins over the theme's or template's value; unset keys take the look's |
| 4 | `docx_pdf_theme_styles` | T3 | Body, headings, code, links, admonitions, tables, sidebars, quotes and captions take the theme's fonts, sizes, colours and spacing (bundled `default` and a fixture theme) |
| 5 | `docx_pdf_theme_page_and_title_page` | T3 | Page size, layout and margins; the title page's logo, alignment and styles; covers from theme keys |
| 6 | `docx_pdf_theme_running_content` | T4 | Recto and verso header and footer lines, fields, images and borders from the theme, parsed by the same code as the PDF's |
| 7 | `docx_fonts_are_embedded` | T5 | The look's fonts embedded, obfuscated, referenced from the font table and readable back; `fsType` respected with warnings; `embed_fonts = false` names only |
| 8 | `docx_template_is_used` | T6 | A fixture template's styles, numbering, page and headers and footers are in the export; missing styles added; nothing of its body |
| 9 | `docx_template_is_untrusted` | T6 | `.dotm`, `vbaProject`, oversize, malformed XML, external relationships and altChunks refused or dropped with warnings |
| 10 | `docx_look_choice_everywhere` | T7 | The app's picker, the CLI's three options, the MCP arguments and profile keys choose the same look and write the same bytes |

Manual checks are in `docs/qa/docx.md`:
- the reference book exported with the PDF theme, side by side with its PDF;
- a corporate template;
- fonts on a machine without them;
- Word and LibreOffice.

## 10. Work packages

As a GitHub stack, one PR each:

1. **T1: `adoc-theme`.** Move the theme loader, bundled themes and fonts, the font catalogue's resolution and running-content parsing out of `adoc-pdf`, with no change in behaviour.
2. **T2: Choosing the look.** `LookChoice`, `[docx] look` and `template`, precedence, `theme_source` shared, template lookup, `[docx]` over the look, `ExportSummary`'s look. No theme is today's output.
3. **T3: PDF theme to Word styles, page and title page.** `WordLook::from_theme` for styles, page setup, title page and covers.
4. **T4: PDF theme running content.** Headers and footers from the theme.
5. **T5: Embedded fonts.**
6. **T6: Word templates.**
7. **T7: Front ends, docs and QA.** The picker commands, CLI options, MCP arguments, profile keys, the user guide (with the mapping and ignored-keys table), and the QA record.

## 11. Definition of done

1. The reference book exported with its PDF theme is recognisably the PDF in Word and LibreOffice: fonts, colours, heading sizes, page, title page, and headers and footers. The ignored keys are documented.
2. A corporate `.dotx` gives the export its styles, page and running content, with the export's code and admonition styles added.
3. No theme gives today's output.
4. Fonts travel with the document where licences allow, and the warnings say where they do not.
5. The same choice gives the same bytes through the app, the CLI, the MCP server and profiles.
6. A Word-only build compiles no Typst.
7. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check` pass.

## 12. Open decisions

1. **The default look (settled with the user).** The PDF theme, but only a named one (§5.1): a workspace that names no theme keeps today's Word output. A workspace that names one changes its Word output on upgrade; `[docx] look = "none"` restores today's, and release notes must say so.
2. **Font size.** Embedding whole fonts can add megabytes (the bundled Noto subsets are about 2.7 MB in all). Subsetting is deferred; T5 measures the reference book's growth.
3. **Template page setup and `[docx]` paper.** A template's page size wins over the built-in default but loses to a written `[docx] paper`, as §5.1 says. T2 confirms this reads naturally in the settings documentation.
4. **Page size, in order (builder's call).** The page's size, orientation and margins are the first given of:
   1. the export's or profile's `paper`/`orientation`;
   2. a written `[docx]` key;
   3. the page's `pdf-page-size`, `pdf-page-layout` and `pdf-page-margin` (PDF-theme look only);
   4. the theme's or template's;
   5. the built-in default.

   The attribute parsing moves into `adoc-theme` in T3 with the rest of what both formats read.
5. **`[docx]` over a template (builder's call).** A written `[docx]` key applies over a template as over a theme, from either settings file. For `[docx.styles]`, that means changing the template's own style's font, size or colour, as asked. "The export never redefines a style the template has" means the export never replaces the template's definition wholesale.
6. **Lists under a template (builder's call).** Lists keep the export's own numbering definitions, added after the template's (§5.3), and use the template's list paragraph style where it has one. Mapping AsciiDoc lists onto a template's `List Bullet`/`List Number` numbering is deferred, and documented as a known difference.

## 13. Implementation notes

### T2 (#670): choosing the look

- **Page setup arrives in T2.** `docx_settings_apply_over_the_look` needs the look to give something, so T2 brings the PDF theme's page, a part of T3's scope.
  - **Shared parsing.** PDF export's page parsing (`page_geometry`, the paper table, `sides`) moved into `adoc_theme::page`, so both formats read `page_size`, `page_layout`, `page_margin` and the page's `pdf-page-*` attributes with one function.
  - **Paper.** `adoc_docx::Paper::Custom` carries a theme's size.
  - **T3 keeps the rest:** the styles, title page and covers.
- **Settings:**
  - `[docx] look` (`DocxLook`) and `template`.
  - `DocxSettings::written`, the keys either file wrote: `paper`, `orientation`, `margins.<side>`, `header`, `footer`, `logo` and `odd_even`. `styles` keys are written when they are `Some`.
  - `Settings::pdf_theme_named`, set when a file writes `[pdf] theme`, because its value defaults to `default` either way.
- **Profiles.** A profile's `theme`, `template` or `look = "none"` (at most one, `profile_look`) becomes `DocxProfile::look`, and then `DocxExportArgs::look`. The D6 rule refusing `theme` is lifted; PDF-only keys such as `standards` are still refused.
- **Resolution** (`Engine::docx_look`):
  - It runs on the engine thread as the writer starts, from the first page's (or the book's) attributes.
  - A failure fails the export through the writer's result, as a PDF theme's does.
  - `theme_source` and the new `themes_dir` moved from the PDF-only code to the shared export code, so Word reads them without Typst. The engine's `export` feature brings in `adoc-theme`.
- **Reporting.** `ExportSummary::look` (`ExportLook`: kind, name, path) reports a Word export's look. PDF and HTML exports report none.
- **Templates.** A template is found and checked to exist; T6 reads it.
- **Tests.**
  - `docx_look_follows_precedence` and `docx_settings_apply_over_the_look` (`tests/docx_looks.rs`).
  - `the_look_and_what_was_written_are_read` (`docx_settings.rs`).
  - `profiles_are_read_and_checked`, updated for the lifted rule.

### T3 (#671): theme styles, title page and covers

- **`WordLook`** (`adoc-docx/src/look.rs`) is format-neutral. It holds:
  - `StyleLook` per style id: font, size, colour, bold, italic, capitals, underline, spacing, line height, alignment, shading, border;
  - `BoxLook` per table style: borders, background, header row, stripes;
  - the title page: alignment, logo and its alignment;
  - the covers.
- **Mapping.** `WordLook::from_theme` maps the keys in `MAPPED_KEYS`, read as the PDF template reads them:
  - `title_page_text_align`, else `base_text_align`;
  - `caption_text_align`, else `caption_align`.

  The spec's first table named `title_page_align`, the pre-2.0 spelling.
- **Laying it over the styles.** `parts::styles` gives each built-in style its look through `write_style`: the theme's, then the written `[docx.styles]` keys (`written_look`), so a written key wins.
  - **Order.** Properties are emitted in the schema's order (`PARAGRAPH_ORDER`, `RUN_ORDER`), since Word refuses properties out of order. A theme can add elements a style did not have, such as `w:jc`, `w:pBdr` and `w:caps`.
  - **Table styles** take the box's border colour and width and a cell background. The header row and stripes are `tblStylePr` `firstRow` and `band2Horz`; asciidoctor-pdf stripes even rows by default, which Word counts as the second band after the header.
- **Title page.** It takes the look's alignment and the theme's logo (the page's own logo wins), the logo aligned on its own.
- **Covers.** They come from `cover_front_image`/`cover_back_image`, relative to the theme's folder, when the page names none.
- **Not in Word.** `title_page_background_color` is not mapped: Word has one background for the whole document, not one per section. It joins the ignored keys T7 lists.
- **Tests.** `docx_pdf_theme_styles` and `docx_pdf_theme_page_and_title_page` (`tests/docx_looks.rs`) export a book in `tests/fixtures/docx/themes/brand-theme.yml`, which gives every mapped family a distinct value, and check each family arrived. The first also checks that a written `[docx.styles]` key wins and the rest stays the theme's.
- **On a real theme.** A real corpus theme (one with its own fonts) exported through LibreOffice keeps its heading sizes and colours, its table header shading and its link colours. Its fonts show as substitutes until T5 embeds them.

### T4 (#672): theme headers and footers

- **Translation, not shared parsing.** The theme's running content is translated in `adoc-docx` (`look.rs`, `running`) into the Word writer's own slot language. It is not parsed by the PDF's `running.rs`, which writes Typst markup; moving that into `adoc-theme`, as §7 proposed, would have shared only the attribute rules. The rules are the PDF's:
  - page values (`{page-number}`, `{page-count}`, `{document-title}`, the chapter, section and part titles) are kept;
  - other attributes take the document's values, and a line naming an unset one is dropped;
  - character references are decoded;
  - an `image:` macro standing alone is a picture from the theme's folder, as `{image:<path>|<points>}`;
  - an absolute `pdfwidth`, `scaledwidth` or `width` sizes it, and anything else makes it a line high.
- **Odd and even pages.** Even (verso) pages show only their own content, with no fallback to recto, as the PDF template does (`contents.at(side, default: (:))`). A periphery with no `<periphery>_height` shows none.
- **Fields.**
  - `{part-title}` is `STYLEREF "heading 1"`.
  - `{section-title}` and `{section-or-chapter-title}` are the level below the chapter's; Word's fields cannot fall back from one style to another.
  - `*bold*`, `_italic_` and `` `monospace` `` marks are left out with their text kept, rather than formatted; this is an approximation.
- **Position.** The distance from the page's edge follows the PDF's box: within `<periphery>_height`, by `_vertical_align` and `_padding`, with the text taken as 1.2 times the font size.
- **Styles.** The Header and Footer styles take `<periphery>_font_*` and a rule from `_border_color`/`_border_width`: below the header, above the footer. They are written through the same look overlay as the other styles (`paragraph_look`, `run_look`).
- **What wins.** Written `[docx]` `header`, `footer`, `margins.header`, `margins.footer` and `odd_even` keys win over the theme's.
- **Not used.** `<periphery>_columns` is ignored: Word's left, centre and right tab stops place the slots.
- **Tests.**
  - `docx_pdf_theme_running_content` (`tests/docx_looks.rs`) covers odd and even headers and footers, fields, an attribute, a dropped line, the marks, a character reference, a one-inch image, the style's colour, size and rule, the header's distance, and a written footer winning while the theme's header stays.
  - `running_content_is_read_as_the_pdf_reads_it` (`look.rs`) covers half-written content, which never panics, marks inside words, and image macros.
- **On a real theme.** A corpus page exported in its own theme shows the theme's logo, section title and rule in the header, and its logo, copyright line and "Page n of m" in the footer.

### T5 (#673): embedded fonts

- **Which fonts.** The families of the theme's `font_catalog` that the Word styles name (the look's, and `[docx.styles]`'s), with all four faces the catalogue gives. `font_fallbacks`, the icon fonts and the other bundled fonts are never embedded. A template's own fonts wait for T6; no theme embeds nothing and writes no font table, so its output is unchanged.
- **Finding them.** `adoc_theme::resolve_font` is the order asciidoctor-pdf uses (the fonts folders, `GEM_FONTS_DIR` among them, then the theme's folder, then the bundled fonts), now shared: `adoc-pdf`'s `load` calls it, and the engine's `theme_fonts` (`docx_look.rs`) gives each face as `FontSource::Bundled`, a `File` the export's `FileAccess` permits, or `Missing`. `Engine::fonts_dirs` moved from the Typst-only code to the export pipeline's, so a Word-only build has it.
- **Names (builder's call).** Word and LibreOffice find an embedded font by the family name inside it (name ID 1), not by the font table's label. A family is therefore written under its regular face's own family name (else its first readable face's), in the font table and in every style that names it, whether or not it is embedded; the theme's name is kept only when no face can be read. A face whose own family differs is embedded with a warning. The bundled fonts' own names equal the bundled themes' catalogue names, so their output is unchanged.
- **Embedding.** `adoc-docx`'s `fonts.rs`: each distinct face once, as `word/fonts/font<n>.odttf` (`application/vnd.openxmlformats-officedocument.obfuscatedFont`), referenced from `word/fontTable.xml` (`w:embedRegular`, `w:embedBold`, `w:embedItalic`, `w:embedBoldItalic`, each with its `r:id` and `w:fontKey`), and `w:embedTrueTypeFonts` in the settings. The key is a GUID made from the face's SHA-256, so the same fonts give the same bytes; the first 32 bytes are XORed with the key's bytes read from the GUID's last digit pair to its first (§17.8.1).
- **Reading fonts.** `fonts.rs` has a small table reader of its own (§7's first choice), every count, offset and length checked, since a theme may name any file: `fsType` from `OS/2`, and the family name from `name` (Unicode or Windows Unicode, US English first). `ttf-parser` was tried and dropped: RustSec lists it as unmaintained (RUSTSEC-2026-0192), and `cargo deny` refuses an unmaintained crate a workspace crate depends on directly. Its `permissions()` also gave nothing for combined bits in `OS/2` version 3 and later.
- **Licences.** The least restrictive usage bit wins, as §5.4 says, and the bitmap-only flag refuses.
- **CFF and collections (builder's call).** A face with CFF outlines (`OTTO`) or a font collection (`ttcf`) is named and not embedded, with a warning: Word embeds TrueType outlines. A collection's own family name is not read, so it keeps the catalogue's name.
- **Limits.** A font file larger than a picture may be (64 MB) is not read.
- **Checks.** `assert_valid_package` checks the font table's content type and relationship, each embedded face's relationship, content type and GUID key, that the key opens the part into a font (its sfnt header and first table record), that every font part is a face in the table, and that the settings say fonts are embedded exactly when one is; `the_package_checks_catch_damage` breaks a key, a GUID and the settings.
- **Tests.** `docx_fonts_are_embedded` (`tests/docx_looks.rs`): an aliased family under its own name in the table and the styles, faces embedded, a restricted face and a missing one named with warnings, the same bytes twice, `embed_fonts = false`, no theme. `theme_fonts_outside_the_roots_are_not_embedded`. `fonts.rs` covers the bundled fonts' own names, the key, `fsType` variants, CFF, deduplication, and damaged fonts (every cut-short head, changed directory and name bytes, a huge table count, a name past the end), which never panic; `adoc-theme` the resolution order.
- **Round trip.** A page whose theme names M+ 1mn (not installed on the test machine) as `Brand Mono`, opened in LibreOffice 26.8 and converted to PDF, is drawn in the embedded M+ 1mn regular and bold; named only, it falls back to Liberation Serif. Word itself has not been tried.
- **Size (§12.2).** The reference book with the bundled `default` theme grows from 41 KB to 603 KB: Noto Serif and M+ 1mn, eight faces, about 1.2 MB before compression.
