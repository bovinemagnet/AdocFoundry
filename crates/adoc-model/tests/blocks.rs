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

// --- deleting across blocks -----------------------------------------------------------------

/// The page after deleting between the place after `from` and the place before `to`.
fn delete_between(text: &str, from: &str, to: &str) -> Result<(String, usize), StepError> {
    let start = text.find(from).unwrap() + from.len();
    let end = text.rfind(to).unwrap();
    let model = build(text);
    let result = model.apply(&Step::Delete {
        range: SourceRange::new(start, end),
    })?;
    assert!(result.model.is_none(), "the structure changes");
    assert_eq!(result.selection.start, result.selection.end);
    Ok((apply_edits(text, &result.edits), result.selection.start))
}

#[test]
fn deleting_from_one_paragraph_to_the_next_leaves_one() {
    let text = "First para\n\nSecond para\n\nThird.\n";
    let (after, caret) = delete_between(text, "First ", "para\n\nThird").unwrap();
    assert_eq!(after, "First para\n\nThird.\n");
    assert_eq!(caret, "First ".len());
    let model = build(&after);
    let shown: Vec<_> = model
        .text_blocks()
        .map(|(_, _, text)| text.display.as_str())
        .collect();
    assert_eq!(shown, ["First para", "Third."]);
}

#[test]
fn deleting_across_blocks_removes_those_between() {
    let text = "One two\n\nGone.\n\n----\ncode\n----\n\n* an item\n\n== A section\n\nThree four\n";
    let (after, _) = delete_between(text, "One ", "four").unwrap();
    assert_eq!(after, "One four\n");
}

#[test]
fn a_mark_cut_by_the_selection_stays_closed() {
    let text = "Some *bold words*\n\nand plain text\n";
    let (after, _) = delete_between(text, "*bold", " plain").unwrap();
    assert_eq!(after, "Some *bold* plain text\n");

    let text = "Some plain\n\nthen *bold words* end\n";
    let (after, _) = delete_between(text, "Some ", "words").unwrap();
    assert_eq!(after, "Some *words* end\n");
}

#[test]
fn marks_of_a_kind_meeting_at_the_join_become_one() {
    let text = "A *bold one*\n\nand *bold two* end\n";
    let (after, _) = delete_between(text, "*bold ", "two").unwrap();
    assert_eq!(after, "A *bold two* end\n");
}

#[test]
fn marks_meeting_at_the_join_are_told_apart_by_more_than_their_place_in_the_block() {
    // The bold words are the second mark of their paragraph and the first of the next.
    let text = "x *a* then *bold one*\n\nand *bold two* end\n";
    let (after, _) = delete_between(text, "*bold ", "two").unwrap();
    assert_eq!(after, "x *a* then *bold two* end\n");

    // The first mark of each paragraph, of different kinds, stays two marks.
    let text = "A *bold one*\n\nan _em two_ end\n";
    let (after, _) = delete_between(text, "*bold ", "two").unwrap();
    let model = build(&after);
    let (_, _, block) = model.text_blocks().next().unwrap();
    assert_eq!(block.display, "A bold two end", "{after}");
    let bold = block.marks_at(block.display.find("old").unwrap() + 1);
    let emphasis = block.marks_at(block.display.find("two").unwrap() + 1);
    assert!(bold.contains(adoc_model::Mark::Strong), "{after}");
    assert!(!bold.contains(adoc_model::Mark::Emphasis), "{after}");
    assert!(emphasis.contains(adoc_model::Mark::Emphasis), "{after}");
    assert!(!emphasis.contains(adoc_model::Mark::Strong), "{after}");
}

#[test]
fn an_atom_the_selection_touches_goes_whole() {
    let text = "Before xref:a.adoc[the link] after\n\nnext line\n";
    let (after, _) = delete_between(text, "the ", "line").unwrap();
    assert_eq!(after, "Before line\n");
}

#[test]
fn markup_the_join_would_make_is_escaped() {
    let text = "Price *\n\nstars* now\n";
    let (after, _) = delete_between(text, "Price *", "* now").unwrap();
    let shown: Vec<_> = build(&after)
        .text_blocks()
        .map(|(_, _, text)| text.display.clone())
        .collect();
    assert_eq!(shown, ["Price ** now"], "{after}");
}

#[test]
fn a_join_at_the_start_guards_a_tail_that_starts_a_block() {
    let text = "Whole\n\nmiddle * not a list\n";
    let (after, _) = delete_between(text, "", "* not").unwrap();
    let model = build(&after);
    let (_, node, block) = model.text_blocks().next().unwrap();
    assert_eq!(node.kind, adoc_model::NodeKind::Paragraph, "{after}");
    assert_eq!(block.display, "* not a list");
}

#[test]
fn a_heading_takes_the_text_of_a_paragraph_on_one_line() {
    let text = "== A heading\n\nFirst line\nsecond line\n\nAfter.\n";
    let (after, _) = delete_between(text, "== A ", "line\nsecond").unwrap();
    assert_eq!(after, "== A line second line\n\nAfter.\n");
}

#[test]
fn list_items_join_and_the_items_after_stay() {
    let text = "* one item\n* gone\n* two item\n* three\n";
    let (after, _) = delete_between(text, "one ", "item\n* three").unwrap();
    assert_eq!(after, "* one item\n* three\n");
}

#[test]
fn the_rest_of_a_list_stays_a_list_after_a_paragraph() {
    let text = "Para text\n\n* one item\n* two\n";
    let (after, _) = delete_between(text, "Para ", "item\n* two").unwrap();
    assert_eq!(after, "Para item\n\n* two\n");
    let model = build(&after);
    assert!(matches!(
        model.node(&[1]).unwrap().kind,
        adoc_model::NodeKind::List { .. }
    ));
}

#[test]
fn a_heading_s_following_line_stays_its_own_paragraph() {
    let text = "Para text\n\n== A heading\nRight after.\n";
    let (after, _) = delete_between(text, "Para ", "heading").unwrap();
    assert_eq!(after, "Para heading\n\nRight after.\n");
}

#[test]
fn a_list_continuation_is_refused_when_the_list_would_go() {
    let text = "Para text\n\n* one item\n+\nAttached.\n";
    assert!(matches!(
        delete_between(text, "Para ", "item"),
        Err(StepError::Unsupported(_))
    ));
}

#[test]
fn selecting_whole_blocks_removes_them() {
    let text = "Keep.\n\n[.lead]\nFirst\n\nSecond\n\nKeep too.\n";
    let start = text.find("First").unwrap();
    let end = text.find("Second").unwrap() + "Second".len();
    let model = build(text);
    let result = model
        .apply(&Step::Delete {
            range: SourceRange::new(start, end),
        })
        .unwrap();
    let after = apply_edits(text, &result.edits);
    assert_eq!(after, "Keep.\n\nKeep too.\n");
    assert_eq!(result.selection.start, after.find("Keep too.").unwrap());
}

#[test]
fn the_caret_skips_the_attributes_of_the_block_after_those_removed() {
    let text = "Keep.\n\nFirst\n\nSecond\n\n[.lead]\nKeep too.\n";
    let start = text.find("First").unwrap();
    let end = text.find("Second").unwrap() + "Second".len();
    let result = build(text)
        .apply(&Step::Delete {
            range: SourceRange::new(start, end),
        })
        .unwrap();
    let after = apply_edits(text, &result.edits);
    assert_eq!(after, "Keep.\n\n[.lead]\nKeep too.\n");
    assert_eq!(result.selection.start, after.find("Keep too.").unwrap());
}

#[test]
fn selecting_the_last_blocks_leaves_the_caret_on_the_block_before() {
    let text = "Keep.\n\nFirst\n\nSecond\n";
    let start = text.find("First").unwrap();
    let end = text.find("Second").unwrap() + "Second".len();
    let model = build(text);
    let result = model
        .apply(&Step::Delete {
            range: SourceRange::new(start, end),
        })
        .unwrap();
    assert_eq!(apply_edits(text, &result.edits), "Keep.\n");
    assert_eq!(result.selection.start, "Keep.".len());
}

#[test]
fn deleting_across_refuses_what_only_the_source_can_edit() {
    for between in [
        "// a comment",
        "////\na comment block\n////",
        ":attribute: value",
        "ifdef::x[]\nkept\nendif::[]",
        "include::other.adoc[]",
    ] {
        let text = format!("One two\n\n{between}\n\nThree four\n");
        assert!(delete_between(&text, "One ", "four").is_err(), "{between}");
    }
}

#[test]
fn a_comment_inside_a_section_between_is_refused_but_code_that_looks_like_one_goes() {
    let text = "One two\n\n== Section\n\n// a comment\n\nThree four\n";
    assert!(delete_between(text, "One ", "four").is_err());
    let text = "One two\n\n----\n// in code\n----\n\nThree four\n";
    assert_eq!(
        delete_between(text, "One ", "four").unwrap().0,
        "One four\n"
    );
}

#[test]
fn a_selection_ending_in_read_only_text_is_refused() {
    let path = Path::new("page.adoc");
    let text = "One two\n\nA *bold* word.\n";
    let mut dom = AcdcParser.parse(&ParseInput::new(path, text));
    // A parser that read the paragraph without formatting, so only the source edits it.
    if let adoc_dom::BlockKind::Paragraph { inlines } = &mut dom.blocks[1].kind {
        *inlines = vec![adoc_dom::Inline::Text("A *bold* word.".to_owned())];
    }
    let model = Model::build(path, text, &dom);
    assert!(!model.blocks[1].is_editable());
    assert_eq!(
        model.apply(&Step::Delete {
            range: SourceRange::new("One ".len(), offset(text, "bold")),
        }),
        Err(StepError::NotEditable)
    );
}

#[test]
fn deleting_across_the_edge_of_a_delimited_block_is_refused() {
    for page in [
        "One two\n\n====\nThree four\n====\n",
        "====\nOne two\n====\n\nThree four\n",
        "====\nOne two\n====\n\n====\nThree four\n====\n",
        "One two\n\n****\nThree four\n****\n",
        "One two\n\n____\nThree four\n____\n",
        "One two\n\n--\nThree four\n--\n",
        "One two\n\n[NOTE]\n====\nThree four\n====\n",
    ] {
        assert!(
            matches!(
                delete_between(page, "One ", "four"),
                Err(StepError::Unsupported(_))
            ),
            "{page}"
        );
    }
}

#[test]
fn deleting_inside_a_delimited_block_joins_there() {
    let text = "====\nOne two\n\nThree four\n====\n";
    let (after, _) = delete_between(text, "One ", "four").unwrap();
    assert_eq!(after, "====\nOne four\n====\n");
}

#[test]
fn deleting_into_code_or_a_table_or_from_the_title_is_refused() {
    for (page, from, to) in [
        ("One two\n\n----\ncode here\n----\n", "One ", "here"),
        ("----\ncode here\n----\n\nOne two\n", "code ", "two"),
        ("One two\n\n|===\n|A cell\n|===\n", "One ", "cell"),
        ("|===\n|A cell\n|===\n\nOne two\n", "A ", "two"),
        ("|===\n|A cell |B cell\n|===\n", "A ", "cell\n"),
        ("= The title\n\nOne two\n", "The ", "two"),
        ("Term one:: its text\n\nOne two\n", "its ", "two"),
    ] {
        assert!(delete_between(page, from, to).is_err(), "{page}");
    }
}

// --- a block before ---------------------------------------------------------------------------

fn insert_before(text: &str, at: &str, block: NewBlock) -> Result<(String, usize), StepError> {
    let model = build(text);
    let result = model.apply(&Step::InsertBlockBefore {
        at: text.find(at).unwrap(),
        block,
    })?;
    assert!(result.model.is_none());
    Ok((apply_edits(text, &result.edits), result.selection.end))
}

fn paragraph(text: &str) -> NewBlock {
    NewBlock::Paragraph {
        text: text.to_owned(),
    }
}

#[test]
fn a_paragraph_goes_before_a_paragraph() {
    let text = "One.\n\nTwo.\n";
    let (after, caret) = insert_before(text, "Two", paragraph("n")).unwrap();
    assert_eq!(after, "One.\n\nn\n\nTwo.\n");
    assert_eq!(&after[..caret], "One.\n\nn");
}

#[test]
fn a_paragraph_goes_before_a_block_s_attributes() {
    let text = "One.\n\n[.lead]\nTwo.\n";
    let (after, _) = insert_before(text, "Two", paragraph("n")).unwrap();
    assert_eq!(after, "One.\n\nn\n\n[.lead]\nTwo.\n");
}

#[test]
fn a_paragraph_goes_before_a_heading() {
    let text = "= Title\n\n== Section\n";
    let (after, _) = insert_before(text, "Section", paragraph("n")).unwrap();
    assert_eq!(after, "= Title\n\nn\n\n== Section\n");
}

#[test]
fn a_paragraph_goes_before_an_admonition_paragraph() {
    let text = "NOTE: Careful.\n";
    let (after, _) = insert_before(text, "Careful", paragraph("n")).unwrap();
    assert_eq!(after, "n\n\nNOTE: Careful.\n");
}

#[test]
fn a_paragraph_goes_inside_a_delimited_block() {
    let text = "====\nInside.\n====\n";
    let (after, _) = insert_before(text, "Inside", paragraph("n")).unwrap();
    assert_eq!(after, "====\nn\n\nInside.\n====\n");
}

#[test]
fn a_new_paragraph_before_that_starts_a_block_is_guarded() {
    let text = "One.\n";
    let (after, _) = insert_before(text, "One", paragraph("* x")).unwrap();
    assert_eq!(after, "{empty}* x\n\nOne.\n");
}

#[test]
fn a_list_item_goes_before_a_list_item() {
    let text = "* one\n* two\n";
    let block = NewBlock::ListItem {
        text: "n".to_owned(),
    };
    let (after, caret) = insert_before(text, "two", block).unwrap();
    assert_eq!(after, "* one\n* n\n* two\n");
    assert_eq!(&after[..caret], "* one\n* n");
    assert_eq!(build(&after).text_blocks().count(), 3);
}

#[test]
fn a_new_block_before_is_refused_where_it_cannot_go() {
    assert!(insert_before("= Title\n\nOne.\n", "Title", paragraph("n")).is_err());
    assert!(insert_before("* one\n", "one", paragraph("n")).is_err());
    assert!(insert_before("One.\n", "One", NewBlock::Code).is_err());
    assert!(insert_before("Two words.\n", "words", paragraph("n")).is_err());
    assert!(insert_before("|===\n|A\n|===\n", "A", paragraph("n")).is_err());
    assert!(insert_before("* one\n+\nAttached.\n", "Attached", paragraph("n")).is_err());
}
