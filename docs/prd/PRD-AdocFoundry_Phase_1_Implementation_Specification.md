# PRD: AdocFoundry — Phase 1 Implementation Specification

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.1.0 |
| Date | 2026-10-23 |
| Status | Proposed |
| Scope | Phase 1: AsciiDoc source editor with Antora support and link following |
| Depends on | `docs/design/DESIGN-AdocFoundry_Architecture.md` (the architecture and technology design) |

## 1. Document Purpose

This document is the implementation plan for Phase 1 of **AdocFoundry**, a native Rust desktop editor for AsciiDoc documentation structured for Antora, targeting macOS, Linux and Windows.

It is written for implementation by an AI coding agent (Claude Code). It favours concrete repository layout, crate responsibilities, trait signatures, acceptance tests, implementation order and explicit scope boundaries over product language. Where the architecture design doc explains *why*, this document says *what* and *in which order*.

Phase 1 delivers a usable editor, not a preview. The objective is an application shell and engine whose boundaries will carry Phases 2 to 4 without a rewrite, built on the language crates that already exist in `bovinemagnet/zed-asciidoc`.

Phase 1 must provide:

- A GPUI desktop application that opens a folder, discovers Antora components in it, and edits `.adoc` files.
- Syntax highlighting, folding and outline from the tree-sitter grammars.
- A sidebar with the Antora navigation tree, module/family browser and diagnostics list.
- In-process language features: go-to-definition, hover, completion, diagnostics, workspace symbols.
- Link following for `xref:`, `include::` and `image::` targets with a navigation stack.
- Cross-component and cross-version Antora resolution and `page-aliases`.
- Find and replace across a component; rename of a page that updates xrefs.
- A headless CLI (`adocfoundry-cli check`) and a language server (`adoc-ls`) built from the same engine and language crates.
- Packaging and CI for all three operating systems.

---

## 2. Decisions and Assumptions

Two questions were open in the design doc. This specification proceeds on the recommended answers. Change them here, not in code, if they are wrong.

| Question | Decision | Consequence |
|---|---|---|
| Where do the five `adoc-*` crates live? | **Move them into this repository.** `zed-asciidoc` becomes a thin Zed extension that depends on `adoc-ls` published from AdocFoundry. | Work package 1.2 performs the migration; history is preserved with `git subtree` or a filtered import. |
| Licence | **Apache-2.0**, matching `zed-asciidoc` and `tree-sitter-asciidoc`. | `cargo deny` permits MIT, Apache-2.0, BSD-2/3, MPL-2.0, Unlicense, Zlib, OFL-1.1 (fonts). GPL and LGPL are denied. |

Further assumptions:

- The GUI framework is **GPUI** consumed through `gpui-pre` and `gpui-component` (Longbridge snapshot crates, Apache-2.0), gated by the spike in §12. If the spike fails, the UI crate is rewritten on **egui/eframe** and nothing below `adocfoundry` changes. The spec therefore describes the UI crate's responsibilities and its boundary with the engine, not GPUI API calls.
- Zed's `editor`, `text`, `rope`, `multi_buffer`, `language` and `project` crates are **GPL-3.0** and must never be dependencies.
- Rust toolchain: stable, pinned in `rust-toolchain.toml` to the version `gpui-component` 0.8 builds with (verified in the spike; expected 1.95 or later). Edition 2021 to match the migrated crates; migrate to 2024 as a separate task after Phase 1.

---

## 3. Product

| | |
|---|---|
| Product name | AdocFoundry |
| Repository | `https://github.com/bovinemagnet/AdocFoundry` |
| App binary | `adocfoundry` |
| CLI binary | `adocfoundry-cli` |
| Language server binary | `adoc-ls` (unchanged name; Zed, VS Code and Neovim users keep working) |
| Settings file | `adocfoundry.toml` in the platform config directory, plus `.adocfoundry.toml` at a workspace root |

---

## 4. Architectural Principles

1. **One engine, three front ends.** `adoc-engine` owns all state and all mutations. The GPUI app, the CLI and the LSP are consumers. If a feature can be tested through the engine without a window, it is implemented in the engine.
2. **Layered crates, enforced.** L0 `adoc-core` → L1 language crates → L2 document services → L3 engine → L4 front ends. `cargo xtask layers` fails CI on an upward dependency.
3. **Commands, not method calls, for user actions.** Every user-visible action is a `Command` with a stable `CommandId`, registered once, invoked by UI, CLI and future MCP alike. Undo is recorded per command.
4. **Byte offsets everywhere.** `SourceRange` is the only position type below the front ends. Lines and columns exist only at the LSP boundary and in the editor view.
5. **The source file is the truth.** Phase 1 edits plain text; nothing in the engine stores semantics that cannot be re-derived from the files on disk plus the open buffers.
6. **Conservative diagnostics.** No false positives; warnings not errors for unresolved references; silence for dynamic constructs.
7. **No runtime dependency on Ruby, Node or Java.** `asciidoctor` remains an optional external renderer behind the existing `Renderer` trait for Phase 2's fidelity harness; Phase 1 never calls it from the app.
8. **tree-sitter is for syntax only.** Highlighting, folding, outline hints, injections. Semantics come from `adoc-parser`, `adoc-antora`, `adoc-index` and `adoc-lang`.

---

## 5. Scope

### 5.1 In scope

- Open folder; discover every `antora.yml` beneath it (ignoring `node_modules`, `.git`, `build`, `target`, `.cache`); multiple components and versions in one workspace.
- Plain (non-Antora) `.adoc` folders work too, with relative-path resolution only.
- Editor tabs with the AsciiDoc grammar; soft wrap; line numbers; folding of sections and delimited blocks; bracket matching; source-block injection highlighting for Rust, Java, JavaScript/TypeScript, Python, Go, JSON, YAML, TOML, Shell, SQL, XML/HTML, Dockerfile.
- Outline panel: sections; for `nav.adoc` the navigation tree.
- Sidebar: Antora navigation tree per component version (from `nav:` in `antora.yml`), falling back to the `pages/` directory tree; module and family browser; diagnostics list for the workspace with click-to-location.
- Language features in-process: definition, hover, completion, diagnostics, document symbols, workspace symbols; attribute resolution from `antora.yml` (`asciidoc.attributes`), the playbook if present, and the page.
- Link following: Ctrl/Cmd-click or `F12` on an xref, include, image, attachment or attribute reference; opens the target in a tab (images and attachments open in the OS default app); back/forward navigation stack; peek (inline, read-only) for partials.
- Cross-component and cross-version resolution; `page-aliases`; descriptor re-read on change.
- Workspace find and replace (literal and regex, case and whole-word options) with a results panel and per-match replace.
- Rename page: moves the file and rewrites every `xref:` that resolves to it, with a preview of the edit list and a single undo step.
- Settings: TOML with user and workspace layers; light and dark themes; font family and size; soft wrap; tab width; external tool paths.
- `adocfoundry-cli check <root>`: prints diagnostics for a workspace, exit code 1 on any error-severity diagnostic.
- `adoc-ls`: unchanged behaviour, now built from `adoc-lang`.
- Packaging: macOS universal DMG (signed and notarised when secrets exist), Windows MSI x64 and ARM64, Linux AppImage, Flatpak manifest, deb and rpm for x86_64 and aarch64.

### 5.2 Out of scope for Phase 1

- Any rendered preview, HTML output in the app, WYSIWYG, PDF.
- Editing inside included content as if inline.
- Git integration, collaboration, cloud, telemetry.
- Extension or plugin API.
- Spell checking (candidate for Phase 2 via `harper-asciidoc`).
- Web or mobile builds.

---

## 6. Repository Layout

```text
AdocFoundry/
├── Cargo.toml                      workspace; default-members = ["crates/adocfoundry"]
├── Cargo.lock
├── rust-toolchain.toml
├── deny.toml                       cargo-deny: licence allow-list, advisory policy
├── CLAUDE.md
├── AGENTS.md
├── README.md
├── LICENSE                         Apache-2.0
├── .github/workflows/ci.yml        fmt, clippy, test, deny, layers, build matrix
├── .github/workflows/release.yml   tag → packages for all platforms
├── docs/
│   ├── design/DESIGN-AdocFoundry_Architecture.md
│   └── prd/PRD-AdocFoundry_Phase_1_Implementation_Specification.md
├── grammars/
│   └── tree-sitter-asciidoc/       git submodule, bovinemagnet/tree-sitter-asciidoc, branch zed
├── crates/
│   ├── adoc-core/                  L0 (migrated)
│   ├── adoc-parser/                L1 (migrated)
│   ├── adoc-antora/                L1 (migrated, extended in WP 1.7)
│   ├── adoc-index/                 L1 (migrated)
│   ├── adoc-lang/                  L1 (new: extracted from adoc-ls handlers)
│   ├── adoc-render/                L2 (migrated; unused by the app in Phase 1)
│   ├── adoc-engine/                L3 (new)
│   ├── adoc-ls/                    L4 (migrated, now over adoc-lang)
│   ├── adocfoundry-cli/            L4 (new)
│   └── adocfoundry/                L4 (new; the only crate importing GPUI)
│       └── languages/asciidoc/     highlights.scm, outline.scm, injections.scm, folds.scm, brackets.scm
├── xtask/                          cargo xtask layers | fixtures | package
└── tests/fixtures/
    ├── simple/ includes/ xrefs/ images/ attributes/      (migrated)
    ├── antora-single-component/                           (migrated)
    ├── antora-nested-pages/ antora-nested-includes/       (migrated)
    ├── antora-invalid-descriptor/                         (migrated)
    ├── antora-multi-component/                            (new, WP 1.7)
    └── antora-versions/                                   (new, WP 1.7)
```

Workspace `Cargo.toml` essentials:

```toml
[workspace]
members = ["crates/*", "xtask"]
default-members = ["crates/adocfoundry"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"
repository = "https://github.com/bovinemagnet/AdocFoundry"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.dependencies]
# every entry pinned exactly; crates reference these, never their own versions
```

---

## 7. Migration from zed-asciidoc (WP 1.2)

Import `crates/adoc-core`, `adoc-parser`, `adoc-antora`, `adoc-index`, `adoc-render`, `adoc-ls` and `tests/fixtures/` from `zed-asciidoc` at its current `main` into this repository, preserving history (`git subtree add --prefix=... <zed-asciidoc> main` or `git filter-repo` on a clone, then merge). Do not rewrite code during the import; later packages change it.

Then, in `zed-asciidoc`:

- delete the six crates and the fixtures;
- make `extension.toml` and `src/lib.rs` the whole repository;
- document in its README that `adoc-ls` now ships from AdocFoundry (`cargo install adoc-ls` once published, or `cargo install --git https://github.com/bovinemagnet/AdocFoundry adoc-ls`).

Acceptance: `cargo test --workspace` in AdocFoundry runs the same tests that passed in `zed-asciidoc` with the same names; `zed-asciidoc` builds for `wasm32-wasip2` with no workspace members.

---

## 8. `adoc-lang` — Editor-Agnostic Language Features (WP 1.5a)

### 8.1 Why

`adoc-ls/src/handlers/` already contains editor-agnostic functions operating on byte offsets (`definition_at_offset`, `completion_at_offset`, `hover_at_offset`, `diagnostics`, `document_symbols`, `nav_symbols`, `antora_attributes`, `composed_files`, `resolve_reference`, `resolve_include`, `resolve_image`). The engine must call them in-process without depending on `lsp-server`, `lsp-types` or `url`. Extracting them is a move, not a rewrite.

### 8.2 Contents

Move to `crates/adoc-lang/src/`:

```text
attributes.rs        attribute_definitions, component_attributes, antora_attributes
completion.rs        completion_at_offset
definition.rs        definition_at_offset, reference_target_path, resolve_reference, resolve_include
diagnostics.rs       diagnostics, descriptor_diagnostics
hover.rs             hover_at_offset
images.rs            antora_image_id, resolve_image
includes.rs          composed_files
symbols.rs           document_symbols, nav_symbols (was document_symbols.rs)
render_source.rs     source_for_render
workspace.rs         WorkspaceState (the index + catalog + descriptor-issue parts of adoc-ls ServerState, without open-document bookkeeping)
lib.rs
```

`code_actions.rs` and `execute_command.rs` stay in `adoc-ls`; they are LSP-shaped.

### 8.3 Public surface

```rust
pub struct WorkspaceState {
    pub index: adoc_index::WorkspaceIndex,
    pub antora: adoc_antora::AntoraCatalog,
    pub descriptor_issues: Vec<adoc_antora::DescriptorError>,
}

impl WorkspaceState {
    pub fn new() -> Self;
    pub fn index_roots(&mut self, roots: &[PathBuf]) -> io::Result<usize>;
    pub fn replace_document(&mut self, path: PathBuf, document: adoc_core::Document);
    pub fn remove_document(&mut self, path: &Path);
    pub fn reload_descriptor(&mut self, descriptor_path: &Path) -> Result<(), DescriptorError>;
}

pub struct Location { pub path: PathBuf, pub range: SourceRange }

pub enum DefinitionTarget {
    Location(Location),
    External(PathBuf),            // image, attachment: open with the OS
}

pub fn definition_at_offset(ws: &WorkspaceState, path: &Path, text: &str, offset: usize) -> Option<DefinitionTarget>;
pub fn hover_at_offset(ws: &WorkspaceState, path: &Path, text: &str, offset: usize) -> Option<Hover>;
pub fn completion_at_offset(ws: &WorkspaceState, path: &Path, text: &str, offset: usize) -> Vec<CompletionItem>;
pub fn diagnostics(ws: &WorkspaceState, path: &Path) -> Vec<adoc_core::Diagnostic>;
pub fn workspace_diagnostics(ws: &WorkspaceState) -> Vec<(PathBuf, Vec<adoc_core::Diagnostic>)>;
pub fn document_symbols(document: &adoc_core::Document) -> Vec<DocumentSymbol>;
pub fn workspace_symbols(ws: &WorkspaceState, query: &str) -> Vec<WorkspaceSymbol>;
pub fn references_to(ws: &WorkspaceState, target: &Path) -> Vec<Location>;   // new: used by rename
```

`Hover`, `CompletionItem`, `DocumentSymbol`, `WorkspaceSymbol` are `adoc-lang` types with plain strings and `SourceRange`s; `adoc-ls/src/protocol.rs` maps them to `lsp-types`.

Acceptance: `adoc-ls` compiles against `adoc-lang` with its stdio integration test unchanged; no file in `adoc-lang` imports `lsp_types`, `lsp_server` or `url` (asserted by `cargo xtask layers`).

---

## 9. `adoc-antora` Extensions (WP 1.7)

The model already carries component and version. Implement:

### 9.1 Cross-component and cross-version resolution

`AntoraResolver::resolve` honours all five coordinates of `AntoraResourceId` per Antora 3.2:

- Version omitted, component omitted → current component version (from `AntoraContext`).
- Component given, version omitted → the **latest** version of that component loaded in the catalog. Latest = highest by Antora's version ordering (semantic-ish: `~`/versionless sorts above numbered; numbered compared as dotted integers; non-numeric compared lexically) unless `antora.yml` marks `prerelease: true`, which sorts below released.
- Component given, version given → exactly that.
- Component and version omitted, module given → same component version, that module.
- `ResolutionError::UnknownVersion { component, version }` is a new variant.

### 9.2 `page-aliases`

`AntoraResource` gains `aliases: Vec<AntoraResourceId>` read from the `:page-aliases:` attribute (comma-separated resource IDs, each resolved relative to the page's own coordinates). `AntoraCatalog::resolve` falls back to aliases after the primary key. Diagnostics distinguish `adoc.antora.resolved-via-alias` (info) so writers can update old links.

### 9.3 Descriptor reload

`AntoraCatalog::reload_component(descriptor_path) -> Result<ReloadReport, DescriptorError>` re-reads one `antora.yml`, replaces that component version's modules and resources, and returns which resource IDs appeared or disappeared so the engine can re-run diagnostics for affected files only.

### 9.4 Navigation tree

```rust
pub struct NavTree { pub component: String, pub version: Option<String>, pub items: Vec<NavItem> }
pub struct NavItem { pub title: String, pub target: Option<AntoraResourceId>, pub source: Location, pub children: Vec<NavItem> }

pub fn nav_tree(catalog: &AntoraCatalog, component: &str, version: Option<&str>, read: &dyn Fn(&Path) -> io::Result<String>) -> Result<Vec<NavTree>, NavError>;
```

Built from the `nav:` list in `antora.yml`; each `nav.adoc` is parsed with `adoc-parser` into a nested list of xrefs. Unresolvable targets are kept with `target: None` and a diagnostic `adoc.antora.nav-unresolved`.

Fixtures: `tests/fixtures/antora-multi-component/` (two components, cross-component xrefs, an alias) and `tests/fixtures/antora-versions/` (one component at `1.0`, `2.0` and `~`, an xref with and without a version, a `prerelease: true` version).

Acceptance tests (names are normative): `resolves_cross_component`, `resolves_latest_version_when_omitted`, `prerelease_sorts_below_released`, `resolves_via_page_alias`, `reload_descriptor_reports_added_and_removed`, `nav_tree_nests_by_list_depth`, `nav_tree_marks_unresolved`.

---

## 10. `adoc-engine` (WP 1.2, 1.3, 1.8)

### 10.1 Responsibilities

- Workspace roots, `WorkspaceState` (from `adoc-lang`), file watcher.
- Open documents: text buffer, dirty flag, version counter, parsed `Document`, per-document diagnostics.
- Command registry and invocation.
- Undo history per document.
- Settings (user + workspace layers).
- Events to front ends.
- Navigation history (back/forward) — it is engine state so the CLI tests can exercise link following.

The engine is single-threaded by design: one `Engine` owned by the front end, mutated only through `&mut self` on the UI thread. Long work (initial indexing, workspace search) runs on a `std::thread` worker that sends results back through `EngineEvent`s; the front end calls `Engine::pump()` to apply them. This keeps the engine testable without an async runtime and matches both GPUI (`cx.spawn` + channel) and egui (frame loop) front ends.

### 10.2 Core types

```rust
pub struct Engine { /* private */ }

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(u64);

pub struct OpenDocument {
    pub id: DocumentId,
    pub path: PathBuf,           // may not exist on disk for untitled
    pub text: ropey::Rope,       // byte-indexed (ropey 2.0 beta); front ends mirror, never own
    pub version: u64,
    pub dirty: bool,
    pub parsed: adoc_core::Document,
    pub diagnostics: Vec<adoc_core::Diagnostic>,
}

pub struct TextEdit { pub range: SourceRange, pub replacement: String }   // byte offsets into the pre-edit text

pub enum EngineEvent {
    WorkspaceIndexed { roots: Vec<PathBuf>, files: usize },
    DocumentOpened(DocumentId),
    DocumentChanged { id: DocumentId, version: u64, edits: Vec<TextEdit> },
    DocumentSaved(DocumentId),
    DocumentClosed(DocumentId),
    DiagnosticsUpdated { path: PathBuf },
    CatalogChanged { descriptor: PathBuf },
    NavigationChanged,            // back/forward availability or active document changed
    SearchResults { request: SearchId, results: Vec<SearchMatch>, done: bool },
    CommandFailed { command: CommandId, error: String },
    SettingsChanged,
}
```

### 10.3 Engine API

```rust
impl Engine {
    pub fn new(settings: Settings) -> Self;
    pub fn open_workspace(&mut self, roots: Vec<PathBuf>) -> Result<(), EngineError>;   // starts indexing on a worker
    pub fn workspace(&self) -> &adoc_lang::WorkspaceState;

    pub fn open(&mut self, path: &Path) -> Result<DocumentId, EngineError>;
    pub fn open_untitled(&mut self) -> DocumentId;
    pub fn document(&self, id: DocumentId) -> Option<&OpenDocument>;
    pub fn documents(&self) -> impl Iterator<Item = &OpenDocument>;
    pub fn close(&mut self, id: DocumentId) -> Result<(), EngineError>;
    pub fn save(&mut self, id: DocumentId) -> Result<(), EngineError>;

    pub fn active(&self) -> Option<DocumentId>;
    pub fn set_active(&mut self, id: DocumentId);

    pub fn invoke(&mut self, command: CommandId, args: CommandArgs) -> Result<CommandOutcome, EngineError>;
    pub fn commands(&self) -> &CommandRegistry;

    pub fn undo(&mut self, id: DocumentId) -> bool;
    pub fn redo(&mut self, id: DocumentId) -> bool;

    pub fn subscribe(&mut self) -> Receiver<EngineEvent>;     // std::sync::mpsc; one per front end
    pub fn pump(&mut self) -> usize;                             // apply worker results; returns events applied

    pub fn settings(&self) -> &Settings;
    pub fn reload_settings(&mut self) -> Result<(), EngineError>;
}
```

Text mutation is only reachable through commands; there is no public `apply_edits`. The command implementations use a crate-private `Engine::apply_edits(id, Vec<TextEdit>, undo_group: UndoGroup)` that updates the rope, bumps the version, re-parses with `adoc-parser`, replaces the index entry, recomputes diagnostics for the document and its includers (`WorkspaceIndex::includers`), and emits `DocumentChanged` and `DiagnosticsUpdated`.

Re-parse runs synchronously on every edit in Phase 1: `adoc-parser` is linear and fast enough (the spike measures a 5,000-line file; budget 5 ms). Debouncing is added in Phase 2 when `adoc-dom` arrives.

### 10.4 Commands

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CommandId(&'static str);          // e.g. "document.save", "navigate.follow_link"

pub struct CommandSpec {
    pub id: CommandId,
    pub title: &'static str,               // shown in the palette; English, title case
    pub category: CommandCategory,         // Document, Edit, Navigate, Workspace, View, Help
    pub default_binding: Option<&'static str>,   // "cmd-s" style, platform-mapped by the front end
    pub when: CommandContext,              // Always | ActiveDocument | Selection | Antora
}

pub enum CommandArgs { None, Document(DocumentId), Offset { id: DocumentId, offset: usize }, Range { id: DocumentId, range: SourceRange }, Text { id: DocumentId, range: SourceRange, text: String }, Path(PathBuf), Search(SearchRequest), Rename { id: DocumentId, new_path: PathBuf }, Custom(serde_json::Value) }

pub enum CommandOutcome { Done, Opened(DocumentId), Location(adoc_lang::Location), External(PathBuf), Preview(EditPreview), Cancelled }

pub trait Command: Send + Sync {
    fn spec(&self) -> &CommandSpec;
    fn run(&self, engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError>;
}

pub struct CommandRegistry { /* private */ }
impl CommandRegistry {
    pub fn register(&mut self, command: Box<dyn Command>) -> Result<(), RegistryError>;   // duplicate id is an error
    pub fn get(&self, id: CommandId) -> Option<&dyn Command>;
    pub fn all(&self) -> impl Iterator<Item = &CommandSpec>;
}
```

Phase 1 command set (ids are normative; the UI binds them):

| Id | Category | Args | Outcome |
|---|---|---|---|
| `workspace.open_folder` | Workspace | `Path` | `Done` |
| `workspace.reindex` | Workspace | `None` | `Done` |
| `document.open` | Document | `Path` | `Opened` |
| `document.save` / `document.save_all` / `document.close` | Document | `Document` / `None` / `Document` | `Done` |
| `edit.insert_text` | Edit | `Text` | `Done` (undoable) |
| `edit.replace_range` | Edit | `Text` | `Done` (undoable) |
| `edit.undo` / `edit.redo` | Edit | `Document` | `Done` |
| `navigate.definition` | Navigate | `Offset` | `Location` or `External` |
| `navigate.follow_link` | Navigate | `Offset` | `Opened`, `External`, or `Location` with the document opened and activated |
| `navigate.back` / `navigate.forward` | Navigate | `None` | `Opened` |
| `navigate.peek_include` | Navigate | `Offset` | `Location` (read-only view decided by the UI) |
| `workspace.search` | Workspace | `Search` | `Done`; results arrive as `SearchResults` events |
| `workspace.replace` | Workspace | `Custom { request, replacement, matches }` | `Preview` then `Done` after `workspace.apply_preview` |
| `workspace.apply_preview` | Workspace | `Custom { preview_id }` | `Done` (one undo group across files) |
| `page.rename` | Workspace | `Rename` | `Preview` listing the move and every xref edit; `workspace.apply_preview` applies |
| `settings.open` / `settings.reload` | View | `None` | `External` / `Done` |

Undo: `UndoStack` per document of `UndoEntry { group: UndoGroup, inverse: Vec<TextEdit>, forward: Vec<TextEdit>, version_before, version_after }`. Consecutive `edit.insert_text` of single characters within 500 ms coalesce into one group. Multi-file previews (`workspace.replace`, `page.rename`) create one `UndoGroup` id shared across documents; `edit.undo` on any of them undoes the whole group and reports which documents changed.

### 10.5 File watcher

`notify` 6.x with a 250 ms debounce. Events: `.adoc`/`.asciidoc`/`.ad` created/modified/removed → re-index that file (if not open and dirty; a dirty open document is never overwritten; the UI shows a "changed on disk" banner); `antora.yml` modified → `reload_component`; directory create/remove under a module → re-index the root. All watcher work goes through `pump()`.

### 10.6 Settings

```toml
[editor]
font_family = "JetBrains Mono"   # falls back to the platform monospace
font_size = 13
soft_wrap = true
tab_width = 2
theme = "auto"                   # auto | light | dark

[workspace]
ignore = ["node_modules", ".git", "build", "target", ".cache"]

[tools]
asciidoctor = ""                 # optional absolute path; discovered on PATH if empty
```

User file: `dirs::config_dir()/adocfoundry/adocfoundry.toml`. Workspace file: `<root>/.adocfoundry.toml`, which may override `editor.*` and `workspace.ignore` only. Unknown keys warn (logged, shown once in the UI), never fail.

Acceptance tests (engine, headless): `open_workspace_indexes_fixture`, `open_edit_save_round_trip`, `edit_reparses_and_updates_diagnostics`, `undo_redo_single_group`, `insert_text_coalesces_within_window`, `follow_link_opens_target_and_pushes_history`, `back_forward_navigation`, `watcher_reindexes_changed_file` (uses a temp dir and a real `notify` watcher), `descriptor_change_reloads_component`, `rename_page_rewrites_xrefs_as_one_undo_group`, `settings_workspace_overrides_user`, `unknown_setting_key_warns_not_fails`.

---

## 11. `adocfoundry` — the GPUI Application (WP 1.1, 1.4, 1.6, 1.8)

### 11.1 Structure

```text
crates/adocfoundry/src/
├── main.rs             gpui::App::new().run; creates Engine; opens the main window
├── app.rs              AppState: Engine, active window ids, theme; the pump timer
├── workspace_view.rs   window layout: title bar, sidebar dock, editor tabs, status bar
├── editor/
│   ├── mod.rs          SourceEditorView: wraps gpui-component's editor; mirrors the engine rope
│   ├── bindings.rs     keymap: CommandSpec.default_binding → gpui actions
│   ├── language.rs     tree-sitter grammar registration, query loading
│   └── links.rs        link detection under cursor, hover underline, click handling
├── sidebar/
│   ├── nav_tree.rs     Antora navigation tree + pages fallback
│   ├── families.rs     module/family browser
│   └── diagnostics.rs  workspace diagnostics list
├── outline_view.rs
├── search_view.rs      find/replace panel with results and preview
├── peek_view.rs        read-only inline view for navigate.peek_include
├── palette.rs          command palette listing CommandRegistry::all()
├── settings_view.rs    opens the TOML in the editor; reload on save
└── theme.rs            light/dark tokens mapped to gpui-component theme
languages/asciidoc/     highlights.scm outline.scm injections.scm folds.scm brackets.scm
```

### 11.2 Boundary with the engine

- Views hold `Entity<AppState>`; `AppState` owns the `Engine`. All mutations are `engine.invoke(CommandId, args)`; the result and subsequent `EngineEvent`s drive re-render. Views never hold a second copy of document semantics (sections, references, diagnostics); they read them from `engine.document(id)` at render time.
- The editor widget owns a display buffer for performance; on every widget edit it calls `edit.insert_text`/`edit.replace_range` with byte ranges, and on `DocumentChanged` events from other sources (undo, replace-all, rename) it reconciles its buffer from the engine rope. The engine rope is the truth; a mismatch is a bug and a debug assertion.
- Line/column conversion for the caret uses `adoc_core::LineIndex` from the parsed document, never a second line index.
- The front end calls `engine.pump()` on a 16 ms timer when there are pending events, otherwise on demand.

### 11.3 tree-sitter integration

Build the two grammars from the submodule with the `cc` crate in `build.rs` (as the upstream `tree-sitter-asciidoc` crate does) and register them with gpui-component's language registry. Queries: start from the `zed-asciidoc` `languages/` queries; add `folds.scm` (sections by level, delimited blocks, lists). Injections: inline grammar into paragraphs, titles, table cells, macro targets; source languages by the `[source,<lang>]` positional attribute.

Known grammar limits (flat lists, line-by-line paragraphs, unscoped conditionals) are accepted for highlighting; do not try to fix them in queries.

### 11.4 Link following

`links.rs` asks `adoc_lang::definition_at_offset` for the token under the pointer on hover (debounced 100 ms) and underlines the token's `SourceRange` when a target exists. Click with the platform modifier, or `F12`, invokes `navigate.follow_link`. `Opened` activates the tab; `Location` scrolls to the range and places the caret; `External` opens via `open` crate. Unresolvable targets show the diagnostic message in a tooltip, no navigation.

### 11.5 Windows

Phase 1 needs one window, but the spike must prove `open_window` with a second view sharing `AppState`, because Phase 2's detachable preview depends on it. Keep window creation in `app.rs` behind `open_secondary_window(kind)` so Phase 2 adds a `WindowKind::Preview` without touching the main window code.

### 11.6 Accessibility and IME

Every interactive element gets a role and label through gpui's AccessKit integration; the editor exposes text content and selection. IME composition must work in the editor on all three platforms (tested manually with Japanese input on macOS and Windows, ibus on Linux). These are spike exit criteria, not later polish.

Acceptance (manual checklist in `docs/qa/phase1.md`, plus automated where possible): opens fixture workspace and shows the nav tree; highlighting matches Zed's for the same file; Cmd/Ctrl-click on each link type navigates; back/forward; find/replace across the component with preview; rename page with preview; undo across the rename; dark and light themes; window resize and HiDPI on each OS.

---

## 12. GPUI Spike (WP 1.1) — go/no-go

Two weeks, before any other package. Deliverable: `crates/adocfoundry` compiling on macOS (Apple Silicon and Intel), Windows x64 and Linux x86_64 (Wayland and X11), from one pinned `gpui-pre`/`gpui-component` snapshot, with:

1. gpui-component's editor showing `tests/fixtures/antora-nested-pages/modules/ROOT/pages/index.adoc` plus a synthetic 5,000-line `.adoc` with the AsciiDoc grammars and injections; scrolling and typing at 60 fps (measured with the frame counter; no frame over 16 ms during a 10-second typing burst).
2. `adoc_lang::definition_at_offset` wired in-process to a click handler (a temporary stub of `navigate.follow_link`).
3. A second window opened with `open_window`, showing a read-only copy of the same text, updating as the first is edited.
4. IME composition in the editor on all three platforms.
5. The snapshot upgraded once during the spike (to the next weekly `gpui-pre`) with the changes required recorded in `docs/qa/spike-gpui.md`.

Pass: all five on all platforms. Fail on any: switch the UI crate to egui/eframe 0.37 (`egui_extras` + `egui_code_editor` or a custom text widget over the engine rope), keep every other crate as specified, and record the decision in the design doc.

---

## 13. `adocfoundry-cli` (WP 1.2, 1.9)

```text
adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
adocfoundry-cli symbols <root> <query>
adocfoundry-cli follow <file> <byte-offset>      # prints the resolved target; used by tests and scripting
adocfoundry-cli --version
```

Built on `adoc-engine` only. `check` opens the workspace, waits for `WorkspaceIndexed`, prints `workspace_diagnostics` sorted by path and offset (text: `path:line:col: severity code message`; json: an array of `{path, range: {start, end}, line, column, severity, code, message}`), exit code 1 if any diagnostic meets `--fail-on` (default `error`).

Acceptance: `crates/adocfoundry-cli/tests/cli.rs` runs `check` on every Antora fixture and snapshots the JSON output (`insta` crate); `follow` on `tests/fixtures/xrefs/index.adoc` returns `other.adoc`.

---

## 14. `adoc-ls` after migration (WP 1.5a)

Behaviour unchanged. Internals: `protocol.rs` and `state.rs` call `adoc-lang`; `state.rs` keeps only open-document bookkeeping and position-encoding negotiation. `preview.rs`, `code_actions.rs`, `execute_command.rs` stay. The stdio integration test must pass without modification.

---

## 15. CI, Packaging and Release (WP 1.9)

`ci.yml`:

- `lint`: ubuntu; fmt, clippy `-D warnings`, `cargo deny check`, `cargo xtask layers`.
- `test`: matrix ubuntu-latest, macos-14, windows-latest; `cargo test --workspace --exclude adocfoundry`; then `cargo build -p adocfoundry` (no display needed to build).
- `grammar`: builds the submodule grammars and runs their corpus tests so a grammar bump that breaks highlighting fails here, not in the app.

`release.yml` on tag `v*`:

- macOS: universal binary (`lipo`), `.app` bundle, DMG; sign and notarise when `APPLE_*` secrets exist, else unsigned with a README note.
- Windows: MSI via `cargo-wix` for x64 and ARM64; portable zip.
- Linux: AppImage via `cargo-appimage` or `linuxdeploy`; `cargo-deb`; `cargo-generate-rpm`; Flatpak manifest in `packaging/flatpak/`; x86_64 and aarch64 (cross via `cross`).
- `adoc-ls` and `adocfoundry-cli` binaries attached for every target.

`cargo xtask layers` reads `Cargo.toml` of every crate, assigns layers from a table in `xtask/src/layers.rs`, and fails if any dependency points to a higher or equal-layer crate except `adoc-ls`→`adoc-lang`-style same-layer exceptions listed explicitly. It also fails if `gpui`/`gpui-pre`/`gpui-component`/`egui` appears outside `crates/adocfoundry`, or `lsp-types`/`lsp-server` outside `crates/adoc-ls`.

---

## 16. Implementation Sequence

Each package is one or more PRs; each leaves `cargo test --workspace` green. Do not start a package before its dependencies are merged.

| # | Package | Depends on | Spec | Normative tests |
|---|---|---|---|---|
| 1.1 | GPUI spike | — | §12 | spike checklist in `docs/qa/spike-gpui.md` |
| 1.2 | Workspace bring-up: migrate crates and fixtures; `adoc-engine` skeleton (`Engine`, `OpenDocument`, `apply_edits`, `CommandRegistry`, `UndoStack`, `EngineEvent`); `adocfoundry-cli --version`; CI `lint` + `test`; `xtask layers` | 1.1 pass | §6, §7, §10.2–10.4, §15 | `open_edit_save_round_trip`, `undo_redo_single_group`, `insert_text_coalesces_within_window`, layers check passing |
| 1.3 | Workspace, catalog, index in the engine; worker indexing; file watcher; descriptor reload; settings | 1.2 | §10.1, §10.5, §10.6, §9.3 | `open_workspace_indexes_fixture`, `watcher_reindexes_changed_file`, `descriptor_change_reloads_component`, `settings_workspace_overrides_user`, `unknown_setting_key_warns_not_fails` |
| 1.5a | Extract `adoc-lang`; rebuild `adoc-ls` on it | 1.2 | §8, §14 | `adoc-ls` stdio test unchanged; layers check forbids LSP types in `adoc-lang` |
| 1.5b | Language commands in the engine (`navigate.definition`, hover and completion providers exposed to the UI, diagnostics per document and workspace) | 1.3, 1.5a | §10.4 | `edit_reparses_and_updates_diagnostics` |
| 1.4 | App shell: workspace window, editor tabs over the engine, sidebar nav tree, families, diagnostics list, outline, palette, themes | 1.1, 1.3 | §11.1–11.3 | manual checklist §11 |
| 1.6 | Link following and navigation stack in engine and UI; peek | 1.5b, 1.4 | §10.4, §11.4 | `follow_link_opens_target_and_pushes_history`, `back_forward_navigation`, CLI `follow` |
| 1.7 | `adoc-antora` extensions: cross-component, versions, aliases, nav tree, new fixtures | 1.5a | §9 | the seven tests in §9 |
| 1.8 | Workspace search and replace with preview; page rename | 1.6 | §10.4 | `rename_page_rewrites_xrefs_as_one_undo_group`, search tests with the `insta` snapshot of results on `antora-multi-component` |
| 1.9 | CLI `check`/`symbols`/`follow`; release workflow; packaging for all targets; `docs/qa/phase1.md` | 1.3, 1.6 | §13, §15 | `cli.rs` snapshots; a release dry run producing artefacts for every target |

Order of merge: 1.1 → 1.2 → 1.5a → 1.3 → 1.5b → 1.7 → 1.4 → 1.6 → 1.8 → 1.9. (1.5a before 1.3 so the engine is built on `adoc-lang` from the start; 1.7 before 1.4 so the sidebar has the nav tree API.)

---

## 17. Definition of Done for Phase 1

- A user opens the documentation repository of a real Antora project (reference: Antora's own `docs/` or any public component) and the nav tree matches the published site's navigation.
- Every xref, include and image in that component's nav pages resolves or carries a diagnostic that `antora` itself would also report; no false positives on a clean component.
- `adocfoundry-cli check` reports the same diagnostics as `adoc-ls` for the same files (asserted by a test that runs both over the fixtures).
- Rename of a page rewrites all xrefs; one undo restores everything.
- Packages install and launch on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ (AppImage) with no external runtime.
- `README.md` documents installation, settings, and the feature list with what is not yet implemented.
- The architecture doc's open questions in §2 are marked decided.

---

## 18. Critical Design Decisions to Preserve

1. The engine is the only owner of document text and semantics; views mirror, never own.
2. All mutations are commands with stable ids and undo groups; no exceptions for "small" edits.
3. `adoc-lang` is where language behaviour lives; `adoc-ls` and the engine are both thin over it.
4. Byte offsets below the front ends; `LineIndex` at the edge.
5. Framework imports confined to `crates/adocfoundry`; the egui fallback must remain a one-crate swap.
6. Zed GPL crates are never dependencies.
7. Diagnostics conservative; parsers panic-free on partial input.
8. tree-sitter for syntax, never semantics.
9. Exact dependency pins and `cargo deny` on every PR.
10. Phase 2 hooks exist but are empty: `open_secondary_window`, the `Renderer` trait, the engine's event stream. Do not fill them in Phase 1.

---

## 19. Working Rules for Claude Code

- Follow `CLAUDE.md`. Read §10 and §11 before touching the engine or UI; read §9 before touching `adoc-antora`.
- Implement signatures as written. If one cannot work, keep the name and shape, change the minimum, and explain in the PR.
- One work package per PR, titled `WP 1.x: <package name>`, with the normative tests present and passing.
- Never modify the migrated crates in WP 1.2 beyond what compilation in the new workspace requires.
- Never add a dependency from the GPL list; never add a dependency without an exact pin and a `cargo deny` pass.
- When the spike or a package reveals that the spec is wrong, update the spec in the same PR and say so in the description.
