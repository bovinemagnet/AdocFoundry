# Phase 4 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Phase 4 acceptance (spec §10 and §13) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ (AppImage) before a release. Record the build, the platform and the result in the table at the end.

Automated coverage for reference: the theme loader against asciidoctor-pdf's own loader, key by key, for the bundled themes and a custom theme (`crates/adoc-pdf/tests/themes.rs`); the emitter, escaping and text that survives compilation over the render fixtures and every documentation page (`tests/emit.rs`); compilation, the world's file access and error mapping (`tests/compile.rs`); running content, parity, the title page and the table of contents (`tests/running.rs`); the index, PDF/A-2a, PDF/UA-1, the tag tree and metadata (`tests/standards.rs`); the engine's export of a page and of a component as a book (`crates/adoc-engine/tests/export.rs`); the CLI (`crates/adocfoundry-cli/tests/cli.rs`); the app's export commands and their messages on GPUI's test platform (`export_command_writes_a_pdf`, `export_component_exports_the_active_page_s_component_with_unsaved_edits`, `export_errors_name_the_file_and_line`); and the visual regression against asciidoctor-pdf (`crates/adocfoundry-cli/tests/pdf_regression.rs`).

## Setup

1. `git submodule update --init`, then `cargo build --release -p adocfoundry -p adocfoundry-cli`.
2. Install asciidoctor-pdf 2.3 (`gem install asciidoctor-pdf`, or Homebrew's `asciidoctor`) for the side-by-side checks.
3. A PDF viewer that shows two documents side by side, and a PDF/A and PDF/UA validator for the standards (veraPDF 1.30, `brew install verapdf`).

## Reading distance (Definition of Done)

- [x] `adocfoundry-cli pdf tests/fixtures/pdf/reference/article.adoc` and `asciidoctor-pdf` of the same page, side by side: the same four pages, the same page breaks, and every line of body text within 1.7 pt of asciidoctor-pdf's (measured line by line on macOS), apart from the footnote at the foot of the last page (3.6 pt) and the text drawn inside the SVG image (3.0 pt, a different SVG renderer); at reading distance the two cannot be told apart. The visual regression test measures the same pages: a mean grey difference of at most 0.0085 per page and the ink within 1.8 pt.
- [x] The same with `tests/fixtures/pdf/themes/acme-theme.yml` and `:title-page:`: title page with logo, header with the document title, footers with the section title and a bold page number on recto and verso; the same six pages (macOS).
- [x] A real documentation page with the project's own asciidoctor-pdf theme and fonts (`--theme …/jdr-full-theme.yml --fonts-dir …/fonts`): header and footer images, `1 of 5` page numbers, Montserrat and JetBrains Mono, every line within 0.7 pt of asciidoctor-pdf's (macOS).

## Themes

- [x] A custom theme written for asciidoctor-pdf loads without edits: the fixture theme, and eleven themes from a real project, key for key against asciidoctor-pdf's loader (`ADOC_PDF_THEME_CORPUS`, macOS).
- [x] A theme with `font.fallbacks` renders a glyph the base font lacks: a theme extending `default` with asciidoctor-pdf's `M+ 1p Fallback` font in `font.catalog` and `font.fallbacks` draws Japanese text and arrows in the fallback font, on a line within 0.05 pt of asciidoctor-pdf's `default-with-font-fallbacks` (macOS).
- [x] `default-sans` and `default-for-print` side by side with asciidoctor-pdf: now part of the visual regression test, at most 0.0072 and 0.0087 mean grey difference per page, with the ink within 3.6 pt and 1.8 pt (macOS).

## Books

- [x] `adocfoundry-cli pdf tests/fixtures/pdf/book/guide` (copied elsewhere first): title page with the version, table of contents with parts, chapters and sections, the start page as the preface, each chapter on a new page, the Mermaid diagram drawn, links between pages work in a PDF viewer (macOS).
- [x] A large real component: a documentation project's component of 212 pages in 17 nav files exports as a book of 1,838 pages in 55 s (release build). Every page appears once and in navigation order, checked against the nav files by a script that does not use the exporter. The bookmarks are the 17 parts in navigation order with their chapters, 3,093 in all (macOS). The run found and fixed: nav titles written as links, untitled pages, document titles cut at `: `, tag directives left in paragraphs, spaced include attributes, and pages the parser rejects being exported without a warning.
- [x] `[pdf] site_url` set: in the book fixture, the reference to the other component is a link to `https://docs.example.com/other/index.html` in the PDF's link annotations; without `site_url` it is text (macOS). The link was not clicked in a viewer.

## Standards

- [x] `--standard pdf-a-2a` passes veraPDF's PDF/A-2a profile: the reference article (43,438 checks, none failed) and the book fixture with both standards (36,972 checks, none failed) (veraPDF 1.30.3, macOS).
- [x] `--standard pdf-ua-1` passes veraPDF's PDF/UA-1 profile: the reference article (13,062 checks, none failed) and the book fixture with both standards (5,938 checks, none failed) (veraPDF 1.30.3, macOS).
- [ ] A screen reader reads headings, lists, tables and image alternatives in order. What it reads is the tag tree, now checked by `the_tag_tree_reads_in_order` (`tests/standards.rs`): headings in order with their titles, lists as `L` with `Lbl` and `LBody` for every item, tables with `TH` and `TD`, and the image as a `Figure` with its alt text. The check found lists tagged as `Div`, now fixed. Description lists are still tagged `Div`. Listening with VoiceOver, NVDA or Orca still needs a person.

## The app

- [ ] Export Page to PDF… from the palette asks where to save (suggesting the page's name), the status bar shows Exporting PDF… while it runs, and the notification's Open opens the PDF. On GPUI's test platform `export_command_writes_a_pdf` checks that it asks, that the export is running once a path is chosen (the status bar's condition), and the notification's text. The suggested name and Open need the native dialog and the system's handler, which the test platform does not provide; on macOS the palette lists the command and choosing it opens the save dialog.
- [x] Export Component to PDF… exports the active page's component; unsaved edits are in the PDF: `export_component_exports_the_active_page_s_component_with_unsaved_edits` on GPUI's test platform. From a page of the book fixture the export is the whole `guide` book, and 150 unsaved paragraphs lengthen it while the file on disk is unchanged.
- [x] A theme error (a broken `pdf-theme`) and a Typst error (an unreadable image) are reported with the file and line: `export_errors_name_the_file_and_line` on GPUI's test platform, and by hand with the CLI (`broken-theme.yml: error: line 2 column 4: …` and `imaged.adoc:5: failed to decode image …`).

## Scale

- [x] Every page of a real 786-page documentation project exports as an article without an error, in about 40 ms a page in a release build (`ADOC_PDF_CORPUS`, macOS).

## Results

| Date | Build | Platform | Result |
|---|---|---|---|
| 2026-10-24 | `phase-4` branch, debug and release | macOS 26 (arm64) | The checks marked above pass. App: the palette lists Export Page to PDF…, and choosing it opens the save dialog; the dialog could not be completed from a script, so the app checks need a person. Windows, the Linux app and the screen reader are not yet run. |
| 2026-10-24 | `phase-4` branch, debug and release | Linux arm64 (Debian 12 container, Rust 1.99.1) | `cargo test --workspace --exclude adocfoundry`: 525 passed, none failed (tests that need asciidoctor or asciidoctor-pdf skip themselves). The CLI's PDFs of the reference article and the book fixture are byte for byte the same as macOS's. The app and the AppImage need a display and were not run. |

## Known limitations found

- acdc-parser rejects some input Asciidoctor reads: `subs=` on an include, and a tab between a section title's `=` and its text. The page is exported as its source with a warning naming the file and the parser's message (two pages of the 212 above).
- acdc-parser keeps tag directives in two places Asciidoctor drops them: the lines of a tagged include that mark other tags, and a callout list item's text, where the directive is joined to the item (145 lines in the book above).
- Memory: Typst holds the whole layout while it compiles, about 9 MB a PDF page; the 1,838-page book peaked at 17 GB. Typst's cache is emptied after each compile, which frees about 11 GB. What stays allocated afterwards in a long-running process (15.5 GB after a 2,062-page book, 4.5 GB after 712 pages) is the capacity of comemo's emptied tables: its call-tree slabs, edge maps and per-call constraint tables, found with `malloc_history`, with no leaks. A second export reuses it rather than growing. Freeing it needs typesetting in a child process or a change to comemo.
- Description lists are tagged `Div`, not as lists.
