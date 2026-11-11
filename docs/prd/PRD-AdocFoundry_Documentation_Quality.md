# PRD: AdocFoundry — Documentation Quality

| Field | Value |
| --- | --- |
| Status | Approved for implementation, 2026-11-03; the open decisions in §12 are settled in the work package they affect |
| Date | 2026-11-03 |
| Scope | Antora-aware quality audits, health report, configurable CI gates and desktop dashboard |
| Related work | Epic #299; #85 Vale, #86 external links, #104 compatibility inspector, #289 stale graph pages, #291 new Antora project |
| Architecture | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `CLAUDE.md`; crate layering in `xtask/src/layers.rs` |

## 1. Purpose and problem

AdocFoundry detects malformed and unresolved references, exposes a workspace-wide Problems panel, understands Antora components/modules/versions, and maintains an incremental graph of xrefs, includes and navigation. It does not yet assemble these findings into a deliberate, reproducible documentation quality assessment with Antora-aware policies, thresholds, audit reporting and release gates. Documentation owners need to know whether a documentation set is publishable, what is missing from navigation, and what has regressed since a baseline.

This feature provides a **deterministic quality audit** backed by existing resolution and indexing code and surfaced consistently in the GUI, CLI, and optionally MCP/LSP. It must not claim to measure writing quality objectively.

## 2. Goals

1. Report actionable Antora-specific quality findings by location, affected resource and stable rule ID.
2. Distinguish component versions and the *effective* navigation of each published component version.
3. Reuse existing parsers, diagnostics and graph/index data, with incremental or revision-cached evaluation after the first audit.
4. Supply deterministic JSON, human-readable text and SARIF 2.1.0 for CI integration.
5. Offer a useful quality dashboard in GPUI, with drill-down into the source and quick fixes when safe.
6. Support project-configurable rule severity, exclusions and threshold-based CI failure.
7. Remain usable offline; external link checking and Vale integration are separate optional providers.

## 3. Non-goals

- Reimplement Antora, Vale, Asciidoctor, browser crawling, or a site publisher.
- Treat all pages outside nav as defective: internal/unlisted pages and intentionally excluded content are legitimate.
- Automatically rewrite author documents, replace the existing Problems panel or establish a universal numeric writing-quality score.
- Check remote playbook repositories absent from the indexed checkout as though their content were available.
- Run network requests implicitly during editing or CI.

## 4. User workflows

### 4.1 Author

Open the Quality panel; see findings grouped by component version, module, rule and severity; select a finding to open the exact source range. Suppress a known exception with a narrowly scoped configuration entry and a documented reason.

### 4.2 Documentation owner

Audit a whole workspace or a selected component/version; inspect navigation coverage and unresolved references; export a machine-readable report; optionally fail CI on new errors or policy violations. Present trend comparisons only when an explicit baseline file is provided.

### 4.3 Release reviewer

Run an offline quality gate against the publication target, identify unresolved resources and pages unlisted in the selected navigation, and see whether the report is complete, partial or unavailable because the component is not checked out.

## 5. Rules and semantics

### 5.1 Audit structure

A finding has stable `rule_id`, severity (`info`, `warning`, `error`), message, normalised source URI, **byte-offset** `SourceRange` when available, Antora coordinate (component/version/module/family/path) when resolvable, evidence and optional suggested remediation. The report includes schema version, scope, index/catalog revision, rule set, counts and completeness metadata. Deterministically sort by coordinate/path, range, rule ID and message. Report generation must not mutate files.

A rule identifier is permanent once released; version the schema if its meaning materially changes. Reuse existing `DiagnosticCode` values and outputs as inputs; do not fork resolution logic or generate duplicate copies of the same diagnostic.

### 5.2 Initially supported rules

- `quality.navigation.unlisted-page`: an indexed Antora **page** absent from *all effective navigation trees* of the relevant component/version; exclude partials, examples, images, attachments, nav files, and pages explicitly designated unlisted by policy. Do not confuse an unreferenced page with an unlisted page.
- Unresolved navigation entries: a statically specified nav target that fails Antora resolution is already reported as the navigation file's own `adoc.unresolved-xref-file` (and a missing nav file as `adoc.antora.missing-nav-file`). The audit aggregates those codes; it does not define a second `quality.*` rule for the same fault. (Corrected in Q4: this first named `adoc.antora.nav-unresolved`, which the navigation trees produce and which restates the file diagnostic.)
- `quality.page.missing-title`: a statically inspectable page lacks a usable document title; configurable as warning. Only report when current parser provides sufficient evidence.
- `quality.image.missing-alt`: a statically inspectable image macro lacks meaningful alternative text when the macro form requires an author-provided description; handle attributes and dynamic macros conservatively, with a documented heuristic. Decorative images and generated diagrams need an explicit policy/exception.
- Existing unresolved xref/include/image/anchor and descriptor diagnostics: aggregate existing codes into the audit without changing their resolver semantics.

**Not part of initial release:** external URL validation (#86), Vale prose linting (#85), last-modified age (#289), and renderer/theme compatibility (#104). Design provider interfaces so those can contribute to a future unified report, without importing their implementation in the first work package.

### 5.3 Antora-specific correctness

Scopes: workspace; component; component version. Respect ROOT module defaulting, versionless components, page aliases, explicit version references, multiple nav files and nested nav lists. Read navigation from the catalog's effective configuration, not only `modules/ROOT/nav.adoc`. A page appearing in several nav trees counts once. Links to a target via alias resolve to the canonical page but may separately receive the existing alias-use advisory. A remote component missing from the local catalog yields a **partial coverage** status, not a false error. Dynamic includes/attributes and conditionals cannot be asserted without a known attribute context; skip them rather than invent errors, and count each skipped construct in the report (see §5.4).

If an Antora playbook is absent, report local catalog coverage and identify the scope as checkout-level, not verified published-site coverage. If a named authoring profile is used in future (#102), attach profile identity to each report.

### 5.4 Completeness and the CI policy

Completeness describes **coverage**: whether every resource in the requested scope was analysed by every enabled rule and provider. A report is:

- `complete`: every in-scope resource was analysed by every enabled rule and provider.
- `partial`: some in-scope coverage is missing. Each gap is listed with a stable reason code and the resources or components it affects. The initial reasons are `remote-component-missing` (a playbook source or referenced component is not in the local catalog), `scope-not-indexed` (an in-scope folder could not be read or indexing was cut short) and `provider-unavailable` (an enabled optional provider, §5.2, could not run). New reasons are added with the schema version.
- `unavailable`: nothing in the requested scope could be analysed, for example a component or version that is not checked out.

A construct a rule cannot assert, such as a dynamic include, an unknown attribute or a conditional, is a **skip**, not a coverage gap. Skips are counted per rule and listed with their locations, but they do not make a report partial. Otherwise almost every real workspace would be partial, and strict mode would be unusable.

The CI policy is explicit, and a partial result is never presented as an unqualified pass:

| Result | Default | Strict (`--require-complete` or `require_complete = true`) |
| --- | --- | --- |
| `complete` | Exit 0, or 1 if findings meet `fail_on` | Same |
| `partial` | Exit 0 or 1 by `fail_on`; reported as passing *with partial coverage*, with its reasons | Exit 2 |
| `unavailable` | Exit 2 | Exit 2 |
| Invalid configuration or failed execution | Exit 2 | Exit 2 |

Exit 2 takes precedence over 1, and 1 over 0. The report is always written in full before the process exits, including when it exits 2. The text summary's first line states the completeness. JSON has a top-level `completeness` object with its reasons. In SARIF, each gap is a `toolExecutionNotifications` entry of the run's invocation, and the completeness is recorded in the run's `properties`. The documented CI example uses strict mode. Default mode suits local runs and workspaces with known remote sources.

### 5.5 Configuration (proposed contract)

```toml
[quality]
enabled = true
fail_on = "error"                       # none | warning | error
require_complete = false                # true: anything short of complete coverage exits 2 (§5.4)

[quality.rules]
"quality.navigation.unlisted-page" = "warning"
"quality.page.missing-title" = "warning"
"quality.image.missing-alt" = "warning"

[[quality.exclusions]]
rule = "quality.navigation.unlisted-page"
resource = "product:ROOT:internal-notes.adoc"
reason = "Deliberately unlisted staff-only page"
```

The exclusion syntax is proposed, not a pre-existing settings interface. Validate unknown rule IDs, invalid resource coordinates and duplicate conflicting exclusions. Do not silently suppress `error` level build failures. Exclusions should be audit-visible, reviewable and capable of targeting individual resources; a project-wide glob is a later enhancement subject to abuse/misconfiguration review.

## 6. Architecture and implementation strategy

Respect the crate ladder in `CLAUDE.md` and `cargo xtask layers`. Suggested responsibility split:

| Layer | Responsibility |
| --- | --- |
| `adoc-core` | Only add stable value types if demonstrably reused; no UI, policy or transport types |
| `adoc-antora`, `adoc-index`, `adoc-lang` | Expose existing resolved nav and diagnostic facts; no independent second resolver or full scans on every edit |
| `adoc-audit` (L2, new) | The report and finding model, rule registry, rules, policy and exclusion validation, deterministic ordering, and the JSON and SARIF documents; pure, synchronous functions over borrowed catalog, nav, index and diagnostic data |
| `adoc-engine` | Own audit orchestration: workers, settings, revisions, cached per-file/per-component rule results, invalidation and events; calls `adoc-audit`, never re-implements a rule |
| `adocfoundry-cli` | `audit` subcommand, formatting, exit codes and report writing |
| `adocfoundry` | Quality panel, filtering, drill-down and status; never compute audit on UI render |
| `adoc-mcp` | Optional read-only audit tool after the headless API is stable; follow existing root safety bounds |

`adoc-audit` is created in Q1, so the boundary between rules and orchestration is enforced by `cargo xtask layers` from the first package rather than extracted later. It sits at L2 and may depend on `adoc-core`, `adoc-antora`, `adoc-index` and `adoc-lang` only: no `adoc-engine`, `adoc-git`, worker, settings-file, LSP or UI types, no threads, no filesystem writes, no subprocesses and no network. It needs entries in `LAYERS` in `xtask/src/layers.rs`, `[workspace.dependencies]` and, once it holds code, the crate ladder in `CLAUDE.md`. Front ends (`adocfoundry`, `adocfoundry-cli`, `adoc-ls`, `adoc-mcp`) reach it only through `adoc-engine`.

`adoc-ls` gets the crate through the engine. Q1 decides whether the engine names it unconditionally or behind an optional feature, as `typst` and `git` are; if a feature, `cargo xtask layers` checks the default `adoc-ls` build leaves it out. Either way the engine starts incremental audit upkeep only after the first audit request or when `[quality] enabled` asks for it, so a language server whose client never asks for an audit does no audit work while typing.

Never add a feature that scans every document after each keypress; reuse the index's file contributions, revisions and incremental updates. A complete first audit may traverse the index on a worker, then track deltas.

### Proposed conceptual Rust API (not an implementation guarantee)

```rust
pub struct QualityAudit { /* schema version, scope, findings, totals, completeness */ }
pub enum AuditScope { Workspace, Component { name: String, version: Option<String> } }
pub struct AuditOptions { /* rules, exclusions, profile */ }
impl Engine {
    pub fn quality_audit(&self, scope: &AuditScope, options: &AuditOptions)
        -> Result<QualityAudit, QualityError>;
}
```

The exact API should account for async worker execution and revision staleness, rather than synchronously blocking the UI.

## 7. CLI, MCP and UI

**Proposed command** (not implemented):

```sh
adocfoundry-cli audit <root> [--component <name>] [--version <version>] \
  [--format text|json|sarif] [--output <path>] [--fail-on none|warning|error] \
  [--require-complete]
```

Exit codes follow the CI policy in §5.4. `--require-complete` overrides `[quality] require_complete`. Ensure `--format json` and `sarif` write a single parseable document without mixing progress output; use stderr for progress/errors. Document paths and secrets must not leak from diagnostic output unintentionally; SARIF must use valid URIs and 1-based locations. Do not claim SARIF is validated until fixture-driven validation runs.

GPUI: an overview, plus the existing Problems panel for individual findings, so there are never two competing lists of problems:

- **Quality overview** (a sidebar tab, `SidebarTab::Quality`). It shows the audit status (complete, partial or unavailable, with the reasons, and a stale marker when the index has moved on since the last result), counts by severity and rule, navigation coverage per component version as listed pages out of pages (with denominators), skip counts, and exclusions with their reasons. It also has the action that runs an audit. It lists no individual findings: every count opens Problems filtered to that rule, severity or component version.
- **Problems** lists every finding. The engine's `Problem` gains the finding's origin, either an existing `DiagnosticCode` or an audit rule ID, so `quality.*` rule IDs never become `adoc-core` diagnostic codes. Problems gains filters by severity, code or rule, and component, version and module, which the overview's links set. Only `quality.*` findings are added; the existing diagnostics the audit aggregates are already in Problems and are not listed twice. Excluded findings are not listed; the overview counts them.

As with Problems today (`Engine::enable_problems`), only a front end that asks for audit results pays for them. Score is intentionally omitted from v1; present factual counts and coverage ratios with denominators.

MCP can expose `quality_audit` read-only with bounded pagination/size; no network checking implicit in tool calls. LSP can continue using its existing diagnostics and need not duplicate the dashboard protocol.

## 8. Performance, reliability and security

- Initial assessment is worker-based; UI remains responsive.
- On changed file, invalidate only that file's findings and affected nav/descriptor context. A catalog-shape change may invalidate component-wide rules; make scope and cost observable.
- Incremental assessment must not read every page from disk for each edit. If a rule requires an uncached full check, defer to an explicit audit action.
- Keep rule outputs stable under reordered file-system enumeration and across OS path separators.
- No filesystem mutation, telemetry, network request or external subprocess by the core audit.
- Do not assume that ignored folders, dynamic Antora sources or remote components are fully indexed.

## 9. Acceptance tests

1. `audit_unlisted_page_detects_only_page_family`: multi-component fixture, partials/examples/assets omitted.
2. `audit_navigation_union_deduplicates_multiple_navs`: one page in two nav files counted once.
3. `audit_alias_and_versionless_resources`: aliases and versionless version resolved correctly.
4. `audit_dynamic_content_is_conservative`: unknown attributes/conditionals do not produce fabricated errors.
5. `audit_reuses_existing_diagnostics`: no duplicate xref errors and stable source spans.
6. `audit_changes_are_incremental`: edit one page; verify bounded invalidation and no full index scan in steady state.
7. `audit_json_and_sarif_are_deterministic`: fixed fixture and snapshot; parseable schema, valid locations.
8. `audit_cli_exit_codes`: below/at threshold, invalid config, unavailable scope.
9. `audit_cli_strict_mode_fails_incomplete`: a partial report exits 0 or 1 by default, 2 with `--require-complete`, and is written in full either way.
10. `audit_unavailable_never_passes`: an unavailable scope exits 2 with or without strict mode.
11. `audit_skips_do_not_make_partial`: a dynamic include is counted as a skip, and the report stays complete.
12. `audit_panel_opens_exact_source_location`: headless GPUI test platform; no display dependency.
13. `audit_findings_listed_once_in_problems`: `quality.*` findings appear in Problems; aggregated existing diagnostics are not listed twice.
14. `audit_overview_counts_filter_problems`: selecting a count in the overview filters Problems to exactly those findings.
15. `audit_is_offline`: no external subprocess/network during core audit.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers`; add budget benchmarks if audits affect the editing path.

## 10. Work packages and existing issues

| WP | Deliverable | Issue |
| --- | --- | --- |
| Q1 | New `adoc-audit` crate: data model, rules registry, severity and config contract | #300 |
| Q2 | Antora navigation coverage + title/alt rules | #301 |
| Q3 | Incremental audit engine and completeness semantics | #302 |
| Q4 | CLI text/JSON/SARIF output + CI exit gates | #303 |
| Q5 | Quality overview, and audit findings and filters in Problems | #304 |
| Q6 | Optional integration adapters, docs and end-to-end QA | #305; coordinate with #85, #86, #104, #289 |

Implement in order Q1 → Q2/Q3 → Q4/Q5 → Q6. Prefer one reviewable PR per work package. Update this PRD in the same PR when implementation diverges; add an Implementation Notes section and preserve decisions.

## 11. Definition of done

An Antora multi-component fixture can be audited offline from CLI and GUI with matching findings; rules and exclusions are deterministic; navigation coverage is correct; CLI/SARIF support gating; existing diagnostics are reused; no new GUI or indexing latency regressions; documentation and tests are updated.

## 12. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's PR and in the Implementation Notes.

- How are publication-only nav variations represented? (Remote playbook sources are `remote-component-missing`, §5.4.)

## 13. Implementation notes

Existing repository issues mentioned above remain independent and must not be silently marked completed by this PRD.

### Q1 (#300): the audit model, rules and workspace policy

- **The crate.** `adoc-audit` (L2) depends on `adoc-core`, `adoc-antora` and `serde` only. `adoc-index` and `adoc-lang` are added in Q2, when its rules first use them. `adoc-engine` names the crate unconditionally, with no optional feature, since it brings in no new external dependency and does no work until asked. The engine re-exports it as `adoc_engine::audit`, and `ENGINE_ONLY` in `xtask/src/layers.rs` refuses a front end that names it directly.
- **`adoc-core`** gained `DiagnosticCode::ALL` and `DiagnosticCode::from_name`, so a report can read existing codes back.
- **Existing diagnostics.** A finding for an existing diagnostic carries its `DiagnosticCode`, severity and message unchanged. `[quality.rules]` cannot change such a severity (a setting that tries is warned about), but an exclusion can name the code for one resource.
- **Severities.** `[quality.rules]` accepts `off` as well as `info`, `warning` and `error`; a rule turned off contributes no findings or skips. All three registered rules default to `warning`. Whether unlisted pages should default to `info` (§12) is still open and is settled in Q2.
- **Exclusions.**
  - The resource ID must name its component and module. With no version it covers every version, `~@` covers only the versionless component, and the family defaults to `page`.
  - A finding with no coordinate is never excluded.
  - A second exclusion for the same code and resource is refused, and an exclusion with no reason is refused.
  - Excluded findings stay in the report with their reason and are counted, but never fail a gate.
- **Layers.** `[quality]` may be set in the user file and the workspace file. A rule's severity from the workspace file wins, and the exclusions of both files apply.
- **`[quality] enabled`** defaults to `false`: the audit runs only when asked for until a workspace turns upkeep on (§6).
- **Report.**
  - Paths are relative to a workspace root with `/` separators, and roots appear by their display names, never as absolute paths.
  - `AuditReport` is built by `AuditBuilder::finish`, which applies the policy, sorts deterministically and keeps identical findings once.
  - The CI table of §5.4 is `AuditReport::verdict` (`Verdict::exit_code` gives 0, 1 or 2), so the CLI and MCP share one decision.
- **Docs.** The user documentation of `[quality]` waits until an audit can be run (Q4 and Q6).

### Q2 (#301): navigation coverage and the initial content rules

- **Entry point.** `adoc_audit::audit(&AuditInput, AuditScope) -> AuditBuilder`. The input is the `WorkspaceState`, the normalised roots, and a reader for navigation files, so unsaved buffers can stand in for files on disk. It reads every in-scope file once: a whole audit, meant for a worker. Q3 makes it incremental. `adoc-audit` now depends on `adoc-index` and `adoc-lang` as well.
- **Existing diagnostics** are every in-scope file's `adoc_lang::diagnostics` (what the Problems panel shows), and each `antora.yml`'s, with their codes, severities, messages and ranges unchanged. A broken descriptor is reported on each file of its component, as in Problems.
- **No absolute paths.** A message's paths beneath a workspace root are written relative to it with `/` separators, so no absolute path leaves the machine in a report; `adoc.antora.invalid-descriptor`'s message names the descriptor by its full path. Likewise a navigation file that cannot be read is named relative to its root, and the reader's error message is left out, since a front end's reader may put a path in it.
- **Navigation diagnostics (corrected in Q4).** Q2 added the navigation trees' `adoc.antora.nav-unresolved` diagnostics on the grounds that no file diagnostic reported them. That was wrong: a navigation file is indexed and diagnosed like any file, so each unresolved entry is already its `adoc.unresolved-xref-file`, and Q2's report listed the fault twice. Q4 leaves the trees' diagnostics out. An entry naming an attribute gets no diagnostic, since `adoc-lang` stays silent on dynamic targets.
- **Unlisted pages.**
  - A published page is listed when an entry of one of its own component version's navigation files reaches it, including through a page alias. An entry in another component's navigation does not list it (§5.2: "the relevant component/version").
  - The start page (`start_page`, else `ROOT:index.adoc`) counts as listed.
  - A published page is an `.adoc` file in a `pages` folder whose name, and the folders on whose path, do not begin with `_` or `.`. The real corpus has `.gitkeep`, `.md`, `.html` and `.pdf` files there, which Antora does not publish as pages.
  - A version that lists no navigation files has nothing to be listed in, and its pages are not reported.
  - When a version's navigation has an `include::` line (which the navigation parser does not follow), an `{attribute}` target, or a file that cannot be read, its unlisted pages are skips.
- **Default severity.** Unlisted pages default to `warning`, settling the open decision in §12. On the 533-page real corpus, 309 pages are unlisted, and a sample of them appear in no navigation file. `[quality.rules]` sets the rule to `info` or `off` for a workspace that lists pages sparingly.
- **Coverage.** Each version's `NavCoverage` counts its navigation files, published pages, listed pages (each once, however many entries reach it) and undetermined pages, for the overview's ratios (§7).
- **Missing title.** Checked on published pages. An empty `=` line is reported, and no title at all is reported unless a `doctitle` attribute gives one. An include before the first section may supply the title, so that case is a skip.
- **Missing alt text.**
  - Checked on the block image macros of published pages and of partials. The parser records block images only, so inline images are not checked; recording them would change `adoc-parser`, which runs on every keystroke and feeds the editor's own image diagnostics.
  - Since #428 the parser records inline images as well (`ImageDirective::inline`), so inline images are checked too, their remedy written as `image:target[What it shows]`.
  - Text counts as written in any form: a first positional attribute, a named `alt`, an attribute reference, or text quoted empty (`[""]`), the author's sign of a decorative image. Only a macro with no text at all is reported. Other decorative or generated images are left to exclusions.
- **Unavailable scope.** A component or version not in the catalog makes the audit unavailable (`remote-component-missing`).

### Q3 (#302): the incremental audit in the engine

- **Parts.** `adoc-audit` now builds an audit from parts that never depend on the scope:
  - `audit_file` gives a file's existing diagnostics and content findings (`FileAudit`).
  - `audit_version` gives a component version's navigation diagnostics, unlisted pages and coverage (`VersionAudit`).
  - `assemble` puts the parts for a scope together.
  - `audit` computes the parts in scope and assembles them. The engine keeps the parts and assembles them on request. Both go through `assemble`, so `Engine::audit_report` (kept) and `Engine::audit_now` (computed there and then, for the CLI in Q4) give the same report, which `tests/audit.rs` checks after every kind of change.
- **Unlisted-page locations.** An unlisted-page finding no longer points at the page's title range. A version's part then depends only on its navigation files' text and the catalog, so a page edit, which changes neither, can never leave it stale.
- **Upkeep** starts with `Engine::enable_audit` or `[quality] enabled`. Until then the engine does no audit work, so `adoc-ls` pays nothing by default. The first build runs on a worker over a copy of the workspace, reading open documents from a copy of their buffers.
- **Invalidation** follows the problems, to keep the audit's existing diagnostics agreeing with the Problems panel:
  - An edit audits again, at the next `pump`, the files `adoc_lang::diagnosed_with` gives.
  - A version is audited again only when the text of one of its own navigation files changed. A nav file that merely names the edited page is not enough, since its part does not depend on the page.
  - A new catalog revision audits every version's navigation again, which reads only the navigation files.
  - A workspace-wide change builds it all again on a worker: indexing, files changed on disk, a rename or replace, closing a page, a restored session, a version change. That is every caller of `refresh_open_diagnostics` but one: opening a page, which re-diagnoses the open documents but changes no parse, leaves the audit alone.
- **Measured, not timed.** The probes `AuditFile` and `AuditVersion` count the parts audited. `audit_changes_are_incremental` checks that one page edit audits between one and four files, no version, and no whole build. `a_navigation_edit_audits_its_own_version_again` checks that a nav edit audits exactly one version. These hold however fast the machine is, which a wall-clock budget would not.

### Q4 (#303): the command-line audit and CI gates

- **SARIF is in Q4,** settling the open decision in §12, since the report already carries everything it needs. `adoc_audit::to_sarif` writes SARIF 2.1.0:
  - A finding is a result; an excluded finding is a result with an `external` suppression giving its reason.
  - Gaps are tool execution notifications, and completeness, counts and coverage are run properties.
  - Locations are relative to the root (`uriBaseId` `ROOT<n>`), with no absolute `originalUriBaseIds`.
  - Regions are 1-based lines and character columns (`columnKind` `unicodeCodePoints`) from `Region::of`, given by the caller, which reads the files. A finding without a range (an unlisted page, a missing title) names only its file.
  - It is checked structurally (relative URIs, 1-based regions, rule IDs known, levels valid) and by snapshot, not yet against the SARIF schema.
- **Scope** is `--component [<version>@]<name>`, as `pdf --component` takes it, rather than the separate `--version` that §7 proposed.
- **Policy.** It comes from the workspace's own `.adocfoundry.toml` and never from the user file, so CI gets the same result on every machine. `--fail-on` and `--require-complete` override it at audit time (`Engine::audit_now_with`), because opening a workspace reads its settings file again.
- **Invalid configuration.** Any problem with the `[quality]` settings (`QualitySettings::problems`) makes the command exit 2 after writing the report.
- **Coverage gaps.**
  - Folders the index could not read are now kept (`Engine::unreadable`) and become `scope-not-indexed` gaps in every audit whose scope holds them, which is what makes an audit partial in practice.
  - `remote-component-missing` from the playbook's content sources is not detected yet. Telling a source that is not checked out from one that is needs the playbook's URLs matched against local repositories, and that is left to a later package. Until then a workspace whose playbook names other repositories audits as complete over what it has.
- **Output.**
  - The text format states completeness on its first line and the result on its last.
  - A finding's message is joined onto one line in the text format and kept whole in JSON and SARIF.
  - JSON is the report itself. `--output` writes to a file, and standard output then holds nothing.
- **Snapshots.** The JSON and SARIF snapshots hold placeholders for the index and catalog revisions and the tool version, which change without the audit changing.

### Q5 (#304): the Quality overview, and audit findings and filters in Problems

- **When the app audits.** The app keeps the audit only once asked: Run Audit (`quality.run_audit`, in the overview, File › Run Quality Audit and the palette), or `[quality] enabled = true` from the start. Running it again builds the whole audit afresh. Before then the overview says no audit has run, and Problems holds only diagnostics, so opening a workspace costs nothing extra.
- **Origin.** `Problem::code` became a `FindingCode` (an existing `DiagnosticCode` or a `quality.*` rule ID), rather than a second field beside a `DiagnosticCode`. `Problem::whole_file` marks a finding with no range (an unlisted page, a missing title), shown without a line and column and opened at the file's start.
- **Listed once, under the policy.** Only `quality.*` findings are added. They come from the kept parts through `adoc_audit::Policy::apply`, the function `AuditBuilder::finish` now uses, so a problem has the report's severity, and excluded findings and rules turned off are not listed. Since only rule severities are configurable, an aggregated diagnostic's severity is the same in the overview and in Problems, so a count by severity filters Problems to exactly the findings counted.
- **Cost.** Each finding's line and column are worked out when its part is audited (on the worker for a whole build), so listing reads no file. An edit updates only the files whose parts changed and the files a re-audited version's findings name. Rebuilding every finding cost about 10 ms per edit in a component of 2,000 pages; per file it is about 55 µs (the `perf` probe `audit_upkeep_in_a_component_of_2000_pages`). The overview is built from `audit_report` only while it is shown, and otherwise marked outdated until it is next shown.
- **Stale marker.** `Engine::audit_stale` is true while a build or a change waits to be audited after a first result. It shows mostly during a whole build, since an edit's update arrives at the next pump.
- **Filters.** `ProblemFilter` (severity, code, component, version, module), `Engine::filtered_problems` and `Engine::problem_place` are in the engine. The filter itself is view state in the app, set from the filter bar or by the overview's links. A component descriptor belongs to its component version and to no module, and a module chosen without a component version matches that module in any.
- **Links from the overview.** A severity or rule count filters by that severity or code. A component version's coverage filters to its unlisted pages (that version and `quality.navigation.unlisted-page`), and is a link only when it has some. Skips and exclusions are counted, and each exclusion is shown with its reason, but they are not links, since Problems lists neither.
- **Tests.** `audit_findings_listed_once_in_problems`, `excluded_findings_are_not_problems`, `quality_problems_follow_edits`, `problems_filter_by_severity_code_and_place` and `run_audit_builds_the_audit_then_builds_it_again` are in the engine's `tests/audit.rs`. `audit_panel_opens_exact_source_location`, `audit_overview_counts_filter_problems` and `problem_filter_choices_clear_one_by_one` run on GPUI's test platform. The Filter ▾ menu's items are not driven by a test (gpui-component's popup menu gives its items no selectors); they set the same filter the tested paths do.

### Q6 (#305): optional providers, documentation and end-to-end QA

- **Providers are an interface, not implementations.** `adoc-audit`'s `providers.rs` registers the four planned providers, with stable names, what each needs and the issue it is tracked in: `compatibility` (#104), `external-links` (#86), `page-age` (#289) and `vale` (#85). Those issues stay as they are, and none is implemented here.
  - `[quality] providers = [...]` enables providers by name. An unknown name is a `[quality]` problem, and both settings files' providers apply.
  - Whoever runs a provider reports it to the builder with `provider_ran` or `provider_unavailable(reason)`. `finish` gives every registered provider a `ProviderStatus`: `disabled`, `ran`, or `unavailable` with its reason.
  - An enabled provider that nothing ran is unavailable, giving a `provider-unavailable` gap (§5.4). The report is then partial: it passes by default, exits 2 in strict mode, and leaves the core findings unchanged.
  - The states appear in JSON (`providers`), in SARIF's run properties, as a `Provider <name>:` line in the text format, and under Optional providers in the Quality panel.
- **Schema.** `providers` is a new top-level field, added to an unreleased schema, so `SCHEMA_VERSION` stays 1. The three snapshots changed only by this field.
- **Offline, checked.** `cargo xtask layers` now checks that `adoc-audit`'s sources name no file, process, network or thread API (`ADOC_AUDIT_FORBIDDEN_PATHS`), with the test `audit_is_offline` (PRD test 15). A provider's work runs where its needs are allowed: a tool on an engine worker, Git history through `adoc-git`, and the network only in `adocfoundry`, which hands results to the engine. The developer guide describes that route.
- **End to end.** `cli_and_app_report_the_same_audit` audits a copy of the multi-component fixture under a workspace policy that has a severity override, an exclusion and an enabled provider. It checks that the command line's JSON equals the report the engine keeps, as the app keeps it, apart from the revisions. It also checks that every `quality.*` finding the text format prints is the problem the app lists, at the same line and column.
- **Real corpus.** On the 533-page real corpus, `adocfoundry-cli audit` takes 2.1 s including indexing and finds 426 warnings, with complete coverage and nothing written. The manual checks of the app, the code-scanning upload and validation against the SARIF schema are listed in `docs/qa/quality.md` and have not been run.
- **Documentation.** The user guide's new *The quality audit* page covers the rules, completeness, the policy, exclusions, CI, the providers and the scope limitations. The limitations are:
  - remote sources are not detected;
  - navigation is read as written, so a nav file's includes and attribute entries are skipped rather than expanded;
  - inline images are not checked (lifted by #428);
  - ignored folders are not gaps;
  - existing diagnostics keep their severity;
  - there is no score.

### Review fixes (#543–#548)

Fixes from the review of `crates/adoc-audit` on 11 November 2026.

- **One unreadable navigation file (#543).** `adoc_antora::nav_tree_each` reads each navigation file of a version on its own. A file that cannot be read no longer drops the trees of the others: the pages they list count as listed, and only the pages none of them lists are skips.
- **Alternative text on the block attribute line (#544).** The parser records, for a block image, the block attribute lines above it (`ImageDirective::block_attributes`; they reach past blank lines, titles and comments, as Asciidoctor's block metadata does). A named `alt` or a first positional attribute there counts as written; a line holding only an id, role or option shorthand gives none.
- **Titles from includes (#545).** The parser marks an include that no body content precedes and that is in no verbatim block (`IncludeDirective::before_body`). Only such an include may supply a title, so an include in a listing block, or after the body has begun, leaves the finding. An empty `=` line is no title: the parser reads no heading from it, so the branch reporting an empty title, which could never run, is gone.
- **Navigation syntax (#546).** The navigation parser reads `-` items as list items of the first level and the `<<page.adoc#fragment,text>>` shorthand as an entry's target, as Asciidoctor and so Antora do.
- **Distributed component versions (#547).** The catalog keeps every `antora.yml` of a component version (`AntoraCatalog::descriptors_of`, `all_descriptors`, in root order); `component` is still the last one inserted. Navigation trees, the audit's coverage, the graph's navigation files and the engine's lookups read the navigation of each, each against its own root, and reloading one descriptor removes only its own folder's modules and resources. Antora says only one descriptor should list `nav`; reading all of them avoids reporting a page as unlisted when more than one does.
- **Smaller findings (#548).** A navigation entry keeps its resource's family, and only a page counts as listed (the PDF export's navigation and New Page's entry lookup take pages only too). The navigation tree records the `include::` lines and attribute entries it cannot follow (`NavTree::unfollowed`), skipping comments and delimited blocks as the entries do, so the audit no longer reads each navigation file a second time. `AuditScope::covers` reads a version `~` as the versionless version, for the CLI's `--component '~@name'` and the engine's gaps. An unavailable audit is an unsuccessful SARIF run. Images in hidden pages are checked, as partials' are. A root in a message is rewritten only where it ends a path. `adoc_audit::locate` and `file_at` are the one mapping between files and report locations, which the engine uses too. An unlisted page's evidence is built once per version, roots are taken as given, and `audit` filters its files once.
- **Left open from #548.** A title written `# Title` is not read as a title: the line parser reads `=` headings only, and changing that is the parser's concern. Findings in navigation files and `antora.yml` still cannot be excluded; the user documentation now says so.
