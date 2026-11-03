# PRD: AdocFoundry — Documentation Impact Analysis

| Field | Value |
| --- | --- |
| Status | Approved for implementation, 2026-11-03; the open decisions in §14 are settled in the work package they affect |
| Date | 2026-11-03 |
| Scope | Antora-aware dependency and change impact analysis across local checkouts, CLI and pull-request workflows |
| Related work | Epic #306; #100 dependency and impact inspector; #101 semantic component comparison; #287 graph path; #289 page age; #90 relative references after move |
| Architecture | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `CLAUDE.md`; `adoc-index`, `adoc-lang`, `adoc-graph`, `adoc-git` |

## 1. Purpose and problem

AdocFoundry already resolves xrefs, includes, navigation and Antora version coordinates, and represents their edges in a link graph. The graph UI and Find References help authors explore links, but they do not provide a *change-oriented* answer: when a page, shared partial, asset or navigation resource changes, which published pages or outputs might need review, and why? Reviewers currently rely on text diffs and individual file inspection.

Provide explainable, conservative impact analysis for local changes or Git ref comparisons, with reliable source evidence, a deterministic headless report and optional CI/PR reporting.

## 2. Goals

1. For changed resources, identify direct and transitive consumers using typed dependency edges and meaningful impact categories.
2. Identify effects on Antora pages, nav trees, component versions and configured PDF profiles when evidence exists.
3. Explain each result with at least one dependency path and change origin; support inspection in the desktop graph.
4. Compare Git base/head (or working tree) without mutating the checkout or requiring Git network transports in the engine.
5. Expose a CLI JSON/text report suitable for CI, with bounded size and explicit incomplete/unknown status.
6. Share a stable engine service with the existing #100 inspector and, later, #101 semantic comparison.
7. Avoid false certainty when remote components, dynamic attributes, include tags or build-time extensions are not analysed.

## 3. Non-goals

- Automatically determine the semantic meaning or truth of edited prose.
- Automatically update impacted pages, write PR comments from the engine, or perform GitHub API calls from lower crates.
- Reimplement complete Git merge/checkout logic or an Antora production build.
- Assume every transitive xref is a render-time dependency; model **readability/navigation reachability** separately from **content inclusion**.
- Claim a PDF publication is affected unless its content selection is known or report the impact as tentative.

## 4. User workflows

**Before editing/moving/deleting:** right-click a page/partial and choose Inspect Impact; see direct incoming references, transitive include consumers, nav visibility and named export profiles (existing #100 covers the baseline inspector).

**After a local change:** view an Impact tab for the Git diff against `HEAD` or working tree; list changed resources, affected pages/versions/publications with why, and drill down to the linking source.

**Pull request reviewer:** run a headless command against Git refs to generate JSON/Markdown output; optionally attach it to CI or use a separate GitHub Action to publish a comment. Check whether changed partials influence multiple release versions or publication profiles.

## 5. Dependency and impact semantics

### 5.1 Edge definitions

| Edge | Interpretation | Default impact policy |
| --- | --- | --- |
| `include` file → partial/example/file | Rendered content composition; dependent parent may change when target changes | Propagate transitively from changed target to all include consumers |
| `xref` source → page/anchor | Discoverability and link relationship; editing target prose does **not** change source render | Report direct referencing pages as *reference review* for target move/remove, alias/id changes; do not propagate ordinary prose changes to all xref consumers |
| `nav` navigation → entry/page and hierarchical links | Site navigation/readability; nav changes affect component navigation | Report navigation membership and component version, not document text dependency |
| Image/resource dependency | Image macro or asset reference, if resolvable | A changed asset may alter pages rendering it; only when the resource is indexed/known |
| PDF profile → selected page/component/nav | Publication selection | Include profile when selected content intersects affected pages; mark uncertain where config/context is incomplete |

The existing `adoc-graph` has `Xref`, `Include`, `Nav` and page/partial/nav/unresolved nodes. **Do not overload it to imply render dependencies**: adapt or extend a typed per-file dependency view in the engine/`adoc-lang` and preserve link source locations, path direction and edge meaning. Reuse `adoc-index` reverse lookups (`xrefs_naming`, `references_with_fragment`, includes) and existing graph store where possible.

### 5.2 Change types

Support added, modified, removed and renamed paths. Detect renames from Git data where reasonably reliable; report both origin and destination when uncertain. Changed partial contents cause transitive render impacts; changed anchors, page moves or removals trigger xref integrity review; changed `nav.adoc` or `antora.yml` triggers scoped structural impacts; changed normal page prose impacts that page and selected publications but not unrelated xref sources. Attribute declarations and conditional content are **context-sensitive**: calculate only verifiable consumers, and mark unknown contexts.

### 5.3 Completeness and evidence

Every affected item records `impact_kind`, `confidence` (`confirmed` or `potential`), changed-resource origin, affected Antora coordinate, explanation path, and source spans if available. A report has an overall `complete`, `partial`, `unavailable` status with reasons (e.g., remote component not loaded, refs absent, dynamic include). Cap path depth and result count with explicit truncation markers, not silent omission. Distinguish direct/transitive and render/reference/navigation/publication impacts.

### 5.4 Avoid misleading results

- A changed page *referenced* by 1,000 other pages is not 1,000 changed rendered pages.
- An unversioned ROOT module must not be accidentally collapsed with a versioned module.
- Graph paths displayed in #287 are user-navigation links, not necessarily dependency provenance; use a separate semantic impact traversal.
- A partial with multiple include contexts may expand differently depending on attribute values; mark potential when contexts differ or are unknown.
- Branch coverage is limited by local checkouts and playbook/catalog knowledge. Do not pretend to have analysed remote source repositories.

## 6. Git comparison inputs

Proposed modes:

1. `--against HEAD` compares unsaved/open buffer + working tree to `HEAD` where available, documenting whether the command sees unsaved buffers (CLI never does; app does).
2. `--base <git-ref> --head <git-ref>` compares refs using local Git object data and a base/head file list. No checkout, git executable or remote fetch in `adoc-git`.
3. Explicit changed-resource list for tools/CI without a Git repository.

Resolve Git refs with clear typed errors. Report files absent from the available catalog as unanalysed, not unaffected. The working tree might be dirty: CLI ref-to-ref results must ignore it unless explicitly requested.

### 6.1 Dependencies from both revisions

A comparison of two revisions uses dependency information from **both** of them. The workspace's live index describes only the working tree. Checking changed paths against it alone would miss dependencies that exist only in the base revision: the consumers of a removed partial, xrefs to a page's old location after a move, xrefs to an anchor that was removed, and nav entries removed from base.

- **Revision snapshot.** For each side of a comparison, the engine builds a `DependencySnapshot` from that revision's files. It holds the catalog (components, versions, modules, aliases), the effective nav trees, the typed dependency edges of §5.1 with their source spans, and each page's anchors.
  - **Git refs:** both snapshots come from Git object data at the resolved commits. The engine lists each commit's tree under the workspace roots, honouring `[workspace] ignore`, reads blob text, and feeds it to the same parsing and resolution code the live index uses: `parse_component_descriptor`, `parse_nav`, `WorkspaceIndex::index_file` and `adoc-lang`'s link resolution. No checkout and no second resolver.
  - **`--against HEAD`:** the base snapshot is `HEAD`'s; the head side is the live workspace (open buffers in the app, files on disk in the CLI).
  - **Explicit paths:** there is no base revision. The head side is the live workspace, and the report's completeness is `partial`, with reason `no-base-revision`, whenever a removed or renamed path is given, since its former consumers cannot be known.
- **Never substitute the live index for a ref.** In ref-to-ref mode the live index is not consulted. If a snapshot cannot be built, for example because a ref is missing or a descriptor in that revision is unreadable, the report is `partial` or `unavailable` with the reason. It does not quietly fall back to the working tree.
- **Rules over two snapshots** (applied by `adoc-impact`):
  - **Removed resource:** its consumers come from base. A consumer that still names it in head is a confirmed reference-integrity impact. One that no longer does is reported as resolved by the change.
  - **Renamed or moved resource:** consumers of the old path come from base, and consumers of the new path from head. A consumer that still names the old location in head is a confirmed reference-integrity impact, unless a head-side `page-aliases` entry covers it, in which case it is resolved through the alias.
  - **Modified resource:** render consumers are the union of both sides' include closures. Anchors present in base and absent from head make their xref consumers, from both sides, reference-review impacts.
  - **Added resource:** consumers come from head, including references that were unresolved in base and now resolve.
  - **Navigation and descriptors:** effective nav membership and component versions are compared between the snapshots, per component version.
- **Evidence side.** Every impact records which snapshot supplied its evidence path (`base`, `head` or `both`), so a reviewer can tell a dependency that was removed from one that was added.
- **Cost.** Building a snapshot traverses one revision's documentation files once. It is done only for an explicit comparison, on a worker, and cached by the commit's tree ID; it never runs per edit and never touches the live index. A large-fixture budget test covers it.

## 7. Proposed headless contracts

Rust (illustrative):

```rust
pub struct DependencySnapshot { /* catalog, nav trees, typed edges with spans, anchors; one revision */ }
pub enum EvidenceSide { Base, Head, Both }
pub enum ChangeSource { WorkingTree, GitRefs { base: String, head: String }, Paths(Vec<PathBuf>) }
pub enum ImpactKind { Render, ReferenceIntegrity, Navigation, Publication }
pub enum ImpactConfidence { Confirmed, Potential }
pub struct ImpactReport { /* schema_version, base/head, changes, impacts, paths, completeness */ }
impl Engine {
    pub fn analyse_impact(&self, source: &ChangeSource) -> Result<ImpactReport, ImpactError>;
}
```

CLI (proposed only):

```sh
adocfoundry-cli impact <root> --base main --head HEAD --format json
adocfoundry-cli impact <root> --against HEAD --format text
adocfoundry-cli impact <root> --changed-file docs/modules/ROOT/partials/auth.adoc --format json
```

Exit `0` for completed analysis, `2` for invalid/unavailable sources or incomplete analysis when `--require-complete` is given. **Do not fail CI merely because a document has many consumers**. Gating should be a separate explicit policy (e.g., unresolved high-severity links introduced by change, handled by the audit feature). Standard output must contain only the selected report format.

### Illustrative JSON contract

```json
{
  "schema_version": 1,
  "scope": {"kind":"git-refs","base":"main","head":"HEAD"},
  "completeness": "complete",
  "changed": [{"path":"docs/modules/ROOT/partials/auth.adoc","kind":"modified"}],
  "impacts": [{
    "resource":"product:ROOT:install.adoc",
    "kind":"render",
    "confidence":"confirmed",
    "evidence":"head",
    "via":["docs/modules/ROOT/partials/auth.adoc","docs/modules/ROOT/pages/install.adoc"]
  }]
}
```

The exact schema and path normalisation must be agreed in WP I1; this is only representative.

## 8. Architecture

| Layer | Responsibility |
| --- | --- |
| `adoc-git` (L2) | Resolve a ref to a commit, list a commit's tree, read a blob's text at a commit, and diff two trees with rename detection (it has `file_at` for a branch tip and `head_content` today); read-only, no network transports, no GitHub calls |
| `adoc-index`, `adoc-lang`, `adoc-antora` (L1) | Existing context-aware dependency resolution and reverse mapping, source spans, resource identities. `adoc-antora` gains discovery from a list of files and their text, beside today's discovery from disk (`discover_antora_workspace`), sharing one set of rules so a revision snapshot resolves exactly as the live workspace does |
| `adoc-graph` (L1) | Generic keyed graph structures/path walks only; keep independent of AsciiDoc semantics |
| `adoc-impact` (L2, new) | Typed impact rules, reverse traversal over typed dependency edges, provenance paths, confidence, completeness, version and profile scoping, bounded truncation and the report schema; pure, synchronous functions over the changes and the base and head `DependencySnapshot`s (§6.1) |
| `adoc-engine` (L3) | Turn Git refs, the working tree or explicit paths into the changes and the two snapshots (Git only behind its `git` feature), build snapshots from Git objects or the live workspace, run `adoc-impact` on workers, cache snapshots by tree ID and results by index/catalog revision, publish events |
| `adocfoundry` (L4) | Impact inspection, graph highlighting, drill-down, diff preview |
| `adocfoundry-cli` / `adoc-mcp` (L4) | Headless reporting, bounded output; optional read-only MCP tool |
| CI/GitHub Action | External integration consuming CLI JSON, subject to user approval and explicit permissions |

`cargo xtask layers` enforces dependencies. `adoc-impact` is created in I1, so the boundary between impact rules and orchestration is enforced from the first package rather than extracted later. It sits at L2 and may depend on `adoc-core`, `adoc-antora`, `adoc-index`, `adoc-lang` and `adoc-graph` only. It never touches Git: it takes the changes (added, modified, removed, renamed with origin and destination) and the two snapshots as data, so it needs no same-layer pair with `adoc-git`, and the engine without `git` can still analyse explicit paths. No `adoc-engine`, worker, LSP or UI types, threads, filesystem writes, subprocesses or network. It needs entries in `LAYERS` in `xtask/src/layers.rs`, `[workspace.dependencies]` and, once it holds code, the crate ladder in `CLAUDE.md`. Front ends, `adoc-ls` included, reach it only through `adoc-engine`; I1 decides whether the engine names it behind an optional feature, as with the quality audit's `adoc-audit`.

Rename and move planning (`rename_preview` in the engine's `search.rs`, `links.rs`, and anchor renames in `adoc-lang`'s `references.rs`) is not moved by this PRD. A separate `adoc-refactor` crate is deferred until a feature needs edits planned without an engine, such as #100's preview before a move or delete, or a move-page or extract-partial command; that feature's PRD then carries the extraction as a work package.

Because the graph stores merged edges without the complete source provenance, verify whether the existing index and `LinkStore` can supply evidence; extend the data model for source evidence if needed rather than reconstructing links with a second parser.

Build impact from changed resource IDs and reverse edges; avoid scanning all files on every request or editor settle. Cache by index/catalog revision and Git ref SHA where sensible. Impact work runs on workers, not GUI `Render`. Establish explicit output caps and budget tests for large repositories.

## 9. UI and automation

UI: `Inspect Impact` on a page/partial; a comparison selector for working tree or two available refs; grouped results (render, references, navigation, publications), sortable by component/version; highlight provenance paths in the existing graph; click to open source. Reuse #100 rather than adding a competing inspector. Do not call an impact report a graph shortest path.

CI: a generated Markdown summary/JSON artifact in a GitHub Actions job can be a first integration. Creating or updating PR comments is a later opt-in GitHub-specific adapter with minimal permissions, explicit repo/PR identity, size limits and idempotent update behaviour. Never write comments automatically from `adoc-engine`.

## 10. Performance, security and reliability

- No network or Git process required for core analysis; Git integration must be optional where feature flags disable it.
- Ref comparisons are read-only and do not checkout branches or modify the working tree.
- Avoid recursive cycles for partials (visited set with deterministic traversal); bound depth and count.
- Paths restricted to configured roots for MCP; avoid leaking absolute local paths or file contents into public CI output.
- Never claim impact coverage over remote Antora content not present locally.
- Recompute only affected reverse traversals; preserve the live editor's keystroke frame budget.

## 11. Tests and acceptance criteria

1. `impact_partial_propagates_transitively`: nested partial included from two pages produces two render impacts with paths.
2. `impact_page_prose_does_not_propagate_xrefs`: changing prose never treats all referencing pages as changed output.
3. `impact_removed_anchor_flags_reference_review`: source ranges for xrefs are identified and explained.
4. `impact_nav_change_is_version_scoped`: navigation changes reported only in corresponding component/version context.
5. `impact_profiles_only_when_selected`: PDF profile intersection is accurate; unknown selection marked potential.
6. `impact_handles_cycles_and_truncation`: deterministic, bounded output with explicit truncation.
7. `impact_git_refs_are_read_only`: fixture repo with changed, moved, removed files; working tree remains unchanged.
8. `impact_missing_remote_components_reports_partial`: no invented unaffected status.
9. `impact_report_is_deterministic`: ordered JSON/text snapshots on Linux/macOS/Windows fixture.
10. `impact_ui_drilldown_and_graph_path`: GPUI headless test; no render-time scans.
11. `impact_cli_handles_no_git`: explicit-path mode works without Git; ref mode errors appropriately.
12. `impact_removed_partial_consumers_come_from_base`: a partial removed between base and head reports the pages that included it in base, although none exists in head's index.
13. `impact_moved_page_finds_base_references`: xrefs to a page's old location, found in base, are confirmed reference-integrity impacts when head still names the old location, and resolved when a head `page-aliases` entry covers them.
14. `impact_ref_mode_ignores_live_index`: with a dirty working tree and an open edited buffer, a ref-to-ref report is identical to the one from a clean checkout.
15. `impact_explicit_removed_path_is_partial`: a removed path in explicit-path mode gives `partial` with `no-base-revision`.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers`; include large-fixture budget tests before UI integration.

## 12. Work packages and existing issues

| WP | Deliverable | Issue |
| --- | --- | --- |
| I1 | New `adoc-impact` crate: typed dependency and impact semantics, evidence, completeness and report schema | #307; coordinate with #100 |
| I2 | Git base/head, worktree/explicit changed-resource input, and revision snapshots from Git objects | #308 |
| I3 | Reverse dependency traversal over base and head snapshots, and publication impacts | #309 |
| I4 | CLI text/JSON reporting, deterministic snapshots and CI summary | #310 |
| I5 | GUI inspector/graph provenance and end-to-end QA | #311; extends #100 |

Order I1 → I2/I3 → I4/I5. Semantic diff #101 remains a distinct follow-on feature; do not fold it into the initial impact analysis milestone. Changes to an impacted document should never be auto-written by this feature.

## 13. Definition of done

Given a local Antora fixture with a changed shared partial, an altered xref target and a modified nav file, the engine and CLI report distinct and accurate render/reference/navigation impacts, each with provenance and version identity; partial coverage is explicit; the app can navigate to those sources; the analysis requires no network or checkout mutation and passes CI and performance tests.

## 14. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's PR and in the Implementation Notes.

- Scope of image/attachment dependency edges in the first work package.
- Whether comparison uses only Git refs or also unsaved buffers in the GUI's initial release.
- Whether the first CI output is Markdown + JSON or includes a GitHub Actions example workflow.
- Path representation for renamed pages and component versions absent from head (evidence sides are settled in §6.1).
- How far to extend #100 versus treating its existing inspector as a prerequisite.

## 15. Implementation notes

### I1 and I3 (#307, #309)

- **One PR for both.** The rules over two snapshots are the model: a `DependencySnapshot` and the base/head evidence sides (I1) are only meaningful with the rules that read them (I3), so `adoc-impact` lands with both. The Git-built snapshots (I2) and the engine's orchestration follow.
- **Snapshot = a `WorkspaceState`.** A `DependencySnapshot` is one revision's index and catalog, borrowed for the live workspace or owned for a revision; the typed edges of §5.1 are not copied out into a second store but asked of the index's reverse maps and `adoc-lang`'s resolution, so evidence (source spans, written targets) is the index's own and nothing is resolved a second way or scanned whole.
- **Decision (§14, first item): image and attachment edges are not in this package.** The index has no reverse map of images, and adding one is index work beyond these packages; a changed asset gives the gap `asset-consumers-not-analysed`, never a claim that nothing is affected.
- **Navigation referrers.** An xref in a navigation file `antora.yml` lists is a navigation impact, not a reference-integrity one, so a moved page's broken entry is reported under navigation, with its status.
- **Added files** report referrers that now resolve (`now_resolves`); with no base they are `potential`.
- **Not in a catalog.** A changed file in no component version either side holds gives `not-in-catalog`: what uses it from a component this workspace does not have cannot be known (§5.4, remote components).
- **Limits.** `Limits { depth: 32, impacts: 1000 }` by default; counts are before the cut.
- **Engine.** The engine depends on `adoc-impact` unconditionally and re-exports it (`adoc_engine::impact`); it is in `ENGINE_ONLY`, so no front end names it. Without the engine's `git` feature, explicit paths remain analysable. Existing #100/#101/#287/#289 should not be duplicated or closed by creation of the planning documents.
