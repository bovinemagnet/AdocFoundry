# Changelog

All notable changes are recorded here. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

### Security

- The MCP server now confines what a page reads, not only the paths its tools are given. Before, rendering or exporting a page inside the roots read whatever its includes, images and PDF theme named outside them (such as `include::../outside.adoc[]`). External diagram tools are not run by the MCP server, since they read files of their own.

### Fixed

- Saving no longer writes over a change made to the file outside the editor. The engine refuses with a save conflict; the app asks whether to reload the file's version or overwrite it, and the CLI and MCP server return the conflict. Two commands, Reload from Disk and Save, Overwriting the File on Disk, make the choice explicitly (#88).
- Saves write a temporary file and rename it over the page, keeping its permissions and any symbolic link, so a failed write cannot leave a truncated file.
- A rename whose file move fails leaves nothing changed: the reference edits already made are undone, files already moved are moved back, and nothing is left in the undo history. Every move is also checked before anything changes, so a destination inside a file is refused at once (#89).
- Applying a Replace All or rename preview is refused when a file it edits has changed since the preview was made, instead of editing the new text at the old offsets. The confirmation shows the text the edits were computed against, the buffer for an unsaved page (#91).

## 0.2.0 — 2026-10-25

- Bold, Italic, Code and Highlight work in the source editor too, with the visual editor's keys (Cmd/Ctrl-B, -I, -E): they wrap the selection in the mark's delimiters, or remove them when the selection is already marked.
- Tabs have a close button and Zed's tab menu (Close, Close Others, Close Left, Close Right, Close Clean, Close All), each also a command with Zed's key, and the tab bar has back and forward arrows.
- A new app icon, in the macOS bundle, the Linux packages and AppImage, and on Windows in the executable, the Start menu and Programs and Features.
- The AppImage holds `adocfoundry-cli`, `adoc-ls` and `adoc-mcp` beside the app, as the other packages do.

## 0.1.0 — 2026-10-24

The first release: the four phases of the editor, and the release-readiness work of Phase 5.

### Editor (Phase 1)

- A source editor for AsciiDoc laid out the Antora way: components, versions, modules and navigation in the sidebar, with tree-sitter highlighting, folding and outline.
- Link following for xrefs, includes, images, attachments and attributes, across components and versions, with hover and completion.
- Diagnostics for unresolved references, unknown modules and components, duplicate anchors and invalid descriptors, in a Problems panel.
- Workspace search and replace, and page rename that rewrites the xrefs, nav entries and includes that name the page, each one undo step.

### Preview (Phase 2)

- A native rendered preview, docked or in its own window, pinned or following the active page, with scroll sync and click-to-source.
- Diagrams rendered in process (Mermaid) or through optional tools on `PATH` (Graphviz, PlantUML, D2), cached by checksum.

### Visual editing (Phase 3)

- WYSIWYG editing of paragraphs, headings, lists, tables and inline formatting, writing the smallest source edits, with read-only blocks for constructs it cannot edit safely.
### Git

- Changed-line markers in both editors, a Changes panel with revert hunk, branch-aware Antora versions, blame on hover, open on the remote, and stage, commit and push (#2).

### PDF export (Phase 4)

- Pages as articles and component versions as books, typeset in process by Typst, styled by asciidoctor-pdf themes loaded as asciidoctor-pdf loads them.
- PDF/A-2a and PDF/UA-1 on request.

### Phase 5

- STEM typeset from AsciiMath and LaTeX in the PDF, the preview (block formulas as images, inline formulas in a tooltip) and the HTML backend, with the source as each formula's alternative text; a formula that cannot be converted shows as its source with a warning naming its line.
- `adoc-ls` runs on the engine: the same diagnostics as the app, references, file and anchor renames, `adoc.checkWorkspace` and `adoc.exportPdf`. Existing clients keep working: the binary, `--stdio` and the preview command ids are unchanged. The default build has neither Typst nor Git; `--features typst,git` adds them.
- `adoc-mcp`: an MCP server over stdio for assistants and tools, read-only unless started with `--allow-write`.
- Fixes: the status bar no longer shows Indexing… on every keystroke (#19); the palette lists only commands that apply (#17); the active tab stays in view (#13); the status bar follows the visual caret (#15); long diagnostics show in full on hover (#18); word movement and double-click selection in the visual editor (#14); code blocks highlighted in the visual editor (#16); description lists tagged as lists in PDF/UA (#31); tag directives dropped from tagged includes and callouts (#33); pages with `subs=` on an include or a tab after a section title's `=` no longer show as source (#32); footnotes placed as asciidoctor-pdf places them (#35); a 1,833-page book exports in about half the memory and a quarter of the time (#34).
- Found before the release: an image that cannot be read is shown as its alt text with a warning, as a missing one is, instead of failing the export; special sections (preface, glossary, bibliography and the like) are not numbered and appendices are lettered (`Appendix A:`), in the preview, HTML and PDF; a PDF viewer's page labels match the printed page numbers, with the front matter in roman numerals; a single page's references to other pages link to their full address on the site, which can come from the playbook's `site.url`; `normalize_path` keeps leading `..` components; and spans in files with CRLF line endings, as Windows writes them, cover the right text.
- Diagrams and STEM render locally, in Rust or through tools on `PATH`; `cargo xtask layers` checks that no crate but the app's can reach an HTTP client (#3).
- CI on Linux, macOS and Windows, with the asciidoctor-pdf comparison and veraPDF validation (#27).
- Packages for macOS (DMG, signed ad hoc), Windows (MSI and zip, unsigned) and Linux (AppImage, deb, rpm), each with the app and the three command-line tools, smoke-tested on each platform. Developer ID signing and notarisation follow in a later release.

### Known issues

- The AppImage holds only the app; take `adocfoundry-cli`, `adoc-ls` and `adoc-mcp` from their own archives or the deb or rpm. Fixed in the release workflow for the next release.
