//! The quality audit kept by the engine: off until asked for, built once on a worker, then kept
//! current file by file and version by version, always matching a fresh audit of the same state.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{mpsc::Receiver, Arc},
    time::{Duration, Instant},
};

use adoc_core::{
    probe::{self, Probe},
    SourceRange,
};
use adoc_engine::{
    audit::{AuditReport, AuditScope, FindingCode, RuleId},
    commands::ids,
    normalize_path, CommandArgs, Engine, EngineEvent, ManualClock, NewPage, Settings, INDEX_SETTLE,
};

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
        Self {
            _directory: directory,
            root,
            clock,
            engine,
            events,
        }
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
    assert!(copy.until_audit_changed(), "kept without being asked for");
    let report = copy.checked();
    assert!(
        paths_with(&report, UNLISTED).is_empty(),
        "the settings' policy applies"
    );
}
