# Phase 1 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Phase 1 acceptance (spec §11 and §17) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ (AppImage) before a release. Record the build, the platform and the result in the table at the end.

Automated coverage for reference: the engine's headless tests (`crates/adoc-engine/tests/`), the CLI snapshot tests (`crates/adocfoundry-cli/tests/cli.rs`), and the app's test-platform tests (`crates/adocfoundry/src/ui_tests.rs`) for editor sync, undo routing, link following and the second window.

## Setup

1. `git submodule update --init`, then `cargo run --release -p adocfoundry -- tests/fixtures/antora-multi-component`.
2. For a real component, clone Antora's own documentation (`https://gitlab.com/antora/antora`, `docs/`) and open that folder.

## Workspace and sidebar

- [ ] The Navigation tab shows each component version, nested as its `nav.adoc` lists it; a component without `nav` shows its pages by module.
- [ ] On Antora's own docs, the navigation matches the published site's navigation.
- [ ] The Modules tab lists every module and its families with their resources.
- [ ] The Problems tab lists workspace diagnostics; clicking one opens the file at the location.
- [ ] Editing `antora.yml` (rename the component) updates the sidebar without a restart.

## Editor

- [ ] Highlighting of titles, attributes, blocks, macros, inline formatting and `[source,rust]` blocks matches Zed's for the same file.
- [ ] Sections and delimited blocks fold.
- [ ] Typing, deleting and pasting update the outline and diagnostics as you type.
- [ ] Undo and redo (Cmd/Ctrl-Z, Cmd/Ctrl-Shift-Z) undo typing runs as one step.
- [ ] Hovering an xref, include, image or `{attribute}` shows the target.
- [ ] Completion appears after `xref:`, `include::`, `image::`, `{` and `#`.
- [ ] A file changed on disk while open and clean reloads; while dirty, a notification appears and the buffer is kept.
- [ ] Typing in a 5,000-line file is smooth: no dropped frames during a 10-second burst. Measure on an idle desktop with the window visible:
  `ADOCFOUNDRY_TYPING_BENCH=10 adocfoundry <folder> <5000-line file>` types into the editor for ten seconds and prints the frame intervals (median, p99, worst, and counts over 16.7 ms and 25 ms). Run it once on a three-line file as a control; at 60 Hz the median is about 16.7 ms, and frames over 25 ms are dropped.
- [ ] IME: Japanese input composes and commits correctly (macOS, Windows); ibus on Linux.

## Link following

- [ ] Cmd/Ctrl-click and F12 on an `xref:`, `include::` and `{attribute}` open the target and place the caret.
- [ ] Cmd/Ctrl-click on an `image::` or attachment opens it in the system's default application.
- [ ] Go Back and Go Forward (title-bar arrows, Ctrl-minus, Ctrl-Shift-minus) move through the history.
- [ ] Alt-F12 on an `include::` of a partial shows it read-only; Open opens it in a tab.
- [ ] An unresolvable target shows the diagnostic and does not navigate.

## Find, replace and rename

- [ ] Cmd/Ctrl-Shift-F finds across the workspace; regex, case and whole-word options apply; clicking a result opens it.
- [ ] Replace All shows a preview; Apply changes every listed file; one undo restores them all.
- [ ] F2 renames the active page; the preview lists the move and every xref edit; Apply moves the file and rewrites the xrefs; one undo moves it back and restores every xref.

## Window, theme and settings

- [ ] Light and dark themes (Cmd/Ctrl-K Cmd/Ctrl-T); `editor.theme` in settings is honoured.
- [ ] Open Settings opens the user `adocfoundry.toml`; saving it applies the change; an unknown key shows a warning once.
- [ ] Cmd/Ctrl-Shift-P lists every command with its title.
- [ ] Cmd/Ctrl-K Cmd/Ctrl-N opens a read-only second window that follows edits.
- [ ] Window resize and HiDPI scaling render correctly.

## Packages

- [ ] The package installs and launches with no Ruby, Node or Java on the machine.
- [ ] `adoc-ls --version` and `adocfoundry-cli --version` run from the package.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
| 2026-10-23 | `phase-1` (release) | Linux x86_64, KDE Plasma Wayland | Partial | Typing benchmark passed on the 5,000-line document (as smooth as the control; see `docs/qa/spike-gpui.md`). Navigation tree, highlighting, diagnostics, outline and status bar checked visually on a real five-module component. Link following, undo routing, IME composition (input-handler level), second window and search panel are covered by the app's test-platform tests. The rest of this list is still to do by hand. |
