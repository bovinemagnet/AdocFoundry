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
fn antora_example_includes_show_in_listings() {
    // A listing that includes an Antora example shows the example's lines, read-only, and
    // leaves the directive in the source; the ordinary listing after it stays editable.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("antora.yml"), "name: demo\nversion: ~\n").unwrap();
    let module = root.join("modules/ROOT");
    fs::create_dir_all(module.join("pages")).unwrap();
    fs::create_dir_all(module.join("examples/sql")).unwrap();
    fs::write(
        module.join("examples/sql/query.sql"),
        "SELECT id\nFROM users;\n",
    )
    .unwrap();
    let source = "= Queries\n\n[source,sql]\n----\ninclude::example$sql/query.sql[]\n----\n\n\
                  [source,sql]\n----\nSELECT 1;\n----\n";
    let path = page(&module.join("pages"), "queries.adoc", source);
    let (mut engine, _) = engine_with_clock();
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);

    let visual = engine.visual(id).expect("a visual model");
    let included = &visual.model.blocks[1];
    assert_eq!(
        included.kind,
        adoc_model::NodeKind::Verbatim {
            name: "listing".to_owned()
        }
    );
    assert!(!included.is_editable());
    let block = adoc_render::block_by_trail(&visual.view, included.origin.as_deref().unwrap())
        .expect("the listing in the view tree");
    let adoc_render::ViewKind::Code { text: shown, .. } = &block.kind else {
        panic!("{:?}", block.kind);
    };
    assert_eq!(shown.trim_end(), "SELECT id\nFROM users;");
    let ordinary = &visual.model.blocks[2];
    assert!(matches!(ordinary.kind, adoc_model::NodeKind::Code { .. }));
    assert!(ordinary.is_editable());
    assert_eq!(text(&engine, id), source);
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

const INSERTS: [adoc_engine::CommandId; 5] = [
    ids::EDIT_INSERT_TABLE,
    ids::EDIT_INSERT_CODE_BLOCK,
    ids::EDIT_INSERT_ADMONITION,
    ids::EDIT_INSERT_LINK,
    ids::EDIT_INSERT_IMAGE,
];

fn source_insert(
    engine: &mut Engine,
    command: adoc_engine::CommandId,
    id: DocumentId,
    range: SourceRange,
) -> SourceRange {
    match engine
        .invoke(command, CommandArgs::Range { id, range })
        .unwrap()
    {
        CommandOutcome::Selection(selection) => selection,
        other => panic!("unexpected outcome {other:?}"),
    }
}

/// Replaces the selection with typed text, as the editor would.
fn type_over(engine: &mut Engine, id: DocumentId, selection: SourceRange, typed: &str) {
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: selection,
                text: typed.to_owned(),
            },
        )
        .unwrap();
}

#[test]
fn insert_commands_are_offered_in_source_mode() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, _clock) = engine_with_clock();
    engine
        .open(&page(directory.path(), "page.adoc", "= Page\n"))
        .unwrap();
    assert_eq!(
        engine.editor_mode(engine.active().unwrap()),
        EditorMode::Source
    );
    for command in INSERTS {
        assert!(engine.command_available(command), "{command}");
    }
}

#[test]
fn a_table_inserted_in_source_is_the_visual_editor_s_table() {
    let start = "= Page\n\nA paragraph.\n";
    let end_of_paragraph = start.len() - 1;
    let directory = tempfile::tempdir().unwrap();

    // In the source editor: insert at the caret, then type the first header cell.
    let (mut engine, _clock) = engine_with_clock();
    let id = engine
        .open(&page(directory.path(), "source.adoc", start))
        .unwrap();
    let caret = SourceRange::new(end_of_paragraph, end_of_paragraph);
    let selection = source_insert(&mut engine, ids::EDIT_INSERT_TABLE, id, caret);
    type_over(&mut engine, id, selection, "Name");
    let from_source = text(&engine, id);

    // In the visual editor, the same.
    let (mut engine, _clock) = engine_with_clock();
    let id = engine
        .open(&page(directory.path(), "visual.adoc", start))
        .unwrap();
    show_visual(&mut engine, id);
    let selection = visual(
        &mut engine,
        ids::EDIT_INSERT_TABLE,
        id,
        caret,
        VisualValue::Table {
            columns: 2,
            rows: 2,
        },
    )
    .unwrap();
    type_over(&mut engine, id, selection, "Name");
    assert_eq!(from_source, text(&engine, id));
    assert!(
        from_source.contains("|===\n|Name |Column 2\n"),
        "{from_source}"
    );
}

#[test]
fn each_insert_is_one_undo_step_in_source() {
    let start = "= Page\n\nfirst line\nsecond line\n";
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let id = engine
        .open(&page(directory.path(), "page.adoc", start))
        .unwrap();
    let end = start.len();

    let selection = source_insert(
        &mut engine,
        ids::EDIT_INSERT_ADMONITION,
        id,
        SourceRange::new(end, end),
    );
    let after = text(&engine, id);
    assert!(after.ends_with("second line\n\nNOTE: Note.\n"), "{after}");
    assert_eq!(&after[selection.start..selection.end], "Note.");
    assert!(engine.undo(id));
    assert_eq!(text(&engine, id), start);

    // A code block goes around the selected lines.
    let lines = SourceRange::new(start.find("first").unwrap(), end - 1);
    source_insert(&mut engine, ids::EDIT_INSERT_CODE_BLOCK, id, lines);
    assert_eq!(
        text(&engine, id),
        "= Page\n\n----\nfirst line\nsecond line\n----\n"
    );
    assert!(engine.undo(id));

    // A link around the selected text, the caret at its target.
    let word = SourceRange::new(start.find("first").unwrap(), start.find(" line").unwrap());
    let caret = source_insert(&mut engine, ids::EDIT_INSERT_LINK, id, word);
    type_over(&mut engine, id, caret, "https://example.com");
    assert!(
        text(&engine, id).contains("link:https://example.com[first] line"),
        "{}",
        text(&engine, id)
    );
    assert!(engine.undo(id));
    assert!(engine.undo(id));
    assert_eq!(text(&engine, id), start);

    // An image at the caret, the caret at its target.
    let caret = source_insert(
        &mut engine,
        ids::EDIT_INSERT_IMAGE,
        id,
        SourceRange::new(end, end),
    );
    type_over(&mut engine, id, caret, "flow.png");
    assert!(text(&engine, id).ends_with("second line\n\nimage::flow.png[]\n"));
}

fn source_insert_at(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(at, at),
                text: typed.to_owned(),
            },
        )
        .unwrap();
}

#[test]
fn a_model_in_the_background_is_rebuilt_when_it_is_shown() {
    let directory = tempfile::tempdir().unwrap();
    let first = page(directory.path(), "first.adoc", "First.\n");
    let second = page(directory.path(), "second.adoc", "Second.\n");
    let (mut engine, clock) = engine_with_clock();
    let second_id = engine.open(&second).unwrap();
    show_visual(&mut engine, second_id);
    let first_id = engine.open(&first).unwrap();
    show_visual(&mut engine, first_id);
    assert_eq!(engine.active(), Some(first_id));

    source_insert_at(&mut engine, second_id, "Second".len(), " page");
    clock.advance(MODEL_SETTLE);
    engine.pump();
    assert_eq!(
        shown(&engine, second_id),
        ["Second."],
        "not shown, not rebuilt"
    );
    assert!(!engine.has_pending(), "nothing waits for it either");

    engine.set_active(second_id);
    assert_eq!(shown(&engine, second_id), ["Second page."]);
    assert_eq!(
        engine.visual(second_id).unwrap().version,
        version(&engine, second_id)
    );
}

#[test]
fn a_command_that_changes_nothing_leaves_a_settling_model_to_settle() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(directory.path(), "page.adoc", "First.\n");
    let (mut engine, clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    show_visual(&mut engine, id);
    source_insert_at(&mut engine, id, "First".len(), " and second");
    let receiver = engine.subscribe();

    engine.invoke(ids::APP_ABOUT, CommandArgs::None).unwrap();
    assert!(
        !events(&receiver)
            .iter()
            .any(|event| matches!(event, EngineEvent::ModelChanged { .. })),
        "a read-only command rebuilds nothing"
    );
    assert_eq!(shown(&engine, id), ["First."]);

    clock.advance(MODEL_SETTLE);
    engine.pump();
    assert_eq!(shown(&engine, id), ["First and second."]);
}

#[test]
fn closing_a_tab_shows_the_next_page_s_model_current() {
    let directory = tempfile::tempdir().unwrap();
    let first = page(directory.path(), "first.adoc", "First.\n");
    let second = page(directory.path(), "second.adoc", "Second.\n");
    let (mut engine, clock) = engine_with_clock();
    let second_id = engine.open(&second).unwrap();
    show_visual(&mut engine, second_id);
    let first_id = engine.open(&first).unwrap();
    source_insert_at(&mut engine, second_id, "Second".len(), " page");
    clock.advance(MODEL_SETTLE);
    engine.pump();

    engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(first_id))
        .unwrap();
    assert_eq!(engine.active(), Some(second_id));
    assert_eq!(shown(&engine, second_id), ["Second page."]);
}
