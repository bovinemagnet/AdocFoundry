//! Taking what lies between two places of a page's text.

use std::ops::Range;

use adoc_dom::{ListKind, SourceSpan};

use super::{
    captioned, content_of, heading, shown_text, verbatim_text, Extent, Flow, Fragment,
    FragmentBlock, FragmentCell, FragmentDescription, FragmentItem, FragmentKind, LeafKey, Part,
    Place,
};
use crate::{DiagramView, ViewBlock, ViewCell, ViewInline, ViewKind, ViewTree};

impl Flow {
    /// What lies between two places of `view`, which this is the flow of. `bullet` gives the
    /// character a list item is written with.
    #[must_use]
    pub fn cut(
        &self,
        view: &ViewTree,
        from: Place,
        to: Place,
        bullet: &dyn Fn(&SourceSpan) -> Option<char>,
    ) -> Fragment {
        let cutter = Cutter {
            flow: self,
            view,
            from,
            to,
            bullet,
        };
        let mut blocks = Vec::new();
        cutter.page(&mut blocks);
        Fragment {
            blocks,
            footnotes: view.footnotes.clone(),
        }
    }
}

/// The number drawn before item `index` of an ordered list: counted from its `start`, down when
/// it is reversed.
#[must_use]
pub fn list_number(block: &ViewBlock, items: usize, index: usize) -> i64 {
    let reversed = block.options.contains("reversed");
    let start = block
        .attributes
        .get("start")
        .and_then(|start| start.trim().parse::<i64>().ok())
        .unwrap_or(if reversed { items as i64 } else { 1 });
    let index = index as i64;
    if reversed {
        start.saturating_sub(index)
    } else {
        start.saturating_add(index)
    }
}

struct Cutter<'a> {
    flow: &'a Flow,
    view: &'a ViewTree,
    from: Place,
    to: Place,
    bullet: &'a dyn Fn(&SourceSpan) -> Option<char>,
}

impl Cutter<'_> {
    /// The selected range of a leaf.
    fn range(&self, trail: &[usize], part: Part) -> Option<Range<usize>> {
        let leaf = self.flow.position(&LeafKey::new(trail, part))?;
        self.flow.selected(leaf, self.from, self.to)
    }

    /// Whether the selection lies in one leaf and leaves some of it out.
    fn inside_one_leaf(&self) -> bool {
        self.from.leaf == self.to.leaf
            && (self.from.offset > 0
                || self.to.offset < self.flow.leaves[self.from.leaf].text.len())
    }

    /// The selected part of a leaf's inlines.
    fn inlines(&self, trail: &[usize], part: Part, content: &[ViewInline]) -> Vec<ViewInline> {
        let Some(range) = self.range(trail, part) else {
            return Vec::new();
        };
        self.own(trail, part, &range).map_or_else(
            || cut_inlines(content, range.clone()),
            |text| {
                let mut out = Vec::new();
                for (index, line) in text.split('\n').enumerate() {
                    if index > 0 {
                        out.push(ViewInline::LineBreak);
                    }
                    out.push(ViewInline::Text(line.to_owned()));
                }
                out
            },
        )
    }

    /// The selected part of a leaf whose text is an editor's own.
    fn own(&self, trail: &[usize], part: Part, range: &Range<usize>) -> Option<String> {
        let leaf = &self.flow.leaves[self.flow.position(&LeafKey::new(trail, part))?];
        leaf.own.then(|| leaf.text[range.clone()].to_owned())
    }

    /// The selected part of a leaf that is text alone.
    fn text(&self, trail: &[usize], part: Part) -> Option<String> {
        let range = self.range(trail, part)?;
        let leaf = self.flow.position(&LeafKey::new(trail, part))?;
        Some(self.flow.leaves[leaf].text[range].to_owned())
    }

    /// Whether the selection takes every leaf of `extent`, or passes over a block with none.
    fn covers(&self, extent: Extent) -> bool {
        match extent
            .end
            .checked_sub(1)
            .filter(|last| *last >= extent.first)
        {
            Some(last) => {
                self.from <= Place::new(extent.first, 0)
                    && self.to >= Place::new(last, self.flow.leaves[last].text.len())
            }
            None => self.from.leaf < extent.first && self.to.leaf >= extent.first,
        }
    }

    /// Whether the selection lies wholly in `extent`.
    fn within(&self, extent: Extent) -> bool {
        self.from.leaf >= extent.first && self.to.leaf < extent.end
    }

    fn page(&self, out: &mut Vec<FragmentBlock>) {
        if let Some(title) = &self.view.title {
            let content = self.inlines(&[], Part::DocumentTitle, title);
            if !content.is_empty() {
                out.push(bare(if self.inside_one_leaf() {
                    FragmentKind::Paragraph(content)
                } else {
                    FragmentKind::Heading {
                        level: 0,
                        discrete: false,
                        content,
                        number: None,
                        style: None,
                    }
                }));
            }
        }
        if let Some(text) = self.text(&[], Part::Contents) {
            out.push(bare(FragmentKind::Text(text)));
        }
        let mut trail = Vec::new();
        for (index, block) in self.view.blocks.iter().enumerate() {
            trail.push(index);
            self.block(block, &mut trail, out);
            trail.pop();
        }
    }

    fn children(
        &self,
        blocks: &[ViewBlock],
        next: &mut usize,
        trail: &mut Vec<usize>,
    ) -> Vec<FragmentBlock> {
        let mut out = Vec::new();
        for block in blocks {
            trail.push(*next);
            self.block(block, trail, &mut out);
            trail.pop();
            *next += 1;
        }
        out
    }

    /// The part of `block` the selection covers.
    fn block(&self, block: &ViewBlock, trail: &mut Vec<usize>, out: &mut Vec<FragmentBlock>) {
        let Some(extent) = self.flow.extents.get(trail.as_slice()).copied() else {
            return;
        };
        let untouched = extent.first >= extent.end && !self.covers(extent)
            || extent.first < extent.end
                && (self.to.leaf < extent.first || self.from.leaf >= extent.end);
        if untouched {
            return;
        }
        let whole = self.covers(extent);
        if let ViewKind::Section {
            level,
            number,
            title,
            blocks,
        } = &block.kind
        {
            self.section(block, *level, number.as_deref(), title, trail, out);
            out.extend(self.children(blocks, &mut 0, trail));
            return;
        }
        let title = self.inlines(trail, Part::Title, &captioned(block));
        let kinds = self.kind(block, trail, whole, extent);
        let mut kinds = kinds.into_iter();
        // A block's title goes with the block when any of the block is taken with it.
        match kinds.next() {
            Some(first) if title.is_empty() => out.push(FragmentBlock {
                whole: whole.then(|| block.span.clone()),
                ..first
            }),
            Some(first) => {
                // The caption is kept apart, as it is counted where the block goes.
                let caption = block.caption.as_ref().filter(|_| {
                    self.range(trail, Part::Title)
                        .is_some_and(|range| range.start == 0)
                });
                let title = match caption {
                    Some(caption) => cut_inlines(&title, caption.len() + 1..usize::MAX),
                    None => title,
                };
                out.push(FragmentBlock {
                    caption: caption.cloned(),
                    title,
                    whole: whole.then(|| block.span.clone()),
                    ..first
                });
            }
            None if !title.is_empty() => out.push(bare(FragmentKind::Paragraph(title))),
            None => {}
        }
        out.extend(kinds);
    }

    fn is_own(&self, trail: &[usize], part: Part) -> bool {
        self.flow
            .position(&LeafKey::new(trail, part))
            .is_some_and(|leaf| self.flow.leaves[leaf].own)
    }

    fn section(
        &self,
        block: &ViewBlock,
        level: u8,
        number: Option<&str>,
        title: &[ViewInline],
        trail: &[usize],
        out: &mut Vec<FragmentBlock>,
    ) {
        let own = self.is_own(trail, Part::Heading);
        // An editor's own text has no number.
        let drawn = heading(number.filter(|_| !own), title);
        let content = self.inlines(trail, Part::Heading, &drawn);
        if content.is_empty() {
            return;
        }
        if self.inside_one_leaf() {
            out.push(bare(FragmentKind::Paragraph(content)));
            return;
        }
        let prefix = number.filter(|_| !own).map_or(0, |number| number.len() + 1);
        let whole_start = self
            .range(trail, Part::Heading)
            .is_some_and(|range| range.start == 0);
        let (content, number) = if whole_start && prefix > 0 {
            (
                cut_inlines(&content, prefix..usize::MAX),
                number.map(str::to_owned),
            )
        } else {
            (content, None)
        };
        let style = block.style.clone();
        out.push(bare(FragmentKind::Heading {
            level,
            discrete: false,
            content,
            number,
            style,
        }));
    }

    /// The selected part of `block` itself, usually one block; none when only its title is
    /// selected.
    fn kind(
        &self,
        block: &ViewBlock,
        trail: &mut Vec<usize>,
        whole: bool,
        extent: Extent,
    ) -> Vec<FragmentBlock> {
        // What is inside a container is taken alone when the selection is.
        let inside = self.within(extent) && !whole;
        let wrapped = |blocks: Vec<FragmentBlock>,
                       wrap: &dyn Fn(Vec<FragmentBlock>) -> FragmentKind| {
            if inside || blocks.is_empty() {
                blocks
            } else {
                vec![bare(wrap(blocks))]
            }
        };
        match &block.kind {
            ViewKind::Section { .. } => Vec::new(),
            ViewKind::Heading { level, title } => {
                let content = self.inlines(trail, Part::Heading, title);
                some(content, |content| {
                    if self.inside_one_leaf() {
                        FragmentKind::Paragraph(content)
                    } else {
                        FragmentKind::Heading {
                            level: *level,
                            discrete: true,
                            content,
                            number: None,
                            style: None,
                        }
                    }
                })
            }
            ViewKind::Paragraph(content) => some(
                self.inlines(trail, Part::Body, content),
                FragmentKind::Paragraph,
            ),
            ViewKind::List { kind, items } => self.list(block, *kind, items, trail),
            ViewKind::DescriptionList(items) => {
                let mut next = 0;
                let mut out: Vec<FragmentDescription> = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let term = self.inlines(trail, Part::Term(index), &item.term);
                    let content = self.inlines(trail, Part::Description(index), &item.content);
                    let blocks = self.children(&item.blocks, &mut next, trail);
                    if !(term.is_empty() && content.is_empty() && blocks.is_empty()) {
                        out.push(FragmentDescription {
                            term,
                            content,
                            blocks,
                        });
                    }
                }
                // One text alone, or only what is inside an item, is not a list.
                match out.as_mut_slice() {
                    [] => Vec::new(),
                    [only]
                        if only.term.is_empty()
                            || only.content.is_empty() && only.blocks.is_empty() =>
                    {
                        let mut blocks = Vec::new();
                        for content in [&mut only.term, &mut only.content] {
                            if !content.is_empty() {
                                blocks.push(bare(FragmentKind::Paragraph(std::mem::take(content))));
                            }
                        }
                        blocks.append(&mut only.blocks);
                        blocks
                    }
                    _ => vec![bare(FragmentKind::Descriptions(out))],
                }
            }
            ViewKind::CalloutList(items) => {
                let mut next = 0;
                let mut out = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let part = Part::Callout(index);
                    let content = self.inlines(trail, part, &item.content);
                    let blocks = self.children(&item.blocks, &mut next, trail);
                    if !(content.is_empty() && blocks.is_empty()) {
                        out.push(FragmentItem {
                            marked: self.marked(trail, part),
                            bullet: '*',
                            number: item.number as i64,
                            checked: None,
                            content,
                            blocks,
                        });
                    }
                }
                self.items(out, FragmentKind::Callouts)
            }
            ViewKind::Admonition { kind, blocks } => {
                let label = self.range(trail, Part::Label).is_some();
                let taken = self.children(blocks, &mut 0, trail);
                if taken.is_empty() && label {
                    return vec![bare(FragmentKind::Text(kind.label().to_owned()))];
                }
                wrapped(taken, &|blocks| FragmentKind::Admonition {
                    kind: *kind,
                    label,
                    blocks,
                })
            }
            ViewKind::Code {
                language, callouts, ..
            } => {
                let Some(range) = self.range(trail, Part::Body) else {
                    return Vec::new();
                };
                let Some(text) = self.text(trail, Part::Body) else {
                    return Vec::new();
                };
                let taken = if self.is_own(trail, Part::Body) {
                    // An editor's own text holds its callouts as they are written.
                    Vec::new()
                } else {
                    let full = verbatim_text(&block.kind).unwrap_or_default();
                    taken_callouts(full, &range, callouts)
                };
                vec![bare(FragmentKind::Code {
                    language: language.clone(),
                    text,
                    callouts: taken,
                })]
            }
            ViewKind::Literal(_) => self.verbatim(trail, FragmentKind::Literal),
            ViewKind::Passthrough(_) => self.verbatim(trail, FragmentKind::Passthrough),
            ViewKind::Stem { latex, .. } => self.verbatim(trail, |source| FragmentKind::Stem {
                latex: *latex,
                source,
            }),
            ViewKind::Unsupported { .. } => self.verbatim(trail, FragmentKind::Text),
            ViewKind::Diagram(DiagramView::Svg(_)) => {
                if whole {
                    vec![bare(FragmentKind::Diagram(None))]
                } else {
                    Vec::new()
                }
            }
            ViewKind::Diagram(
                DiagramView::Pending { kind, .. } | DiagramView::Failed { kind, .. },
            ) => self.verbatim(trail, |source| {
                FragmentKind::Diagram(Some((kind.clone(), source)))
            }),
            ViewKind::Quote {
                attribution,
                citation,
                blocks,
            } => {
                let taken = self.children(blocks, &mut 0, trail);
                let line = self.text(trail, Part::Attribution);
                if taken.is_empty() {
                    return line
                        .map(|line| bare(FragmentKind::Text(line)))
                        .into_iter()
                        .collect();
                }
                wrapped(taken, &|blocks| FragmentKind::Quote {
                    attribution: attribution.clone().filter(|_| line.is_some()),
                    citation: citation.clone().filter(|_| line.is_some()),
                    blocks,
                })
            }
            ViewKind::Verse {
                attribution,
                citation,
                ..
            } => {
                let line = self.text(trail, Part::Attribution);
                match self.text(trail, Part::Body) {
                    Some(text) => vec![bare(FragmentKind::Verse {
                        attribution: attribution.clone().filter(|_| line.is_some()),
                        citation: citation.clone().filter(|_| line.is_some()),
                        text,
                    })],
                    None => line
                        .map(|line| bare(FragmentKind::Text(line)))
                        .into_iter()
                        .collect(),
                }
            }
            ViewKind::Sidebar(blocks) => {
                wrapped(self.children(blocks, &mut 0, trail), &FragmentKind::Sidebar)
            }
            ViewKind::Example(blocks) => {
                wrapped(self.children(blocks, &mut 0, trail), &FragmentKind::Example)
            }
            ViewKind::Open(blocks) => {
                wrapped(self.children(blocks, &mut 0, trail), &FragmentKind::Open)
            }
            ViewKind::Table(table) => {
                let mut next = 0;
                let header = table.header.is_some();
                let rows: Vec<Vec<FragmentCell>> = table
                    .header
                    .iter()
                    .chain(&table.rows)
                    .chain(&table.footer)
                    .map(|row| self.row(row, &mut next, trail))
                    .collect();
                let header = header && rows.first().is_some_and(|row| has_text(row));
                let mut rows: Vec<_> = rows.into_iter().filter(|row| has_text(row)).collect();
                let mut filled = rows
                    .iter_mut()
                    .flatten()
                    .filter(|cell| !cell.blocks.is_empty());
                match (filled.next(), filled.next()) {
                    (None, _) => Vec::new(),
                    // One cell's text is not a table.
                    (Some(cell), None) if !whole => std::mem::take(&mut cell.blocks),
                    _ => vec![bare(FragmentKind::Table { header, rows })],
                }
            }
            ViewKind::Image(image) => self
                .range(trail, Part::Body)
                .map(|_| bare(FragmentKind::Image(image.clone())))
                .or_else(|| {
                    (image.alt.is_empty() && whole)
                        .then(|| bare(FragmentKind::Image(image.clone())))
                })
                .into_iter()
                .collect(),
            ViewKind::Media { kind, targets } => vec![bare(FragmentKind::Media {
                kind: *kind,
                targets: targets.clone(),
            })],
            ViewKind::ThematicBreak => vec![bare(FragmentKind::Rule)],
            ViewKind::PageBreak => vec![bare(FragmentKind::PageBreak)],
            ViewKind::Toc => self
                .text(trail, Part::Contents)
                .map(|text| bare(FragmentKind::Text(text)))
                .into_iter()
                .collect(),
        }
    }

    fn verbatim(
        &self,
        trail: &[usize],
        kind: impl FnOnce(String) -> FragmentKind,
    ) -> Vec<FragmentBlock> {
        self.text(trail, Part::Body)
            .map(|text| bare(kind(text)))
            .into_iter()
            .collect()
    }

    fn row(&self, row: &[ViewCell], next: &mut usize, trail: &mut Vec<usize>) -> Vec<FragmentCell> {
        row.iter()
            .map(|cell| FragmentCell {
                blocks: self.children(&cell.blocks, next, trail),
                colspan: cell.colspan.max(1),
                rowspan: cell.rowspan.max(1),
            })
            .collect()
    }

    /// Whether an item's marker is copied with its text: the selection takes it from its start,
    /// and is not part of this one text alone.
    fn marked(&self, trail: &[usize], part: Part) -> bool {
        self.range(trail, part)
            .is_some_and(|range| range.start == 0 && !self.inside_one_leaf())
    }

    fn list(
        &self,
        block: &ViewBlock,
        kind: ListKind,
        items: &[crate::ViewListItem],
        trail: &mut Vec<usize>,
    ) -> Vec<FragmentBlock> {
        let mut next = 0;
        let mut out = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let part = Part::Item(index);
            let content = self.inlines(trail, part, &item.content);
            let blocks = self.children(&item.blocks, &mut next, trail);
            if content.is_empty() && blocks.is_empty() {
                continue;
            }
            out.push(FragmentItem {
                marked: self.marked(trail, part)
                    || item.content.is_empty()
                        && blocks.first().is_some_and(|block| block.whole.is_some()),
                bullet: (self.bullet)(&item.span).unwrap_or('*'),
                number: list_number(block, items.len(), index),
                checked: item.checked,
                content,
                blocks,
            });
        }
        let reversed = block.options.contains("reversed");
        self.items(out, |items| FragmentKind::List {
            kind,
            items,
            reversed,
        })
    }

    /// The items taken of a list, as a list, unless they are one text's part alone or only what
    /// is inside an item.
    fn items(
        &self,
        mut items: Vec<FragmentItem>,
        list: impl FnOnce(Vec<FragmentItem>) -> FragmentKind,
    ) -> Vec<FragmentBlock> {
        if items
            .iter()
            .all(|item| item.content.is_empty() && !item.marked)
        {
            return items.into_iter().flat_map(|item| item.blocks).collect();
        }
        if let [only] = items.as_mut_slice() {
            if self.inside_one_leaf() {
                return vec![bare(FragmentKind::Paragraph(std::mem::take(
                    &mut only.content,
                )))];
            }
        }
        vec![bare(list(items))]
    }
}

fn bare(kind: FragmentKind) -> FragmentBlock {
    FragmentBlock {
        caption: None,
        title: Vec::new(),
        kind,
        whole: None,
    }
}

fn some(
    content: Vec<ViewInline>,
    kind: impl FnOnce(Vec<ViewInline>) -> FragmentKind,
) -> Vec<FragmentBlock> {
    if content.is_empty() {
        Vec::new()
    } else {
        vec![bare(kind(content))]
    }
}

fn has_text(row: &[FragmentCell]) -> bool {
    row.iter().any(|cell| !cell.blocks.is_empty())
}

/// The callouts of `code` whose lines' ends are in `range`, by line within it.
fn taken_callouts(
    code: &str,
    range: &Range<usize>,
    callouts: &[adoc_dom::Callout],
) -> Vec<(usize, usize)> {
    let first_line = code[..range.start].matches('\n').count();
    let mut end = 0;
    let ends: Vec<usize> = code
        .split('\n')
        .map(|line| {
            let at = end + line.len();
            end = at + 1;
            at
        })
        .collect();
    callouts
        .iter()
        .filter(|callout| {
            ends.get(callout.line)
                .is_some_and(|end| range.start <= *end && *end <= range.end)
        })
        .map(|callout| (callout.line - first_line, callout.number))
        .collect()
}

/// The part of `inlines` in `range` of their shown text: formatting is kept around what is left
/// of it, and what cannot be split is taken whole.
#[must_use]
pub(crate) fn cut_inlines(inlines: &[ViewInline], range: Range<usize>) -> Vec<ViewInline> {
    fn visit(
        inlines: &[ViewInline],
        range: &Range<usize>,
        at: &mut usize,
        out: &mut Vec<ViewInline>,
    ) {
        for inline in inlines {
            let start = *at;
            if let Some(content) = content_of(inline) {
                let mut kept = Vec::new();
                visit(content, range, at, &mut kept);
                if !kept.is_empty() {
                    out.push(with_content(inline, kept));
                }
                continue;
            }
            *at += shown_text(std::slice::from_ref(inline)).len();
            match inline {
                ViewInline::Text(text) | ViewInline::Raw(text) => {
                    let from = range.start.saturating_sub(start).min(text.len());
                    let to = range.end.saturating_sub(start).min(text.len());
                    let (from, to) = (text.floor_char_boundary(from), text.floor_char_boundary(to));
                    if from < to {
                        let part = text[from..to].to_owned();
                        out.push(match inline {
                            ViewInline::Raw(_) => ViewInline::Raw(part),
                            _ => ViewInline::Text(part),
                        });
                    }
                }
                _ if range.start < *at && range.end > start => out.push(inline.clone()),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    visit(inlines, &range, &mut 0, &mut out);
    out
}

/// `inline`, formatting around other content, around `content` instead.
fn with_content(inline: &ViewInline, content: Vec<ViewInline>) -> ViewInline {
    let mut copy = inline.clone();
    match &mut copy {
        ViewInline::Strong(inside)
        | ViewInline::Emphasis(inside)
        | ViewInline::Code(inside)
        | ViewInline::Mark(inside)
        | ViewInline::Superscript(inside)
        | ViewInline::Subscript(inside)
        | ViewInline::Span {
            content: inside, ..
        }
        | ViewInline::Link {
            content: inside, ..
        }
        | ViewInline::Xref {
            content: inside, ..
        }
        | ViewInline::IndexTerm {
            content: inside, ..
        } => *inside = content,
        _ => {}
    }
    copy
}
