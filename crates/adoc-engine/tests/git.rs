//! Headless engine tests of Git: discovery, status, line changes and branch switches, against
//! temporary repositories built with `gix`.

use std::{
    fs,
    path::Path,
    sync::mpsc::Receiver,
    thread,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineEvent, GitChange, Head, HunkKind, Settings,
};
use adoc_git::testing::TestRepo;

/// Pumps the engine until `done` holds or ten seconds pass.
fn pump_until(engine: &mut Engine, mut done: impl FnMut(&Engine) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        engine.pump();
        if done(engine) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

fn engine() -> Engine {
    Engine::new(Settings::load(None, None).unwrap())
}

/// A repository with one committed page, `docs/page.adoc`.
fn repository(dir: &Path) -> TestRepo {
    let test = TestRepo::init(dir);
    test.write("docs/page.adoc", "= Page\n\nFirst paragraph.\n");
    test.commit_all("first");
    test
}

fn events_of(receiver: &Receiver<EngineEvent>) -> Vec<EngineEvent> {
    receiver.try_iter().collect()
}

#[test]
fn workspace_without_git_has_no_repositories() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("page.adoc"), "= Page\n").unwrap();
    // A temporary directory may itself sit inside a repository on some machines.
    if dir
        .path()
        .ancestors()
        .any(|ancestor| ancestor.join(".git").exists())
    {
        return;
    }
    let mut engine = engine();
    let events = engine.subscribe();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&dir.path().join("page.adoc")).unwrap();
    pump_until(&mut engine, |engine| !engine.has_pending());

    assert!(engine.git().repositories().is_empty());
    assert!(engine.git().hunks_for(id).is_empty());
    assert!(!events_of(&events).iter().any(|event| matches!(
        event,
        EngineEvent::GitHeadChanged { .. }
            | EngineEvent::GitStatusChanged { .. }
            | EngineEvent::GitHunksChanged(_)
    )));
}

#[test]
fn discovery_reports_head_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let test = repository(dir.path());
    test.write("docs/new.adoc", "= New\n");
    let root = dir.path().join("docs");
    let mut engine = engine();
    let events = engine.subscribe();
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));

    let repositories = engine.git().repositories();
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0].workdir, dir.path());
    assert_eq!(repositories[0].roots, [root.clone()]);
    assert_eq!(
        engine.git().head_for(&root),
        Some(&Head::Branch("main".to_owned()))
    );
    let new = engine
        .git()
        .status_for(&dir.path().join("docs/new.adoc"))
        .expect("the new page has a status");
    assert_eq!(new.worktree, GitChange::Untracked);
    assert!(engine
        .git()
        .status_for(&dir.path().join("docs/page.adoc"))
        .is_none());
    let seen = events_of(&events);
    assert!(seen.contains(&EngineEvent::GitHeadChanged {
        root: root.clone(),
        head: Head::Branch("main".to_owned()),
    }));
    assert!(seen.contains(&EngineEvent::GitStatusChanged { root }));
}

#[test]
fn hunks_follow_document_edits() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path());
    let mut engine = engine();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let page = dir.path().join("docs/page.adoc");
    let id = engine.open(&page).unwrap();
    pump_until(&mut engine, |engine| !engine.has_pending());
    assert!(engine.git().hunks_for(id).is_empty(), "nothing changed yet");

    let events = engine.subscribe();
    let text = "= Page\n\nFirst paragraph.\n";
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(text.len(), text.len()),
                text: "\nA new paragraph.\n".to_owned(),
            },
        )
        .unwrap();
    assert!(
        pump_until(&mut engine, |engine| !engine.git().hunks_for(id).is_empty()),
        "the edit is marked without saving"
    );
    assert!(events_of(&events).contains(&EngineEvent::GitHunksChanged(id)));
    let hunks = engine.git().hunks_for(id);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].kind, HunkKind::Added);
    assert_eq!(hunks[0].lines, 3..5);
    assert_eq!(
        hunks[0].range,
        SourceRange::new(text.len(), text.len() + "\nA new paragraph.\n".len())
    );

    // Undoing the edit clears the marks.
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert!(pump_until(&mut engine, |engine| engine
        .git()
        .hunks_for(id)
        .is_empty()));
}

#[test]
fn head_change_emits_event() {
    let dir = tempfile::tempdir().unwrap();
    let test = repository(dir.path());
    let mut engine = engine();
    let events = engine.subscribe();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    // Let the watcher settle before switching.
    thread::sleep(Duration::from_millis(100));
    events_of(&events);

    test.switch_branch("v1.0");
    let mut seen = Vec::new();
    let switched = pump_until(&mut engine, |_| {
        seen.extend(events.try_iter());
        seen.iter().any(|event| {
            matches!(event, EngineEvent::GitHeadChanged { head: Head::Branch(name), .. } if name == "v1.0")
        })
    });
    assert!(switched, "the branch switch is seen: {seen:?}");
    assert_eq!(
        engine.git().head_for(dir.path()),
        Some(&Head::Branch("v1.0".to_owned()))
    );
}

#[test]
fn git_disabled_setting_skips_discovery() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path());
    let config = tempfile::tempdir().unwrap();
    let user = config.path().join("adocfoundry.toml");
    fs::write(&user, "[git]\nenabled = false\n").unwrap();
    let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
    let events = engine.subscribe();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&dir.path().join("docs/page.adoc")).unwrap();
    pump_until(&mut engine, |engine| !engine.has_pending());

    assert!(engine.git().repositories().is_empty());
    assert!(engine.git().hunks_for(id).is_empty());
    assert!(!events_of(&events)
        .iter()
        .any(|event| matches!(event, EngineEvent::GitHeadChanged { .. })));

    // Turning it on finds the repository.
    fs::write(&user, "[git]\nenabled = true\n").unwrap();
    engine.reload_settings().unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert_eq!(engine.git().repositories().len(), 1);
}
