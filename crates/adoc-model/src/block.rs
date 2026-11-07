//! Structural steps: splitting and joining blocks, new blocks, block types and lists.
//!
//! Each changes as little source as it can (a prefix, a marker, a blank line) and leaves the
//! model to be rebuilt from the new text, so structure always comes from the parser.

use adoc_core::SourceRange;
use adoc_dom::{AdmonitionKind, ListKind};

use crate::{
    build::list_marker,
    inline::{flatten, starts_block, write, Caret, Unit, UnitKind},
    mapping::{minimal_edit, Assoc, Mapping},
    step::{sanitise, split_display, StepError, StepResult},
    tree::{BlockPath, Model, Node, NodeKind, TextBlock, TextContext},
    SourceEdit,
};

/// What a paragraph becomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockType {
    Paragraph,
    /// A section heading, level 1 (`==`) to 5.
    Heading(u8),
    /// An admonition paragraph, `NOTE: text`.
    Admonition(AdmonitionKind),
    /// A delimited listing block.
    Code,
    Quote,
}

/// A block inserted after the one holding the caret, when the caret is at its end.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NewBlock {
    Paragraph {
        text: String,
    },
    /// A list item after the caret's item, with the same marker.
    ListItem {
        text: String,
    },
    /// An empty code block, the caret inside it.
    Code,
    /// An admonition paragraph with its label selected, ready to be typed over.
    Admonition(AdmonitionKind),
    /// A table with a header row and empty body rows, the first heading selected.
    Table {
        columns: usize,
        rows: usize,
    },
    Image {
        target: String,
        alt: String,
    },
}

fn unsupported(why: &'static str) -> StepError {
    StepError::Unsupported(why)
}

/// The source of a new block, and the part of it selected once it is inserted (the caret where
/// typing goes next, or a placeholder to type over). The visual editor's new blocks and the
/// source editor's insert commands share it, so both write the same source.
/// `text`, which may be pasted, as one paragraph can hold it: no blank line to end it, and no line
/// that would start another block.
fn paragraph_text(text: &str) -> Result<String, StepError> {
    let text = sanitise(text, TextContext::Flow);
    let text = text.trim_matches('\n');
    if text.is_empty() {
        return Err(unsupported("a new block needs text"));
    }
    let lines: Vec<String> = text
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let guard = if starts_block(line, index == 0) {
                "{empty}"
            } else {
                ""
            };
            format!("{guard}{line}")
        })
        .collect();
    Ok(lines.join("\n"))
}

/// `text`, which may be pasted, as the one line of a new list item.
fn list_item_text(text: &str) -> Result<String, StepError> {
    let text = sanitise(text, TextContext::Line);
    let text = text.trim();
    if text.is_empty() {
        return Err(unsupported("a new block needs text"));
    }
    Ok(text.to_owned())
}

pub(crate) fn new_block_body(
    new: &NewBlock,
) -> Result<(String, std::ops::Range<usize>), StepError> {
    Ok(match new {
        NewBlock::Paragraph { text } => {
            let body = paragraph_text(text)?;
            let length = body.len();
            (body, length..length)
        }
        NewBlock::Code => ("----\n\n----".to_owned(), 5..5),
        NewBlock::Admonition(kind) => {
            let label = admonition_label(*kind);
            let placeholder = format!("{}.", kind.label());
            let start = label.len() + 2;
            (
                format!("{label}: {placeholder}"),
                start..start + placeholder.len(),
            )
        }
        NewBlock::Table { columns, rows } => {
            let columns = (*columns).clamp(1, 20);
            let header: Vec<String> = (1..=columns)
                .map(|column| format!("|Column {column}"))
                .collect();
            let mut body = format!("|===\n{}\n", header.join(" "));
            for _ in 0..(*rows).max(1) {
                body.push('\n');
                body.push_str(&Model::empty_row(columns));
            }
            body.push_str("\n|===");
            (body, 6..6 + "Column 1".len())
        }
        NewBlock::Image { target, alt } => {
            let target = target.trim();
            if target.is_empty() || target.contains(char::is_whitespace) {
                return Err(unsupported("an image target has no spaces"));
            }
            let body = format!("image::{target}[{}]", alt.replace(']', "\\]"));
            let length = body.len();
            (body, length..length)
        }
        NewBlock::ListItem { .. } => return Err(unsupported("a list item follows a list item")),
    })
}

fn admonition_label(kind: AdmonitionKind) -> &'static str {
    match kind {
        AdmonitionKind::Note => "NOTE",
        AdmonitionKind::Tip => "TIP",
        AdmonitionKind::Important => "IMPORTANT",
        AdmonitionKind::Caution => "CAUTION",
        AdmonitionKind::Warning => "WARNING",
    }
}

/// A list item marker one level deeper or shallower, or `None` when it cannot change.
fn deeper(marker: &str) -> String {
    match marker.chars().next() {
        Some('*') => format!("{marker}*"),
        Some('.') => format!("{marker}."),
        Some('-') => "**".to_owned(),
        _ => "..".to_owned(),
    }
}

fn shallower(marker: &str) -> Option<String> {
    let first = marker.chars().next()?;
    (marker.len() > 1 && marker.chars().all(|c| c == first) && matches!(first, '*' | '.'))
        .then(|| marker[1..].to_owned())
}

impl Model {
    fn line_start(&self, offset: usize) -> usize {
        self.source[..offset]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.source[offset..]
            .find('\n')
            .map_or(self.source.len(), |index| offset + index)
    }

    /// The editable node whose text block holds `at`.
    fn owner(&self, at: usize) -> Result<(BlockPath, &Node, &TextBlock), StepError> {
        if at > self.source.len() || !self.source.is_char_boundary(at) {
            return Err(StepError::OutOfRange);
        }
        let (path, node, text) = self.text_block_at(at).ok_or(StepError::NotEditable)?;
        if !node.is_editable() {
            return Err(StepError::NotEditable);
        }
        Ok((path, node, text))
    }

    fn parent(&self, path: &[usize]) -> Option<&Node> {
        path.split_last()
            .and_then(|(_, parent)| (!parent.is_empty()).then(|| self.node(parent)).flatten())
    }

    /// A result whose model is rebuilt, with the caret where `caret` goes.
    fn structural(&self, edits: Vec<SourceEdit>, caret: usize) -> Result<StepResult, StepError> {
        Ok(StepResult {
            selection: SourceRange::new(caret, caret),
            edits,
            model: None,
        })
    }

    /// As [`Self::structural`], the caret being `at` before the edits.
    fn moved(&self, mut edits: Vec<SourceEdit>, at: usize) -> Result<StepResult, StepError> {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let caret = Mapping::new(&edits).map(at, Assoc::After);
        self.structural(edits, caret)
    }

    /// The written halves of a text block split at source offset `at`, without the
    /// whitespace at the split.
    fn halves(&self, block: &TextBlock, at: usize) -> Result<(String, String), StepError> {
        let display = block.display_offset(at).ok_or(StepError::OutOfRange)?;
        let flat = flatten(&self.source, block);
        let (units, first, _) = split_display(flat.units, display, display);
        let mut left: Vec<Unit> = units[..first].to_vec();
        let mut right: Vec<Unit> = units[first..].to_vec();
        trim_units(&mut left, true);
        trim_units(&mut right, false);
        Ok((write(&left).text, write(&right).text))
    }

    pub(crate) fn split_block(&self, at: usize) -> Result<StepResult, StepError> {
        let (path, node, block) = self.owner(at)?;
        if block.plain {
            return Err(unsupported("a new line in code is typed as text"));
        }
        let display = block.display_offset(at).ok_or(StepError::OutOfRange)?;
        if display == 0 || display >= block.display.len() {
            return Err(unsupported("a new block goes before or after this one"));
        }
        let in_list = self
            .parent(&path)
            .is_some_and(|parent| matches!(parent.kind, NodeKind::ListItem { .. }));
        let (left, right) = self.halves(block, at)?;
        let start = block.range.start;
        match &node.kind {
            NodeKind::Paragraph | NodeKind::Section { .. } | NodeKind::Heading { .. }
                if !in_list =>
            {
                let guard = if starts_block(right.lines().next().unwrap_or(""), true) {
                    "{empty}"
                } else {
                    ""
                };
                let text = format!("{left}\n\n{guard}{right}");
                let caret = start + left.len() + 2 + guard.len();
                self.structural(vec![SourceEdit::new(block.range, text)], caret)
            }
            NodeKind::ListItem { marker, checked } => {
                let line = self.line_start(node.range.start);
                let indent: String = self.source[line..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                let checkbox = if checked.is_some() { " [ ]" } else { "" };
                let prefix = format!("{indent}{marker}{checkbox} ");
                let text = format!("{left}\n{prefix}{right}");
                let caret = start + left.len() + 1 + prefix.len();
                self.structural(vec![SourceEdit::new(block.range, text)], caret)
            }
            _ => Err(unsupported("this block cannot be split")),
        }
    }

    pub(crate) fn insert_block(&self, at: usize, new: &NewBlock) -> Result<StepResult, StepError> {
        let (path, node, block) = self.owner(at)?;
        match new {
            NewBlock::ListItem { text } => {
                let NodeKind::ListItem { marker, checked } = &node.kind else {
                    return Err(unsupported("a list item follows a list item"));
                };
                let line = self.line_start(node.range.start);
                let indent: String = self.source[line..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                let checkbox = if checked.is_some() { " [ ]" } else { "" };
                let text = list_item_text(text)?;
                let inserted = format!("\n{indent}{marker}{checkbox} {text}");
                let caret = node.range.end + inserted.len();
                self.structural(vec![SourceEdit::insert(node.range.end, inserted)], caret)
            }
            NewBlock::Paragraph { .. }
            | NewBlock::Code
            | NewBlock::Admonition(_)
            | NewBlock::Table { .. }
            | NewBlock::Image { .. } => {
                let (body, select) = new_block_body(new)?;
                if node.kind == NodeKind::Title {
                    // After the header: before the first block of the body.
                    let first = self.blocks.get(1).map(|next| next.range.start);
                    let (at, inserted, base) = match first {
                        Some(first) => (first, format!("{body}\n\n"), first),
                        None => {
                            let end = self.source.trim_end().len();
                            (end, format!("\n\n{body}"), end + 2)
                        }
                    };
                    return self.selected(
                        vec![SourceEdit::insert(at, inserted)],
                        SourceRange::new(base + select.start, base + select.end),
                    );
                }
                let after = match &node.kind {
                    NodeKind::Section { .. } | NodeKind::Heading { .. } => block.range.end,
                    NodeKind::Cell | NodeKind::Term | NodeKind::Description => {
                        return Err(unsupported("a paragraph cannot follow this text"));
                    }
                    _ => self.outermost_list_end(&path).unwrap_or_else(|| {
                        // A paragraph of an admonition paragraph goes after the admonition.
                        match self.parent(&path) {
                            Some(parent)
                                if matches!(
                                    parent.kind,
                                    NodeKind::Admonition {
                                        delimited: false,
                                        ..
                                    }
                                ) =>
                            {
                                parent.range.end
                            }
                            _ => node.range.end,
                        }
                    }),
                };
                let inserted = format!("\n\n{body}");
                let base = after + 2;
                self.selected(
                    vec![SourceEdit::insert(after, inserted)],
                    SourceRange::new(base + select.start, base + select.end),
                )
            }
        }
    }

    /// Whether a list item, or else a paragraph, can go before the block starting at `at`.
    #[must_use]
    pub fn takes_block_before(&self, at: usize, list_item: bool) -> bool {
        let text = "x".to_owned();
        let new = if list_item {
            NewBlock::ListItem { text }
        } else {
            NewBlock::Paragraph { text }
        };
        self.insert_block_before(at, &new).is_ok()
    }

    /// Enter at the start of a block: a paragraph or list item before it, the caret after the
    /// new text. Written before the block's attributes, so they stay with it.
    pub(crate) fn insert_block_before(
        &self,
        at: usize,
        new: &NewBlock,
    ) -> Result<StepResult, StepError> {
        let (path, node, block) = self.owner(at)?;
        if block.plain || block.display_offset(at) != Some(0) {
            return Err(unsupported("a new block goes before the start of a block"));
        }
        match new {
            NewBlock::ListItem { text } => {
                let NodeKind::ListItem { marker, checked } = &node.kind else {
                    return Err(unsupported("a list item goes before a list item"));
                };
                let line = self.line_start(node.range.start);
                let indent: String = self.source[line..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                let checkbox = if checked.is_some() { " [ ]" } else { "" };
                let text = list_item_text(text)?;
                let item = format!("{indent}{marker}{checkbox} {text}");
                let caret = line + item.len();
                self.structural(vec![SourceEdit::insert(line, format!("{item}\n"))], caret)
            }
            NewBlock::Paragraph { .. } => {
                if !matches!(
                    node.kind,
                    NodeKind::Paragraph | NodeKind::Section { .. } | NodeKind::Heading { .. }
                ) {
                    return Err(unsupported("a paragraph cannot go before this text"));
                }
                let parent = self.parent(&path);
                if parent.is_some_and(|parent| matches!(parent.kind, NodeKind::ListItem { .. })) {
                    return Err(unsupported("a paragraph cannot go before this text"));
                }
                let start = match parent {
                    // The paragraph is the whole block, whose marker or attributes come first.
                    Some(parent)
                        if !self.has_opening_fence(parent, node.range.start)
                            && parent.range.start < node.range.start
                            && !matches!(parent.kind, NodeKind::Section { .. }) =>
                    {
                        parent.range.start
                    }
                    _ => node.range.start,
                };
                let (body, select) = new_block_body(new)?;
                self.structural(
                    vec![SourceEdit::insert(start, format!("{body}\n\n"))],
                    start + select.end,
                )
            }
            _ => Err(unsupported("a paragraph or list item goes before a block")),
        }
    }

    /// Whether the line above `child` in `parent` opens a delimited block.
    fn has_opening_fence(&self, parent: &Node, child: usize) -> bool {
        matches!(
            parent.kind,
            NodeKind::Quote
                | NodeKind::Sidebar
                | NodeKind::Example
                | NodeKind::Open
                | NodeKind::Admonition { .. }
        ) && self.source[parent.range.start..child]
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .is_some_and(is_fence)
    }

    /// Deletes `range`, which starts in the text block at `path` and ends in a later one: what
    /// is left of the two joins in the first and the blocks between go. Refused where that would
    /// take something only the source can edit, or the fence of a delimited block.
    pub(crate) fn delete_across(
        &self,
        range: SourceRange,
        path: &[usize],
        first: &TextBlock,
    ) -> Result<StepResult, StepError> {
        let (last_path, last_node, last) = self
            .text_block_at(range.end)
            .ok_or(StepError::NotEditable)?;
        if !last_node.is_editable() {
            return Err(StepError::NotEditable);
        }
        let first_node = self.node(path).ok_or(StepError::NotEditable)?;
        if !joins(first_node, first) || !joins(last_node, last) {
            return Err(unsupported(
                "this selection can only be deleted in the source",
            ));
        }
        if self.fences_around(path) != self.fences_around(&last_path) {
            return Err(unsupported(
                "the selection crosses the edge of a block; delete it in the source",
            ));
        }
        let gap = SourceRange::new(first.range.end, last.range.start);
        let read_only = self.walk().into_iter().any(|(node_path, node)| {
            let shared = node_path.len() < path.len().min(last_path.len())
                && path.starts_with(&node_path)
                && last_path.starts_with(&node_path);
            !shared
                && !node.is_editable()
                && node.range.start < gap.end
                && gap.start < node.range.end
        });
        if read_only || self.comment_in(gap) {
            return Err(unsupported(
                "the selection holds what only the source can edit; delete it there",
            ));
        }

        let start = first
            .display_offset(range.start)
            .ok_or(StepError::OutOfRange)?;
        let end = last
            .display_offset_towards(range.end, true)
            .ok_or(StepError::OutOfRange)?;
        let flat = flatten(&self.source, first);
        let (units, cut, _) = split_display(flat.units, start, first.display.len());
        let mut kept: Vec<Unit> = units[..cut].to_vec();
        let tail = flatten(&self.source, last);
        let (units, _, from) = split_display(tail.units, 0, end);
        let mut after: Vec<Unit> = units[from..].to_vec();
        // Nothing left on one side: the block's text starts or ends here, without the whitespace
        // the page would not keep there, which would leave the caret outside the block.
        if kept.is_empty() {
            trim_units(&mut after, false);
        }
        if after.is_empty() {
            trim_units(&mut kept, true);
        }
        let touched = join_frames(&kept, &mut after, flat.next_id);
        if first.context == TextContext::Line {
            for unit in &mut after {
                if unit.kind == UnitKind::Text {
                    unit.shown = unit.shown.replace('\n', " ");
                    unit.source = unit.source.replace('\n', " ");
                }
            }
        }
        let caret = kept.iter().map(|unit| unit.shown.len()).sum::<usize>();
        kept.append(&mut after);

        let in_list = |node: &Node| matches!(node.kind, NodeKind::ListItem { .. });
        let following = self.source[last.range.end..].lines().nth(1).unwrap_or("");
        if following.trim() == "+" && (kept.is_empty() || !in_list(first_node)) {
            return Err(unsupported(
                "the blocks attached to that list item would be left behind",
            ));
        }
        if kept.is_empty() {
            return self.delete_whole_blocks(path, first_node, last);
        }
        let caret = Caret {
            display: caret,
            sticky: caret > 0,
        };
        let realised = self
            .context(first)
            .realise(kept, &touched, (caret, caret))?;
        let mut text = realised.text;
        // What follows the last block directly stays apart from the joined text.
        let stays_apart = !following.trim().is_empty()
            && !matches!(last_node.kind, NodeKind::Paragraph)
            && !(in_list(first_node) && in_list(last_node));
        if stays_apart {
            text.push('\n');
        }
        let whole = SourceRange::new(first.range.start, last.range.end);
        let old = &self.source[whole.start..whole.end];
        let edits = minimal_edit(whole, old, &text).into_iter().collect();
        self.structural(edits, first.range.start + realised.selection.0)
    }

    /// Whether a comment, which no node holds and the visual editor does not show, lies in `gap`.
    fn comment_in(&self, gap: SourceRange) -> bool {
        let has_comment = |text: &str| text.lines().any(|line| line.trim_start().starts_with("//"));
        let mut from = gap.start;
        for (_, node) in self.walk() {
            if node.children.is_empty() && node.range.start < gap.end && from < node.range.end {
                if from < node.range.start && has_comment(&self.source[from..node.range.start]) {
                    return true;
                }
                from = from.max(node.range.end);
            }
        }
        from < gap.end && has_comment(&self.source[from..gap.end])
    }

    /// The delimited blocks holding the node at `path`, outermost first.
    fn fences_around(&self, path: &[usize]) -> Vec<SourceRange> {
        (1..path.len())
            .filter_map(|length| self.node(&path[..length]))
            .filter(|node| {
                matches!(
                    node.kind,
                    NodeKind::Quote
                        | NodeKind::Sidebar
                        | NodeKind::Example
                        | NodeKind::Open
                        | NodeKind::Table { .. }
                        | NodeKind::DescriptionList
                        | NodeKind::Admonition {
                            delimited: true,
                            ..
                        }
                )
            })
            .map(|node| node.range)
            .collect()
    }

    /// A selection of every character of the blocks from the one at `path` to `last`: they go
    /// with their attributes and markers, and the blank lines after them, and the caret moves
    /// to the next text or else to the end of the text before.
    fn delete_whole_blocks(
        &self,
        path: &[usize],
        first_node: &Node,
        last: &TextBlock,
    ) -> Result<StepResult, StepError> {
        let mut start = match self.parent(path) {
            Some(parent)
                if matches!(
                    parent.kind,
                    NodeKind::Admonition {
                        delimited: false,
                        ..
                    }
                ) =>
            {
                parent.range.start
            }
            _ => self.line_start(first_node.range.start),
        };
        let rest = &self.source[last.range.end..];
        let mut end = last.range.end + (rest.len() - rest.trim_start().len());
        if end == self.source.len() {
            // Nothing follows: the page ends where the text before does, as it did here.
            let before = self.source[..start].trim_end().len();
            if before > 0 {
                end = self.source.len() - usize::from(rest.ends_with('\n'));
                start = before;
            }
        }
        let removed = end - start;
        let next = self
            .text_blocks()
            .find(|(_, _, text)| text.range.start >= end)
            .map(|(_, _, text)| text.range.start - removed);
        let previous = self
            .text_blocks()
            .take_while(|(_, _, text)| text.range.end <= start)
            .last()
            .map(|(_, _, text)| text.range.end);
        let caret = next.or(previous).unwrap_or(start);
        self.structural(
            vec![SourceEdit::delete(SourceRange::new(start, end))],
            caret,
        )
    }

    /// As [`Self::structural`], with a selection.
    fn selected(
        &self,
        edits: Vec<SourceEdit>,
        selection: SourceRange,
    ) -> Result<StepResult, StepError> {
        Ok(StepResult {
            selection,
            edits,
            model: None,
        })
    }

    /// The end of the outermost list holding the node at `path`.
    fn outermost_list_end(&self, path: &[usize]) -> Option<usize> {
        (1..path.len())
            .filter_map(|length| self.node(&path[..length]))
            .find(|node| matches!(node.kind, NodeKind::List { .. }))
            .map(|list| list.range.end)
    }

    pub(crate) fn join_backward(&self, at: usize) -> Result<StepResult, StepError> {
        let (path, node, block) = self.owner(at)?;
        if block.display_offset(at) != Some(0) {
            return Err(unsupported("joining starts at the start of a block"));
        }
        match &node.kind {
            NodeKind::Section { .. } | NodeKind::Heading { .. } => {
                let line = self.line_start(block.range.start);
                self.moved(
                    vec![SourceEdit::delete(SourceRange::new(
                        line,
                        block.range.start,
                    ))],
                    at,
                )
            }
            NodeKind::ListItem { marker, .. } => {
                if shallower(marker).is_some() {
                    return self.outdent(at);
                }
                let index = *path.last().unwrap_or(&0);
                let marker_start = self.line_start(node.range.start);
                let remove = SourceEdit::delete(SourceRange::new(marker_start, block.range.start));
                if index == 0 {
                    return self.lift(&path, node, block, at);
                }
                let list = self.parent(&path).ok_or(StepError::NotEditable)?;
                let previous = &list.children[index - 1];
                if !previous.children.is_empty() {
                    return Err(unsupported("the item before has blocks of its own"));
                }
                self.moved(vec![remove], at)
            }
            NodeKind::Paragraph => {
                let parent = self.parent(&path);
                if let Some(admonition) = parent.filter(|parent| {
                    matches!(
                        parent.kind,
                        NodeKind::Admonition {
                            delimited: false,
                            ..
                        }
                    )
                }) {
                    return self.moved(
                        vec![SourceEdit::delete(SourceRange::new(
                            admonition.range.start,
                            block.range.start,
                        ))],
                        at,
                    );
                }
                let index = *path.last().unwrap_or(&0);
                let siblings = match parent {
                    Some(parent) => &parent.children,
                    None => &self.blocks,
                };
                let previous = index
                    .checked_sub(1)
                    .and_then(|previous| siblings.get(previous))
                    .ok_or(unsupported("there is no paragraph before"))?;
                let previous_text = previous
                    .text
                    .as_ref()
                    .filter(|_| previous.kind == NodeKind::Paragraph && previous.is_editable())
                    .ok_or(unsupported("only paragraphs join"))?;
                if node.range.start != block.range.start {
                    return Err(unsupported("the paragraph has attributes of its own"));
                }
                let gap = SourceRange::new(previous_text.range.end, block.range.start);
                if !self.source[gap.start..gap.end].trim().is_empty() {
                    return Err(unsupported("something lies between the paragraphs"));
                }
                // Only the blank line goes: the paragraph's lines stay as they are.
                let caret = gap.start + 1;
                self.structural(vec![SourceEdit::new(gap, "\n")], caret)
            }
            _ => Err(unsupported("this block does not join the one before")),
        }
    }

    /// The list item at `path` made a paragraph, with blank lines either side.
    fn lift(
        &self,
        path: &[usize],
        item: &Node,
        block: &TextBlock,
        at: usize,
    ) -> Result<StepResult, StepError> {
        let index = *path.last().unwrap_or(&0);
        let list = self.parent(path).ok_or(StepError::NotEditable)?;
        let line = self.line_start(item.range.start);
        let mut edits = Vec::new();
        if index > 0 {
            edits.push(SourceEdit::new(
                SourceRange::new(line, block.range.start),
                "\n",
            ));
        } else {
            edits.push(SourceEdit::delete(SourceRange::new(
                line,
                block.range.start,
            )));
        }
        let follows = index + 1 < list.children.len() || !item.children.is_empty();
        if follows {
            edits.push(SourceEdit::insert(block.range.end, "\n"));
        }
        self.moved(edits, at)
    }

    pub(crate) fn set_block_type(
        &self,
        at: usize,
        kind: BlockType,
    ) -> Result<StepResult, StepError> {
        let (path, node, block) = self.owner(at)?;
        let parent = self.parent(&path);
        let admonition = parent.filter(|parent| {
            matches!(
                parent.kind,
                NodeKind::Admonition {
                    delimited: false,
                    ..
                }
            )
        });
        let text = &self.source[block.range.start..block.range.end];
        match (&node.kind, kind) {
            (NodeKind::Paragraph, _) => {
                let in_section = parent
                    .is_none_or(|parent| matches!(parent.kind, NodeKind::Section { .. }))
                    || admonition.is_some();
                // An admonition paragraph's prefix belongs to the paragraph's type.
                let prefix_start =
                    admonition.map_or(block.range.start, |parent| parent.range.start);
                let whole = SourceRange::new(prefix_start, block.range.end);
                let edit = match kind {
                    BlockType::Paragraph => {
                        SourceEdit::delete(SourceRange::new(prefix_start, block.range.start))
                    }
                    BlockType::Heading(level) => {
                        if !in_section || !(1..=5).contains(&level) {
                            return Err(unsupported("a heading goes in a section"));
                        }
                        let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        let marker = "=".repeat(usize::from(level) + 1);
                        SourceEdit::new(whole, format!("{marker} {line}"))
                    }
                    BlockType::Admonition(kind) => SourceEdit::new(
                        SourceRange::new(prefix_start, block.range.start),
                        format!("{}: ", admonition_label(kind)),
                    ),
                    BlockType::Code => {
                        SourceEdit::new(whole, format!("----\n{}\n----", block.display))
                    }
                    BlockType::Quote => SourceEdit::new(whole, format!("____\n{text}\n____")),
                };
                self.moved(vec![edit], at)
            }
            (NodeKind::Section { .. } | NodeKind::Heading { .. }, BlockType::Paragraph) => {
                let line = self.line_start(block.range.start);
                self.moved(
                    vec![SourceEdit::delete(SourceRange::new(
                        line,
                        block.range.start,
                    ))],
                    at,
                )
            }
            (NodeKind::Section { .. } | NodeKind::Heading { .. }, BlockType::Heading(level)) => {
                let line = self.line_start(block.range.start);
                let marker = "=".repeat(usize::from(level.clamp(1, 5)) + 1);
                self.moved(
                    vec![SourceEdit::new(
                        SourceRange::new(line, block.range.start),
                        format!("{marker} "),
                    )],
                    at,
                )
            }
            (NodeKind::Code { .. } | NodeKind::Literal, BlockType::Paragraph) => {
                let caret = node.range.start;
                self.structural(
                    vec![SourceEdit::new(node.range, block.display.clone())],
                    caret,
                )
            }
            (NodeKind::ListItem { .. }, BlockType::Paragraph) => self.lift(&path, node, block, at),
            _ => Err(unsupported("this block cannot take that type")),
        }
    }

    pub(crate) fn toggle_list(&self, at: usize, kind: ListKind) -> Result<StepResult, StepError> {
        let (path, node, block) = self.owner(at)?;
        match &node.kind {
            NodeKind::Paragraph => {
                let marker = match kind {
                    ListKind::Unordered => "* ",
                    ListKind::Ordered => ". ",
                    ListKind::Checklist => "* [ ] ",
                };
                self.moved(vec![SourceEdit::insert(block.range.start, marker)], at)
            }
            NodeKind::ListItem { checked, .. } => {
                let list = self.parent(&path).ok_or(StepError::NotEditable)?;
                let NodeKind::List { kind: current } = list.kind else {
                    return Err(StepError::NotEditable);
                };
                let current = if checked.is_some() {
                    ListKind::Checklist
                } else {
                    current
                };
                if current == kind {
                    return self.lift(&path, node, block, at);
                }
                // Every item of the list takes the new kind.
                let mut edits = Vec::new();
                for item in &list.children {
                    let NodeKind::ListItem { marker, .. } = &item.kind else {
                        continue;
                    };
                    let Some(text) = &item.text else {
                        continue;
                    };
                    let line = self.line_start(item.range.start);
                    let indent = self.source[line..]
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .count();
                    let depth = marker.len().max(1);
                    let new_marker = match kind {
                        ListKind::Ordered => ".".repeat(depth),
                        ListKind::Unordered | ListKind::Checklist => "*".repeat(depth),
                    };
                    let checkbox = if kind == ListKind::Checklist {
                        " [ ]"
                    } else {
                        ""
                    };
                    edits.push(SourceEdit::new(
                        SourceRange::new(line + indent, text.range.start),
                        format!("{new_marker}{checkbox} "),
                    ));
                }
                self.moved(edits, at)
            }
            _ => Err(unsupported("only paragraphs and list items make lists")),
        }
    }

    /// The marker edits that deepen (or make shallower) `item` and every item nested in it.
    fn remark(&self, item: &Node, deepen: bool) -> Result<Vec<SourceEdit>, StepError> {
        let mut edits = Vec::new();
        let mut stack = vec![item];
        while let Some(node) = stack.pop() {
            if let NodeKind::ListItem { marker, .. } = &node.kind {
                let line = self.line_start(node.range.start);
                let text = &self.source[line..self.line_end(line)];
                let (_, _, _) = list_marker(text).ok_or(StepError::NotEditable)?;
                let indent = text.len() - text.trim_start().len();
                let new = if deepen {
                    deeper(marker)
                } else {
                    shallower(marker).ok_or(unsupported("the item is at the top level"))?
                };
                edits.push(SourceEdit::new(
                    SourceRange::new(line + indent, line + indent + marker.len()),
                    new,
                ));
            }
            stack.extend(node.children.iter());
        }
        Ok(edits)
    }

    pub(crate) fn indent(&self, at: usize) -> Result<StepResult, StepError> {
        let (path, node, _) = self.owner(at)?;
        if !matches!(node.kind, NodeKind::ListItem { .. }) {
            return Err(unsupported("only list items indent"));
        }
        if path.last() == Some(&0) {
            return Err(unsupported("the first item of a list cannot be indented"));
        }
        let edits = self.remark(node, true)?;
        self.moved(edits, at)
    }

    pub(crate) fn outdent(&self, at: usize) -> Result<StepResult, StepError> {
        let (path, node, block) = self.owner(at)?;
        let NodeKind::ListItem { marker, .. } = &node.kind else {
            return Err(unsupported("only list items outdent"));
        };
        if shallower(marker).is_none() {
            return self.lift(&path, node, block, at);
        }
        let edits = self.remark(node, false)?;
        self.moved(edits, at)
    }

    pub(crate) fn toggle_checked(&self, at: usize) -> Result<StepResult, StepError> {
        let (_, node, block) = self.owner(at)?;
        let NodeKind::ListItem {
            checked: Some(checked),
            ..
        } = &node.kind
        else {
            return Err(unsupported("only checklist items are checked"));
        };
        let line = self.line_start(node.range.start);
        let head = &self.source[line..block.range.start];
        let open = head.rfind('[').ok_or(StepError::NotEditable)?;
        let range = SourceRange::new(line + open, line + open + 3);
        let new = if *checked { "[ ]" } else { "[x]" };
        self.moved(vec![SourceEdit::new(range, new)], at)
    }
}

/// Whether the text of `node` can end up in, or be taken into, the block a selection joins.
fn joins(node: &Node, text: &TextBlock) -> bool {
    !text.plain
        && matches!(
            node.kind,
            NodeKind::Paragraph
                | NodeKind::Section { .. }
                | NodeKind::Heading { .. }
                | NodeKind::ListItem { .. }
        )
}

/// Whether `line` is a line that opens or closes a delimited block.
fn is_fence(line: &str) -> bool {
    line == "--"
        || (line.len() >= 4
            && ['=', '*', '_', '-', '.', '+', '/']
                .iter()
                .any(|fence| line.chars().all(|character| character == *fence)))
}

/// Makes the frames of `after` distinct from those of `kept`, whose ids stop before `next_id`,
/// and one with them where the same mark meets at the join. Returns the frames at the join.
fn join_frames(kept: &[Unit], after: &mut [Unit], next_id: u32) -> Vec<u32> {
    let mut same: Vec<(u32, u32)> = Vec::new();
    if let (Some(left), Some(right)) = (kept.last(), after.first()) {
        for (left, right) in left.stack.iter().zip(&right.stack) {
            if left.mark != right.mark
                || left.delimiter != right.delimiter
                || left.prefix != right.prefix
            {
                break;
            }
            same.push((right.id, left.id));
        }
    }
    for unit in after.iter_mut() {
        for frame in &mut unit.stack {
            frame.id = same
                .iter()
                .find(|(right, _)| *right == frame.id)
                .map_or(frame.id + next_id, |(_, left)| *left);
        }
    }
    kept.last()
        .into_iter()
        .chain(after.first())
        .flat_map(|unit| unit.stack.iter().map(|frame| frame.id))
        .collect()
}

/// Drops whitespace at the end (`end`) or start of `units`, where a block is split.
fn trim_units(units: &mut Vec<Unit>, end: bool) {
    loop {
        let index = if end {
            units.len().checked_sub(1)
        } else {
            (!units.is_empty()).then_some(0)
        };
        let Some(index) = index else {
            return;
        };
        let unit = &mut units[index];
        if unit.kind != UnitKind::Text {
            return;
        }
        let trimmed = if end {
            unit.source.trim_end().to_owned()
        } else {
            unit.source.trim_start().to_owned()
        };
        if trimmed.is_empty() {
            units.remove(index);
            continue;
        }
        unit.shown.clone_from(&trimmed);
        unit.source = trimmed;
        return;
    }
}
