//! Structural steps: splitting and joining blocks, block types, lists.

use std::path::Path;

use adoc_core::SourceRange;
use adoc_dom::{AcdcParser, AdmonitionKind, DocumentParser, ListKind, ParseInput};
use adoc_model::{apply_edits, invert, BlockType, Model, NewBlock, Step, StepError};

fn build(text: &str) -> Model {
    let path = Path::new("page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    Model::build(path, text, &dom)
}

fn offset(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"))
}

/// Applies `step` to `text`, checks its inverse, and returns the new text and the caret.
fn apply(text: &str, step: Step) -> (String, usize) {
    let model = build(text);
    let result = model
        .apply(&step)
        .unwrap_or_else(|error| panic!("{step:?}: {error}"));
    let after = apply_edits(text, &result.edits);
    assert_eq!(apply_edits(&after, &invert(text, &result.edits)), text);
    (after, result.selection.end)
}

/// What the rebuilt model shows: each text block.
fn shown(text: &str) -> Vec<String> {
    build(text)
        .text_blocks()
        .map(|(_, _, block)| block.display.clone())
        .collect()
}

#[test]
fn enter_splits_paragraphs_headings_and_items() {
    let text = "Some *bold words* here.\n";
    let (after, caret) = apply(
        text,
        Step::SplitBlock {
            at: offset(text, "words"),
        },
    );
    assert_eq!(after, "Some *bold*\n\n*words* here.\n");
    assert_eq!(&after[caret..caret + 1], "*");
    assert_eq!(shown(&after), ["Some bold", "words here."]);

    let text = "== A heading title\n\nText.\n";
    let (after, _) = apply(
        text,
        Step::SplitBlock {
            at: offset(text, "title"),
        },
    );
    assert_eq!(after, "== A heading\n\ntitle\n\nText.\n");

    let text = "* first item\n** nested\n* last\n";
    let (after, caret) = apply(
        text,
        Step::SplitBlock {
            at: offset(text, "item"),
        },
    );
    assert_eq!(after, "* first\n* item\n** nested\n* last\n");
    assert_eq!(&after[caret..caret + 4], "item");

    let text = "* [x] done task\n";
    let (after, _) = apply(
        text,
        Step::SplitBlock {
            at: offset(text, "task"),
        },
    );
    assert_eq!(after, "* [x] done\n* [ ] task\n");
}

#[test]
fn a_split_that_would_start_another_block_is_guarded() {
    let text = "Before * after.\n";
    let (after, _) = apply(
        text,
        Step::SplitBlock {
            at: offset(text, "* after"),
        },
    );
    assert_eq!(after, "Before\n\n{empty}* after.\n");
    assert_eq!(shown(&after), ["Before", "* after."]);
}

#[test]
fn new_blocks_go_after_the_block_the_caret_ends() {
    let text = "= Title\n:attr: value\n\nFirst.\n\n* item\n\n== Section\n";
    let after_paragraph = offset(text, "First.") + "First.".len();
    let (after, caret) = apply(
        text,
        Step::InsertBlock {
            at: after_paragraph,
            block: NewBlock::Paragraph {
                text: "Second.".to_owned(),
            },
        },
    );
    assert_eq!(
        after,
        "= Title\n:attr: value\n\nFirst.\n\nSecond.\n\n* item\n\n== Section\n"
    );
    assert_eq!(&after[caret - 7..caret], "Second.");

    let after_item = offset(text, "item") + 4;
    let (after, _) = apply(
        text,
        Step::InsertBlock {
            at: after_item,
            block: NewBlock::ListItem {
                text: "x".to_owned(),
            },
        },
    );
    assert_eq!(
        after,
        "= Title\n:attr: value\n\nFirst.\n\n* item\n* x\n\n== Section\n"
    );

    // After the document title, the new paragraph goes after the header.
    let (after, _) = apply(
        text,
        Step::InsertBlock {
            at: offset(text, "Title") + 5,
            block: NewBlock::Paragraph {
                text: "Lead.".to_owned(),
            },
        },
    );
    assert_eq!(
        after,
        "= Title\n:attr: value\n\nLead.\n\nFirst.\n\n* item\n\n== Section\n"
    );

    let after_heading = offset(text, "Section") + 7;
    let (after, _) = apply(
        text,
        Step::InsertBlock {
            at: after_heading,
            block: NewBlock::Paragraph {
                text: "Body.".to_owned(),
            },
        },
    );
    assert_eq!(
        after,
        "= Title\n:attr: value\n\nFirst.\n\n* item\n\n== Section\n\nBody.\n"
    );
}

#[test]
fn backspace_at_a_block_start_joins_or_lifts() {
    let text = "One *bold*\n\n*two* more.\n";
    let (after, caret) = apply(
        text,
        Step::JoinBackward {
            at: offset(text, "*two") + 1,
        },
    );
    // Only the blank line goes: the second line is left as it was.
    assert_eq!(after, "One *bold*\n*two* more.\n");
    assert_eq!(shown(&after), ["One bold\ntwo more."]);
    assert_eq!(&after[caret..caret + 4], "*two");

    let text = "== Heading\n\nText.\n";
    let (after, _) = apply(
        text,
        Step::JoinBackward {
            at: offset(text, "Heading"),
        },
    );
    assert_eq!(after, "Heading\n\nText.\n");
    assert!(
        build(text)
            .apply(&Step::JoinBackward {
                at: offset(text, "Text")
            })
            .is_err(),
        "a paragraph does not join a heading"
    );

    let text = "NOTE: Careful now.\n";
    let (after, _) = apply(
        text,
        Step::JoinBackward {
            at: offset(text, "Careful"),
        },
    );
    assert_eq!(after, "Careful now.\n");

    let text = "* one\n* two\n* three\n";
    let (after, _) = apply(
        text,
        Step::JoinBackward {
            at: offset(text, "two"),
        },
    );
    assert_eq!(after, "* one\ntwo\n* three\n");
    assert_eq!(shown(&after), ["one\ntwo", "three"]);
    let (after, _) = apply(
        text,
        Step::JoinBackward {
            at: offset(text, "one"),
        },
    );
    assert_eq!(after, "one\n\n* two\n* three\n");
    let nested = "* one\n** two\n";
    let (after, _) = apply(
        nested,
        Step::JoinBackward {
            at: offset(nested, "two"),
        },
    );
    assert_eq!(after, "* one\n* two\n", "a nested item is outdented first");
}

#[test]
fn block_types_change_the_prefix_only() {
    let text = "[[intro]]\nA paragraph\nover lines.\n\nNext.\n";
    let at = offset(text, "paragraph");
    let (after, _) = apply(
        text,
        Step::SetBlockType {
            at,
            kind: BlockType::Heading(2),
        },
    );
    assert_eq!(after, "[[intro]]\n=== A paragraph over lines.\n\nNext.\n");
    let (back, _) = apply(
        &after,
        Step::SetBlockType {
            at: offset(&after, "paragraph"),
            kind: BlockType::Paragraph,
        },
    );
    assert_eq!(back, "[[intro]]\nA paragraph over lines.\n\nNext.\n");
    let (level, _) = apply(
        &after,
        Step::SetBlockType {
            at: offset(&after, "paragraph"),
            kind: BlockType::Heading(1),
        },
    );
    assert_eq!(level, "[[intro]]\n== A paragraph over lines.\n\nNext.\n");

    let text = "Mind the gap.\n";
    let (after, _) = apply(
        text,
        Step::SetBlockType {
            at: 0,
            kind: BlockType::Admonition(AdmonitionKind::Warning),
        },
    );
    assert_eq!(after, "WARNING: Mind the gap.\n");
    let (after, _) = apply(
        &after,
        Step::SetBlockType {
            at: offset(&after, "Mind"),
            kind: BlockType::Admonition(AdmonitionKind::Tip),
        },
    );
    assert_eq!(after, "TIP: Mind the gap.\n");

    let text = "Run *this* now.\n";
    let (after, _) = apply(
        text,
        Step::SetBlockType {
            at: 0,
            kind: BlockType::Code,
        },
    );
    assert_eq!(after, "----\nRun this now.\n----\n");
    let (after, _) = apply(
        &after,
        Step::SetBlockType {
            at: offset(&after, "Run"),
            kind: BlockType::Paragraph,
        },
    );
    assert_eq!(after, "Run this now.\n");

    let text = "Wise words.\n";
    let (after, _) = apply(
        text,
        Step::SetBlockType {
            at: 0,
            kind: BlockType::Quote,
        },
    );
    assert_eq!(after, "____\nWise words.\n____\n");
}

#[test]
fn lists_toggle_indent_and_check() {
    let text = "Buy milk.\n";
    let (after, _) = apply(
        text,
        Step::ToggleList {
            at: 0,
            kind: ListKind::Unordered,
        },
    );
    assert_eq!(after, "* Buy milk.\n");
    let (after, _) = apply(
        &after,
        Step::ToggleList {
            at: offset(&after, "Buy"),
            kind: ListKind::Ordered,
        },
    );
    assert_eq!(after, ". Buy milk.\n");
    let (after, _) = apply(
        &after,
        Step::ToggleList {
            at: offset(&after, "Buy"),
            kind: ListKind::Checklist,
        },
    );
    assert_eq!(after, "* [ ] Buy milk.\n");
    let (after, _) = apply(
        &after,
        Step::ToggleChecked {
            at: offset(&after, "Buy"),
        },
    );
    assert_eq!(after, "* [x] Buy milk.\n");
    let (after, _) = apply(
        &after,
        Step::ToggleList {
            at: offset(&after, "Buy"),
            kind: ListKind::Checklist,
        },
    );
    assert_eq!(after, "Buy milk.\n");

    let text = "* one\n* two\n** under two\n* three\n";
    let (after, _) = apply(
        text,
        Step::IndentListItem {
            at: offset(text, "two"),
        },
    );
    assert_eq!(after, "* one\n** two\n*** under two\n* three\n");
    let (back, _) = apply(
        &after,
        Step::OutdentListItem {
            at: offset(&after, "two"),
        },
    );
    assert_eq!(back, text);
    assert_eq!(
        build(text).apply(&Step::IndentListItem {
            at: offset(text, "one")
        }),
        Err(StepError::Unsupported(
            "the first item of a list cannot be indented"
        )),
    );
    let (lifted, _) = apply(
        text,
        Step::ToggleList {
            at: offset(text, "three"),
            kind: ListKind::Unordered,
        },
    );
    assert_eq!(lifted, "* one\n* two\n** under two\n\nthree\n");
    let middle = "* a\n* b\n* c\n";
    let (lifted, _) = apply(
        middle,
        Step::OutdentListItem {
            at: offset(middle, "b"),
        },
    );
    assert_eq!(lifted, "* a\n\nb\n\n* c\n");
    assert_eq!(shown(&lifted), ["a", "b", "c"]);
}

#[test]
fn structural_steps_rebuild_the_model() {
    let text = "Some text.\n";
    let model = build(text);
    let result = model.apply(&Step::SplitBlock { at: 4 }).unwrap();
    assert!(result.model.is_none(), "structure comes from a fresh parse");
    let range = SourceRange::new(0, 0);
    let _ = range;
}

#[test]
fn code_blocks_and_admonitions_are_inserted_ready_to_type() {
    let text = "Intro.\n\nMore.\n";
    let model = build(text);
    let result = model
        .apply(&Step::InsertBlock {
            at: 3,
            block: NewBlock::Code,
        })
        .unwrap();
    let after = apply_edits(text, &result.edits);
    assert_eq!(after, "Intro.\n\n----\n\n----\n\nMore.\n");
    let typed = build(&after)
        .apply(&Step::InsertText {
            at: result.selection.start,
            text: "x = 1".to_owned(),
            marks: adoc_model::MarkSet::EMPTY,
        })
        .unwrap();
    assert_eq!(
        apply_edits(&after, &typed.edits),
        "Intro.\n\n----\nx = 1\n----\n\nMore.\n"
    );

    let result = model
        .apply(&Step::InsertBlock {
            at: 3,
            block: NewBlock::Admonition(AdmonitionKind::Tip),
        })
        .unwrap();
    let after = apply_edits(text, &result.edits);
    assert_eq!(after, "Intro.\n\nTIP: Tip.\n\nMore.\n");
    assert_eq!(&after[result.selection.start..result.selection.end], "Tip.");
}
