# AdocFoundry

Forge your documentation in real time.

AdocFoundry is a native desktop editor for AsciiDoc documentation laid out the Antora way, written in Rust for macOS, Linux and Windows. It is a thin GPUI application over a headless engine, and the same engine powers a command-line checker (`adocfoundry-cli`) and a language server (`adoc-ls`) for other editors.

The project is built in phases: (1) a source editor with Antora support and link following, (2) a native rendered preview, (3) WYSIWYG editing, (4) PDF export with themes, (5) release readiness: fixes, typeset STEM, `adoc-ls` on the engine, an MCP server and packages, then (6) inline STEM in the text flow, MCP over HTTP, signed releases and updates. **All six phases are implemented** and released, the current release being 0.5.0. The manual QA on each platform continues in [`docs/qa/phase5.md`](docs/qa/phase5.md) and [`docs/qa/phase6.md`](docs/qa/phase6.md). The architecture is described in [`docs/design/DESIGN-AdocFoundry_Architecture.md`](docs/design/DESIGN-AdocFoundry_Architecture.md) and each phase's scope in [`docs/prd/PRD-AdocFoundry_Phase_1_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_1_Implementation_Specification.md), [`docs/prd/PRD-AdocFoundry_Phase_2_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_2_Implementation_Specification.md), [`docs/prd/PRD-AdocFoundry_Phase_3_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_3_Implementation_Specification.md), [`docs/prd/PRD-AdocFoundry_Phase_4_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_4_Implementation_Specification.md), [`docs/prd/PRD-AdocFoundry_Phase_5_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_5_Implementation_Specification.md) and [`docs/prd/PRD-AdocFoundry_Phase_6_Implementation_Specification.md`](docs/prd/PRD-AdocFoundry_Phase_6_Implementation_Specification.md).

## What works

**Workspace and Antora**

- Opens a folder and discovers every `antora.yml` beneath it, so several components and versions live in one workspace. Plain AsciiDoc folders work too, with relative-path resolution.
- Resolves Antora resource IDs in all five coordinates (`version@component:module:family$path`) following Antora 3.2: a component without a version means its latest version (the versionless `~` version first, then released versions above `prerelease` ones); a component without a module means its `ROOT` module.
- Honours `:page-aliases:`: an xref to an alias resolves to the page and carries an information diagnostic suggesting the current id.
- Re-reads `antora.yml` when it changes, and re-indexes files changed on disk. A buffer with unsaved edits is never overwritten; you are told the file changed. Saving it then asks whether to reload the file's version (Undo brings your edits back) or overwrite it with yours, rather than silently writing over the change. Saves go through a temporary file, so a failed write never leaves a truncated page.

**Sidebar**

- *Navigation*: each component version's navigation as its `nav.adoc` files list it, or its pages by module when it declares none.
- *Modules*: every module with its families (pages, partials, examples, images, attachments) and their resources.
- *Files*: the folders and files on disk beneath each workspace folder, of every type, read a folder at a time as you open them. Dotfiles show; `workspace.ignore` entries are hidden unless Show Excluded Files is on; links show but are not followed. A click opens a file in a preview tab (text files that are not AsciiDoc open as plain, highlighted text, outside the index; images open in the image viewer), PDFs open in the system's viewer, and other binary files (fonts, archives, programs) are refused, opening only through Open Externally, so selecting a file never runs it. Right-click for Copy Path, Copy Relative Path, Reveal in Finder (or File Explorer, or File Manager), Refresh and Collapse All; Reveal in Files shows the active document there.
- Both open two levels deep by default (`[sidebar.navigation]` and `[sidebar.modules]` `default_expansion`: a number of levels, or `"all"` for the old fully open trees). The branches you open and close stay that way through re-indexing and are remembered for the workspace across restarts (`[sidebar] remember_expansion`). Right-click either panel for Expand All, Collapse All, Collapse One Level and Reset to Configured Default; the View menu and the palette have them too, for the panel shown.
- Files, Navigation and Modules follow the active page: whenever another page or image becomes active, each selects its row, opening the folders and branches above it, without taking focus. Navigation selects the first entry linking to the page, preferring one without a `#fragment` (or keeps the one you clicked), and clears its selection for a page in no navigation file. `[sidebar] follow_active = false` turns this off.
- *Problems*: workspace diagnostics grouped by file, each with its line and column; click one to go to it. The engine keeps them, diagnosing the whole workspace on a worker and, after an edit, only the pages it can affect, so the panel never slows typing.
- *Outline* of the active document; for a navigation file, its navigation tree.
- Drag a page from Navigation or Modules, or a section from the Outline, into the source or visual editor to insert an xref to it where you drop it, as one undo step. The target is the shortest resource ID that resolves to the page from the page being edited (`other.adoc`, `module:other.adoc`, `component:module:other.adoc`, or `component::other.adoc` for a ROOT module), with `version@` only for a version other than the latest, never an alias; outside Antora it is the page's relative path. The link text is the page's or section's title, a section adds its id (`guide.adoc#setup`), and a section of the page itself becomes `<<setup,Setup>>`.

**Editor**

- Syntax highlighting from the AsciiDoc tree-sitter grammars, with the inline grammar injected into paragraphs, titles, table cells and macro targets, and source blocks highlighted for Rust, Java, JavaScript, TypeScript, Python, Go, JSON, YAML, TOML, shell, SQL and HTML.
- Folding of sections, delimited blocks and lists; line numbers; soft wrap.
- Diagnostics as you type for unresolved xrefs, anchors, includes and images, duplicate anchors, unknown Antora modules and resources, invalid resource IDs and invalid component descriptors. Inside `antora.yml`, at the line concerned: a missing `name` or `version`, a key Antora does not use, a `nav` file that does not exist, and an attribute value that is a list or mapping. Diagnostics are conservative: warnings rather than errors, and nothing for dynamic constructs such as `include::{attr}/x.adoc[]`.
- Hover and completion for xref, include and image targets, anchors and `{attributes}`.
- Undo and redo that treat a run of typing as one step.
- Spell checking of prose in the source and visual editors: misspelt words are underlined half a second after typing stops, and right-clicking one offers suggestions (each replaces the word as one undo step), Add to Workspace Dictionary (`.adocfoundry-words.txt` in the workspace root, to commit with the repository) and Add to User Dictionary (`spelling-words.txt` beside the user settings file). Code, literal blocks, inline code, attributes, references, links and macros are never checked, and included text is checked in its own file. A page is checked in its `:lang:`, else `[spelling] language`; British and American English are built in, and `[spelling] dictionaries` adds Hunspell dictionaries. `adoc-ls` publishes the same misspellings as hints with quick fixes.
- The status bar shows the page's word count and reading time (`120 words · 1 min read`), and the selection's count while there is one (`12 of 120 words`). Only prose counts: not markup, attributes, code, diagrams or formulas. The page is counted on a worker half a second after typing stops.
- Optional auto-save (`[editor] auto_save`): `after_delay` saves a page a second after typing stops, and `on_focus_change` when you leave its tab or the window. It saves through Save, so a file changed on disk is never overwritten: the conflict is reported once, and that page is left until you save, overwrite or reload it.
- Crash recovery: a page's unsaved text is copied to the app's state folder two seconds after typing stops and when the app quits, and the copy goes once the page is saved, closed or undone back to its saved text. If the app ends without saving, the next launch lists what it left, each beside the lines it changes in the file on disk, to recover (as one undo step, unsaved) or discard; File › Recover Unsaved Changes… lists it again. A copy of the app still running is never offered another's unsaved text, and a page renamed in the app is recovered at its new path.
- Local history: every save keeps the text saved, and first the text it replaces when that is not kept already (the file before your first save, or a change made outside the app), in the app's state folder and independently of Git. File › Local History… lists the page's versions, shows what the page has changed since the one selected, and restores it as one undo step, unsaved. Versions are kept for `[history] keep_days`, at most `max_versions` of each page and `max_size_mb` in all, the oldest going first; pages over 4 MB are not kept. A page renamed in the app keeps its history.
- Tabs close from their X, or from a right-click menu that acts on the tab clicked: Close, Close Others, Close Left, Close Right, Close Clean (the tabs without unsaved changes) and Close All, asking once about any unsaved changes. Arrows at the start of the tab bar go back and forward.
- The last session comes back: launched with no folder or file, the app reopens the workspace's folders, the tabs in their order with each page's editor mode and caret, the page in the preview tab, the active tab and the preview's layout (shown, docked or detached, scroll sync, PDF, the pinned page), skipping files and folders that have gone. The window is shown first and the tabs open straight after it; a visual page behind the active one builds its visual editor when it is shown. It is saved when the app quits or its window closes. File › Open Recent Folder… (also in the palette) lists the last ten folders opened as a workspace, most recent first, to open again.
- Quick Open (Cmd/Ctrl-P, also in the File menu) finds a file by name, title, Antora resource id or path: each word typed must match one of them, its letters in order and in any case (`gde` finds `guide.adoc`, `beta refer` beta's `reference.adoc`). Each row shows the title, the resource id (`1.0@alpha:ROOT:guide.adoc`) and the path. Files opened recently come first, most recent first, then the rest by how well they match; Enter opens the one chosen as an ordinary tab. It reads the index, never the disk, and answers in under 5 ms even in a workspace of 20,000 pages.
- Browsing opens pages in one preview tab, its title in italics, which the next page browsed to replaces in place, as in VS Code and Zed. Browsing means clicking a page in Navigation, Modules, search results, Problems or Changes, following a link, and Back and Forward. Editing or saving the page, double-clicking its tab or sidebar entry, or Keep Open (tab menu, File menu or palette) makes it an ordinary tab. A page with unsaved changes, or pinned in the rendered preview, is never replaced. Open File and files named at start-up always open as ordinary tabs. `preview_tabs = false` under `[editor]` keeps every page open, as before.
- Images open in an image tab, from Files, a module's images, Open File, the command line or an image macro followed as a link, under the same preview-tab rules as pages. PNG (with transparency), JPEG (turned as its EXIF orientation says), SVG, BMP, ICO, the first frame of GIF and WebP and the first page of TIFF are shown; HEIC, HEIF and AVIF say they cannot be, and offer Open Externally, as does a damaged file. The toolbar shows the format, size and file size, Zoom In and Out, Fit to Window (`Cmd`/`Ctrl` `9`), Actual Size (`0`: one image pixel to one screen pixel, an SVG's own size) and Reload; scroll or drag to pan, and transparent pixels show over a checkerboard. An image is read-only and never text: no buffer, no index, nothing Save could write. It reloads when the watcher sees it change; an image that fails to reload keeps its last pixels, marked stale. An SVG shows only what it holds: scripts never run, links are not followed and linked images are not loaded. Files over 256 MB, over 16,384 pixels a side or over 64 megapixels are refused with their size.
- Bold, Italic and Code (Cmd/Ctrl-B, -I, -E) and Highlight wrap the selection in its delimiters, or remove them when it is already marked: the constrained form (`*bold*`) where it is valid, else the unconstrained one (`un**break**able`), each line of a selection on its own.
- Insert Table, Insert Code Block, Insert Admonition and Insert Image add the visual editor's skeleton at the caret, with blank lines around it, and select where typing goes next; a code block goes around the selected lines. Insert Link (Cmd/Ctrl-Shift-L) wraps the selection as `link:[text]` with the caret at the target. Each is one undo step.
- Format Table (Format menu and palette) pads the cells of the table at the caret so its `|` separators line up, as one undo step, without changing what it renders. Tables with cells spread over lines, spans or styles, or another separator, are left alone with the reason.

**Link following**

- Cmd-click (Ctrl-click on Linux and Windows) or F12 on an `xref:`, `include::` or `{attribute}` opens the target and places the caret there; images and attachments open in the system's default application.
- Back and forward through the places you have followed links to.
- Alt-F12 on an `include::` shows the included file read-only.
- Find References (Alt-Shift-F12, Go menu) lists every reference to the anchor or section heading at the caret (`<<id>>` and `xref:page.adoc#id[]`, across components), or else every xref, include and nav entry that names the page, grouped by file in a panel below the editor; clicking one opens it there.
- Go to Symbol in Workspace (Cmd/Ctrl-T) lists the sections, anchors and page titles across the workspace whose words start with what you type, each with its page and, in Antora, its component version and module; Enter opens it there, recorded in back and forward history. Go to Symbol in Page (Cmd/Ctrl-R) does the same for the active page. Both are in the Go menu.

**Preview**

- A native rendered preview beside the editor (Cmd/Ctrl-K V), drawn by the app itself: no browser, Asciidoctor, Node.js or Java. It re-renders 100 ms after you stop typing in the page or in any file it includes, unsaved edits included.
- It follows the active page; Pin keeps it on one page. Each page's scroll position is remembered.
- Detach moves it into its own window; closing that window docks it again.
- Scroll sync both ways: moving the caret scrolls the preview to that block, and scrolling the preview moves the source to the topmost visible block. Sync can be switched off.
- A live PDF preview: View › Toggle PDF Preview, or PDF in the preview's header, shows the page as Export Page to PDF typesets it, in its theme and fonts, re-typeset on a worker a moment after you stop typing (about an eighth of a second on an ordinary page), so the editor never waits. Clicking a line on a page goes to its source.
- Clicking a block places the caret on its source, opening the partial it came from when there is one. Clicking an xref follows it, recorded in back and forward history; external links open the browser and attachments the default application.
- Unresolved xrefs are underlined in red, and missing images show a placeholder naming the target.
- Includes resolve as Antora resolves them, including qualified ids such as `include::other:partial$note.adoc[]`, and images and attachments are also found in a module's older `assets/images` and `assets/attachments` directories.
- Source blocks are highlighted with the editor's grammars. Mermaid diagrams render in process; Graphviz, PlantUML and D2 diagrams render through `dot`, `plantuml` or `d2`, from `[tools]` or else `PATH`, and otherwise show their source with the reason. At most four diagrams render at once, and a tool that runs past `[tools] diagram_timeout` or writes more than 32 MB is stopped. Rendered diagrams are cached in `.cache/adocfoundry/diagrams` in the workspace.
- STEM is typeset: block formulas are drawn as images, and inline formulas are drawn in the line of text, on its baseline and at its size, with their source in a tooltip; the visual editor draws them the same way. A formula that cannot be converted shows as its source.

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

**PDF export**

- Export Page to PDF… writes the active page as an article; Export Component to PDF… writes its Antora component version as a book in navigation order: a title page, a table of contents, a part for each navigation file (or titled list) when there are several, a chapter for each top-level entry and sections beneath, with cross references between its pages as links inside the PDF. Both are in the command palette and in `adocfoundry-cli pdf`.
- Publication profiles: `[pdf.profiles.<name>]` in `.adocfoundry.toml` saves an export's content (a page, a component version, or navigation files in a chosen order), output, theme, fonts, attributes and metadata, and standards. File › Export PDF Profile… and `adocfoundry-cli pdf --profile <name>` export it, writing the same PDF; Export Page to PDF with Theme… offers the bundled themes and the workspace's own.
- Export Page to HTML… writes the active page, unsaved edits included, as a standalone HTML file: what `adocfoundry-cli render` writes, with local images, diagrams and formulas embedded so the file opens anywhere.
- Typeset in process by Typst, with no Asciidoctor, Ruby, Java or network; unsaved edits are exported, diagrams are rendered first, and Typst's errors point at the AsciiDoc line that caused them.
- Styled by asciidoctor-pdf themes, loaded as asciidoctor-pdf loads them (`extends`, `$variables`, arithmetic, units, colours, the font catalogue), so an existing theme loads without edits. Of the 567 keys in asciidoctor-pdf 2.3's Theme Keys Reference, 263 take effect as they do there, 24 are approximated and 280 are ignored; the user guide's PDF theme keys page lists each one, and a test keeps that list in step with the template. The bundled `default`, `default-sans`, `default-for-print` and `base` themes use the same Noto and M+ fonts asciidoctor-pdf ships; on the reference article, every line lands within 2 pt of asciidoctor-pdf's.
- Block attributes and options are honoured in PDF and HTML export as asciidoctor-pdf and Asciidoctor honour them: an ordered list's numbering style (`[loweralpha]`, `[upperroman]`, or by nesting depth), `start` and `%reversed`; a table's `frame`, `grid`, `stripes`, `width` and `%autowidth`, and cells aligned vertically (`.^`, `.>`); an image's `align`, `float`, `link` and `pdfwidth`; a block's own `caption`; and `%collapsible` blocks (a disclosure in HTML, `▼ Title` and the indented contents in PDF). The preview does not show them yet.
- Source blocks highlighted, when `source-highlighter` is set, with the editor's tree-sitter grammars in the preview's light colours, so a listing looks as it does in the preview; languages without a grammar fall back to Typst's highlighter.
- Headers and footers with recto and verso columns, page numbers and section titles; a title page with logo, authors and revision; dot leaders in the table of contents; an index from `((terms))` in an `[index]` section; PDF/A-1b, PDF/A-1a, PDF/A-2a and PDF/UA-1 on request.
- STEM typeset from AsciiMath or LaTeX (following `:stem:`) in STIX Two Math, with the source as each formula's alternative text; a formula that cannot be converted, or uses anything but maths, is shown as its source with a warning naming its line.

**Find, replace and rename**

- Find in the workspace, with regular expression, case and whole-word options. Results are listed as they are found, open pages are searched with their unsaved edits, and a search stops at 10,000 matches and says so.
- Replace across the workspace shows every change first; applying it is one undo step.
- New Page… (Cmd/Ctrl-N, also in the File menu, in an Antora workspace) asks for a title and a module, the active page's to begin with, writes `pages/<slug>.adoc` holding `= Title` (`Release Notes` gives `release-notes.adoc`) and opens it. Unless told not to, it adds `* xref:<slug>.adoc[]` to the module's `nav.adoc` after the active page's entry, at its level, or else at the end, so the page resolves and is in the Navigation panel at once. One undo takes the entry out and deletes the file, as long as the file is still as it was made.
- Rename Page moves the file and rewrites every xref that leads to it, shown as a preview first; one undo moves the page back and restores every xref.
- F2 on an anchor (`[[id]]`, `[#id]`, also with roles and options as in `[#id.role%option]`, or `[id=id]`), a section heading or a reference to one renames the anchor instead (Rename Anchor): its declaration and every `<<id>>` and `xref:page.adoc#id[]` that names it, in every page, shown as a preview first and undone as one step. A section without an anchor gets `[[new-id]]` above its heading, and references written with its generated id (Antora's or Asciidoctor's form) are rewritten. Elsewhere F2 renames the page.
- A replace or rename is undone and redone only as a whole. If one of its pages was edited afterwards, undo says which; undo that page's later edits first.

**Also**

- A ☰ menu at the left of the title bar that opens into File, Edit, Format, View, Go, Git and Help menus, as in Zed; a command palette listing every command; light and dark themes; a read-only second window that follows the active document; and TOML settings.
- Help › About AdocFoundry shows the version, build and licence, links to the repository and the documentation, and every third-party component the app is built from (the crates of the release build, and the bundled fonts, dictionaries and PDF themes) with its version, licence and full licence text, ready to copy.

## Installing

Releases are on the [GitHub releases page](https://github.com/bovinemagnet/AdocFoundry/releases). Every package carries the app and the `adocfoundry-cli`, `adoc-ls` and `adoc-mcp` binaries; `adoc-ls`, `adocfoundry-cli` and `adoc-mcp` are also published as archives of their own. Nothing else is needed at run time: no Ruby, Node.js or Java. Each package and archive ships the third-party notices as `THIRD-PARTY-NOTICES.txt`, beside the dictionaries' licence `SCOWL.txt`.

- **macOS** (universal): open `AdocFoundry-<version>-macos-universal.dmg` and drag AdocFoundry to Applications. A release built with the project's signing credentials is signed with a Developer ID and notarised, so it opens without a prompt; one built without them (every release up to 0.2.1) is signed ad hoc, so macOS asks before opening it the first time: Control-click the app, choose Open, and confirm. The release notes say which a release is. The command-line tools are in `AdocFoundry.app/Contents/MacOS`; link them onto your `PATH`, for example `ln -s /Applications/AdocFoundry.app/Contents/MacOS/adoc-ls /usr/local/bin/`.
- **Windows** (x64 and ARM64): run the MSI, which can add the tools to `PATH`, or unpack the portable zip. A release built with the signing credentials has Authenticode signatures on the executables and the MSI; an unsigned one (every release up to 0.2.1) makes SmartScreen ask before running it: choose More info, then Run anyway.
- **Linux** (x86_64 and aarch64): the AppImage (`chmod +x`, then run it), the deb (`sudo apt install ./AdocFoundry-….deb`) or the rpm (`sudo dnf install ./adocfoundry-….rpm`). A Flatpak manifest is in `packaging/flatpak/`.

The release workflow (`.github/workflows/release.yml`, run on `v*` tags) builds these, signs them when the signing secrets are set (see the developer documentation's Building and releasing page), and smoke-tests each platform's downloads (`packaging/smoke-test.py`), signatures included. It runs CI on the tagged commit too, and publishes the release only once that passes. Before anything is built it checks that the tag names the version `Cargo.toml`, `src/docs/antora.yml` and `CHANGELOG.md` give (`cargo xtask bump <version>` writes all three), the smoke tests check every binary reports it, and the release notes start with the changelog's section for it. On Linux, `cargo xtask package`, `cargo deb -p adocfoundry` and `cargo generate-rpm -p crates/adocfoundry` build the same packages locally.

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

`adocfoundry [folder [file…]]` opens a folder as the workspace and any files in tabs. Open Folder (Cmd/Ctrl-Shift-O) does the same from inside the app. Launched with neither, it reopens the last session.

| Command | macOS | Linux and Windows |
|---|---|---|
| Command palette | Cmd-Shift-P | Ctrl-Shift-P |
| Open folder / file | Cmd-Shift-O / Cmd-O | Ctrl-Shift-O / Ctrl-O |
| Quick Open | Cmd-P | Ctrl-P |
| New page | Cmd-N | Ctrl-N |
| Save / Save all / Close | Cmd-S / Cmd-Alt-S / Cmd-W | Ctrl-S / Ctrl-Alt-S / Ctrl-W |
| Undo / Redo | Cmd-Z / Cmd-Shift-Z | Ctrl-Z / Ctrl-Shift-Z |
| Follow link | Cmd-click, F12, Cmd-Enter | Ctrl-click, F12, Ctrl-Enter |
| Close others / left / right | Cmd-Alt-T / Cmd-K E / Cmd-K T | Ctrl-Alt-T / Ctrl-K E / Ctrl-K T |
| Close clean / all | Cmd-K U / Cmd-K Cmd-W | Ctrl-K U / Ctrl-K Ctrl-W |
| Back / Forward | Ctrl-minus / Ctrl-Shift-minus | Ctrl-minus / Ctrl-Shift-minus |
| Peek include | Alt-F12 | Alt-F12 |
| Toggle preview | Cmd-K V | Ctrl-K V |
| Show source / visual / split | Cmd-K S / Cmd-K W / Cmd-K D | Ctrl-K S / Ctrl-K W / Ctrl-K D |
| Bold / Italic / Code | Cmd-B / Cmd-I / Cmd-E | Ctrl-B / Ctrl-I / Ctrl-E |
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

Settings are TOML. The user file is `adocfoundry/adocfoundry.toml` in the platform configuration directory (`~/.config` on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on Windows); Open Settings creates it. A workspace may override `editor.*`, `workspace.ignore` and `pdf.*` in `.adocfoundry.toml` at its root. Saving either file applies it. Unknown keys produce a warning, never an error.

```toml
[editor]
font_family = "JetBrains Mono"   # falls back to the platform monospace
font_size = 13
soft_wrap = true
tab_width = 2
theme = "auto"                   # auto | light | dark
preview_tabs = true              # false keeps every page you open in a tab of its own
auto_save = "off"                # off | on_focus_change | after_delay (a second after typing stops)

[workspace]
ignore = [".cache", ".git", ".zed", "node_modules", "build", "target", "dist", ".idea", ".gradle"]   # never indexed, discovered or watched

[sidebar]
remember_expansion = true        # user file only
follow_active = true             # reveal the active page in Files, Navigation and Modules

[sidebar.navigation]
default_expansion = 2            # levels, 0 or more, or "all" to open every branch

[sidebar.modules]
default_expansion = 2

[tools]
asciidoctor = ""                 # optional absolute path; discovered on PATH if empty
git = ""                         # the git that pushes use; discovered on PATH if empty
dot = ""                         # Graphviz; e.g. "/opt/homebrew/bin/dot" for an app started from the Dock
plantuml = ""                    # discovered on PATH if empty
d2 = ""                          # discovered on PATH if empty
diagram_timeout = 30             # seconds a diagram tool may run before it is stopped

[git]
enabled = true                   # false: look for no repository
show_markers = true              # change markers in both editors
blame_on_hover = false           # line blame when resting on the gutter
remote = "origin"                # the remote that links to the hosting site use

[git.hosts]                      # self-hosted sites: github | gitlab | bitbucket | azure
# "git.example.com" = "gitlab"

[pdf]
theme = "default"                # a bundled theme, a name in themes_dir, or a .yml file
# themes_dir = "resources/themes"  # relative to the workspace root, like -a pdf-themesdir
# fonts_dir = "resources/fonts"    # like -a pdf-fontsdir; the bundled fonts are searched after it
# site_url = "https://docs.example.com"  # where references out of an export lead; else the playbook's site.url
standards = []                   # "pdf-a-1b", "pdf-a-1a", "pdf-a-2a", "pdf-ua-1"

# [pdf.profiles.manual]          # a publication: run with File › Export PDF Profile… or `pdf --profile manual`
# nav = ["docs/modules/admin/nav.adoc", "docs/modules/ROOT/nav.adoc"]  # or page = "…" or component = "1.0@docs"
# output = "build/manual.pdf"    # relative to the workspace root; <name>.pdf by default
# theme = "acme"
# attributes = { pdf-title = "The Manual", product = "Acme" }

[spelling]
enabled = true                   # underline misspelt words in prose
language = "en_GB"               # for pages with no lang attribute
dictionaries = ""                # folder of extra Hunspell .aff/.dic files (user file only)

[updates]
check = true                     # look for a newer release at start-up, at most once a day

[history]                        # user file only
enabled = true                   # keep each page's saved versions, for File › Local History…
keep_days = 30                   # versions older than this are removed
max_versions = 50                # at most this many versions of each page
max_size_mb = 256                # the whole history, oldest versions removed first
```

A page's `pdf-theme`, `pdf-themesdir` and `pdf-fontsdir` attributes win over it, as they do for asciidoctor-pdf.

With `updates.check` on, the app reads GitHub's releases feed for this repository through the system's `curl` at start-up, at most once a day, and says once when a newer release exists; Check for Updates in the command palette checks whenever asked. The request carries nothing but the app's version in its user agent. The last check is remembered in `update-state.toml` beside the settings file. An app installed from a signed DMG or MSI offers to update itself: it downloads the package, checks it against the release's `SHA256SUMS` and its signature against its own, installs it and restarts; it will not while a document has unsaved changes. The AppImage carries update information for AppImageUpdate, and the other packages are left to their package managers.

Unsaved text for recovery and the local history are kept in the `recovery` and `history` folders of the app's state folder, and the last session and the folders opened recently in its `session.json` and `recent-folders.json`. The state folder is apart from the settings file, which is often synchronised or kept in a repository: `~/.local/state/adocfoundry` on Linux (`$XDG_STATE_HOME`), `~/Library/Application Support/adocfoundry` on macOS and `%LOCALAPPDATA%\adocfoundry` on Windows. Only the app writes them, never `adoc-ls`, `adoc-mcp` or the CLI.

`tools.asciidoctor` is used only by `adoc-ls`'s HTML preview; the app's preview needs no external tool. `dot`, `plantuml` and `d2` are the diagram tools; each is found on `PATH` when left empty, which an app started from the Dock or Finder may not share with your shell.

## Command line

```text
adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
adocfoundry-cli symbols <root> <query>
adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]
adocfoundry-cli pdf <file|component-dir> [--root <dir>] [--component [<version>@]<name>]
    [--theme <name|file.yml>] [--fonts-dir <dir>] [--output <file.pdf>]
    [--standard pdf-a-1b|pdf-a-1a|pdf-a-2a|pdf-ua-1] [--typst <file.typ>]
adocfoundry-cli pdf --profile <name> [<root>] [--output <file.pdf>] [--typst <file.typ>]
adocfoundry-cli git-status <root> [--format text|json]
adocfoundry-cli doctor [<root>] [--format text|json]
```

`check` prints the workspace's diagnostics (text: `path:line:col: severity code message`) and exits with 1 when any meets `--fail-on` (default `error`), which suits CI. It reports exactly what the editor and `adoc-ls` report.

`render` writes a page as a standalone HTML file, as the preview renders it, waiting for its diagrams; without `--output` it writes to standard output. Without `--root`, the workspace is the folder above the page's Antora component, or the page's own folder outside Antora. `--parser asciidoc-parser` renders with the second document parser, for comparison; the app always uses `acdc`.

`pdf` exports a page to `<page>.pdf` beside it, or a component directory (or `--component`) as a book to `<name>.pdf`, and prints the file and its page count. Warnings print as `path:line: warning: message`; the exit code is 1 when the export fails. `--typst` also writes the generated Typst, for inspection.

`git-status` prints the branch of the repository holding the root and its changed AsciiDoc files as `git status --short` does, with the number of changed line runs against `HEAD` in each (`branch main`, ` M docs/page.adoc (2 changed)`); `--format json` gives the same as an object. The other commands do not look at Git.

`doctor` prints a report to paste into an issue: the version and build, the OS and architecture, the PDF theme and whether its fonts are found, each optional tool (`asciidoctor`, `dot`, `plantuml`, `d2`, `git`) with its path and version or why it is missing, and the settings files the app reads (and `<root>`'s, when given) with any warnings. In the app, Help › Show Diagnostics shows the same report, adding the graphics backend and the editor and interface fonts, with a Copy button. The bug report template asks for it.

## Language server

`adoc-ls --stdio` runs the same engine as the app and serves any LSP editor:

- Diagnostics (the same codes and severities as the Problems panel, republished for every open page a change affects), definition, hover, completion, and document and workspace symbols.
- References: on a page, the pages that link to it and the includes that include it; on an anchor or a reference to one, every reference to that anchor.
- Renaming: renaming an `.adoc` file in the editor first rewrites the xrefs and nav entries that name it (`workspace/willRenameFiles`); renaming an anchor id renames it and its references.
- Commands: `adoc.renderPreview`, `adoc.renderLivePreview` and `adoc.stopLivePreview` open an HTML preview, through Asciidoctor when `tools.asciidoctor` is set or `asciidoctor` is on `PATH`, else through the engine, with diagrams and typeset STEM; `adoc.checkWorkspace` publishes every file's diagnostics; `adoc.exportPdf { uri, component?, output, theme?, standards? }` writes a PDF, in builds with Typst.

Settings come from the files the app reads: the user settings file and the workspace's `.adocfoundry.toml`. Git is off unless the user settings file sets `[git] enabled = true`.

The [`adoc-ls` README](crates/adoc-ls/README.md) covers installation, editor set-up, the commands, settings and diagnostic codes.

The Zed extension [AsciiDoc and Antora](https://github.com/bovinemagnet/zed-asciidoc-antora) (`asciidoc-antora`) uses an `adoc-ls` on `PATH` when there is one. Otherwise it downloads `adoc-ls` from the latest release here, for Linux, macOS and Windows on x86-64 and AArch64, and keeps it up to date. If the release cannot be fetched, it falls back to its most recent earlier download. The release archives are built with Typst and Git. Zed's `lsp.adoc-ls.binary` setting can name another binary instead.

To run your own build, install it on `PATH`. The default build is lean, without Typst or Git:

```sh
cargo install --git https://github.com/bovinemagnet/AdocFoundry adoc-ls
```

With PDF export, typeset STEM in the preview and Git:

```sh
cargo install --git https://github.com/bovinemagnet/AdocFoundry adoc-ls --features typst,git
```

## MCP server

`adoc-mcp <root>… [--allow-write] [--http <addr>]` serves an Antora workspace to MCP clients (assistants and tools) over stdio, or over Streamable HTTP on a loopback address, through the same engine as the app. Its tools read pages (by path or resource id, with unsaved edits), list components and navigation, resolve xref, include and image targets, check the workspace (the same diagnostics as `adocfoundry-cli check --format json`), find symbols, search, render a page as text or HTML and report Git status. With `--allow-write` it also edits pages, renames them (rewriting the xrefs, nav entries and includes that name them) and exports PDF; without it those tools are not offered. Every page is also a `file://` resource, and `antora:{resource_id}` reads one by its Antora id. Paths outside the roots are refused, and so is every file a page would read outside them: includes, images, PDF themes and fonts. External diagram tools are not run, since they read files of their own, so only Mermaid diagrams render. The server makes no network requests, and long results are cut with the total stated.

```json
{
  "mcpServers": {
    "adocfoundry": { "command": "adoc-mcp", "args": ["/path/to/docs"] }
  }
}
```

With `--http 127.0.0.1:8765` (any other loopback address, or port 0 for a free one) the server listens for MCP's Streamable HTTP transport at `/mcp` instead. A non-loopback address is refused. Every request must carry `Authorization: Bearer <token>`, where the token is `ADOC_MCP_TOKEN`, or a random one printed once to standard error when that is unset. Clients that connect to a running server take its URL and header:

```json
{
  "mcpServers": {
    "adocfoundry": {
      "url": "http://127.0.0.1:8765/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

## Not implemented yet

- In PDF export: PDF/UA-2 (Typst writes only PDF/UA-1), and asciidoctor-pdf's Ruby extensions.
- In the visual editor: selections and deletions across blocks, Enter at the start of a block, and editing included content in place (Edit partial opens the partial).
- Editing included content in place, plugins.
- Merging, rebasing, conflicts and history: use a Git client.
- XML and Dockerfile highlighting inside source blocks: the editor component has no grammar for them yet.
- Verified IME composition and frame timing on every platform, the packages on macOS and Windows, and the preview checked by hand on each platform: see [`docs/qa/phase1.md`](docs/qa/phase1.md), [`docs/qa/phase2.md`](docs/qa/phase2.md), [`docs/qa/phase3.md`](docs/qa/phase3.md), [`docs/qa/phase4.md`](docs/qa/phase4.md) and [`docs/qa/spike-gpui.md`](docs/qa/spike-gpui.md).

## Developing

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo xtask layers      # the crate dependency ladder
cargo xtask package     # release build and archive for this platform in dist/
cargo xtask notices     # the third-party notices; with a file name, written to it
```

The workspace is a ladder of crates, each depending only on those below it:

```text
adocfoundry (GPUI app)   adocfoundry-cli   adoc-ls (LSP)   adoc-mcp (MCP)
adoc-engine              documents, commands, undo, workspace, watcher, settings, search, preview, visual model
adoc-render   adoc-diagrams   adoc-model   adoc-git   adoc-pdf   view tree and HTML; diagrams; the editable model; Git; PDF
adoc-parser   adoc-dom   adoc-antora   adoc-index   adoc-lang
adoc-core
```

`adoc-dom` holds the full document tree behind the `DocumentParser` trait and is the only crate that names a parser crate (acdc-parser, and asciidoc-parser for comparison). `adoc-model` is the visual editor's model: it reads each text block of a page losslessly (every source byte is text, a mark delimiter or part of an atom), checks its reading against the parser's, and turns edits into the smallest source edits; its property tests (`cargo test -p adoc-model --test roundtrip`) check that building and writing back is the identity on every fixture and documentation page and that a long random editing session changes only the lines it edited. The fidelity harness (`cargo test -p adocfoundry-cli --test fidelity`) compares `render` with Asciidoctor over `tests/fixtures/render/` (block structure, paragraph text, and how block attributes and options show: list numbering, table frame, grid, stripes and width, cell alignment, image placement, collapsible blocks), and skips itself when `asciidoctor --version` fails.

`adoc-pdf` loads asciidoctor-pdf themes (checked key by key against asciidoctor-pdf's own loader in `tests/fixtures/pdf/themes/expected/`), writes the view tree as Typst with every document string a Typst string literal, and compiles it in process with bundled fonts. The visual regression harness (`cargo test -p adocfoundry-cli --test pdf_regression`) renders reference documents with `asciidoctor-pdf` and with `adocfoundry-cli pdf`, rasterises both with `hayro` and compares every page; it skips itself without `asciidoctor-pdf` and writes failing pages to `target/pdf-regression/`.

## Licence

Apache-2.0. See [LICENSE](LICENSE). The third-party components and their licences are listed in Help › About AdocFoundry and in each package's `THIRD-PARTY-NOTICES.txt`, generated from Cargo's metadata by `xtask/src/notices.rs`.
