# Phase 3 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Phase 3 acceptance (spec §7 and §10) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ (AppImage) before a release. Record the build, the platform and the result in the table at the end.

Automated coverage for reference: the model's tests (`crates/adoc-model/tests/`: building every block kind, the scanner against the DOM, steps and their inverses, escaping, delimiter forms, block, list, table and image steps), the round-trip property tests (`crates/adoc-model/tests/roundtrip.rs`: identity over every fixture and documentation page, edits staying inside the touched block, and a session of some 700 random steps over five real pages whose line diff touches only the edited lines), the engine's visual tests (`crates/adoc-engine/tests/visual.rs`), and the app's test-platform tests (`crates/adocfoundry/src/ui_tests.rs`) for rendering, typing, clicking, Enter, the toolbar, IME, source-to-visual sync, shared undo and included content.

## Setup

1. `git submodule update --init`, then `cargo run --release -p adocfoundry -- <folder> <page>` on a copy of a real documentation project kept in Git, so `git diff` shows exactly what changed.
2. Open a page with headings, lists, a table, an image, an admonition, a code block and an include (`tests/fixtures/render/blocks.adoc` has all of them).
3. Switch to Visual (button beside the tab, or Cmd/Ctrl-K W).

## The hour of editing (Definition of Done)

- [ ] Edit a real page in Visual mode for an hour: rewrite sentences, bold and italicise, add and remove links, split and join paragraphs, change headings, build and reorder lists, edit table cells, insert an image and a table. Save, then `git diff`: every changed line is one you edited; no other line changed, no blank lines added or removed elsewhere, no attribute lists, comments or delimiters rewritten.
- [ ] `adocfoundry-cli render` of the page before and after shows only the changes you made.

## Typing and selecting

- [ ] Clicking places the caret where clicked; dragging and Shift-click select; Shift with the arrow keys extends the selection, across blocks.
- [ ] Up and Down keep the caret's horizontal position across lines and blocks; Home and End go to the block's start and end.
- [ ] Typing `2 * 3`, `snake_case`, `it's` and `--` writes them as typed; typing `*word*`, `{name}`, a URL or `<<id>>` writes them escaped and they show as typed.
- [ ] Typing `* ` at the start of a paragraph keeps it a paragraph (`{empty}` is written before it).
- [ ] IME: Japanese composes in place, underlined, and commits once (macOS, Windows; ibus on Linux). The candidate window sits at the caret.
- [ ] Copy copies the selected text; Paste types the clipboard's text.

## Formatting and links

- [ ] Cmd/Ctrl-B, -I and -E toggle bold, italic and code on a selection, and with nothing selected on what is typed next; the toolbar buttons show what is on.
- [ ] Removing bold from the middle of a bold word keeps the rest bold (the delimiters double where they must).
- [ ] Insert Link links a selection to a URL, a page (`other.adoc#part`) and an anchor id; changing a link's target keeps its text; Remove Link leaves the text.

## Blocks

- [ ] Enter in the middle of a paragraph, heading and list item splits it; at the end, a new paragraph or item appears and is written only when typed into; Enter on an empty new item leaves the list.
- [ ] Backspace at the start of a heading makes it a paragraph; of a paragraph after another joins them; of a list item outdents or lifts it.
- [ ] The block-type picker turns a paragraph into each heading level, a code block, a quote and each admonition, and back.
- [ ] Bulleted, numbered and checklist buttons make, change and lift lists; Tab and Shift-Tab indent and outdent with nested items; clicking a checkbox toggles it.
- [ ] In a code block, Enter starts a new line and typing never ends the block.

## Tables and images

- [ ] Typing in a cell edits that cell only; a typed `|` is escaped.
- [ ] The Table menu inserts a row below and a column after the caret's, and deletes them; `cols` follows; a table with spans says its structure is edited in the source.
- [ ] Insert → Image… adds an image; clicking an image edits its target and alt text, keeping its other attributes.

## Source and visual together

- [ ] Split mode shows both; typing in the source updates the visual editor shortly after you stop; typing in the visual editor updates the source at once.
- [ ] Each side follows the other's caret.
- [ ] Undo in either editor undoes the last change from either.
- [ ] Included content shows read-only with Edit partial, which opens the partial; clicking a diagram shows its source.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
| 2026-10-23 | `phase-3` (release) | Linux x86_64 (X11 through XWayland) | Partial | Switched to Visual and Split, clicked to place the caret, typed, drag-selected a word and made it bold from the toolbar (the source showed `*plus*`), opened the block-type picker. Found and fixed: a hard line break left a space at the start of the next line, and headings were not bold in their own text runs. Bold text and headings show at regular weight with this machine's IBM Plex Sans, which is installed as a variable font that GPUI draws at its default weight; the preview is affected the same way. The checks above are still to do by hand. |
