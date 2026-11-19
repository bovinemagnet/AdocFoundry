# DocBook Export — Manual Checks

The checks that automated tests cannot make: how DocBook toolchains read the documents AdocFoundry writes (`docs/prd/PRD-AdocFoundry_DocBook_Export.md` §7 and §11; epic #615). The automated tests check every document they write against the official DocBook 5.2.1 RELAX NG schema in process, and run the export's own structural check (ids, links, picture paths); none of that is a DocBook processor, and the schema's Schematron rules are not run. Evidence from consumers is recorded here and is never needed to build, export or test.

No consumer has been checked yet.

## Samples

Export the reference fixture as a book, and one page as an article:

```sh
cargo run -p adocfoundry-cli -- docbook tests/fixtures/docx/reference --component 2.0@manual --output /tmp/docbook/manual.xml
cargo run -p adocfoundry-cli -- docbook tests/fixtures/docx/reference/manual/modules/ROOT/pages/tables.adoc --root tests/fixtures/docx/reference --output /tmp/docbook/tables.xml
```

The book has a preface, two parts, nested sections, pictures (PNG, JPEG and SVG) in `manual_assets/`, a Mermaid diagram, formulas, footnotes (one repeated), CALS tables with spans and a nested table, code with callouts, admonitions, links between chapters and a link out of the book.

## Consumers

| Consumer | Book | Article |
|---|---|---|
| Jing and the DocBook 5.2.1 RELAX NG schema, with the Schematron rules (by hand) | [ ] | [ ] |
| The DocBook XSLT 2 stylesheets (xslTNG), to HTML | [ ] | [ ] |
| The DocBook XSL 1.79 stylesheets, to HTML and to FO/PDF | [ ] | [ ] |
| Oxygen XML Editor (DocBook 5 framework) | [ ] | [ ] |

Record each consumer's version, and the system, beside its result.

## On each consumer

- [ ] The document is accepted without a validation error; record any Schematron warning with what it refers to.
- [ ] The book's preface, parts, chapters and sections are as the navigation gives them, with the component's title and version.
- [ ] Every cross reference lands on its target, including one to another component's page and one to a section of another chapter; the link out of the book shows its text.
- [ ] A footnote and its repeated reference both lead to the one note.
- [ ] Pictures show from `manual_assets/`, with their captions, and the document and its folder still work once moved together elsewhere.
- [ ] Tables keep their header and footer rows, column widths, alignments and spanned cells, and the table inside a cell.
- [ ] Code keeps its whitespace, and its callouts lead to their explanations.
- [ ] Formulas show their pictures, or their source where none was drawn.
- [ ] Keys, buttons and menus, admonitions, sidebars, examples and quotations are rendered as the stylesheets render them.
- [ ] A hard line break (`<?asciidoc-br?>`) is a line break, or ignored without an error.

## Results

Record each run here: the date, the AdocFoundry commit, the consumer and its version, what passed, and what did not, with an issue for each failure.
