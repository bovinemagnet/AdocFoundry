//! `[editor] auto_save`: saving after typing stops, or when a page loses focus.

use std::{
    fs,
    path::Path,
    sync::{mpsc::Receiver, Arc},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, AutoSave, CommandArgs, DocumentId, Engine, EngineEvent, ManualClock, Settings,
    AUTO_SAVE_DELAY,
};

fn engine(auto_save: AutoSave) -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let mut settings = Settings::load(None, None).unwrap().with_git_opt_in();
    settings.editor.auto_save = auto_save;
    (Engine::with_clock(settings, Arc::new(clock.clone())), clock)
}

fn page(dir: &Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, "= Page\n").unwrap();
    path
}

fn type_text(engine: &mut Engine, id: DocumentId, text: &str) {
    let end = engine.document(id).unwrap().text.len();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

/// Waits for the saves started so far, which are written off the engine thread.
fn written(engine: &mut Engine) {
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
}

fn dirty(engine: &Engine, id: DocumentId) -> bool {
    engine.document(id).unwrap().dirty
}

fn failures(events: &Receiver<EngineEvent>) -> usize {
    events
        .try_iter()
        .filter(|event| matches!(event, EngineEvent::CommandFailed { command, .. } if *command == ids::DOCUMENT_SAVE))
        .count()
}

#[test]
fn the_setting_is_read_and_off_by_default() {
    assert_eq!(Settings::default().editor.auto_save, AutoSave::Off);
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("adocfoundry.toml");
    for (value, expected) in [
        ("off", AutoSave::Off),
        ("on_focus_change", AutoSave::OnFocusChange),
        ("after_delay", AutoSave::AfterDelay),
    ] {
        fs::write(&user, format!("[editor]\nauto_save = \"{value}\"\n")).unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.editor.auto_save, expected);
        assert!(settings.warnings.is_empty());
    }
    fs::write(&user, "[editor]\nauto_save = \"sometimes\"\n").unwrap();
    let settings = Settings::load(Some(&user), None).unwrap();
    assert_eq!(settings.editor.auto_save, AutoSave::Off);
    assert_eq!(settings.warnings.len(), 1);
}

#[test]
fn after_delay_saves_once_typing_stops() {
    let dir = tempfile::tempdir().unwrap();
    let path = page(dir.path(), "page.adoc");
    let (mut engine, clock) = engine(AutoSave::AfterDelay);
    let events = engine.subscribe();
    let id = engine.open(&path).unwrap();

    type_text(&mut engine, id, "\nOne");
    clock.advance(AUTO_SAVE_DELAY - Duration::from_millis(1));
    type_text(&mut engine, id, " two.\n");
    clock.advance(AUTO_SAVE_DELAY - Duration::from_millis(1));
    engine.pump();
    assert!(dirty(&engine, id), "not while typing goes on");
    assert!(engine.has_pending(), "a save is due");

    clock.advance(Duration::from_millis(1));
    engine.pump();
    written(&mut engine);
    assert!(!dirty(&engine, id), "the unsaved marker clears");
    assert_eq!(fs::read_to_string(&path).unwrap(), "= Page\n\nOne two.\n");
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::DocumentSaved(id)));
}

#[test]
fn off_never_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = page(dir.path(), "page.adoc");
    let (mut engine, clock) = engine(AutoSave::Off);
    let id = engine.open(&path).unwrap();
    type_text(&mut engine, id, "\nText.\n");
    clock.advance(AUTO_SAVE_DELAY * 10);
    engine.pump();
    engine.focus_lost();
    assert!(dirty(&engine, id));
    assert!(!engine.has_pending());
    assert_eq!(fs::read_to_string(&path).unwrap(), "= Page\n");
}

#[test]
fn on_focus_change_saves_the_page_left_and_every_page_when_the_window_is() {
    let dir = tempfile::tempdir().unwrap();
    let first = page(dir.path(), "first.adoc");
    let second = page(dir.path(), "second.adoc");
    let (mut engine, clock) = engine(AutoSave::OnFocusChange);
    let first = engine.open(&first).unwrap();
    let second = engine.open(&second).unwrap();

    type_text(&mut engine, second, "\nEdited.\n");
    clock.advance(AUTO_SAVE_DELAY * 10);
    engine.pump();
    assert!(dirty(&engine, second), "not after a delay");

    engine.set_active(first);
    written(&mut engine);
    assert!(!dirty(&engine, second), "the tab left is saved");

    type_text(&mut engine, first, "\nAlso.\n");
    engine.focus_lost();
    written(&mut engine);
    assert!(!dirty(&engine, first), "the window losing focus saves it");
}

#[test]
fn a_refused_save_is_reported_once_until_saved_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let path = page(dir.path(), "page.adoc");
    let (mut engine, clock) = engine(AutoSave::AfterDelay);
    let events = engine.subscribe();
    let id = engine.open(&path).unwrap();

    type_text(&mut engine, id, "\nMine.\n");
    fs::write(&path, "= Page\n\nTheirs.\n").unwrap();
    clock.advance(AUTO_SAVE_DELAY);
    engine.pump();
    written(&mut engine);
    assert_eq!(failures(&events), 1);
    assert!(dirty(&engine, id));
    assert_eq!(fs::read_to_string(&path).unwrap(), "= Page\n\nTheirs.\n");

    type_text(&mut engine, id, "More.\n");
    clock.advance(AUTO_SAVE_DELAY);
    engine.pump();
    assert_eq!(failures(&events), 0, "not reported again");
    assert!(!engine.has_pending());

    // Overwriting by hand settles the conflict, and auto-save resumes.
    engine
        .invoke(ids::DOCUMENT_OVERWRITE, CommandArgs::Document(id))
        .unwrap();
    type_text(&mut engine, id, "Last.\n");
    clock.advance(AUTO_SAVE_DELAY);
    engine.pump();
    written(&mut engine);
    assert!(!dirty(&engine, id));
    assert!(fs::read_to_string(&path).unwrap().ends_with("Last.\n"));
}

#[test]
fn untitled_pages_are_left_alone() {
    let (mut engine, clock) = engine(AutoSave::AfterDelay);
    let events = engine.subscribe();
    let id = engine.open_untitled();
    type_text(&mut engine, id, "= Draft\n");
    clock.advance(AUTO_SAVE_DELAY);
    engine.pump();
    engine.focus_lost();
    assert!(dirty(&engine, id));
    assert_eq!(failures(&events), 0);
    assert!(!engine.has_pending());
}

/// An engine with a workspace open on `dir`, so the watcher sees the engine's own writes. The
/// workspace's settings turn `after_delay` on, since opening it reloads them.
fn workspace_engine(dir: &Path) -> (Engine, ManualClock) {
    fs::write(
        dir.join(".adocfoundry.toml"),
        "[editor]\nauto_save = \"after_delay\"\n",
    )
    .unwrap();
    let (mut engine, clock) = engine(AutoSave::AfterDelay);
    engine.open_workspace(vec![dir.to_path_buf()]).unwrap();
    engine.wait_for_workers(Duration::from_secs(10));
    (engine, clock)
}

fn saved(events: &Receiver<EngineEvent>) -> usize {
    events
        .try_iter()
        .filter(|event| matches!(event, EngineEvent::DocumentSaved(_)))
        .count()
}

/// Pumps until the watcher has had time to report the engine's own write.
fn settle_watcher(engine: &mut Engine) {
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(50));
        engine.pump();
    }
}

#[test]
fn an_auto_save_is_written_off_the_engine_thread() {
    let dir = tempfile::tempdir().unwrap();
    let path = page(dir.path(), "page.adoc");
    let (mut engine, clock) = workspace_engine(dir.path());
    let id = engine.open(&path).unwrap();
    let events = engine.subscribe();
    let long = "A line of a long page.\n".repeat(45_000);
    type_text(&mut engine, id, &long);

    clock.advance(AUTO_SAVE_DELAY);
    engine.pump();
    assert!(engine.has_pending(), "the save is in flight");
    assert!(dirty(&engine, id), "not saved until the write is done");
    assert_eq!(saved(&events), 0);

    written(&mut engine);
    assert!(!dirty(&engine, id));
    assert_eq!(saved(&events), 1);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("= Page\n{long}")
    );

    settle_watcher(&mut engine);
    assert!(!dirty(&engine, id));
    assert!(!events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::DocumentChangedOnDisk(_))));
}

#[test]
fn typing_during_an_auto_save_leaves_the_page_dirty_and_saveable() {
    let dir = tempfile::tempdir().unwrap();
    let path = page(dir.path(), "page.adoc");
    let (mut engine, clock) = workspace_engine(dir.path());
    let id = engine.open(&path).unwrap();
    let events = engine.subscribe();
    type_text(&mut engine, id, "First.\n");
    clock.advance(AUTO_SAVE_DELAY);
    engine.pump();
    type_text(&mut engine, id, "Second.\n");

    written(&mut engine);
    assert!(dirty(&engine, id), "the second edit is not on disk");
    assert_eq!(fs::read_to_string(&path).unwrap(), "= Page\nFirst.\n");
    settle_watcher(&mut engine);
    assert!(!events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::DocumentChangedOnDisk(_))));

    engine.save(id).unwrap();
    assert!(!dirty(&engine, id));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "= Page\nFirst.\nSecond.\n"
    );
}

#[test]
fn saving_by_hand_waits_for_an_auto_save_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let path = page(dir.path(), "page.adoc");
    let (mut engine, clock) = workspace_engine(dir.path());
    let id = engine.open(&path).unwrap();
    type_text(&mut engine, id, "First.\n");
    clock.advance(AUTO_SAVE_DELAY);
    engine.pump();
    type_text(&mut engine, id, "Second.\n");

    engine.save(id).unwrap();
    assert!(!dirty(&engine, id));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "= Page\nFirst.\nSecond.\n"
    );
}
