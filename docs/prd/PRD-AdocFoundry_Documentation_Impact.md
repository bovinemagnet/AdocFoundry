# PRD: AdocFoundry — Documentation Impact Analysis

| Field | Value |
| --- | --- |
| Status | Proposed / draft — not approved for implementation |
| Date | 2026-11-03 |
| Scope | Antora-aware dependency and change impact analysis across local checkouts, CLI and pull-request workflows |
| Related work | #100 dependency and impact inspector; #101 semantic component comparison; #287 graph path; #289 page age; #90 relative references after move |
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

## 7. Proposed headless contracts

Rust (illustrative):

```rust
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
    "via":["docs/modules/ROOT/partials/auth.adoc","docs/modules/ROOT/pages/install.adoc"]
  }]
}
```

The exact schema and path normalisation must be agreed in WP I1; this is only representative.

## 8. Architecture

| Layer | Responsibility |
| --- | --- |
| `adoc-git` (L2) | Git ref/working tree file-change metadata; no network transports, no GitHub calls |
| `adoc-index`, `adoc-lang`, `adoc-antora` (L1) | Existing context-aware dependency resolution and reverse mapping, source spans, resource identities |
| `adoc-graph` (L1) | Generic keyed graph structures/path walks only; keep independent of AsciiDoc semantics |
| `adoc-engine` (L3) | Orchestrate typed impact rules, report scope, version filtering, confidence/completeness, worker caching |
| `adocfoundry` (L4) | Impact inspection, graph highlighting, drill-down, diff preview |
| `adocfoundry-cli` / `adoc-mcp` (L4) | Headless reporting, bounded output; optional read-only MCP tool |
| CI/GitHub Action | External integration consuming CLI JSON, subject to user approval and explicit permissions |

`cargo xtask layers` enforces dependencies. Avoid a new crate until boundaries justify it. Because the graph stores merged edges without the complete source provenance, verify whether the existing index and `LinkStore` can supply evidence; extend the data model for source evidence if needed rather than reconstructing links with a second parser.

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

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers`; include large-fixture budget tests before UI integration.

## 12. Work packages and existing issues

| WP | Deliverable | Recommended issue |
| --- | --- | --- |
| I1 | Typed dependency and impact semantics, evidence, completeness and report schema | New issue; coordinate with #100 |
| I2 | Git base/head, worktree/explicit changed-resource input | New issue |
| I3 | Incremental reverse dependency traversal and publication impacts | New issue |
| I4 | CLI text/JSON reporting, deterministic snapshots and CI summary | New issue |
| I5 | GUI inspector/graph provenance and end-to-end QA | New issue; extend #100 |

Order I1 → I2/I3 → I4/I5. Semantic diff #101 remains a distinct follow-on feature; do not fold it into the initial impact analysis milestone. Changes to an impacted document should never be auto-written by this feature.

## 13. Definition of done

Given a local Antora fixture with a changed shared partial, an altered xref target and a modified nav file, the engine and CLI report distinct and accurate render/reference/navigation impacts, each with provenance and version identity; partial coverage is explicit; the app can navigate to those sources; the analysis requires no network or checkout mutation and passes CI and performance tests.

## 14. Open decisions before approval

- Scope of image/attachment dependency edges in the first work package.
- Whether comparison uses only Git refs or also unsaved buffers in the GUI's initial release.
- Whether the first CI output is Markdown + JSON or includes a GitHub Actions example workflow.
- Path and evidence representation for renamed pages and component versions absent from head.
- How far to extend #100 versus treating its existing inspector as a prerequisite.

## 15. Implementation notes

No implementation exists yet under this specification. Existing #100/#101/#287/#289 should not be duplicated or closed by creation of the planning documents.
