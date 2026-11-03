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
- Unresolved navigation entries: a statically specified nav target that fails Antora resolution is already reported as `adoc.antora.nav-unresolved` (and a missing nav file as `adoc.antora.missing-nav-file`). The audit aggregates those codes under the navigation category; it does not define a second `quality.*` rule for the same fault.
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

- Should unlisted pages be warnings by default, or informational in all new workspaces?
- How are publication-only nav variations represented? (Remote playbook sources are `remote-component-missing`, §5.4.)
- Does SARIF belong in the initial release or the immediate following work package?

## 13. Implementation notes

No implementation exists yet under this specification. Existing repository issues mentioned above remain independent and must not be silently marked completed by creating this PRD.
