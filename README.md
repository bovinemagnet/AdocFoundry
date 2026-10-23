# AdocFoundry

Forge your documentation in real time.

AdocFoundry is a native desktop editor for AsciiDoc documentation laid out the Antora way, written in Rust for macOS, Linux and Windows. It is a thin GPUI application over a headless engine, and the same engine powers a command-line checker (`adocfoundry-cli`) and a language server (`adoc-ls`) for other editors.

The project is built in phases: (1) a source editor with Antora support and link following, (2) a native rendered preview, (3) WYSIWYG editing, (4) PDF export with themes. **Phases 1 to 3 are implemented.** The architecture is described in [`docs/design/DESIGN-AdocFoundry_Architecture.md`](docs/design/DESIGN-AdocFoundry_Architecture.md) and each phase's scope in [`docs/prd/PRD-AdocFoundry_Phase_1_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_1_Implementation_Specification.md), [`docs/prd/PRD-AdocFoundry_Phase_2_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_2_Implementation_Specification.md) and [`docs/prd/PRD-AdocFoundry_Phase_3_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_3_Implementation_Specification.md).

## What works

**Workspace and Antora**

- Opens a folder and discovers every `antora.yml` beneath it, so several components and versions live in one workspace. Plain AsciiDoc folders work too, with relative-path resolution.
- Resolves Antora resource IDs in all five coordinates (`version@component:module:family$path`) following Antora 3.2: a component without a version means its latest version (the versionless `~` version first, then released versions above `prerelease` ones); a component without a module means its `ROOT` module.
- Honours `:page-aliases:`: an xref to an alias resolves to the page and carries an information diagnostic suggesting the current id.
- Re-reads `antora.yml` when it changes, and re-indexes files changed on disk. A buffer with unsaved edits is never overwritten; you are told the file changed.

**Sidebar**

- *Navigation*: each component version's navigation as its `nav.adoc` files list it, or its pages by module when it declares none.
- *Modules*: every module with its families (pages, partials, examples, images, attachments) and their resources.
- *Problems*: workspace diagnostics grouped by file; click one to go to it.
- *Outline* of the active document; for a navigation file, its navigation tree.

**Editor**

- Syntax highlighting from the AsciiDoc tree-sitter grammars, with the inline grammar injected into paragraphs, titles, table cells and macro targets, and source blocks highlighted for Rust, Java, JavaScript, TypeScript, Python, Go, JSON, YAML, TOML, shell, SQL and HTML.
- Folding of sections, delimited blocks and lists; line numbers; soft wrap.
- Diagnostics as you type for unresolved xrefs, anchors, includes and images, duplicate anchors, unknown Antora modules and resources, invalid resource IDs and invalid component descriptors. Diagnostics are conservative: warnings rather than errors, and nothing for dynamic constructs such as `include::{attr}/x.adoc[]`.
- Hover and completion for xref, include and image targets, anchors and `{attributes}`.
- Undo and redo that treat a run of typing as one step.

**Link following**

- Cmd-click (Ctrl-click on Linux and Windows) or F12 on an `xref:`, `include::` or `{attribute}` opens the target and places the caret there; images and attachments open in the system's default application.
- Back and forward through the places you have followed links to.
- Alt-F12 on an `include::` shows the included file read-only.

**Preview**

- A native rendered preview beside the editor (Cmd/Ctrl-K V), drawn by the app itself: no browser, Asciidoctor, Node.js or Java. It re-renders 100 ms after you stop typing in the page or in any file it includes, unsaved edits included.
- It follows the active page; Pin keeps it on one page. Each page's scroll position is remembered.
- Detach moves it into its own window; closing that window docks it again.
- Scroll sync both ways: moving the caret scrolls the preview to that block, and scrolling the preview moves the source to the topmost visible block. Sync can be switched off.
- Clicking a block places the caret on its source, opening the partial it came from when there is one. Clicking an xref follows it, recorded in back and forward history; external links open the browser and attachments the default application.
- Unresolved xrefs are underlined in red, and missing images show a placeholder naming the target.
- Includes resolve as Antora resolves them, including qualified ids such as `include::other:partial$note.adoc[]`, and images and attachments are also found in a module's older `assets/images` and `assets/attachments` directories.
- Source blocks are highlighted with the editor's grammars. Mermaid diagrams render in process; Graphviz, PlantUML and D2 diagrams render when `dot`, `plantuml` or `d2` is on `PATH`, and otherwise show their source with the reason. Rendered diagrams are cached in `.cache/adocfoundry/diagrams` in the workspace.
- STEM blocks show their maths as styled source.

**Visual editing**

- Each tab shows its page as Source, Visual or both side by side (Split): buttons beside the tabs, or Cmd/Ctrl-K S, W and D. The visual editor shows the page rendered and edits it in place.
- The source stays the truth. Every visual edit becomes the smallest change to the source that makes it, so the Git diff touches only the lines you edited; text the editor does not understand is never rewritten.
- Typing, deleting, selecting with the mouse and keyboard, and IME composition. A character that would otherwise become markup (a `*` that would make text bold, `{name}`, a URL) is escaped where it is typed, and only then.
- Bold, Italic and Code (Cmd/Ctrl-B, -I, -E) and Highlight toggle on the selection, or with nothing selected on what you type next; marks keep the delimiters they were written with. Insert Link (Cmd/Ctrl-Shift-L) links the selection to a URL, page or anchor, or changes the link at the caret.
- Enter splits a paragraph, heading or list item, and at the end of one starts another; Backspace at a block's start joins or lifts it. A block-type picker turns paragraphs into headings, admonitions, code blocks and quotes; lists are made, changed and indented (Tab, Shift-Tab) from the toolbar or the keyboard, and checkboxes toggle on click.
- Table cells are edited in place; rows and columns are inserted and deleted from the Table menu (tables without spans). Images are inserted, and clicking one edits its target and alt text.
- Undo and redo are shared with the source editor: one history whichever editor made the change. In Split mode each side follows the other's caret.
- Included content, diagrams, STEM, passthroughs, attribute entries, conditionals and anything the editor and the parser read differently are shown read-only; clicking them shows their source. Block titles show above their blocks and are edited in the source.

**Git**

- The status bar shows the branch of the active document's repository (or `detached @ 1a2b3c4`). Repositories are found and read in-process with `gix`; nothing runs `git`. A folder outside Git behaves as before.
- The engine tracks the status of the repository's files and the changed lines of every open document against `HEAD`, including unsaved edits; a commit, a branch switch or staging is picked up from `.git` without a restart.
- Change markers beside the line numbers (green added, blue modified, a red triangle where lines were deleted) and beside changed blocks in the visual editor. Clicking a marker shows the lines as they were, with Revert Change (one undo step) and Copy. Alt-Down and Alt-Up go to the next and previous change.
- A Changes panel in the sidebar lists the changed pages and Antora resource files under the roots; clicking one opens it at its first change.
- Branch-aware Antora versions: the status bar and the Navigation panel show the page's version and branch (`product@2.0 · main`). The playbook's content sources and each matching branch's `antora.yml`, read from the repository without checking anything out, say which branch publishes which version; an xref to a version published from another branch warns (`adoc.antora.version-not-on-branch`). Checking out another branch in the same folder re-indexes without a restart, and Open in Another Version opens the same page in another version's checkout in the workspace.
- Toggle Blame shows who last changed each line beside the editor (`abc1234 Paul Snow, 3 days ago`, or `Not committed`), and with `blame_on_hover` resting on the gutter shows the commit's summary too. Open on Remote and Copy Remote Link give the caret's line on GitHub, GitLab, Bitbucket or Azure DevOps (self-hosted sites named under `[git.hosts]`).
- Staging and committing from the Changes panel: a box per file stages it, Stage All stages them all, and Commit… opens a message dialog (Commit, or Commit and Push). Save and Commit saves the page, stages it and starts the message as `docs: update <title>`. The status bar shows `↑n` for commits not yet pushed. Pushing runs your `git` (so credential helpers and SSH keys work), never forces and never sets an upstream; everything else goes through `gix`. No merge, rebase or history: use a Git client for those (epic [#2](https://github.com/bovinemagnet/AdocFoundry/issues/2)).

**Find, replace and rename**

- Find in the workspace, with regular expression, case and whole-word options.
- Replace across the workspace shows every change first; applying it is one undo step.
- Rename Page moves the file and rewrites every xref that leads to it, shown as a preview first; one undo moves the page back and restores every xref.

**Also**

- A command palette listing every command, light and dark themes, a read-only second window that follows the active document, and TOML settings.

## Installing

There are no published releases yet. The release workflow in `packaging/ci/` (not active until macOS and Windows have been tested by hand) builds a macOS universal DMG, Windows MSI and portable zip (x64 and ARM64) and Linux AppImage, deb and rpm (x86_64 and aarch64); there is also a Flatpak manifest in `packaging/flatpak/`. On Linux, `cargo xtask package`, `cargo deb -p adocfoundry` and `cargo generate-rpm -p crates/adocfoundry` build the same packages locally. Nothing else is needed at run time: no Ruby, Node.js or Java.

To build from source you need Rust (stable, via rustup) and, on Linux, the libraries GPUI uses:

```sh
# Debian and Ubuntu
sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev \
  libfontconfig-dev libxcb1-dev libx11-xcb-dev libssl-dev

git clone --recurse-submodules https://github.com/bovinemagnet/AdocFoundry.git
cd AdocFoundry
cargo run --release -p adocfoundry -- path/to/docs
```

If you cloned without `--recurse-submodules`, run `git submodule update --init` first: the grammars live in `grammars/tree-sitter-asciidoc`.

## Using it

`adocfoundry [folder [file…]]` opens a folder as the workspace and any files in tabs. Open Folder (Cmd/Ctrl-Shift-O) does the same from inside the app.

| Command | macOS | Linux and Windows |
|---|---|---|
| Command palette | Cmd-Shift-P | Ctrl-Shift-P |
| Open folder / file | Cmd-Shift-O / Cmd-O | Ctrl-Shift-O / Ctrl-O |
| Save / Save all / Close | Cmd-S / Cmd-Alt-S / Cmd-W | Ctrl-S / Ctrl-Alt-S / Ctrl-W |
| Undo / Redo | Cmd-Z / Cmd-Shift-Z | Ctrl-Z / Ctrl-Shift-Z |
| Follow link | Cmd-click, F12, Cmd-Enter | Ctrl-click, F12, Ctrl-Enter |
| Back / Forward | Ctrl-minus / Ctrl-Shift-minus | Ctrl-minus / Ctrl-Shift-minus |
| Peek include | Alt-F12 | Alt-F12 |
| Toggle preview | Cmd-K V | Ctrl-K V |
| Show source / visual / split | Cmd-K S / Cmd-K W / Cmd-K D | Ctrl-K S / Ctrl-K W / Ctrl-K D |
| Bold / Italic / Code (visual) | Cmd-B / Cmd-I / Cmd-E | Ctrl-B / Ctrl-I / Ctrl-E |
| Insert link (visual) | Cmd-Shift-L | Ctrl-Shift-L |
| Bulleted / numbered / checklist (visual) | Cmd-Shift-U / -N / -X | Ctrl-Shift-U / -N / -X |
| Indent / outdent list item (visual) | Tab / Shift-Tab | Tab / Shift-Tab |
| Find / Replace in workspace | Cmd-Shift-F / Cmd-Shift-H | Ctrl-Shift-F / Ctrl-Shift-H |
| Rename page | F2 | F2 |
| Next / previous change | Option-Down / Option-Up | Alt-Down / Alt-Up |
| Settings | Cmd-, | Ctrl-, |
| Toggle theme | Cmd-K Cmd-T | Ctrl-K Ctrl-T |
| Read-only second window | Cmd-K Cmd-N | Ctrl-K Ctrl-N |

Pin Preview, Detach Preview and Toggle Preview Scroll Sync are buttons on the preview's toolbar and commands in the palette. The visual editor's toolbar holds the block-type picker, the marks, Link, the lists, an Insert menu (image, table, code block, admonition) and, in a table cell, the Table menu; every one of them is also in the palette.

## Settings

Settings are TOML. The user file is `adocfoundry/adocfoundry.toml` in the platform configuration directory (`~/.config` on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on Windows); Open Settings creates it. A workspace may override `editor.*` and `workspace.ignore` in `.adocfoundry.toml` at its root. Saving either file applies it. Unknown keys produce a warning, never an error.

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
git = ""                         # the git that pushes use; discovered on PATH if empty

[git]
enabled = true                   # false: look for no repository
show_markers = true              # change markers in both editors
blame_on_hover = false           # line blame when resting on the gutter
remote = "origin"                # the remote that links to the hosting site use

[git.hosts]                      # self-hosted sites: github | gitlab | bitbucket | azure
# "git.example.com" = "gitlab"
```

`tools.asciidoctor` is used only by `adoc-ls`'s HTML preview; the app's preview needs no external tool. Diagram tools are found on `PATH` and cannot be set here yet.

## Command line

```text
adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
adocfoundry-cli symbols <root> <query>
adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]
adocfoundry-cli git-status <root> [--format text|json]
```

`check` prints the workspace's diagnostics (text: `path:line:col: severity code message`) and exits with 1 when any meets `--fail-on` (default `error`), which suits CI. It reports exactly what the editor and `adoc-ls` report.

`render` writes a page as a standalone HTML file, as the preview renders it, waiting for its diagrams; without `--output` it writes to standard output. Without `--root`, the workspace is the folder above the page's Antora component, or the page's own folder outside Antora. `--parser asciidoc-parser` renders with the second document parser, for comparison; the app always uses `acdc`.

`git-status` prints the branch of the repository holding the root and its changed AsciiDoc files as `git status --short` does, with the number of changed line runs against `HEAD` in each (`branch main`, ` M docs/page.adoc (2 changed)`); `--format json` gives the same as an object. The other commands do not look at Git.

## Language server

`adoc-ls --stdio` serves the same diagnostics, definition, hover, completion, document and workspace symbols, and an HTML preview through Asciidoctor, to any LSP editor. The Zed extension [zed-asciidoc](https://github.com/bovinemagnet/zed-asciidoc) starts it from `PATH`:

```sh
cargo install --git https://github.com/bovinemagnet/AdocFoundry adoc-ls
```

## Not implemented yet

- PDF export (Phase 4).
- In the visual editor: selections and deletions across blocks, Enter at the start of a block, and editing included content in place (Edit partial opens the partial).
- Typeset STEM: the preview shows the maths source until Phase 4.
- Settings for diagram tool paths; `dot`, `plantuml` and `d2` are found on `PATH` only.
- Editing included content in place, spell checking, plugins.
- Merging, rebasing, conflicts and history: use a Git client.
- XML and Dockerfile highlighting inside source blocks: the editor component has no grammar for them yet.
- Diagnostics inside `antora.yml` itself, beyond reporting that it cannot be read.
- Verified IME composition and frame timing on every platform, the packages on macOS and Windows, and the preview checked by hand on each platform: see [`docs/qa/phase1.md`](docs/qa/phase1.md), [`docs/qa/phase2.md`](docs/qa/phase2.md), [`docs/qa/phase3.md`](docs/qa/phase3.md) and [`docs/qa/spike-gpui.md`](docs/qa/spike-gpui.md).

## Developing

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo xtask layers      # the crate dependency ladder
cargo xtask package     # release build and archive for this platform in dist/
```

The workspace is a ladder of crates, each depending only on those below it:

```text
adocfoundry (GPUI app)   adocfoundry-cli   adoc-ls (LSP)
adoc-engine              documents, commands, undo, workspace, watcher, settings, search, preview, visual model
adoc-render   adoc-diagrams   adoc-model   adoc-git   view tree and HTML; diagrams; the editable model; Git
adoc-parser   adoc-dom   adoc-antora   adoc-index   adoc-lang
adoc-core
```

`adoc-dom` holds the full document tree behind the `DocumentParser` trait and is the only crate that names a parser crate (acdc-parser, and asciidoc-parser for comparison). `adoc-model` is the visual editor's model: it reads each text block of a page losslessly (every source byte is text, a mark delimiter or part of an atom), checks its reading against the parser's, and turns edits into the smallest source edits; its property tests (`cargo test -p adoc-model --test roundtrip`) check that building and writing back is the identity on every fixture and documentation page and that a long random editing session changes only the lines it edited. The fidelity harness (`cargo test -p adocfoundry-cli --test fidelity`) compares `render` with Asciidoctor over `tests/fixtures/render/`, and skips itself when `asciidoctor --version` fails.

## Licence

Apache-2.0. See [LICENSE](LICENSE).
