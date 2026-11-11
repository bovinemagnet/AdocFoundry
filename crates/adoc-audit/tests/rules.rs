//! The audit's rules over `tests/fixtures/audit`: navigation coverage, titles, alternative text
//! and the existing diagnostics, as the Quality PRD's acceptance tests 1 to 5 name them.

use std::{
    fs,
    path::{Path, PathBuf},
};

use adoc_audit::{
    audit, AuditInput, AuditReport, AuditScope, Completeness, FindingCode, NavCoverage, Policy,
    RuleId, Severity,
};
use adoc_core::DiagnosticCode;
use adoc_lang::WorkspaceState;

fn root() -> PathBuf {
    adoc_index::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/audit"),
    )
}

fn workspace() -> WorkspaceState {
    let mut workspace = WorkspaceState::new();
    workspace.index_roots(&[root()]).unwrap();
    workspace
}

fn run(workspace: &WorkspaceState, scope: AuditScope) -> AuditReport {
    let roots = [root()];
    let read = |path: &Path| fs::read_to_string(path);
    let input = AuditInput {
        workspace,
        roots: &roots,
        read: &read,
    };
    audit(&input, scope).finish(&Policy::default())
}

fn whole() -> AuditReport {
    run(&workspace(), AuditScope::Workspace)
}

/// `(path, code)` of every finding with `code`.
fn found(report: &AuditReport, code: FindingCode) -> Vec<String> {
    report
        .findings()
        .iter()
        .filter(|finding| finding.code == code)
        .map(|finding| finding.location.path.clone())
        .collect()
}

fn skipped(report: &AuditReport, code: FindingCode) -> Vec<String> {
    report
        .skips()
        .iter()
        .filter(|skip| skip.code == code)
        .map(|skip| skip.location.path.clone())
        .collect()
}

const UNLISTED: FindingCode = FindingCode::Rule(RuleId::UNLISTED_PAGE);
const TITLE: FindingCode = FindingCode::Rule(RuleId::MISSING_TITLE);
const ALT: FindingCode = FindingCode::Rule(RuleId::MISSING_ALT);
const NAV_UNRESOLVED: FindingCode = FindingCode::Diagnostic(DiagnosticCode::AntoraNavUnresolved);

fn coverage<'a>(
    report: &'a AuditReport,
    component: &str,
    version: Option<&str>,
) -> &'a NavCoverage {
    report
        .coverage()
        .iter()
        .find(|coverage| coverage.component == component && coverage.version.as_deref() == version)
        .unwrap_or_else(|| panic!("no coverage for {component} {version:?}"))
}

#[test]
fn audit_unlisted_page_detects_only_page_family() {
    let report = whole();
    // Not the start page, a page listed through its alias, a hidden page, partials or an
    // example: only the page in no navigation of its version.
    assert_eq!(
        found(&report, UNLISTED),
        ["product/modules/ROOT/pages/orphan.adoc"]
    );
    let finding = report
        .findings()
        .iter()
        .find(|finding| finding.code == UNLISTED)
        .unwrap();
    assert_eq!(finding.severity, Severity::Warning);
    let coordinate = finding.coordinate.as_ref().unwrap();
    assert_eq!(coordinate.version.as_deref(), Some("3.0"));
    assert_eq!(coordinate.path, "orphan.adoc");
    assert_eq!(
        finding
            .evidence
            .iter()
            .filter_map(|evidence| evidence.location.as_ref())
            .map(|location| location.path.as_str())
            .collect::<Vec<_>>(),
        [
            "product/modules/ROOT/nav.adoc",
            "product/modules/admin/nav.adoc"
        ],
        "each navigation file looked in"
    );
}

#[test]
fn audit_navigation_union_deduplicates_multiple_navs() {
    let report = whole();
    assert!(!found(&report, UNLISTED)
        .iter()
        .any(|path| path.ends_with("users.adoc")));
    // Eight published pages in 3.0; `users.adoc`, in both navigation files, counts once.
    let product = coverage(&report, "product", Some("3.0"));
    assert_eq!(
        (product.pages, product.listed, product.undetermined),
        (8, 7, 0)
    );
    assert_eq!(product.navigation_files, 2);
}

#[test]
fn audit_alias_and_versionless_resources() {
    let report = whole();
    let unlisted = found(&report, UNLISTED);
    assert!(!unlisted.iter().any(|path| path.ends_with("configure.adoc")));
    assert!(
        !unlisted.iter().any(|path| path.starts_with("product-2.0/")),
        "2.0 lists its orphan; only 3.0's is reported"
    );
    assert_eq!(
        coverage(&report, "product", Some("2.0")).listed,
        2,
        "its start page and its orphan"
    );
    let guide = coverage(&report, "guide", None);
    assert_eq!((guide.pages, guide.listed, guide.undetermined), (3, 2, 1));
}

#[test]
fn audit_dynamic_content_is_conservative() {
    let report = whole();
    assert!(report.completeness().is_complete(), "skips are not gaps");
    // The guide's navigation includes another file and names an attribute: its unlisted page
    // may be listed after all.
    assert_eq!(
        skipped(&report, UNLISTED),
        ["guide/modules/ROOT/pages/extra.adoc"]
    );
    assert!(!found(&report, UNLISTED)
        .iter()
        .any(|path| path.starts_with("guide/")));
    // The entry naming an attribute is not reported as unresolved, under any code.
    assert!(!report
        .findings()
        .iter()
        .any(|finding| finding.location.path == "guide/modules/ROOT/nav.adoc"));
    // A title that may come from an include is skipped.
    assert_eq!(
        skipped(&report, TITLE),
        ["product/modules/ROOT/pages/header-include.adoc"]
    );
}

#[test]
fn audit_reuses_existing_diagnostics() {
    let workspace = workspace();
    let report = run(&workspace, AuditScope::Workspace);
    let orphan = root().join("product/modules/ROOT/pages/orphan.adoc");
    let existing = adoc_lang::diagnostics(&workspace, &orphan);
    assert_eq!(existing.len(), 1, "{existing:?}");
    let aggregated: Vec<_> = report
        .findings()
        .iter()
        .filter(|finding| {
            finding.location.path == "product/modules/ROOT/pages/orphan.adoc"
                && matches!(finding.code, FindingCode::Diagnostic(_))
        })
        .collect();
    assert_eq!(aggregated.len(), 1, "listed once");
    assert_eq!(
        aggregated[0].code,
        FindingCode::Diagnostic(existing[0].code)
    );
    assert_eq!(aggregated[0].message, existing[0].message);
    assert_eq!(aggregated[0].location.range, Some(existing[0].range));
    assert_eq!(aggregated[0].severity, Severity::from(existing[0].severity));

    // The navigation's unresolved entry is the nav file's own diagnostic, reported once: the
    // tree's `nav-unresolved`, which restates it, is not added.
    assert!(found(&report, NAV_UNRESOLVED).is_empty());
    let nav = root().join("product/modules/ROOT/nav.adoc");
    let unresolved: Vec<_> = report
        .findings()
        .iter()
        .filter(|finding| finding.location.path == "product/modules/ROOT/nav.adoc")
        .filter(|finding| {
            finding.code == FindingCode::Diagnostic(DiagnosticCode::UnresolvedXrefFile)
        })
        .collect();
    assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
    assert!(adoc_lang::diagnostics(&workspace, &nav)
        .iter()
        .any(|diagnostic| diagnostic.range == unresolved[0].location.range.unwrap()));
}

#[test]
fn pages_without_a_title_are_found() {
    let report = whole();
    assert_eq!(
        found(&report, TITLE),
        ["product/modules/ROOT/pages/untitled.adoc"]
    );
}

#[test]
fn images_without_alternative_text_are_found_in_pages_and_partials() {
    let report = whole();
    let missing: Vec<(&str, usize)> = report
        .findings()
        .iter()
        .filter(|finding| finding.code == ALT)
        .map(|finding| {
            (
                finding.location.path.as_str(),
                finding.location.range.unwrap().start,
            )
        })
        .collect();
    let images = fs::read_to_string(root().join("product/modules/ROOT/pages/images.adoc")).unwrap();
    let snippet =
        fs::read_to_string(root().join("product/modules/ROOT/partials/snippet.adoc")).unwrap();
    assert_eq!(
        missing,
        [
            (
                "product/modules/ROOT/pages/images.adoc",
                images.find("image::a.png").unwrap()
            ),
            (
                "product/modules/ROOT/pages/images.adoc",
                images.find("image:c.png").unwrap()
            ),
            (
                "product/modules/ROOT/partials/snippet.adoc",
                snippet.find("image::p.png").unwrap()
            ),
        ],
        "described, named, attribute and explicitly empty texts are the author's; the \
         inline `image:c.png[width=20]` has none"
    );
    let remediations: Vec<_> = report
        .findings()
        .iter()
        .filter(|finding| finding.code == ALT)
        .filter_map(|finding| finding.remediation.as_deref())
        .collect();
    assert!(
        remediations.contains(
            &"Describe the image in the macro's first attribute: `image:c.png[What it shows]`"
        ),
        "an inline image's remedy is written inline: {remediations:?}"
    );
}

#[test]
fn a_component_scope_reports_only_that_component_version() {
    let workspace = workspace();
    let report = run(
        &workspace,
        AuditScope::Component {
            name: "product".to_owned(),
            version: Some("2.0".to_owned()),
        },
    );
    assert!(report.findings().is_empty(), "{:#?}", report.findings());
    assert_eq!(report.coverage().len(), 1);

    let report = run(
        &workspace,
        AuditScope::Component {
            name: "product".to_owned(),
            version: None,
        },
    );
    assert!(report
        .findings()
        .iter()
        .all(|finding| finding.location.path.starts_with("product")));
    assert_eq!(report.coverage().len(), 2, "both versions");
}

#[test]
fn a_scope_names_the_versionless_version_as_tilde() {
    let report = run(
        &workspace(),
        AuditScope::Component {
            name: "guide".to_owned(),
            version: Some("~".to_owned()),
        },
    );
    assert!(report.completeness().is_complete(), "{report:#?}");
    assert_eq!(report.coverage().len(), 1);
    assert_eq!(
        skipped(&report, UNLISTED),
        ["guide/modules/ROOT/pages/extra.adoc"]
    );
}

#[test]
fn a_component_not_in_the_catalog_is_unavailable() {
    let report = run(
        &workspace(),
        AuditScope::Component {
            name: "platform".to_owned(),
            version: None,
        },
    );
    assert!(matches!(
        report.completeness(),
        Completeness::Unavailable(_)
    ));
    assert!(report.findings().is_empty());
}

#[test]
fn the_same_workspace_gives_the_same_report() {
    assert_eq!(whole(), whole());
    assert_eq!(whole().roots(), ["audit"]);
}

#[test]
fn an_unreadable_navigation_file_is_named_by_its_relative_path() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    let component = root.join("docs");
    fs::create_dir_all(component.join("modules/ROOT/pages")).unwrap();
    fs::write(
        component.join("antora.yml"),
        "name: docs\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    fs::write(component.join("modules/ROOT/pages/index.adoc"), "= Docs\n").unwrap();
    fs::write(component.join("modules/ROOT/pages/other.adoc"), "= Other\n").unwrap();
    let mut workspace = WorkspaceState::new();
    workspace.index_roots(std::slice::from_ref(&root)).unwrap();
    let roots = [root.clone()];
    let read = |path: &Path| fs::read_to_string(path);
    let input = AuditInput {
        workspace: &workspace,
        roots: &roots,
        read: &read,
    };
    let report = audit(&input, AuditScope::Workspace).finish(&Policy::default());
    let skip = report
        .skips()
        .iter()
        .find(|skip| skip.code == UNLISTED)
        .expect("the unlisted page is skipped");
    assert!(
        skip.reason.contains("docs/modules/ROOT/nav.adoc"),
        "{}",
        skip.reason
    );
    let text = serde_json::to_string(&report).unwrap();
    let absolute = root.to_string_lossy().replace('\\', "\\\\");
    assert!(!text.contains(&*absolute), "an absolute path in {text}");
}

#[test]
fn existing_messages_name_files_relative_to_their_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    let component = root.join("broken");
    fs::create_dir_all(component.join("modules/ROOT/pages")).unwrap();
    fs::write(component.join("antora.yml"), "name: [unclosed\n").unwrap();
    fs::write(
        component.join("modules/ROOT/pages/index.adoc"),
        "= Broken\n",
    )
    .unwrap();
    let mut workspace = WorkspaceState::new();
    workspace.index_roots(std::slice::from_ref(&root)).unwrap();
    let roots = [root.clone()];
    let read = |path: &Path| fs::read_to_string(path);
    let input = AuditInput {
        workspace: &workspace,
        roots: &roots,
        read: &read,
    };
    let report = audit(&input, AuditScope::Workspace).finish(&Policy::default());
    let invalid: Vec<&str> = report
        .findings()
        .iter()
        .filter(|finding| {
            finding.code == FindingCode::Diagnostic(DiagnosticCode::AntoraInvalidDescriptor)
        })
        .map(|finding| finding.message.as_str())
        .collect();
    assert!(!invalid.is_empty(), "{:#?}", report.findings());
    assert!(
        invalid
            .iter()
            .all(|message| message.contains("`broken/antora.yml` is invalid")),
        "{invalid:#?}"
    );
    let text = serde_json::to_string(&report).unwrap();
    assert!(
        !text.contains(&*root.to_string_lossy()),
        "an absolute path in {text}"
    );
}

/// The report of a workspace made of `files`, each a path from the root and its text.
fn audit_files(files: &[(&str, &str)], scope: AuditScope) -> AuditReport {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    for (path, text) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let mut workspace = WorkspaceState::new();
    workspace.index_roots(std::slice::from_ref(&root)).unwrap();
    let roots = [root];
    let read = |path: &Path| fs::read_to_string(path);
    let input = AuditInput {
        workspace: &workspace,
        roots: &roots,
        read: &read,
    };
    audit(&input, scope).finish(&Policy::default())
}

#[test]
fn a_missing_navigation_file_leaves_what_the_others_list_listed() {
    let report = audit_files(
        &[
            (
                "docs/antora.yml",
                "name: docs\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n- modules/admin/nav.adoc\n",
            ),
            (
                "docs/modules/ROOT/nav.adoc",
                "* xref:a.adoc[A]\n* xref:b.adoc[B]\n",
            ),
            ("docs/modules/ROOT/pages/index.adoc", "= Docs\n"),
            ("docs/modules/ROOT/pages/a.adoc", "= A\n"),
            ("docs/modules/ROOT/pages/b.adoc", "= B\n"),
            ("docs/modules/ROOT/pages/c.adoc", "= C\n"),
            ("docs/modules/admin/pages/x.adoc", "= X\n"),
        ],
        AuditScope::Workspace,
    );
    let coverage = coverage(&report, "docs", None);
    assert_eq!(
        (coverage.pages, coverage.listed, coverage.undetermined),
        (5, 3, 2)
    );
    assert_eq!(
        skipped(&report, UNLISTED),
        [
            "docs/modules/ROOT/pages/c.adoc",
            "docs/modules/admin/pages/x.adoc"
        ]
    );
    assert!(found(&report, UNLISTED).is_empty());
}

#[test]
fn dash_items_and_the_xref_shorthand_list_their_pages() {
    let report = audit_files(
        &[
            (
                "docs/antora.yml",
                "name: docs\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
            ),
            (
                "docs/modules/ROOT/nav.adoc",
                "* xref:a.adoc[A]\n* <<shorthand.adoc#,Shorthand>>\n- xref:dash.adoc[Dash]\n",
            ),
            ("docs/modules/ROOT/pages/index.adoc", "= Docs\n"),
            ("docs/modules/ROOT/pages/a.adoc", "= A\n"),
            ("docs/modules/ROOT/pages/shorthand.adoc", "= Shorthand\n"),
            ("docs/modules/ROOT/pages/dash.adoc", "= Dash\n"),
            ("docs/modules/ROOT/pages/orphan.adoc", "= Orphan\n"),
        ],
        AuditScope::Workspace,
    );
    assert_eq!(
        found(&report, UNLISTED),
        ["docs/modules/ROOT/pages/orphan.adoc"]
    );
    let coverage = coverage(&report, "docs", None);
    assert_eq!((coverage.pages, coverage.listed), (5, 4));
}

#[test]
fn a_version_in_two_descriptors_reads_both_navigations() {
    let report = audit_files(
        &[
            (
                "one/antora.yml",
                "name: docs\nversion: ~\nnav:\n- modules/one/nav.adoc\n",
            ),
            ("one/modules/one/nav.adoc", "* xref:page-one.adoc[]\n"),
            ("one/modules/one/pages/page-one.adoc", "= One\n"),
            (
                "two/antora.yml",
                "name: docs\nversion: ~\nnav:\n- modules/two/nav.adoc\n",
            ),
            ("two/modules/two/nav.adoc", "* xref:page-two.adoc[]\n"),
            ("two/modules/two/pages/page-two.adoc", "= Two\n"),
            ("two/modules/ROOT/pages/index.adoc", "= Docs\n"),
        ],
        AuditScope::Workspace,
    );
    assert!(
        found(&report, UNLISTED).is_empty(),
        "{:#?}",
        report.findings()
    );
    assert_eq!(report.coverage().len(), 1);
    let coverage = coverage(&report, "docs", None);
    assert_eq!(
        (coverage.navigation_files, coverage.pages, coverage.listed),
        (2, 3, 3)
    );
}

const DOCS: (&str, &str) = ("docs/antora.yml", "name: docs\nversion: ~\n");

#[test]
fn alternative_text_on_the_block_attribute_line_counts() {
    let report = audit_files(
        &[
            DOCS,
            (
                "docs/modules/ROOT/pages/index.adoc",
                "= Images\n\n[alt=\"A sunset\"]\nimage::a.png[]\n\n[A sunset over the sea]\nimage::b.png[]\n\n.Caption\n[#fig,alt=Sunset]\nimage::c.png[]\n\n[.right]\nimage::d.png[]\n\n[#fig2]\n[%interactive]\nimage::e.png[]\n",
            ),
        ],
        AuditScope::Workspace,
    );
    let reported: Vec<String> = report
        .findings()
        .iter()
        .filter(|finding| finding.code == ALT)
        .map(|finding| finding.message.clone())
        .collect();
    assert_eq!(
        reported,
        [
            "The image `d.png` has no alternative text",
            "The image `e.png` has no alternative text"
        ],
        "an id, role or option line gives no text"
    );
}

#[test]
fn a_title_comes_only_from_an_include_before_the_body() {
    let report = audit_files(
        &[
            DOCS,
            ("docs/modules/ROOT/pages/index.adoc", "= Docs\n"),
            (
                "docs/modules/ROOT/pages/untitled-listing.adoc",
                "Some text.\n\n[source,java]\n----\ninclude::example$Demo.java[]\n----\n",
            ),
            (
                "docs/modules/ROOT/pages/untitled-footer.adoc",
                "Some text.\n\nMore text.\n\ninclude::partial$snippet.adoc[]\n",
            ),
            (
                "docs/modules/ROOT/pages/header-include.adoc",
                "include::partial$header.adoc[]\n\nSome text.\n",
            ),
            ("docs/modules/ROOT/partials/header.adoc", "= Header\n"),
            ("docs/modules/ROOT/partials/snippet.adoc", "Snippet.\n"),
            ("docs/modules/ROOT/examples/Demo.java", "class Demo {}\n"),
        ],
        AuditScope::Workspace,
    );
    assert_eq!(
        found(&report, TITLE),
        [
            "docs/modules/ROOT/pages/untitled-footer.adoc",
            "docs/modules/ROOT/pages/untitled-listing.adoc"
        ]
    );
    assert_eq!(
        skipped(&report, TITLE),
        ["docs/modules/ROOT/pages/header-include.adoc"]
    );
}

#[test]
fn an_empty_title_line_is_no_title() {
    let report = audit_files(
        &[
            DOCS,
            ("docs/modules/ROOT/pages/index.adoc", "= \n\nText.\n"),
        ],
        AuditScope::Workspace,
    );
    let messages: Vec<&str> = report
        .findings()
        .iter()
        .filter(|finding| finding.code == TITLE)
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(messages, ["The page has no document title"]);
}

#[test]
fn images_in_hidden_pages_are_checked_as_partials_are() {
    let report = audit_files(
        &[
            DOCS,
            (
                "docs/modules/ROOT/pages/index.adoc",
                "= Docs\n\ninclude::_fragment.adoc[]\n",
            ),
            (
                "docs/modules/ROOT/pages/_fragment.adoc",
                "image::shot.png[]\n",
            ),
        ],
        AuditScope::Workspace,
    );
    assert_eq!(
        found(&report, ALT),
        ["docs/modules/ROOT/pages/_fragment.adoc"]
    );
    assert!(found(&report, TITLE).is_empty(), "a hidden page needs none");
}

#[test]
fn an_entry_naming_a_partial_does_not_list_the_page_of_its_name() {
    let report = audit_files(
        &[
            (
                "docs/antora.yml",
                "name: docs\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
            ),
            (
                "docs/modules/ROOT/nav.adoc",
                "* xref:partial$snippet.adoc[]\n",
            ),
            ("docs/modules/ROOT/pages/index.adoc", "= Docs\n"),
            ("docs/modules/ROOT/pages/snippet.adoc", "= Snippet\n"),
            ("docs/modules/ROOT/partials/snippet.adoc", "Snippet.\n"),
        ],
        AuditScope::Workspace,
    );
    assert_eq!(
        found(&report, UNLISTED),
        ["docs/modules/ROOT/pages/snippet.adoc"]
    );
}

#[test]
fn an_include_the_navigation_does_not_read_hides_nothing() {
    let report = audit_files(
        &[
            (
                "docs/antora.yml",
                "name: docs\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
            ),
            (
                "docs/modules/ROOT/nav.adoc",
                "* xref:a.adoc[]\n////\ninclude::partial$more-nav.adoc[]\n////\n----\ninclude::partial$more-nav.adoc[]\n----\n",
            ),
            ("docs/modules/ROOT/pages/index.adoc", "= Docs\n"),
            ("docs/modules/ROOT/pages/a.adoc", "= A\n"),
            ("docs/modules/ROOT/pages/orphan.adoc", "= Orphan\n"),
            (
                "docs/modules/ROOT/partials/more-nav.adoc",
                "* xref:orphan.adoc[]\n",
            ),
        ],
        AuditScope::Workspace,
    );
    assert!(skipped(&report, UNLISTED).is_empty());
    assert_eq!(
        found(&report, UNLISTED),
        ["docs/modules/ROOT/pages/orphan.adoc"]
    );
}
