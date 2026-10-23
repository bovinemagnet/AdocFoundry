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
            .parsed
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
        engine.document(id).unwrap().parsed.sections[0].title,
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
