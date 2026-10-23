//! Steps: edits of the model, each producing the smallest source edits that make it.

use std::fmt;

use adoc_core::SourceRange;

use crate::{
    build::Builder,
    inline::{constrained_fits, flatten, Caret, Context, Flat, Frame, Refusal, Unit, UnitKind},
    mapping::{apply_edits, minimal_edit, Mapping},
    tree::{Atom, BlockPath, Mark, MarkSet, Model, Node, Piece, TextBlock, TextContext},
    SourceEdit,
};

/// An edit of the model. Positions are byte offsets into the page, at caret stops.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Inserts typed text at `at`, carrying `marks`.
    InsertText {
        at: usize,
        text: String,
        marks: MarkSet,
    },
    /// Deletes the text in `range`; atoms it touches go whole.
    Delete {
        range: SourceRange,
    },
    /// Adds `mark` where any of `range` lacks it, or else removes it.
    ToggleMark {
        range: SourceRange,
        mark: Mark,
    },
    /// Makes `range` a link to `target` (a URL, a page or an anchor id), or changes the link
    /// it is inside; `text` replaces the link's text when given.
    SetLink {
        range: SourceRange,
        target: String,
        text: Option<String>,
    },
    /// Replaces the link at `at` with its text.
    RemoveLink {
        at: usize,
    },
    /// Enter: splits the block at `at` in two.
    SplitBlock {
        at: usize,
    },
    /// Backspace at the start of a block: joins it to the block before, or lifts a list item
    /// out of its list.
    JoinBackward {
        at: usize,
    },
    /// Adds a block after the block holding `at`.
    InsertBlock {
        at: usize,
        block: crate::NewBlock,
    },
    SetBlockType {
        at: usize,
        kind: crate::BlockType,
    },
    /// Makes a paragraph a list item of `kind`, changes the kind of the list, or lifts the item
    /// out of a list of that kind.
    ToggleList {
        at: usize,
        kind: adoc_dom::ListKind,
    },
    IndentListItem {
        at: usize,
    },
    OutdentListItem {
        at: usize,
    },
    ToggleChecked {
        at: usize,
    },
    /// Raw source edits: the form an inverse takes.
    Replace {
        edits: Vec<SourceEdit>,
    },
}

/// What a step does to the source.
#[derive(Clone, Debug, PartialEq)]
pub struct StepResult {
    /// Sorted, non-overlapping, in the coordinates of the text before the step.
    pub edits: Vec<SourceEdit>,
    /// The selection afterwards, in the coordinates of the text after the step.
    pub selection: SourceRange,
    /// The model after the step, for steps that change no structure; `None` means the model is
    /// rebuilt from the new text.
    pub model: Option<Model>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepError {
    /// The position is not in text the model edits.
    NotEditable,
    /// The position or range is outside the page or not on a character boundary.
    OutOfRange,
    /// No way of writing the edit keeps the meaning of the text around it.
    WouldChangeMeaning,
    /// The edit is not available here.
    Unsupported(&'static str),
}

impl fmt::Display for StepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEditable => formatter.write_str("this text is not editable here; edit it in the source"),
            Self::OutOfRange => formatter.write_str("the position is outside the page"),
            Self::WouldChangeMeaning => formatter.write_str(
                "the edit cannot be written without changing the text around it; edit it in the source",
            ),
            Self::Unsupported(what) => formatter.write_str(what),
        }
    }
}

impl std::error::Error for StepError {}

impl From<Refusal> for StepError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::WouldChangeMeaning => Self::WouldChangeMeaning,
        }
    }
}

impl Model {
    /// What `step` would do. The model itself is unchanged: apply the result's edits to the
    /// document, and use the result's model (or rebuild).
    pub fn apply(&self, step: &Step) -> Result<StepResult, StepError> {
        match step {
            Step::InsertText { at, text, marks } => self.insert_text(*at, text, *marks),
            Step::Delete { range } => self.delete(*range),
            Step::ToggleMark { range, mark } => self.toggle_mark(*range, *mark),
            Step::SetLink {
                range,
                target,
                text,
            } => self.set_link(*range, target, text.as_deref()),
            Step::RemoveLink { at } => self.remove_link(*at),
            Step::SplitBlock { at } => self.split_block(*at),
            Step::JoinBackward { at } => self.join_backward(*at),
            Step::InsertBlock { at, block } => self.insert_block(*at, block),
            Step::SetBlockType { at, kind } => self.set_block_type(*at, *kind),
            Step::ToggleList { at, kind } => self.toggle_list(*at, *kind),
            Step::IndentListItem { at } => self.indent(*at),
            Step::OutdentListItem { at } => self.outdent(*at),
            Step::ToggleChecked { at } => self.toggle_checked(*at),
            Step::Replace { edits } => self.replace(edits),
        }
    }

    /// The flattened inline block holding `range`, which must lie in one editable text block.
    fn inline_block(&self, range: SourceRange) -> Result<(BlockPath, &TextBlock), StepError> {
        if range.start > range.end {
            return Err(StepError::OutOfRange);
        }
        self.check_offset(range.end)?;
        let (path, block) = self.editable_at(range.start)?;
        if range.end > block.range.end {
            return Err(StepError::Unsupported("a link stays within one block"));
        }
        if block.plain {
            return Err(StepError::NotEditable);
        }
        Ok((path, block))
    }

    fn set_link(
        &self,
        range: SourceRange,
        target: &str,
        text: Option<&str>,
    ) -> Result<StepResult, StepError> {
        let target = target.trim();
        if target.is_empty() || target.contains(char::is_whitespace) {
            return Err(StepError::Unsupported("a link target has no spaces"));
        }
        let (path, block) = self.inline_block(range)?;
        let flat = flatten(&self.source, block);
        // A link the selection lies in is changed, keeping its form and text.
        let existing = flat.units.iter().zip(&flat.ranges).position(|(unit, at)| {
            matches!(
                unit.kind,
                UnitKind::Atom(Atom::Link { .. } | Atom::Xref { .. })
            ) && at.start <= range.start
                && range.end <= at.end
                && (!range.is_empty() || (at.start < range.start && range.start < at.end))
        });
        let (mut units, first, last, link_text, macro_form) = match existing {
            Some(index) => {
                let unit = &flat.units[index];
                let old_text = match &unit.kind {
                    UnitKind::Atom(Atom::Link { text, .. } | Atom::Xref { text, .. }) => {
                        text.clone()
                    }
                    _ => None,
                };
                let macro_form = unit.source.starts_with("xref:");
                let link_text = text.map(str::to_owned).or(old_text);
                (flat.units.clone(), index, index + 1, link_text, macro_form)
            }
            None => {
                let start = block
                    .display_offset(range.start)
                    .ok_or(StepError::OutOfRange)?;
                let end = block
                    .display_offset_towards(range.end, true)
                    .ok_or(StepError::OutOfRange)?;
                let selected = block.display[start..end].to_owned();
                let (units, first, last) = split_display(flat.units.clone(), start, end);
                let link_text = text
                    .map(str::to_owned)
                    .or_else(|| (!selected.is_empty()).then_some(selected));
                (units, first, last, link_text, false)
            }
        };
        let stack = match units.get(first) {
            Some(unit) if first < last => unit.stack.clone(),
            _ => frames_around(&flat, range.start),
        };
        let source = link_source(target, link_text.as_deref(), macro_form);
        let url = is_url(target);
        let shown = link_text.clone().unwrap_or_else(|| {
            if url {
                target.to_owned()
            } else {
                crate::scan::xref_label(target)
            }
        });
        let atom = if url {
            Atom::Link {
                target: target.to_owned(),
                text: link_text,
            }
        } else {
            Atom::Xref {
                target: target.to_owned(),
                text: link_text,
            }
        };
        let display = units[..first]
            .iter()
            .map(|unit| unit.shown.len())
            .sum::<usize>();
        let length = shown.len();
        units.splice(
            first..last,
            [Unit {
                kind: UnitKind::Atom(atom),
                shown,
                source,
                stack,
            }],
        );
        let selection = (
            Caret {
                display,
                sticky: false,
            },
            Caret {
                display: display + length,
                sticky: true,
            },
        );
        let realised = self.context(block).realise(units, &[], selection)?;
        self.finish(&path, block, &realised.text, realised.selection)
    }

    fn remove_link(&self, at: usize) -> Result<StepResult, StepError> {
        let (path, block) = self.inline_block(SourceRange::new(at, at))?;
        let flat = flatten(&self.source, block);
        let index = flat
            .units
            .iter()
            .zip(&flat.ranges)
            .position(|(unit, range)| {
                matches!(
                    unit.kind,
                    UnitKind::Atom(Atom::Link { .. } | Atom::Xref { .. })
                ) && range.start <= at
                    && at <= range.end
            })
            .ok_or(StepError::Unsupported("there is no link here"))?;
        let mut units = flat.units.clone();
        let display = units[..index]
            .iter()
            .map(|unit| unit.shown.len())
            .sum::<usize>();
        let shown = units[index].shown.clone();
        let stack = units[index].stack.clone();
        let length = shown.len();
        units[index] = Unit {
            kind: UnitKind::Text,
            source: shown.clone(),
            shown,
            stack,
        };
        let caret = Caret {
            display: display + length,
            sticky: true,
        };
        let realised = self.context(block).realise(units, &[], (caret, caret))?;
        self.finish(&path, block, &realised.text, realised.selection)
    }

    fn check_offset(&self, offset: usize) -> Result<(), StepError> {
        if offset > self.source.len() || !self.source.is_char_boundary(offset) {
            return Err(StepError::OutOfRange);
        }
        Ok(())
    }

    /// The editable text block containing `offset`.
    fn editable_at(&self, offset: usize) -> Result<(BlockPath, &TextBlock), StepError> {
        self.check_offset(offset)?;
        let (path, node, text) = self.text_block_at(offset).ok_or(StepError::NotEditable)?;
        if !node.is_editable() {
            return Err(StepError::NotEditable);
        }
        Ok((path, text))
    }

    fn context<'a>(&'a self, block: &'a TextBlock) -> Context<'a> {
        Context {
            source: &self.source,
            attributes: &self.attributes,
            block,
        }
    }

    fn insert_text(&self, at: usize, text: &str, marks: MarkSet) -> Result<StepResult, StepError> {
        let (path, block) = self.editable_at(at)?;
        let text = sanitise(text, block.context);
        if text.is_empty() {
            return Ok(self.unchanged(SourceRange::new(at, at)));
        }
        let display = block.display_offset(at).ok_or(StepError::OutOfRange)?;
        if block.plain {
            return self.edit_plain(
                &path,
                block,
                SourceEdit::insert(at, text.clone()),
                at + text.len(),
            );
        }
        let flat = flatten(&self.source, block);
        let index = split_at_source(&flat, at)?;
        let mut units = index.units;
        let context: Vec<Frame> = frames_around(&flat, at);
        let stack: Vec<Frame> = context
            .iter()
            .filter(|frame| marks.contains(frame.mark) || frame.mark == Mark::Span)
            .cloned()
            .collect();
        let mut touched: Vec<u32> = context
            .iter()
            .filter(|frame| !stack.contains(frame))
            .map(|frame| frame.id)
            .collect();
        // Marks either side may need the unconstrained form once text runs up against them.
        for neighbour in [index.at.checked_sub(1), Some(index.at)]
            .into_iter()
            .flatten()
        {
            if let Some(unit) = units.get(neighbour) {
                touched.extend(unit.stack.iter().map(|frame| frame.id));
            }
        }
        let shown_after = {
            let mut shown = block.display.clone();
            shown.insert_str(display, &text);
            shown
        };
        // A new mark takes the typed text without its outer whitespace, which stays outside.
        let core_start = text.len() - text.trim_start().len();
        let core_end = text.trim_end().len().max(core_start);
        let mut core_stack = stack.clone();
        let mut next_id = flat.next_id;
        for mark in marks.iter().filter(|mark| *mark != Mark::Span) {
            if core_start == core_end || core_stack.iter().any(|frame| frame.mark == mark) {
                continue;
            }
            core_stack.push(new_frame(
                next_id,
                mark,
                &shown_after,
                display + core_start,
                display + core_end,
            ));
            touched.push(next_id);
            next_id += 1;
        }
        let parts = [
            (&text[..core_start], &stack),
            (&text[core_start..core_end], &core_stack),
            (&text[core_end..], &stack),
        ];
        let inserted: Vec<Unit> = parts
            .into_iter()
            .filter(|(part, _)| !part.is_empty())
            .map(|(part, stack)| Unit {
                kind: UnitKind::Text,
                shown: part.to_owned(),
                source: part.to_owned(),
                stack: stack.clone(),
            })
            .collect();
        units.splice(index.at..index.at, inserted);
        let caret = Caret {
            display: display + text.len(),
            sticky: true,
        };
        let realised = self
            .context(block)
            .realise(units, &touched, (caret, caret))?;
        let mut result = self.finish(&path, block, &realised.text, realised.selection)?;
        // Typed text written as it was typed is reported as an insertion at the caret, which a
        // diff cannot tell apart from one beside it when the text repeats its neighbour.
        let relative = at - block.range.start;
        let old = &self.source[block.range.start..block.range.end];
        if realised.text.len() == old.len() + text.len()
            && realised.text.starts_with(&old[..relative])
            && realised.text[relative..].starts_with(text.as_str())
            && realised.text.ends_with(&old[relative..])
        {
            result.edits = vec![SourceEdit::insert(at, text.clone())];
        }
        Ok(result)
    }

    fn delete(&self, range: SourceRange) -> Result<StepResult, StepError> {
        if range.start > range.end {
            return Err(StepError::OutOfRange);
        }
        self.check_offset(range.end)?;
        let (path, block) = self.editable_at(range.start)?;
        if range.end > block.range.end {
            return Err(StepError::Unsupported("deleting across blocks"));
        }
        if range.is_empty() {
            return Ok(self.unchanged(range));
        }
        if block.plain {
            return self.edit_plain(&path, block, SourceEdit::delete(range), range.start);
        }
        let start = block
            .display_offset(range.start)
            .ok_or(StepError::OutOfRange)?;
        let end = block
            .display_offset_towards(range.end, true)
            .ok_or(StepError::OutOfRange)?;
        let flat = flatten(&self.source, block);
        let (units, first, last) = split_display(flat.units, start, end);
        let mut touched: Vec<u32> = Vec::new();
        for index in [first.checked_sub(1), Some(last)].into_iter().flatten() {
            if let Some(unit) = units.get(index) {
                touched.extend(unit.stack.iter().map(|frame| frame.id));
            }
        }
        let display = units[..first]
            .iter()
            .map(|unit| unit.shown.len())
            .sum::<usize>();
        let mut kept = units[..first].to_vec();
        kept.extend_from_slice(&units[last..]);
        let caret = Caret {
            display,
            sticky: display > 0,
        };
        let realised = self
            .context(block)
            .realise(kept, &touched, (caret, caret))?;
        self.finish(&path, block, &realised.text, realised.selection)
    }

    fn toggle_mark(&self, range: SourceRange, mark: Mark) -> Result<StepResult, StepError> {
        if range.start > range.end {
            return Err(StepError::OutOfRange);
        }
        self.check_offset(range.start)?;
        self.check_offset(range.end)?;
        // The editable text blocks the range covers, with the display range in each.
        let mut covered: Vec<(BlockPath, &TextBlock, usize, usize)> = Vec::new();
        for (path, node, text) in self.text_blocks() {
            if text.range.end < range.start || text.range.start > range.end || text.plain {
                continue;
            }
            if !node.is_editable() {
                continue;
            }
            let start = text
                .display_offset(range.start.max(text.range.start))
                .unwrap_or(0);
            let end = text
                .display_offset(range.end.min(text.range.end))
                .unwrap_or(text.display.len());
            if start < end || range.is_empty() {
                covered.push((path, text, start, end));
            }
        }
        if covered.is_empty() {
            return Err(StepError::NotEditable);
        }
        if range.is_empty() {
            return Err(StepError::Unsupported("select the text to format"));
        }
        let remove = covered
            .iter()
            .all(|(_, text, start, end)| text.has_mark(*start..*end, mark));
        let mut edits = Vec::new();
        let mut written: Vec<(SourceRange, String, (usize, usize))> = Vec::new();
        for (_, block, start, end) in &covered {
            let (start, end) = if remove {
                (*start, *end)
            } else {
                trim_whitespace(&block.display, *start, *end)
            };
            if start >= end {
                continue;
            }
            if !remove
                && !mark.has_forms()
                && block.display[start..end].contains(char::is_whitespace)
            {
                return Err(StepError::WouldChangeMeaning);
            }
            let flat = flatten(&self.source, block);
            let next_id = flat.next_id;
            // Adding a mark absorbs marks of the same kind that overlap the range.
            let (start, end) = if remove {
                (start, end)
            } else {
                absorb(&flat.units, mark, start, end)
            };
            let (mut units, first, last) = split_display(flat.units, start, end);
            let mut touched = Vec::new();
            if remove {
                for unit in &mut units[first..last] {
                    touched.extend(
                        unit.stack
                            .iter()
                            .filter(|frame| frame.mark == mark)
                            .map(|frame| frame.id),
                    );
                    unit.stack.retain(|frame| frame.mark != mark);
                }
            } else {
                let frame = new_frame(next_id, mark, &block.display, start, end);
                touched.push(frame.id);
                for unit in &mut units[first..last] {
                    touched.extend(
                        unit.stack
                            .iter()
                            .filter(|existing| existing.mark == mark)
                            .map(|existing| existing.id),
                    );
                    unit.stack.retain(|existing| existing.mark != mark);
                }
                let uniform = units[first..last]
                    .windows(2)
                    .all(|pair| pair[0].stack == pair[1].stack);
                for unit in &mut units[first..last] {
                    touched.extend(unit.stack.iter().map(|existing| existing.id));
                    if uniform {
                        unit.stack.push(frame.clone());
                    } else {
                        unit.stack.insert(0, frame.clone());
                    }
                }
            }
            let selection = (
                Caret {
                    display: start,
                    sticky: false,
                },
                Caret {
                    display: end,
                    sticky: true,
                },
            );
            let realised = self.context(block).realise(units, &touched, selection)?;
            let old = &self.source[block.range.start..block.range.end];
            if let Some(edit) = minimal_edit(block.range, old, &realised.text) {
                edits.push(edit);
            }
            written.push((block.range, realised.text, realised.selection));
        }
        if written.is_empty() {
            return Ok(self.unchanged(range));
        }
        // Selection: from the first block's start to the last block's end, shifted by the
        // edits before each.
        let mapping = Mapping::new(&edits);
        let shift = |range: SourceRange| mapping.map(range.start, crate::Assoc::Before);
        let (first_range, _, first_selection) = &written[0];
        let (last_range, _, last_selection) = &written[written.len() - 1];
        let selection = SourceRange::new(
            shift(*first_range) + first_selection.0,
            shift(*last_range) + last_selection.1,
        );
        let mut model = self.clone();
        for (old_range, text, _) in written.iter().rev() {
            model = model.with_block_text(*old_range, text);
        }
        Ok(StepResult {
            edits,
            selection,
            model: Some(model),
        })
    }

    fn replace(&self, edits: &[SourceEdit]) -> Result<StepResult, StepError> {
        let mut previous = 0;
        for edit in edits {
            if edit.range.start < previous || edit.range.start > edit.range.end {
                return Err(StepError::OutOfRange);
            }
            self.check_offset(edit.range.start)?;
            self.check_offset(edit.range.end)?;
            previous = edit.range.end;
        }
        let mapping = Mapping::new(edits);
        let end = edits.last().map_or(0, |edit| edit.range.end);
        let caret = mapping.map(end, crate::Assoc::After);
        Ok(StepResult {
            edits: edits.to_vec(),
            selection: SourceRange::new(caret, caret),
            model: None,
        })
    }

    fn unchanged(&self, selection: SourceRange) -> StepResult {
        StepResult {
            edits: Vec::new(),
            selection,
            model: Some(self.clone()),
        }
    }

    /// An edit of code or literal text, refused when it would end the block early.
    fn edit_plain(
        &self,
        path: &[usize],
        block: &TextBlock,
        edit: SourceEdit,
        caret: usize,
    ) -> Result<StepResult, StepError> {
        let old = &self.source[block.range.start..block.range.end];
        let local = SourceEdit::new(
            SourceRange::new(
                edit.range.start - block.range.start,
                edit.range.end - block.range.start,
            ),
            edit.text.clone(),
        );
        let new = apply_edits(old, &[local]);
        let opening = self.source[..block.range.start]
            .trim_end_matches('\n')
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .trim_end();
        if !opening.is_empty() && new.lines().any(|line| line.trim_end() == opening) {
            return Err(StepError::WouldChangeMeaning);
        }
        let relative = caret - block.range.start;
        self.finish(path, block, &new, (relative, relative))
    }

    /// The result of giving the block at `path` the text `new`.
    fn finish(
        &self,
        _path: &[usize],
        block: &TextBlock,
        new: &str,
        selection: (usize, usize),
    ) -> Result<StepResult, StepError> {
        let old = &self.source[block.range.start..block.range.end];
        let edits: Vec<SourceEdit> = minimal_edit(block.range, old, new).into_iter().collect();
        let start = block.range.start;
        Ok(StepResult {
            edits,
            selection: SourceRange::new(start + selection.0, start + selection.1),
            model: Some(self.with_block_text(block.range, new)),
        })
    }

    /// The model with the text block at `range` holding `new` instead, its pieces rescanned and
    /// every other range moved.
    pub(crate) fn with_block_text(&self, range: SourceRange, new: &str) -> Model {
        let old = &self.source[range.start..range.end];
        let Some(edit) = minimal_edit(range, old, new) else {
            return self.clone();
        };
        let mapping = Mapping::new(std::slice::from_ref(&edit));
        let mut model = self.clone();
        model.source = apply_edits(&self.source, std::slice::from_ref(&edit));
        let new_range = SourceRange::new(range.start, range.start + new.len());
        let mut footnotes = 0;
        let mut seen_block = false;
        for node in &mut model.blocks {
            remap(node, &mapping, range, &mut footnotes, &mut seen_block);
        }
        let builder = Builder {
            page: &self.path,
            source: &model.source,
            attributes: &self.attributes,
            footnotes: std::cell::Cell::new(footnotes),
        };
        let edited = text_block_mut(&mut model.blocks, range).map(|text| text.context);
        if let Some(context) = edited {
            let rescanned = builder.text_block(new_range, context);
            if let Some(text) = text_block_mut(&mut model.blocks, range) {
                *text = rescanned;
            }
        }
        model
    }
}

/// Moves every range in `node` through `mapping`, except the pieces of the edited block (at
/// `edited`), which are rescanned; counts the footnotes before it.
fn remap(
    node: &mut Node,
    mapping: &Mapping,
    edited: SourceRange,
    footnotes: &mut u32,
    seen: &mut bool,
) {
    let is_edited = node.text.as_ref().is_some_and(|text| text.range == edited);
    node.range = mapping.map_range(node.range);
    if let Some(text) = &mut node.text {
        if is_edited {
            *seen = true;
        } else {
            if !*seen {
                *footnotes += count_footnotes(&text.pieces);
            }
            text.range = mapping.map_range(text.range);
            remap_pieces(&mut text.pieces, mapping);
        }
    }
    for child in &mut node.children {
        remap(child, mapping, edited, footnotes, seen);
    }
}

fn remap_pieces(pieces: &mut [Piece], mapping: &Mapping) {
    for piece in pieces {
        match piece {
            Piece::Text { range } | Piece::Atom { range, .. } => *range = mapping.map_range(*range),
            Piece::Mark {
                open,
                close,
                children,
                ..
            } => {
                *open = mapping.map_range(*open);
                *close = mapping.map_range(*close);
                remap_pieces(children, mapping);
            }
        }
    }
}

fn count_footnotes(pieces: &[Piece]) -> u32 {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Atom {
                atom: Atom::Footnote { .. },
                ..
            } => 1,
            Piece::Mark { children, .. } => count_footnotes(children),
            _ => 0,
        })
        .sum()
}

/// The text block whose range (before remapping) started at `range.start`: its node's range
/// has moved, but the block keeps its start because edits stay inside it.
fn text_block_mut(nodes: &mut [Node], range: SourceRange) -> Option<&mut TextBlock> {
    for node in nodes {
        let holds = node
            .text
            .as_ref()
            .is_some_and(|text| text.range.start == range.start && text.range.end >= range.start);
        if holds {
            return node.text.as_mut();
        }
        if let Some(found) = text_block_mut(&mut node.children, range) {
            return Some(found);
        }
    }
    None
}

/// Text as it may be typed into a block of `context`.
fn sanitise(text: &str, context: TextContext) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    match context {
        TextContext::Verbatim => text,
        TextContext::Line | TextContext::Cell => text.replace('\n', " "),
        // A blank line would end the paragraph: keep single line breaks only.
        TextContext::Flow => {
            let mut out = String::with_capacity(text.len());
            for character in text.chars() {
                if character == '\n' && out.ends_with('\n') {
                    continue;
                }
                out.push(character);
            }
            out
        }
    }
}

fn new_frame(id: u32, mark: Mark, display: &str, start: usize, end: usize) -> Frame {
    let character = mark.delimiter();
    let constrained = !mark.has_forms() || constrained_fits(display, start, end);
    Frame {
        id,
        mark,
        prefix: String::new(),
        delimiter: if constrained {
            character.to_string()
        } else {
            [character, character].iter().collect()
        },
    }
}

/// The frames whose content contains source offset `at`, outermost first.
fn frames_around(flat: &Flat, at: usize) -> Vec<Frame> {
    let mut frames: Vec<&(Frame, usize, usize)> = flat
        .frames
        .iter()
        .filter(|(_, open_end, close_start)| *open_end <= at && at <= *close_start)
        .collect();
    frames.sort_by_key(|(_, open_end, _)| *open_end);
    frames
        .into_iter()
        .map(|(frame, _, _)| frame.clone())
        .collect()
}

/// The units with the text unit at `at` split, and the index new text goes in at.
struct Split {
    units: Vec<Unit>,
    at: usize,
}

fn split_at_source(flat: &Flat, at: usize) -> Result<Split, StepError> {
    let mut units = Vec::with_capacity(flat.units.len() + 1);
    let mut insert_at = None;
    for (unit, range) in flat.units.iter().zip(&flat.ranges) {
        if insert_at.is_none() && range.start >= at {
            insert_at = Some(units.len());
        }
        if insert_at.is_none() && range.start < at && at < range.end {
            if unit.kind != UnitKind::Text {
                return Err(StepError::OutOfRange);
            }
            let split = at - range.start;
            units.push(Unit {
                shown: unit.shown[..split].to_owned(),
                source: unit.source[..split].to_owned(),
                ..unit.clone()
            });
            insert_at = Some(units.len());
            units.push(Unit {
                shown: unit.shown[split..].to_owned(),
                source: unit.source[split..].to_owned(),
                ..unit.clone()
            });
            continue;
        }
        units.push(unit.clone());
    }
    let at = insert_at.unwrap_or(units.len());
    Ok(Split { units, at })
}

/// The units split so that display offsets `start` and `end` fall between units (atoms
/// widen the range to take them whole), with the indices of the first unit inside and the
/// first unit after.
pub(crate) fn split_display(
    units: Vec<Unit>,
    start: usize,
    end: usize,
) -> (Vec<Unit>, usize, usize) {
    let mut out = Vec::with_capacity(units.len() + 2);
    let mut at = 0;
    let mut first = None;
    let mut last = None;
    for unit in units {
        let length = unit.shown.len();
        let (unit_start, unit_end) = (at, at + length);
        at = unit_end;
        let cut = |offset: usize| unit_start < offset && offset < unit_end;
        if unit.kind == UnitKind::Text && (cut(start) || cut(end)) {
            let mut points = vec![0];
            for offset in [start, end] {
                if cut(offset) {
                    points.push(offset - unit_start);
                }
            }
            points.push(length);
            points.dedup();
            for pair in points.windows(2) {
                let piece_start = unit_start + pair[0];
                if first.is_none() && piece_start >= start {
                    first = Some(out.len());
                }
                if last.is_none() && piece_start >= end {
                    last = Some(out.len());
                }
                out.push(Unit {
                    shown: unit.shown[pair[0]..pair[1]].to_owned(),
                    source: unit.source[pair[0]..pair[1]].to_owned(),
                    ..unit.clone()
                });
            }
            continue;
        }
        // An atom the range touches goes whole.
        let inside = if unit.kind == UnitKind::Text || length == 0 {
            unit_start >= start
        } else {
            unit_end > start
        };
        if first.is_none() && inside {
            first = Some(out.len());
        }
        if last.is_none() && unit_start >= end {
            last = Some(out.len());
        }
        out.push(unit);
    }
    let first = first.unwrap_or(out.len());
    let last = last.unwrap_or(out.len()).max(first);
    (out, first, last)
}

fn is_url(target: &str) -> bool {
    target.contains("://") || target.starts_with("mailto:")
}

/// A link as AsciiDoc: a URL with its text in brackets, an `xref:` to a page, or `<<id>>` for
/// an anchor in the page (unless the link it replaces was an `xref:`).
fn link_source(target: &str, text: Option<&str>, macro_form: bool) -> String {
    let text = text.map(|text| text.replace(']', "\\]"));
    if is_url(target) {
        return format!("{target}[{}]", text.unwrap_or_default());
    }
    let page = target.contains(['.', '$', ':', '/']) || target.starts_with('#');
    if page || macro_form {
        return format!("xref:{target}[{}]", text.unwrap_or_default());
    }
    match text {
        Some(text) => format!("<<{target},{text}>>"),
        None => format!("<<{target}>>"),
    }
}

/// `start..end` widened to take in every frame of `mark` it overlaps.
fn absorb(units: &[Unit], mark: Mark, start: usize, end: usize) -> (usize, usize) {
    let mut extents: Vec<(u32, usize, usize)> = Vec::new();
    let mut at = 0;
    for unit in units {
        let unit_end = at + unit.shown.len();
        for frame in unit.stack.iter().filter(|frame| frame.mark == mark) {
            match extents.iter_mut().find(|(id, _, _)| *id == frame.id) {
                Some(extent) => extent.2 = unit_end,
                None => extents.push((frame.id, at, unit_end)),
            }
        }
        at = unit_end;
    }
    extents
        .into_iter()
        .filter(|(_, from, to)| *from < end && start < *to)
        .fold((start, end), |(start, end), (_, from, to)| {
            (start.min(from), end.max(to))
        })
}

/// `start..end` of `display` without whitespace at either end.
fn trim_whitespace(display: &str, start: usize, end: usize) -> (usize, usize) {
    let text = &display[start..end];
    let leading = text.len() - text.trim_start().len();
    let trailing = text.len() - text.trim_end().len();
    (start + leading, (end - trailing).max(start + leading))
}
