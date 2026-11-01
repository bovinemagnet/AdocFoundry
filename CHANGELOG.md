# Changelog

All notable changes are recorded here. Versions follow [Semantic Versioning](https://semver.org/).

## 0.5.0 — 2026-11-01

### Added

- Files, Navigation and Modules follow the active page: whenever another page or image becomes active, each panel selects its row, opening the folders and branches above it and scrolling it into view, without taking focus. `[sidebar] follow_active = false` turns this off (#151).
- A live PDF preview: View › Toggle PDF Preview (or PDF in the preview's header) shows the preview target's pages as Export Page to PDF typesets them, in the same theme and fonts, typeset on a worker after the page settles (about 0.12 s from a keystroke on the reference article) and rasterised off the UI thread; a click on a line goes to its source (#62).
- PDF/A-1b and PDF/A-1a output: `--standard pdf-a-1b|pdf-a-1a`, `[pdf] standards` and the MCP server's `export_pdf`, validated by veraPDF in CI. PDF/A-1 forbids transparency, so a page with a transparent image or a Mermaid diagram fails with the line of the image; PDF/UA-2 waits on Typst, which writes only PDF/UA-1 (#63).
- The user guide lists every key in asciidoctor-pdf 2.3's Theme Keys Reference as supported, approximated (and how) or ignored by PDF export, generated from `crates/adoc-pdf/src/theme/compatibility.txt`; a test checks that every key is classified and that no key the template reads is marked ignored (#106).
- PDF export highlights source blocks with the editor's tree-sitter grammars and the preview's queries and light colours, so a listing looks as it does in the preview, when `source-highlighter` is set; Bash, Go, HTML, Java, JavaScript, JSON, Python, Rust, SQL, TOML, TypeScript and YAML, with other languages left to Typst's highlighter as before (#61).
- PDF publication profiles: `[pdf.profiles.<name>]` in the workspace settings saves what to export (a page, a component version, or navigation files in a chosen order), where, the theme, fonts, attributes (and so the metadata) and standards. File › Export PDF Profile… and `adocfoundry-cli pdf --profile <name>` run one and write the same PDF; File › Export Page to PDF with Theme… offers the bundled and workspace themes (#103).

- Spell checking of prose in the source and visual editors and in `adoc-ls`: misspelt words are underlined, with suggestions and Add to Workspace Dictionary or Add to User Dictionary in the context menu. Pages are checked in their `lang`, else `[spelling] language` (British English by default), with bundled British and American English dictionaries and any Hunspell dictionaries in `[spelling] dictionaries` (#65).

- Help › About AdocFoundry: the version, build and licence, links to the repository and the documentation, and every third-party component the app is built from with its version, licence and full licence text, ready to copy. The list is generated at build time from Cargo's metadata (every crate of the release build) and the bundled fonts, dictionaries and PDF themes, and each package ships it as `THIRD-PARTY-NOTICES.txt`; a dependency without a licence text fails the build and a test (#161).
- Quick Open (Cmd-P on macOS, Ctrl-P elsewhere; File menu, palette) is a fuzzy finder over the workspace's indexed files: each word typed matches the file name, title, Antora resource id or path, and files made active recently come first (#71).
- Go to Symbol in Workspace (Cmd/Ctrl-T) picks from the section titles, anchors and page titles of the whole workspace, and Go to Symbol in Page (Cmd/Ctrl-R) from the active page's, in page order (#70).
- Find References (Alt-Shift-F12, Go menu, palette) lists every reference to the anchor or page at the caret, across components, in a panel below the editor (#68).
- Rename Anchor (F2 on an anchor, a section heading or a reference to one; Edit menu, palette) renames an anchor, or a section's generated id, and rewrites every reference to it in every page and component as one undo step; F2 anywhere else still renames the page (#69).
- New Page… (Cmd/Ctrl-N, File menu, palette; in an Antora workspace) creates a page in a chosen module from its title and adds it to the module's navigation after the active page's entry (#72).
- Started with no folder or file, the app brings back the last session: the workspace's folders, the open tabs in order with each page's editor mode and caret, the active and preview tabs, and the preview's layout. File › Open Recent Folder… lists the folders opened recently (#75).
- Dragging a page from Navigation or Modules, or a section from the Outline, into the source or visual editor inserts an xref to it, with the shortest resource id that resolves from the page being edited and the page's or section's title as its text (#74).
- Crash recovery: two seconds after typing stops, and when the app quits, the text of every page with unsaved edits is kept; after a crash the next launch offers Recover Unsaved Changes, listing each page with the lines it would change (#107).
- Local history: every save keeps the version saved, and File › Local History… lists a page's versions, shows what has changed since one and restores it as one undo step. `[history]` sets how much is kept (30 days, 50 versions a page and 256 MB in all by default) (#107).
- Optional auto-save: `[editor] auto_save = "after_delay"` saves a page a second after its last edit, and `"on_focus_change"` when you leave its tab or the window. It is off by default, and never overwrites a file changed on disk (#76).
- The status bar shows the active page's word count and reading time, for example `120 words · 1 min read`, or the words in the selection (#77).
- File › Export Page to HTML… writes the page as one HTML file, as `adocfoundry-cli render` does, with its unsaved edits and with local images and diagrams embedded (#78).
- Format › Format Table lines up the `|` separators of the table at the caret as one undo step, and leaves a table it cannot format safely as it is, saying why (#84).
- Insert Table, Code Block, Admonition, Image and Link work in the source editor as well as the visual editor, each as one undo step (#83).
- Problems inside `antora.yml` are reported at their lines, as warnings: a missing `name` or `version`, a top-level key Antora does not use, a `nav` entry naming a file that does not exist, and an attribute value that is a list or mapping (#67).
- `[tools] dot`, `plantuml` and `d2` set the paths of Graphviz, PlantUML and D2, for an app started from the Dock, which does not see a shell's `PATH` (#64). `[tools] diagram_timeout` (30 seconds by default) stops a diagram tool that runs too long or writes more than 32 MB, and at most four diagrams render at once (#94).

### Changed

- Faster on long pages and large workspaces (#200–#209):
  - A keystroke no longer copies the page or parses it again, so typing in a long page costs about what it does in a short one, and only the views a change affects are redrawn (#201, #202).
  - The visual editor and the preview lay out only the blocks in view, so long pages open and scroll without waiting for the whole page (#203, #204).
  - Saves, recovery records and the app's state files are written off the UI thread, and Git's change markers are diffed against `HEAD`'s text read once, not on every change (#205, #207).
  - At start-up, folders are watched and indexed off the UI thread, and the last session comes back after the window is first drawn, building each page's visual editor only when its tab is shown (#206).
  - Quick Open keeps its list between keystrokes, Find in Workspace streams its results from a worker and stops at 10,000 matches, and indexing a large workspace needs about a quarter of the memory it did (#208).
  - The Problems panel is worked out by the engine, the whole workspace on a worker, instead of while the sidebar is drawn, and typing waits for a pause before updating the index (#181, #182).
- A change made to several files at once (a page rename or Replace All) is undone and redone only as a whole: undo is refused, naming the page, while a page it changed has been edited since (#95).
- Releases: a tag that disagrees with the version in `Cargo.toml`, `src/docs/antora.yml` or this changelog fails before anything is built, and a release is published only once CI passes on the tagged commit (#97, #193).

### Fixed

- PDF export reads `index_columns` for the number of index columns, as asciidoctor-pdf does, instead of `index_column_count`; a theme that set it no longer fails to typeset (#106).
- Bold, italic, monospace and highlighted text broken over two lines of a paragraph is highlighted in the editor.
- On Linux the app's windows show the AdocFoundry icon in the task bar and window switcher instead of the desktop's placeholder: every window now has the app id `io.github.bovinemagnet.AdocFoundry`, which the desktop entry's name and `StartupWMClass` match (#153).
- Antora resource ids with a version before a module or path are read as Antora reads them: `2.0@index.adoc` is version 2.0 of the current module, and `2.0@security:index.adoc` module `security` of it (#172).
- Insert Column and Delete Column handle `cols` given as a count, such as `[cols=3]` (#173).
- Anchor ids written with a role, an option or `id=` (`[#intro.lead]`, `[#intro%collapsible]`, `[id=intro]`) are indexed, so references to them resolve and Rename Anchor rewrites them in place (#174).
- Typing straight after a save or auto-save no longer reports the editor's own write as a change on disk (#180).
- A PDF theme that extends itself, directly or through another theme, is refused with the chain of files instead of crashing the app (#189).
- HTML and PDF export keep block attributes and options: list numbering styles, table options, image placement, collapsible blocks and vertical cell alignment (#186).
- When two indexes of the workspace finished out of order, the older could replace the newer; it is now dropped (#93).

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
