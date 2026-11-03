# New Antora Project — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | The New Antora Project epic (#291) where automated tests cannot reach: the platform's folder picker, the dialog and welcome as drawn, and real folders |

Run each item on macOS 13+, Windows 10+ and Ubuntu 22.04+, from a build of `main` after #294, unless the item names a platform. Record the build, the platform and the result in the table at the end.

## Automated coverage, for reference

- **Engine:** `crates/adoc-engine/tests/new_project.rs` (layout, descriptor read back, subfolder of a code repository, refusals, field problems, the dialog's starting request, the welcome, Git, undo, resolution, a folder outside the workspace) and `scaffold_failure_undoes_itself` in `src/new_project.rs`.
- **Command line and MCP:** `new_project_scaffolds_a_component_that_checks_clean` in `crates/adocfoundry-cli/tests/cli.rs`; `mcp_create_project_reuses_engine_command` in `crates/adoc-mcp/tests/creation.rs`.
- **App:** `empty_folder_shows_the_welcome_panel`, `non_antora_pages_folder_offers_a_project`, `new_project_dialog_validates_live_and_previews_the_tree`, `new_project_opens_index_with_preview_and_trees` and `new_project_toast_undo_removes_the_files` in `crates/adocfoundry/src/ui_tests.rs`, on GPUI's test platform.

## Checks

1. **Welcome, empty folder.** Open an empty folder. The editor area says "This folder has no documentation yet", with New Antora Project…, Open a Different Folder… and up to five recent folders; a recent folder opens when clicked.
2. **Welcome, pages without Antora.** Open a folder holding `.adoc` files and no `antora.yml`. The welcome says "These pages are not in an Antora component"; New Antora Project… starts with "Create inside a subfolder" ticked and `docs` given.
3. **Welcome, no folder and after Close All.** Start with no folder: "Open a folder to begin". In a folder without a component, open a page and close every tab: the welcome comes back.
4. **Choose….** In the dialog, Choose… opens the platform's folder picker; the folder picked fills Location, and the file list and problems follow it.
5. **Live checks.** Type a name with a capital or a space, a version with a space or `/`, a subfolder that exists, a folder inside an existing component, and a folder that is not empty without a subfolder: each problem shows under its own field, Create and Enter do nothing, and clearing the problem brings the file list back.
6. **Git checkbox.** Point Location at a folder inside a repository: "Initialise a Git repository" is not shown, and no repository is made inside it. Outside any repository it is shown and ticked; Create makes `.git` and commits nothing (the Changes panel lists the new files).
7. **After Create.** `index.adoc` opens with the preview docked and rendering it, the Files panel shows it, Navigation lists its one entry, and a notification says "Created <name> with N files." with Undo. `adocfoundry-cli check <folder>` reports nothing.
8. **Undo.** Undo in the notification removes every file still as it was made and the folders left empty; a file edited after Create is kept; a `.git` made is kept. The welcome returns.
9. **A folder outside the workspace.** With a component open, make a project in another folder: it joins the workspace as a second root, and its pages resolve.
10. **Antora builds it.** On a machine with Node.js and network access, in the folder holding `antora-playbook.yml`: `npm install && npm run build` writes `build/site/index.html` with no `WARN` in the log, before any commit, for a project in an empty folder and for one in `docs/` of a code repository. Without a repository (Git left out), the build stops saying a local content source must be a Git repository, as the playbook's comment warns. `new_project_writes_a_playbook_that_antora_accepts` runs this when `antora` is on `PATH`.
11. **Keyboard.** Tab moves through the fields in order, Escape cancels, and Enter creates only when nothing is wrong.

## Results

| Build | Platform | Items | Result | Notes |
|---|---|---|---|---|
| | | | | |
