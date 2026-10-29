//! The round-trip rules as properties: building and serialising gives the page back, a step's
//! edits stay inside the block it touches, and a long editing session changes only the lines
//! that were edited.

use std::path::{Path, PathBuf};

use adoc_core::SourceRange;
use adoc_dom::{AcdcParser, AdmonitionKind, DocumentParser, ListKind, ParseInput};
use adoc_model::{apply_edits, BlockType, Mark, MarkSet, Model, Step};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn build(path: &Path, text: &str) -> Model {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    Model::build(path, text, &dom)
}

fn corpus() -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "adoc")
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(&repository().join("tests/fixtures"), &mut out);
    visit(&repository().join("src/docs"), &mut out);
    out.sort();
    out
}

/// A small deterministic generator, so failures reproduce.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

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
];

const MARKS: &[Mark] = &[Mark::Strong, Mark::Emphasis, Mark::Code, Mark::Highlight];

/// A random step on an editable text block of `model`, with the range of that block.
fn random_step(model: &Model, random: &mut Random) -> Option<(Step, SourceRange)> {
    let blocks: Vec<_> = model
        .text_blocks()
        .filter(|(_, node, text)| node.is_editable() && !text.plain)
        .collect();
    if blocks.is_empty() {
        return None;
    }
    let pick = random.below(blocks.len());
    let (path, node, text) = &blocks[pick];
    let (node, text) = (*node, *text);
    // Changing a list's kind changes every item of the list.
    let list = path
        .split_last()
        .and_then(|(_, parent)| model.node(parent))
        .filter(|parent| matches!(parent.kind, adoc_model::NodeKind::List { .. }))
        .map_or(node.range, |list| list.range);
    let display_length = text.display.len();
    // One step in five changes structure, within the node (and, joining, the block before).
    if random.below(5) == 0 {
        let at = text.range.start;
        let previous_start = model
            .text_blocks()
            .take_while(|(_, _, other)| other.range.start < text.range.start)
            .last()
            .map_or(node.range.start, |(_, _, other)| other.range.start);
        let middle = text
            .display
            .char_indices()
            .map(|(index, _)| index)
            .nth(text.display.chars().count() / 2);
        let (step, start) = match random.below(6) {
            0 => (
                Step::SplitBlock {
                    at: text.source_offset(middle?, text.marks_at(middle?))?,
                },
                node.range.start,
            ),
            1 => (Step::JoinBackward { at }, previous_start),
            2 => (
                Step::SetBlockType {
                    at,
                    kind: [
                        BlockType::Heading(2),
                        BlockType::Paragraph,
                        BlockType::Admonition(AdmonitionKind::Tip),
                    ][random.below(3)],
                },
                node.range.start,
            ),
            3 => {
                let step = Step::ToggleList {
                    at,
                    kind: [ListKind::Unordered, ListKind::Ordered, ListKind::Checklist]
                        [random.below(3)],
                };
                return Some((step, list));
            }
            4 => (Step::IndentListItem { at }, node.range.start),
            _ => (Step::OutdentListItem { at }, node.range.start),
        };
        let start = model.source[..start]
            .rfind('\n')
            .map_or(0, |index| index + 1)
            .min(start);
        return Some((
            step,
            SourceRange::new(start.min(node.range.start), node.range.end),
        ));
    }
    let char_offset = |random: &mut Random| {
        let boundaries: Vec<usize> = text
            .display
            .char_indices()
            .map(|(index, _)| index)
            .chain([display_length])
            .collect();
        boundaries[random.below(boundaries.len())]
    };
    let choice = random.below(10);
    let start = char_offset(random);
    let marks = text.marks_at(start);
    let step = if choice < 5 || display_length == 0 {
        Step::InsertText {
            at: text.source_offset(start, marks)?,
            text: WORDS[random.below(WORDS.len())].to_owned(),
            marks,
        }
    } else {
        let end = char_offset(random);
        let (from, to) = (start.min(end), start.max(end));
        let range = SourceRange::new(
            text.source_offset(from, text.marks_at(from))?,
            text.source_offset(to, text.marks_at(to))?,
        );
        if range.start >= range.end {
            return None;
        }
        if choice < 7 {
            Step::Delete { range }
        } else {
            Step::ToggleMark {
                range,
                mark: MARKS[random.below(MARKS.len())],
            }
        }
    };
    Some((step, text.range))
}

/// The lines of `text` that `range` touches, as line indices.
fn lines_touched(text: &str, range: SourceRange) -> std::ops::RangeInclusive<usize> {
    let first = text[..range.start].matches('\n').count();
    let last = text[..range.end].matches('\n').count();
    first..=last
}

#[test]
fn serialise_is_identity_on_the_corpus() {
    let corpus = corpus();
    assert!(corpus.len() > 30);
    for path in corpus {
        let text = std::fs::read_to_string(&path).unwrap();
        let model = build(&path, &text);
        assert!(
            model.serialise() == text,
            "{} does not round-trip",
            path.display()
        );
        for (_, _, block) in model.text_blocks() {
            assert_eq!(
                block.serialise(&text),
                &text[block.range.start..block.range.end]
            );
        }
    }
}

#[test]
fn edits_stay_inside_the_touched_block() {
    let mut random = Random(7);
    let mut checked = 0;
    for path in corpus() {
        let text = std::fs::read_to_string(&path).unwrap();
        let model = build(&path, &text);
        for _ in 0..40 {
            let Some((step, block)) = random_step(&model, &mut random) else {
                continue;
            };
            let Ok(result) = model.apply(&step) else {
                continue;
            };
            for edit in &result.edits {
                assert!(
                    block.start <= edit.range.start && edit.range.end <= block.end,
                    "{}: {step:?} edited {:?} outside {block:?}",
                    path.display(),
                    edit.range
                );
            }
            let after = apply_edits(&model.source, &result.edits);
            assert!(model.serialise() == text);
            if let Some(local) = &result.model {
                assert!(local.serialise() == after, "{step:?}");
            }
            checked += 1;
        }
    }
    assert!(checked > 500, "only {checked} steps applied");
}

#[test]
fn a_long_editing_session_touches_only_edited_lines() {
    let pages = [
        "src/docs/modules/ROOT/pages/using-the-editor.adoc",
        "src/docs/modules/ROOT/pages/preview.adoc",
        "src/docs/modules/developer/pages/engine.adoc",
        "tests/fixtures/render/blocks.adoc",
        "tests/fixtures/render/inline.adoc",
    ];
    let mut random = Random(2026);
    let mut applied = 0;
    for page in pages {
        let path = repository().join(page);
        let original = std::fs::read_to_string(&path).unwrap();
        let mut model = build(&path, &original);
        // For each current line, the original line it still is, if untouched.
        let mut origin: Vec<Option<usize>> = (0..original.lines().count()).map(Some).collect();
        let mut targeted: Vec<usize> = Vec::new();
        for _ in 0..150 {
            let Some((step, block)) = random_step(&model, &mut random) else {
                continue;
            };
            let Ok(result) = model.apply(&step) else {
                continue;
            };
            // The writer edited the lines of this block.
            for line in lines_touched(&model.source, block) {
                if let Some(Some(original_line)) = origin.get(line) {
                    targeted.push(*original_line);
                }
            }
            for edit in result.edits.iter().rev() {
                let touched = lines_touched(&model.source, edit.range);
                assert!(
                    lines_touched(&model.source, block).contains(touched.start())
                        && lines_touched(&model.source, block).contains(touched.end()),
                    "{page}: {step:?} touched lines outside its block"
                );
                let added = edit.text.matches('\n').count();
                let (first, last) = (*touched.start(), *touched.end());
                let last = last.min(origin.len().saturating_sub(1));
                if first < origin.len() {
                    origin.splice(first..=last, std::iter::repeat_n(None, added + 1));
                }
            }
            let after = apply_edits(&model.source, &result.edits);
            model = match result.model {
                Some(local) => local,
                None => build(&path, &after),
            };
            assert!(model.source == after);
            applied += 1;
        }
        // Rebuilding from the final text gives the model the session ended with.
        let rebuilt = build(&path, &model.source);
        assert!(rebuilt.serialise() == model.source);
        targeted.sort_unstable();
        targeted.dedup();
        // Every line still traced to the original is unchanged, and every original line that
        // is not was in a block a step targeted. (A line diff cannot say which of two equal
        // blank lines survived an edit between them, so the tracing decides.)
        let original_lines: Vec<&str> = original.lines().collect();
        let current: Vec<&str> = model.source.lines().collect();
        assert_eq!(origin.len(), current.len(), "{page}: lines lost track of");
        for (now, was) in origin.iter().enumerate() {
            if let Some(was) = was {
                assert_eq!(
                    current[now],
                    original_lines[*was],
                    "{page}: line {} changed without being edited",
                    was + 1
                );
            }
        }
        let kept: std::collections::BTreeSet<usize> = origin.iter().flatten().copied().collect();
        for line in (0..original_lines.len()).filter(|line| !kept.contains(line)) {
            assert!(
                targeted.binary_search(&line).is_ok(),
                "{page}: line {} changed without being edited: {:?}",
                line + 1,
                original_lines[line]
            );
        }
    }
    assert!(applied > 500, "only {applied} steps applied");
}

#[test]
fn the_session_model_matches_a_rebuild() {
    let path = repository().join("src/docs/modules/ROOT/pages/using-the-editor.adoc");
    let original = std::fs::read_to_string(&path).unwrap();
    let mut model = build(&path, &original);
    let mut random = Random(11);
    let (mut same, mut steps) = (0, 0);
    for _ in 0..200 {
        let Some((step, _)) = random_step(&model, &mut random) else {
            continue;
        };
        let Ok(result) = model.apply(&step) else {
            continue;
        };
        let after = apply_edits(&model.source, &result.edits);
        let rebuilt = build(&path, &after);
        if let Some(local) = result.model {
            steps += 1;
            let shown = |model: &Model| -> Vec<(String, MarkSet)> {
                model
                    .text_blocks()
                    .map(|(_, _, text)| (text.display.clone(), text.marks_at(text.display.len())))
                    .collect()
            };
            same += usize::from(shown(&local) == shown(&rebuilt));
        }
        model = rebuilt;
    }
    // The local update and the parser agree on what every block shows almost always; where
    // they do not, the rebuild after `MODEL_SETTLE` corrects the view.
    assert!(
        same * 100 >= steps * 95,
        "{same} of {steps} local models matched a rebuild"
    );
}
