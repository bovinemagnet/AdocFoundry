# Phase 2 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Phase 2 acceptance (spec §8 and §11) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ (AppImage) before a release. Record the build, the platform and the result in the table at the end.

Automated coverage for reference: the engine's preview tests (`crates/adoc-engine/tests/preview.rs`: settle, included-file changes, stale results, diagram republishing), the view tree and HTML tests (`crates/adoc-render/tests/view.rs`), the DOM tests over both parsers (`crates/adoc-dom/tests/`), the diagram tests (`crates/adoc-diagrams`), the CLI `render` and fidelity tests (`crates/adocfoundry-cli/tests/cli.rs` and `fidelity.rs`), and the app's test-platform tests (`crates/adocfoundry/src/ui_tests.rs`) for rendering the active document, click-to-source, opening a partial from the preview and the detached window following edits.

## Setup

1. `git submodule update --init`, then `cargo run --release -p adocfoundry -- .` from the repository root, and open `src/docs/modules/developer/pages/architecture.adoc` (Mermaid diagrams, partial-free) and `tests/fixtures/render/blocks.adoc` (every block kind, with a partial).
2. For a real component, open a project with partials and images (Antora's own documentation, `https://gitlab.com/antora/antora`, `docs/`, will do).
3. For the fidelity harness, install Asciidoctor (`gem install asciidoctor`) and run `cargo test -p adocfoundry-cli --test fidelity -- --nocapture`.

## Rendering

- [ ] Every page of the real component renders with no block shown as raw source. STEM is the exception: it shows its maths as styled source by design.
- [ ] Headings, lists (nested, description, callout, checklist), admonitions, tables (spans, header and footer, AsciiDoc cells), quotes, verses, sidebars, examples and footnotes look right in light and dark themes.
- [ ] `[source,rust]` and other source blocks are highlighted with the same colours as the editor.
- [ ] Images from `image::` and `image$` resolve, including from a module's `assets/images`; a missing image shows "Missing image: <target>".
- [ ] An unresolved xref is drawn with a wavy underline; clicking it says the reference does not resolve.
- [ ] Mermaid diagrams appear with no browser, Node.js or Java on the machine, and their labels are drawn.
- [ ] With `dot` on `PATH`, a `[graphviz]` block renders; without it, the block shows its source and why.
- [ ] A rendered diagram appears without delay when the page is reopened (served from `.cache/adocfoundry/diagrams`).

## Following and editing

- [ ] Toggle Preview (Cmd/Ctrl-K V) opens the docked preview to the right of the editor and closes it again.
- [ ] Switching tabs changes the previewed page; Pin keeps it on one page until Unpin.
- [ ] Each page's preview scroll position is kept when you switch away and back.
- [ ] Typing updates the preview shortly after you stop, without flashing the old text.
- [ ] Editing an open partial updates every previewed page that includes it, before saving.
- [ ] Typing steadily in a long page with diagrams stays smooth (no dropped frames while the preview re-renders).

## Scroll sync and clicking

- [ ] Moving the caret scrolls the preview to the block containing it.
- [ ] Scrolling the preview moves the source to the topmost visible block.
- [ ] "Sync off" stops both; "Sync on" restores them.
- [ ] Clicking a paragraph, a table cell and a list item places the caret on its source line.
- [ ] Clicking content that came from a partial opens the partial in a tab at the right line.
- [ ] Clicking an xref opens the target page; Go Back returns.
- [ ] Clicking an external link opens the browser; clicking an attachment link opens the default application.

## Detached window

- [ ] Detach moves the preview into its own window, which follows the active page and edits.
- [ ] Closing the detached window docks the preview again; Dock does the same.
- [ ] The detached window can sit on a second monitor at a different scale factor and renders sharply.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
| 2026-10-23 | `phase-2` (debug CLI) | Linux x86_64 | Partial | `adocfoundry-cli render` over every page of two real components: this repository's `src/docs` (17 pages, 9 Mermaid diagrams, all rendered as SVG) and a 34-page component with 15 shared partials, modules using `assets/images`, and 234 images. No block fell back to raw source, every image resolved to an existing file, and every partial's text appeared on each page that includes it. The fidelity harness skipped itself: the installed Asciidoctor.js CLI fails to start under the installed Node.js. The app checks above are still to do by hand. |
