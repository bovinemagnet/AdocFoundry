//! The engine's impact analysis (Impact PRD §§6 and 11): change sources turned into the changes
//! and both sides' snapshots, from Git object data for revisions, on a temporary repository
//! holding `tests/fixtures/impact`.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids,
    impact::{Completeness, EvidenceSide, ImpactKind, ImpactReport, ImpactStatus, Limits},
    normalize_path, ChangeSource, CommandArgs, Engine, ImpactError, Settings,
};
use adoc_git::testing::TestRepo;

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

const NESTED: &str = "v1/modules/ROOT/partials/nested.adoc";
const INTRO: &str = "v1/modules/ROOT/partials/intro.adoc";
const INDEX: &str = "v1/modules/ROOT/pages/index.adoc";
const INSTALL: &str = "v1/modules/ROOT/pages/install.adoc";
const SETUP: &str = "v1/modules/ROOT/pages/setup.adoc";
const GUIDE: &str = "v1/modules/ROOT/pages/guide.adoc";

/// A repository holding the fixture: `base` is its first commit; the second removes `nested`
/// (and `intro`'s include of it) and moves `install` to `setup`.
struct Repo {
    _directory: tempfile::TempDir,
    repo: TestRepo,
    root: PathBuf,
    base: String,
    head: String,
}

impl Repo {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(directory.path());
        let repo = TestRepo::init(&root);
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/impact"),
            &root,
        );
        let base = repo.commit_all("base");
        let install = fs::read_to_string(root.join(INSTALL)).unwrap();
        repo.remove(INSTALL);
        repo.write(SETUP, &install);
        repo.remove(NESTED);
        repo.write(INTRO, "An introduction.\n");
        let head = repo.commit_all("head");
        Self {
            _directory: directory,
            repo,
            root,
            base,
            head,
        }
    }

    fn engine(&self) -> Engine {
        let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
        engine.open_workspace(vec![self.root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        engine
    }

    fn refs(&self) -> ChangeSource {
        ChangeSource::GitRefs {
            base: self.base.clone(),
            head: self.head.clone(),
        }
    }
}

fn of(report: &ImpactReport, kind: ImpactKind, origin: &str) -> Vec<String> {
    report
        .impacts
        .iter()
        .filter(|impact| impact.kind == kind && impact.origin == origin)
        .map(|impact| impact.path.clone())
        .collect()
}

/// Every file beneath `root` but `.git`, and its bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                folders.push(path);
            } else {
                files.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    files
}

#[test]
fn impact_git_refs_are_read_only() {
    let repo = Repo::new();
    let mut engine = repo.engine();
    let before = snapshot(&repo.root);
    let head_file = fs::read(repo.root.join(".git/HEAD")).unwrap();
    let report = engine
        .analyse_impact(&repo.refs(), Limits::default())
        .unwrap();

    // The removed partial's consumers come from base, where the page was still `install`; the
    // moved page's references come from base too.
    assert_eq!(of(&report, ImpactKind::Render, NESTED), [INDEX, INSTALL]);
    let moved: Vec<(String, Option<ImpactStatus>, EvidenceSide)> = report
        .impacts
        .iter()
        .filter(|impact| impact.kind == ImpactKind::ReferenceIntegrity && impact.origin == INSTALL)
        .map(|impact| (impact.path.clone(), impact.status, impact.evidence))
        .collect();
    assert_eq!(
        moved,
        [
            (
                GUIDE.to_owned(),
                Some(ImpactStatus::Broken),
                EvidenceSide::Both
            ),
            (
                INDEX.to_owned(),
                Some(ImpactStatus::Broken),
                EvidenceSide::Both
            ),
        ]
    );
    let changes: Vec<(&str, &str)> = report
        .changes
        .iter()
        .map(|change| (change.kind, change.path.as_str()))
        .collect();
    assert!(changes.contains(&("renamed", SETUP)), "{changes:?}");
    assert!(changes.contains(&("removed", NESTED)), "{changes:?}");
    assert!(report.is_complete(), "{:?}", report.completeness);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["source"]["kind"], "git-refs");
    assert_eq!(json["source"]["base"]["commit"], repo.base);

    // Nothing was checked out, staged or written.
    assert_eq!(snapshot(&repo.root), before);
    assert_eq!(fs::read(repo.root.join(".git/HEAD")).unwrap(), head_file);
}

#[test]
fn impact_ref_mode_ignores_live_index() {
    let repo = Repo::new();
    let clean = repo
        .engine()
        .analyse_impact(&repo.refs(), Limits::default())
        .unwrap();

    // A dirty working tree and an edited, unsaved buffer change nothing in a ref comparison.
    let mut engine = repo.engine();
    repo.repo.write(
        GUIDE,
        "= Guide\n\nNo links now.\n\ninclude::partial$intro.adoc[]\n",
    );
    let index = engine.open(&repo.root.join(INDEX)).unwrap();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id: index,
                range: SourceRange::new(0, 0),
                text: "// unsaved\n".to_owned(),
            },
        )
        .unwrap();
    engine.pump();
    let dirty = engine
        .analyse_impact(&repo.refs(), Limits::default())
        .unwrap();
    assert_eq!(
        serde_json::to_string_pretty(&dirty).unwrap(),
        serde_json::to_string_pretty(&clean).unwrap()
    );

    // Against `HEAD`, the working tree and the buffer are the head side.
    let working = engine
        .analyse_impact(&ChangeSource::WorkingTree, Limits::default())
        .unwrap();
    let changed: Vec<&str> = working
        .changes
        .iter()
        .map(|change| change.path.as_str())
        .collect();
    assert_eq!(changed, [GUIDE, INDEX]);
    let json = serde_json::to_value(&working).unwrap();
    assert_eq!(json["source"]["kind"], "working-tree");
    assert_eq!(json["source"]["unsaved_buffers"], true);
    // The guide now includes `intro`: rendered differently, from head's evidence.
    assert!(of(&working, ImpactKind::Render, GUIDE).contains(&GUIDE.to_owned()));
}

#[test]
fn impact_explicit_removed_path_is_partial() {
    let repo = Repo::new();
    let mut engine = repo.engine();
    let report = engine
        .analyse_impact(
            &ChangeSource::Paths(vec![PathBuf::from(INSTALL), PathBuf::from(INTRO)]),
            Limits::default(),
        )
        .unwrap();
    let Completeness::Partial { gaps } = &report.completeness else {
        panic!("{:?}", report.completeness);
    };
    assert_eq!(gaps[0].reason, "no-base-revision");
    assert_eq!(gaps[0].affected, [INSTALL]);
    // The present path is analysed against the working tree.
    assert_eq!(of(&report, ImpactKind::Render, INTRO), [INDEX, SETUP]);
    assert!(matches!(
        engine.analyse_impact(
            &ChangeSource::Paths(vec![PathBuf::from("/elsewhere/x.adoc")]),
            Limits::default(),
        ),
        Err(ImpactError::OutsideRoots(_))
    ));
}

#[test]
fn an_unknown_ref_or_a_folder_outside_git_is_an_error() {
    let repo = Repo::new();
    let mut engine = repo.engine();
    assert!(matches!(
        engine.analyse_impact(
            &ChangeSource::GitRefs {
                base: "no-such-branch".into(),
                head: "HEAD".into(),
            },
            Limits::default(),
        ),
        Err(ImpactError::UnknownRevision(_))
    ));
    let plain = tempfile::tempdir().unwrap();
    fs::write(plain.path().join("page.adoc"), "= Page\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine
        .open_workspace(vec![normalize_path(plain.path())])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    assert!(matches!(
        engine.analyse_impact(&ChangeSource::WorkingTree, Limits::default()),
        Err(ImpactError::NoRepository(_))
    ));
}
