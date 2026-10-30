//! Crash recovery: unsaved edits written to the recovery store once typing pauses, and offered
//! back by the next session when the app ended without saving them.

use std::{fs, path::PathBuf, sync::Arc};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DiffLine, DocumentId, Engine, ManualClock,
    Recoverable, Settings, RECOVERY_DIR, RECOVERY_SETTLE,
};

/// A settings folder and a folder holding one page.
struct Setup {
    _config: tempfile::TempDir,
    pages: tempfile::TempDir,
    user: PathBuf,
}

impl Setup {
    fn new() -> Self {
        let config = tempfile::tempdir().unwrap();
        let pages = tempfile::tempdir().unwrap();
        let user = config.path().join("adocfoundry.toml");
        fs::write(&user, "").unwrap();
        fs::write(pages.path().join("page.adoc"), "= Page\n\nSaved text.\n").unwrap();
        Self {
            _config: config,
            pages,
            user,
        }
    }

    fn page(&self) -> PathBuf {
        adoc_engine::normalize_path(&self.pages.path().join("page.adoc"))
    }

    /// The app's state folder, apart from the settings file.
    fn state(&self) -> PathBuf {
        self.user.parent().unwrap().join("state")
    }

    fn store(&self) -> PathBuf {
        self.state().join(RECOVERY_DIR)
    }

    /// The settings as the app loads them, with its state folder.
    fn settings(&self) -> Settings {
        Settings::load(Some(&self.user), None)
            .unwrap()
            .with_git_opt_in()
            .with_state_dir(Some(self.state()))
    }

    /// The records in the store.
    fn records(&self) -> usize {
        fs::read_dir(self.store()).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
    }

    /// An app session: an engine with recovery on.
    fn session(&self) -> (Engine, ManualClock) {
        let clock = ManualClock::new();
        let mut engine = Engine::with_clock(self.settings(), Arc::new(clock.clone()));
        engine.enable_recovery();
        (engine, clock)
    }
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

fn settle(engine: &mut Engine, clock: &ManualClock) {
    clock.advance(RECOVERY_SETTLE);
    engine.pump();
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

fn offered(engine: &mut Engine) -> Vec<Recoverable> {
    match engine.invoke(ids::DOCUMENT_RECOVER_UNSAVED, CommandArgs::None) {
        Ok(CommandOutcome::Recoverable(items)) => items,
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unsaved_edit_is_offered_back_after_the_app_is_killed() {
    let setup = Setup::new();
    let (mut first, clock) = setup.session();
    let id = first.open(&setup.page()).unwrap();
    type_text(&mut first, id, "Unsaved paragraph.\n");
    assert!(first.has_pending(), "the record waits for typing to pause");
    settle(&mut first, &clock);
    assert_eq!(setup.records(), 1);
    // The app is killed: nothing is saved or closed.
    drop(first);

    let (mut second, _) = setup.session();
    let items = offered(&mut second);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, setup.page());
    assert!(!items[0].untitled);
    assert_eq!(items[0].text, "= Page\n\nSaved text.\nUnsaved paragraph.\n");
    assert_eq!(
        second.recovery_diff(&items[0]),
        vec![
            DiffLine::Same("= Page".into()),
            DiffLine::Same(String::new()),
            DiffLine::Same("Saved text.".into()),
            DiffLine::Added("Unsaved paragraph.".into()),
        ]
    );

    let outcome = second
        .invoke(
            ids::DOCUMENT_RECOVER,
            CommandArgs::Path(items[0].record.clone()),
        )
        .unwrap();
    let CommandOutcome::Opened(recovered) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(second.active(), Some(recovered));
    assert_eq!(
        text(&second, recovered),
        "= Page\n\nSaved text.\nUnsaved paragraph.\n"
    );
    assert!(second.document(recovered).unwrap().dirty, "not saved yet");
    assert!(offered(&mut second).is_empty());
    // One undo step takes the page back to the file.
    assert!(second.undo(recovered));
    assert_eq!(text(&second, recovered), "= Page\n\nSaved text.\n");
}

#[test]
fn nothing_is_written_before_typing_pauses_unless_flushed() {
    let setup = Setup::new();
    let (mut engine, _) = setup.session();
    let id = engine.open(&setup.page()).unwrap();
    type_text(&mut engine, id, "x");
    engine.pump();
    assert_eq!(setup.records(), 0);
    // The app writes what is waiting when it quits.
    engine.write_recovery_now();
    assert_eq!(setup.records(), 1);
}

#[test]
fn the_record_follows_typing_and_goes_once_the_page_is_saved_closed_or_clean() {
    let setup = Setup::new();
    let (mut engine, clock) = setup.session();
    let id = engine.open(&setup.page()).unwrap();

    type_text(&mut engine, id, "one");
    settle(&mut engine, &clock);
    type_text(&mut engine, id, " two");
    settle(&mut engine, &clock);
    assert_eq!(setup.records(), 1, "one record per page, rewritten");
    let (mut other, _) = setup.session();
    drop(engine);
    let items = offered(&mut other);
    assert!(items[0].text.ends_with("one two"), "{:?}", items[0].text);
    drop(other);

    let (mut engine, clock) = setup.session();
    let id = engine.open(&setup.page()).unwrap();
    type_text(&mut engine, id, "edit");
    settle(&mut engine, &clock);
    let before = setup.records();
    engine.save(id).unwrap();
    assert_eq!(setup.records(), before - 1, "saving removes it");

    type_text(&mut engine, id, "again");
    settle(&mut engine, &clock);
    assert!(engine.undo(id));
    assert!(!engine.document(id).unwrap().dirty);
    assert_eq!(
        setup.records(),
        before - 1,
        "undoing to the saved text removes it"
    );

    type_text(&mut engine, id, "more");
    settle(&mut engine, &clock);
    engine.close(id).unwrap();
    assert_eq!(
        setup.records(),
        before - 1,
        "closing without saving discards it"
    );
}

#[test]
fn a_running_session_s_edits_are_not_offered() {
    let setup = Setup::new();
    let (mut first, clock) = setup.session();
    let id = first.open(&setup.page()).unwrap();
    type_text(&mut first, id, "live");
    settle(&mut first, &clock);

    let (mut second, _) = setup.session();
    assert!(
        offered(&mut second).is_empty(),
        "the first app is still running"
    );
    assert!(
        offered(&mut first).is_empty(),
        "a session never offers its own"
    );
    drop(first);
    assert_eq!(offered(&mut second).len(), 1);
}

#[test]
fn an_untitled_page_is_recovered_as_a_new_untitled_page() {
    let setup = Setup::new();
    let (mut first, clock) = setup.session();
    let id = first.open_untitled();
    type_text(&mut first, id, "= Draft\n");
    settle(&mut first, &clock);
    drop(first);

    let (mut second, _) = setup.session();
    let items = offered(&mut second);
    assert!(items[0].untitled);
    assert_eq!(
        second.recovery_diff(&items[0]),
        vec![DiffLine::Added("= Draft".into())]
    );
    let Ok(CommandOutcome::Opened(id)) = second.invoke(
        ids::DOCUMENT_RECOVER,
        CommandArgs::Path(items[0].record.clone()),
    ) else {
        panic!("not recovered");
    };
    let document = second.document(id).unwrap();
    assert!(document.untitled && document.dirty);
    assert_eq!(text(&second, id), "= Draft\n");
}

#[test]
fn a_page_whose_file_was_deleted_is_recovered_at_its_path() {
    let setup = Setup::new();
    let (mut first, clock) = setup.session();
    let id = first.open(&setup.page()).unwrap();
    type_text(&mut first, id, "kept");
    settle(&mut first, &clock);
    drop(first);
    fs::remove_file(setup.page()).unwrap();

    let (mut second, _) = setup.session();
    let items = offered(&mut second);
    let Ok(CommandOutcome::Opened(id)) = second.invoke(
        ids::DOCUMENT_RECOVER,
        CommandArgs::Path(items[0].record.clone()),
    ) else {
        panic!("not recovered");
    };
    assert_eq!(second.document(id).unwrap().path, setup.page());
    assert!(text(&second, id).ends_with("kept"));
    second.save(id).unwrap();
    assert!(fs::read_to_string(setup.page()).unwrap().ends_with("kept"));
}

#[test]
fn discarding_forgets_the_record() {
    let setup = Setup::new();
    let (mut first, clock) = setup.session();
    let id = first.open(&setup.page()).unwrap();
    type_text(&mut first, id, "unwanted");
    settle(&mut first, &clock);
    drop(first);

    let (mut second, _) = setup.session();
    let items = offered(&mut second);
    second
        .invoke(
            ids::DOCUMENT_DISCARD_RECOVERY,
            CommandArgs::Path(items[0].record.clone()),
        )
        .unwrap();
    assert!(offered(&mut second).is_empty());
    assert_eq!(setup.records(), 0);
    // Nothing of the ended session is left behind.
    assert_eq!(fs::read_dir(setup.store()).unwrap().count(), 0);
    let page = second.open(&setup.page()).unwrap();
    assert_eq!(text(&second, page), "= Page\n\nSaved text.\n");
}

#[test]
fn only_records_offered_can_be_recovered() {
    let setup = Setup::new();
    let (mut engine, _) = setup.session();
    for record in [setup.page(), setup.store().join("../../adocfoundry.toml")] {
        assert!(engine
            .invoke(ids::DOCUMENT_RECOVER, CommandArgs::Path(record.clone()))
            .is_err());
        assert!(engine
            .invoke(ids::DOCUMENT_DISCARD_RECOVERY, CommandArgs::Path(record))
            .is_err());
    }
    assert!(setup.page().exists() && setup.user.exists());
}

#[test]
fn front_ends_without_recovery_write_nothing() {
    let setup = Setup::new();
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(setup.settings(), Arc::new(clock.clone()));
    let id = engine.open(&setup.page()).unwrap();
    type_text(&mut engine, id, "language server buffer");
    settle(&mut engine, &clock);
    engine.write_recovery_now();
    assert!(!setup.store().exists());
}

#[test]
fn without_a_state_folder_nothing_is_written() {
    let setup = Setup::new();
    let clock = ManualClock::new();
    let settings = Settings::load(Some(&setup.user), None)
        .unwrap()
        .with_git_opt_in();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine.enable_recovery();
    let id = engine.open(&setup.page()).unwrap();
    type_text(&mut engine, id, "x");
    settle(&mut engine, &clock);
    engine.write_recovery_now();
    assert!(offered(&mut engine).is_empty());
    // Nothing beside the settings file, nor beside the page.
    let config = setup.user.parent().unwrap();
    assert_eq!(fs::read_dir(config).unwrap().count(), 1);
    assert_eq!(fs::read_dir(setup.pages.path()).unwrap().count(), 1);
}

#[test]
fn a_page_renamed_with_unsaved_edits_is_recovered_at_its_new_path() {
    let setup = Setup::new();
    let (mut first, clock) = setup.session();
    first
        .open_workspace(vec![setup.pages.path().to_path_buf()])
        .unwrap();
    assert!(first.wait_for_workers(std::time::Duration::from_secs(10)));
    let id = first.open(&setup.page()).unwrap();
    type_text(&mut first, id, "kept");
    settle(&mut first, &clock);
    let renamed = adoc_engine::normalize_path(&setup.pages.path().join("renamed.adoc"));
    let Ok(CommandOutcome::Preview(preview)) = first.invoke(
        ids::PAGE_RENAME,
        CommandArgs::Rename {
            id,
            new_path: renamed.clone(),
        },
    ) else {
        panic!("no rename preview");
    };
    first
        .invoke(
            ids::WORKSPACE_APPLY_PREVIEW,
            CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
        )
        .unwrap();
    first.write_recovery_now();
    drop(first);

    let (mut second, _) = setup.session();
    let items = offered(&mut second);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, renamed);
}
