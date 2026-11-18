//! Toggle Line Comment (#722): `//` line comments over the lines a selection touches.

use std::time::Duration;

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, CommandArgs, CommandOutcome, DocumentId, Engine, Settings};

fn engine_with(text: &str) -> (tempfile::TempDir, Engine, DocumentId) {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, text).unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    (directory, engine, id)
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

/// Toggles over `start..end` and gives the selection the command returned.
fn toggle(engine: &mut Engine, id: DocumentId, start: usize, end: usize) -> SourceRange {
    let range = SourceRange::new(start, end);
    match engine
        .invoke(
            ids::EDIT_TOGGLE_LINE_COMMENT,
            CommandArgs::Range { id, range },
        )
        .unwrap()
    {
        CommandOutcome::Selection(selection) => selection,
        other => panic!("expected a selection, got {other:?}"),
    }
}

#[test]
fn the_caret_line_is_commented_and_uncommented() {
    let page = "= Title\n\nFirst line.\nSecond line.\n";
    let (_directory, mut engine, id) = engine_with(page);
    let caret = page.find("line.").unwrap();

    let selection = toggle(&mut engine, id, caret, caret);
    assert_eq!(
        text(&engine, id),
        "= Title\n\n// First line.\nSecond line.\n"
    );
    assert_eq!(selection, SourceRange::new(caret + 3, caret + 3));

    let selection = toggle(&mut engine, id, caret + 3, caret + 3);
    assert_eq!(text(&engine, id), page);
    assert_eq!(selection, SourceRange::new(caret, caret));
}

#[test]
fn every_line_the_selection_touches_is_toggled_and_blank_lines_are_left() {
    let page = "one\n\ntwo\nthree\nfour\n";
    let (_directory, mut engine, id) = engine_with(page);
    // From inside "one" to the start of "four": "four" is not touched.
    let start = 1;
    let end = page.find("four").unwrap();

    let selection = toggle(&mut engine, id, start, end);
    let commented = "// one\n\n// two\n// three\nfour\n";
    assert_eq!(text(&engine, id), commented);
    assert_eq!(
        selection,
        SourceRange::new(4, commented.find("four").unwrap())
    );

    // A backwards selection over the same lines takes them back out, and stays backwards.
    let selection = toggle(&mut engine, id, selection.end, selection.start);
    assert_eq!(text(&engine, id), page);
    assert_eq!(selection, SourceRange::new(end, 1));
}

#[test]
fn a_mix_of_comments_and_text_is_commented_and_comes_back() {
    let page = "// already\ntext\n//no space\n";
    let (_directory, mut engine, id) = engine_with(page);

    toggle(&mut engine, id, 0, page.len());
    assert_eq!(text(&engine, id), "// // already\n// text\n// //no space\n");

    let length = text(&engine, id).len();
    toggle(&mut engine, id, 0, length);
    assert_eq!(text(&engine, id), page, "toggling twice restores the text");

    // Only comments: each loses its `//` and one space after it, if there is one.
    let comments = "// a\n//b\n//  c\n";
    let (_directory, mut engine, id) = engine_with(comments);
    toggle(&mut engine, id, 0, comments.len());
    assert_eq!(text(&engine, id), "a\nb\n c\n");
}

#[test]
fn block_delimiters_and_triple_slashes_are_not_line_comments() {
    // Asciidoctor reads `//` as a line comment only when no third `/` follows: `////` opens a
    // comment block and `///x` is text, so neither is uncommented.
    let page = "////\n///x\n";
    let (_directory, mut engine, id) = engine_with(page);
    toggle(&mut engine, id, 0, page.len());
    assert_eq!(text(&engine, id), "// ////\n// ///x\n");
}

#[test]
fn line_endings_are_kept_and_one_undo_takes_it_all_back() {
    let page = "one\r\ntwo\r\nthree";
    let (_directory, mut engine, id) = engine_with(page);
    toggle(&mut engine, id, 0, page.len());
    assert_eq!(text(&engine, id), "// one\r\n// two\r\n// three");

    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), page);
}

#[test]
fn only_blank_lines_change_nothing() {
    let page = "text\n\n  \nmore\n";
    let (_directory, mut engine, id) = engine_with(page);
    let blank = page.find("\n\n").unwrap() + 1;
    let selection = toggle(&mut engine, id, blank, blank + 3);
    assert_eq!(text(&engine, id), page);
    assert_eq!(selection, SourceRange::new(blank, blank + 3));
    assert!(engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .is_ok());
    assert_eq!(text(&engine, id), page, "nothing was added to undo");
}

#[test]
fn commented_lines_are_left_out_of_the_page() {
    let page = "= Title\n\nKept.\n\nHidden paragraph.\n";
    let (_directory, mut engine, id) = engine_with(page);
    let hidden = page.find("Hidden").unwrap();
    toggle(&mut engine, id, hidden, hidden);
    let rendered = engine.render_blocking(id, Duration::from_secs(10)).unwrap();
    let html = adoc_render::to_html(&rendered.view, &adoc_render::HtmlOptions::default());
    assert!(html.contains("Kept."), "{html}");
    assert!(!html.contains("Hidden"), "{html}");
}

#[test]
fn the_command_has_its_shortcut() {
    let engine = Engine::new(Settings::default().with_git_opt_in());
    let spec = engine
        .commands()
        .all()
        .find(|spec| spec.id == ids::EDIT_TOGGLE_LINE_COMMENT)
        .expect("the command is registered");
    assert_eq!(spec.title, "Toggle Line Comment");
    assert_eq!(spec.default_binding, Some("cmd-/"));
}
