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
    assert_eq!(&*indexed.document.text, "= Edited\n");
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

/// Writes `barrier`, a file of its own, and pumps until the watcher has reported it, so that
/// every change made before it has been seen too; returns how many times the page `id` was
/// reported changed on disk meanwhile. `step` runs each round, to move a manual clock on.
fn reports_until_barrier(
    engine: &mut Engine,
    events: &std::sync::mpsc::Receiver<EngineEvent>,
    id: adoc_engine::DocumentId,
    barrier: &Path,
    mut step: impl FnMut(),
) -> usize {
    thread::sleep(Duration::from_millis(100));
    fs::write(barrier, "= Barrier\n").unwrap();
    let barrier = adoc_index::normalize_path(barrier);
    let mut reported = 0;
    let mut seen = false;
    assert!(pump_until(engine, |_| {
        step();
        for event in events.try_iter() {
            match event {
                EngineEvent::DocumentChangedOnDisk(changed) if changed == id => reported += 1,
                EngineEvent::DiagnosticsUpdated { path } if path == barrier => seen = true,
                _ => {}
            }
        }
        seen
    }));
    reported
}

fn type_at_start(engine: &mut Engine, id: adoc_engine::DocumentId, text: &str) {
    engine
        .invoke(
            adoc_engine::commands::ids::EDIT_INSERT_TEXT,
            adoc_engine::CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(0, 0),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

#[test]
fn own_save_followed_by_typing_is_not_reported() {
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
    type_at_start(&mut engine, id, "// one\n");
    engine
        .invoke(
            adoc_engine::commands::ids::DOCUMENT_SAVE,
            adoc_engine::CommandArgs::Document(id),
        )
        .unwrap();
    // Typing resumes before the watcher reports the save.
    type_at_start(&mut engine, id, "// two\n");

    let barrier = directory.path().join("barrier.adoc");
    assert_eq!(
        reports_until_barrier(&mut engine, &events, id, &barrier, || {}),
        0
    );
    assert_eq!(
        engine.document(id).unwrap().text.to_string(),
        "// two\n// one\n= Page\n"
    );
}

#[test]
fn auto_save_does_not_report_own_write() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    // Opening the workspace reads its settings file.
    fs::write(
        directory.path().join(".adocfoundry.toml"),
        "[editor]\nauto_save = \"after_delay\"\n",
    )
    .unwrap();
    let clock = adoc_engine::ManualClock::new();
    let settings = Settings::load(None, None).unwrap().with_git_opt_in();
    let mut engine = Engine::with_clock(settings, std::sync::Arc::new(clock.clone()));
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();
    type_at_start(&mut engine, id, "// one\n");
    assert!(pump_until(&mut engine, |engine| {
        clock.advance(adoc_engine::AUTO_SAVE_DELAY);
        !engine.document(id).unwrap().dirty
    }));
    assert_eq!(fs::read_to_string(&page).unwrap(), "// one\n= Page\n");
    type_at_start(&mut engine, id, "// two\n");

    let barrier = directory.path().join("barrier.adoc");
    let reported = reports_until_barrier(&mut engine, &events, id, &barrier, || {
        clock.advance(adoc_engine::WATCH_DEBOUNCE / 4);
    });
    assert_eq!(reported, 0);
}

#[test]
fn external_change_is_reported_once() {
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
    type_at_start(&mut engine, id, "// mine\n");

    thread::sleep(Duration::from_millis(100));
    fs::write(&page, "= Theirs\n").unwrap();
    let first = directory.path().join("first.adoc");
    assert_eq!(
        reports_until_barrier(&mut engine, &events, id, &first, || {}),
        1
    );
    fs::write(&page, "= Theirs again\n").unwrap();
    let second = directory.path().join("second.adoc");
    assert_eq!(
        reports_until_barrier(&mut engine, &events, id, &second, || {}),
        0,
        "reported once until the page is saved or reloaded"
    );

    // After a reload, a new change is reported again.
    engine
        .invoke(
            adoc_engine::commands::ids::DOCUMENT_RELOAD,
            adoc_engine::CommandArgs::Document(id),
        )
        .unwrap();
    type_at_start(&mut engine, id, "// mine\n");
    thread::sleep(Duration::from_millis(100));
    fs::write(&page, "= Theirs once more\n").unwrap();
    let third = directory.path().join("third.adoc");
    assert_eq!(
        reports_until_barrier(&mut engine, &events, id, &third, || {}),
        1
    );
}

/// True once the index holds `path`.
fn indexed(engine: &Engine, path: &Path) -> bool {
    engine.workspace().index.file(path).is_some()
}

#[test]
fn opening_a_workspace_neither_watches_nor_indexes_on_the_callers_thread() {
    let directory = tempfile::tempdir().unwrap();
    for page in 0..100 {
        fs::write(
            directory.path().join(format!("page-{page}.adoc")),
            "= Page\n",
        )
        .unwrap();
    }
    let modules = directory.path().join("node_modules");
    for package in 0..20_000 {
        fs::create_dir_all(modules.join(format!("package-{package}"))).unwrap();
    }

    let mut engine = engine_without_user_settings();
    let started = Instant::now();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    let opened = started.elapsed();
    assert!(
        opened < Duration::from_millis(50),
        "open_workspace took {opened:?}"
    );
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    assert_eq!(engine.workspace().index.files().count(), 100);

    // The watcher is in place once the index is: a page written now, beside the others or in a
    // folder made after opening, is still reported.
    let beside = directory.path().join("beside.adoc");
    fs::write(&beside, "= Beside\n").unwrap();
    assert!(pump_until(&mut engine, |engine| indexed(engine, &beside)));
    let folder = directory.path().join("later");
    fs::create_dir(&folder).unwrap();
    assert!(pump_until(&mut engine, |engine| !engine.is_indexing()));
    let nested = folder.join("nested.adoc");
    fs::write(&nested, "= Nested\n").unwrap();
    assert!(
        pump_until(&mut engine, |engine| indexed(engine, &nested)),
        "a page in a new folder is reported"
    );
}

#[test]
fn workspace_ignore_setting_is_honoured_by_the_index() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".adocfoundry.toml"),
        "[workspace]\nignore = [\"vendor\"]\n",
    )
    .unwrap();
    let kept = directory.path().join("kept.adoc");
    fs::write(&kept, "= Kept\n").unwrap();
    let vendor = directory.path().join("vendor/docs");
    fs::create_dir_all(vendor.join("modules/ROOT/pages")).unwrap();
    fs::write(vendor.join("antora.yml"), "name: vendored\nversion: ~\n").unwrap();
    let skipped = vendor.join("modules/ROOT/pages/index.adoc");
    fs::write(&skipped, "= Vendored\n").unwrap();

    let mut engine = engine_without_user_settings();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));

    assert!(indexed(&engine, &kept));
    assert!(
        !indexed(&engine, &skipped),
        "pages under vendor are not indexed"
    );
    assert!(
        engine.workspace().antora.components().next().is_none(),
        "nor are components under it discovered"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_subdirectory_does_not_abort_indexing() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let readable = directory.path().join("readable.adoc");
    fs::write(&readable, "= Readable\n").unwrap();
    let locked = directory.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden.adoc"), "= Hidden\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        // Running as root: nothing is unreadable.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let mut engine = engine_without_user_settings();
    let events = engine.subscribe();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        indexed(&engine, &readable),
        "the rest of the workspace is indexed"
    );
    let events: Vec<_> = events.try_iter().collect();
    assert!(events
        .iter()
        .any(|event| matches!(event, EngineEvent::WorkspaceIndexed { files: 1, .. })));
    assert!(
        events.iter().any(|event| matches!(
            event,
            EngineEvent::CommandFailed { error, .. } if error.contains("locked")
        )),
        "the folder that could not be read is reported: {events:?}"
    );
}

/// The edits of the first `DocumentChanged` of `id` among `events`.
fn changed_edits(
    events: &std::sync::mpsc::Receiver<EngineEvent>,
    id: adoc_engine::DocumentId,
) -> Option<Vec<adoc_engine::TextEdit>> {
    events.try_iter().find_map(|event| match event {
        EngineEvent::DocumentChanged {
            id: changed, edits, ..
        } if changed == id => Some(edits),
        _ => None,
    })
}

const BEFORE: &str = "= Page\n\nFirst.\n\nLast.\n";
const AFTER: &str = "= Page\n\nFirst, changed.\n\nLast.\n";

/// Only the words written in, at the offset they were written at.
fn the_change() -> Vec<adoc_engine::TextEdit> {
    vec![adoc_engine::TextEdit::insert(
        "= Page\n\nFirst".len(),
        ", changed",
    )]
}

#[test]
fn clean_document_follows_disk_with_minimal_edit() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, BEFORE).unwrap();
    let mut engine = engine_without_user_settings();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&page).unwrap();
    let events = engine.subscribe();

    thread::sleep(Duration::from_millis(100));
    fs::write(&page, AFTER).unwrap();
    let mut edits = None;
    assert!(pump_until(&mut engine, |_| {
        edits = edits.take().or_else(|| changed_edits(&events, id));
        edits.is_some()
    }));
    assert_eq!(edits.unwrap(), the_change());
    let document = engine.document(id).unwrap();
    assert_eq!(document.text.to_string(), AFTER);
    assert!(!document.dirty);

    engine
        .invoke(
            adoc_engine::commands::ids::EDIT_UNDO,
            adoc_engine::CommandArgs::Document(id),
        )
        .unwrap();
    assert_eq!(engine.document(id).unwrap().text.to_string(), BEFORE);
}

#[test]
fn reload_replaces_only_what_changed() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, BEFORE).unwrap();
    let mut engine = engine_without_user_settings();
    let id = engine.open(&page).unwrap();
    let events = engine.subscribe();
    fs::write(&page, AFTER).unwrap();
    engine.reload(id).unwrap();
    assert_eq!(changed_edits(&events, id), Some(the_change()));
    assert_eq!(engine.document(id).unwrap().text.to_string(), AFTER);
    assert!(!engine.document(id).unwrap().dirty);
}

#[test]
fn settings_reload_on_external_change() {
    let directory = tempfile::tempdir().unwrap();
    let settings_file = directory.path().join(adoc_engine::WORKSPACE_SETTINGS_FILE);
    fs::write(&settings_file, "[editor]\ntab_width = 4\n").unwrap();
    fs::write(directory.path().join("page.adoc"), "= Page\n").unwrap();
    let mut engine = engine_without_user_settings();
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert_eq!(engine.settings().editor.tab_width, 4);
    let events = engine.subscribe();

    thread::sleep(Duration::from_millis(100));
    fs::write(&settings_file, "[editor]\ntab_width = 2\n").unwrap();
    assert!(pump_until(&mut engine, |engine| {
        engine.settings().editor.tab_width == 2
    }));
    // Pumped a while longer, the change is applied once.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        engine.pump();
        thread::sleep(Duration::from_millis(25));
    }
    let reloads = events
        .try_iter()
        .filter(|event| matches!(event, EngineEvent::SettingsChanged))
        .count();
    assert_eq!(reloads, 1);
}

#[test]
fn user_settings_reload_on_external_change() {
    let user = tempfile::tempdir().unwrap();
    // The folder as the watcher reports it: macOS's temporary folder is behind a link, and
    // Windows would give a resolved path its `\\?\` form.
    let folder = if cfg!(target_os = "macos") {
        fs::canonicalize(user.path()).unwrap()
    } else {
        user.path().to_path_buf()
    };
    let user_file = folder.join(adoc_engine::USER_SETTINGS_FILE);
    fs::write(&user_file, "[editor]\nfont_size = 14\n").unwrap();
    let workspace = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("page.adoc"), "= Page\n").unwrap();
    let mut engine = Engine::new(
        Settings::load(Some(&user_file), None)
            .unwrap()
            .with_git_opt_in(),
    );
    engine
        .open_workspace(vec![workspace.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert_eq!(engine.settings().editor.font_size, 14);

    thread::sleep(Duration::from_millis(100));
    fs::write(&user_file, "[editor]\nfont_size = 18\n").unwrap();
    assert!(pump_until(&mut engine, |engine| {
        engine.settings().editor.font_size == 18
    }));
}
