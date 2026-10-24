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

#[test]
fn marks_and_links_are_one_undo_step_each() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "Some plain words.\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    let plain = SourceRange::new(5, 10);
    let selection = visual(
        &mut engine,
        ids::EDIT_TOGGLE_STRONG,
        id,
        plain,
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "Some *plain* words.\n");
    assert_eq!(&text(&engine, id)[selection.start..selection.end], "plain");
    visual(
        &mut engine,
        ids::EDIT_TOGGLE_STRONG,
        id,
        selection,
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "Some plain words.\n");

    let words = text(&engine, id).find("words").unwrap();
    visual(
        &mut engine,
        ids::EDIT_INSERT_LINK,
        id,
        SourceRange::new(words, words + 5),
        VisualValue::Link {
            target: "https://example.com".to_owned(),
            text: None,
        },
    )
    .unwrap();
    assert_eq!(
        text(&engine, id),
        "Some plain https://example.com[words].\n"
    );
    let inside = text(&engine, id).find("[words").unwrap() + 2;
    visual(
        &mut engine,
        ids::EDIT_REMOVE_LINK,
        id,
        SourceRange::new(inside, inside),
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "Some plain words.\n");

    for expected in [
        "Some plain https://example.com[words].\n",
        "Some plain words.\n",
        "Some *plain* words.\n",
        "Some plain words.\n",
    ] {
        engine
            .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
            .unwrap();
        assert_eq!(text(&engine, id), expected);
    }
}

#[test]
fn structural_steps_rebuild_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "First part second part.\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    let at = text(&engine, id).find("second").unwrap();
    let caret = visual(
        &mut engine,
        ids::VISUAL_SPLIT_BLOCK,
        id,
        SourceRange::new(at, at),
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "First part\n\nsecond part.\n");
    assert_eq!(shown(&engine, id), ["First part", "second part."]);
    assert_eq!(caret.start, text(&engine, id).find("second").unwrap());

    // Backspace at the start of the second paragraph joins them again, leaving its line.
    visual(
        &mut engine,
        ids::VISUAL_DELETE_BACKWARD,
        id,
        caret,
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "First part\nsecond part.\n");
    assert_eq!(shown(&engine, id), ["First part\nsecond part."]);

    let start = SourceRange::new(0, 0);
    visual(
        &mut engine,
        ids::EDIT_TOGGLE_BULLET_LIST,
        id,
        start,
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(text(&engine, id), "* First part\nsecond part.\n");
    visual(
        &mut engine,
        ids::EDIT_SET_BLOCK_TYPE,
        id,
        SourceRange::new(2, 2),
        VisualValue::BlockType(adoc_engine::BlockType::Paragraph),
    )
    .unwrap();
    assert_eq!(text(&engine, id), "First part\nsecond part.\n");

    // A paragraph typed after the last block.
    let end = text(&engine, id).trim_end().len();
    visual(
        &mut engine,
        ids::VISUAL_INSERT_BLOCK,
        id,
        SourceRange::new(end, end),
        VisualValue::NewBlock(adoc_engine::NewBlock::Paragraph {
            text: "T".to_owned(),
        }),
    )
    .unwrap();
    assert_eq!(text(&engine, id), "First part\nsecond part.\n\nT\n");
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), "First part\nsecond part.\n");
    assert_eq!(engine.visual(id).unwrap().version, version(&engine, id));
}

#[test]
fn tables_and_images_are_edited_through_commands() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "Intro.\n");
    let (mut engine, _) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    let selection = visual(
        &mut engine,
        ids::EDIT_INSERT_TABLE,
        id,
        SourceRange::new(2, 2),
        VisualValue::Table {
            columns: 2,
            rows: 1,
        },
    )
    .unwrap();
    assert_eq!(
        text(&engine, id),
        "Intro.\n\n|===\n|Column 1 |Column 2\n\n| |\n|===\n"
    );
    // Typing replaces the selected heading.
    visual(
        &mut engine,
        ids::VISUAL_INSERT_TEXT,
        id,
        selection,
        VisualValue::Text("Name".to_owned()),
    )
    .unwrap();
    let cell = text(&engine, id).find("Name").unwrap();
    visual(
        &mut engine,
        ids::EDIT_INSERT_TABLE_COLUMN,
        id,
        SourceRange::new(cell, cell),
        VisualValue::None,
    )
    .unwrap();
    assert_eq!(
        text(&engine, id),
        "Intro.\n\n|===\n|Name | |Column 2\n\n| | |\n|===\n"
    );

    visual(
        &mut engine,
        ids::EDIT_INSERT_IMAGE,
        id,
        SourceRange::new(2, 2),
        VisualValue::Image {
            target: "a.png".to_owned(),
            alt: "A".to_owned(),
        },
    )
    .unwrap();
    let image = text(&engine, id).find("image::").unwrap();
    visual(
        &mut engine,
        ids::EDIT_EDIT_IMAGE,
        id,
        SourceRange::new(image, image),
        VisualValue::Image {
            target: "b.png".to_owned(),
            alt: "B".to_owned(),
        },
    )
    .unwrap();
    assert!(text(&engine, id).starts_with("Intro.\n\nimage::b.png[B]\n\n|===\n"));
}

#[test]
fn visual_commands_are_unavailable_in_source_mode() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let id = engine
        .open(&page(directory.path(), "page.adoc", "= Page\n\nWords.\n"))
        .unwrap();
    // Source mode: the visual editor's own commands are not offered; document commands are,
    // and so are the marks, which the source editor toggles too.
    assert!(!engine.command_available(ids::EDIT_TOGGLE_BULLET_LIST));
    assert!(!engine.command_available(ids::VISUAL_SPLIT_BLOCK));
    assert!(engine.command_available(ids::EDIT_TOGGLE_STRONG));
    assert!(engine.command_available(ids::VIEW_SHOW_VISUAL));
    assert!(engine.command_available(ids::DOCUMENT_SAVE));
    show_visual(&mut engine, id);
    assert!(engine.command_available(ids::EDIT_TOGGLE_BULLET_LIST));
    engine
        .invoke(ids::VIEW_SHOW_SPLIT, CommandArgs::Document(id))
        .unwrap();
    assert!(engine.command_available(ids::EDIT_TOGGLE_BULLET_LIST));
    engine
        .invoke(ids::VIEW_SHOW_SOURCE, CommandArgs::Document(id))
        .unwrap();
    assert!(!engine.command_available(ids::EDIT_TOGGLE_BULLET_LIST));
    // Unknown commands are not available.
    assert!(!engine.command_available(adoc_engine::CommandId::new("no.such_command")));
}

#[test]
fn marks_toggle_a_source_selection() {
    // Bold, italic, code and highlight work in the source editor too, on its selection.
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "= Page\n\nA bold word.\n");
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    assert_eq!(engine.editor_mode(id), EditorMode::Source);
    assert!(engine.command_available(ids::EDIT_TOGGLE_STRONG));

    let bold = SourceRange::new(10, 14);
    let outcome = engine
        .invoke(
            ids::EDIT_TOGGLE_STRONG,
            CommandArgs::Range { id, range: bold },
        )
        .unwrap();
    assert_eq!(text(&engine, id), "= Page\n\nA *bold* word.\n");
    assert_eq!(outcome, CommandOutcome::Selection(SourceRange::new(11, 15)));

    // Toggled again from that selection, the mark goes; one undo brings it back.
    engine
        .invoke(
            ids::EDIT_TOGGLE_STRONG,
            CommandArgs::Range {
                id,
                range: SourceRange::new(11, 15),
            },
        )
        .unwrap();
    assert_eq!(text(&engine, id), "= Page\n\nA bold word.\n");
    assert!(engine.undo(id));
    assert_eq!(text(&engine, id), "= Page\n\nA *bold* word.\n");

    engine
        .invoke(
            ids::EDIT_TOGGLE_CODE,
            CommandArgs::Range {
                id,
                range: SourceRange::new(17, 21),
            },
        )
        .unwrap();
    assert_eq!(text(&engine, id), "= Page\n\nA *bold* `word`.\n");
}
