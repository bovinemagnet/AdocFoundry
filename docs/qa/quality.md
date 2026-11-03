# Documentation Quality — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Quality audit acceptance (Quality PRD §9–§11) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ and Ubuntu 22.04+, from a build of `main` after #305 or from a release, unless the item names a platform. Record the build, the platform and the result in the table at the end.

## Automated coverage, for reference

- **Rules and the report:** `crates/adoc-audit/tests/rules.rs` covers the PRD's tests 1–5, titles, alternative text, scopes, unavailable scopes, determinism and unreadable navigation. `crates/adoc-audit/src/report.rs` covers the schema snapshot, ordering, the policy and the providers. `crates/adoc-audit/src/sarif.rs` covers regions and URIs.
- **The engine:** `crates/adoc-engine/tests/audit.rs` covers the kept audit matching a fresh one, incremental upkeep measured by probes, the audit's findings in Problems listed once, exclusions, edits, filters and Run Audit.
- **The command line:** `crates/adocfoundry-cli/tests/audit.rs` covers exit codes, strict mode, skips, the JSON and SARIF snapshots, suppressed exclusions, relative paths, unavailable providers, and `cli_and_app_report_the_same_audit`. That last test checks that the command line's report and the app's kept audit agree, finding by finding, under a workspace policy.
- **The app:** `audit_panel_opens_exact_source_location`, `audit_overview_counts_filter_problems` and `problem_filter_choices_clear_one_by_one` in `crates/adocfoundry/src/ui_tests.rs`. GPUI's test platform draws no pixels, so the items below are checked by eye.
- **Offline:** `audit_is_offline` in `xtask/src/layers.rs` checks that the audit crate names no file, process, network or thread API.
- **Cost:** `audit_upkeep_in_a_component_of_2000_pages` in `crates/adoc-engine/tests/perf.rs`, ignored by default. Run it with `cargo test --release -p adoc-engine --test perf audit_upkeep -- --ignored --nocapture`.

## Setup

1. A build of the app and of `adocfoundry-cli`, and a clone of the repository for `tests/fixtures/audit`.
2. A real documentation workspace of a few hundred pages or more.
3. For the CI item, a GitHub repository with code scanning enabled.

## The Quality panel

- [ ] Open `tests/fixtures/audit` and choose the Quality tab. It says no audit has run yet, with a Run Audit button.
- [ ] Press Run Audit. "Auditing…" shows at most briefly. Then the panel shows:
  - "Complete";
  - Warnings 6 and Information 1;
  - the rules with their counts;
  - coverage `guide: 2 of 3 pages listed, 1 undetermined`, `product 2.0: 2 of 2 pages listed` and `product 3.0: 7 of 8 pages listed`;
  - skips.
- [ ] Check the panel's layout in the light theme and the dark. Rows that link to Problems are drawn as links, hover, and are readable. Long rows end in an ellipsis within the sidebar's width.
- [ ] Click `quality.image.missing-alt: 2`. Problems opens with two rows, one under `images.adoc` and one under `snippet.adoc`, and a `quality.image.missing-alt ×` button. Click a row: the page opens with the image macro selected.
- [ ] Click `product 3.0: 7 of 8 pages listed`. Problems shows `orphan.adoc` alone, with no line or column, and clicking it opens the page at its start.
- [ ] In Problems, open Filter ▾. It offers the severities, codes and rules, component versions and modules present, with the current choices checked. Choosing an item filters the list, and choosing it again clears it. Choosing a component version narrows the Module items to that version's modules.
- [ ] Remove the title line of `orphan.adoc`. Within a moment of pausing, a `quality.page.missing-title` problem appears for it, and the Quality panel's counts follow. Typing stays as quick as before the audit ran.
- [ ] Add `* xref:orphan.adoc[]` to `product/modules/ROOT/nav.adoc`. The unlisted page disappears from Problems, and coverage reads `8 of 8`.
- [ ] Write `[quality]\nproviders = ["vale"]` in the fixture's `.adocfoundry.toml` and save. The panel turns Partial, gives the reason that the `vale` provider could not run, and lists `vale: unavailable` under Optional providers. The findings are unchanged.
- [ ] Add an exclusion for `3.0@product:ROOT:orphan.adoc` with a reason and save. The unlisted page leaves Problems and appears under Excluded by the settings with its reason.
- [ ] Press Run Audit again. "Out of date" shows while the new audit runs, then clears.
- [ ] Restore the fixture's files afterwards with `git checkout tests/fixtures/audit`.

## The command line and the app agree

- [ ] On the real workspace, run `adocfoundry-cli audit <root> --format json --output audit.json`. Then open the same folder in the app and press Run Audit. The panel's counts by severity and rule, and its coverage rows, equal the report's `counts`, findings by code and `coverage`.

## CI

- [ ] In a repository with code scanning, add the CI example from the user guide's command-line page: strict mode, SARIF uploaded with `github/codeql-action/upload-sarif`. The upload succeeds, and findings appear as alerts on the right lines, with excluded findings shown as suppressed.
- [ ] Validate `audit.sarif` against the SARIF 2.1.0 schema, for example with the SARIF Multitool's `validate` command. The tests check its structure but not the schema.
- [ ] Make one folder unreadable and run in strict mode. The job fails with exit 2, and the report is still uploaded.

## A large workspace

- [ ] On the real workspace, Run Audit finishes within a second or two, and typing in a page stays as quick with the audit kept as without.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
| 2026-11-03 | `quality-providers-qa` release | macOS, command line only | Pass, app items open | `adocfoundry-cli audit` on the real 533-page corpus took 2.1 s, including indexing. It was complete, with 426 warnings: 309 unlisted pages, 66 images without alternative text, 41 pages without a title and 10 unresolved anchors. No file in the corpus was written. The app items, the code-scanning upload and the schema validation have not been run. |
