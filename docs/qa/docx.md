# Word (DOCX) Export — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Word export where automated tests cannot reach: opening, editing and saving the documents in Microsoft Word and LibreOffice Writer, field refresh, and how they look. The tests read the package back; they cannot run Word |

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

## Known differences and limitations

- **Pagination is Word's.** Word lays out the pages, so page breaks and page counts differ from the PDF's, and between Word and LibreOffice.
- **Page counts.** "Page n of m" counts every page in `m`, covers and title page included, since `NUMPAGES` counts the whole document. The body's own numbering starts at 1.
- **Fields in other programs.** The contents' page numbers, `NUMPAGES` and the chapter titles in headers are fields. Word fills them in when the document opens (it asks first). LibreOffice, and conversions from the command line, show them as last saved until they are updated: the contents without page numbers, as seen in LibreOffice 26.8.
- **Formulas are pictures.** Each formula is typeset as a picture with its source as alternative text, not an editable Word equation (OMML is deferred).
- **Remote images.** They are linked, never fetched, so they show only where the network allows.
- **Unsupported constructs.** Video and audio are their addresses. A block the writer does not know is its source, with a warning.
- **No Word templates.** `.dotx` templates are not used (PRD §3). Styles are changed through `[docx.styles]`.
- **LibreOffice merges identical pictures.** On saving, LibreOffice merges pictures whose pixels are the same (the reference book's logo and cover), so the saved file has one fewer image part. Every picture still shows.

## Results

| Build | Application | Platform | Result | Notes |
|---|---|---|---|---|
| `docx-8-quality` | LibreOffice 26.8.3.1 (headless) | macOS 26 | Pass, with differences noted | The reference book converts to PDF (9 pages) and to text without error. Covers, title page, headers and footers (odd, even, first), lists, the merged table, code, admonitions, the PNG, JPEG, SVG, diagram and both formulas, links and the repeated footnote all show as intended. The contents show no page numbers until fields are updated. Saved back to DOCX by LibreOffice, the text is unchanged and all 11 drawings and 11 fields remain, the two identical pictures merged into one part. Items 1–10 in the desktop applications remain to be run. |
| | Microsoft Word | macOS | | |
| | Microsoft Word | Windows | | |
| | LibreOffice Writer | Windows | | |
| | LibreOffice Writer | Ubuntu | | |
