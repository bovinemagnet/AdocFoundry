//! The active page's word count and reading time, counted once typing settles.

use std::{fs, sync::Arc, time::Duration};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, count_words, CommandArgs, DocumentId, Engine, EngineEvent, ManualClock,
    Settings, WordCount, WORD_COUNT_SETTLE,
};

const WAIT: Duration = Duration::from_secs(10);

fn engine_with_clock() -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(
        Settings::load(None, None).unwrap().with_git_opt_in(),
        Arc::new(clock.clone()),
    );
    engine.enable_word_count();
    (engine, clock)
}

/// Pumps until the workers are idle, as a front end's timer would.
fn settle(engine: &mut Engine) {
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    engine.pump();
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
fn words_are_runs_holding_a_letter_or_digit() {
    assert_eq!(count_words(""), 0);
    assert_eq!(count_words("well-known words — and 42 ----"), 4);
    assert_eq!(count_words("  *bold*  _it_\n`code`  "), 3);
}

#[test]
fn reading_time_rounds_up_to_whole_minutes() {
    assert_eq!(WordCount::new(0).minutes, 0);
    assert_eq!(WordCount::new(1).minutes, 1);
    assert_eq!(WordCount::new(200).minutes, 1);
    assert_eq!(WordCount::new(201).minutes, 2);
}

#[test]
fn only_prose_is_counted() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(
        &page,
        concat!(
            "= A Title Here\n",
            ":product: Foundry\n\n",
            "Intro paragraph with *five* words.\n\n",
            "== Section Two\n\n",
            "* one item\n",
            "* second item here\n\n",
            "[source,rust]\n",
            "----\n",
            "fn main() { println!(\"not counted\"); }\n",
            "----\n\n",
            "NOTE: Admonition text counts.\n\n",
            "[mermaid]\n",
            "....\n",
            "graph TD; A-->B\n",
            "....\n\n",
            "|===\n",
            "|Cell one |Cell two\n",
            "|===\n",
        ),
    )
    .unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&page).unwrap();
    settle(&mut engine);

    // 3 in the title, 5, 2, 2 + 3 in the list, 3 in the note and 4 in the table.
    assert_eq!(engine.word_count(id), Some(WordCount::new(22)));
}

#[test]
fn the_count_follows_typing_once_it_settles() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nOne two three.\n").unwrap();
    let (mut engine, clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();

    assert!(engine.has_pending(), "a count is due");
    settle(&mut engine);
    assert_eq!(
        engine.word_count(id),
        Some(WordCount::new(4)),
        "a page just opened is counted at once"
    );
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::WordCountChanged(id)));
    assert!(!engine.has_pending());

    let end = "= Page\n\nOne two three".len();
    type_text(&mut engine, id, end, " four");
    clock.advance(WORD_COUNT_SETTLE - Duration::from_millis(1));
    settle(&mut engine);
    type_text(&mut engine, id, end + " four".len(), " five");
    clock.advance(WORD_COUNT_SETTLE - Duration::from_millis(1));
    settle(&mut engine);
    assert_eq!(
        engine.word_count(id),
        Some(WordCount::new(4)),
        "the last count stays while typing goes on"
    );
    clock.advance(Duration::from_millis(1));
    settle(&mut engine);
    assert_eq!(engine.word_count(id), Some(WordCount::new(6)));
}

#[test]
fn only_the_active_page_is_counted() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.adoc");
    let second = directory.path().join("second.adoc");
    fs::write(&first, "= First\n\nOne.\n").unwrap();
    fs::write(&second, "= Second\n\nOne two.\n").unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let first = engine.open(&first).unwrap();
    let second = engine.open(&second).unwrap();
    settle(&mut engine);
    assert_eq!(engine.word_count(first), None);
    assert_eq!(engine.word_count(second), Some(WordCount::new(3)));

    // Switching shows the other page's count without waiting.
    engine.set_active(first);
    settle(&mut engine);
    assert_eq!(engine.word_count(first), Some(WordCount::new(2)));
}

#[test]
fn front_ends_that_do_not_ask_count_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nWords.\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    settle(&mut engine);
    assert!(!engine.has_pending());
    assert_eq!(engine.word_count(id), None);
}

/// The words of a rendered page's text without what the renderer adds: list numbers and
/// markers, footnote markers and the "Table 1." of a caption.
fn reader_words(text: &str) -> usize {
    let generated = |word: &str| {
        let number = |inner: &str| !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit());
        word.strip_suffix('.').is_some_and(number)
            || word
                .strip_prefix('[')
                .and_then(|word| word.strip_suffix(']'))
                .is_some_and(|inner| number(inner) || inner == "x" || inner == " ")
    };
    text.lines()
        .map(|line| {
            let mut words: Vec<&str> = line.split_whitespace().collect();
            if words.first() == Some(&"Table") && words.get(1).is_some_and(|word| generated(word)) {
                words.remove(0);
            }
            count_words(
                &words
                    .into_iter()
                    .filter(|word| !generated(word))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        })
        .sum()
}

#[test]
fn the_count_matches_the_rendered_text() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for page in [
        "render/tables.adoc",
        "render/inline.adoc",
        "antora-multi-component/alpha/modules/ROOT/pages/index.adoc",
        "antora-multi-component/alpha/modules/ROOT/pages/guide.adoc",
    ] {
        let (mut engine, _clock) = engine_with_clock();
        let id = engine.open(&root.join(page)).unwrap();
        settle(&mut engine);
        let rendered = engine.render_blocking(id, WAIT).unwrap();
        let rendered = reader_words(&adoc_engine::to_text(&rendered.view));
        assert_eq!(
            engine.word_count(id).map(|count| count.words),
            Some(rendered),
            "{page}"
        );
    }
}
