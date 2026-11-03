# Impact Inspector — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | The app's impact inspector (#311, Impact 5/5) and Delete File (#100) where automated tests cannot reach: real repositories, the panel and graph window as drawn, and timing |

Run each item on macOS 13+, Windows 10+ and Ubuntu 22.04+, from a build of `main` after #311, on a clone of a documentation repository with several components and versions (and on `tests/fixtures/impact`). Record the build, the platform and the result in the table at the end.

## Automated coverage, for reference

- **Engine:** `crates/adoc-engine/tests/impact_inspection.rs` (a worker, out of date, Inspect Impact of a page, graph steps, a newer request, failures) and `tests/impact.rs` (the comparisons themselves).
- **App:** `impact_ui_drilldown_and_graph_path` and `delete_file_lists_what_breaks_and_undo_puts_it_back` in `crates/adocfoundry/src/ui_tests.rs`.
- **Delete File (#100):** `crates/adoc-engine/tests/deletion.rs`.
- **Rules:** `crates/adoc-impact/tests/impact.rs`.

## Checks

1. **Inspect Impact of a partial.** Right-click a partial included by several pages (a tab, or the Files panel) and choose Inspect Impact. The Impact tab opens, says "Analysing…" briefly, then lists every page that includes it (transitively) under Rendered content, each with "Render dependency: … is in …". Nothing on disk changes.
2. **Inspect Impact of a page.** Choose Edit › Inspect Impact on a page other pages link to: References to review lists each linking page as broken; clicking one opens the page at the xref. Navigation lists the navigation file that lists it.
3. **Changes since HEAD.** Edit a shared partial without saving, then Git › Analyse Impact of Changes: the source line says "unsaved edits included", and the pages that include the partial are listed.
4. **Compare Revisions.** Git › Compare Revisions… with a branch and `HEAD` (or two tags): the results match `adocfoundry-cli impact <root> --base <branch> --head HEAD`. The working tree, the checked-out branch and `git status` are unchanged.
5. **Out of date.** After a result, type in any page: the panel says "Out of date"; Again brings a fresh result and the note goes.
6. **Partial and failures.** In a folder with no repository, Analyse Impact of Changes says it could not analyse, naming Git. A comparison with an unknown ref names it. Change an image and analyse: the result is partial, with the reason that image consumers are not analysed.
7. **Filter.** On a result spanning two component versions, the filter buttons show one version at a time, and All shows every result again.
8. **Graph.** Choose Graph on a render result: the graph window opens with the path numbered, titled "Render dependency", the steps listed as "by include, followed back"; on a reference result, "Reader path". Clear removes it; Show Path (Shift-click) afterwards shows a shortest path titled "Path".
9. **Responsiveness.** On a workspace of a few thousand pages, Compare Revisions on two distant tags: typing stays smooth while it runs, and a second request replaces the first.
10. **Delete File.** Right-click a partial included by several pages and choose Delete…: the confirmation says "Working out what depends on it…" briefly, then lists "Included in" each page; Cancel leaves it. Delete a page instead: the confirmation lists the pages linking to it and the navigation listing it; Delete closes its tab, those pages show unresolved references at once, and the Files panel no longer lists it. Undo in the notification, or Edit › Undo Delete, puts it back and reopens it, and the problems go. Delete an image: the confirmation says its users are not traced. On a workspace on another volume than the home folder, deleting and undoing still work.

## Results

| Build | Platform | Items | Result | Notes |
|---|---|---|---|---|
| | | | | |
