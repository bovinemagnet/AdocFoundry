# PRD: AdocFoundry — MCP Agent Workflows and Safe Automation

| Field | Value |
| --- | --- |
| Status | Approved for implementation, 2026-11-03; the open decisions in §14 are settled in the work package they affect |
| Date | 2026-11-03 |
| Scope | Expand `adoc-mcp` from a document-access interface into a safe, Antora-aware documentation engineering interface for MCP clients |
| Related work | Epic #313; Quality and Impact PRDs (`docs/prd/PRD-AdocFoundry_Documentation_Quality.md`, `docs/prd/PRD-AdocFoundry_Documentation_Impact.md`, merged in #298); [Quality epic #299](https://github.com/bovinemagnet/AdocFoundry/issues/299); [Impact epic #306](https://github.com/bovinemagnet/AdocFoundry/issues/306); [New Module #290](https://github.com/bovinemagnet/AdocFoundry/issues/290); [New Project #291](https://github.com/bovinemagnet/AdocFoundry/issues/291); [Dependency Inspector #100](https://github.com/bovinemagnet/AdocFoundry/issues/100) |
| Architecture | `CLAUDE.md`, `docs/design/DESIGN-AdocFoundry_Architecture.md`, `crates/adoc-mcp/`, and crate constraints checked by `cargo xtask layers` |
| Target protocol | Model Context Protocol as supported by the pinned `rmcp` (=3.7.0, protocol versions up to 2026-07-28); negotiate feature support rather than assuming every client implements the latest protocol |

## 1. Purpose and problem

AdocFoundry is a native, Antora-aware AsciiDoc editor with a shared headless engine, CLI, language server and MCP server. The existing `adoc-mcp` can read pages, list components and navigation, resolve resource targets, check diagnostics, search, render, inspect the link graph and Git status, edit existing pages, rename pages and export PDFs. It serves stdio or authenticated loopback HTTP and confines file access to configured workspace roots.

The server's principal limitation is that clients must still assemble many **Antora-specific workflows** themselves: discover where a new page belongs, create it without breaking navigation, understand transitive effects of changing a partial, distinguish an incomplete audit from a clean one, or protect against stale edits across simultaneous clients. A model that knows how to edit files is not necessarily an Antora-aware documentation tool.

**Product outcome:** an MCP client can discover, analyse, propose, validate and safely apply documentation changes using the same deterministic domain capabilities that the AdocFoundry app and CLI use. The MCP adapter remains thin; it does not reimplement AsciiDoc parsing, Git comparison, quality rules, link resolution or source refactoring.

## 2. Goals

1. Provide token-efficient, structured discovery of Antora resources, effective page context, page outlines and incoming references.
2. Expose the proposed Documentation Quality audit and Documentation Impact reports through read-only MCP tools, **only after their engine APIs are implemented**.
3. Offer domain-aware creation of pages, partials, modules and eventually projects by invoking established engine commands rather than writing arbitrary files in the MCP layer.
4. Allow clients to stage and inspect multi-file changes and apply them only against validated, unchanged document revisions.
5. Expose clear server-enforced permissions for reading, document changes, publication artifact writes and optional external command execution.
6. Deliver machine-readable output, stable error codes, pagination and explicit truncation/completeness semantics, with compatibility fallbacks for older clients.
7. Work reliably with multiple MCP sessions, long-running operations and concurrent edits to the same workspace.
8. Keep the service local-first, useful without an AI model in the process, and consistent with the CLI and GPUI front ends.

## 3. Non-goals

- Embedding a model, choosing an AI vendor, managing AI conversations or implementing autonomous background agents inside `adoc-mcp`.
- Replacing the Antora generator, adding a general-purpose shell tool, or automatically installing Node.js, Ruby or Java.
- Creating a general Git client, fetching remote refs or repositories by default, pushing commits or posting GitHub comments from the engine.
- Making every missing or dynamic Antora reference an error, or treating a documentation-quality score as objective truth.
- Automatically accepting agent-generated edits, bypassing user approval settings in MCP clients or allowing document instructions to override server permissions.
- Duplicating Quality #299, Impact #306, New Module #290 or New Project #291 in a parallel MCP implementation.
- Implementing remote hosted collaboration or multi-tenant public HTTP service; the current loopback-only model remains the supported transport.

## 4. Existing baseline (verified against repository on 2026-11-03)

### 4.1 Current MCP tools

| Category | Actual exported tool names | Notes |
| --- | --- | --- |
| Discovery | `list_components`, `get_nav`, `read_page` | `read_page` also returns title, attributes and include targets |
| Resolution and analysis | `resolve`, `check`, `symbols`, `search`, `render_page`, `link_graph`, `git_status` | `link_graph` supports workspace, neighbourhood and shortest reader path; JSON or SVG |
| Mutations | `edit_page`, `rename_page`, `export_pdf` | Only registered when `--allow-write` is supplied; `export_pdf` writes an artifact |
| Resources | `file://...` page resources, `antora:{resource_id}` template | Server advertises resources and tools; not MCP prompts or resource subscriptions |

The current `server.rs` returns JSON encoded as text content, limits array results to 500 items and text responses to 200,000 bytes, and lists all indexed resources in one response despite receiving pagination parameters. It uses a shared engine behind `Arc<Mutex<State>>`, with work dispatched through blocking workers. These are improvement opportunities, not reasons to replace the existing implementation.

### 4.2 Existing protections and compatibility

- stdio is the default transport; Streamable HTTP requires a bearer token and only listens on loopback addresses.
- `FileAccess::Within` and path confinement apply to engine reads as well as tool-supplied file paths; preserve containment and symlink checks for all new operations.
- `edit_page` accepts a unique `old_text` or byte range, and attempts to undo partial edits on error. It does **not** expose an expected buffer/file revision precondition.
- `rename_page` creates and applies an engine preview, then saves changed files; it currently provides no separate client-visible review stage.
- `--allow-write` is existing public behaviour; preserve it for existing tools, and document any more granular capabilities without silently broadening its meaning.

## 5. User workflows

### 5.1 Author a new page

An MCP client lists component versions and modules, retrieves nearby pages and navigation structure, prepares a new page in an existing module, previews the proposed page and navigation edits, then applies and validates the operation. The result includes the canonical resource ID and every changed file.

### 5.2 Review documentation quality

A documentation owner asks for an audit of `product@3.0`. The MCP tool returns rule findings, component-qualified locations, counts, exclusions and completeness. A dynamic include that cannot be asserted is a *skipped construct* as defined in the Quality PRD, not necessarily an incomplete audit. A missing remote component or unavailable required provider is reported as partial/unavailable coverage.

### 5.3 Inspect the impact of a change

A reviewer asks what changes when a shared partial is edited or when `main` is compared with `HEAD`. The MCP tool identifies render-time consumers, reference-integrity concerns, navigation/publication effects and evidence paths. For Git refs it uses dependency snapshots from **both revisions**; a missing old ref is never silently replaced with the live workspace index.

### 5.4 Safely edit across pages

An agent proposes a multi-file content change. The engine returns a preview with stable operation ID, before/after revisions, file list, diff, validation results and expiry. An explicit `apply_changes` call succeeds only while every relevant revision and path permission still matches; otherwise it fails with a conflict and no intended changes are committed.

### 5.5 Prepare a documentation release

A read-only tool combines the available quality audit, publication profiles and structural validation. An optional, separately permitted build can invoke the actual Antora toolchain, returning bounded logs and generated output paths. The server never claims that native page rendering is an identical substitute for a production Antora build.

## 6. Product decisions

| Decision | Requirement |
| --- | --- |
| Headless-first | Domain logic belongs in the existing engine or domain crates. MCP validates requests and serialises results. |
| No duplicate resolvers | Use `adoc-antora`, `adoc-index`, `adoc-lang`, `adoc-graph`, `adoc-audit` and `adoc-impact` as available. |
| Correctness over convenience | If context, source range or ref snapshot is unavailable, return `partial`/`unavailable` or a typed error; never invent a target. |
| Stable tool contracts | Add versioned fields; prefer additive changes. Do not rename or remove existing public tools in the first release. |
| Token efficiency | Paginate/filter lists. Distinguish compact metadata from full text. Never silently truncate findings used to support decisions. |
| Read-only default | Mutation and external process execution must be separately permitted and never inferred from textual instructions in documents. |
| Native Rust consistency | Keep `cargo xtask layers` passing; no UI or MCP types in domain crates. |
| No universal AI workflow assumptions | MCP tools are deterministic; optional prompts provide recipes, not embedded model logic. |
| Antora version semantics | Preserve component, versionless `~`, module, family, alias, branch and effective navigation behaviour. |
| Revision safety | A draft edit cannot be applied after underlying inputs change; stale plans return explicit conflicts. |

## 7. Proposed tool catalogue and contracts

Tools in this section are **proposed**, not available in the current binary. All new tool outputs include `schema_version`, `workspace_revision` where applicable, `completeness` when analysis coverage matters, and `truncated`/`next_cursor` when applicable. Tool argument schemas must reject ambiguous selection rather than guessing a component, version or root.

### 7.1 M1: Protocol quality and pagination (foundation)

Keep all existing tool names. Update response construction to provide MCP `structuredContent` plus JSON-schema output declarations **where supported by the pinned SDK and negotiated protocol version**. Keep existing text content as a backwards-compatible fallback. Add stable error codes: `invalid_argument`, `not_found`, `outside_workspace`, `permission_denied`, `conflict`, `stale_preview`, `index_incomplete`, `unavailable`, `timeout`, `result_too_large`, and `internal_error`.

Use cursor-based pagination for `list_resources`, `list_pages` and large diagnostic/reference reports. Cursors are opaque, scoped to the query and index revision, and rejected if invalid or stale; deterministic ordering uses canonical Antora coordinates and normalised paths. Keep total counts only where inexpensive and well defined. Preserve the existing response-size limits but replace lossy truncation in analytical tools with a continuation or a clear `incomplete` signal.

The pinned `rmcp` 3.7.0 provides what M1 needs: `CallToolResult::structured_content`, a tool's `output_schema` (filled by its `Json<T>` wrapper), `ToolAnnotations`, and `resources/subscribe` for M7. Contract tests pin the protocol version actually negotiated.

Tool annotations (read-only, destructive, idempotent and open-world/external) are metadata only: they never grant permissions. Detect which protocol capabilities clients actually negotiated; unsupported optional features degrade cleanly.

### 7.2 M2: Structured Antora discovery

| Tool | Input | Result | Reuse |
| --- | --- | --- | --- |
| `list_pages` | root/component/version/module/family filters, cursor, limit | Paged page metadata, resource ID, title, source path, nav membership, revision | Engine workspace catalog and index |
| `get_outline` | one path or resource ID; optional headings/anchors | Ordered section and anchor tree with byte ranges | `adoc-lang` document symbols |
| `get_page_context` | one page and include flags for nav/attributes/dependencies/consumers | Effective resource identity, context provenance where known, nav placement, resolved includes, incoming links, attribute values with unknowns | Existing page read, Antora resolver, graph/index, later #99 |
| `find_references` | resource ID or path; optional anchor/kinds; cursor | Exact incoming xref/include/nav references with source ranges, kind and resolution confidence | `adoc-lang` references and index reverse maps |

`get_page_context` complements `read_page`; it should not return full page text unless explicitly requested. If effective attribute provenance is not yet available from #99, the tool must indicate that the field is unsupported/unknown rather than presenting page-header attributes as the effective values. Do not infer inbound references from merged graph edges when source ranges are required; reuse the index's reference evidence.

A proposed `list_pages` request and abbreviated result:

```json
{
  "component": "product",
  "version": "3.0",
  "module": "ROOT",
  "family": "pages",
  "limit": 50
}
```

```json
{
  "schema_version": 1,
  "items": [
    {
      "resource_id": "3.0@product:ROOT:install.adoc",
      "path": "modules/ROOT/pages/install.adoc",
      "title": "Installation",
      "in_navigation": true
    }
  ],
  "next_cursor": null,
  "workspace_revision": 42
}
```

### 7.3 M3: Documentation Quality interface

**Tool:** `quality_audit` (read-only; depends on Quality epic #299, especially #300–#303).

Arguments: workspace/component/version scope; optional rule IDs, severity floor, include/exclude suppressed findings, page size and cursor; optional `require_complete` for tool-level policy. Responses expose the `adoc-audit` report schema directly (or a stable transport projection), with counts, rule metadata, Antora coordinates, byte ranges, evidence, exclusions, skipped constructs, completeness reasons and any continuation.

Rules remain in `adoc-audit`, orchestration in `adoc-engine`. Do not invoke Vale or external-link checks implicitly; provider execution has its own explicit policy. A `complete` report excludes arbitrary text-quality judgements. A partial report is not presented as an unqualified pass, regardless of whether the host client elects to proceed.

A `quality_audit` result must agree with `adocfoundry-cli audit` for an identical indexed snapshot and ruleset. Existing `check` remains available for lightweight diagnostics.

### 7.4 M4: Documentation Impact interface

**Tool:** `analyse_impact` (read-only; depends on Impact epic #306, especially #307–#310).

Input selector is *exactly one* of working-tree changes against `HEAD`, two local Git refs, or an explicit list of changed paths. Optionally select component/version scope, evidence verbosity, maximum depth and pagination. Return changed resources, impact kinds (`render`, `reference_integrity`, `navigation`, `publication`), confirmed/potential classifications, affected resource identities and provenance paths.

For two-ref mode, the engine must build or load typed dependency snapshots from both Git revisions, **without checking out or fetching** and without accidentally consulting the current working tree. Dynamic includes, absent remote Antora sources and unknown publication selections are transparently represented as uncertainty. Do not turn xrefs into render-time dependencies or use the graph's shortest reader path as the impact explanation.

Example request:

```json
{
  "source": {"type": "git_refs", "base": "main", "head": "HEAD"},
  "include_provenance": true,
  "limit": 100
}
```

MCP must return the same semantic result as the headless CLI for the same refs and workspace data. `analyse_impact` never changes the repository or publishes comments.

### 7.5 M5: Staged changes and optimistic concurrency

**Tools:** `preview_changes` and `apply_changes` (write capability required to apply; preview itself is read-only in effect but may reveal sensitive diffs).

The preview request supports a bounded list of typed operations (text replacements, new page/partial, rename, navigation update) and *expected revision hashes* for all edited resources. The engine validates candidate edits, computes file additions/deletions/updates, checks Antora resource validity and returns a diff and any reported diagnostics. The preview **does not write to disk or mutate live document state**.

The response contains `preview_id` (opaque and unguessable), `workspace_revision`, the set of expected file content hashes and paths, a sorted change list, validation findings, `can_apply` and `expires_at`. The server keeps preview state per session/principal, caps its count/bytes and expires it after a documented interval. It must not accept an arbitrary caller-supplied preview ID that references another session's plan.

`apply_changes` takes `preview_id`, a client-generated optional idempotency key and `save` preference. It rechecks workspace roots, source hashes, target existence/absence and permissions immediately before commit; after a mismatch, it returns `stale_preview` or `conflict` with no intentional change. Apply operations through existing engine commands/undo infrastructure. Test rollback on validation errors and disk write failures; when a disk-level multi-file transaction cannot be guaranteed, do not claim full atomicity—stage file writes and provide best-effort recovery with explicit partial-failure reporting until a transactional implementation is proven.

Illustrative request:

```json
{
  "operations": [
    {
      "kind": "create_page",
      "component": "product",
      "module": "ROOT",
      "title": "Configuring Authentication",
      "file_name": "authentication.adoc",
      "add_to_navigation": true
    }
  ]
}
```

This design is preferable to directly exposing more unguarded text-write tools. Existing `edit_page` and `rename_page` remain for backwards compatibility; their permission behaviour must remain documented. Add optional expected revision parameters to those tools later without breaking existing callers.

### 7.6 M6: Antora-aware structural creation

| Tool | Behaviour | Prerequisite |
| --- | --- | --- |
| `create_page` | Create `pages/<slug>.adoc` in the selected component/module, with an optional validated nav insertion | Existing engine `page.new` |
| `create_partial` | Create a partial in the selected module and return canonical include target | Existing engine `page.new_partial` |
| `create_module` | Create module resource families, optional `nav.adoc`/index page and update `antora.yml` | #290 |
| `create_project` | Scaffold Antora component, playbook and optional Git init from an approved template | #291–#295 |

All tools are adapters to the **same engine commands** used in the desktop application; no MCP-side `std::fs::write` equivalents are allowed. Every operation must reject ambiguous component/version selection, collisions, forbidden symlinks, invalid slugs, unsupported templates and paths outside roots. Prefer the M5 preview/apply workflow as the normal client path. If convenience methods apply immediately, require the same write capability, expected revision checks and a clear response listing changed files.

`create_project` is deliberately deferred until the underlying engine feature is implemented and path/scope policy is resolved. Never create a new workspace root outside the server's originally authorised roots merely because an agent requested it.

### 7.7 M7: Optional publication, prompts and subscriptions

**Publication:** `validate_publication` combines available audit results, effective Antora navigation and publication-profile validation into a read-only report with explicit coverage. It is a separate operation from `build_site`.

**Build:** `build_site` is optional and requires a distinct external-execution capability (proposed `--allow-build`). It delegates to the command being introduced under #293, uses bounded process spawning, reports logs/output paths and requires explicit invocation. Antora/Node processes may execute arbitrary extensions and access files or network themselves: ordinary root confinement in `adoc-mcp` cannot sandbox those subprocesses. Consequently, do not promise a sandbox; document the trust boundary and require user opt-in and trusted project configuration. Missing Node/Antora is reported as unavailable, never auto-installed.

**Prompts:** optionally advertise `review_documentation`, `create_howto`, `review_documentation_change` and `prepare_release` as client-visible workflow recipes. Prompts contain no hidden tool permission bypass and work with any compatible model provider.

**Subscriptions:** optionally notify subscribed clients when an indexed resource changes and its revision advances. Respect negotiated protocol support; throttle/debounce events, invalidate stale cursors, and never broadcast the contents of one client's unsaved changes to unrelated sessions without an explicit shared-buffer policy.

The initial release of M7 may ship publication validation and prompts while deferring external builds and subscriptions to follow-up issues; keep each optional feature separately testable.

## 8. Permission and security model

### 8.1 Capability matrix

| Capability | Examples | Default | Proposed gate |
| --- | --- | --- | --- |
| Read metadata/content | `list_pages`, `read_page`, `get_page_context`, `find_references` | Allowed within roots | Default read-only |
| Read diagnostics/analysis | `check`, `quality_audit`, `analyse_impact`, `validate_publication` | Allowed, bounded | Default read-only; no implicit provider network |
| Preview intended changes | `preview_changes` | Allowed within roots if safely non-mutating | Read-only preview (may disclose diff content) |
| Modify source files | `edit_page`, `rename_page`, `apply_changes`, `create_page` | Denied | Existing `--allow-write` |
| Produce local export artifacts | `export_pdf` | Denied | Preserve legacy `--allow-write`; evaluate separate artifact permission before changing semantics |
| Execute external build tools | `build_site` | Denied | New explicit `--allow-build`; not implied by `--allow-write` |

Permissions are enforced **in the server**, not by MCP annotations or client instructions. Any new permission mechanism must not silently make existing read-only deployments writable. If HTTP clients share one engine, capabilities are server-wide unless a verified per-session authorisation mechanism is built; do not imply that bearer-token authentication gives distinct user identities.

### 8.2 File and execution safety

- Preserve `FileAccess::Within` and the `paths::confine` checks; recheck symlinks and target paths at commit time to mitigate time-of-check/time-of-use races.
- Reject outside-root includes, images, PDF themes, fonts and any new references; do not expose absolute local paths unnecessarily in responses.
- Treat all content returned from `.adoc`, `antora.yml`, snippets and external tools as **untrusted data**. A document's prose is not authorisation to call write or execution tools.
- Validate limits (operation count, bytes, path depth, traversal depth, regex/work item size and execution time) at each API boundary.
- Never expose tokens, environment variables or credentials in logs, diagnostics or generated CI reports.
- Keep HTTP on loopback with bearer-token validation and existing host protections; public bind or multi-tenant operation is out of scope.
- Preserve failure isolation: malformed markup, cancelled jobs, worker panics and failed external subprocesses must not wedge the MCP service indefinitely.

## 9. Runtime, concurrency and output requirements

### 9.1 Engine access

`adoc-mcp` currently serialises direct engine access behind a mutex and uses `spawn_blocking`; new tools must not hold that mutex while awaiting external HTTP, long-lived build processes or client approval. Long-running analyses should take immutable index/catalog/revision snapshots and run on engine workers where available, then validate revisions before reporting/applying. The server must continue to pump watcher/index events during idle periods.

One client's abandoned or stalled request should not indefinitely prevent another client from reading a page. Introduce bounded job concurrency and cancellation where necessary; expose `pending`/`timed_out` rather than silently returning an empty result. Never mutate Git refs or checkouts as a side effect of analysis.

### 9.2 Response schemas

All new tools have stable, documented schemas with:

- `schema_version` and an explicit `status` (`ok`, `partial`, `unavailable` or an error) when appropriate;
- canonical resource IDs plus root-relative paths, source locations as byte offsets (with line/column at protocol boundaries if available);
- deterministic sort order, a query/revision-bound cursor and a declared page size;
- a separately reported `completeness` for quality/impact analyses and `skipped` evidence as applicable;
- typed errors without leaking internal debug traces or local filesystem secrets.

Prefer structured results, not free-form prose, for automation. Results must distinguish **empty** (`no findings`) from **incomplete** (`could not inspect everything`) and **truncated** (`more results available`).

### 9.3 Performance budgets (targets, not verified measurements)

- No new full-workspace scan on every keystroke or render event. Warm metadata/reference queries should use indexed lookups and revision caches.
- `list_pages` and `find_references` operate incrementally and are paginated for 20,000-page fixture workspaces.
- Background audit and impact work must not regress the current editor frame-time budgets; add measured release-mode benchmarks before setting hard numerical limits.
- Existing response and subprocess timeouts remain upper bounds until targeted benchmarks justify changes; large results must paginate instead of relying on silent `MAX_ITEMS` clipping.

## 10. Architecture and crate responsibilities

```text
L4  adoc-mcp                 transport, schemas, permissions, cursors,
                             session-scoped previews, subscriptions
    adocfoundry-cli         same quality/impact headless contracts
    adocfoundry             same engine commands and previews

L3  adoc-engine              command registry, workers, snapshots,
                             validation, undo, caches and events

L2  adoc-audit (planned)     deterministic quality rules/reports
    adoc-impact (planned)    typed impact traversal and provenance
    adoc-git                local Git objects/status, no transport

L1  adoc-antora             catalog and resource-id semantics
    adoc-index              document index and reverse lookups
    adoc-lang               symbols, reference resolution, diagnostics
    adoc-graph              generic link graph and traversals

L0  adoc-core               shared domain types only
```

The layer diagram is conceptual and omits unrelated existing crates. New `adoc-audit` and `adoc-impact` belong to their respective PRDs, **not** to an MCP work package. No new MCP-specific domain crate is initially proposed. If any new crate is introduced later, update `LAYERS` in `xtask/src/layers.rs`, `[workspace.dependencies]`, `CLAUDE.md` and architecture documentation and pass `cargo xtask layers`.

The server's proposed modules could be divided by concern (for example `tools/discovery.rs`, `tools/analysis.rs`, `tools/mutations.rs`, `responses.rs`, `permissions.rs`) rather than growing a single `server.rs` indefinitely. This is an implementation detail; avoid a large refactor merely to move files before new behaviour is added.

## 11. Testing strategy and normative acceptance cases

Every work package adds unit, integration and negative/security tests. MCP transport tests exercise actual JSON-RPC request/response bodies where feasible, not only Rust helper functions. Use temporary Antora workspaces and temporary Git repositories; never test against a contributor's real checkout.

### 11.1 Protocol and discovery

1. `mcp_existing_tools_are_backwards_compatible`: current tool names, permissions and text fallback still work.
2. `mcp_structured_result_matches_text_fallback`: structured payload and compatibility text describe the same result.
3. `mcp_resources_cursor_stable_and_bounded`: resources paginate deterministically with no duplicates or omissions.
4. `mcp_cursor_rejects_stale_index_revision`: an index change invalidates or consistently snapshots a cursor.
5. `mcp_list_pages_honours_component_version_and_family`: versionless and versioned components are not conflated.
6. `mcp_find_references_returns_exact_spans`: xref, include, nav and anchor references have expected locations.
7. `mcp_page_context_distinguishes_known_and_unknown_attributes`: dynamic contexts do not invent values.

### 11.2 Quality and impact

8. `mcp_quality_matches_cli`: same fixture, scope and policy produce identical rules, counts and completeness.
9. `mcp_quality_reports_skips_separately_from_coverage`: dynamic content skips do not automatically downgrade coverage.
10. `mcp_impact_partial_propagates_with_evidence`: nested partial changes reach relevant rendering pages.
11. `mcp_impact_prose_does_not_propagate_via_xref`: ordinary prose changes do not rewrite the meaning of link dependencies.
12. `mcp_impact_ref_comparison_uses_two_snapshots`: removed and renamed paths use base and head dependency evidence, not working-tree substitution.
13. `mcp_analysis_pagination_does_not_hide_incomplete`: truncated analytical results preserve coverage and continuation metadata.

### 11.3 Safe mutations

14. `mcp_preview_does_not_mutate`: no disk or buffer changes until an explicit apply.
15. `mcp_apply_rejects_stale_revision`: concurrent edits invalidate a preview, with no source overwritten.
16. `mcp_preview_ids_are_session_bound`: a second HTTP session cannot apply another session's plan.
17. `mcp_multi_file_failure_is_reported_and_recoverable`: partial filesystem write failure is handled according to the documented transaction guarantee, without claiming success.
18. `mcp_create_page_updates_nav_and_undo`: page/resource identity, nav edit and grouped undo agree with desktop semantics.
19. `mcp_create_module_reuses_engine_command`: no MCP-specific Antora layout logic or duplicated `antora.yml` writing.
20. `mcp_write_denied_by_default`: every mutating tool is absent or refused without write permission, including convenience wrappers.

### 11.4 Security, runtime and transport

21. `mcp_symlink_escape_is_rejected_at_apply`: cannot redirect file writes outside workspace via symlink race or changed target.
22. `mcp_http_requires_token_and_loopback`: retain existing transport restrictions.
23. `mcp_document_instructions_cannot_grant_permissions`: untrusted AsciiDoc text never changes capability state.
24. `mcp_build_not_enabled_by_allow_write`: external execution is separately gated.
25. `mcp_timeout_does_not_block_other_clients_forever`: stalled analysis/build does not deadlock the server.
26. `mcp_resource_change_notifications_are_revision_scoped`: when subscriptions are enabled, notified subscribers receive only authorised resource changes.

Build checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers`, `cargo deny check`. Add feature-specific `adoc-mcp` integration tests and measured budget tests for large synthetic Antora workspaces.

## 12. Work packages and issue decomposition

| WP | Issue | Deliverable | Dependencies | Acceptance milestone |
| --- | --- | --- | --- | --- |
| M1 | #314 | MCP result contracts, tool metadata, stable errors, resource/list pagination | Existing server | Existing clients still work; structured outputs and cursors verified |
| M2 | #315 | `list_pages`, `get_outline`, `get_page_context`, `find_references` | Existing catalog/index; #99 for full attribute provenance | A client can locate any relevant page without reading the entire workspace |
| M3 | #316 | Read-only `quality_audit` tool | Quality #300–#303; coordinate with #305 | MCP quality matches CLI with explicit completeness |
| M4 | #317 | Read-only `analyse_impact` tool | Impact #307–#310 | MCP impact matches CLI, including two-ref snapshots |
| M5 | #318 | Change previews, revision checks, session ownership and safe apply | Existing engine preview/undo infrastructure | Stale or conflicting writes are rejected; multi-file failure contract tested |
| M6 | #319 | Antora-aware `create_page`, `create_partial`, `create_module`; defer `create_project` as required | Existing page commands, #290, #291–#295 | Created content resolves and nav/undo are correct |
| M7 | #320 | `validate_publication`, optional `build_site`, prompts/subscriptions, end-to-end QA and documentation | Quality/Impact contracts; #293 for builds | Read-only release report and opt-in optional capabilities verified |

**Suggested order:** M1 → M2; M3/M4 as their engine APIs stabilise; M5 before expanding agent-driven mutations; M6 once its underlying engine commands exist; M7 after the core workflows. One reviewable PR per work package, with sub-issues if a package grows. Do not close existing Quality, Impact or New Project issues merely because an MCP adapter exists.

## 13. Definition of done

The initial MCP Agent Workflows milestone is complete when an MCP client can, on a multi-component Antora fixture:

1. Discover pages and their effective context without transferring every file's body.
2. Find exact incoming references and page outlines.
3. Obtain quality and impact reports consistent with the CLI, including honest completeness and provenance.
4. Preview a page plus navigation change, receive an explicit diff, apply it only against validated revisions and reject stale edits.
5. Create an Antora page via an engine command, with correct resource ID and navigation, with the server read-only by default.
6. Access typed/paginated responses through stdio and authenticated loopback HTTP without loosening path/security boundaries.
7. Pass the workspace's lint, test, layering and dependency checks; provide usage examples and negative/security tests.

`create_project`, external site builds and resource subscriptions are **follow-on capabilities** unless separately accepted into the first milestone. The absence of those optional features does not block completion of the core MCP milestone.

## 14. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's PR and in the Implementation Notes.

1. Should `preview_changes` be available in read-only mode, or should building previews for a mutation require `--allow-write` even without applying?
2. Should a client-facing `create_page` apply immediately by default or always return a preview first? Preferred: preview first for structural/multi-file changes.
3. Should `--allow-write` continue to imply PDF artifact export, or should a new explicit artifact-write capability be introduced with a deprecation period?
4. What are the preview TTL, maximum staged bytes, concurrency limits and idempotency-key retention period?
5. How are HTTP sessions isolated when the engine exposes unsaved editor buffers, and what is the authorised sharing policy?
6. Should M7's `build_site` require an additional interactive approval workflow, or is explicit `--allow-build` plus trusted local configuration sufficient?
7. Is `validate_publication` a dedicated engine query or an orchestration layer over existing audit/profile reports? It should avoid duplicate rule logic either way.
8. How should uncommitted/unsaved buffers be represented in Git-impact comparisons and shared MCP sessions?

## 15. Implementation notes

### M1 (#314)

- **Structured results.** `responses::answer` gives every tool's JSON as text (unchanged) and as `structured_content`; `call_tool` drops the structured content, and `list_tools` the output schemas, for a session whose negotiated protocol predates 2025-06-18. The output schemas (`schemas.rs`) are written by hand rather than through `Json<T>`: the tools build their JSON from engine types that implement no `JsonSchema`, and `adoc-engine` takes no `schemars` dependency for an adapter's sake. They describe each answer's top level, with nested engine reports as objects.
- **Errors.** A failure is a tool error whose text is the message, as before, and whose structured content is `{ "error": { "code", "message" } }`. `EngineError` maps to codes in one place (`From<EngineError> for Failure`). Methods that are not tools (`resources/list`, `resources/read`) answer JSON-RPC errors with the code in `data.code`. A stale or foreign cursor is `invalid_argument` with a message saying so; no new code was added for it.
- **Write tools without `--allow-write`** stay unlisted, as before; a call is now refused with `permission_denied` instead of rmcp's "tool not found".
- **Cursors** hold the query, the workspace revision and an offset, hex-encoded. The revision is `Engine::workspace_revision`, new in this package: it counts observed changes of (whole-index replacements, index revision, catalog revision), since the index's and catalog's own revisions restart with a whole index. Offsets are sound because the list is the same while the revision is. `resources/list` orders by Antora resource id, then root-relative path, with files outside any component (no id) first. Pages are at most 500; `check` and `symbols` take `limit`. `list_components`, `search` and `link_graph` are not paged in M1: their cut lists keep `truncated` and add `incomplete: true`. `index_incomplete` is answered by the paged lists while a whole index runs (`Engine::is_indexing_workspace`, new), not while watcher changes wait.
- **Runtime.** Tools still lock the one engine for their work; what changed is the wait: at most `LOCK_WAIT` (30 s), then `timeout`, and at most `MAX_WAITING` (16) requests waiting, then `unavailable`. A render, search, Git status or export still holds the engine up to its own 120 s limit, so taking immutable snapshots off the lock remains for the packages that add long analyses.
- **Annotations** are set on every tool and grant nothing.
- **Fixed on the way:** `get_nav` without a `version` looked up the versionless component only; it now takes the latest version, as its description said.

### M2 (#315)

- **`list_pages`** lists any family (`page` by default, `any` for all), since the catalog holds them in one order; `in_navigation` is given for pages only. Versions are named as `antora.yml` names them, `~` for the versionless one, and are never conflated. `version` or `module` without `component` is `invalid_argument`; an unknown component or version is `not_found`. Its cursor resumes after the last coordinate given (keyset), so a page reads only what it returns plus what the filters skip (the engine's `ListResource` probe; about 2 ms for a page of 500 in a 20,000-page workspace, release build); it gives no `total`, which would read every resource.
- **`in_navigation`** means a navigation file listed by the page's own component version's `antora.yml` links to it, read from the graph's nav links: the versionless version's navigation never makes a numbered version's page listed.
- **`find_references`** is paged by offset over the resolved list, which comes from the index's reverse maps (`references_to`, `includes_of`, `references_to_anchor`, the last new in `adoc-lang`); a page with 20,000 references resolves them all per request (about 120 ms). Every listed reference was resolved to the target, so `confidence` is always `resolved`; unresolvable dynamic targets are not listed. Columns are one-based bytes, as `check` gives them.
- **`get_page_context`** gives declared attributes (page and `antora.yml`), each with its value as written, `dynamic` when it names another attribute and `unset` for `:name!:`, and `effective_values: "unknown"` until #99. Incoming references are counted by kind; `find_references` lists them.
- **`get_outline`** uses `adoc-lang`'s document symbols (navigation symbols for a navigation file, its text read back through `FileEntry::text`, since the index keeps no text of files read from disk), with section ids from `adoc_lang::xref::section_ids` (now public).

### M3 (#316)

- **Policy.** `quality_audit` reads the policy as `adocfoundry-cli audit` does, from the first root's `.adocfoundry.toml` alone, and audits through `Engine::audit_now_with`, the CLI's call; `fail_on` and `require_complete` override it as `--fail-on` and `--require-complete` do. Invalid `[quality]` settings are listed in `policy_problems` and give exit code 2 and `outcome: untrustworthy`, as the CLI exits.
- **Projection.** The answer is the report's own JSON (its `schema_version`), with `findings` filtered (`rules`, `severity`) and paged, `region`s added, `excluded` left out unless `include_excluded`, and `findings_total`, `next_cursor`, `verdict`, `exit_code`, `outcome`, `policy_problems` and `workspace_revision` added. Counts, coverage, skips and completeness are the whole report's on every page.
- **Outcome.** `pass` only when the verdict passes and the audit is complete; `pass_partial` when it passes with gaps, so a partial audit is never shown as a plain pass whatever the client does with it.
- **Scope.** A component's versions are named as the audit's `AuditScope` takes them: every version, or a numbered one. `version: "~"` is refused, as the scope cannot select the versionless version alone.
- **Cost.** Each call audits afresh (as the CLI does), so paging recomputes the audit per page; the kept, incremental audit follows the user's layered settings, not the workspace-only policy, so it is not used here.
- **Test against the CLI.** `mcp_quality_matches_cli` compares the reassembled pages with the CLI's JSON snapshot of the same fixture, which its own tests keep current.

### M5 (#318)

Decisions settled (§14):

1. **Preview without `--allow-write`: yes.** `preview_changes` writes nothing to disk, changes no buffer and indexes nothing (it stages on a clone of the workspace state), and its diffs show only what `read_page` already shows. It answers `can_apply: false` with a reason on a read-only server; `apply_changes` is a write tool, unlisted and refused with `permission_denied` without the flag.
4. **Limits.** A preview takes at most 64 operations; a session keeps at most 16 previews and 4 MiB of staged text, the oldest dropped first; previews expire after 15 minutes; idempotency keys are kept 15 minutes, a retry with one returning the first answer (success or failure). A preview is consumed by its first apply, successful or not.
5. **Sessions.** Previews belong to the session that made them (`Server::for_session` per HTTP session; stdio is one session); an id from another session is unknown (`stale_preview`, the same answer as an expired or applied one, so nothing about other sessions is revealed). All sessions share one engine and therefore its buffers: unsaved text from `edit_page` or a `save: false` apply is what every session reads. That is the existing behaviour of a server-wide engine, kept rather than given a shared-buffer policy; per-session buffers would need an engine per principal.

Other notes:

- **Operations** are `replace`, `create_page`, `create_partial` and `rename`; a navigation update is a `replace` on the navigation file or `create_page`'s `add_to_navigation`. A page's navigation entry goes at the end of the module's navigation file, as there is no active page to follow.
- **Revision checks** use the SHA-256 of each file's current text (its buffer when open), recorded at preview time and compared again at apply; an optional `expected_hash` per operation is checked at preview time. `apply_staged` checks the base texts again inside the engine.
- **Atomicity.** The checks and the engine's change are all or nothing (`apply_staged` rolls back made files, edits and moves on failure). Disk writes are not: made files are written as they are made (as New Page writes them), and edited pages are saved one by one. A failed save is reported as `internal_error` with `details` (`applied`, `saved`, `failed`, `recovery`), the change staying in the buffers; no success is claimed. A transactional multi-file save is left for later.
- **Validation findings** are each changed file's diagnostics after the change, on a clone of the workspace state with every staged file indexed and every new resource in the catalog.
- `edit_page` and `rename_page` are unchanged.

### M6 (#319)

Decision 2 settled (§14): **preview first is the normal path, and the convenience tools apply at once.** `preview_changes` takes `create_page` and `create_partial` operations, staging the same plan `page.new` makes, for review and for changes of several files. `create_page` and `create_partial` are write tools (only with `--allow-write`) for a client that has the user's agreement already; they apply immediately through `page.new` and `page.new_partial`, list the files changed, and take `expected_nav_hash` as their revision check (a new file's absence is checked by the command itself, a collision answering `conflict`).

- `create_page` stages the plan first, without changing anything, to confine every path it would touch and check the navigation file's hash, then invokes `page.new`, so the page, its file name from the title, its entry and its one undo step are the app's. With no active page, the entry goes at the end of the module's navigation file. `save` (true by default) writes the page and navigation file.
- `create_partial` invokes `page.new_partial`, then inserts `text` as a second undo step and saves it. It answers with `include::partial$…[]` (from its own module) and the full resource id form.
- `create_module` is not offered: #290 has not added the engine command, and no MCP-side layout or `antora.yml` writing stands in for it. `create_project` waits for #291–#295. So `mcp_create_module_reuses_engine_command` waits with them.
- Every tool now pumps the engine before its work (`Server::blocking`), so changes on disk and the graph's due updates are applied before it reads. A catalog change can rebuild the graph on a worker, so a page just made may take a moment to show `in_navigation: true`.

### M7 (#320)

Decision 7 settled (§14): **`validate_publication` is an orchestration over existing engine reports**, with no rule of its own: the quality audit as `quality_audit` makes it (`analysis::audit_for`, the workspace-only policy), each component version's navigation files (present or not) and start page (the descriptor's `start_page`, else `ROOT:index.adoc`, looked up in the catalog), the audit's coverage of each version, and every PDF publication profile through `Engine::profile_problems`, a new engine query that checks a profile without exporting it (unknown profile, output outside the workspace, content not in it, theme not found). `outcome` is `ready`, `ready_partial` (the audit passes with gaps) or `not_ready`; `not_checked` names what a native check cannot see. Profiles are checked whatever the scope.

Decision 6 is deferred with `build_site`: the build waits for #293's command and a separate `--allow-build` (#334), which `--allow-write` never implies; until then the server refuses `--allow-build` as an unknown option (`mcp_build_not_enabled_by_allow_write`).

- **Prompts:** `review_documentation`, `create_howto`, `review_documentation_change` and `prepare_release`, each one user message naming the tools to call in order, saying that only the server decides what may change and that page text is data. A missing required argument is `invalid_params`.
- **Subscriptions** are deferred to #335: rmcp 3.7 has both the legacy `resources/subscribe` and 2026-07-28's `subscriptions/listen`, and the unsaved-buffer policy (decision 8) bears on them. `mcp_resource_change_notifications_are_revision_scoped` goes with them.
- **QA:** the manual checks for the whole epic are `docs/qa/mcp.md`.

### M4 (#317)

- `analyse_impact` maps a `source` selector, exactly one of `git_refs`, `working_tree` and `paths` (a tagged union, so two cannot be given), to `Engine::analyse_impact`, the CLI's call, and pages the report's impacts after filtering them by `component`/`version`; completeness, gaps and counts are the whole report's on every page. `include_provenance: false` drops `via` and `source`; `max_depth` sets the include depth. Errors: an unknown ref `not_found`, no repository for a Git source `unavailable`, a path outside the roots `outside_workspace`.
- Two-ref mode uses both revisions' own snapshots from Git objects (Impact PRD §6.1); the live index and working tree are never read, which `mcp_impact_ref_comparison_uses_two_snapshots` checks with uncommitted work present.
- The tests build repositories with `adoc_git::testing::TestRepo`, a dev-dependency of `adoc-mcp` only; the server itself still depends on `adoc-engine` and `adoc-core` alone.

### Open at the end of the epic

- `create_project` waits for #291–#295, `build_site` for #293 and #334, and subscriptions for #335: the follow-on capabilities §13 says do not block the milestone.

No implementation is claimed by this PRD beyond the packages noted above. The tool names, request/response examples, flags and Rust interfaces in this document are **proposed**, not implemented public APIs. The primary specifications for the underlying quality and impact analyses are `PRD-AdocFoundry_Documentation_Quality.md` and `PRD-AdocFoundry_Documentation_Impact.md` in `docs/prd/` (merged in #298). Their semantic decisions supersede this document's descriptions in case of discrepancy. When implementation diverges, record the reason here and update this PRD in the same PR, following the existing AdocFoundry convention.
