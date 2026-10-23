//! Steps: the source edits they make, their inverses, and the model they leave.

use std::path::Path;

use adoc_core::SourceRange;
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_model::{apply_edits, invert, Mark, MarkSet, Model, Step, StepError, StepResult};

fn build(text: &str) -> Model {
    let path = Path::new("page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    Model::build(path, text, &dom)
}

/// Applies `step`, checks that its inverse restores the text and that the model it leaves is
/// the one a rebuild gives, and returns the new text and the result.
fn apply(model: &Model, step: Step) -> (String, StepResult) {
    let result = model
        .apply(&step)
        .unwrap_or_else(|error| panic!("{step:?}: {error}"));
    let after = apply_edits(&model.source, &result.edits);
    assert_eq!(
        apply_edits(&after, &invert(&model.source, &result.edits)),
        model.source,
        "the inverse restores the text"
    );
    if let Some(local) = &result.model {
        assert_eq!(local.source, after);
        let rebuilt = build(&after);
        assert_eq!(
            local.blocks, rebuilt.blocks,
            "the local model is the rebuilt one: {after:?}"
        );
    }
    (after, result)
}

fn offset(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"))
}

fn strong() -> MarkSet {
    MarkSet::EMPTY.with(Mark::Strong)
}

#[test]
fn typing_inserts_plain_text_where_the_caret_is() {
    let model = build("A paragraph.\n\nAnother one.\n");
    let at = offset(&model.source, "graph");
    let (after, result) = apply(
        &model,
        Step::InsertText {
            at,
            text: "ra".to_owned(),
            marks: MarkSet::EMPTY,
        },
    );
    assert_eq!(after, "A pararagraph.\n\nAnother one.\n");
    assert_eq!(result.selection, SourceRange::new(at + 2, at + 2));
    assert_eq!(result.edits.len(), 1);
    assert_eq!(result.edits[0].range, SourceRange::new(at, at));
}

#[test]
fn typing_follows_the_marks_the_caret_carries() {
    let model = build("Some *bold* text.\n");
    let inside = offset(&model.source, "* text");
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at: inside,
            text: "er".to_owned(),
            marks: strong(),
        },
    );
    assert_eq!(after, "Some *bolder* text.\n");
    let outside = inside + 1;
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at: outside,
            text: "!".to_owned(),
            marks: MarkSet::EMPTY,
        },
    );
    assert_eq!(after, "Some *bold*! text.\n");
    // Typing in the middle of bold text with bold turned off splits the mark.
    let middle = offset(&model.source, "ld*");
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at: middle,
            text: "x".to_owned(),
            marks: MarkSet::EMPTY,
        },
    );
    assert_eq!(after, "Some **bo**x**ld** text.\n");
    // Typing with bold turned on outside any mark adds one.
    let plain = offset(&model.source, " text");
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at: plain,
            text: " new".to_owned(),
            marks: strong(),
        },
    );
    assert_eq!(after, "Some *bold* *new* text.\n");
}

#[test]
fn typed_markup_is_escaped() {
    let cases = [
        ("Start *here.\n", "here.", "*", "Start \\*here*.\n"),
        (
            "About this.\n",
            "this",
            "{product} ",
            "About \\{product} this.\n",
        ),
        (
            "Link here.\n",
            "here",
            "https://example.com ",
            "Link \\https://example.com here.\n",
        ),
        ("See it.\n", "it", "<<id>> ", "See \\<<id>> it.\n"),
        ("Two by three.\n", "by", "* ", "Two * by three.\n"),
        (
            "A snake.\n",
            "snake",
            "snake_case_",
            "A snake_case_snake.\n",
        ),
    ];
    for (text, before, typed, expected) in cases {
        let model = build(&format!(":product: Foundry\n\n{text}"));
        let at = if typed == "*" {
            offset(&model.source, before) + before.len() - 1
        } else {
            offset(&model.source, before)
        };
        let (after, _) = apply(
            &model,
            Step::InsertText {
                at,
                text: typed.to_owned(),
                marks: MarkSet::EMPTY,
            },
        );
        assert_eq!(
            after,
            format!(":product: Foundry\n\n{expected}"),
            "typing {typed:?}"
        );
    }
}

#[test]
fn typing_at_a_line_start_cannot_start_another_block() {
    let model = build("Introduction text.\n");
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at: 0,
            text: "* ".to_owned(),
            marks: MarkSet::EMPTY,
        },
    );
    assert_eq!(after, "{empty}* Introduction text.\n");
    let built = build(&after);
    assert_eq!(
        built.blocks[0].text.as_ref().unwrap().display,
        "* Introduction text."
    );
}

#[test]
fn deleting_removes_text_atoms_and_emptied_marks() {
    let model = build("One *two* three (C) four.\n");
    let start = offset(&model.source, "*two*");
    let (after, result) = apply(
        &model,
        Step::Delete {
            range: SourceRange::new(start, start + "*two* ".len()),
        },
    );
    assert_eq!(after, "One three (C) four.\n");
    assert_eq!(result.selection.start, start);
    // Deleting part of the replacement takes all of it.
    let at = offset(&model.source, "(C)");
    let (after, _) = apply(
        &model,
        Step::Delete {
            range: SourceRange::new(at + 1, at + 2),
        },
    );
    assert_eq!(after, "One *two* three  four.\n");
}

#[test]
fn toggle_mark_keeps_delimiter_form() {
    let model = build("A **bold** word and more.\n");
    let more = offset(&model.source, "more");
    let (after, result) = apply(
        &model,
        Step::ToggleMark {
            range: SourceRange::new(more, more + 4),
            mark: Mark::Strong,
        },
    );
    assert_eq!(after, "A **bold** word and *more*.\n");
    assert_eq!(&after[result.selection.start..result.selection.end], "more");

    let bold = offset(&model.source, "ld**");
    let (after, _) = apply(
        &model,
        Step::ToggleMark {
            range: SourceRange::new(bold, bold + 2),
            mark: Mark::Strong,
        },
    );
    assert_eq!(after, "A **bo**ld word and more.\n");

    let constrained = build("A *bold* word.\n");
    let at = offset(&constrained.source, "ld*");
    let (after, _) = apply(
        &constrained,
        Step::ToggleMark {
            range: SourceRange::new(at, at + 2),
            mark: Mark::Strong,
        },
    );
    assert_eq!(
        after, "A **bo**ld word.\n",
        "a split constrained mark becomes unconstrained"
    );

    let inside_word = build("Unconstrained text.\n");
    let at = offset(&inside_word.source, "con");
    let (after, _) = apply(
        &inside_word,
        Step::ToggleMark {
            range: SourceRange::new(at, at + 3),
            mark: Mark::Emphasis,
        },
    );
    assert_eq!(after, "Un__con__strained text.\n");
}

#[test]
fn toggle_mark_spans_blocks_and_skips_whitespace() {
    let model = build("First para.\n\nSecond para.\n");
    let start = offset(&model.source, "para.");
    let end = offset(&model.source, "Second") + "Second".len();
    let (after, result) = apply(
        &model,
        Step::ToggleMark {
            range: SourceRange::new(start, end),
            mark: Mark::Emphasis,
        },
    );
    assert_eq!(after, "First _para._\n\n_Second_ para.\n");
    assert_eq!(
        &after[result.selection.start..result.selection.end],
        "para._\n\n_Second"
    );
}

#[test]
fn nested_marks_survive_edits_around_them() {
    let model = build("Text *with _nested_ marks* here.\n");
    let at = offset(&model.source, "nested");
    let marks = MarkSet::EMPTY.with(Mark::Strong).with(Mark::Emphasis);
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at: at + 3,
            text: "X".to_owned(),
            marks,
        },
    );
    assert_eq!(after, "Text *with _nesXted_ marks* here.\n");
}

#[test]
fn code_text_takes_anything_but_its_delimiter() {
    let model = build("----\nfn main() {}\n----\n");
    let at = offset(&model.source, "{}") + 1;
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at,
            text: "\n    *x*;\n".to_owned(),
            marks: MarkSet::EMPTY,
        },
    );
    assert_eq!(after, "----\nfn main() {\n    *x*;\n}\n----\n");
    let refused = model.apply(&Step::InsertText {
        at,
        text: "\n----\n".to_owned(),
        marks: MarkSet::EMPTY,
    });
    assert_eq!(refused, Err(StepError::WouldChangeMeaning));
}

#[test]
fn cells_escape_bars_and_titles_stay_on_one_line() {
    let model = build("|===\n|a |b\n|===\n");
    let at = offset(&model.source, "a ") + 1;
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at,
            text: " | c".to_owned(),
            marks: MarkSet::EMPTY,
        },
    );
    assert_eq!(after, "|===\n|a \\| c |b\n|===\n");

    let model = build("== A title\n\nText.\n");
    let at = offset(&model.source, "title") + 5;
    let (after, _) = apply(
        &model,
        Step::InsertText {
            at,
            text: "\nmore".to_owned(),
            marks: MarkSet::EMPTY,
        },
    );
    assert_eq!(after, "== A title more\n\nText.\n");
}

#[test]
fn read_only_text_refuses_steps() {
    let model = build("[stem]\n++++\nx^2\n++++\n\ninclude::missing.adoc[]\n");
    for at in [
        offset(&model.source, "x^2"),
        offset(&model.source, "missing"),
    ] {
        let result = model.apply(&Step::InsertText {
            at,
            text: "y".to_owned(),
            marks: MarkSet::EMPTY,
        });
        assert_eq!(result, Err(StepError::NotEditable));
    }
}

#[test]
fn steps_invert_exactly() {
    let text = "= Title\n\nA *bold* and _it_ paragraph with xref:a.adoc[a link] (C).\n\n* item *one*\n* item two\n\n|===\n|cell |other\n|===\n";
    let model = build(text);
    let mut steps = Vec::new();
    for (index, _) in text.char_indices().step_by(3) {
        steps.push(Step::InsertText {
            at: index,
            text: "w*".to_owned(),
            marks: MarkSet::EMPTY,
        });
        steps.push(Step::Delete {
            range: SourceRange::new(index, (index + 4).min(text.len())),
        });
        steps.push(Step::ToggleMark {
            range: SourceRange::new(index, (index + 6).min(text.len())),
            mark: Mark::Strong,
        });
    }
    let mut applied = 0;
    for step in steps {
        let Ok(result) = model.apply(&step) else {
            continue;
        };
        let after = apply_edits(&model.source, &result.edits);
        assert_eq!(
            apply_edits(&after, &invert(&model.source, &result.edits)),
            text,
            "{step:?}"
        );
        applied += 1;
    }
    assert!(applied > 40, "only {applied} steps applied");
}
