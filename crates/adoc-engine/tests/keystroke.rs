//! What a typed character costs: nothing the whole document's size decides. Counted with
//! `adoc_core::probe`, so a reintroduced whole-document pass fails however fast the machine.

use std::{fmt::Write as _, fs, sync::Arc, time::Duration};

use adoc_core::{
    probe::{self, Probe},
    SourceRange,
};
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, ManualClock, Settings, INDEX_SETTLE,
};

/// A page of a few thousand lines, open in a workspace, with a manual clock.
fn page() -> (tempfile::TempDir, Engine, ManualClock, DocumentId) {
    let directory = tempfile::tempdir().unwrap();
    let mut text = String::from("= Page\n:product: Foundry\n\n");
    for section in 1..=300 {
        let _ = write!(
            text,
            "== Section {section}\n\nSee <<section-{section}>> and xref:other.adoc[].\n\n* one\n* two\n\n"
        );
    }
    fs::write(directory.path().join("page.adoc"), &text).unwrap();
    fs::write(directory.path().join("other.adoc"), "= Other\n").unwrap();
    let clock = ManualClock::new();
    let settings = Settings::load(None, None).unwrap().with_git_opt_in();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&directory.path().join("page.adoc")).unwrap();
    (directory, engine, clock, id)
}

fn type_char(engine: &mut Engine, id: DocumentId, at: usize, typed: &str) {
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
fn one_keystroke_parses_at_most_once() {
    let (_directory, mut engine, clock, id) = page();
    let at = engine
        .document(id)
        .unwrap()
        .text_string()
        .find("See ")
        .unwrap();
    probe::reset();
    for (index, typed) in "Typed words".chars().enumerate() {
        type_char(&mut engine, id, at + index, &typed.to_string());
    }
    // Typing waits for its pause; the pause parses the page once, for the index.
    assert_eq!(probe::count(Probe::Parse), 0);
    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert_eq!(probe::count(Probe::Parse), 1);
    assert!(engine
        .workspace()
        .index
        .files()
        .any(|file| file.document.text.contains("Typed wordsSee")));
}

#[test]
fn typing_never_hashes_the_document() {
    let (_directory, mut engine, _clock, id) = page();
    probe::reset();
    for index in 0..20 {
        type_char(&mut engine, id, 10 + index, "x");
    }
    assert_eq!(probe::count(Probe::HashText), 0);
    assert!(engine.document(id).unwrap().dirty);

    // Typed back to the saved text, the page is clean again: only then is it hashed.
    let text = engine.document(id).unwrap().text_string();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(10, 30),
                text: String::new(),
            },
        )
        .unwrap();
    assert!(text.len() > 20);
    assert!(!engine.document(id).unwrap().dirty);
    assert_eq!(probe::count(Probe::HashText), 1);
}

#[test]
fn completion_during_typing_does_not_reparse() {
    let (_directory, mut engine, _clock, id) = page();
    let at = engine.document(id).unwrap().text.len();
    probe::reset();
    for (index, typed) in "\n<<sec".chars().enumerate() {
        type_char(&mut engine, id, at + index, &typed.to_string());
        let caret = at + index + 1;
        let _ = engine.completions(id, caret);
        let _ = engine.hover(id, caret);
    }
    // Each keystroke's completion reads the page as typed: one parse of it, not one per ask.
    assert!(
        probe::count(Probe::Parse) <= 6,
        "{}",
        probe::count(Probe::Parse)
    );
    assert!(!engine.completions(id, at + 6).is_empty());
    let parses = probe::count(Probe::Parse);
    let _ = engine.completions(id, at + 6);
    let _ = engine.hover(id, at + 6);
    let _ = engine.definition(id, at + 6);
    assert_eq!(
        probe::count(Probe::Parse),
        parses,
        "asking again parses nothing"
    );
}

#[test]
fn only_typing_inside_something_that_completes_asks_for_completions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    fs::write(&path, "= Page\n\nSome prose and xref:\nThe {pro and <<\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    let id = engine.open(&path).unwrap();
    let text = engine.document(id).unwrap().text_string();
    let after = |needle: &str| text.find(needle).unwrap() + needle.len();
    assert!(!engine.completes_after(id, after("Some pro"), "s"));
    assert!(!engine.completes_after(id, after("Some prose"), " "));
    assert!(engine.completes_after(id, after("xref:"), "g"));
    assert!(engine.completes_after(id, after("{pro"), "d"));
    assert!(engine.completes_after(id, after("<<"), "s"));
    // Past the end, or inside a character, is harmless.
    assert!(!engine.completes_after(id, 10_000, "x"));
    let _ = engine.completes_after(id, 1, "é");
}
