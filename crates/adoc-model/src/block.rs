//! Structural steps: splitting and joining blocks, new blocks, block types and lists.
//!
//! Each changes as little source as it can (a prefix, a marker, a blank line) and leaves the
//! model to be rebuilt from the new text, so structure always comes from the parser.

use adoc_core::SourceRange;
use adoc_dom::{AdmonitionKind, ListKind};

use crate::{
    build::list_marker,
    inline::{flatten, starts_block, write, Unit, UnitKind},
    mapping::{Assoc, Mapping},
    step::{split_display, StepError, StepResult},
    tree::{BlockPath, Model, Node, NodeKind, TextBlock},
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
}

fn unsupported(why: &'static str) -> StepError {
    StepError::Unsupported(why)
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
                let inserted = format!("\n{indent}{marker}{checkbox} {text}");
                let caret = node.range.end + inserted.len();
                self.structural(vec![SourceEdit::insert(node.range.end, inserted)], caret)
            }
            NewBlock::Paragraph { .. } | NewBlock::Code | NewBlock::Admonition(_) => {
                let (body, select) = match new {
                    NewBlock::Paragraph { text } => {
                        let guard = if starts_block(text.lines().next().unwrap_or(""), true) {
                            "{empty}"
                        } else {
                            ""
                        };
                        let body = format!("{guard}{text}");
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
                    NewBlock::ListItem { .. } => unreachable!("handled above"),
                };
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
