//! Finding the text block at a source offset.

use std::path::Path;

use adoc_core::probe::{self, Probe};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_model::{BlockPath, Model};

fn build(text: &str) -> Model {
    let path = Path::new("page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    Model::build(path, text, &dom)
}

/// The innermost text block holding `offset`, found by visiting every one.
fn by_walking(model: &Model, offset: usize) -> Option<BlockPath> {
    model
        .text_blocks()
        .filter(|(_, _, text)| text.range.start <= offset && offset <= text.range.end)
        .last()
        .map(|(path, ..)| path)
}

const PAGE: &str = "= Title\n\nA *bold* paragraph.\n\n== Section\n\n* One\n** Nested\n* Two\n\n\
[NOTE]\n====\nInside a note.\n====\n\n|===\n|a |b\n|c |d\n|===\n\nTerm:: Its description.\n\n\
----\ncode\n----\n\nLast.\n";

#[test]
fn every_offset_finds_the_block_a_walk_finds() {
    let model = build(PAGE);
    for offset in 0..=PAGE.len() {
        assert_eq!(
            model.text_block_at(offset).map(|(path, ..)| path),
            by_walking(&model, offset),
            "at {offset}: {:?}",
            &PAGE[offset.saturating_sub(5)..(offset + 5).min(PAGE.len())]
        );
    }
}

#[test]
fn finding_a_block_does_not_walk_the_model_each_time() {
    let text = "Paragraph.\n\n".repeat(300);
    let model = build(&text);
    probe::reset();
    for offset in (0..text.len()).step_by(7) {
        assert!(model.text_block_at(offset).is_some() || text.as_bytes()[offset] == b'\n');
    }
    assert!(
        probe::count(Probe::Walk) <= 1,
        "walked {} times",
        probe::count(Probe::Walk)
    );
}

#[test]
fn an_edited_model_finds_blocks_where_they_moved() {
    let text = "First.\n\nSecond.\n";
    let model = build(text);
    assert!(model.text_block_at(text.find("Second").unwrap()).is_some());
    let step = adoc_model::Step::InsertText {
        at: 0,
        text: "Longer ".to_owned(),
        marks: adoc_model::MarkSet::EMPTY,
    };
    let edited = model.apply(&step).unwrap().model.unwrap();
    let after = adoc_model::apply_edits(text, &model.apply(&step).unwrap().edits);
    let second = after.find("Second").unwrap();
    assert_eq!(
        edited.text_block_at(second).map(|(path, ..)| path),
        Some(vec![1])
    );
}
