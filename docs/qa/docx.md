# Word (DOCX) Export — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Word export where automated tests cannot reach: opening, editing and saving the documents in Microsoft Word and LibreOffice Writer, field refresh, and how they look, with no theme, with a PDF theme and with a Word template (the Word looks PRD, epic #668). The tests read the package back; they cannot run Word |

Run each item in Microsoft Word (desktop: Microsoft 365 or 2021 on macOS and Windows) and in LibreOffice Writer (7.6 or later on macOS, Windows and Ubuntu). Record the build, the application and version, the platform and the result in the table at the end. Record a difference as a difference, with what was seen, rather than leaving the item out.

## The documents

Export them from a copy of the fixtures, since an export writes its diagram cache beside the workspace:

```sh
cp -R tests/fixtures/docx/reference /tmp/reference
cargo run -p adocfoundry-cli -- docx /tmp/reference/manual --root /tmp/reference --output /tmp/manual.docx
cargo run -p adocfoundry-cli -- docx tests/fixtures/pdf/reference/article.adoc --output /tmp/article.docx
ADOC_DOCX_OUT=/tmp/docx-samples cargo test -p adoc-docx --test package -- --ignored
```

- **`manual.docx`, the reference book.** Version 2.0 of the `manual` component, from two navigation files. It holds:
  - covers, a title page with a logo, and the table of contents;
  - odd, even and first-page headers and footers, with the logo, the chapter's title and live page numbers;
  - nested lists and a checklist, a table with merged cells, code with a callout, admonitions, a sidebar, an example and a quotation;
  - PNG, JPEG and SVG images, a Mermaid diagram, and inline and block formulas;
  - footnotes (one referred to twice), links inside the book and to the web, and text from a partial nested in another.
- **`article.docx`.** The PDF reference article as one page.
- **`docx-samples/`.** The writer's own samples, one construct each.
- **Looks.** The reference book again, in each look:

  ```sh
  cargo run -p adocfoundry-cli -- docx /tmp/reference/manual --root /tmp/reference --theme default --output /tmp/manual-default.docx
  cargo run -p adocfoundry-cli -- pdf /tmp/reference/manual --root /tmp/reference --theme default --output /tmp/manual-default.pdf
  cargo run -p adocfoundry-cli -- docx /tmp/reference/manual --root /tmp/reference --template /path/to/corporate.dotx --output /tmp/manual-corporate.docx
  cargo run -p adocfoundry-cli -- docx /tmp/reference/manual --root /tmp/reference --no-theme --output /tmp/manual-plain.docx
  ```

  The corporate template is any real one: a company's own, or one saved from Word or LibreOffice as a template, with its own headings, a header with a logo and a footer with a page number.

## Automated coverage, for reference

- **Package:**
  - `adoc_docx::testing::assert_valid_package` is run on every document the tests write. It checks:
    - the ZIP, with every entry's CRC;
    - the content types;
    - every part's relationships and their targets, headers' and footers' media included;
    - that every part is well-formed XML;
    - styles by kind, in the document, footnotes, headers and footers;
    - numbering, footnotes, and bookmarks that are unique and paired;
    - every internal link leading to a bookmark;
    - that every field is whole;
    - that sections' header and footer references lead to header and footer parts.
  - `the_package_checks_catch_damage` (`crates/adoc-docx/tests/validator.rs`) shows each of those checks failing on a damaged package.
- **The reference book:** `docx_reference_book_is_complete` (`crates/adoc-engine/tests/docx_export.rs`). It checks:
  - every structural check above;
  - the pages and their order, and nothing from version 1.0 or the other component;
  - every construct, as its Word structure;
  - one located warning;
  - the same bytes from a second export and from a copy of the workspace elsewhere.
- **Failures:**
  - `docx_problems_are_located`: missing images, in a page and in a partial, a diagram nothing renders, a video and source cut short, each placed at its file and line.
  - `a_timed_out_export_leaves_the_output`.
  - `docx_failed_export_keeps_existing_output`, `docx_bad_images_are_reported`.
  - The CLI's and MCP server's refusals: `docx_cli_exports_and_gates`, `docx_mcp_export_is_write_gated`.
- **Looks** (`crates/adoc-engine/tests/docx_looks.rs` unless named):
  - `docx_look_follows_precedence`, `docx_settings_apply_over_the_look`;
  - `docx_pdf_theme_styles`, `docx_pdf_theme_page_and_title_page`, `docx_pdf_theme_running_content`;
  - `docx_fonts_are_embedded`, with every embedded face's key checked to open it as a font;
  - `docx_template_is_used`, `docx_template_is_untrusted`, and `crates/adoc-docx/tests/template.rs`;
  - `docx_look_choice_everywhere`, in `crates/adocfoundry-cli/tests/cli.rs` and `crates/adoc-mcp/tests/docx.rs`, and the app's `export_page_to_word_with_theme_offers_the_looks`: each front end's choice writes the engine's bytes;
  - `every_key_read_is_mapped_and_every_mapped_key_is_read` (`look.rs`) and `the_word_theme_keys_table_is_current` (`crates/adoc-docx/tests/theme_keys.rs`), which keep the Word theme keys page true.
- **Size:** `docx_large_book_is_bounded` (`crates/adoc-engine/tests/docx_large.rs`), with 600 pages and budgets for time, the longest pump and, on Linux, peak memory.

## Checks

1. **Opens cleanly.** `manual.docx` and `article.docx` open with no repair or "unreadable content" prompt. Word may ask to update fields that refer to other files; this is `w:updateFields`, and answering Yes fills in the table of contents.
2. **Table of contents.** After the update, the contents list the parts, chapters and sections with page numbers, and each entry leads to its heading. In LibreOffice, Tools › Update › Update All does the same.
3. **Page numbers and running content.**
   - The covers and the title page have no header or footer.
   - The body starts at page 1.
   - Odd pages show the logo at the left and the chapter's title at the right, and even pages mirror them.
   - The body's first page shows the book's title in the header.
   - Footers read "Page n of m".
   - After editing a chapter's title, the headers that show it follow once fields are updated (F9 in Word).
4. **Editing.** Type into a paragraph, a list item, a table cell and a footnote. Add a row to the merged table. Apply a heading style to a paragraph and update the contents. Nothing breaks the layout or the numbering of the lists.
5. **Styles.** The Styles pane lists the headings, Source Code, Caption, Quote and the Adoc styles. Changing Heading 2's font changes every chapter heading.
6. **Pictures offline.** Copy `manual.docx` alone to another machine, or disconnect from the network, and reopen it. Every picture, formula and diagram shows, each with its alternative text (Word: right-click › View Alt Text).
7. **Links.** "the requirements" and "security" go to their headings. "its site" opens `https://example.com/product`. The repeated footnote's reference shows the first footnote's number.
8. **Save and reopen.** Save in the application's own DOCX format, close, and reopen: no repair prompt, and the text, tables, pictures and fields are all there.
9. **Accessibility.** Word's Check Accessibility reports no missing alternative text. The Navigation pane lists the headings in order. Reading order in the pictures-and-captions section is picture then caption.
10. **Paper and orientation.** Export with `[docx] paper = "letter"` and `orientation = "landscape"`. The document is US Letter, landscape, with the settings' margins.
11. **The PDF theme, side by side.** Open `manual-default.docx` beside `manual-default.pdf`. The fonts, colours, heading sizes, page size and margins, the title page and the headers and footers match, allowing for Word's own pagination. Every style is still a Word style: changing Heading 1 in the Styles pane changes every chapter.
12. **Fonts travel.** On a machine without Noto Serif and M+ 1mn (or with them removed), `manual-default.docx` shows its text and code in them: they are embedded. Word may open a document whose font allows preview and print only as read-only; the export warned of it.
13. **A corporate template.** `manual-corporate.docx` uses the template's styles, page, header with its logo and footer with its page number. Code blocks, admonitions and captions look right, though the template has no styles for them. Nothing of the template's own body text appears. Save, close and reopen: no repair prompt.
14. **The picker.** In the app, Export › Export Page to Word with Theme… lists the bundled themes, the workspace's themes, the templates beside them and No theme. Choosing each exports in it, and the notification names the look.
15. **No theme.** `manual-plain.docx` is the export's own look, as before the looks existed.

## Known differences and limitations

- **Pagination is Word's.** Word lays out the pages, so page breaks and page counts differ from the PDF's, and between Word and LibreOffice.
- **Page counts.** "Page n of m" counts every page in `m`, covers and title page included, since `NUMPAGES` counts the whole document. The body's own numbering starts at 1.
- **Fields in other programs.** The contents' page numbers, `NUMPAGES` and the chapter titles in headers are fields. Word fills them in when the document opens (it asks first). LibreOffice, and conversions from the command line, show them as last saved until they are updated: the contents without page numbers, as seen in LibreOffice 26.8.
- **Formulas are pictures.** Each formula is typeset as a picture with its source as alternative text, not an editable Word equation (OMML is deferred).
- **Remote images.** They are linked, never fetched, so they show only where the network allows.
- **Unsupported constructs.** Video and audio are their addresses. A block the writer does not know is its source, with a warning.
- **Theme keys.** A PDF theme's keys Word cannot express are ignored; the Word theme keys page lists which. Fonts are embedded whole, not subset, so a document grows by its fonts (the reference book by about 560 KB with the bundled default theme).
- **Templates.** A template's body, cover pages, building blocks, content controls in the body and custom XML are not used. Lists keep the export's own numbering, not the template's List Number and List Bullet. A header or footer that refers outside the template, or holds a field that could read anything, is left out with a warning.
- **LibreOffice merges identical pictures.** On saving, LibreOffice merges pictures whose pixels are the same (the reference book's logo and cover), so the saved file has one fewer image part. Every picture still shows.

## Results

| Build | Application | Platform | Result | Notes |
|---|---|---|---|---|
| `docx-8-quality` | LibreOffice 26.8.3.1 (headless) | macOS 26 | Pass, with differences noted | The reference book converts to PDF (9 pages) and to text without error. Covers, title page, headers and footers (odd, even, first), lists, the merged table, code, admonitions, the PNG, JPEG, SVG, diagram and both formulas, links and the repeated footnote all show as intended. The contents show no page numbers until fields are updated. Saved back to DOCX by LibreOffice, the text is unchanged and all 11 drawings and 11 fields remain, the two identical pictures merged into one part. Items 1–10 in the desktop applications remain to be run. |
| `look-5-fonts` | LibreOffice 26.8 (headless) | macOS 26 | Pass | A page whose theme names M+ 1mn (not installed) as `Brand Mono`, converted to PDF: drawn in the embedded M+ 1mn regular and bold. Named only (`embed_fonts = false`), it falls back to Liberation Serif. Item 12, in LibreOffice only. |
| `look-6-templates` | LibreOffice 26.8 (headless) | macOS 26 | Pass | A template LibreOffice saved (`.dotx`) read with no warnings and used for an export, which passed the package checks; converted to PDF, it shows the template's footer and page number, the numbered list and the added code style. Part of item 13. |
| `main`, 2026-11-18 | LibreOffice Writer (version not recorded) | Linux | Pass | A Word export with a PDF theme, opened by the maintainer: the theme's look shows as intended. The contents had no page numbers on opening; Tools › Update › Update All filled them in correctly (item 2; whether each entry leads to its heading was not recorded). |
| `main`, 2026-11-18 | WordCraft (version not recorded) | Linux | Pass | The same export: the theme's look shows as intended. The contents had no page numbers on opening; updating the table filled them in correctly (item 2; whether each entry leads to its heading was not recorded). |
| | Microsoft Word | macOS | | |
| | Microsoft Word | Windows | | |
| | LibreOffice Writer | Windows | | |
| | LibreOffice Writer | Ubuntu | | |
