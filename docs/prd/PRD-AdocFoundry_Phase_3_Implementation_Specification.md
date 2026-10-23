# PRD: AdocFoundry — Phase 3 Implementation Specification

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Date | 2026-10-23 |
| Status | Proposed |
| Scope | Phase 3: WYSIWYG editing |
| Depends on | `docs/design/DESIGN-AdocFoundry_Architecture.md` (Phase 3 section), Phase 2 as merged |

## 1. Document Purpose

Phase 3 makes the rendered document editable. The source text stays the source of truth: the visual editor is a projection of it, every visual edit is a step on `adoc-model` that becomes a minimal edit of the source, and text the model does not understand is never rewritten.

Phase 3 must provide:

- `adoc-model`: an editable, typed tree built from `adoc-dom`'s AST and the page text, in which every inline piece knows the exact source bytes it came from; steps with their inverses; position mapping.
- A serialiser that follows the round-trip rules, with a property test that building and serialising is the identity on the fixture corpus, and that edits touch only what was edited.
- An engine model service: a visual model per document shown in visual mode, kept in step with the source, and the visual editing commands, all recorded in the same undo history as source edits.
- The app's visual editor: caret, selection, hit testing and IME composition over rendered text; inline editing (text, marks, links); block editing (paragraphs, headings, lists, admonitions, code blocks); tables and images; source, visual and split modes; a formatting toolbar and block-type picker; every command in the palette.

## 2. Decisions

| Question | Decision | Consequence |
|---|---|---|
| prosemirror-rs | **evaluated and not adopted**; its design (steps with inverses, step maps, marks on text) is followed in-house | See §2.1. |
| Source of truth | the page text, as in Phases 1 and 2 | The model is rebuilt from source whenever the two could differ; a visual edit is a source edit. |
| Positions | **byte offsets into the page source**, at the caret stops the model defines | One coordinate system across engine, source editor and visual editor; a selection survives a model rebuild unchanged and maps through any edit like a source selection. |
| Inline structure | `adoc-model` scans each text block's source **losslessly** (every byte belongs to a text run, a mark delimiter or an atom) and **checks the result against the DOM** | No change to `adoc-dom`'s inlines; when the scan and the parser disagree about a block, the block is read-only rather than wrong. |
| Undo | the engine's existing undo history; a visual step's inverse is the source edits that restore the text before it | Undo and redo are shared by the source and visual editors with no second history. |
| Local application | typing, deletion and mark changes inside one text block update the model in place (the block is rescanned); structural steps rebuild the model from a fresh parse | Typing stays immediate on long pages (a full acdc parse of a 2,000-line page takes about 12 ms in a release build); structure is always exactly what the parser sees. |
| Reconciliation | after a visual edit, the model is rebuilt from a parse once edits pause for `MODEL_SETTLE` (100 ms) | Anything the local update approximated is corrected from source. |
| Typed characters that would become markup | escaped, then checked by rescanning; refused when no escape keeps the meaning | Typing `*` in visual mode never makes text bold. |
| Includes | read-only in the visual editor, with "Edit partial" | As the design's rule 4; editing inside includes is a follow-on. |
| Conditionals | the active branch is editable; an inactive branch is preserved and shown collapsed | As rule 5; directives are never touched. |
| Editor mode | per document, engine state: `Source`, `Visual` or `Split` | Front ends render the mode; the engine keeps a model only for documents shown visually. |

### 2.1 prosemirror-rs spike

`prosemirror` 0.5.2 (MIT, fiduswriter) was built against an AsciiDoc-shaped schema (doc, section, heading, paragraph, admonition, listing, list and item, an atom node for verbatim source; strong, emphasis and code marks). Replace steps applied, inverted and mapped positions correctly, and the crate passes `cargo deny` with the workspace policy. It is not adopted because:

- **Positions are UTF-16 code units**, for parity with JavaScript. Every AdocFoundry crate works in byte offsets of the source; a second coordinate system would run through the whole edit path.
- **Schemas and attributes are runtime JSON.** Node kinds are strings and attributes `serde_json::Value`s, so the typed tree the design asks for would be a layer of conversions on top.
- **Node types live in a thread-local global store** (`with_types` scopes, leaked interned names), against the workspace rule to avoid global mutable state; every engine worker would need the scope.
- **It enables `serde_json`'s `preserve_order` feature**, which feature unification would turn on for the whole workspace and change the key order of every JSON output.
- What it adds over an in-house model is ProseMirror's document-position arithmetic. With the source as the truth, a step's effect is a set of source edits, and source offsets already map through edits.

## 3. Architecture

```text
L4  adocfoundry (visual/ module)   adocfoundry-cli   adoc-ls
L3  adoc-engine   model service: editor mode → model build → visual commands → source edits → undo
L2  adoc-model (tree, scanner, steps, serialiser)   adoc-render   adoc-diagrams
L1  adoc-dom   adoc-parser   adoc-antora   adoc-index   adoc-lang
L0  adoc-core
```

`adoc-model` depends on `adoc-core` and `adoc-dom`. The engine depends on it. The app sees the model only through the engine.

Data flow of a visual keystroke: the visual editor holds a selection in source offsets → it invokes `visual.insert_text` with the selection and the marks the text should carry → the engine asks the model for the step's result (source edits, the selection after, and the updated model when the step is local) → the edits are applied to the document and recorded in undo history → `DocumentChanged` and `ModelChanged` are published → the source editor applies the edits to its buffer, the visual editor redraws from the model, and the preview re-renders after its settle time.

A change from anywhere else (the source editor, undo, replace, a reload from disk) marks the model stale. A typed source edit rebuilds it after `MODEL_SETTLE`; any other command rebuilds it before returning. Visual commands are refused while the model is stale.

## 4. `adoc-model` (WP 3.1)

```rust
pub struct Model {
    pub path: PathBuf,           // the page
    pub source: String,          // the text the model describes
    pub blocks: Vec<Node>,
}

pub struct Node {
    pub kind: NodeKind,
    pub range: SourceRange,      // the node's whole source in the page, metadata lines included
    pub access: Access,
    pub text: Option<TextBlock>, // a title, a paragraph's text, a list item's principal text, code
    pub children: Vec<Node>,
    pub origin: Option<Vec<usize>>, // the DOM trail it was built from, for read-only rendering
}

pub enum Access {
    Editable,
    ReadOnly(ReadOnly),          // shown, never edited in place
}
pub enum ReadOnly { Verbatim, Included { path: PathBuf, target: String }, Inactive, Unchecked }

pub enum NodeKind {
    Section { level: u8 }, Heading { level: u8 }, Paragraph,
    List { kind: ListKind }, ListItem { marker: String, checked: Option<bool> },
    DescriptionList, DescriptionItem,
    Admonition { kind: AdmonitionKind, delimited: bool },
    Code { language: Option<String> }, Literal,
    Quote, Sidebar, Example, Open,
    Table { columns: usize }, Row { section: RowSection }, Cell,
    Image { target: String, alt: String },
    Include { target: String }, Conditional { directive: String },
    Verbatim { name: String },   // anything else: diagrams, STEM, passthrough, attribute entries, …
}

pub struct TextBlock {
    pub range: SourceRange,      // the inline content only: `Title` of `== Title`
    pub pieces: Vec<Piece>,      // tile `range` exactly, in source order
    pub plain: bool,             // code and literal text: no marks or atoms
}

pub enum Piece {
    Text { range: SourceRange },                              // shown exactly as written
    Mark { mark: Mark, open: SourceRange, close: SourceRange, children: Vec<Piece> },
    Atom { atom: Atom, range: SourceRange },                  // shown as a unit, never split
}
pub enum Mark { Strong, Emphasis, Code, Highlight, Superscript, Subscript }
pub struct MarkSet(..);
pub enum Atom {
    Xref { target: String, text: Option<String> }, Link { target: String, text: Option<String> },
    Image { target: String, alt: String }, Footnote { text: String }, Anchor { id: String },
    Attribute { name: String, value: Option<String> }, Replacement { shown: String },
    Escape { shown: String }, Passthrough { shown: String }, LineBreak, Macro { name: String, shown: String },
}

impl Model {
    pub fn build(path: &Path, source: &str, dom: &Dom) -> Model;   // never panics
    pub fn text_block_at(&self, offset: usize) -> Option<TextBlockRef<'_>>;
    pub fn caret_stops(&self, offset: usize) -> Vec<CaretStop>;   // source offsets showing one display position
    pub fn display_offset(&self, offset: usize) -> Option<(BlockPath, usize)>;
    pub fn source_offset(&self, block: &BlockPath, display: usize, marks: MarkSet) -> Option<usize>;
    pub fn apply(&self, step: &Step) -> Result<StepResult, StepError>;
}

pub struct StepResult {
    pub edits: Vec<SourceEdit>,      // in pre-step coordinates, sorted, non-overlapping
    pub selection: SourceRange,      // in post-step coordinates
    pub model: Option<Model>,        // the updated model, for local steps; `None` means rebuild
}
pub fn invert(before: &str, edits: &[SourceEdit]) -> Vec<SourceEdit>;   // restores `before`
pub struct Mapping { .. }            // maps source offsets through edits, with an association side
```

A text block's **display text** is its pieces' text in order: a `Text` piece shows its source exactly (a line break inside a paragraph is one byte shown as a space), an atom shows its `shown` text, a mark shows its children. Display offsets are bytes of the display text. A **caret stop** is a source offset inside a `Text` piece or at a piece edge; at an edge shared with a mark delimiter there are two stops (inside and outside the mark), told apart by the marks the caret carries.

**Checked against the DOM.** A text block is editable only when its display text, with marks removed and atoms shown as the DOM shows them, equals the DOM's plain text for the same block; otherwise it is `ReadOnly(Unchecked)`. Blocks the scanner does not model (diagrams, STEM, passthroughs, unsupported blocks, attribute entries, AsciiDoc table cells) are `Verbatim`. Content from an include is `Included`, grouped under one `Include` node at the directive. An inactive conditional branch is a `Conditional` node.

Steps (every position is a source offset; ranges may span blocks only where stated):

| Step | Effect |
|---|---|
| `InsertText { at, text, marks }` | Inserts at a caret stop; escapes characters that would become markup; adds missing marks by wrapping. Local. |
| `Delete { range }` | Within one block: removes text, whole atoms and emptied marks. Local. Across blocks: joins the first and last text blocks. Structural. |
| `ToggleMark { range, mark }` | Adds the mark where any of the range lacks it, else removes it; splits or extends mark pieces, keeping their delimiter form. Local per block; may span blocks. |
| `SplitBlock { at }` | Enter: splits a paragraph, heading, list item or admonition paragraph; a new line in code. |
| `JoinBackward { at }` | Backspace at the start of a text block: joins with the previous paragraph, or lifts a list item out of its list. |
| `SetBlockType { at, kind }` | Paragraph, heading 1–5, admonition paragraph, code block, quote. |
| `ToggleList { at, kind }`, `IndentListItem`, `OutdentListItem`, `ToggleChecked` | List structure and markers. |
| `SetLink { range, target, text }`, `RemoveLink { at }` | Wraps a selection in a URL or xref, edits one, or replaces it with its text. |
| `InsertBlock { at, block }` | After the block at `at`: image, table, code block, admonition, thematic break. |
| `SetImage { at, target, alt }` | Edits a block image's target and alt text, keeping its other attributes. |
| `InsertTableRow`, `DeleteTableRow`, `InsertTableColumn`, `DeleteTableColumn` | For tables whose rows the model can address. |
| `Replace { edits }` | Raw source edits: the form inverses take. |

Every step's result is verified by rescanning the blocks it touched: the text and marks they show must be the ones the step intended. A step that cannot be expressed without changing the meaning of other text fails with `StepError::WouldChangeMeaning` and changes nothing.

Acceptance tests: `builds_every_block_kind`, `pieces_tile_every_text_block`, `scanner_agrees_with_the_dom`, `disagreeing_blocks_are_read_only`, `steps_invert_exactly`, `mapping_follows_edits`, `typed_markup_is_escaped`, `toggle_mark_keeps_delimiter_form`, `included_content_is_read_only`, `half_typed_input_does_not_panic`.

## 5. Serialiser and round-trip rules (WP 3.2)

`Model::serialise()` writes the document from its nodes, pieces and the gaps between them (blank lines, comments, attribute entries, directives). The rules:

1. **Untouched nodes keep their source byte for byte**, attribute lists, block titles, comments and blank-line spacing included. A step's edits lie inside the text blocks or block lines it touched.
2. **Touched nodes keep their style**: a mark keeps its delimiters (`*` or `**`, with any role), a list item its marker (`*`, `-`, `**`, `.`), an admonition its form (`NOTE:` or `[NOTE]` with `====`), a code block its delimiter length. New constructs use `*`, `_`, `` ` `` and `#` constrained when the boundaries allow, unconstrained otherwise; `*` and `.` for list markers; `----` for code.
3. **Resource ids are kept as written.** Xrefs, links, images and includes are atoms; only `SetLink` and `SetImage` rewrite them, and only the part the user changed.
4. **Includes** render in place and are read-only; "Edit partial" opens the partial in the source editor.
5. **Conditionals** (`ifdef`, `ifndef`, `ifeval`) show the branch that applies; an inactive branch is shown collapsed and kept verbatim.

Acceptance tests: `serialise_is_identity_on_the_corpus` (every fixture under `tests/fixtures/` and every page of `src/docs`), `edits_stay_inside_the_touched_block` (random steps at random caret stops), `a_long_editing_session_touches_only_edited_lines` (a scripted session of several hundred steps over real pages: every line that differs from the original in a line diff is one a step touched; this is the phase's Definition of Done in test form).

## 6. Engine model service (WP 3.4, 3.7)

```rust
pub const MODEL_SETTLE: Duration = Duration::from_millis(100);
pub enum EditorMode { Source, Visual, Split }
pub struct VisualDocument { pub document: DocumentId, pub version: u64, pub model: Model, pub view: ViewTree }
impl Engine {
    pub fn editor_mode(&self, id: DocumentId) -> EditorMode;
    pub fn visual(&self, id: DocumentId) -> Option<Arc<VisualDocument>>;   // `None` while none is built
    pub fn refresh_visual(&mut self, id: DocumentId);                      // rebuild now if stale
}
EngineEvent::ModelChanged { id: DocumentId, version: u64 }
EngineEvent::EditorModeChanged(DocumentId)
CommandArgs::Visual(VisualArgs { id, selection: SourceRange, marks: MarkSet, value: VisualValue })
CommandOutcome::Selection(SourceRange)
```

`VisualDocument::view` is the view tree built from the same parse, used to draw read-only nodes (images, diagrams, included content) exactly as the preview does; the model's `origin` trails index into it.

Commands:

| Id | Title | Binding (visual editor) |
|---|---|---|
| `view.show_source`, `view.show_visual`, `view.show_split` | Show Source / Visual / Split | `cmd-k s`, `cmd-k w`, `cmd-k d` |
| `visual.insert_text` | Type | — (text input) |
| `visual.delete_backward`, `visual.delete_forward` | Delete | `backspace`, `delete` |
| `visual.split_block` | New Paragraph | `enter` |
| `edit.toggle_strong`, `edit.toggle_emphasis`, `edit.toggle_code`, `edit.toggle_highlight` | Bold, Italic, Code, Highlight | `cmd-b`, `cmd-i`, `cmd-e`, `cmd-shift-h` |
| `edit.set_block_type` | Set Block Type | — (block picker) |
| `edit.toggle_bullet_list`, `edit.toggle_numbered_list`, `edit.toggle_checklist` | Lists | `cmd-shift-u`, `cmd-shift-n`, `cmd-shift-x` (see §11) |
| `edit.indent_list_item`, `edit.outdent_list_item` | Indent, Outdent | `tab`, `shift-tab` |
| `edit.insert_link`, `edit.remove_link` | Insert Link, Remove Link | `cmd-shift-l` |
| `edit.insert_image`, `edit.edit_image` | Insert Image, Edit Image | — |
| `edit.insert_table`, `edit.insert_table_row`, `edit.delete_table_row`, `edit.insert_table_column`, `edit.delete_table_column` | Tables | — |
| `edit.insert_code_block`, `edit.insert_admonition` | Insert Code Block, Insert Admonition | — |

Undo: a run of visual typing coalesces like source typing; every other visual command is one undo group. Undo and redo apply to the document whichever editor invoked them, and rebuild the model before returning.

Acceptance tests: `visual_typing_edits_the_source`, `visual_typing_is_one_undo_step`, `undo_restores_source_and_model`, `source_edits_rebuild_the_model_after_settle`, `stale_model_refuses_visual_edits`, `structural_steps_rebuild_the_model`, `included_files_rebuild_the_model`, `editor_mode_requests_a_model`.

## 7. App visual editor (WP 3.3–3.6, 3.8)

```text
crates/adocfoundry/src/visual/
├── mod.rs        VisualView: renders a VisualDocument, holds the selection, dispatches commands
├── text.rs       an editable text element: styled runs, wrapping, caret, selection, hit testing
├── input.rs      keyboard actions and the platform input handler (typing, IME composition)
├── blocks.rs     model nodes → elements; read-only nodes through the preview's block renderer
└── toolbar.rs    formatting toolbar and block-type picker
```

- **Modes**: each editor tab shows Source, Visual, or both side by side (Split); the mode is engine state.
- **Caret and selection**: a selection is a pair of source offsets. Clicking, dragging, shift-clicking and the arrow, Home and End keys move it within and between text blocks; the caret carries the marks of the text before it. Selection is drawn across blocks.
- **Hit testing**: a click maps to the nearest caret stop in the text block under the pointer; clicking a read-only node selects it and offers "Edit source" (or "Edit partial").
- **IME**: the view is the window's input handler while focused; composition is shown underlined in place and committed as one `visual.insert_text`.
- **Inline editing**: typing, deletion, the mark commands, links (a small dialog for target and text).
- **Block editing**: Enter, Backspace at the start of a block, block-type picker, lists (toggle, indent, outdent, checkboxes), admonitions, code blocks (plain monospace text; Enter inserts a line).
- **Tables and images**: cell text is edited in place; rows and columns through the toolbar; images through a dialog (target and alt), drawn from their resolved file.
- **Toolbar**: block-type picker, Bold, Italic, Code, Highlight, Link, lists, indent and outdent, Insert (image, table, code, admonition), mode switch. Every command is in the palette.

App tests (test platform): `visual_mode_renders_the_document`, `typing_in_visual_mode_edits_the_source`, `clicking_text_places_the_caret`, `enter_splits_the_paragraph`, `toolbar_bold_toggles_strong`, `source_edits_update_the_visual_editor`, `undo_is_shared_between_editors`, `ime_composition_commits_once`, `included_content_is_not_editable`.

## 8. Source-only in Phase 3

Rendered read-only in the visual editor, with a one-click jump to the source: tables with AsciiDoc (`a`) cells, STEM, attribute entries, extension and unknown macros, diagrams (their source is edited in the source editor), passthroughs, anything the scanner and the parser disagree about, and included content.

## 9. Implementation Sequence

| # | Package | Depends on |
|---|---|---|
| 3.1 | `adoc-model`: tree, scanner, positions, steps, inverses, mapping; prosemirror-rs evaluation (§2.1) | Phase 2 |
| 3.2 | Serialiser with the round-trip rules; identity and edit-locality property tests | 3.1 |
| 3.3 | Visual view: caret, selection, hit testing, IME; engine model service and editor modes | 3.1 |
| 3.4 | Inline editing: text, marks, links | 3.2, 3.3 |
| 3.5 | Block editing: paragraphs, headings, lists, admonitions, code blocks | 3.4 |
| 3.6 | Tables and images | 3.5 |
| 3.7 | Source ↔ visual live sync, split mode and shared undo | 3.4 |
| 3.8 | Toolbar, block picker, command palette | 3.5 |

## 10. Definition of Done for Phase 3

- A documentation page edited for an hour in visual mode produces a Git diff that touches only the lines the writer changed (automated as `a_long_editing_session_touches_only_edited_lines`, and checked by hand in `docs/qa/phase3.md`).
- Typing, marks, links, headings, lists, admonitions, code blocks, table cells and images can be edited visually, with undo shared with the source editor.
- Source and visual views of one document stay in step side by side.
- Nothing the model does not understand is rewritten: it is read-only in the visual editor.
- README, the documentation site and `docs/qa/phase3.md` describe visual editing and what stays source-only.

## 11. Implementation Notes

Deviations found while implementing are recorded here, as in Phase 2 §12.

| Area | Specification | Implementation | Why |
|---|---|---|---|
| Model types | `Node`, `TextBlock`, `Piece` as listed | `TextBlock` also carries its `display` text and a `context` (`Flow`, `Line`, `Cell`, `Verbatim`) that says what typed text may contain; `Piece::Atom` carries the text it `shown`s; marks include `Span` (text with a role, no formatting of its own); node kinds include `Title`, `Term` and `Description`; `text_block_at` returns the path, node and block rather than a `TextBlockRef` | The view draws from the display text and atoms' shown text directly; a cell must escape a vertical bar and a title stay on one line; a description list item has two text blocks. |
| Inline structure | the scanner's reading checked against the DOM | as specified; the check compares the characters shown (ignoring whitespace) and their marks, `Span` ignored on both sides, and link text without its inner formatting | Over this repository's documentation and the render fixtures, 1,188 of 1,189 text blocks agree; the one that does not is an acdc reading of an image macro inside backticks, and is read-only. |
| Block ranges | the DOM's spans | block starts are moved past whitespace and block ends to the end of their line; a section runs to the next block of its parent; text ranges are found from the source lines (heading markers, list markers, metadata lines) | acdc spans can be a byte short at either end on pages with multi-byte text, and a section's span ends early when its last child is included. |
| `kbd:`, `btn:`, `menu:` | atoms | atoms only when the document sets `experimental`, as Asciidoctor requires | Without it they are text, and the DOM check agrees. |
| Typed markup | escaped, then checked | the escape goes before the first construct the rescan finds that the step did not intend, which may be existing text (`*here` then a typed `*` escapes the first star); a line of flowing text that would start another block is guarded with `{empty}`; a vertical bar typed in a cell gets a backslash before it | Escaping only what forms markup keeps typed text as written, and Asciidoctor shows a backslash before anything else literally. |
| Mark changes | keep delimiter form | as specified; when a split mark or new text next to a mark makes the constrained form invalid, the marks involved take the unconstrained form; a new mark leaves the typed text's outer whitespace outside; adding a mark absorbs marks of the same kind it overlaps | `*bo*ld` would not parse; `*bold *` neither; `` `a`, `b` `` made code becomes `` `a, b` `` rather than nested backticks. |
| `VisualArgs` | `{ id, selection, marks, value }` | also `version`, the document version the selection is in | A selection from an older version may point at different text; a stale model is rebuilt first, but a stale selection is refused. |
| Step errors | `StepError` | reported as `EngineError::Refused` with the step's message; a join that is not possible (Backspace at the start of a paragraph after a heading) does nothing rather than report | A refused join is common and harmless; anything else is worth telling the user. |
| Deleting across blocks | `Delete` across blocks joins the first and last text blocks | refused ("deleting across blocks"); typing over or pressing Enter on a selection that spans blocks is refused too | The join's effect on everything between the blocks is easier to judge in the source; deferred. |
| Joining | Backspace joins with the previous paragraph | only the blank line between the paragraphs is removed, so the second paragraph's line is untouched and the joined text shows a space where the paragraphs met; a list item joins the item before by losing its marker; a paragraph does not join a heading | Removing only the separator keeps the Git diff to the lines the user changed. |
| Enter at the end of a block | `SplitBlock` | the new paragraph or list item exists only in the view until its first character, which `visual.insert_block` writes; Enter on a pending list item leaves the list; Enter at the start of a block does nothing | AsciiDoc has no empty paragraph or empty list item. |
| Split | — | the whitespace at the split point is dropped from both halves | Otherwise the left half keeps a trailing space, which breaks a constrained mark around it. |
| Tables | `InsertTableRow`, `InsertTableColumn` | rows go below the caret's (below the header, before the first body row, so an implicit header stays one) and columns after it; a table without `cols` takes its column count from its first row; widths written with `*` are left to the source editor | Inserting directly after an implicit header row would turn the header into a body row. |
| Highlight binding | `cmd-shift-h` | none | `cmd-shift-h` is Replace in Workspace. Highlight is on the toolbar and in the palette. |
| Palette | every command in the palette | the visual editor's commands run from the palette by closing it, which returns focus to the editor, and dispatching from there | The palette dispatches from its own search field, where only workspace actions are handled; found in the hour-long editing session. |
| Typing marks | — | with nothing selected, Bold, Italic, Code and Highlight toggle the marks the next typed text carries, held by the view; they reset when the caret moves | There is nothing in the source to change until text is typed. |
| Split mode | source and visual side by side | each follows the other's caret: the source scrolls to the visual caret, and the visual editor brings the source caret's block into view | Keeps both halves on the same text without moving the caret the user is not using. |
| Images | a dialog for target and alt text | clicking an editable block image opens it; the image is drawn from the file it resolves to | A click on read-only content (diagrams, included blocks) shows its source instead. |
| Bold in the release build on Linux | — | with the system's IBM Plex Sans installed as a variable font, GPUI draws its default weight, so bold text and headings show at regular weight in the preview and the visual editor alike | A font loading limit outside the editor; recorded in `docs/qa/phase3.md`. |
| List bindings | `cmd-shift-8`, `-7`, `-9` | `cmd-shift-u`, `cmd-shift-n`, `cmd-shift-x` | On Linux GPUI reports Shift with a digit as the shifted symbol with Shift dropped (Ctrl+Shift+8 arrives as Ctrl+`*`, and the symbol depends on the layout), so the digit bindings never matched. Found in the hour-long editing session. |
