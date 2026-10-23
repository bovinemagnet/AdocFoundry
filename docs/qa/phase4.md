# Phase 4 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Phase 4 acceptance (spec §10 and §13) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ (AppImage) before a release. Record the build, the platform and the result in the table at the end.

Automated coverage for reference: the theme loader against asciidoctor-pdf's own loader, key by key, for the bundled themes and a custom theme (`crates/adoc-pdf/tests/themes.rs`); the emitter, escaping and text that survives compilation over the render fixtures and every documentation page (`tests/emit.rs`); compilation, the world's file access and error mapping (`tests/compile.rs`); running content, parity, the title page and the table of contents (`tests/running.rs`); the index, PDF/A-2a, PDF/UA-1 and metadata (`tests/standards.rs`); the engine's export of a page and of a component as a book (`crates/adoc-engine/tests/export.rs`); the CLI (`crates/adocfoundry-cli/tests/cli.rs`); the app's export command on GPUI's test platform (`export_command_writes_a_pdf`); and the visual regression against asciidoctor-pdf (`crates/adocfoundry-cli/tests/pdf_regression.rs`).

## Setup

1. `git submodule update --init`, then `cargo build --release -p adocfoundry -p adocfoundry-cli`.
2. Install asciidoctor-pdf 2.3 (`gem install asciidoctor-pdf`, or Homebrew's `asciidoctor`) for the side-by-side checks.
3. A PDF viewer that shows two documents side by side, and a PDF/A and PDF/UA validator (veraPDF) for the standards.

## Reading distance (Definition of Done)

- [x] `adocfoundry-cli pdf tests/fixtures/pdf/reference/article.adoc` and `asciidoctor-pdf` of the same page, side by side: the same four pages, the same page breaks, and every line of text within 1.7 pt of asciidoctor-pdf's (measured line by line on macOS); at reading distance the two cannot be told apart. The visual regression test measures the same pages: a mean grey difference of at most 0.0085 per page and the ink within 1.8 pt.
- [x] The same with `tests/fixtures/pdf/themes/acme-theme.yml` and `:title-page:`: title page with logo, header with the document title, footers with the section title and a bold page number on recto and verso; the same six pages (macOS).
- [x] A real documentation page with the project's own asciidoctor-pdf theme and fonts (`--theme …/jdr-full-theme.yml --fonts-dir …/fonts`): header and footer images, `1 of 5` page numbers, Montserrat and JetBrains Mono, every line within 0.7 pt of asciidoctor-pdf's (macOS).

## Themes

- [x] A custom theme written for asciidoctor-pdf loads without edits: the fixture theme, and eleven themes from a real project, key for key against asciidoctor-pdf's loader (`ADOC_PDF_THEME_CORPUS`, macOS).
- [ ] A theme with `font.fallbacks` renders a glyph the base font lacks (a CJK character with a fallback font installed in the theme).
- [ ] `default-sans` and `default-for-print` side by side with asciidoctor-pdf.

## Books

- [x] `adocfoundry-cli pdf tests/fixtures/pdf/book/guide` (copied elsewhere first): title page with the version, table of contents with parts, chapters and sections, the start page as the preface, each chapter on a new page, the Mermaid diagram drawn, links between pages work in a PDF viewer (macOS).
- [ ] A large real component: every page appears once and in navigation order; the bookmarks panel mirrors the table of contents.
- [ ] `[pdf] site_url` set: a reference to another component opens the published page in the browser.

## Standards

- [ ] `--standard pdf-a-2a` passes veraPDF's PDF/A-2a profile.
- [ ] `--standard pdf-ua-1` passes veraPDF's PDF/UA-1 profile; a screen reader reads headings, lists, tables and image alternatives in order.

## The app

- [ ] Export Page to PDF… from the palette asks where to save (suggesting the page's name), the status bar shows Exporting PDF… while it runs, and the notification's Open opens the PDF.
- [ ] Export Component to PDF… exports the active page's component; unsaved edits are in the PDF.
- [ ] A theme error (a broken `pdf-theme`) and a Typst error (an unreadable image) are reported with the file and line.

## Scale

- [x] Every page of a real 786-page documentation project exports as an article without an error, in about 40 ms a page in a release build (`ADOC_PDF_CORPUS`, macOS).

## Results

| Date | Build | Platform | Result |
|---|---|---|---|
| 2026-10-24 | `phase-4` branch, debug and release | macOS 26 (arm64) | The checks marked above pass. |
