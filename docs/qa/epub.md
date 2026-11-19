# EPUB Export — Manual Checks

The checks that automated tests cannot make: how real reading systems show the books AdocFoundry writes (`docs/prd/PRD-AdocFoundry_EPUB_Export.md` §10 and §11; epic #614). The automated tests read every book back in process with the crate's own ZIP reader and an XML parser: the container, the package, the navigation, well-formed XHTML, and every link and picture reference (`adoc_epub::testing`). None of that is a reading system, and none of it is EPUBCheck. EPUB export is not called ready until these checks pass on the readers listed.

No reader has been checked yet.

## Samples

Export the reference fixture as a book, and one page as an article:

```sh
cargo run -p adocfoundry-cli -- epub tests/fixtures/docx/reference/manual --component 2.0@manual --output /tmp/epub/manual.epub
cargo run -p adocfoundry-cli -- epub tests/fixtures/docx/reference/manual/modules/ROOT/pages/tables.adoc --root tests/fixtures/docx/reference --output /tmp/epub/tables.epub
```

The book has two components' pages, nested navigation, a cover, pictures (PNG, JPEG and a drawn SVG), formulas, footnotes, tables with spans and a nested table, code, admonitions, links between chapters and a link out of the book. Open each file in the readers below.

## Readers

| Reader | Book | Article |
|---|---|---|
| Apple Books (macOS or iOS) | [ ] | [ ] |
| Thorium Reader (Windows, macOS or Linux) | [ ] | [ ] |
| Calibre's viewer | [ ] | [ ] |
| KOReader, or an e-reader with EPUB support (Kobo, PocketBook) | [ ] | [ ] |

Record each reader's version, and the device and system, beside its result.

## On each reader

- [ ] The book opens without a repair prompt or an error, with its title and author in the library.
- [ ] The cover shows in the library and as the first page.
- [ ] The book opens at its cover, then the first chapter; the reader's "go to start" goes to the first chapter.
- [ ] The table of contents lists the parts and pages with their sections nested below them, and each entry lands on its heading.
- [ ] A link to another chapter's section lands there, and Back returns.
- [ ] A footnote mark leads to its note (or shows it as a pop-up), and the note's number leads back.
- [ ] A link to a web address opens it, or asks to.
- [ ] Changing the font size reflows the text: no line runs off the page, code wraps, and pictures shrink to the page with their proportions kept.
- [ ] Changing the font, and a dark or sepia theme, are honoured; the text stays readable in each (note anything drawn white on white).
- [ ] Tables show their header row, their spanned cells, and the table inside a cell.
- [ ] Admonitions, sidebars and examples are set apart; keys show as keys.
- [ ] Formulas show as pictures; Japanese and Greek text shows in a suitable font.
- [ ] Read aloud (VoiceOver with Apple Books, or the reader's own): the book is read in its language, headings are announced as headings, and a picture is read as its description.

## EPUBCheck, outside the tests

EPUBCheck is not part of the tests or the build (PRD §7). The `epubcheck` CI job runs it over the reference book as a separate job (EPUB 9/9). To run it by hand:

```sh
java -jar epubcheck.jar /tmp/epub/manual.epub
```

- [ ] No error and no warning; record the EPUBCheck version, and any message with what it refers to.

## Results

Record each run here: the date, the AdocFoundry commit, the reader and its version, what passed, and what did not, with an issue for each failure.
