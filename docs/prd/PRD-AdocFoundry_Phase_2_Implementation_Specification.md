# PRD: AdocFoundry — Phase 2 Implementation Specification

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Date | 2026-10-23 |
| Status | Proposed |
| Scope | Phase 2: native rendered preview |
| Depends on | `docs/design/DESIGN-AdocFoundry_Architecture.md` (Phase 2 section), Phase 1 as merged |

## 1. Document Purpose

Phase 2 adds the full AsciiDoc document model and renders it natively beside the source. The preview is a first-class view: docked or torn off into its own window, following the active page, scrolling with the source, and clicking through to the exact source line, including inside included partials.

Phase 2 must provide:

- `adoc-dom`: a full, owned AST with source spans that map through includes, behind a `DocumentParser` trait, implemented with acdc-parser and, second, asciidoc-parser.
- `adoc-render`: a framework-free view tree built from the AST with references, images and diagrams resolved, and an HTML backend for export and fidelity diffing.
- `adoc-diagrams`: Mermaid rendered to SVG in-process, other diagram kinds through configured external tools, all cached by checksum.
- An engine preview service: debounced worker-thread parse, include resolution through the Antora catalog and open buffers, a `PreviewReady` event.
- The app's preview: docked pane, detachable window, follow-active-page with pin, scroll sync, click-to-source, link navigation, diagnostics overlay.
- `adocfoundry-cli render`, and a fidelity harness against Asciidoctor.

## 2. Decisions

| Question | Decision | Consequence |
|---|---|---|
| Primary parser | **acdc-parser 0.11** | Owns its tree, positions carry the include chain and original line/column, custom `IncludeSourceProvider`. |
| Second parser | **asciidoc-parser 0.31** behind the same trait (WP 2.8) | Run over the same corpus; differences are reported, not failures. |
| AST exposure | `adoc-dom` defines its **own** node types; parser crates never leak | Parser choice stays reversible; only `adoc-dom` names `acdc_parser` or `asciidoc_parser` (enforced by `cargo xtask layers`). |
| View tree | in `adoc-render` (L2), framework-free | The GPUI renderer, the HTML backend, the CLI and Phase 3 all share one tree. |
| HTML backend | AdocFoundry's own, from the view tree | No acdc HTML converter is published; the fidelity harness diffs it against Asciidoctor. |
| Mermaid | **merman 0.9.2**, `default-features = false`, `all-diagrams` + `complete-svg` | No ELK (EPL-2.0 is outside the licence allow-list); dagre/cytoscape layouts only. |
| STEM | rendered as styled math source in Phase 2 | True typesetting arrives with Typst in Phase 4; the block is still rendered, not raw. |
| Asciidoctor.js in QuickJS | **out of scope** | The fidelity harness uses an `asciidoctor` executable when one is on `PATH` (Ruby or Asciidoctor.js) and skips otherwise. |
| Safe mode | the engine parses with includes **enabled** through its own provider | Only workspace files the engine resolves are read; nothing from the network. |

## 3. Architecture

```text
L4  adocfoundry (preview/ module)   adocfoundry-cli (render)   adoc-ls
L3  adoc-engine   preview service: debounce → worker parse → view build → PreviewReady
L2  adoc-render (view tree, HTML)   adoc-diagrams (merman, providers, cache)
L1  adoc-dom (AST, DocumentParser; the only crate naming parser crates)   adoc-parser adoc-antora adoc-index adoc-lang
L0  adoc-core
```

`adoc-render` depends on `adoc-dom` and `adoc-core`; `adoc-diagrams` on `adoc-core` only. The engine depends on both. The app renders the view tree; it never sees the AST.

Data flow on an edit: the editor edits the rope (Phase 1) → after `PREVIEW_SETTLE` (100 ms) without further edits to the previewed document or any file it includes, the engine snapshots the composed files' texts and the include resolution map and parses on a worker thread → `pump` builds the view tree on the engine thread (resolution needs the catalog and index) → diagrams are taken from the cache or rendered on a worker, a placeholder standing in until then → `EngineEvent::PreviewReady { id, version }`.

## 4. `adoc-dom` (WP 2.1, 2.8)

```rust
pub struct SourceSpan {
    pub path: PathBuf,          // the file the content is in: the page, or the partial it came from
    pub range: SourceRange,     // byte range in that file
    pub include_chain: Vec<String>, // include targets as written, outermost first; empty for the page
}

pub struct Dom {
    pub title: Option<Vec<Inline>>,
    pub attributes: BTreeMap<String, String>,   // after header and caller attributes
    pub blocks: Vec<Block>,
    pub warnings: Vec<DomWarning>,              // parser warnings with spans
}

pub enum Block {                      // every variant carries `span: SourceSpan`, `id`, `roles`, `title`
    Section { level, title, blocks },
    Paragraph { inlines },
    List { kind: ListKind /* Unordered | Ordered | Checklist */, items: Vec<ListItem> },
    DescriptionList { items },
    CalloutList { items },
    Admonition { kind: AdmonitionKind, blocks },
    Listing { language: Option<String>, text: String, callouts },
    Literal { text },
    Quote { attribution, citation, blocks }, Verse { attribution, citation, text },
    Sidebar { blocks }, Example { blocks }, Open { blocks },
    Table { columns, header: Option<Row>, rows: Vec<Row>, footer: Option<Row>, frame, grid },
    Image { target, alt, width, height },
    Diagram { kind: String, source: String, attributes },   // [mermaid], [plantuml], [graphviz], …
    Stem { notation, source },
    Passthrough { content },
    ThematicBreak, PageBreak, Toc,
    Unsupported { name: String, source: String },          // never silently dropped
}

pub enum Inline {
    Text(String), Bold(Vec<Inline>), Italic(..), Monospace(..), Highlight(..),
    Superscript(..), Subscript(..), Link { target, text }, Xref { target, text },
    Anchor { id }, Image { target, alt }, Footnote { id, text }, LineBreak,
    Keyboard(Vec<String>), Button(String), Menu(Vec<String>), Stem(String), Passthrough(String),
}

pub trait IncludeResolver: Send + Sync {
    /// The file `target` names when written in `from`, and its text.
    fn resolve(&self, from: &Path, target: &str) -> Option<(PathBuf, String)>;
}

pub struct ParseInput<'a> {
    pub path: &'a Path, pub text: &'a str,
    pub attributes: Vec<(String, String)>,      // Antora, playbook and component attributes
    pub includes: &'a dyn IncludeResolver,
}

pub trait DocumentParser: Send + Sync {
    fn name(&self) -> &'static str;
    fn parse(&self, input: &ParseInput<'_>) -> Dom;   // never panics; half-typed input is normal
}
pub struct AcdcParser;          // WP 2.1
pub struct AsciidocParser;      // WP 2.8
```

Acceptance tests (normative): `parses_every_block_kind` (one fixture per `Block` variant), `spans_map_through_includes` (a block from a partial reports the partial's path and line), `antora_attributes_apply`, `half_typed_input_does_not_panic`, `unsupported_constructs_are_kept`; WP 2.8 adds `second_parser_matches_the_corpus` (block kinds and texts agree; differences listed, not failed).

## 5. `adoc-render` (WP 2.2)

```rust
pub struct ViewTree { pub title: Option<Vec<ViewInline>>, pub blocks: Vec<ViewBlock>, pub toc: Vec<TocEntry>, pub footnotes: Vec<ViewFootnote> }
// ViewBlock / ViewInline mirror the DOM, plus resolution:
//   section numbers and ids, TOC entries;
//   xrefs → ResolvedLink { target: Option<Location>, text, unresolved: bool };
//   links → external URL; images → resolved file path or a missing marker with the resource id;
//   diagrams → DiagramView::{Svg(PathBuf), Pending(source), Failed(message, source)};
//   footnote numbers; callout numbers.
pub trait ViewResolver { fn xref(&self, from: &Path, target: &str) -> ResolvedLink; fn image(&self, from: &Path, target: &str) -> Option<PathBuf>; fn diagram(&self, kind: &str, source: &str, attributes: &BTreeMap<String,String>) -> DiagramView; }
pub fn build_view(dom: &Dom, resolver: &dyn ViewResolver) -> ViewTree;
pub fn to_html(view: &ViewTree, options: &HtmlOptions) -> String;   // standalone page or fragment
pub fn block_at(view: &ViewTree, path: &Path, offset: usize) -> Option<BlockPath>;  // scroll sync
```

Rendering coverage, in order: sections and TOC; paragraphs and inline formatting (constrained and unconstrained, roles, replacements); lists (nested, description, callout, checklist); admonitions; listing and source blocks; tables (column widths, spans, header and footer, AsciiDoc cells); images (`imagesdir`, Antora `image$`); quotes, verses, sidebars, examples, open blocks; footnotes; STEM (as styled source); diagrams.

Acceptance tests: `builds_sections_toc_and_numbers`, `resolves_xrefs_and_marks_unresolved`, `resolves_antora_images`, `html_contains_every_block_kind`, `block_at_finds_the_innermost_block`.

## 6. `adoc-diagrams` (WP 2.6)

```rust
pub struct DiagramRequest { pub kind: String, pub source: String, pub attributes: BTreeMap<String,String> }
pub trait DiagramProvider: Send + Sync { fn kinds(&self) -> &[&str]; fn render(&self, request: &DiagramRequest) -> Result<String /* SVG */, DiagramError>; }
pub struct MermaidProvider;                       // merman, in-process
pub struct ExternalProvider { kind, program: PathBuf, args }   // dot, plantuml, mmdc, d2: found on PATH or set in settings
pub struct DiagramCache { dir: PathBuf }          // `<workspace>/.cache/adocfoundry/diagrams/<sha256>.svg`
pub struct Diagrams { providers: Vec<Box<dyn DiagramProvider>>, cache: DiagramCache }
impl Diagrams { pub fn cached(&self, req) -> Option<PathBuf>; pub fn render(&self, req) -> Result<PathBuf, DiagramError>; }
```

The cache key is a checksum of kind, source and the attributes that affect output. External tools are optional and discovered, never assumed; a missing tool is a `Failed` view with the reason. Spawning tests take a `spawn_guard()`.

Acceptance tests: `mermaid_renders_svg_in_process`, `cache_hits_by_checksum`, `missing_tool_fails_gracefully`, `diagram_attributes_change_the_key`.

## 7. Engine preview service (WP 2.3)

```rust
pub const PREVIEW_SETTLE: Duration = Duration::from_millis(100);
impl Engine {
    pub fn preview(&self, id: DocumentId) -> Option<Arc<RenderedPreview>>;   // view tree + version + source path
    pub fn request_preview(&mut self, id: DocumentId);                        // the front end shows a preview of `id`
    pub fn release_preview(&mut self, id: DocumentId);
}
pub struct RenderedPreview { pub document: DocumentId, pub version: u64, pub view: ViewTree }
EngineEvent::PreviewReady { id: DocumentId, version: u64 }
```

Only requested documents are previewed. A change to the document or to any file it includes (open or on disk) schedules a re-parse after `PREVIEW_SETTLE`. The worker gets an owned snapshot: the page text, the texts of every file it can include (from open buffers and the index), and the Antora attributes. A newer request supersedes an older one; a stale result is dropped. Diagrams render on a worker; when one finishes, the preview is rebuilt from the cached DOM and published again.

Command: `view.toggle_preview` (View, `cmd-k v`), `view.detach_preview` (View), `view.toggle_scroll_sync` (View), `preview.pin` (View).

Acceptance tests: `preview_updates_after_settle`, `preview_follows_included_file_changes`, `stale_preview_results_are_dropped`, `diagram_completion_republishes_preview`.

## 8. App preview (WP 2.3–2.5)

```text
crates/adocfoundry/src/preview/
├── mod.rs        PreviewView: renders a RenderedPreview, follows the active document unless pinned
├── blocks.rs     ViewBlock → GPUI elements (sections, lists, tables, admonitions, code, images, diagrams)
├── inlines.rs    ViewInline → styled text runs and clickable links
└── sync.rs       scroll sync and click-to-source
```

- **Docked**: a resizable pane right of the editor; toggled by `view.toggle_preview`.
- **Detachable**: `open_secondary_window(WindowKind::Preview)`; closing the window re-docks.
- **Follow and pin**: the preview shows the active document; pin freezes it on one page. Each page's preview scroll position is remembered.
- **Scroll sync**: moving the caret scrolls the preview to the block containing it; scrolling the preview moves the source to the topmost visible block; a toggle turns both off.
- **Click to source**: clicking a block places the caret at its span; for content from a partial, the partial opens in a tab at the right line.
- **Links**: xrefs navigate (through `navigate.follow_link` semantics, recorded in history); external links open the browser; images and attachments open natively.
- **Diagnostics overlay**: unresolved xrefs render as marked links; missing images as placeholders naming the resource id.
- **Code**: listing blocks with a language are highlighted with the same tree-sitter grammars the editor uses.

App tests (test platform): `preview_renders_the_active_document`, `clicking_a_block_moves_the_caret`, `clicking_partial_content_opens_the_partial`, `detached_preview_follows_edits`.

## 9. CLI and fidelity (WP 2.7)

```text
adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]
```

The fidelity harness renders every file in `tests/fixtures/render/` with `adocfoundry-cli render` and with `asciidoctor` (when on `PATH`), normalises both (block structure and text content; class names, whitespace and attribute order ignored) and reports the differences. A test asserts that every fixture's block structure matches; it is skipped, with a message, when no `asciidoctor` is found.

Acceptance tests: `render_writes_html`, `fidelity_block_structure_matches_asciidoctor`.

## 10. Implementation Sequence

| # | Package | Depends on |
|---|---|---|
| 2.1 | `adoc-dom`: types, `DocumentParser`, acdc implementation, render fixture corpus, span mapping through includes | Phase 1 |
| 2.2 | `adoc-render`: view tree, resolver, HTML backend, `block_at` | 2.1 |
| 2.6 | `adoc-diagrams`: merman, external providers, cache | 2.2 |
| 2.3 | Engine preview service; app docked preview with follow and pin | 2.2, 2.6 |
| 2.4 | Detachable preview window | 2.3 |
| 2.5 | Scroll sync, click-to-source, link navigation | 2.3 |
| 2.7 | `adocfoundry-cli render`; fidelity harness | 2.2 |
| 2.8 | asciidoc-parser as the second `DocumentParser` | 2.1 |

## 11. Definition of Done for Phase 2

- The Antora documentation of a real project renders with no block shown as raw source (STEM excepted, as styled source by decision).
- Mermaid diagrams appear without a browser, Node or Java.
- A page assembled from partials can be clicked through to the partial's source line.
- The preview follows the active page, can be detached and re-docked, and keeps scroll sync both ways.
- The fidelity harness passes for the render corpus where `asciidoctor` is available.
- README, the documentation site and `docs/qa/phase2.md` describe the preview and what is not yet implemented.

## 12. Implementation Notes

Deviations found while implementing are recorded here, as in Phase 1 §20.

| Area | Specification | Implementation | Why |
|---|---|---|---|
| acdc-parser licence | acdc-parser 0.11 (MIT OR Apache-2.0) | used, with its `evalexpr` dependency replaced by AdocFoundry's own `vendor/evalexpr` through `[patch.crates-io]` | acdc-parser depends on `evalexpr = "13"`, which is AGPL-3.0-only. acdc uses only `eval_float` and `eval_int` (for `ifeval` operands); the stand-in implements those two in-house under Apache-2.0. See `vendor/README.md`. |
| `IncludeResolver` | `resolve(from, target)` | `read(requested_path)`: the parser hands over the path it computed from the including file's directory and the target as written | acdc's include provider receives that path, not the pair. A resolver recognises Antora ids such as `partial$x.adoc` inside it. |
| DOM shape | a variant per block, each carrying span, id, roles and title | `Block { meta: BlockMeta, kind: BlockKind }`, plus `Heading` (discrete headings) and `Media` (audio and video) kinds | One place for the common fields; the extra kinds exist in AsciiDoc and must not fall back to raw source. |
| `build_view` | `build_view(dom, resolver)` | `build_view(source, dom, resolver)` | The DOM does not carry the page path, and local images and references resolve against it. |
| `ViewResolver::image` | `image(from, target)` | `image(from, target, imagesdir)` | `imagesdir` is a document attribute the resolver cannot see; outside Antora it decides where an image lives. |
| `block_at` result | `Option<BlockPath>` | `Option<Vec<usize>>` (child indices from the root), read back with `block_by_trail` | A plain index trail needs no new type; list items' and table cells' blocks count as children. |
| acdc spans | — | `Location::absolute_end` is the start of the last character; the converter extends spans past it, using the page or included file's text | Without it every span was one character short, and a cell's single-character paragraph had an empty span. |
| acdc image targets | — | block (`image::ROOT:image$x.svg[]`) and inline (`image:ROOT:x.svg[]`) images whose target contains `:` or `$` are recovered from the paragraph or text acdc leaves | acdc rejects those characters in an image target, so every Antora image id fell back to text. |
