//! Spell checking of the prose of open pages, once typing settles.

use std::{fs, path::PathBuf, sync::Arc, time::Duration};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, EngineEvent, ManualClock, Settings,
    SPELLING_SETTLE,
};

const WAIT: Duration = Duration::from_secs(30);

fn engine_with(settings: Settings) -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(settings.with_git_opt_in(), Arc::new(clock.clone()));
    engine.enable_spelling();
    (engine, clock)
}

fn engine_with_clock() -> (Engine, ManualClock) {
    engine_with(Settings::load(None, None).unwrap())
}

/// Pumps until the workers are idle, as a front end's timer would.
fn settle(engine: &mut Engine) {
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    engine.pump();
}

fn page(directory: &tempfile::TempDir, name: &str, text: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, text).unwrap();
    path
}

fn misspelt(engine: &Engine, id: DocumentId) -> Vec<String> {
    engine
        .misspellings(id)
        .iter()
        .map(|misspelling| misspelling.word.clone())
        .collect()
}

fn type_text(engine: &mut Engine, id: DocumentId, offset: usize, text: &str) {
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

#[test]
fn a_misspelt_word_in_a_paragraph_is_found() {
    let directory = tempfile::tempdir().unwrap();
    let text = "= Guide\n\nWe recieve the colour.\n";
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let found = engine.misspellings(id);
    assert_eq!(found.len(), 1, "{found:?}");
    let start = text.find("recieve").unwrap();
    assert_eq!(found[0].word, "recieve");
    assert_eq!(
        found[0].range,
        SourceRange::new(start, start + "recieve".len())
    );
    assert!(found[0].suggestions.iter().any(|word| word == "receive"));
}

#[test]
fn words_in_a_source_block_are_never_flagged() {
    let directory = tempfile::tempdir().unwrap();
    let text = concat!(
        "= Guide\n",
        ":product: Foundary\n\n",
        "[source,rust]\n",
        "----\n",
        "let qwzxa = recieve();\n",
        "----\n\n",
        "....\n",
        "literal qwzxb\n",
        "....\n\n",
        "Use `qwzxc` and xref:other.adoc[qwzxd] and https://example.com/qwzxe[] and {product}.\n\n",
        "Text with qwzxy.\n",
    );
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxy"]);
}

#[test]
fn included_text_is_checked_in_its_own_file() {
    let directory = tempfile::tempdir().unwrap();
    page(&directory, "part.adoc", "Included qwzxa.\n");
    let path = page(
        &directory,
        "page.adoc",
        "= Page\n\ninclude::part.adoc[]\n\nOwn qwzxy.\n",
    );
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxy"]);
}

#[test]
fn a_word_split_by_marks_or_touching_a_reference_is_not_checked() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(
        &directory,
        "page.adoc",
        "= Page\n:x: y\n\n**qw**zxy and {x}qwzxb and *qwzxc* here.\n",
    );
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxc"]);
}

#[test]
fn the_page_language_chooses_the_dictionary() {
    let directory = tempfile::tempdir().unwrap();
    let british = page(
        &directory,
        "gb.adoc",
        "= Page\n\nThe color and the colour.\n",
    );
    let american = page(
        &directory,
        "us.adoc",
        "= Page\n:lang: en-US\n\nThe color and the colour.\n",
    );
    let german = page(&directory, "de.adoc", "= Seite\n:lang: de\n\nThe qwzxy.\n");
    let (mut engine, _clock) = engine_with_clock();
    let british = engine.open(&british).unwrap();
    let american = engine.open(&american).unwrap();
    let german = engine.open(&german).unwrap();
    for _ in 0..3 {
        settle(&mut engine); // one page a job
    }
    assert_eq!(misspelt(&engine, british), ["color"]);
    assert_eq!(misspelt(&engine, american), ["colour"]);
    assert!(misspelt(&engine, german).is_empty(), "no German dictionary");
}

#[test]
fn nothing_is_checked_without_enable_spelling() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(&directory, "page.adoc", "= Page\n\nA qwzxy.\n");
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(
        Settings::load(None, None).unwrap().with_git_opt_in(),
        Arc::new(clock),
    );
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert!(engine.misspellings(id).is_empty());
    assert!(!engine.has_pending());
}

#[test]
fn checking_waits_for_typing_to_settle() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(&directory, "page.adoc", "= Page\n\nOne two.\n");
    let (mut engine, clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert!(misspelt(&engine, id).is_empty());
    assert!(!engine.has_pending());

    type_text(&mut engine, id, "= Page\n\nOne two".len(), " qwzxy");
    clock.advance(SPELLING_SETTLE - Duration::from_millis(1));
    settle(&mut engine);
    assert!(misspelt(&engine, id).is_empty(), "not yet");
    clock.advance(Duration::from_millis(1));
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxy"]);
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::SpellingChanged(id)));
}

#[test]
fn misspellings_move_with_edits_and_go_when_typed_into() {
    let directory = tempfile::tempdir().unwrap();
    let text = "= Page\n\nA qwzxy word.\n";
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let start = text.find("qwzxy").unwrap();
    assert_eq!(
        engine.misspellings(id)[0].range,
        SourceRange::new(start, start + 5)
    );

    type_text(&mut engine, id, text.find("A qwzxy").unwrap(), "Big ");
    assert_eq!(
        engine.misspellings(id)[0].range,
        SourceRange::new(start + 4, start + 9),
        "moved before any check"
    );
    type_text(&mut engine, id, start + 9, "s");
    assert!(engine.misspellings(id).is_empty(), "typed into");
}

#[test]
fn a_misspelt_word_is_replaced_in_one_undo_step() {
    let directory = tempfile::tempdir().unwrap();
    let text = "= Page\n\nWe recieve it.\n";
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let misspelling = engine.misspellings(id)[0].clone();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: misspelling.range,
                text: "receive".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(
        engine.document(id).unwrap().text_string(),
        "= Page\n\nWe receive it.\n"
    );
    assert!(engine.misspellings(id).is_empty());
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(engine.document(id).unwrap().text_string(), text);
}

#[test]
fn nothing_is_checked_when_spelling_is_off() {
    let directory = tempfile::tempdir().unwrap();
    let user = page(&directory, "settings.toml", "[spelling]\nenabled = false\n");
    let path = page(&directory, "page.adoc", "= Page\n\nA qwzxy.\n");
    let (mut engine, _clock) = engine_with(Settings::load(Some(&user), None).unwrap());
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert!(engine.misspellings(id).is_empty());
    assert!(!engine.has_pending());
}
