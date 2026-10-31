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
    assert_eq!(repositories[0].roots, std::slice::from_ref(&root));
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

/// A repository with a page of ten numbered lines, open in an engine over it.
fn ten_lines(dir: &Path) -> (TestRepo, Engine, adoc_engine::DocumentId, String) {
    let test = TestRepo::init(dir);
    let text: String = (1..=10).map(|number| format!("line {number}\n")).collect();
    test.write("page.adoc", &text);
    test.commit_all("first");
    let mut engine = engine();
    engine.open_workspace(vec![dir.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&dir.join("page.adoc")).unwrap();
    (test, engine, id, text)
}

fn replace(engine: &mut Engine, id: adoc_engine::DocumentId, find: &str, with: &str) {
    let text = engine.document(id).unwrap().text.to_string();
    let start = text.find(find).unwrap();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(start, start + find.len()),
                text: with.to_owned(),
            },
        )
        .unwrap();
}

fn text_of(engine: &Engine, id: adoc_engine::DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

#[test]
fn revert_hunk_restores_head_text_as_one_undo() {
    let dir = tempfile::tempdir().unwrap();
    let (_test, mut engine, id, original) = ten_lines(dir.path());
    replace(&mut engine, id, "line 3\n", "line three\nand more\n");
    replace(&mut engine, id, "line 8\n", "");
    let edited = text_of(&engine, id);

    // Any offset inside the first change reverts it, and only it.
    let inside = edited.find("and more").unwrap();
    engine
        .invoke(
            ids::GIT_REVERT_HUNK,
            CommandArgs::Offset { id, offset: inside },
        )
        .unwrap();
    let reverted = text_of(&engine, id);
    assert_eq!(reverted, original.replace("line 8\n", ""));

    // The deletion reverts from the line after it.
    let after = reverted.find("line 9").unwrap();
    engine
        .invoke(
            ids::GIT_REVERT_HUNK,
            CommandArgs::Offset { id, offset: after },
        )
        .unwrap();
    assert_eq!(text_of(&engine, id), original);

    // Each revert is one undo step.
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text_of(&engine, id), reverted);
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text_of(&engine, id), edited);

    // Outside any change there is nothing to revert.
    assert!(engine
        .invoke(ids::GIT_REVERT_HUNK, CommandArgs::Offset { id, offset: 0 })
        .is_err());
}

#[test]
fn next_change_wraps_around() {
    let dir = tempfile::tempdir().unwrap();
    let (_test, mut engine, id, _) = ten_lines(dir.path());
    replace(&mut engine, id, "line 2\n", "line two\n");
    replace(&mut engine, id, "line 7\n", "line seven\n");
    let text = text_of(&engine, id);
    let two = text.find("line two").unwrap();
    let seven = text.find("line seven").unwrap();

    let go = |engine: &mut Engine, command, offset| match engine
        .invoke(command, CommandArgs::Offset { id, offset })
        .unwrap()
    {
        adoc_engine::CommandOutcome::Location(location) => location.range.start,
        other => panic!("expected a location, got {other:?}"),
    };
    assert_eq!(go(&mut engine, ids::GIT_NEXT_CHANGE, 0), two);
    assert_eq!(go(&mut engine, ids::GIT_NEXT_CHANGE, two), seven);
    assert_eq!(
        go(&mut engine, ids::GIT_NEXT_CHANGE, seven),
        two,
        "wraps to the first"
    );
    assert_eq!(go(&mut engine, ids::GIT_PREVIOUS_CHANGE, seven), two);
    assert_eq!(
        go(&mut engine, ids::GIT_PREVIOUS_CHANGE, two),
        seven,
        "wraps to the last"
    );
    assert_eq!(go(&mut engine, ids::GIT_PREVIOUS_CHANGE, text.len()), seven);

    // A document without changes has nowhere to go.
    replace(&mut engine, id, "line two\n", "line 2\n");
    replace(&mut engine, id, "line seven\n", "line 7\n");
    assert!(engine
        .invoke(ids::GIT_NEXT_CHANGE, CommandArgs::Document(id))
        .is_err());
}

/// A docs repository whose playbook builds `main` and the `v*` branches from `docs/`, with
/// `main` unversioned and branch `v1.0` publishing version 1.0. Its page links to version 1.0.
fn versioned_repository(dir: &Path) -> TestRepo {
    let test = TestRepo::init(dir);
    test.write(
        "antora-playbook.yml",
        "site:\n  title: Docs\ncontent:\n  sources:\n  - url: .\n    branches: [main, 'v*']\n    start_path: docs\n",
    );
    test.write("docs/antora.yml", "name: product\nversion: ~\n");
    test.write(
        "docs/modules/ROOT/pages/index.adoc",
        "= Home\n\nSee xref:1.0@product::index.adoc[the 1.0 home].\n",
    );
    test.commit_all("main");
    test.switch_branch("v1.0");
    test.write("docs/antora.yml", "name: product\nversion: '1.0'\n");
    test.commit_all("version 1.0");
    test.checkout("main");
    test
}

fn version_warnings(engine: &Engine, id: adoc_engine::DocumentId) -> Vec<String> {
    engine
        .document(id)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_str() == "adoc.antora.version-not-on-branch")
        .map(|diagnostic| diagnostic.message.clone())
        .collect()
}

#[test]
fn head_change_reloads_descriptors_and_rediagnoses() {
    let dir = tempfile::tempdir().unwrap();
    let test = versioned_repository(dir.path());
    let mut engine = engine();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let page = dir.path().join("docs/modules/ROOT/pages/index.adoc");
    let id = engine.open(&page).unwrap();
    assert!(
        pump_until(&mut engine, |engine| !version_warnings(engine, id)
            .is_empty()),
        "the xref to 1.0 is reported on main"
    );
    assert_eq!(
        version_warnings(&engine, id),
        ["version 1.0 is published from branch v1.0, not main"]
    );
    assert_eq!(
        engine.version_badge(&page).as_deref(),
        Some("product@latest")
    );

    // Let the watcher settle, then check out the version branch in the same folder.
    thread::sleep(Duration::from_millis(100));
    test.checkout("v1.0");
    assert!(
        pump_until(&mut engine, |engine| {
            engine.version_badge(&page).as_deref() == Some("product@1.0")
                && version_warnings(engine, id).is_empty()
        }),
        "the switch reloads the descriptor and clears the warning: {:?}",
        version_warnings(&engine, id)
    );
}

#[test]
fn open_in_version_resolves_same_resource_in_other_root() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-versions");
    let roots: Vec<_> = ["main", "v1", "v2", "next"]
        .iter()
        .map(|root| adoc_index::normalize_path(&fixture.join(root)))
        .collect();
    let mut engine = engine();
    engine.open_workspace(roots.clone()).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let feature = |root: &Path| root.join("modules/ROOT/pages/feature.adoc");
    let id = engine.open(&feature(&roots[2])).unwrap();

    let others: Vec<Option<String>> = engine
        .other_versions(id)
        .into_iter()
        .map(|(version, _)| version)
        .collect();
    // Latest first as Antora orders them: unversioned, then released, then the prerelease.
    assert_eq!(
        others,
        [None, Some("1.0".to_owned()), Some("3.0".to_owned())]
    );
    let outcome = engine
        .invoke(
            ids::NAVIGATE_OPEN_IN_VERSION,
            CommandArgs::Custom(serde_json::json!({ "document": id.get(), "version": "1.0" })),
        )
        .unwrap();
    let adoc_engine::CommandOutcome::Opened(opened) = outcome else {
        panic!("expected a document, got {outcome:?}");
    };
    assert_eq!(engine.document(opened).unwrap().path, feature(&roots[1]));
    assert!(engine
        .invoke(
            ids::NAVIGATE_OPEN_IN_VERSION,
            CommandArgs::Custom(serde_json::json!({ "document": id.get(), "version": "9.9" })),
        )
        .is_err());
}

#[test]
fn blame_is_lazy_and_invalidated_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("page.adoc", "one\ntwo\nthree\n");
    let first = test.commit_all("Write the page");
    let mut engine = engine();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&dir.path().join("page.adoc")).unwrap();
    pump_until(&mut engine, |engine| !engine.has_pending());

    assert!(
        engine.blame_for(id).is_none(),
        "nothing is computed until asked"
    );
    let events = engine.subscribe();
    engine.request_blame(id);
    assert!(pump_until(&mut engine, |engine| engine
        .blame_for(id)
        .is_some()));
    assert!(events_of(&events).contains(&EngineEvent::GitBlameChanged(id)));
    let blame = engine.blame_for(id).unwrap();
    assert_eq!(blame.len(), 3);
    assert!(blame
        .iter()
        .all(|line| line.commit.as_deref() == Some(&first[..7])));

    // An edit is overlaid from the hunks, without computing blame again.
    replace(&mut engine, id, "two\n", "two, edited\n");
    assert!(pump_until(&mut engine, |engine| !engine
        .git()
        .hunks_for(id)
        .is_empty()));
    let blame = engine.blame_for(id).unwrap();
    assert_eq!(blame[1].commit, None);
    assert_eq!(blame[1].summary, "Not committed");
    assert_eq!(blame[2].commit.as_deref(), Some(&first[..7]));

    // Saving forgets it; the next request computes it again.
    engine.save(id).unwrap();
    assert!(engine.blame_for(id).is_none());
}

#[test]
fn remote_links_need_a_known_host_and_a_pushed_branch() {
    let dir = tempfile::tempdir().unwrap();
    let (test, mut engine, id, text) = ten_lines(dir.path());
    let offset = text.find("line 3").unwrap();
    let link = |engine: &mut Engine| {
        engine.invoke(
            ids::NAVIGATE_OPEN_ON_REMOTE,
            CommandArgs::Offset { id, offset },
        )
    };
    assert!(link(&mut engine).is_err(), "no remote");

    test.add_remote("origin", "git@github.com:bovinemagnet/AdocFoundry.git");
    let error = link(&mut engine).unwrap_err().to_string();
    assert!(error.contains("not pushed"), "{error}");

    test.track_remote("origin");
    let outcome = link(&mut engine).unwrap();
    assert_eq!(
        outcome,
        adoc_engine::CommandOutcome::Url(
            "https://github.com/bovinemagnet/AdocFoundry/blob/main/page.adoc?plain=1#L3".to_owned()
        )
    );
    assert_eq!(
        engine
            .invoke(
                ids::NAVIGATE_COPY_REMOTE_LINK,
                CommandArgs::Offset { id, offset }
            )
            .unwrap(),
        outcome
    );
}

/// A repository with an identity and one committed page, open in an engine.
fn committing_repository(dir: &Path) -> (TestRepo, Engine, adoc_engine::DocumentId) {
    let test = TestRepo::init(dir);
    test.set_identity("Ada Writer", "ada@example.com");
    test.write(
        "docs/modules/ROOT/pages/guide.adoc",
        "= The Guide\n\nText.\n",
    );
    test.write("README.md", "readme\n");
    test.commit_all("first");
    let mut engine = engine();
    engine.open_workspace(vec![dir.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine
        .open(&dir.join("docs/modules/ROOT/pages/guide.adoc"))
        .unwrap();
    (test, engine, id)
}

#[test]
fn commit_saves_dirty_documents_first() {
    let dir = tempfile::tempdir().unwrap();
    let (_test, mut engine, id) = committing_repository(dir.path());
    replace(&mut engine, id, "Text.", "Text, edited.");
    assert!(engine.document(id).unwrap().dirty);
    let page = engine.document(id).unwrap().path.clone();

    let outcome = engine
        .invoke(
            ids::GIT_COMMIT,
            CommandArgs::Custom(serde_json::json!({
                "message": "docs: edit the guide",
                "paths": [page.to_str().unwrap()],
            })),
        )
        .unwrap();
    let adoc_engine::CommandOutcome::Committed { commit, saved } = outcome else {
        panic!("expected a commit, got {outcome:?}");
    };
    assert_eq!(
        saved,
        std::slice::from_ref(&page),
        "the outcome says what was saved"
    );
    assert!(!engine.document(id).unwrap().dirty);
    assert_eq!(
        std::fs::read_to_string(&page).unwrap(),
        "= The Guide\n\nText, edited.\n"
    );
    // The commit holds the saved text: nothing differs from `HEAD` any more.
    assert!(pump_until(&mut engine, |engine| engine
        .git()
        .status_for(&page)
        .is_none()
        && engine.git().hunks_for(id).is_empty()));
    let blame = adoc_git::Repository::discover(dir.path())
        .unwrap()
        .unwrap()
        .blame(&page, None)
        .unwrap();
    assert_eq!(blame[2].commit.as_deref(), Some(&commit[..7]));

    // Nothing left to commit.
    assert!(engine
        .invoke(
            ids::GIT_COMMIT,
            CommandArgs::Custom(serde_json::json!({ "message": "again" })),
        )
        .is_err());
}

#[test]
fn save_and_commit_prefills_message() {
    let dir = tempfile::tempdir().unwrap();
    let (_test, mut engine, id) = committing_repository(dir.path());
    replace(&mut engine, id, "Text.", "Text, edited.");
    let outcome = engine
        .invoke(ids::GIT_SAVE_AND_COMMIT, CommandArgs::Document(id))
        .unwrap();
    let page = engine.document(id).unwrap().path.clone();
    assert_eq!(
        outcome,
        adoc_engine::CommandOutcome::CommitDraft {
            message: "docs: update The Guide".to_owned(),
            paths: vec![page.clone()],
        }
    );
    assert!(!engine.document(id).unwrap().dirty, "saved");
    // Staged: the index holds the change, the working tree matches it.
    assert!(pump_until(&mut engine, |engine| engine
        .git()
        .status_for(&page)
        .is_some_and(|status| status.index == GitChange::Modified
            && status.worktree == GitChange::Unmodified)));
}

#[test]
fn stage_all_stages_the_changed_writer_files() {
    let dir = tempfile::tempdir().unwrap();
    let (test, mut engine, _) = committing_repository(dir.path());
    test.write("docs/modules/ROOT/pages/new.adoc", "= New\n");
    test.write("README.md", "readme, changed\n");
    engine.invoke(ids::GIT_REFRESH, CommandArgs::None).unwrap();
    assert!(pump_until(&mut engine, |engine| engine
        .git()
        .status_for(&dir.path().join("README.md"))
        .is_some()));
    engine
        .invoke(ids::GIT_STAGE_ALL_ADOC, CommandArgs::None)
        .unwrap();
    let new = dir.path().join("docs/modules/ROOT/pages/new.adoc");
    assert!(pump_until(&mut engine, |engine| engine
        .git()
        .status_for(&new)
        .is_some_and(|status| status.index == GitChange::Added)));
    let readme = engine
        .git()
        .status_for(&dir.path().join("README.md"))
        .unwrap();
    assert_eq!(readme.index, GitChange::Unmodified, "not a writer's file");
}

#[test]
fn stage_all_ignores_family_names_above_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    // A workspace that itself sits in a folder named like an Antora family.
    let root = dir.path().join("examples");
    let (test, mut engine, _) = committing_repository(&root);
    test.write("README.md", "readme, changed\n");
    engine.invoke(ids::GIT_REFRESH, CommandArgs::None).unwrap();
    assert!(pump_until(&mut engine, |engine| engine
        .git()
        .status_for(&root.join("README.md"))
        .is_some()));
    assert!(
        engine.changed_writer_files().is_empty(),
        "{:?}",
        engine.changed_writer_files()
    );
}

#[test]
fn push_disabled_without_git_executable() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("page.adoc", "one\n");
    test.commit_all("first");
    let config = tempfile::tempdir().unwrap();
    let user = config.path().join("adocfoundry.toml");
    fs::write(&user, "[tools]\ngit = \"/no/such/git\"\n").unwrap();
    let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert!(engine.git_executable().is_none());
    let error = engine
        .invoke(ids::GIT_PUSH, CommandArgs::None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("[tools] git"), "{error}");
}

#[test]
fn typing_does_not_reopen_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path());
    let mut engine = engine();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&dir.path().join("docs/page.adoc")).unwrap();
    pump_until(&mut engine, |engine| !engine.has_pending());
    let opened = adoc_git::testing::discoveries(dir.path());
    assert!(opened > 0);

    for typed in 1..=10 {
        let end = engine.document(id).unwrap().text.len();
        engine
            .invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(end, end),
                    text: "x".to_owned(),
                },
            )
            .unwrap();
        assert!(pump_until(&mut engine, |engine| !engine.has_pending()));
        assert_eq!(engine.git().hunks_for(id).len(), 1, "after {typed} typed");
    }
    assert_eq!(
        adoc_git::testing::discoveries(dir.path()),
        opened,
        "each settle diffs against the HEAD text already read"
    );
}

#[test]
fn blame_is_built_once_until_something_changes() {
    let dir = tempfile::tempdir().unwrap();
    let test = TestRepo::init(dir.path());
    test.write("page.adoc", "one\ntwo\nthree\n");
    test.commit_all("Write the page");
    let mut engine = engine();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&dir.path().join("page.adoc")).unwrap();
    pump_until(&mut engine, |engine| !engine.has_pending());
    engine.request_blame(id);
    assert!(pump_until(&mut engine, |engine| engine
        .blame_for(id)
        .is_some()));

    let first = engine.blame_for(id).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &first,
        &engine.blame_for(id).unwrap()
    ));

    replace(&mut engine, id, "two\n", "two, edited\n");
    assert!(pump_until(&mut engine, |engine| !engine
        .git()
        .hunks_for(id)
        .is_empty()));
    let edited = engine.blame_for(id).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &edited));
    assert_eq!(edited[1].commit, None);
    assert!(std::sync::Arc::ptr_eq(
        &edited,
        &engine.blame_for(id).unwrap()
    ));
}

#[test]
fn a_renamed_page_is_compared_with_its_new_path_in_head() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path());
    let mut engine = engine();
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&dir.path().join("docs/page.adoc")).unwrap();
    pump_until(&mut engine, |engine| !engine.has_pending());
    assert!(engine.git().hunks_for(id).is_empty());

    let renamed = adoc_engine::normalize_path(&dir.path().join("docs/renamed.adoc"));
    let Ok(adoc_engine::CommandOutcome::Preview(preview)) = engine.invoke(
        ids::PAGE_RENAME,
        CommandArgs::Rename {
            id,
            new_path: renamed,
        },
    ) else {
        panic!("no rename preview");
    };
    engine
        .invoke(
            ids::WORKSPACE_APPLY_PREVIEW,
            CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
        )
        .unwrap();

    // `docs/renamed.adoc` is not in `HEAD`: every line is added.
    assert!(
        pump_until(&mut engine, |engine| engine
            .git()
            .hunks_for(id)
            .first()
            .is_some_and(
                |hunk| hunk.kind == HunkKind::Added && hunk.lines == (0..3)
            )),
        "{:?}",
        engine.git().hunks_for(id)
    );
}
