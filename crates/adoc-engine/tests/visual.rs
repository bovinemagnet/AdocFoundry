//! Headless tests of the visual model service: editor modes, visual commands, and keeping the
//! model in step with the source.

use std::{
    fs,
    path::Path,
    sync::{mpsc::Receiver, Arc},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DocumentId, EditorMode, Engine, EngineError,
    EngineEvent, ManualClock, MarkSet, Settings, VisualArgs, VisualValue, COALESCE_WINDOW,
    MODEL_SETTLE,
};

fn engine_with_clock() -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let engine = Engine::with_clock(Settings::load(None, None).unwrap(), Arc::new(clock.clone()));
    (engine, clock)
}

fn page(directory: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = directory.join(name);
    fs::write(&path, text).unwrap();
    path
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text_string()
}

/// The text every text block of the visual model shows.
fn shown(engine: &Engine, id: DocumentId) -> Vec<String> {
    engine
        .visual(id)
        .expect("a visual model")
        .model
        .text_blocks()
        .map(|(_, _, text)| text.display.clone())
        .collect()
}

fn version(engine: &Engine, id: DocumentId) -> u64 {
    engine.document(id).unwrap().version
}

fn show_visual(engine: &mut Engine, id: DocumentId) {
    engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
        .unwrap();
}

fn visual(
    engine: &mut Engine,
    command: adoc_engine::CommandId,
    id: DocumentId,
    selection: SourceRange,
    value: VisualValue,
) -> Result<SourceRange, EngineError> {
    let args = VisualArgs {
        id,
        version: version(engine, id),
        selection,
        marks: MarkSet::EMPTY,
        value,
    };
    match engine.invoke(command, CommandArgs::Visual(args))? {
        CommandOutcome::Selection(selection) => Ok(selection),
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn type_at(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) -> SourceRange {
    visual(
        engine,
        ids::VISUAL_INSERT_TEXT,
        id,
        SourceRange::new(at, at),
        VisualValue::Text(typed.to_owned()),
    )
    .unwrap()
}

fn events(receiver: &Receiver<EngineEvent>) -> Vec<EngineEvent> {
    receiver.try_iter().collect()
}

#[test]
fn editor_mode_requests_a_model() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "= Page\n\nA *bold* word.\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    let receiver = engine.subscribe();
    assert_eq!(engine.editor_mode(id), EditorMode::Source);
    assert!(engine.visual(id).is_none());

    show_visual(&mut engine, id);
    assert_eq!(engine.editor_mode(id), EditorMode::Visual);
    assert_eq!(shown(&engine, id), ["Page", "A bold word."]);
    let seen = events(&receiver);
    assert!(seen.contains(&EngineEvent::EditorModeChanged(id)));
    assert!(seen.contains(&EngineEvent::ModelChanged { id, version: 0 }));

    engine
        .invoke(ids::VIEW_SHOW_SPLIT, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.editor_mode(id), EditorMode::Split);
    assert!(engine.visual(id).is_some());

    engine
        .invoke(ids::VIEW_SHOW_SOURCE, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(engine.editor_mode(id), EditorMode::Source);
    assert!(engine.visual(id).is_none());
}

#[test]
fn visual_typing_edits_the_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "A *bold* word.\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    let receiver = engine.subscribe();

    let at = "A *bold* ".len();
    let selection = type_at(&mut engine, id, at, "new ");
    assert_eq!(text(&engine, id), "A *bold* new word.\n");
    assert_eq!(selection, SourceRange::new(at + 4, at + 4));
    assert_eq!(shown(&engine, id), ["A bold new word."]);
    assert_eq!(engine.visual(id).unwrap().version, version(&engine, id));
    let seen = events(&receiver);
    assert!(seen.iter().any(
        |event| matches!(event, EngineEvent::DocumentChanged { id: changed, .. } if *changed == id)
    ));
    assert!(seen.iter().any(
        |event| matches!(event, EngineEvent::ModelChanged { id: changed, .. } if *changed == id)
    ));

    // Typing over a selection replaces it; deleting removes the character before the caret.
    let word = text(&engine, id).find("word").unwrap();
    visual(
        &mut engine,
        ids::VISUAL_INSERT_TEXT,
        id,
        SourceRange::new(word, word + 4),
        VisualValue::Text("text".to_owned()),
    )
    .unwrap();
    assert_eq!(text(&engine, id), "A *bold* new text.\n");
    let end = text(&engine, id).find('.').unwrap();
    visual(
        &mut engine,
        ids::VISUAL_DELETE_BACKWARD,
        id,
        SourceRange::new(end, end),
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "A *bold* new tex.\n");
    let inside = text(&engine, id).find("old").unwrap();
    visual(
        &mut engine,
        ids::VISUAL_DELETE_FORWARD,
        id,
        SourceRange::new(inside, inside),
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "A *bld* new tex.\n");
}

#[test]
fn visual_typing_is_one_undo_step() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "Some text.\n");
    let (mut engine, clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);

    let mut at = "Some".len();
    for character in [" ", "n", "e", "w"] {
        at = type_at(&mut engine, id, at, character).start;
        clock.advance(COALESCE_WINDOW / 4);
    }
    assert_eq!(text(&engine, id), "Some new text.\n");
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "Some text.\n");
    assert_eq!(shown(&engine, id), ["Some text."]);
}

#[test]
fn undo_restores_source_and_model() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "Plain *bold*.\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    let start = text(&engine, id).find("bold").unwrap();
    visual(
        &mut engine,
        ids::VISUAL_DELETE_BACKWARD,
        id,
        SourceRange::new(start, start + 4),
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "Plain .\n");
    assert_eq!(shown(&engine, id), ["Plain ."]);

    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "Plain *bold*.\n");
    let visual_document = engine.visual(id).unwrap();
    assert_eq!(visual_document.version, version(&engine, id));
    assert_eq!(shown(&engine, id), ["Plain bold."]);

    engine
        .invoke(ids::EDIT_REDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(shown(&engine, id), ["Plain ."]);
}

#[test]
fn source_edits_rebuild_the_model_after_settle() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "First.\n");
    let (mut engine, clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);

    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(5, 5),
                text: " and second".to_owned(),
            },
        )
        .unwrap();
    engine.pump();
    assert_eq!(shown(&engine, id), ["First."], "nothing before the pause");
    assert!(engine.has_pending());

    clock.advance(MODEL_SETTLE);
    engine.pump();
    assert_eq!(shown(&engine, id), ["First and second."]);
    assert_eq!(engine.visual(id).unwrap().version, version(&engine, id));
}

#[test]
fn stale_model_refuses_visual_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "Text.\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    let old_version = version(&engine, id);
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "More ".to_owned(),
            },
        )
        .unwrap();
    let args = VisualArgs {
        id,
        version: old_version,
        selection: SourceRange::new(1, 1),
        marks: MarkSet::EMPTY,
        value: VisualValue::Text("x".to_owned()),
    };
    let result = engine.invoke(ids::VISUAL_INSERT_TEXT, CommandArgs::Visual(args));
    assert!(matches!(result, Err(EngineError::Refused(_))));
    assert_eq!(text(&engine, id), "More Text.\n");

    // A selection in the current version is applied, the stale model rebuilt first.
    type_at(&mut engine, id, "More Text".len(), "s");
    assert_eq!(text(&engine, id), "More Texts.\n");
    assert_eq!(shown(&engine, id), ["More Texts."]);
}

#[test]
fn read_only_text_refuses_visual_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "[stem]\n++++\nx^2\n++++\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    let result = visual(
        &mut engine,
        ids::VISUAL_INSERT_TEXT,
        id,
        SourceRange::new(13, 13),
        VisualValue::Text("y".to_owned()),
    );
    assert!(matches!(result, Err(EngineError::Refused(_))));
}

#[test]
fn included_files_rebuild_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let partial = page(directory.path(), "partial.adoc", "From the partial.\n");
    let path = page(
        directory.path(),
        "page.adoc",
        "Before.\n\ninclude::partial.adoc[]\n\nAfter.\n",
    );
    let (mut engine, clock) = engine_with_clock();
    let page_id = engine.open(&path).unwrap();
    let partial_id = engine.open(&partial).unwrap();
    engine.set_active(page_id);
    show_visual(&mut engine, page_id);
    let included = |engine: &Engine| {
        let visual = engine.visual(page_id).unwrap();
        visual
            .view
            .blocks
            .iter()
            .map(|block| match &block.kind {
                adoc_render::ViewKind::Paragraph(content) => {
                    adoc_render::ViewInline::plain_text(content)
                }
                _ => String::new(),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        included(&engine),
        ["Before.", "From the partial.", "After."]
    );

    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: partial_id,
                range: SourceRange::new(0, 0),
                text: "Now ".to_owned(),
            },
        )
        .unwrap();
    clock.advance(MODEL_SETTLE + Duration::from_millis(1));
    engine.pump();
    assert_eq!(
        included(&engine),
        ["Before.", "Now From the partial.", "After."]
    );
}
