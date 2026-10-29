//! Format Table on the table at the caret.

use std::time::Duration;

use adoc_engine::{commands::ids, CommandArgs, Engine, EngineError, Settings};

const RAGGED: &str = "\
= Tables

.Settings
[cols=\"1,3\",options=\"header\"]
|===
|Key|Meaning

|`auto_save`|When pages are saved
|dot | Graphviz's `dot`, *if* set
|d2|
|===
";

const ALIGNED: &str = "\
= Tables

.Settings
[cols=\"1,3\",options=\"header\"]
|===
| Key         | Meaning

| `auto_save` | When pages are saved
| dot         | Graphviz's `dot`, *if* set
| d2          |
|===
";

fn engine_with(text: &str) -> (tempfile::TempDir, Engine, adoc_engine::DocumentId) {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, text).unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    (directory, engine, id)
}

fn text(engine: &Engine, id: adoc_engine::DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

fn html(engine: &mut Engine, id: adoc_engine::DocumentId) -> String {
    let rendered = engine.render_blocking(id, Duration::from_secs(10)).unwrap();
    adoc_render::to_html(
        &rendered.view,
        &adoc_render::HtmlOptions { standalone: false },
    )
}

fn format(
    engine: &mut Engine,
    id: adoc_engine::DocumentId,
    offset: usize,
) -> Result<(), EngineError> {
    engine
        .invoke(ids::EDIT_FORMAT_TABLE, CommandArgs::Offset { id, offset })
        .map(|_| ())
}

#[test]
fn formatting_a_ragged_table_aligns_it_and_renders_the_same() {
    let (_directory, mut engine, id) = engine_with(RAGGED);
    let before = html(&mut engine, id);
    assert!(before.contains("Graphviz"), "{before}");

    format(&mut engine, id, RAGGED.find("|dot").unwrap()).unwrap();
    assert_eq!(text(&engine, id), ALIGNED);
    assert_eq!(html(&mut engine, id), before, "the table renders as before");

    // Formatting it again changes nothing, and adds nothing to undo.
    format(&mut engine, id, ALIGNED.find("| dot").unwrap()).unwrap();
    assert_eq!(text(&engine, id), ALIGNED);
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), RAGGED, "one undo step");
}

#[test]
fn a_table_it_cannot_format_is_left_alone_with_the_reason() {
    let spans = "|===\n|a 2+|b\n|c|d|e\n|===\n";
    let (_directory, mut engine, id) = engine_with(spans);
    let refused = format(&mut engine, id, 6).unwrap_err().to_string();
    assert!(refused.contains("span"), "{refused}");
    assert_eq!(text(&engine, id), spans);

    let missing = format(&mut engine, id, spans.len()).unwrap_err();
    assert!(matches!(missing, EngineError::NotFound(_)), "{missing}");
}
