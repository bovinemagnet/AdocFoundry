# New Antora Project — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | The New Antora Project epic (#291) where automated tests cannot reach: the platform's folder picker, the dialog and welcome as drawn, and real folders |

Run each item on macOS 13+, Windows 10+ and Ubuntu 22.04+, from a build of `main` after #295, unless the item names a platform. Record the build, the platform and the result in the table at the end.

## Automated coverage, for reference

- **Engine:** `crates/adoc-engine/tests/new_project.rs` (layout, descriptor read back, subfolder of a code repository, refusals, field problems, the dialog's starting request, the welcome, Git, undo, resolution, a folder outside the workspace) and `scaffold_failure_undoes_itself` in `src/new_project.rs`.
- **Command line and MCP:** `new_project_scaffolds_a_component_that_checks_clean` in `crates/adocfoundry-cli/tests/cli.rs`; `mcp_create_project_reuses_engine_command` in `crates/adoc-mcp/tests/creation.rs`.
- **Templates:** `crates/adoc-engine/tests/templates.rs` (every built-in checks clean, single page, the user's and a workspace's folders listed after the built-ins, a template kept inside the project) and the unit tests in `src/templates.rs`; `new_project_from_a_template_checks_clean` (CLI), `mcp_create_project_from_a_template` (MCP), `template_picker_hides_unused_fields` (app).
- **Build Site:** `build_site_disabled_without_node`, `build_site_needs_a_playbook` and `build_site_streams_output_and_times_out` in `crates/adoc-engine/tests/build_site.rs`; `build_site_panel_shows_the_log_and_opens_the_site` in `crates/adocfoundry/src/ui_tests.rs` and `build_site_says_it_needs_node_only_beside_a_playbook` in `src/menus.rs`.
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
10. **Build Site.** On a machine with Node.js, make a project inside a Git repository and choose File › Build Site: a dialog names the folder and the commands (`npm install`, `npm run build`) and warns that they run the project's code; Cancel runs nothing. Build: the Build tab opens with `$ npm install`, npm's output, `$ npm run build` and Antora's, as they arrive; it ends "Built." with Open Site, which opens `build/site/index.html` in the browser, and a notification says the site is built. Build again: it does not ask, and there is no `npm install` this time. In a new window on the same folder it asks again. Outside any repository, the build fails with Antora's message that a local content source must be a Git repository. With Node.js off `PATH` (or `[tools] node` set to a missing file) and the app restarted, Build Site is disabled with "Needs Node.js: install it, or set [tools] node" (or "No npm beside [tools] node") under it. On macOS, start the app from the Dock with `[tools] node` set to Homebrew's `node` and check the build finds `node`. Set `[tools] build_timeout = 2` and build afresh after removing `node_modules`: the build stops after 2 s and no `npm` or `node` process is left running (Activity Monitor, `ps`, Task Manager).
11. **Antora builds it.** On a machine with Node.js and network access, in the folder holding `antora-playbook.yml`: `npm install && npm run build` writes `build/site/index.html` with no `WARN` in the log, before any commit, for a project in an empty folder and for one in `docs/` of a code repository. Without a repository (Git left out), the build stops saying a local content source must be a Git repository, as the playbook's comment warns. `new_project_writes_a_playbook_that_antora_accepts` runs this when `antora` is on `PATH`.
12. **Templates.** In the dialog, the picker lists Minimal, Product documentation, Two components and Single page, each with its description; choosing one changes the file list, and Single page hides the name, version, README and playbook. Make one of each in an empty folder inside a Git repository: `adocfoundry-cli check` reports nothing, and Build Site builds Product documentation (five modules in Navigation, pagination at the foot of each page) and Two components (the guide's index as the site's start page). Reopen the Two components project from its parent folder: Build Site is still offered.
13. **Your own template.** Copy `crates/adoc-engine/templates/product-docs` into the `templates` folder beside the user settings file, rename it and change its `template.toml`'s `name`: reopen the dialog (no restart) and it is listed after the built-ins. Add a file named `{{name}}-notes.adoc` with `{{title}}` in it: the project gets `<name>-notes.adoc` with the title. A link in the template is left out of the project. A binary file, or a `template.toml` that does not read, keeps the template out of the picker, with "Not offered:" and the reason under it; a component `path = ".."` is refused under the picker when the template is chosen. Point a workspace's `.adocfoundry.toml` `[templates] dir` at a folder inside it: its templates follow the user's; a folder outside it is ignored with a settings warning.
14. **Keyboard.** Tab moves through the fields in order, Escape cancels, and Enter creates only when nothing is wrong.

## Results

| Build | Platform | Items | Result | Notes |
|---|---|---|---|---|
| | | | | |
