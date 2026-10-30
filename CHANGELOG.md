# Changelog

All notable changes are recorded here. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

### Fixed

- Bold, italic, monospace and highlighted text broken over two lines of a paragraph is highlighted in the editor.

## 0.4.0 — 2026-10-27

### Added

- An application menu: a ☰ button at the left of the title bar opens into File, Edit, Format, View, Go, Git and Help menus, as in Zed (#116).
- Preview tabs: browsing to a page shows it in one reusable, italic preview tab until it is edited, saved, double-clicked or kept with Keep Open; `[editor] preview_tabs = false` keeps every page open as before (#117).
- A Files view in the sidebar shows the folders and files on disk beneath each workspace folder, whatever their type, read lazily a folder at a time and updated by the file watcher. Text files that are not AsciiDoc open as text, outside the index; PDFs open in the system's viewer; other binary files are refused, and open only through Open Externally. Copy Path, Copy Relative Path, Reveal in Finder / File Explorer / File Manager, Show Excluded Files and Reveal in Files (#118).
- An image viewer: images open in image tabs from the Files and Modules panels, Open File, the command line and image links, sharing the preview tab and the tab order with pages. PNG, JPEG (with EXIF orientation), SVG, BMP, ICO, GIF and WebP (first frame) and TIFF (first page) are shown, with Fit to Window, Actual Size, zoom from 5% to 3200%, panning, a checkerboard behind transparency, the format, size and file size, and Reload; HEIC, HEIF, AVIF and damaged files say why they cannot be shown and offer Open Externally. Images reload when they change on disk, are never opened as text, and are never written by Save or Save All. SVGs never run scripts or load what they link to (#125).
- A doctor report for support requests: `adocfoundry-cli doctor [<root>]` and Help › Show Diagnostics in the app print the version and build, the OS and architecture, the graphics backend and fonts (in the app), the PDF theme's fonts, each optional tool with its path and version or why it is missing, and the settings files with their warnings, ready to paste into an issue. The new bug report template asks for it (#105).
- The Navigation and Modules panels remember the branches you open and close, through re-indexing and, for each workspace, across restarts (`[sidebar] remember_expansion`, on by default). Right-clicking either panel offers Expand All, Collapse All, Collapse One Level and Reset to Configured Default (#121).

### Changed

- Navigation and Modules now open two levels deep instead of fully. `[sidebar.navigation]` and `[sidebar.modules]` `default_expansion` set the depth for each panel; `"all"` restores fully open trees (#121).

### Fixed

- In the source editor, inline highlighting (bold, italics, links, cross references) stopped partway down longer pages: each line of a paragraph was highlighted again on its own, using up the editor's limit on highlighted regions. A `[source]` paragraph is no longer highlighted as prose either (#130).
- Inline highlighting now reaches about three times as far down long pages: only text holding inline markup goes to the inline grammar, so plain prose no longer counts towards that limit (#131).
- Source highlighting now handles text after a block in a table cell, comments inside sections, links followed by `]`, and attributes or titles above a comment, from the latest AsciiDoc grammar (#129).

## 0.3.0 — 2026-10-25

### Added

- Inline STEM is typeset in the line of text in the preview and the visual editor: each formula sits on the text's baseline at the text's size, with its source in a tooltip (#57). Inline formula images also keep their descenders and limits, which the earlier tooltip images cut off.
- `adoc-mcp --http <addr>` serves MCP's Streamable HTTP transport at `/mcp` on a loopback address, to clients that send `Authorization: Bearer <token>` (`ADOC_MCP_TOKEN`, or a random token printed at start-up). Write tools still need `--allow-write`, and every path is still confined to the roots (#56).
- The app checks for a newer release at start-up, at most once a day, through the system's `curl`, and says once when there is one, with its release notes a click away. Check for Updates checks on demand, and `[updates] check = false` turns the start-up check off (#55).
- Updating in place: from a signed DMG or MSI, the update notice offers Update…, which downloads the package, checks it against the release's new `SHA256SUMS` and its signature against the running app's, installs it and restarts. The AppImage carries update information and its `.zsync` is published for AppImageUpdate (#55).
- Signed releases: with the project's signing secrets, the release workflow signs the macOS app and tools with a Developer ID and the hardened runtime, notarises and staples the app and the DMG, and signs the Windows executables and MSI with Authenticode through Azure Artifact Signing. The smoke test checks the signatures. Without the secrets, packages build unsigned as before (#54).

### Fixed

- An include whose `tag=` or `tags=` names an attribute (`include::{pagesdir}/schema.adoc[tag=method-{QUERY_REF}]`) included nothing: the attribute was not substituted in the tag, as Asciidoctor does. The preview, the visual editor and PDF export now include the tagged region.
- In the visual editor, an include whose target is written with an attribute (`include::{partialsdir}/overview.adoc[]`) showed its content and then its directive line again as raw text; consecutive includes of one file by different tags are now separate includes too.
- Commands chosen from the command palette that the workspace handles (Export Page to PDF…, Toggle Preview, Save and the rest) did nothing: the palette dispatched them from its own dialog, where the workspace never saw them. Every command now runs once the palette has closed, as the visual editor's already did.

## 0.2.1 — 2026-10-25

### Security

- The MCP server now confines what a page reads, not only the paths its tools are given. Before, rendering or exporting a page inside the roots read whatever its includes, images and PDF theme named outside them (such as `include::../outside.adoc[]`). External diagram tools are not run by the MCP server, since they read files of their own.

### Fixed

- Saving no longer writes over a change made to the file outside the editor. The engine refuses with a save conflict; the app asks whether to reload the file's version or overwrite it, and the CLI and MCP server return the conflict. Two commands, Reload from Disk and Save, Overwriting the File on Disk, make the choice explicitly (#88).
- Saves write a temporary file and rename it over the page, keeping its permissions and any symbolic link, so a failed write cannot leave a truncated file.
- A rename whose file move fails leaves nothing changed: the reference edits already made are undone, files already moved are moved back, and nothing is left in the undo history. Every move is also checked before anything changes, so a destination inside a file is refused at once (#89).
- Applying a Replace All or rename preview is refused when a file it edits has changed since the preview was made, instead of editing the new text at the old offsets. The confirmation shows the text the edits were computed against, the buffer for an unsaved page (#91).
- Renaming or moving a page rewrites its own references as well as those that name it, so its xrefs, includes and images keep naming what they named: relative to its new folder in a plain document, or qualified with the old module when an Antora page moves to another module, component or version. A reference from the page to itself now keeps pointing at it (#90).

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
