//! Headless engine tests for workspace indexing, the file watcher and descriptor reload.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use adoc_core::DiagnosticCode;
use adoc_engine::{Engine, EngineEvent, Settings};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

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

fn engine_without_user_settings() -> Engine {
    Engine::new(Settings::load(None, None).unwrap().with_git_opt_in())
}

#[test]
fn open_workspace_indexes_fixture() {
    let root = fixture("antora-single-component");
    let mut engine = engine_without_user_settings();
    let events = engine.subscribe();
    engine.open_workspace(vec![root]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));

    let workspace = engine.workspace();
    assert!(workspace.index.files().count() >= 4);
    assert!(workspace.antora.component("demo", Some("latest")).is_some());
    assert!(events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::WorkspaceIndexed { files, .. } if files >= 4)));
    assert_eq!(engine.roots().len(), 1);
}

#[test]
fn open_documents_survive_a_reindex() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let mut engine = engine_without_user_settings();
    let id = engine.open(&page).unwrap();
    engine
        .invoke(
            adoc_engine::commands::ids::EDIT_REPLACE_RANGE,
            adoc_engine::CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(2, 6),
                text: "Edited".to_owned(),
            },
        )
        .unwrap();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let indexed = engine.workspace().index.file(&page).unwrap();
    assert_eq!(indexed.document.text, "= Edited\n");
}

#[test]
fn watcher_reindexes_changed_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.adoc");
    let target = directory.path().join("target.adoc");
    fs::write(&source, "= Source\n\nSee xref:target.adoc#details[].\n").unwrap();
    fs::write(&target, "= Target\n").unwrap();

    let mut engine = engine_without_user_settings();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&source).unwrap();
    let unresolved = |engine: &Engine| {
        engine
            .document(id)
            .unwrap()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::UnresolvedAnchor)
    };
    assert!(unresolved(&engine), "the anchor does not exist yet");

    // Give the watcher a moment to register before writing.
    thread::sleep(Duration::from_millis(100));
    fs::write(&target, "= Target\n\n[[details]]\n== Details\n").unwrap();

    assert!(
        pump_until(&mut engine, |engine| !unresolved(engine)),
        "the watcher re-indexes the changed file and clears the diagnostic"
    );
    let indexed = engine.workspace().index.file(&target).unwrap();
    assert!(indexed
        .document
        .anchors
        .iter()
        .any(|anchor| anchor.id == "details"));
}

#[test]
fn dirty_documents_are_not_overwritten_from_disk() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let mut engine = engine_without_user_settings();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();
    engine
        .invoke(
            adoc_engine::commands::ids::EDIT_INSERT_TEXT,
            adoc_engine::CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(0, 0),
                text: "// mine\n".to_owned(),
            },
        )
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    fs::write(&page, "= Theirs\n").unwrap();

    let mut warned = false;
    assert!(pump_until(&mut engine, |_| {
        warned |= events
            .try_iter()
            .any(|event| event == EngineEvent::DocumentChangedOnDisk(id));
        warned
    }));
    assert_eq!(
        engine.document(id).unwrap().text.to_string(),
        "// mine\n= Page\n"
    );
}

#[test]
fn descriptor_change_reloads_component() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("modules/ROOT/pages")).unwrap();
    fs::write(root.join("antora.yml"), "name: before\nversion: '1.0'\n").unwrap();
    fs::write(root.join("modules/ROOT/pages/index.adoc"), "= Index\n").unwrap();

    let mut engine = engine_without_user_settings();
    let events = engine.subscribe();
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert!(engine
        .workspace()
        .antora
        .component("before", Some("1.0"))
        .is_some());

    thread::sleep(Duration::from_millis(100));
    fs::write(root.join("antora.yml"), "name: after\nversion: '1.0'\n").unwrap();

    assert!(pump_until(&mut engine, |engine| engine
        .workspace()
        .antora
        .component("after", Some("1.0"))
        .is_some()));
    assert!(events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::CatalogChanged { .. })));
}

#[test]
fn open_workspace_rejects_missing_roots() {
    let mut engine = engine_without_user_settings();
    assert!(engine
        .open_workspace(vec![PathBuf::from("/definitely/not/here")])
        .is_err());
}

#[test]
fn typing_does_not_report_indexing() {
    let root = fixture("antora-single-component");
    let mut engine = engine_without_user_settings();
    engine.open_workspace(vec![root.clone()]).unwrap();
    // A whole-workspace index is indexing.
    assert!(engine.is_indexing());
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert!(!engine.is_indexing());

    let page = fs::read_dir(root.join("modules/ROOT/pages"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "adoc")
        })
        .unwrap();
    let id = engine.open(&page).unwrap();
    engine
        .invoke(
            adoc_engine::commands::ids::EDIT_INSERT_TEXT,
            adoc_engine::CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .unwrap();
    // The edit settles a re-parse and a preview, which is pending work but not indexing.
    assert!(engine.has_pending());
    assert!(!engine.is_indexing());
}
