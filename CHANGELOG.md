# Changelog

All notable changes are recorded here. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

### Added

- Impact analysis between Git revisions (#308, Impact 2/5): `Engine::analyse_impact` compares two refs (`ChangeSource::GitRefs`), `HEAD` against the working tree and the open pages' unsaved buffers (`WorkingTree`), or paths named with no Git (`Paths`, partial with `no-base-revision` for a removed one). A revision's dependency snapshot is built from Git object data alone, never a checkout, a fetch or the live index: `adoc-git` gains `resolve_revision`, `tree_files`, `text_at` and `tree_changes` (renames detected), and `adoc-antora` gains `discover_antora_files`, discovery from a revision's file list by the same rules as disk discovery. Snapshots are cached by tree id; a comparison of two commits of a 2,000-page component takes about 0.9 s, 50 ms once cached.
- The documentation impact analysis's model (#307, #309: Impact 1/5 and 3/5): a new pure crate, `adoc-impact` (reached through `adoc_engine::impact`), which works out from the changes and two revisions' index and catalog (`DependencySnapshot`) which pages may render differently (through nested includes, cycles bounded), which references need review (removed or moved pages from the base side, with `page-aliases` resolving them; removed anchors), how each component version's navigation changed, and which publication profiles select affected content, each impact with its evidence side, dependency path and source span. A page's prose change never reaches the pages that merely link to it. The report is deterministic, with explicit completeness (`no-base-revision`, `asset-consumers-not-analysed`, `not-in-catalog`) and truncation. Git revisions and the command line follow.
- MCP `create_page` and `create_partial` (#319, MCP 6/7), write tools that invoke the engine's `page.new` and `page.new_partial` as the app's New Page and New Partial do: a page named from its title (or `file_name`) in a component version's module, with an optional entry at the end of the module's navigation file and an optional `expected_nav_hash`, one undo step, saved by default; a partial with optional text, answering with the includes that name it. Ambiguous versions, collisions, bad names and paths outside the roots are refused. `create_module` waits for #290 and `create_project` for #291–#295. Every tool now pumps the engine before its work, so it reads changes on disk and the link graph as they are.
- MCP staged changes (#318, MCP 5/7): `preview_changes` takes up to 64 operations (`replace` by unique text or byte range, `create_page` as New Page with an optional navigation entry, `create_partial`, `rename` as Rename Page, each with an optional `expected_hash`) and answers with every changed file's diff, SHA-256 hashes before and after and the diagnostics it would have, worked out on a copy of the workspace, writing nothing; it is offered without `--allow-write`. `apply_changes` (write tools only) rechecks paths, hashes and absent files and makes the change as one undo group, or nothing (`stale_preview`, `conflict`, `outside_workspace`); a failed save is an `internal_error` whose `details` list what was saved and what stays in the buffers. Previews are per session (16, 4 MiB, 15 minutes), under unguessable ids; idempotency keys return the first answer. Write tools called without `--allow-write` are still refused, whatever a page says. The engine gains `ChangeSet`, `stage_edits`, `stage_new_page`, `stage_new_partial`, `stage_rename`, `check_staged`, `apply_staged`, `current_text` and `text_hash`; `new_page` now plans through the same `plan_new_page`, and its navigation entry is placed from the navigation file's current text whether or not it is open.
- The MCP `quality_audit` tool (#316, MCP 3/7): the documentation quality audit as `adocfoundry-cli audit --format json` gives it, under the workspace's own `.adocfoundry.toml` policy (never the user's file), for the workspace or a component version, with `fail_on` and `require_complete` overrides. Findings are paged and filterable by rule or diagnostic code and severity, each with its region in lines and columns; every page carries the whole report's completeness, counts and coverage, and adds the `verdict`, the command line's `exit_code` and an `outcome` that is `pass` only for a complete audit (`pass_partial` otherwise). Exclusions are listed on request. No provider runs and nothing is fetched.
- MCP discovery tools (#315, MCP 2/7): `list_pages` pages through Antora resources in resource-id order by root, component, version (`~` for the versionless one, never mixed with numbered versions), module and family, with each page's title and whether its own version's navigation lists it; `get_outline` gives a page's title, sections (with the ids references reach them by) and anchors, or a navigation file's entries, with byte ranges; `get_page_context` gives a page's identity, navigation places, includes and what they resolve to, incoming reference counts and declared attributes, marking effective values unknown until #99; `find_references` lists the exact xrefs, navigation entries, includes or anchor references reaching a page, with ranges, lines and columns, paged. The engine gains `Engine::resources_after` (`ResourceQuery`, `ResourceEntry`: a listing reads only what it returns), `outline_of`, `section_ids`, `in_navigation` and `incoming_references` (`IncomingReference`, `ReferenceKind`), the catalog `AntoraCatalog::resources_from`, and `adoc_lang::references::references_to_anchor`.
- MCP result contracts (#314, MCP 1/7): every `adoc-mcp` tool keeps its name, permissions and JSON text, and also answers with structured content and declares an output schema for sessions on MCP 2025-06-18 or later (older sessions get the text only). Failures carry a stable code in their structured content (`invalid_argument`, `not_found`, `outside_workspace`, `permission_denied`, `conflict`, `stale_preview`, `index_incomplete`, `unavailable`, `timeout`, `result_too_large`, `internal_error`), and a write tool called without `--allow-write` is refused with `permission_denied` rather than as an unknown tool. `resources/list`, `check` and `symbols` page their results with opaque cursors bound to the query and the workspace revision (`Engine::workspace_revision`), refused once the workspace changes; other cut lists say `incomplete`. Tools carry read-only, destructive and idempotent annotations, which grant nothing. A request waits at most 30 s for the engine and at most 16 wait at once, answering `timeout` or `unavailable` instead of blocking. `get_nav` without a version now reads the component's latest version, as documented.
- Optional quality providers and the audit's end-to-end QA (#305): `[quality] providers = [...]` enables `compatibility` (#104), `external-links` (#86), `page-age` (#289) or `vale` (#85). Every report lists each provider's state (disabled, ran or unavailable) in JSON (`providers`), SARIF (run properties), the text output and the Quality panel. None is built yet, so one enabled is unavailable with its reason and a `provider-unavailable` gap: the audit is partial, passing by default and exiting 2 in strict mode, with the core audit's findings unchanged. `cargo xtask layers` checks that the audit crate names no file, process, network or thread API (`audit_is_offline`), and a test checks that the command line's report and the app's kept audit agree finding by finding. The user guide gains *The quality audit* (policy, exclusions, CI, providers and scope limitations), the developer guide the route for adding a provider, and `docs/qa/quality.md` the manual checks.
- The Quality sidebar panel and audit findings in Problems (#304): Run Audit (in the panel, File › Run Quality Audit or the palette) runs the documentation quality audit and keeps it current as you edit, or `[quality] enabled = true` keeps it from the start. The panel shows the audit's status with the reasons for missing coverage, an out-of-date note while a newer audit runs, findings by severity and by rule, each component version's navigation coverage as listed pages out of pages, skips, and exclusions with their reasons; a count opens Problems filtered to exactly those findings. Problems lists the audit's `quality.*` findings beside the diagnostics, each once, and gains a filter by severity, code or rule, component version and module. The engine's `Problem` now names its origin (`code` is a `FindingCode`, an existing diagnostic or an audit rule) and whether it is about the whole file (`whole_file`); `ProblemFilter`, `Engine::filtered_problems`, `Engine::problem_place`, `Engine::run_audit` (`quality.run_audit`), `audit_enabled` and `audit_stale` are new, and `adoc_audit::Policy::apply` applies the policy to one finding as a report does.
- `adocfoundry-cli audit <root>` audits the documentation's quality: what `check` reports, plus pages in no navigation file of their component version, pages with no title and block images with no alternative text. It writes text, JSON or SARIF 2.1.0 (`--format`, `--output`), scoped to the workspace or `--component [<version>@]<name>`, with paths relative to the root. The policy is the workspace's `[quality]` table (`fail_on`, `require_complete`, rule severities, exclusions with a reason); `--fail-on` and `--require-complete` override it. It exits 0 below the threshold, 1 at it, and 2 when the audit is unavailable, partial under `--require-complete` (a folder could not be read), or the policy is invalid, always writing the report first (#299, #300–#303).
- The path between two pages: select a node in the graph and Shift-click another (or Show Path from Selection) to see the shortest path, numbered on the canvas and listed step by step in the window's side panel, following links in their direction or either way; Esc clears it. `adocfoundry-cli graph --path <from> <to> [--undirected]` and the MCP server's `link_graph` with `scope: path` give the same (#287).
- File › Export Graph… saves the link graph as an SVG or PNG image, drawn as the canvas draws it: what the graph window shows, or with no window the workspace laid out afresh, on the theme's background. `adocfoundry-cli graph --format svg [--dark]` and the MCP server's `link_graph` with `format: svg` give the same drawing (#286).
- The link graph (#272): View › Show Graph (Cmd/Ctrl-K G) draws the workspace's pages, partials and navigation files in a window of their own, joined by xrefs, includes and navigation entries, with broken links as optional ghost nodes and orphans shown or hidden. Pan, zoom (wheel or pinch), drag a node to pin it, hover to highlight its neighbours; a click opens the page as a preview and a double click keeps it open. The toolbar searches, filters partials, navigation, orphans, unresolved targets and components, colours by component, module or family, switches to the active page's neighbourhood and resets the layout, and a side panel lists the selected node's links out and backlinks. The window reopens where it was, across restarts, and recentres when that place is no longer on a display.
- The Graph sidebar panel (View › Show Local Graph, Cmd/Ctrl-K Shift-G) shows the active page and its neighbours to a depth of 1 to 3, the page's place in the navigation, and its links out and backlinks, one row for each neighbour naming every kind of link. Show in Graph is in the page tab's and the Files panel's context menus.
- The graph is built once after indexing and kept current file by file while you type and when files change on disk; only a change to the Antora components, versions, modules or navigation files builds it again. A `[graph]` settings table sets what it first shows.
- Create Page on a ghost node's context menu in the graph window makes the page a broken xref expects, in the module and at the path it names, titled from its file name, and opens it; it is New Page underneath, so one undo takes it back. New Page also takes the file to make (`NewPage::file`), and `Engine::new_page_for` gives the page an unresolved target expects.
- The graph window's context menu also offers Create Partial on a ghost include, making the empty file the include expects (`page.new_partial`, `Engine::new_partial_for`), and Insert Xref at Caret on a page and Include at Caret on a partial, which write the shortest resource id that names the file from the active page at its caret (`Engine::link_edit`, `LinkStyle`).
- `adocfoundry-cli graph <root> [--page <file> [--depth <n>]] [--format json|dot]` and the MCP server's read-only `link_graph` tool give the graph to other tools.

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
