# AdocFoundry — Spell Checking Design

Oct 30, 2026 · @Paul Snow

## Summary

Spell checking for the prose of AsciiDoc pages (issue #65). Misspelt words are underlined in the source editor and the visual editor, with suggestions, Add to Workspace Dictionary and Add to User Dictionary in the context menu, and are published by `adoc-ls` as hint diagnostics with quick fixes. Checking uses `spellbook` (pure Rust, Hunspell compatible, MPL-2.0) with bundled SCOWL `en_GB` and `en_US` dictionaries and an optional folder of further Hunspell dictionaries.

**Done when:** a misspelt word in a paragraph is underlined and a suggestion replaces it in one undo step; words in a source block are never flagged.

**Decisions**

| Question | Decision |
| --- | --- |
| Checker | `spellbook` 0.4.2, confined to a new crate `adoc-spell` |
| Dictionaries | SCOWL `en_GB-ise` and `en_US` (size 60) bundled; a configured folder adds or replaces others. LibreOffice's LGPL `en_GB` is not used. |
| What is prose | Text pieces of editable, non-plain text blocks of the visual editor's model (`adoc_model::Model`), outside inline code and atoms |
| Surfaces | Source editor, visual editor, `adoc-ls` |
| Word lists | Workspace: `.adocfoundry-words.txt` at each workspace root. User: `spelling-words.txt` beside the user settings file. |
| Problems panel, CLI `check` | Not included: misspellings are never engine diagnostics |
| Default | On, `en_GB` when a page has no `lang` |
| Not included | Ignore once / ignore all; checking link and xref text and footnotes; the CLI, `adoc-mcp`, preview and PDF export |

## What is checked

The engine parses the page with `adoc-dom`, builds an `adoc_model::Model` from the DOM and the text, and walks `Model::text_blocks()`. A word is checked only when all of these hold:

- Its node's `access` is `Access::Editable`. Included content (`ReadOnly::Included`) is checked when its own file is open; verbatim constructs, inactive conditionals and `ReadOnly::Unchecked` blocks are never checked.
- Its `TextBlock` is not `plain`, so listing, source and literal blocks are never checked.
- It lies within a `Piece::Text` that is not inside a `Piece::Mark` whose mark is `Mark::Code`. Atoms (xrefs, links and bare URLs, attribute references, anchors, macros, footnotes, index terms, passthroughs, replacements, escapes) are never checked, nor is the text shown inside them.

### Words

Each text piece is split by Unicode word boundaries (UAX #29, `unicode-segmentation`). A segment holding a letter is a candidate word. A right single quotation mark (`’`) is read as an apostrophe (`'`) for checking, so *don’t* is one word; hyphenated words are checked part by part, as UAX #29 splits them.

A candidate is skipped, never flagged, when it:

- holds a digit, `_` or `.` (`v2`, `snake_case`, `file.txt`, `e.g`);
- is a single letter;
- is written entirely in capitals (`HTML`, `API`);
- has a capital letter after its first character (`GitHub`, `iOS`, `camelCase`);
- touches the start or end of its text piece where the neighbouring piece is a mark delimiter or an atom with no white space between (`**b**old`, `{product}s`), so a fragment of a word is never flagged.

### Language

The page's language is its `lang` attribute as the DOM resolves it (the page header, or the component's attributes in `antora.yml`), else the setting `[spelling] language` (default `en_GB`). Tags are normalised: `en-GB`, `en_GB` and `en_gb` name the same dictionary.

- An exact match is used.
- A bare language (`en`, which Asciidoctor sets by default) uses the configured language when it is the same language (`en` → `en_GB`), else the first available dictionary of that language in name order.
- A language with no dictionary is not checked, and no message is shown. The doctor report lists the dictionaries found.

Case follows Hunspell's rules: a dictionary word in lower case is accepted capitalised and in capitals.

## `adoc-spell`

A new crate in layer L1. It depends on `adoc-core`, `spellbook`, `unicode-segmentation` and `foldhash` only, and knows nothing of AsciiDoc. It is the only crate that may name `spellbook`; `cargo xtask layers` gains that confinement rule, as for the parser crates in `adoc-dom`.

- `Dictionaries` — the bundled `en_GB` and `en_US` (`include_bytes!`, parsed on first use) and the `.aff`/`.dic` pairs in an optional folder, named by file stem. A folder dictionary replaces a bundled one of the same name. `resolve(lang, configured) -> Option<String>` implements the language rules above. `load(name) -> Result<Arc<Checker>, SpellError>` parses and caches.
- `Checker` — `check(word, lists) -> bool` and `suggest(word) -> Vec<String>` (at most five).
- `words(text) -> impl Iterator<Item = Range<usize>>` — the candidates of plain text after the skip rules that need no context; the engine applies the piece-boundary rule.
- `WordList` — a plain-text file, one word per line, kept sorted and de-duplicated, written with a trailing newline. A listed word accepts itself, its capitalised form and its all-capitals form. Lines that are blank or begin with `#` are ignored. A missing file is an empty list.
- Errors are typed (`SpellError`); no `anyhow`.

## The engine

`crates/adoc-engine/src/spelling.rs`, following the pattern of `words.rs`.

- `Engine::enable_spelling()` opts a front end in. The app and `adoc-ls` call it; the CLI and `adoc-mcp` do not, and never parse a page for spelling.
- With spelling enabled and `[spelling] enabled = true`, each open AsciiDoc document whose version is unchecked is checked on a worker once it has gone unchanged for `SPELLING_SETTLE` (500 ms). One job runs at a time; the active document goes first.
- The worker parses the page, builds the model, collects the prose words, checks them, and computes suggestions for each distinct misspelt word, at most 100 distinct words a page, so a context menu never waits for them.
- `Engine::misspellings(id) -> &[Misspelling]`, where `Misspelling { range: SourceRange, word: String, suggestions: Vec<String> }`, announced by `EngineEvent::SpellingChanged(id)`. A result for a version that is no longer current is dropped. Closing a document forgets its result.
- Between checks, every edit to a document shifts the stored ranges after it and drops the ranges it overlaps, so underlines stay with their words while typing.
- Parsed dictionaries are cached in the engine and shared with workers through `Arc`; there is no global state.
- Word lists are read when a workspace opens. A workspace's `.adocfoundry-words.txt` is read again when the watcher reports a change to it (after a `git pull`, say); the user list, outside the workspace and unwatched, is read again only when the engine writes it. Open documents are then checked again.

### Commands

- Replacing a word with a suggestion uses `edit.replace_range` with `CommandArgs::Text`, already one step in the document's undo history in both editors.
- `spelling.add_to_workspace` — `CommandArgs::Range`: reads the word at the range, adds it to `.adocfoundry-words.txt` at the root of the workspace folder holding the document, and checks the open documents again. Each root's list applies to the pages beneath it.
- `spelling.add_to_user` — `CommandArgs::Range`: the same, for `spelling-words.txt` beside the user settings file, applying to every workspace. Unavailable without a user settings file.

Both are listed in the app's `NOT_IN_MENUS`: they act on a word and are offered from the context menus.

### Settings

```toml
[spelling]
enabled = true          # user and workspace files
language = "en_GB"      # for pages with no lang attribute; user and workspace files
dictionaries = ""       # folder of extra .aff/.dic files; user file only
```

`dictionaries` is accepted only in the user file, as `tools.*` is. An invalid value warns and keeps the layer beneath, as for other keys. An unreadable folder or dictionary is reported by the doctor, not as a settings warning.

### Doctor

`Engine::doctor` gains the spelling section: whether spelling is enabled, the configured language, and each dictionary found with its source (bundled or the folder's path). `adocfoundry-cli doctor` and Help › Show Diagnostics show it.

## Front ends

### Source editor

- Misspellings are added to the editor's diagnostic set at `Hint` severity, which gpui-component draws as a wavy underline, with the message "Unknown word: *word*" on hover. `show_diagnostics` merges the engine's diagnostics and the misspellings and runs on `DiagnosticsUpdated` and `SpellingChanged`. The Problems panel reads the engine's diagnostics, so it never lists misspellings.
- A right-click on a misspelt word adds, above the editor's own items: up to five suggestions, a separator, Add to Workspace Dictionary, and Add to User Dictionary when a user settings file exists. A suggestion sends `edit.replace_range`; the add items invoke the commands with the word's range. The menu uses the input's `context_menu` builder.
- To be verified first: that the caret is at the right-clicked position when the builder runs. If gpui-component does not move it, the editor records the position on the right mouse-down and maps it to an offset itself.

### Visual editor

- Each misspelt source range within a text block is mapped to display offsets through the block's source-to-display mapping (the one inline formulas use), and a wavy underline is painted beneath the laid-out glyphs in `visual/text.rs`.
- A right-click over a misspelt word opens a menu with the same items. Elsewhere, right-click does nothing, as today. A replacement goes through `edit.replace_range`; the model is rebuilt and the selection mapped as for any edit made elsewhere.

### `adoc-ls`

- Misspellings are published as `DiagnosticSeverity::HINT` with code `adoc.spelling` and source `adocfoundry`, merged with the engine's diagnostics in each `publishDiagnostics`, and published again on `SpellingChanged`.
- `textDocument/codeAction` offers, for each spelling diagnostic in the request's range: a `quickfix` "Change to *suggestion*" per suggestion, carrying a `WorkspaceEdit`; and "Add *word* to the workspace dictionary" and "Add *word* to the user dictionary", running the commands `adoc.spelling.addToWorkspace` and `adoc.spelling.addToUser`, which are listed in `executeCommandProvider` and invoke the engine's commands.
- `adoc-ls` has no Cargo feature for spelling; the bundled dictionaries are part of every build.

## Dictionaries and licences

- `crates/adoc-spell/dictionaries/en_GB.aff`, `en_GB.dic`, `en_US.aff`, `en_US.dic`: SCOWL's Hunspell builds (`en_GB-ise` and `en_US`, size 60) of the latest release from the SCOWL project (wordlist.aspell.net), renamed.
- `crates/adoc-spell/dictionaries/README.md` records the release, its download URL and the SCOWL copyright and permission notice, which must appear in all copies.
- `cargo xtask package` copies the notice into every release archive as `licenses/SCOWL.txt`.
- `cargo deny` checks `spellbook` (MPL-2.0) and `foldhash` (Zlib); it does not see data files, so the README is the record for the dictionaries.

## Testing

Each piece is test-first.

- `adoc-spell` unit tests: the skip rules as a table (`GitHub`, `HTML`, `v2`, `file.txt`, `don’t`, single letters); language resolution (`en`, `en-GB`, `en_gb`, `de` with no dictionary, a folder dictionary replacing a bundled one); `WordList` round trip, sorting and casing; `en_GB` accepts *colour* and flags *color*, `en_US` the reverse; suggestions for *recieve* include *receive*.
- `adoc-engine` `tests/spelling.rs`, on fixtures in `tests/fixtures/spelling/`:
  - `a_misspelt_word_in_a_paragraph_is_found` with its byte range and suggestions;
  - `words_in_a_source_block_are_never_flagged`, with literal blocks, inline code, xrefs, links, attribute references and included content;
  - the word-fragment rule beside marks and atoms;
  - `:lang: en-US` switching the dictionary, and a language with no dictionary checking nothing;
  - settling under `ManualClock`, and a result for a superseded version dropped;
  - ranges shifting with edits and dropped where an edit touches them;
  - `a_misspelt_word_is_replaced_in_one_undo_step`, through `edit.replace_range` and one undo;
  - `spelling.add_to_workspace` and `spelling.add_to_user` writing their files and clearing the word; an edited list re-read through the watcher;
  - `[spelling] enabled = false` and an engine without `enable_spelling` checking nothing.
- `adoc-engine` settings tests: the three keys by layer, `dictionaries` refused in the workspace file, invalid values.
- `adocfoundry` `ui_tests`: `a_misspelt_word_is_underlined_and_replaced_in_one_undo_step` in the source editor and in the visual editor; Add to Workspace Dictionary from the menu.
- `adoc-ls` `tests/stdio.rs`: the hint diagnostic, the quick fix's edit, and `adoc.spelling.addToWorkspace`.
- `adoc-engine` `tests/perf.rs` (ignored): checking a long page takes well under `SPELLING_SETTLE`.

## Documentation

- `README.md`: spelling leaves the not-implemented list; the feature and the `[spelling]` settings are described.
- `CHANGELOG.md`: Unreleased › Added.
- `src/docs`: `settings.adoc` (the `[spelling]` keys), `using-the-editor.adoc` (a Spelling section: what is checked, languages, dictionaries, word lists), and the developer module's crate ladder.
- The crate ladder in the project guide, `LAYERS` in `xtask/src/layers.rs`, and the architecture design doc.

## Delivery

One branch, `feat/spell-checking`, and one pull request closing #65, built in commits that each leave the workspace green: `adoc-spell`; the engine and settings; `adoc-ls`; the source editor; the visual editor; documentation.

## Implementation notes

Where the build departs from this design:

- The spelling commands have no keymap action, so they need no `NOT_IN_MENUS` entry. They are listed with the other commands driven by clicks rather than chosen by name in the palette test (`every_command_is_in_the_palette`).
- `adoc-spell` depends on `spellbook` and `unicode-segmentation` only, not `adoc-core`; `foldhash` comes through `spellbook`.
- `adoc-ls` gives its spelling hints the source `adoc-ls`, like its other diagnostics. They are tested in `protocol.rs`'s unit tests rather than `tests/stdio.rs`.
- A word list also accepts a listed word followed by a possessive `'s`.
- The visual editor underlines misspelt words through its text runs' wavy underline rather than painting them.
- The source editor's context menu replaces the editor component's built-in menu, which a custom menu cannot extend. It lists the built-in items after the spelling choices, with the same enabled states, apart from Show Code Actions, which the editor never enables because it has no code action provider.
- The app's UI tests cover replacing a word from the source editor and from the visual editor's menu, but not Add to Workspace Dictionary from the menu; the engine's tests cover that command.
