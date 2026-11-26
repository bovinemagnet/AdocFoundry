//! Attribute values shown beside their references (#87): what `Engine::attribute_values` gives
//! for the lines a front end shows, and that asking costs typing nothing.

use std::{path::Path, sync::Arc, time::Duration};

use adoc_core::{
    probe::{self, Probe},
    SourceRange,
};
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, ManualClock, Settings, INDEX_SETTLE,
};

/// The effective-attributes fixture open in place, Git off, its index page open.
fn open_index() -> (Engine, ManualClock, DocumentId) {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/effective-attributes");
    let clock = ManualClock::new();
    let settings = Settings::load(None, None).unwrap().with_git_opt_in();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine
        .open(&root.join("docs/modules/ROOT/pages/index.adoc"))
        .unwrap();
    (engine, clock, id)
}

/// Each value in the whole of `id` as the reference written, the name and the value.
fn values(engine: &Engine, id: DocumentId) -> Option<Vec<(String, String, String)>> {
    let text = engine.document(id)?.text_string();
    let values = engine.attribute_values(id, SourceRange::new(0, text.len()))?;
    Some(
        values
            .into_iter()
            .map(|value| {
                (
                    text[value.range.start..value.range.end].to_owned(),
                    value.name,
                    value.value,
                )
            })
            .collect(),
    )
}

fn owned(expected: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    expected
        .iter()
        .map(|(written, name, value)| {
            (
                (*written).to_owned(),
                (*name).to_owned(),
                (*value).to_owned(),
            )
        })
        .collect()
}

#[test]
fn attribute_values_resolve_through_the_page_component_and_playbook() {
    let (engine, _clock, id) = open_index();

    assert_eq!(
        values(&engine, id),
        Some(owned(&[
            ("{edition}", "edition", "Enterprise"),
            ("{product}", "product", "Guide"),
            ("{company}", "company", "Foundry Ltd"),
            ("{locked}", "locked", "site"),
            ("{support}", "support", "help desk"),
            ("{edition}", "edition", "Later"),
        ]))
    );
}

/// While typing waits for its pause there is no current parse, and asking parses nothing on
/// the engine's thread: the front end keeps what it showed. Once the pause has parsed the page,
/// the values follow the text.
#[test]
fn attribute_values_wait_for_typing_to_pause() {
    let (mut engine, clock, id) = open_index();
    let text = engine.document(id).unwrap().text_string();
    let at = text.find(":edition: Enterprise").unwrap() + ":edition: ".len();

    probe::reset();
    for (index, typed) in "Cloud ".chars().enumerate() {
        engine
            .invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(at + index, at + index),
                    text: typed.to_string(),
                },
            )
            .unwrap();
    }
    assert_eq!(values(&engine, id), None);
    assert_eq!(probe::count(Probe::Parse), 0, "asking parsed the page");

    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    let found = values(&engine, id).expect("the pause parsed the page");
    assert_eq!(
        found[0],
        owned(&[("{edition}", "edition", "Cloud Enterprise")])[0]
    );
    assert_eq!(
        probe::count(Probe::Parse),
        0,
        "nothing parsed on this thread"
    );
}

#[test]
fn attribute_values_are_off_until_asked_for() {
    assert!(!Settings::default().editor.attribute_values);
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("settings.toml");
    std::fs::write(&user, "[editor]\nattribute_values = true\n").unwrap();
    let settings = Settings::load(Some(&user), None).unwrap();
    assert!(settings.editor.attribute_values);
    assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    assert!(adoc_engine::setting_specs()
        .iter()
        .any(|spec| spec.id == "editor.attribute_values"));
}
