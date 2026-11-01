//! Headless engine tests for documents, editing and undo, driven through commands.

use std::{fs, sync::Arc, time::Duration};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DocumentId, Engine, EngineEvent, ManualClock,
    Settings,
};

fn insert(engine: &mut Engine, id: DocumentId, offset: usize, text: &str) {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(offset, offset),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

#[test]
fn open_edit_save_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("guide.adoc");
    fs::write(&path, "= Guide\n\nHello.\n").unwrap();

    let mut engine = Engine::new(Settings::default());
    let events = engine.subscribe();
    let CommandOutcome::Opened(id) = engine
        .invoke(ids::DOCUMENT_OPEN, CommandArgs::Path(path.clone()))
        .unwrap()
    else {
        panic!("document.open must return Opened");
    };
    assert_eq!(engine.active(), Some(id));
    assert_eq!(
        engine
            .document(id)
            .unwrap()
            .parsed()
            .title
            .as_ref()
            .unwrap()
            .text,
        "Guide"
    );

    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(9, 15),
                text: "Goodbye.".to_owned(),
            },
        )
        .unwrap();
    let document = engine.document(id).unwrap();
    assert!(document.dirty);
    assert_eq!(document.version, 1);

    engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        .unwrap();
    assert!(!engine.document(id).unwrap().dirty);
    assert_eq!(fs::read_to_string(&path).unwrap(), "= Guide\n\nGoodbye.\n");

    let received: Vec<_> = events.try_iter().collect();
    assert!(received.contains(&EngineEvent::DocumentOpened(id)));
    assert!(received.contains(&EngineEvent::DocumentSaved(id)));
    assert!(received
        .iter()
        .any(|event| matches!(event, EngineEvent::DocumentChanged { id: changed, version: 1, .. } if *changed == id)));

    // Re-opening the same path returns the same document.
    assert_eq!(engine.open(&path).unwrap(), id);
    engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(id))
        .unwrap();
    assert!(engine.document(id).is_none());
}

/// Opens `count` untitled documents, in tab order.
fn open_tabs(engine: &mut Engine, count: usize) -> Vec<DocumentId> {
    (0..count).map(|_| engine.open_untitled()).collect()
}

fn open_ids(engine: &Engine) -> Vec<DocumentId> {
    engine.documents().map(|document| document.id).collect()
}

#[test]
fn closing_the_active_tab_activates_its_neighbour() {
    let mut engine = Engine::new(Settings::default());
    let tabs = open_tabs(&mut engine, 6);
    let close = |engine: &mut Engine, id| {
        engine
            .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(id))
            .unwrap();
    };
    // The tab after the closed one takes its place.
    engine.set_active(tabs[1]);
    close(&mut engine, tabs[1]);
    assert_eq!(engine.active(), Some(tabs[2]));
    // The last tab closed gives way to the one before it.
    engine.set_active(tabs[5]);
    close(&mut engine, tabs[5]);
    assert_eq!(engine.active(), Some(tabs[4]));
    // A tab closed in the background leaves the active one alone.
    close(&mut engine, tabs[0]);
    assert_eq!(engine.active(), Some(tabs[4]));
}

#[test]
fn close_others_left_right_clean_and_all() {
    let mut engine = Engine::new(Settings::default());
    let tabs = open_tabs(&mut engine, 5);
    let [a, b, c, d, e] = tabs[..] else {
        unreachable!()
    };
    insert(&mut engine, b, 0, "unsaved");

    // What each command would close, in tab order, so a front end can ask about unsaved ones.
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE, c), vec![c]);
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_OTHERS, c),
        vec![a, b, d, e]
    );
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE_LEFT, c), vec![a, b]);
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE_RIGHT, c), vec![d, e]);
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_CLEAN, c),
        vec![a, c, d, e]
    );
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE_ALL, c), tabs);

    engine
        .invoke(ids::DOCUMENT_CLOSE_RIGHT, CommandArgs::Document(c))
        .unwrap();
    assert_eq!(open_ids(&engine), vec![a, b, c]);
    engine
        .invoke(ids::DOCUMENT_CLOSE_LEFT, CommandArgs::Document(b))
        .unwrap();
    assert_eq!(open_ids(&engine), vec![b, c]);
    engine
        .invoke(ids::DOCUMENT_CLOSE_CLEAN, CommandArgs::None)
        .unwrap();
    assert_eq!(
        open_ids(&engine),
        vec![b],
        "the unsaved document stays open"
    );

    let [f, g] = open_tabs(&mut engine, 2)[..] else {
        unreachable!()
    };
    engine
        .invoke(ids::DOCUMENT_CLOSE_OTHERS, CommandArgs::Document(f))
        .unwrap();
    assert_eq!(open_ids(&engine), vec![f]);
    assert_eq!(engine.active(), Some(f));
    let _ = g;
    engine
        .invoke(ids::DOCUMENT_CLOSE_ALL, CommandArgs::None)
        .unwrap();
    assert!(open_ids(&engine).is_empty());
    assert_eq!(engine.active(), None);
}

#[test]
fn closing_the_tabs_of_an_unknown_document_closes_nothing() {
    let mut engine = Engine::new(Settings::default());
    let [a, b] = open_tabs(&mut engine, 2)[..] else {
        unreachable!()
    };
    engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(b))
        .unwrap();
    assert!(engine.closed_by(ids::DOCUMENT_CLOSE_OTHERS, b).is_empty());
    assert!(engine
        .invoke(ids::DOCUMENT_CLOSE_OTHERS, CommandArgs::Document(b))
        .is_err());
    assert_eq!(open_ids(&engine), vec![a]);
}

#[test]
fn undo_redo_single_group() {
    let mut engine = Engine::new(Settings::default());
    let id = engine.open_untitled();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "== Title\n".to_owned(),
            },
        )
        .unwrap();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(3, 8),
                text: "Heading".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(text(&engine, id), "== Heading\n");

    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "== Title\n");
    assert_eq!(
        engine.document(id).unwrap().parsed().sections[0].title,
        "Title"
    );

    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "");
    assert!(!engine.document(id).unwrap().dirty);
    assert!(!engine.undo(id), "nothing left to undo");

    engine
        .invoke(ids::EDIT_REDO, CommandArgs::Document(id))
        .unwrap();
    engine
        .invoke(ids::EDIT_REDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "== Heading\n");
    assert!(!engine.redo(id), "nothing left to redo");

    // A new edit after an undo discards the redo history.
    engine.undo(id);
    insert(&mut engine, id, 0, "X");
    assert!(!engine.undo_stack(id).unwrap().can_redo());
}

#[test]
fn insert_text_coalesces_within_window() {
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(Settings::default(), Arc::new(clock.clone()));
    let id = engine.open_untitled();

    for (offset, character) in "abc".chars().enumerate() {
        insert(&mut engine, id, offset, &character.to_string());
        clock.advance(Duration::from_millis(100));
    }
    assert_eq!(
        engine.undo_stack(id).unwrap().len(),
        1,
        "typing within 500 ms coalesces"
    );

    clock.advance(Duration::from_millis(600));
    insert(&mut engine, id, 3, "d");
    assert_eq!(
        engine.undo_stack(id).unwrap().len(),
        2,
        "a pause starts a new group"
    );

    // A non-contiguous keystroke also starts a new group.
    insert(&mut engine, id, 0, "z");
    assert_eq!(engine.undo_stack(id).unwrap().len(), 3);
    assert_eq!(text(&engine, id), "zabcd");

    engine.undo(id);
    engine.undo(id);
    assert_eq!(text(&engine, id), "abc");
    engine.undo(id);
    assert_eq!(text(&engine, id), "");
}

#[test]
fn rejects_invalid_arguments_and_edits() {
    let mut engine = Engine::new(Settings::default());
    let events = engine.subscribe();
    let id = engine.open_untitled();
    assert!(engine.invoke(ids::EDIT_UNDO, CommandArgs::None).is_err());
    assert!(engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(5, 9),
                text: String::new(),
            },
        )
        .is_err());
    assert!(events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::CommandFailed { .. })));
    assert!(engine.save(id).is_err(), "untitled documents have no file");
}

/// A page on disk, opened and edited without saving.
fn edited_page(directory: &std::path::Path) -> (Engine, DocumentId, std::path::PathBuf) {
    let path = adoc_engine::normalize_path(&directory.join("page.adoc"));
    fs::write(&path, "= Page\n\nOriginal.\n").unwrap();
    let mut engine = Engine::new(Settings::default());
    let id = engine.open(&path).unwrap();
    insert(&mut engine, id, 0, "// edited\n");
    (engine, id, path)
}

#[test]
fn saving_over_a_file_changed_outside_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id, path) = edited_page(directory.path());
    fs::write(&path, "= Page\n\nChanged outside.\n").unwrap();

    let refused = engine.invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id));
    assert!(
        matches!(&refused, Err(adoc_engine::EngineError::SaveConflict { path: conflict }) if *conflict == path),
        "{refused:?}"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "= Page\n\nChanged outside.\n"
    );
    assert!(engine.document(id).unwrap().dirty);

    // Overwriting is a decision the caller makes explicitly.
    engine
        .invoke(ids::DOCUMENT_OVERWRITE, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "// edited\n= Page\n\nOriginal.\n"
    );
    assert!(!engine.document(id).unwrap().dirty);
    // Saved, the file is ours again: the next save needs no overwrite.
    insert(&mut engine, id, 0, "x");
    engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        .unwrap();
}

#[test]
fn reloading_takes_the_file_on_disk_as_one_undo_step() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id, path) = edited_page(directory.path());
    fs::write(&path, "= Page\n\nChanged outside.\n").unwrap();

    engine
        .invoke(ids::DOCUMENT_RELOAD, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "= Page\n\nChanged outside.\n");
    assert!(!engine.document(id).unwrap().dirty);
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "// edited\n= Page\n\nOriginal.\n");
}

#[test]
fn save_all_saves_the_rest_and_reports_conflicts() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, conflicted, path) = edited_page(directory.path());
    let other = adoc_engine::normalize_path(&directory.path().join("other.adoc"));
    fs::write(&other, "= Other\n").unwrap();
    let other_id = engine.open(&other).unwrap();
    insert(&mut engine, other_id, 0, "// edited\n");
    fs::write(&path, "changed outside\n").unwrap();

    let refused = engine.invoke(ids::DOCUMENT_SAVE_ALL, CommandArgs::None);
    assert!(
        matches!(&refused, Err(adoc_engine::EngineError::SaveConflict { path: conflict }) if *conflict == path),
        "{refused:?}"
    );
    assert_eq!(fs::read_to_string(&other).unwrap(), "// edited\n= Other\n");
    assert_eq!(fs::read_to_string(&path).unwrap(), "changed outside\n");
    assert!(engine.document(conflicted).unwrap().dirty);
}

#[test]
fn saving_a_file_deleted_outside_writes_it_again() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id, path) = edited_page(directory.path());
    fs::remove_file(&path).unwrap();
    engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "// edited\n= Page\n\nOriginal.\n"
    );
}

#[cfg(unix)]
#[test]
fn saving_keeps_permissions_and_links_and_leaves_no_temporary_file() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real.adoc");
    fs::write(&real, "= Real\n").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    let link = adoc_engine::normalize_path(&directory.path().join("link.adoc"));
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let mut engine = Engine::new(Settings::default());
    let id = engine.open(&link).unwrap();
    insert(&mut engine, id, 0, "// edited\n");
    engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        .unwrap();

    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_to_string(&real).unwrap(), "// edited\n= Real\n");
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let names: Vec<_> = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn open_documents_share_one_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = adoc_engine::normalize_path(&directory.path().join("page.adoc"));
    fs::write(&path, "= Page\n\nText.\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&path).unwrap();
    let shared = |engine: &Engine| {
        let parsed = &engine.document(id).unwrap().parsed().text;
        let indexed = &engine.workspace().index.file(&path).unwrap().document.text;
        Arc::ptr_eq(parsed, indexed)
    };
    assert!(shared(&engine), "as opened");

    // After an edit is indexed, the index holds the edited parse's text, not a copy of it.
    insert(&mut engine, id, 7, "\nMore.\n");
    engine
        .invoke(
            ids::NAVIGATE_GO_TO_SYMBOL,
            CommandArgs::Query {
                query: "page".to_owned(),
                document: None,
            },
        )
        .ok();
    assert!(engine
        .workspace()
        .index
        .file(&path)
        .unwrap()
        .document
        .text
        .contains("More."));
    assert!(shared(&engine), "after an edit");
}
