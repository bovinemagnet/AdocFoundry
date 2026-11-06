//! Which edits wait for a pause before they are indexed and diagnosed, and which documents a
//! change re-diagnoses.

use std::{fs, path::Path, sync::Arc, time::Duration};

use adoc_core::{DiagnosticCode, SourceRange};
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, ManualClock, MarkSet, Settings, VisualArgs,
    VisualValue, INDEX_SETTLE,
};

fn engine(root: &Path) -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let settings = Settings::load(None, None).unwrap().with_git_opt_in();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    (engine, clock)
}

/// The text the index holds for `path`.
fn indexed(engine: &Engine, path: &Path) -> String {
    engine
        .workspace()
        .index
        .file(&adoc_engine::normalize_path(path))
        .unwrap()
        .document
        .text
        .to_string()
}

fn replace(engine: &mut Engine, id: DocumentId, range: SourceRange, text: &str) {
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range,
                text: text.to_owned(),
            },
        )
        .unwrap();
}

fn codes(engine: &Engine, id: DocumentId) -> Vec<DiagnosticCode> {
    engine
        .document(id)
        .unwrap()
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn deleting_one_byte_defers_the_index() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSome words.\n").unwrap();
    let (mut engine, clock) = engine(directory.path());
    let id = engine.open(&page).unwrap();

    // Backspace twice: neither deletion is indexed, the second not flushing the first.
    replace(&mut engine, id, SourceRange::new(18, 19), "");
    replace(&mut engine, id, SourceRange::new(17, 18), "");
    assert_eq!(
        engine.document(id).unwrap().text_string(),
        "= Page\n\nSome word\n"
    );
    assert_eq!(indexed(&engine, &page), "= Page\n\nSome words.\n");
    assert!(engine.has_pending());

    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert_eq!(indexed(&engine, &page), "= Page\n\nSome word\n");
}

#[test]
fn visual_typing_defers_the_index() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSome words.\n").unwrap();
    let (mut engine, clock) = engine(directory.path());
    let id = engine.open(&page).unwrap();
    engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
        .unwrap();
    for (at, typed) in [(13, "x"), (14, "y")] {
        let args = VisualArgs {
            id,
            version: engine.document(id).unwrap().version,
            selection: SourceRange::new(at, at),
            marks: MarkSet::EMPTY,
            value: VisualValue::Text(typed.to_owned()),
        };
        engine
            .invoke(ids::VISUAL_INSERT_TEXT, CommandArgs::Visual(args))
            .unwrap();
    }
    assert_eq!(
        engine.document(id).unwrap().text_string(),
        "= Page\n\nSome xywords.\n"
    );
    assert_eq!(indexed(&engine, &page), "= Page\n\nSome words.\n");

    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert_eq!(indexed(&engine, &page), "= Page\n\nSome xywords.\n");
}

#[test]
fn a_paste_or_a_command_that_reads_the_index_is_indexed_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSome words.\n").unwrap();
    let (mut engine, _clock) = engine(directory.path());
    let id = engine.open(&page).unwrap();

    replace(
        &mut engine,
        id,
        SourceRange::new(8, 8),
        "[[pasted]]\nA pasted line.\n",
    );
    assert!(indexed(&engine, &page).contains("[[pasted]]"));

    // A deferred deletion is indexed before a command that reads the index.
    replace(&mut engine, id, SourceRange::new(0, 1), "");
    assert!(indexed(&engine, &page).starts_with('='));
    engine
        .invoke(
            ids::NAVIGATE_GO_TO_SYMBOL,
            CommandArgs::Query {
                query: "pasted".to_owned(),
                document: None,
            },
        )
        .unwrap();
    assert!(!indexed(&engine, &page).starts_with('='));
}

#[test]
fn diagnostics_refresh_only_referencing_documents() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("target.adoc"), "= Target\n\nNo anchor yet.\n").unwrap();
    // Links to an anchor the target does not have yet.
    fs::write(
        root.join("linker.adoc"),
        "= Linker\n\nSee xref:target.adoc#later[].\n",
    )
    .unwrap();
    // A part whose image is judged only while nothing includes it.
    fs::write(root.join("part.adoc"), "image::missing.png[]\n").unwrap();
    fs::write(root.join("book.adoc"), "= Book\n\nNothing included yet.\n").unwrap();
    // Has nothing to do with any of them.
    fs::write(
        root.join("other.adoc"),
        "= Other\n\nSee xref:missing.adoc[].\n",
    )
    .unwrap();
    let (mut engine, clock) = engine(root);
    let target = engine.open(&root.join("target.adoc")).unwrap();
    let linker = engine.open(&root.join("linker.adoc")).unwrap();
    let part = engine.open(&root.join("part.adoc")).unwrap();
    let book = engine.open(&root.join("book.adoc")).unwrap();
    let other = engine.open(&root.join("other.adoc")).unwrap();
    assert_eq!(codes(&engine, linker), [DiagnosticCode::UnresolvedAnchor]);
    assert_eq!(codes(&engine, part), [DiagnosticCode::UnresolvedImage]);
    let unrelated = codes(&engine, other);
    assert_eq!(unrelated, [DiagnosticCode::UnresolvedXrefFile]);

    // The target gains the anchor, by typing (deferred) and by a paste (at once): the page
    // linking to it is re-diagnosed.
    for (at, typed) in [(10, "["), (11, "[later]]\n")] {
        replace(&mut engine, target, SourceRange::new(at, at), typed);
    }
    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert_eq!(codes(&engine, linker), Vec::<DiagnosticCode>::new());
    assert_eq!(codes(&engine, other), unrelated);

    // And loses it again.
    let text = engine.document(target).unwrap().text_string();
    let start = text.find("[[later]]").unwrap();
    replace(
        &mut engine,
        target,
        SourceRange::new(start, start + "[[later]]\n".len()),
        "",
    );
    assert_eq!(codes(&engine, linker), [DiagnosticCode::UnresolvedAnchor]);

    // The book starts including the part, and stops: the part it composes is re-diagnosed.
    replace(
        &mut engine,
        book,
        SourceRange::new(8, 8),
        "include::part.adoc[]\n\n",
    );
    assert_eq!(codes(&engine, part), Vec::<DiagnosticCode>::new());
    replace(
        &mut engine,
        book,
        SourceRange::new(8, 8 + "include::part.adoc[]\n\n".len()),
        "",
    );
    assert_eq!(codes(&engine, part), [DiagnosticCode::UnresolvedImage]);
    assert_eq!(codes(&engine, other), unrelated);
}

/// A page typed into, and a page linking to an anchor it is about to gain.
fn typed_pages() -> (
    tempfile::TempDir,
    Engine,
    ManualClock,
    DocumentId,
    DocumentId,
) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("target.adoc"), "= Target\n\nNo anchor yet.\n").unwrap();
    fs::write(
        root.join("linker.adoc"),
        "= Linker\n\nSee xref:target.adoc#later[].\n",
    )
    .unwrap();
    let (mut engine, clock) = engine(root);
    let target = engine.open(&root.join("target.adoc")).unwrap();
    let linker = engine.open(&root.join("linker.adoc")).unwrap();
    assert_eq!(codes(&engine, linker), [DiagnosticCode::UnresolvedAnchor]);
    // Typed a character at a time, so nothing is indexed before the pause.
    for (at, typed) in (10..).zip("[[later]]\n".chars()) {
        replace(
            &mut engine,
            target,
            SourceRange::new(at, at),
            &typed.to_string(),
        );
    }
    assert_eq!(codes(&engine, linker), [DiagnosticCode::UnresolvedAnchor]);
    (directory, engine, clock, target, linker)
}

#[test]
fn the_pause_parses_the_page_on_a_worker() {
    use adoc_core::probe::{self, Probe};

    let (directory, mut engine, clock, target, linker) = typed_pages();
    let page = directory.path().join("target.adoc");
    probe::reset();
    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert!(engine.has_pending(), "the worker is still to answer");
    assert!(engine.wait_for_workers(Duration::from_secs(10)));

    assert!(indexed(&engine, &page).contains("[[later]]"));
    assert_eq!(codes(&engine, linker), Vec::<DiagnosticCode>::new());
    // The worker's parse is the page's: reading it parses nothing more.
    assert!(engine
        .document(target)
        .unwrap()
        .parsed()
        .text
        .contains("[[later]]"));
    assert_eq!(
        probe::count(Probe::Parse),
        0,
        "nothing parsed on this thread"
    );
}

#[test]
fn typing_while_the_pause_is_indexed_waits_for_the_next() {
    let (directory, mut engine, clock, target, linker) = typed_pages();
    let page = directory.path().join("target.adoc");
    clock.advance(INDEX_SETTLE);
    engine.pump();
    // Typed while the worker parses: its answer is for a version the page has left.
    replace(&mut engine, target, SourceRange::new(0, 0), "x");
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    assert!(!indexed(&engine, &page).contains("[[later]]"));
    assert_eq!(codes(&engine, linker), [DiagnosticCode::UnresolvedAnchor]);

    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert!(indexed(&engine, &page).starts_with("x= Target"));
    assert_eq!(codes(&engine, linker), Vec::<DiagnosticCode>::new());
}

#[test]
fn a_command_while_the_pause_is_indexed_indexes_at_once() {
    let (directory, mut engine, clock, _target, linker) = typed_pages();
    let page = directory.path().join("target.adoc");
    clock.advance(INDEX_SETTLE);
    engine.pump();
    // A command that reads the index does not wait for the worker.
    engine
        .invoke(
            ids::NAVIGATE_GO_TO_SYMBOL,
            CommandArgs::Query {
                query: "later".to_owned(),
                document: None,
            },
        )
        .unwrap();
    assert!(indexed(&engine, &page).contains("[[later]]"));
    assert_eq!(codes(&engine, linker), Vec::<DiagnosticCode>::new());

    let events = engine.subscribe();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    assert!(
        events.try_iter().next().is_none(),
        "the worker's answer, already applied, changes nothing"
    );
    assert!(!engine.has_pending());
}

#[test]
fn closing_the_page_while_the_pause_is_indexed_drops_the_answer() {
    let (directory, mut engine, clock, target, _linker) = typed_pages();
    let page = directory.path().join("target.adoc");
    clock.advance(INDEX_SETTLE);
    engine.pump();
    engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(target))
        .unwrap();
    let after_close = indexed(&engine, &page);
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    assert_eq!(indexed(&engine, &page), after_close);
}
