# Kindle Export — Manual Checks

The checks that automated tests cannot make: how real Kindle readers show the books AdocFoundry writes (`docs/prd/PRD-AdocFoundry_Kindle_Export.md` §12; epic #616). The automated tests read every book back with readers written apart from the writers (`adoc_kindle::testing`, and the `mobi` crate for AZW3), but no reader of ours is a Kindle. Neither format is called ready until these pass on the readers listed.

No Kindle has been checked yet. KFX is deferred: the maintainer will check it on a Kindle device later, and until then 0.12.0's documentation marks it untested. AZW3 has been opened in Calibre, which is not a Kindle; KFX needs Calibre's third-party KFX Input plugin, since Calibre cannot read KFX without it. Calibre is likely more lenient than a Kindle, and is not known to use the trailing byte sequences the writer leaves out, so a pass there shows the file is well formed and readable, not that a Kindle accepts it.

## Samples

Write the sample book in both formats:

```sh
ADOC_KINDLE_OUT=/tmp/kindle cargo test -p adoc-kindle --test kfx -- --ignored
```

It writes `guide.azw3` and `guide.kfx`: two chapters with a cover, a picture, a nested table of contents, a link out of the book, a link between chapters, a footnote, a list, a code block, a table and Japanese text. For a whole book, export the reference fixture from the command line:

```sh
cargo run -p adocfoundry-cli -- kindle tests/fixtures/docx/reference --component 1.0@manual --format azw3 --output /tmp/kindle/manual.azw3
cargo run -p adocfoundry-cli -- kindle tests/fixtures/docx/reference --component 1.0@manual --format kfx --output /tmp/kindle/manual.kfx
```

Copy each file to the device over USB (into `documents/`), or open it in the app.

## Readers

| Reader | AZW3 | KFX |
|---|---|---|
| A current Kindle e-reader, sideloaded over USB | [ ] | [ ] |
| Kindle for iOS or Android | [ ] | [ ] whether it opens a sideloaded KFX at all |

Record each reader's model and firmware or app version beside its result.

## For each format on each reader

- [ ] The book appears in the library with its title and author, and without a repair prompt or an error.
- [ ] The cover shows in the library. A sideloaded book's thumbnail is the device's choice: note what it shows, and whether it changes after the book is opened.
- [ ] The book opens at its start, the first chapter's title.
- [ ] The table of contents lists the chapters with their sections nested below them, and each entry lands on its heading.
- [ ] A link to another chapter's section lands there, and Back returns.
- [ ] A footnote mark leads to its note, and the note's number leads back.
- [ ] A link out of the book offers to open the address.
- [ ] Text reflows at the largest and smallest font sizes, in both orientations and in the dark theme. Nothing is fixed-width but code.
- [ ] Headings, bold, italic and code look as they should. For KFX, note whether the heading sizes, bold, italic and fixed-width runs show: the style properties KFX here writes are not documented.
- [ ] Lists, the description list, the code block and the table read in order. The table's header row stands out.
- [ ] The picture, the diagram and the formula show, fitted to the screen, and the formula's alternative text is its source.
- [ ] Japanese text shows, and search finds a word of it.
- [ ] Highlighting, notes and the dictionary work on the text.
- [ ] For AZW3, going to a chapter from the table of contents, and paging back and forth across chapters, is as quick as in a KindleGen book. The writer leaves out the trailing byte sequences that index the table of contents by text record, which `epub3-kindle` writes (PRD §16, K3). If a Kindle refuses the book or navigates it badly, they are added.

## Results

| Date | Reader | Format | Result | Notes |
|---|---|---|---|---|
| 2026-11-18 | Calibre 9.18.0's e-book viewer, Linux (not a Kindle) | AZW3 | Pass | Exported with AdocFoundry and opened by the maintainer. The book opened; the table of contents, links, footnotes, cover and pictures worked. |
| 2026-11-18 | Calibre 9.18.0's e-book viewer, Linux (not a Kindle) | KFX | Not tested | Calibre has no KFX reader of its own ("No plugin to handle input format: kfx"), so it read nothing of the file. The third-party KFX Input plugin reads KFX; with it installed, the book is to be opened again. |

If a Kindle refuses KFX, record what it says. Whether to release AZW3 alone or hold the feature is then the maintainer's decision (PRD §15, "If KFX proves blocked").
