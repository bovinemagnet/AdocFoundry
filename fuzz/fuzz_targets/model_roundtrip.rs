//! The visual editor's model under arbitrary pages and steps, held to the write-back rules
//! `crates/adoc-model/tests/roundtrip.rs` encodes:
//!
//! - a model built from any page serialises to that page, and each text block to its slice;
//! - a step's edits are sorted, apart, on character boundaries of the page, and its selection
//!   lies in the text after it;
//! - the edits of a step on a block, made at its caret stops, stay inside that block (the
//!   ranges `edits_stay_inside_the_touched_block` uses, for the steps it makes);
//! - the model a step returns describes the text after it, and so does one rebuilt from it;
//! - the inverse of a step's edits gives the page back, as undo relies on.
//!
//! The input is the page, then optionally a NUL byte and the bytes the steps are chosen from.
//! Without them the steps come from a generator seeded by the page, so a corpus of plain pages
//! is edited too. Steps are also made at arbitrary offsets, which must be refused or kept to
//! the rules, never panic.

#![no_main]

use std::{path::Path, sync::Arc};

use adoc_core::SourceRange;
use adoc_dom::{AcdcParser, AdmonitionKind, DocumentParser, ListKind, NoIncludes, ParseInput};
use adoc_model::{
    apply_edits, invert, BlockType, Mark, MarkSet, Model, NewBlock, NodeKind, SourceEdit, Step,
};
use libfuzzer_sys::{
    arbitrary::{Result, Unstructured},
    fuzz_target,
};

/// At most this many steps per input, so one input stays quick.
const STEPS: usize = 8;

/// Typed text, as `roundtrip.rs` types it: markup, and line breaks before what would start
/// another block.
const WORDS: &[&str] = &[
    "word",
    " more",
    "it's",
    " -- ",
    "a*b",
    "snake_case",
    "(C)",
    "x",
    "#1",
    " {",
    "<",
    "...",
    "two  spaces",
    "é",
    "日本",
    "end.",
    " *",
    "_",
    "`",
    "^",
    "~",
    "+",
    "\n[note]",
    "\n----",
    "\n|===",
    "\n,===",
    "\n* item",
    "\n+",
    "\n.Title",
    "\nNOTE: x",
    "\n== H",
    "\n\n",
    "|",
    "[[id]]",
    "xref:a.adoc[]",
    "{attr}",
    "+++",
    "pass:[x]",
    "stem:[x]",
    "link:x[y]",
    "<<a>>",
];

const ADMONITIONS: &[AdmonitionKind] = &[
    AdmonitionKind::Note,
    AdmonitionKind::Tip,
    AdmonitionKind::Important,
    AdmonitionKind::Caution,
    AdmonitionKind::Warning,
];

const LISTS: &[ListKind] = &[ListKind::Unordered, ListKind::Ordered, ListKind::Checklist];

fuzz_target!(|data: &[u8]| {
    let (page, script) = match data.iter().position(|&byte| byte == 0) {
        Some(at) => (&data[..at], data[at + 1..].to_vec()),
        None => (data, generated(data)),
    };
    let text = String::from_utf8_lossy(page).into_owned();
    let mut model = build(&text);
    check_model(&model);
    let mut choices = Unstructured::new(&script);
    for _ in 0..STEPS {
        let Ok(choice) = choose(&model, &mut choices) else {
            break;
        };
        let Some((step, block)) = choice else {
            continue;
        };
        model = check_step(&model, &step, block);
    }
});

fn build(text: &str) -> Model {
    let path = Path::new("fuzz.adoc");
    let dom = AcdcParser.parse(&ParseInput {
        path,
        text,
        attributes: Vec::new(),
        includes: Arc::new(NoIncludes),
    });
    Model::build(path, text, &dom)
}

/// Bytes to choose steps from for a page given without them.
fn generated(page: &[u8]) -> Vec<u8> {
    // FNV-1a, then the generator `roundtrip.rs` uses.
    let mut state = page.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (0..256)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        })
        .collect()
}

fn is_range_of(text: &str, range: SourceRange) -> bool {
    range.start <= range.end
        && range.end <= text.len()
        && text.is_char_boundary(range.start)
        && text.is_char_boundary(range.end)
}

fn check_model(model: &Model) {
    assert!(
        model.serialise() == model.source,
        "the model does not serialise to its page"
    );
    for (_, _, block) in model.text_blocks() {
        assert!(is_range_of(&model.source, block.range), "{:?}", block.range);
        assert!(
            block.serialise(&model.source) == model.source[block.range.start..block.range.end],
            "the text block at {:?} does not serialise to its slice",
            block.range
        );
    }
}

/// Applies `step` to `model`, checks the result, and returns the model after it.
fn check_step(model: &Model, step: &Step, block: Option<SourceRange>) -> Model {
    let Ok(result) = model.apply(step) else {
        return model.clone();
    };
    let source = &model.source;
    let mut previous = 0;
    for edit in &result.edits {
        assert!(
            is_range_of(source, edit.range) && previous <= edit.range.start,
            "{step:?} gave edits out of order or off the page: {:?}",
            result.edits
        );
        previous = edit.range.end;
        if let Some(block) = block {
            assert!(
                block.start <= edit.range.start && edit.range.end <= block.end,
                "{step:?} edited {:?} outside {block:?}",
                edit.range
            );
        }
    }
    let after = apply_edits(source, &result.edits);
    assert!(
        is_range_of(&after, result.selection),
        "{step:?} selected {:?} outside the {} bytes after it",
        result.selection,
        after.len()
    );
    assert!(
        apply_edits(&after, &invert(source, &result.edits)) == *source,
        "undoing {step:?} does not give the page back"
    );
    if let Some(local) = &result.model {
        assert!(
            local.source == after,
            "{step:?}: the model is not of the text after it"
        );
        check_model(local);
    }
    let rebuilt = build(&after);
    check_model(&rebuilt);
    rebuilt
}

/// A step and, when the rules bound its edits, the range they must stay in.
type Choice = Option<(Step, Option<SourceRange>)>;

fn choose(model: &Model, u: &mut Unstructured<'_>) -> Result<Choice> {
    // One step in eight is made at an arbitrary offset instead of a caret stop.
    if u.ratio(1, 8)? {
        return arbitrary_step(model, u).map(|step| Some((step, None)));
    }
    let blocks: Vec<_> = model
        .text_blocks()
        .filter(|(_, node, _)| node.is_editable())
        .collect();
    if blocks.is_empty() {
        return arbitrary_step(model, u).map(|step| Some((step, None)));
    }
    let (path, node, text) = u.choose(&blocks)?;
    let (node, text) = (*node, *text);
    // A caret stop: a character boundary of what the block shows, and the marks there.
    let stops: Vec<usize> = text
        .display
        .char_indices()
        .map(|(index, _)| index)
        .chain([text.display.len()])
        .collect();
    let stop = |u: &mut Unstructured<'_>| -> Result<Option<(usize, usize)>> {
        let display = *u.choose(&stops)?;
        Ok(text
            .source_offset(display, text.marks_at(display))
            .map(|source| (display, source)))
    };
    let range = |u: &mut Unstructured<'_>| -> Result<Option<SourceRange>> {
        let (Some((_, a)), Some((_, b))) = (stop(u)?, stop(u)?) else {
            return Ok(None);
        };
        Ok(Some(SourceRange::new(a.min(b), a.max(b))))
    };
    // The lines of the node, from the start of the line it starts on, and (joining) of the
    // text block before it: what a structural step may edit.
    let line_start = |offset: usize| {
        model.source[..offset]
            .rfind('\n')
            .map_or(0, |index| index + 1)
            .min(offset)
    };
    let node_lines = SourceRange::new(line_start(node.range.start), node.range.end);
    let list = path
        .split_last()
        .and_then(|(_, parent)| model.node(parent))
        .filter(|parent| matches!(parent.kind, NodeKind::List { .. }))
        .map_or(node.range, |list| list.range);
    let at = text.range.start;

    Ok(Some(match u.int_in_range(0..=17)? {
        0..=3 => {
            let Some((display, at)) = stop(u)? else {
                return Ok(None);
            };
            let typed = if u.ratio(3, 4)? {
                (*u.choose(WORDS)?).to_owned()
            } else {
                short_string(u)?
            };
            let marks = if u.ratio(7, 8)? {
                text.marks_at(display)
            } else {
                Mark::ALL
                    .into_iter()
                    .filter(|_| u.ratio(1, 4).unwrap_or(false))
                    .collect()
            };
            (
                Step::InsertText {
                    at,
                    text: typed,
                    marks,
                },
                Some(text.range),
            )
        }
        4 | 5 => {
            let Some(range) = range(u)? else {
                return Ok(None);
            };
            (Step::Delete { range }, Some(text.range))
        }
        6 | 7 => {
            let Some(range) = range(u)? else {
                return Ok(None);
            };
            let mark = *u.choose(&Mark::ALL)?;
            (Step::ToggleMark { range, mark }, Some(text.range))
        }
        8 => {
            let Some(range) = range(u)? else {
                return Ok(None);
            };
            let target =
                (*u.choose(&["https://example.org", "page.adoc", "id", "a b", "]"])?).to_owned();
            let text = if u.arbitrary()? {
                Some(short_string(u)?)
            } else {
                None
            };
            (
                Step::SetLink {
                    range,
                    target,
                    text,
                },
                None,
            )
        }
        9 => {
            let Some((_, at)) = stop(u)? else {
                return Ok(None);
            };
            (Step::RemoveLink { at }, None)
        }
        10 => {
            let Some((_, at)) = stop(u)? else {
                return Ok(None);
            };
            (Step::SplitBlock { at }, Some(node_lines))
        }
        11 => {
            let previous = model
                .text_blocks()
                .take_while(|(_, _, other)| other.range.start < text.range.start)
                .last()
                .map_or(node.range.start, |(_, _, other)| other.range.start);
            (
                Step::JoinBackward { at },
                Some(SourceRange::new(
                    line_start(previous).min(node.range.start),
                    node.range.end,
                )),
            )
        }
        12 => {
            let kind = match u.int_in_range(0..=4)? {
                0 => BlockType::Paragraph,
                1 => BlockType::Heading(u.int_in_range(0..=6)?),
                2 => BlockType::Admonition(*u.choose(ADMONITIONS)?),
                3 => BlockType::Code,
                _ => BlockType::Quote,
            };
            // `roundtrip.rs` bounds headings, paragraphs and admonitions to the node.
            let bound = match kind {
                BlockType::Paragraph | BlockType::Admonition(_) => Some(node_lines),
                BlockType::Heading(level) if (1..=5).contains(&level) => Some(node_lines),
                _ => None,
            };
            (Step::SetBlockType { at, kind }, bound)
        }
        13 => (
            Step::ToggleList {
                at,
                kind: *u.choose(LISTS)?,
            },
            Some(list),
        ),
        14 => {
            let step = if u.arbitrary()? {
                Step::IndentListItem { at }
            } else {
                Step::OutdentListItem { at }
            };
            (step, Some(node_lines))
        }
        15 => {
            let Some((_, at)) = stop(u)? else {
                return Ok(None);
            };
            let step = match u.int_in_range(0..=4)? {
                0 => Step::ToggleChecked { at },
                1 => Step::InsertTableRow { at },
                2 => Step::DeleteTableRow { at },
                3 => Step::InsertTableColumn { at },
                _ => Step::DeleteTableColumn { at },
            };
            (step, None)
        }
        16 => (
            Step::InsertBlock {
                at: text.range.end,
                block: new_block(u)?,
            },
            None,
        ),
        _ => (
            Step::SetImage {
                at: node.range.start,
                target: short_string(u)?,
                alt: short_string(u)?,
            },
            None,
        ),
    }))
}

/// A step at offsets that need not be caret stops, or even character boundaries.
fn arbitrary_step(model: &Model, u: &mut Unstructured<'_>) -> Result<Step> {
    let length = model.source.len();
    let offset = |u: &mut Unstructured<'_>| u.int_in_range(0..=length + 1);
    let at = offset(u)?;
    let range = SourceRange::new(at, offset(u)?);
    Ok(match u.int_in_range(0..=9)? {
        0 => Step::InsertText {
            at,
            text: short_string(u)?,
            marks: MarkSet::EMPTY,
        },
        1 => Step::Delete { range },
        2 => Step::ToggleMark {
            range,
            mark: *u.choose(&Mark::ALL)?,
        },
        3 => Step::SplitBlock { at },
        4 => Step::JoinBackward { at },
        5 => Step::SetBlockType {
            at,
            kind: BlockType::Heading(u.arbitrary()?),
        },
        6 => Step::ToggleList {
            at,
            kind: *u.choose(LISTS)?,
        },
        7 => Step::InsertBlock {
            at,
            block: new_block(u)?,
        },
        8 => Step::RemoveLink { at },
        _ => {
            let mut edits = Vec::new();
            for _ in 0..u.int_in_range(0..=3)? {
                edits.push(SourceEdit {
                    range: SourceRange::new(offset(u)?, offset(u)?),
                    text: short_string(u)?,
                });
            }
            Step::Replace { edits }
        }
    })
}

fn new_block(u: &mut Unstructured<'_>) -> Result<NewBlock> {
    Ok(match u.int_in_range(0..=5)? {
        0 => NewBlock::Paragraph {
            text: short_string(u)?,
        },
        1 => NewBlock::ListItem {
            text: short_string(u)?,
        },
        2 => NewBlock::Code,
        3 => NewBlock::Admonition(*u.choose(ADMONITIONS)?),
        4 => NewBlock::Table {
            columns: u.int_in_range(0..=4)?,
            rows: u.int_in_range(0..=4)?,
        },
        _ => NewBlock::Image {
            target: short_string(u)?,
            alt: short_string(u)?,
        },
    })
}

/// Up to 16 characters, from the script.
fn short_string(u: &mut Unstructured<'_>) -> Result<String> {
    let text: String = u.arbitrary()?;
    Ok(text.chars().take(16).collect())
}
