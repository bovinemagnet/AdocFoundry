//! Impact rules (Impact PRD §11) over two revisions of `tests/fixtures/impact`: a copy indexed
//! as the base, changed on disk, and indexed again as the head. `product` 1.0 (`v1`) has
//! `index` and `install` including `partial$intro.adoc`, which includes `nested.adoc`; `index`
//! links to `install.adoc#setup` and `guide` to `install.adoc`; its navigation lists `index`
//! and `install`. `product` 2.0 (`v2`) lists only its own `index`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use adoc_impact::{
    analyse, Change, ChangeKind, Completeness, Confidence, DependencySnapshot, EvidenceSide,
    Impact, ImpactKind, ImpactReport, ImpactStatus, Limits, Publication, PublicationSelection,
};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Revisions {
    _directory: tempfile::TempDir,
    root: PathBuf,
    base: WorkspaceState,
}

impl Revisions {
    fn new() -> Self {
        Self::of("impact")
    }

    /// A copy of `tests/fixtures/<fixture>`.
    fn of(fixture: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures")
                .join(fixture),
            &root,
        );
        let base = state(&root);
        Self {
            _directory: directory,
            root,
            base,
        }
    }

    /// Takes the files as they are now as the base.
    fn rebase(&mut self) {
        self.base = state(&self.root);
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path(relative)).unwrap()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn remove(&self, relative: &str) {
        fs::remove_file(self.path(relative)).unwrap();
    }

    fn change(&self, relative: &str, kind: ChangeKind) -> Change {
        Change {
            path: self.path(relative),
            kind,
        }
    }

    /// The report of `changes`, the head side indexed from the files as they are now.
    fn report(&self, changes: &[Change], publications: &[Publication]) -> ImpactReport {
        self.report_within(changes, publications, Limits::default())
    }

    fn report_within(
        &self,
        changes: &[Change],
        publications: &[Publication],
        limits: Limits,
    ) -> ImpactReport {
        let head = state(&self.root);
        let roots = [self.root.clone()];
        analyse(
            changes,
            Some(&DependencySnapshot::borrowed(&self.base, &roots)),
            &DependencySnapshot::borrowed(&head, &roots),
            publications,
            limits,
        )
    }
}

fn state(root: &Path) -> WorkspaceState {
    let mut state = WorkspaceState::new();
    state.index_roots(&[root.to_path_buf()]).unwrap();
    state
}

const INTRO: &str = "v1/modules/ROOT/partials/intro.adoc";
const NESTED: &str = "v1/modules/ROOT/partials/nested.adoc";
const INDEX: &str = "v1/modules/ROOT/pages/index.adoc";
const INSTALL: &str = "v1/modules/ROOT/pages/install.adoc";
const GUIDE: &str = "v1/modules/ROOT/pages/guide.adoc";
const NAV: &str = "v1/modules/ROOT/nav.adoc";

fn of_kind(report: &ImpactReport, kind: ImpactKind) -> Vec<&Impact> {
    report
        .impacts
        .iter()
        .filter(|impact| impact.kind == kind)
        .collect()
}

fn paths(impacts: &[&Impact]) -> Vec<String> {
    impacts.iter().map(|impact| impact.path.clone()).collect()
}

#[test]
fn impact_partial_propagates_transitively() {
    let revisions = Revisions::new();
    revisions.write(NESTED, "A nested note, changed.\n");
    let report = revisions.report(&[revisions.change(NESTED, ChangeKind::Modified)], &[]);
    let render = of_kind(&report, ImpactKind::Render);
    assert_eq!(paths(&render), [INDEX, INSTALL]);
    for impact in &render {
        assert_eq!(impact.confidence, Confidence::Confirmed);
        assert_eq!(impact.evidence, EvidenceSide::Both);
        assert_eq!(impact.via, [NESTED, INTRO, impact.path.as_str()]);
        assert_eq!(impact.origin, NESTED);
        assert_eq!(impact.component.as_deref(), Some("product"));
        assert_eq!(impact.version.as_deref(), Some("1.0"));
    }
    assert_eq!(
        render[0].resource_id.as_deref(),
        Some("1.0@product:ROOT:page$index.adoc")
    );
    assert!(report.is_complete());
    assert_eq!(report.changes[0].kind, "modified");
}

#[test]
fn impact_page_prose_does_not_propagate_xrefs() {
    let revisions = Revisions::new();
    revisions.write(
        INSTALL,
        "= Installing\n\ninclude::partial$intro.adoc[]\n\n[#setup]\n== Setting up\n\nOther steps.\n",
    );
    let report = revisions.report(&[revisions.change(INSTALL, ChangeKind::Modified)], &[]);
    // The page itself, and nothing that merely links to it.
    assert_eq!(paths(&of_kind(&report, ImpactKind::Render)), [INSTALL]);
    assert!(of_kind(&report, ImpactKind::ReferenceIntegrity).is_empty());
    assert!(!report.impacts.iter().any(|impact| impact.path == GUIDE));
}

#[test]
fn impact_removed_anchor_flags_reference_review() {
    let revisions = Revisions::new();
    revisions.write(
        INSTALL,
        "= Installing\n\ninclude::partial$intro.adoc[]\n\n== Getting ready\n\nSteps.\n",
    );
    let report = revisions.report(&[revisions.change(INSTALL, ChangeKind::Modified)], &[]);
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    assert_eq!(paths(&references), [INDEX]);
    let review = references[0];
    assert_eq!(review.anchor.as_deref(), Some("setup"));
    assert_eq!(review.status, Some(ImpactStatus::Broken));
    let source = review.source.as_ref().unwrap();
    assert_eq!(source.path, INDEX);
    let text = fs::read_to_string(revisions.path(INDEX)).unwrap();
    assert_eq!(
        &text[source.range.start..source.range.end],
        "xref:install.adoc#setup[setting up]"
    );
}

#[test]
fn impact_nav_change_is_version_scoped() {
    let revisions = Revisions::new();
    revisions.write(NAV, "* xref:index.adoc[]\n* xref:guide.adoc[]\n");
    let report = revisions.report(&[revisions.change(NAV, ChangeKind::Modified)], &[]);
    let navigation = of_kind(&report, ImpactKind::Navigation);
    let listed: Vec<(&str, Option<ImpactStatus>)> = navigation
        .iter()
        .map(|impact| (impact.path.as_str(), impact.status))
        .collect();
    assert_eq!(
        listed,
        [
            // The changed file itself, whose navigation changed.
            (NAV, None),
            (GUIDE, Some(ImpactStatus::Listed)),
            (INSTALL, Some(ImpactStatus::Unlisted)),
        ]
    );
    // Only 1.0's: 2.0's navigation and pages are untouched.
    assert!(navigation
        .iter()
        .all(|impact| impact.version.as_deref() == Some("1.0")));
    assert!(!report
        .impacts
        .iter()
        .any(|impact| impact.path.starts_with("v2/")));
}

#[test]
fn impact_profiles_only_when_selected() {
    let revisions = Revisions::new();
    revisions.write(NESTED, "Changed.\n");
    let publications = [
        Publication {
            name: "one-page".into(),
            selection: PublicationSelection::Page(revisions.path(INDEX)),
        },
        Publication {
            name: "guide-only".into(),
            selection: PublicationSelection::Page(revisions.path(GUIDE)),
        },
        Publication {
            name: "book-1".into(),
            selection: PublicationSelection::Component {
                name: "product".into(),
                version: Some("1.0".into()),
            },
        },
        Publication {
            name: "book-2".into(),
            selection: PublicationSelection::Component {
                name: "product".into(),
                version: Some("2.0".into()),
            },
        },
        Publication {
            name: "elsewhere".into(),
            selection: PublicationSelection::Component {
                name: "absent".into(),
                version: None,
            },
        },
    ];
    let report = revisions.report(
        &[revisions.change(NESTED, ChangeKind::Modified)],
        &publications,
    );
    let mut chosen: Vec<(String, Confidence)> = of_kind(&report, ImpactKind::Publication)
        .iter()
        .map(|impact| (impact.publication.clone().unwrap(), impact.confidence))
        .collect();
    chosen.sort();
    chosen.dedup();
    assert_eq!(
        chosen,
        [
            ("book-1".to_owned(), Confidence::Confirmed),
            ("elsewhere".to_owned(), Confidence::Potential),
            ("one-page".to_owned(), Confidence::Confirmed),
        ]
    );
}

#[test]
fn impact_handles_cycles_and_truncation() {
    let revisions = Revisions::new();
    // intro includes nested, nested now includes intro back.
    revisions.write(NESTED, "A loop.\n\ninclude::partial$intro.adoc[]\n");
    let report = revisions.report(&[revisions.change(NESTED, ChangeKind::Modified)], &[]);
    assert_eq!(
        paths(&of_kind(&report, ImpactKind::Render)),
        [INDEX, INSTALL]
    );

    let head = state(&revisions.root);
    let roots = [revisions.root.clone()];
    let cut = analyse(
        &[revisions.change(NESTED, ChangeKind::Modified)],
        Some(&DependencySnapshot::borrowed(&revisions.base, &roots)),
        &DependencySnapshot::borrowed(&head, &roots),
        &[],
        Limits {
            depth: 32,
            impacts: 1,
        },
    );
    assert_eq!(cut.impacts.len(), 1);
    let truncated = cut.truncated.unwrap();
    assert_eq!((truncated.shown, truncated.total), (1, 2));
    assert_eq!(cut.counts.render, 2, "counted before the cut");
    let shallow = analyse(
        &[revisions.change(NESTED, ChangeKind::Modified)],
        Some(&DependencySnapshot::borrowed(&revisions.base, &roots)),
        &DependencySnapshot::borrowed(&head, &roots),
        &[],
        Limits {
            depth: 1,
            impacts: 100,
        },
    );
    assert!(
        of_kind(&shallow, ImpactKind::Render).is_empty(),
        "one include deep reaches no page"
    );
    // #542: and the report says where the walk stopped.
    let Completeness::Partial { gaps } = &shallow.completeness else {
        panic!("{:?}", shallow.completeness);
    };
    assert_eq!(gaps[0].reason, "include-depth-exceeded");
    assert_eq!(gaps[0].affected, [INTRO]);
}

#[test]
fn impact_missing_remote_components_reports_partial() {
    let revisions = Revisions::new();
    // A changed image, and a file of a component no revision holds: neither is called
    // unaffected.
    revisions.write("v1/modules/ROOT/images/diagram.png", "changed\n");
    revisions.write("remote/modules/ROOT/pages/elsewhere.adoc", "= Elsewhere\n");
    let report = revisions.report(
        &[
            revisions.change("v1/modules/ROOT/images/diagram.png", ChangeKind::Modified),
            revisions.change(
                "remote/modules/ROOT/pages/elsewhere.adoc",
                ChangeKind::Added,
            ),
        ],
        &[],
    );
    let Completeness::Partial { gaps } = &report.completeness else {
        panic!("{:?}", report.completeness);
    };
    let reasons: Vec<&str> = gaps.iter().map(|gap| gap.reason).collect();
    assert!(
        reasons.contains(&"asset-consumers-not-analysed"),
        "{reasons:?}"
    );
    assert!(reasons.contains(&"not-in-catalog"), "{reasons:?}");
}

#[test]
fn impact_removed_partial_consumers_come_from_base() {
    let revisions = Revisions::new();
    revisions.remove(NESTED);
    revisions.write(INTRO, "An introduction.\n");
    let report = revisions.report(
        &[
            revisions.change(NESTED, ChangeKind::Removed),
            revisions.change(INTRO, ChangeKind::Modified),
        ],
        &[],
    );
    let from_nested: Vec<&Impact> = of_kind(&report, ImpactKind::Render)
        .into_iter()
        .filter(|impact| impact.origin == NESTED)
        .collect();
    assert_eq!(paths(&from_nested), [INDEX, INSTALL]);
    assert!(from_nested
        .iter()
        .all(|impact| impact.evidence == EvidenceSide::Base));
}

#[test]
fn impact_moved_page_finds_base_references() {
    let revisions = Revisions::new();
    let moved = "v1/modules/ROOT/pages/setup.adoc";
    let text = fs::read_to_string(revisions.path(INSTALL)).unwrap();
    revisions.remove(INSTALL);
    revisions.write(moved, &text);
    let report = revisions.report(
        &[revisions.change(
            moved,
            ChangeKind::Renamed {
                from: revisions.path(INSTALL),
            },
        )],
        &[],
    );
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    let statuses: Vec<(&str, Option<ImpactStatus>)> = references
        .iter()
        .map(|impact| (impact.path.as_str(), impact.status))
        .collect();
    assert_eq!(
        statuses,
        [
            (GUIDE, Some(ImpactStatus::Broken)),
            (INDEX, Some(ImpactStatus::Broken)),
        ]
    );
    assert!(references
        .iter()
        .all(|impact| impact.evidence == EvidenceSide::Both));
    // The navigation entry to the old place is broken too.
    let navigation = of_kind(&report, ImpactKind::Navigation);
    assert!(navigation
        .iter()
        .any(|impact| impact.path == NAV && impact.status == Some(ImpactStatus::Broken)));

    // With a `page-aliases` entry for the old name, the references resolve through it.
    let revisions = Revisions::new();
    revisions.remove(INSTALL);
    revisions.write(
        moved,
        &text.replacen(
            "= Installing\n",
            "= Installing\n:page-aliases: install.adoc\n",
            1,
        ),
    );
    let report = revisions.report(
        &[revisions.change(
            moved,
            ChangeKind::Renamed {
                from: revisions.path(INSTALL),
            },
        )],
        &[],
    );
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    assert!(!references.is_empty());
    assert!(
        references
            .iter()
            .all(|impact| impact.status == Some(ImpactStatus::ResolvedByAlias)),
        "{references:?}"
    );
}

#[test]
fn impact_explicit_removed_path_is_partial() {
    let revisions = Revisions::new();
    revisions.remove(NESTED);
    let head = state(&revisions.root);
    let roots = [revisions.root.clone()];
    let report = analyse(
        &[revisions.change(NESTED, ChangeKind::Removed)],
        None,
        &DependencySnapshot::borrowed(&head, &roots),
        &[],
        Limits::default(),
    );
    let Completeness::Partial { gaps } = &report.completeness else {
        panic!("{:?}", report.completeness);
    };
    assert_eq!(gaps[0].reason, "no-base-revision");
    assert_eq!(gaps[0].affected, [NESTED]);
    assert!(report.impacts.is_empty(), "no consumer invented");
}

#[test]
fn impact_report_is_deterministic() {
    let revisions = Revisions::new();
    revisions.write(NESTED, "Changed.\n");
    revisions.write(NAV, "* xref:index.adoc[]\n* xref:guide.adoc[]\n");
    let changes = [
        revisions.change(NAV, ChangeKind::Modified),
        revisions.change(NESTED, ChangeKind::Modified),
    ];
    let first = serde_json::to_string_pretty(&revisions.report(&changes, &[])).unwrap();
    let mut reversed = changes.to_vec();
    reversed.reverse();
    let second = serde_json::to_string_pretty(&revisions.report(&reversed, &[])).unwrap();
    assert_eq!(first, second);
    assert!(
        !first.contains(revisions.root.to_string_lossy().as_ref()),
        "no absolute path"
    );
    insta::assert_snapshot!("impact_report", first);
}

fn book_and_nav_profiles(revisions: &Revisions) -> [Publication; 2] {
    [
        Publication {
            name: "book-1".into(),
            selection: PublicationSelection::Component {
                name: "product".into(),
                version: Some("1.0".into()),
            },
        },
        Publication {
            name: "nav-book".into(),
            selection: PublicationSelection::Navigation(vec![revisions.path(NAV)]),
        },
    ]
}

/// Each publication impact as (profile, path).
fn selected(report: &ImpactReport) -> Vec<(String, String)> {
    let mut selected: Vec<(String, String)> = of_kind(report, ImpactKind::Publication)
        .iter()
        .map(|impact| (impact.publication.clone().unwrap(), impact.path.clone()))
        .collect();
    selected.sort();
    selected.dedup();
    selected
}

fn profiles(report: &ImpactReport) -> Vec<String> {
    let mut names: Vec<String> = selected(report).into_iter().map(|(name, _)| name).collect();
    names.dedup();
    names
}

#[test]
fn impact_removed_page_selects_its_component_profile() {
    // #537: the page is gone from head, so whether a profile selected it is base's to say.
    let revisions = Revisions::new();
    revisions.remove(GUIDE);
    let report = revisions.report(
        &[revisions.change(GUIDE, ChangeKind::Removed)],
        &book_and_nav_profiles(&revisions),
    );
    assert_eq!(
        selected(&report),
        [("book-1".to_owned(), GUIDE.to_owned())],
        "{:#?}",
        report.impacts
    );
}

#[test]
fn impact_navigation_profile_follows_its_changed_file() {
    // #537: the file drops an entry, so the profile built from it loses a page.
    let revisions = Revisions::new();
    revisions.write(NAV, "* xref:index.adoc[]\n");
    let report = revisions.report(
        &[revisions.change(NAV, ChangeKind::Modified)],
        &book_and_nav_profiles(&revisions),
    );
    assert_eq!(profiles(&report), ["book-1", "nav-book"]);

    // Reordered, the same pages listed: the order is the profile's and the navigation's.
    let revisions = Revisions::new();
    revisions.write(NAV, "* xref:install.adoc[]\n* xref:index.adoc[]\n");
    let report = revisions.report(
        &[revisions.change(NAV, ChangeKind::Modified)],
        &book_and_nav_profiles(&revisions),
    );
    let navigation = of_kind(&report, ImpactKind::Navigation);
    assert_eq!(paths(&navigation), [NAV], "{navigation:#?}");
    assert_eq!(navigation[0].evidence, EvidenceSide::Both);
    assert_eq!(profiles(&report), ["book-1", "nav-book"]);
}

#[test]
fn impact_renamed_page_is_selected_on_both_sides() {
    // #537: selected under its old path in base and its new one in head.
    let revisions = Revisions::new();
    let moved = "v1/modules/ROOT/pages/setup.adoc";
    let text = revisions.read(INSTALL);
    revisions.remove(INSTALL);
    revisions.write(moved, &text);
    let report = revisions.report(
        &[revisions.change(
            moved,
            ChangeKind::Renamed {
                from: revisions.path(INSTALL),
            },
        )],
        &book_and_nav_profiles(&revisions),
    );
    let book: Vec<String> = selected(&report)
        .into_iter()
        .filter(|(name, _)| name == "book-1")
        .map(|(_, path)| path)
        .collect();
    assert!(book.contains(&INSTALL.to_owned()), "{book:?}");
    assert!(book.contains(&moved.to_owned()), "{book:?}");
}

#[test]
fn impact_dropped_page_alias_breaks_its_references() {
    // #540: the page stays where it is, and the name a reference used is gone.
    let mut revisions = Revisions::new();
    let text = revisions.read(INSTALL);
    revisions.write(
        INSTALL,
        &text.replacen(
            "= Installing\n",
            "= Installing\n:page-aliases: old-install.adoc\n",
            1,
        ),
    );
    revisions.write(GUIDE, "= Guide\n\nRead xref:old-install.adoc[] first.\n");
    revisions.rebase();
    revisions.write(INSTALL, &text);
    let report = revisions.report(&[revisions.change(INSTALL, ChangeKind::Modified)], &[]);
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    assert_eq!(paths(&references), [GUIDE], "{references:#?}");
    assert_eq!(references[0].status, Some(ImpactStatus::Broken));
    assert_eq!(references[0].evidence, EvidenceSide::Both);
    assert_eq!(
        spanned(&revisions, references[0]),
        "xref:old-install.adoc[]"
    );
}

const DESCRIPTOR: &str = "v1/antora.yml";

#[test]
fn impact_new_version_breaks_versioned_references() {
    // #538: every resource of the version has a new coordinate.
    let mut revisions = Revisions::new();
    let other = "v2/modules/ROOT/pages/index.adoc";
    revisions.write(
        other,
        "= Product 2\n\nSee xref:install.adoc[].\n\nOr xref:1.0@product:ROOT:install.adoc[the old steps].\n",
    );
    revisions.rebase();
    revisions.write(
        DESCRIPTOR,
        &revisions.read(DESCRIPTOR).replace("'1.0'", "'1.1'"),
    );
    let report = revisions.report(
        &[revisions.change(DESCRIPTOR, ChangeKind::Modified)],
        &book_and_nav_profiles(&revisions),
    );
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    assert_eq!(paths(&references), [other], "{references:#?}");
    assert_eq!(references[0].status, Some(ImpactStatus::Broken));
    assert_eq!(
        spanned(&revisions, references[0]),
        "xref:1.0@product:ROOT:install.adoc[the old steps]"
    );
    assert!(
        of_kind(&report, ImpactKind::Navigation)
            .iter()
            .any(|impact| impact.path == DESCRIPTOR),
        "{:#?}",
        report.impacts
    );
    assert!(profiles(&report).contains(&"book-1".to_owned()));
}

#[test]
fn impact_changed_version_attributes_are_not_complete_and_empty() {
    // #538: which pages read an attribute is not known, so the report says so.
    let revisions = Revisions::new();
    revisions.write(
        DESCRIPTOR,
        &format!(
            "{}asciidoc:\n  attributes:\n    product-name: Widget\n",
            revisions.read(DESCRIPTOR)
        ),
    );
    let report = revisions.report(&[revisions.change(DESCRIPTOR, ChangeKind::Modified)], &[]);
    let Completeness::Partial { gaps } = &report.completeness else {
        panic!("{report:#?}");
    };
    assert_eq!(gaps[0].reason, "attribute-consumers-not-analysed");
    assert_eq!(gaps[0].affected, [DESCRIPTOR]);
}

const STEPS: &str = "v1/modules/ROOT/partials/steps.adoc";
const INSTALL_WITH_STEPS: &str =
    "= Installing\n\ninclude::partial$intro.adoc[]\n\ninclude::partial$steps.adoc[]\n";

#[test]
fn impact_anchor_moved_into_an_included_partial_is_kept() {
    // #539: the anchor is still on the page as composed.
    let revisions = Revisions::new();
    revisions.write(INSTALL, INSTALL_WITH_STEPS);
    revisions.write(STEPS, "[#setup]\n== Setting up\n\nSteps.\n");
    let report = revisions.report(
        &[
            revisions.change(INSTALL, ChangeKind::Modified),
            revisions.change(STEPS, ChangeKind::Added),
        ],
        &[],
    );
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    assert!(references.is_empty(), "{references:#?}");
}

#[test]
fn impact_anchor_removed_from_an_included_partial_is_reviewed() {
    // #539: the page the partial is composed into loses the anchor.
    let mut revisions = Revisions::new();
    revisions.write(INSTALL, INSTALL_WITH_STEPS);
    revisions.write(STEPS, "[#setup]\n== Setting up\n\nSteps.\n");
    revisions.rebase();
    revisions.write(STEPS, "== Getting ready\n\nSteps.\n");
    let report = revisions.report(&[revisions.change(STEPS, ChangeKind::Modified)], &[]);
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    assert_eq!(paths(&references), [INDEX], "{references:#?}");
    assert_eq!(references[0].anchor.as_deref(), Some("setup"));
    assert_eq!(references[0].status, Some(ImpactStatus::Broken));
    assert_eq!(references[0].origin, STEPS);
    assert_eq!(
        spanned(&revisions, references[0]),
        "xref:install.adoc#setup[setting up]"
    );
}

/// The text `source` spans in the file as it is now.
fn spanned(revisions: &Revisions, impact: &Impact) -> String {
    let source = impact.source.as_ref().expect("a source span");
    let text = revisions.read(&source.path);
    text[source.range.start..source.range.end].to_owned()
}

#[test]
fn impact_include_moved_in_its_consumer_is_one_impact() {
    // #536: the consumer's text shifted, so its include sits at another byte in head.
    let revisions = Revisions::new();
    revisions.write(
        INTRO,
        "An introduction, changed.\n\ninclude::partial$nested.adoc[]\n",
    );
    revisions.write(
        INDEX,
        &revisions
            .read(INDEX)
            .replacen("= Product\n", "= Product\n\nA new paragraph.\n", 1),
    );
    let report = revisions.report(
        &[
            revisions.change(INTRO, ChangeKind::Modified),
            revisions.change(INDEX, ChangeKind::Modified),
        ],
        &[],
    );
    let from_intro: Vec<&Impact> = of_kind(&report, ImpactKind::Render)
        .into_iter()
        .filter(|impact| impact.origin == INTRO)
        .collect();
    assert_eq!(paths(&from_intro), [INDEX, INSTALL], "{from_intro:#?}");
    assert_eq!(from_intro[0].evidence, EvidenceSide::Both);
    assert_eq!(
        spanned(&revisions, from_intro[0]),
        "include::partial$intro.adoc[]"
    );
}

#[test]
fn impact_anchor_reference_moved_in_its_consumer_is_one_impact() {
    // #536: the referrer's text shifted, so its xref sits at another byte in head.
    let revisions = Revisions::new();
    revisions.write(
        INSTALL,
        "= Installing\n\ninclude::partial$intro.adoc[]\n\n== Getting ready\n\nSteps.\n",
    );
    revisions.write(
        INDEX,
        &revisions
            .read(INDEX)
            .replacen("= Product\n", "= Product\n\nA new paragraph.\n", 1),
    );
    let report = revisions.report(
        &[
            revisions.change(INSTALL, ChangeKind::Modified),
            revisions.change(INDEX, ChangeKind::Modified),
        ],
        &[],
    );
    let references = of_kind(&report, ImpactKind::ReferenceIntegrity);
    assert_eq!(paths(&references), [INDEX], "{references:#?}");
    assert_eq!(references[0].evidence, EvidenceSide::Both);
    assert_eq!(references[0].anchor.as_deref(), Some("setup"));
    assert_eq!(
        spanned(&revisions, references[0]),
        "xref:install.adoc#setup[setting up]"
    );
}

#[test]
fn impact_two_references_with_one_target_are_two_impacts() {
    // #542: each reference keeps its own place.
    let mut revisions = Revisions::new();
    revisions.write(
        GUIDE,
        "= Guide\n\nRead xref:install.adoc[] first.\n\nThen xref:install.adoc[] again.\n",
    );
    revisions.rebase();
    let moved = "v1/modules/ROOT/pages/setup.adoc";
    let text = revisions.read(INSTALL);
    revisions.remove(INSTALL);
    revisions.write(moved, &text);
    let report = revisions.report(
        &[revisions.change(
            moved,
            ChangeKind::Renamed {
                from: revisions.path(INSTALL),
            },
        )],
        &[],
    );
    let from_guide: Vec<&Impact> = of_kind(&report, ImpactKind::ReferenceIntegrity)
        .into_iter()
        .filter(|impact| impact.path == GUIDE)
        .collect();
    assert_eq!(from_guide.len(), 2, "{from_guide:#?}");
    let starts: Vec<usize> = from_guide
        .iter()
        .map(|impact| impact.source.as_ref().unwrap().range.start)
        .collect();
    assert_ne!(starts[0], starts[1]);
    for impact in from_guide {
        assert_eq!(impact.status, Some(ImpactStatus::Broken));
        assert_eq!(spanned(&revisions, impact), "xref:install.adoc[]");
    }
}

#[test]
fn impact_outside_antora_follows_includes() {
    // #541: in a plain folder every indexed AsciiDoc file is a page.
    let revisions = Revisions::of("includes");
    revisions.write("partials/intro.adoc", "Changed content.\n");
    let report = revisions.report(
        &[revisions.change("partials/intro.adoc", ChangeKind::Modified)],
        &[],
    );
    assert!(report.is_complete(), "{:#?}", report.completeness);
    let render = of_kind(&report, ImpactKind::Render);
    let index = render
        .iter()
        .find(|impact| impact.path == "index.adoc")
        .unwrap_or_else(|| panic!("{render:#?}"));
    assert_eq!(index.via, ["partials/intro.adoc", "index.adoc"]);
    assert_eq!(index.evidence, EvidenceSide::Both);
}

#[test]
fn impact_ignores_files_no_component_could_hold() {
    // #542: a changed README or CI file is not documentation, and leaves the report complete.
    let revisions = Revisions::new();
    revisions.write("README.md", "# Read me\n");
    revisions.write(".github/workflows/ci.yml", "on: push\n");
    let report = revisions.report(
        &[
            revisions.change("README.md", ChangeKind::Added),
            revisions.change(".github/workflows/ci.yml", ChangeKind::Added),
        ],
        &[],
    );
    assert!(report.is_complete(), "{:#?}", report.completeness);
    assert!(report.impacts.is_empty(), "{:#?}", report.impacts);
}

#[test]
fn impact_cut_keeps_references_to_review_first() {
    // #542: a render impact is cut before a broken reference.
    let revisions = Revisions::new();
    revisions.write(
        INSTALL,
        "= Installing\n\ninclude::partial$intro.adoc[]\n\n== Getting ready\n\nSteps.\n",
    );
    let report = revisions.report_within(
        &[revisions.change(INSTALL, ChangeKind::Modified)],
        &[],
        Limits {
            depth: 32,
            impacts: 1,
        },
    );
    assert_eq!(report.counts.render, 1);
    assert_eq!(report.impacts.len(), 1);
    assert_eq!(report.impacts[0].kind, ImpactKind::ReferenceIntegrity);
}

#[test]
fn impact_now_resolves_only_what_base_left_unresolved() {
    // #542: two new pages linking to each other were not unresolved before.
    let revisions = Revisions::new();
    let one = "v1/modules/ROOT/pages/one.adoc";
    let two = "v1/modules/ROOT/pages/two.adoc";
    revisions.write(one, "= One\n\nSee xref:two.adoc[].\n");
    revisions.write(two, "= Two\n\nSee xref:one.adoc[].\n");
    let report = revisions.report(
        &[
            revisions.change(one, ChangeKind::Added),
            revisions.change(two, ChangeKind::Added),
        ],
        &[],
    );
    assert!(
        !report
            .impacts
            .iter()
            .any(|impact| impact.status == Some(ImpactStatus::NowResolves)),
        "{:#?}",
        report.impacts
    );

    // A reference base held and could not resolve does.
    let mut revisions = Revisions::new();
    revisions.write(GUIDE, "= Guide\n\nRead xref:one.adoc[] first.\n");
    revisions.rebase();
    revisions.write(one, "= One\n");
    let report = revisions.report(&[revisions.change(one, ChangeKind::Added)], &[]);
    let resolving: Vec<&str> = report
        .impacts
        .iter()
        .filter(|impact| impact.status == Some(ImpactStatus::NowResolves))
        .map(|impact| impact.path.as_str())
        .collect();
    assert_eq!(resolving, [GUIDE]);
}

#[test]
fn impact_navigation_lists_only_files_its_side_holds() {
    // #542: head no longer holds the guide, still on disk; its entry, which resolves relative
    // to the navigation file's folder, does not list it.
    let mut revisions = Revisions::new();
    revisions.write(
        NAV,
        "* xref:index.adoc[]\n* xref:install.adoc[]\n* xref:pages/guide.adoc[]\n",
    );
    revisions.rebase();
    let mut head = revisions.base.clone();
    head.remove_document(&revisions.path(GUIDE));
    head.antora.remove_source(&revisions.path(GUIDE));
    let roots = [revisions.root.clone()];
    let report = analyse(
        &[revisions.change(NAV, ChangeKind::Modified)],
        Some(&DependencySnapshot::borrowed(&revisions.base, &roots)),
        &DependencySnapshot::borrowed(&head, &roots),
        &[],
        Limits::default(),
    );
    let unlisted: Vec<&str> = of_kind(&report, ImpactKind::Navigation)
        .iter()
        .filter(|impact| impact.status == Some(ImpactStatus::Unlisted))
        .map(|impact| impact.path.as_str())
        .collect();
    assert_eq!(unlisted, [GUIDE]);
}
