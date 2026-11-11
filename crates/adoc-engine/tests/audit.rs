//! The quality audit kept by the engine: off until asked for, built once on a worker, then kept
//! current file by file and version by version, always matching a fresh audit of the same state.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{mpsc::Receiver, Arc},
    time::{Duration, Instant},
};

use adoc_core::DiagnosticSeverity;
use adoc_core::{
    probe::{self, Probe},
    SourceRange,
};
use adoc_engine::{
    audit::{AuditReport, AuditScope, FindingCode, RuleId},
    commands::ids,
    normalize_path, CommandArgs, Engine, EngineEvent, ManualClock, NewPage, Problem, ProblemFilter,
    ProblemPlace, Settings, INDEX_SETTLE, WATCH_DEBOUNCE,
};

/// How long the watcher must report nothing before a test starts counting.
const QUIET: Duration = Duration::from_millis(300);

fn fixture() -> PathBuf {
    normalize_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/audit"))
}

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

/// A copy of the audit fixture, and an engine over it.
struct Copy {
    _directory: tempfile::TempDir,
    root: PathBuf,
    clock: ManualClock,
    engine: Engine,
    events: Receiver<EngineEvent>,
}

impl Copy {
    fn new(settings: Settings) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        copy_tree(&fixture(), &root);
        let clock = ManualClock::new();
        let mut engine = Engine::with_clock(settings.with_git_opt_in(), Arc::new(clock.clone()));
        let events = engine.subscribe();
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        engine.pump();
        let mut copy = Self {
            _directory: directory,
            root,
            clock,
            engine,
            events,
        };
        copy.quiet();
        copy
    }

    /// Applies what the watcher saw of the copy itself, which FSEvents and Windows can report
    /// after the watch began, until it reports nothing for [`QUIET`]: a test then counts only
    /// the work its own changes cause.
    fn quiet(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut since = Instant::now();
        while since.elapsed() < QUIET && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            self.engine.pump();
            if self.engine.has_pending() {
                self.clock.advance(WATCH_DEBOUNCE);
                self.engine.pump();
                assert!(self.engine.wait_for_workers(Duration::from_secs(10)));
                self.engine.pump();
                since = Instant::now();
            }
        }
        let _ = self.events.try_iter().count();
    }

    fn plain() -> Self {
        Self::new(Settings::load(None, None).unwrap())
    }

    /// The copy with the audit kept, its first build done.
    fn audited() -> Self {
        let mut copy = Self::plain();
        copy.engine.enable_audit();
        assert!(copy.until_audit_changed(), "the first build arrives");
        copy
    }

    /// The copy with the problems and the audit both kept, as the app keeps them, and built.
    fn with_problems(settings: Settings) -> Self {
        let mut copy = Self::new(settings);
        copy.engine.enable_problems();
        copy.engine.enable_audit();
        assert!(copy.settled(), "the problems and the audit arrive");
        copy
    }

    /// Pumps until the audit is built and no work waits, or ten seconds pass.
    fn settled(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.engine.pump();
            if self.engine.audit_builds() > 0 && !self.engine.has_pending() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Pumps until the problems are built and no work waits, or ten seconds pass.
    fn settled_problems(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.engine.pump();
            if !self.engine.problems().is_empty() && !self.engine.has_pending() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// The problems of the file at `relative` with `code`.
    fn problems_with(&self, relative: &str, code: FindingCode) -> Vec<Problem> {
        self.engine
            .problems()
            .get(&normalize_path(&self.root.join(relative)))
            .into_iter()
            .flatten()
            .filter(|problem| problem.code == code)
            .cloned()
            .collect()
    }

    /// Pumps until the audit changes, or ten seconds pass.
    fn until_audit_changed(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.engine.pump();
            if self
                .events
                .try_iter()
                .any(|event| matches!(event, EngineEvent::AuditChanged))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    fn kept(&self) -> AuditReport {
        self.engine
            .audit_report(&AuditScope::Workspace)
            .expect("the audit is built")
    }

    /// The kept audit, after checking it is what auditing the engine's state afresh gives.
    fn checked(&self) -> AuditReport {
        let kept = self.kept();
        assert_eq!(
            kept,
            self.engine.audit_now(AuditScope::Workspace),
            "the kept audit matches a fresh one"
        );
        kept
    }

    /// Replaces the first `old` in the file at `relative` with `new`, in an open buffer, and lets
    /// the index settle.
    fn replace(&mut self, relative: &str, old: &str, new: &str) {
        let id = self.engine.open(&self.root.join(relative)).unwrap();
        self.engine.pump();
        let _ = self.events.try_iter().count();
        let start = self
            .engine
            .document(id)
            .unwrap()
            .text
            .to_string()
            .find(old)
            .unwrap_or_else(|| panic!("`{old}` is not in {relative}"));
        self.engine
            .invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(start, start + old.len()),
                    text: new.to_owned(),
                },
            )
            .unwrap();
        self.clock.advance(INDEX_SETTLE);
    }
}

fn paths_with(report: &AuditReport, code: FindingCode) -> Vec<&str> {
    report
        .findings()
        .iter()
        .filter(|finding| finding.code == code)
        .map(|finding| finding.location.path.as_str())
        .collect()
}

const UNLISTED: FindingCode = FindingCode::Rule(RuleId::UNLISTED_PAGE);
const TITLE: FindingCode = FindingCode::Rule(RuleId::MISSING_TITLE);

#[test]
fn the_audit_is_off_until_asked_for() {
    let mut copy = Copy::plain();
    for _ in 0..20 {
        copy.engine.pump();
    }
    assert_eq!(copy.engine.audit_report(&AuditScope::Workspace), None);
    assert_eq!(copy.engine.audit_builds(), 0);
    assert!(!copy.engine.has_pending(), "no audit work waits");
}

#[test]
fn the_kept_audit_matches_a_fresh_one() {
    let copy = Copy::audited();
    assert_eq!(copy.engine.audit_builds(), 1);
    let report = copy.checked();
    assert_eq!(
        paths_with(&report, UNLISTED),
        ["product/modules/ROOT/pages/orphan.adoc"]
    );
    let scope = AuditScope::Component {
        name: "product".to_owned(),
        version: Some("2.0".to_owned()),
    };
    assert_eq!(
        copy.engine.audit_report(&scope),
        Some(copy.engine.audit_now(scope)),
        "a component's audit is put together from the same parts"
    );
}

#[test]
fn audit_changes_are_incremental() {
    let mut copy = Copy::audited();
    probe::reset();
    copy.replace(
        "product/modules/ROOT/pages/orphan.adoc",
        "= Orphan\n",
        "Orphan\n",
    );
    assert!(copy.until_audit_changed());
    assert_eq!(copy.engine.audit_builds(), 1, "no second whole audit");
    let files = probe::count(Probe::AuditFile);
    assert!(
        (1..=4).contains(&files),
        "only the files the edit can change are audited again, not every file: {files}"
    );
    assert_eq!(
        probe::count(Probe::AuditVersion),
        0,
        "a page edit leaves the navigation's results alone"
    );
    let report = copy.checked();
    assert!(paths_with(&report, TITLE).contains(&"product/modules/ROOT/pages/orphan.adoc"));
}

#[test]
fn closing_a_clean_page_audits_and_parses_nothing() {
    let mut copy = Copy::audited();
    let page = copy.root.join("product/modules/ROOT/pages/orphan.adoc");
    let id = copy.engine.open(&page).unwrap();
    assert!(copy.engine.wait_for_workers(Duration::from_secs(10)));
    copy.engine.pump();
    let _ = copy.events.try_iter().count();
    let builds = copy.engine.audit_builds();
    probe::reset();
    copy.engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(id))
        .unwrap();
    assert!(copy.engine.wait_for_workers(Duration::from_secs(10)));
    copy.engine.pump();
    assert_eq!(probe::count(Probe::Parse), 0, "the index's parse is kept");
    assert_eq!(probe::count(Probe::DiagnoseOpen), 0);
    assert_eq!(probe::count(Probe::AuditFile), 0);
    assert_eq!(probe::count(Probe::AuditVersion), 0);
    assert_eq!(copy.engine.audit_builds(), builds, "no whole audit again");
    // The index reads the page back from its file, as any page not open.
    let entry = copy.engine.workspace().index.file(&page).unwrap();
    assert!(entry.document.text.is_empty());
    assert_eq!(entry.text().unwrap(), fs::read_to_string(&page).unwrap());
    copy.checked();
}

#[test]
fn closing_an_unsaved_page_puts_its_file_back_in_the_audit() {
    let mut copy = Copy::audited();
    copy.replace(
        "product/modules/ROOT/pages/orphan.adoc",
        "= Orphan\n",
        "Orphan\n",
    );
    assert!(copy.until_audit_changed());
    let path = "product/modules/ROOT/pages/orphan.adoc";
    assert!(paths_with(&copy.kept(), TITLE).contains(&path));
    let id = copy
        .engine
        .document_for_path(&copy.root.join(path))
        .unwrap();
    copy.engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(id))
        .unwrap();
    assert!(copy.until_audit_changed());
    assert_eq!(copy.engine.audit_builds(), 1, "no second whole audit");
    let report = copy.checked();
    assert!(!paths_with(&report, TITLE).contains(&path));
}

#[test]
fn a_navigation_edit_audits_its_own_version_again() {
    let mut copy = Copy::audited();
    probe::reset();
    copy.replace(
        "product/modules/ROOT/nav.adoc",
        "* xref:install.adoc[]\n",
        "* xref:install.adoc[]\n* xref:orphan.adoc[]\n",
    );
    assert!(copy.until_audit_changed());
    assert_eq!(copy.engine.audit_builds(), 1);
    assert_eq!(
        probe::count(Probe::AuditVersion),
        1,
        "product 3.0 alone, read from the unsaved buffer"
    );
    let report = copy.checked();
    assert!(
        paths_with(&report, UNLISTED).is_empty(),
        "orphan.adoc is listed now"
    );
}

#[test]
fn a_new_page_is_audited_with_its_version() {
    let mut copy = Copy::audited();
    copy.engine
        .invoke(
            ids::PAGE_NEW,
            CommandArgs::NewPage(NewPage {
                module: copy.root.join("product/modules/ROOT"),
                title: "Added".to_owned(),
                add_to_nav: false,
                file: None,
            }),
        )
        .unwrap();
    assert!(copy.until_audit_changed());
    let deadline = Instant::now() + Duration::from_secs(10);
    while copy.engine.has_pending() && Instant::now() < deadline {
        copy.engine.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    let report = copy.checked();
    assert!(paths_with(&report, UNLISTED).contains(&"product/modules/ROOT/pages/added.adoc"));
}

#[test]
fn the_quality_settings_keep_the_audit() {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("adocfoundry.toml");
    fs::write(
        &user,
        "[quality]\nenabled = true\n\n[quality.rules]\n\"quality.navigation.unlisted-page\" = \"off\"\n",
    )
    .unwrap();
    let mut copy = Copy::new(Settings::load(Some(&user), None).unwrap());
    // Built while the copy settled, or soon after.
    assert!(
        copy.engine.audit_builds() > 0 || copy.until_audit_changed(),
        "kept without being asked for"
    );
    assert!(copy.settled());
    let report = copy.checked();
    assert!(
        paths_with(&report, UNLISTED).is_empty(),
        "the settings' policy applies"
    );
}

const ALT: FindingCode = FindingCode::Rule(RuleId::MISSING_ALT);
const IMAGES: &str = "product/modules/ROOT/pages/images.adoc";

/// Every finding of the kept audit is a problem once: its `quality.*` findings added, the
/// existing diagnostics it aggregates already there and not added again.
#[test]
fn audit_findings_listed_once_in_problems() {
    let copy = Copy::with_problems(Settings::load(None, None).unwrap());
    let report = copy.kept();
    let problems = copy.engine.problems();
    for finding in report.findings() {
        let path = normalize_path(&copy.root.join(&finding.location.path));
        let listed = problems
            .get(&path)
            .into_iter()
            .flatten()
            .filter(|problem| {
                problem.code == finding.code
                    && problem.message == finding.message
                    && match finding.location.range {
                        Some(range) => problem.range == range && !problem.whole_file,
                        None => problem.whole_file,
                    }
            })
            .count();
        assert_eq!(listed, 1, "{finding:?}");
    }
    let rule = |code: &FindingCode| matches!(code, FindingCode::Rule(_));
    assert_eq!(
        problems
            .values()
            .flatten()
            .filter(|problem| rule(&problem.code))
            .count(),
        report
            .findings()
            .iter()
            .filter(|finding| rule(&finding.code))
            .count(),
        "no other quality problem"
    );

    let alt = copy.problems_with(IMAGES, ALT);
    assert_eq!(alt.len(), 2, "the block image and the inline one");
    assert_eq!((alt[0].line, alt[0].column), (3, 1), "where the image is");
    assert_eq!(
        (alt[1].line, alt[1].column),
        (7, 11),
        "where the inline image is"
    );
    assert_eq!(alt[0].severity, DiagnosticSeverity::Warning);
    let unlisted = copy.problems_with("product/modules/ROOT/pages/orphan.adoc", UNLISTED);
    assert!(unlisted[0].whole_file, "about the page, not a place in it");
}

#[test]
fn quality_problems_wait_for_the_audit() {
    let mut copy = Copy::plain();
    copy.engine.enable_problems();
    assert!(copy.settled_problems());
    assert!(
        !copy.engine.problems().is_empty(),
        "the diagnostics are there"
    );
    assert!(copy
        .engine
        .problems()
        .values()
        .flatten()
        .all(|problem| matches!(problem.code, FindingCode::Diagnostic(_))));
}

/// The problems follow the audit's policy: an excluded finding and a rule turned off are not
/// listed, and a change to the settings applies at once.
#[test]
fn excluded_findings_are_not_problems() {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("adocfoundry.toml");
    fs::write(
        &user,
        "[[quality.exclusions]]\nrule = \"quality.navigation.unlisted-page\"\nresource = \"3.0@product:ROOT:orphan.adoc\"\nreason = \"Reached from the release notes\"\n",
    )
    .unwrap();
    let mut copy = Copy::with_problems(Settings::load(Some(&user), None).unwrap());
    assert!(copy
        .problems_with("product/modules/ROOT/pages/orphan.adoc", UNLISTED)
        .is_empty());
    assert_eq!(copy.kept().counts().excluded, 1, "counted in the report");
    assert_eq!(copy.problems_with(IMAGES, ALT).len(), 2);

    fs::write(
        &user,
        "[quality.rules]\n\"quality.image.missing-alt\" = \"off\"\n",
    )
    .unwrap();
    copy.engine.reload_settings().unwrap();
    assert!(copy.settled());
    assert!(copy.problems_with(IMAGES, ALT).is_empty(), "turned off");
    assert_eq!(
        copy.problems_with("product/modules/ROOT/pages/orphan.adoc", UNLISTED)
            .len(),
        1,
        "no longer excluded"
    );
}

/// After an edit the quality problems move with the text, from the buffer.
#[test]
fn quality_problems_follow_edits() {
    let mut copy = Copy::with_problems(Settings::load(None, None).unwrap());
    copy.replace(IMAGES, "= Images\n", "= Images\n\nA new paragraph.\n");
    assert!(copy.settled());
    let alt = copy.problems_with(IMAGES, ALT);
    assert_eq!((alt[0].line, alt[0].column), (5, 1));
    assert_eq!(copy.engine.audit_builds(), 1, "no second whole audit");
    assert_eq!(copy.kept(), copy.engine.audit_now(AuditScope::Workspace));
}

#[test]
fn problems_filter_by_severity_code_and_place() {
    let copy = Copy::with_problems(Settings::load(None, None).unwrap());
    let shown = |filter: &ProblemFilter| -> Vec<(String, FindingCode)> {
        copy.engine
            .filtered_problems(filter)
            .into_iter()
            .flat_map(|(path, problems)| {
                let relative = path
                    .strip_prefix(&copy.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                problems
                    .into_iter()
                    .map(move |problem| (relative.clone(), problem.code))
            })
            .collect()
    };
    assert_eq!(
        shown(&ProblemFilter {
            code: Some(ALT),
            ..ProblemFilter::default()
        }),
        [
            (IMAGES.to_owned(), ALT),
            (IMAGES.to_owned(), ALT),
            ("product/modules/ROOT/partials/snippet.adoc".to_owned(), ALT),
        ]
    );
    let information = shown(&ProblemFilter {
        severity: Some(DiagnosticSeverity::Information),
        ..ProblemFilter::default()
    });
    assert!(!information.is_empty());
    assert!(information.iter().all(|(_, code)| {
        *code == FindingCode::Diagnostic(adoc_core::DiagnosticCode::AntoraResolvedViaAlias)
    }));

    let version = ProblemFilter {
        component: Some("product".to_owned()),
        version: Some(Some("3.0".to_owned())),
        ..ProblemFilter::default()
    };
    let in_version = shown(&version);
    assert!(in_version.iter().any(|(path, _)| path == IMAGES));
    assert!(in_version
        .iter()
        .all(|(path, _)| path.starts_with("product/")));
    let admin = shown(&ProblemFilter {
        module: Some("admin".to_owned()),
        ..version.clone()
    });
    assert!(admin
        .iter()
        .all(|(path, _)| path.starts_with("product/modules/admin/")));
    assert!(shown(&ProblemFilter {
        version: Some(Some("2.0".to_owned())),
        code: Some(ALT),
        ..version
    })
    .is_empty());
    assert_eq!(
        shown(&ProblemFilter::default()).len(),
        copy.engine.problem_count()
    );

    assert_eq!(
        copy.engine
            .problem_place(&copy.root.join("product/modules/ROOT/nav.adoc")),
        Some(ProblemPlace {
            component: "product".to_owned(),
            version: Some("3.0".to_owned()),
            module: Some("ROOT".to_owned()),
        })
    );
    assert_eq!(
        copy.engine
            .problem_place(&copy.root.join("guide/antora.yml")),
        Some(ProblemPlace {
            component: "guide".to_owned(),
            version: None,
            module: None,
        }),
        "a descriptor is its component version's, in no module"
    );
}

/// Run Audit keeps the audit and builds it, and builds it again when asked again; meanwhile the
/// last result is stale.
#[test]
fn run_audit_builds_the_audit_then_builds_it_again() {
    let mut copy = Copy::plain();
    assert!(!copy.engine.audit_enabled());
    copy.engine
        .invoke(ids::QUALITY_RUN_AUDIT, CommandArgs::None)
        .unwrap();
    assert!(copy.engine.audit_enabled());
    assert!(!copy.engine.audit_stale(), "nothing to be stale yet");
    assert!(copy.until_audit_changed());
    assert_eq!(copy.engine.audit_builds(), 1);
    assert!(!copy.engine.audit_stale());

    copy.engine
        .invoke(ids::QUALITY_RUN_AUDIT, CommandArgs::None)
        .unwrap();
    assert!(
        copy.engine.audit_stale(),
        "the result shown is older than asked for"
    );
    assert!(copy.until_audit_changed());
    assert!(copy.settled());
    assert_eq!(copy.engine.audit_builds(), 2);
    assert!(!copy.engine.audit_stale());
}
