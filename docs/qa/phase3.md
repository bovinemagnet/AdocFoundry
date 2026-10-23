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

- [x] Edit a real page in Visual mode for an hour (Linux; see the results for what the session covered): rewrite sentences, bold and italicise, add and remove links, split and join paragraphs, change headings, build and reorder lists, edit table cells, insert an image and a table. Save, then `git diff`: every changed line is one you edited; no other line changed, no blank lines added or removed elsewhere, no attribute lists, comments or delimiters rewritten.
- [x] `adocfoundry-cli render` of the page before and after shows only the changes you made.

## Typing and selecting

- [x] Clicking places the caret where clicked; dragging and Shift-click select; Shift with the arrow keys extends the selection, across blocks.
- [x] Up and Down keep the caret's horizontal position across lines and blocks; Home and End go to the block's start and end.
- [ ] Typing `2 * 3`, `snake_case`, `it's` and `--` writes them as typed; typing `*word*`, `{name}`, a URL or `<<id>>` writes them escaped and they show as typed.
- [ ] Typing `* ` at the start of a paragraph keeps it a paragraph (`{empty}` is written before it).
- [ ] IME: Japanese composes in place, underlined, and commits once (macOS, Windows; ibus on Linux). The candidate window sits at the caret.
- [x] Copy copies the selected text; Paste types the clipboard's text (Linux).

## Formatting and links

- [x] Cmd/Ctrl-B, -I and -E toggle bold, italic and code on a selection, and with nothing selected on what is typed next; the toolbar buttons show what is on.
- [ ] Removing bold from the middle of a bold word keeps the rest bold (the delimiters double where they must).
- [x] Insert Link links a selection to a URL, a page (`other.adoc#part`) and an anchor id; changing a link's target keeps its text; Remove Link leaves the text.

## Blocks

- [x] Enter in the middle of a paragraph, heading and list item splits it; at the end, a new paragraph or item appears and is written only when typed into; Enter on an empty new item leaves the list.
- [x] Backspace at the start of a heading makes it a paragraph; of a paragraph after another joins them; of a list item outdents or lifts it.
- [x] The block-type picker turns a paragraph into each heading level, a code block, a quote and each admonition, and back.
- [x] Bulleted, numbered and checklist buttons make, change and lift lists; Tab and Shift-Tab indent and outdent with nested items; clicking a checkbox toggles it.
- [ ] In a code block, Enter starts a new line and typing never ends the block.

## Tables and images

- [x] Typing in a cell edits that cell only; a typed `|` is escaped.
- [x] The Table menu inserts a row below and a column after the caret's, and deletes them; `cols` follows; a table with spans says its structure is edited in the source (Linux).
- [x] Insert → Image… adds an image; clicking an image edits its target and alt text, keeping its other attributes (Linux).

## Source and visual together

- [ ] Split mode shows both; typing in the source updates the visual editor shortly after you stop; typing in the visual editor updates the source at once.
- [ ] Each side follows the other's caret.
- [x] Undo in either editor undoes the last change from either.
- [x] Included content shows read-only with Edit partial, which opens the partial; clicking a diagram shows its source (Linux).

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
| 2026-10-23 | `phase-3` (release) | Linux x86_64 (X11 through XWayland) | Partial | Switched to Visual and Split, clicked to place the caret, typed, drag-selected a word and made it bold from the toolbar (the source showed `*plus*`), opened the block-type picker. Found and fixed: a hard line break left a space at the start of the next line, and headings were not bold in their own text runs. Bold text and headings show at regular weight with this machine's IBM Plex Sans, which is installed as a variable font that GPUI draws at its default weight; the preview is affected the same way. The checks above are still to do by hand. |
| 2026-10-23 | `phase-3` (release) | Linux x86_64 (X11 through XWayland) | Pass on Linux; other platforms to do | An editing session in the release build on four pages of this repository's documentation, kept in Git, driven with real mouse and keyboard input: some 25 edits — typing, italic, bold and code marks, a link through the dialog, splitting and joining a paragraph, new paragraphs and list items from Enter at a block's end, indenting an item, changing a heading's level and making a paragraph a Tip with the block picker, making a list a checklist and ticking it, inserting a code block, editing a table cell, inserting a table row, undo twice and redo, a sentence typed with backticks and stars (written escaped) and edits inside a code block. Saved and compared with `git diff`: 14 hunks, every changed line one that was edited; `adocfoundry-cli check` reports nothing before or after, every text block of the four pages is still editable, and `render` before and after differs only where the edits were. The session found and fixed five bugs (each with a test): outline navigation did not move the visual caret, long list items did not wrap, Up and Down left a table's column, code was drawn in the proportional font, and Home and End went to the ends of the whole block instead of the line. Not covered here: an hour's length, IME, copy and paste, image dialogs, table columns, included content in the app, and macOS and Windows. |
| 2026-10-23 | `phase-3` (release) | Linux x86_64 (X11 through XWayland) | Pass on Linux; other platforms to do | An hour (3,603 s) of visual editing in the release build on eight pages of this repository's documentation, kept in Git, driven by a script sending real X mouse and keyboard input, saving and committing after every action so each action's diff could be checked alone: 1,137 actions, 706 of them changing the page (typing, marks, deleting, Enter at a block's end, undo, copy and paste, splitting, joining, typing over a selection, the block picker), 366 lines added and 230 removed over the eight pages, no crash. Every commit's diff lies within the block edited, or two adjacent blocks for a join or a new block; no other line changed. One paste changed an xref's target: it went to the source editor, open beside the visual editor after a click on read-only content (the visual editor refuses an insertion there). The session found two bugs, fixed afterwards with tests: commands chosen from the palette did not reach the visual editor, and the list bindings (Shift with a digit) never matched on Linux, now Cmd/Ctrl-Shift-U, -N and -X. A follow-up of some 14 minutes in the rebuilt app made 110 changes, again each within its block, and confirmed both fixes: admonitions, code blocks, table rows and table columns inserted from the palette, and lists made with the new bindings. Cosmetic, left for later: toggling code next to code writes adjacent spans (` ``a````b`` `) where one would do; they render as one run. Not covered: IME, image dialogs, deleting table rows and columns, included content, and macOS and Windows. |
| 2026-10-23 | `phase-3` (release) | Linux x86_64 (X11 through XWayland) | Pass on Linux after fixes; other platforms to do | The checks left open by the hour, run one at a time in the release build on a Git copy of the render fixtures, each change saved and compared with `git diff`. Tables: Delete Table Row and Delete Table Column from the palette removed only the caret's row and column, and `cols` lost the column's width (`1,2,3` became `1,3`); in a table with spans, Insert Table Row said the table's rows and columns are edited in the source and changed nothing. Images: clicking an image opened its dialog and changing the alt text kept the width and height; Insert Image added the image after the caret's block. Links: Insert Link linked the selected word. Included content: the partial's text is not editable from the page, Edit partial opens the partial, and clicking included text opens the partial at that place. Diagrams: clicking one shows its source beside the visual editor. Found and fixed, each with a test: neither dialog focused its first field, so typing went nowhere (which is also why the scripted sessions committed no links); applying the image dialog reported "there is no image here", because the caret had moved past the image to the next text block; block titles were not shown; table cells ignored `cols` and column spans; an AsciiDoc cell showed only its first block; and showing a diagram's source moved the visual caret to the next text block, which in split mode scrolled the source away from the diagram, and the source editor stopped scrolling a few lines short of the place shown. Still open: a row span is drawn in its first row only, as in the preview. |
