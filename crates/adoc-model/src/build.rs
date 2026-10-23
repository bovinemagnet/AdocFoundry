//! Building the model from the DOM and the page text.
//!
//! The DOM gives the block structure and where each block starts; the model finds the exact
//! source of every block and text block itself, reads the text with the scanner, and checks the
//! result against the DOM's inlines. Anything it cannot place with certainty is read-only.

use std::{
    cell::Cell,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use adoc_core::SourceRange;
use adoc_dom::{Block, BlockKind, CellStyle, Dom, Inline, ListKind};

use crate::{
    scan::{push_plain, Scanner},
    tree::{
        Access, Atom, Mark, MarkSet, Model, Node, NodeKind, Piece, ReadOnly, RowSection, TextBlock,
        TextContext,
    },
};

impl Model {
    /// Builds the model of the page at `path` with text `source`, parsed as `dom`. Never panics:
    /// half-typed input is normal, and what cannot be placed is read-only.
    #[must_use]
    pub fn build(path: &Path, source: &str, dom: &Dom) -> Model {
        let builder = Builder {
            page: path,
            source,
            attributes: &dom.attributes,
            footnotes: Cell::new(0),
        };
        let mut blocks = Vec::new();
        let mut body_start = 0;
        if let Some((title, header_end)) = builder.title(dom) {
            blocks.push(title);
            body_start = header_end;
        }
        let top: Vec<(&Block, Vec<usize>)> = dom
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block, vec![index]))
            .collect();
        blocks.extend(builder.children(&top, SourceRange::new(body_start, source.len())));
        Model {
            path: path.to_path_buf(),
            source: source.to_owned(),
            attributes: dom.attributes.clone(),
            blocks,
        }
    }
}

pub(crate) struct Builder<'a> {
    pub(crate) page: &'a Path,
    pub(crate) source: &'a str,
    pub(crate) attributes: &'a BTreeMap<String, String>,
    /// Footnotes numbered so far, for their shown markers.
    pub(crate) footnotes: Cell<u32>,
}

/// A line of the source: its range without the newline.
#[derive(Clone, Copy, Debug)]
struct Line {
    start: usize,
    end: usize,
}

fn node(kind: NodeKind, range: SourceRange) -> Node {
    Node {
        kind,
        range,
        access: Access::Editable,
        text: None,
        children: Vec::new(),
        origin: None,
    }
}

fn read_only(kind: NodeKind, range: SourceRange, why: ReadOnly) -> Node {
    Node {
        access: Access::ReadOnly(why),
        ..node(kind, range)
    }
}

/// A delimiter line: four or more of one character (two for an open block).
fn delimiter_of(line: &str) -> Option<&str> {
    let line = line.trim_end();
    if line == "--" || line.starts_with("```") {
        return Some(line);
    }
    // Tables: `|===`, and `,===`, `:===` and `!===` for the other formats.
    if line.len() >= 4
        && line.starts_with(['|', ',', ':', '!'])
        && line[1..].chars().all(|c| c == '=')
    {
        return Some(line);
    }
    let first = line.chars().next()?;
    (matches!(first, '-' | '.' | '=' | '*' | '_' | '+' | '/')
        && line.len() >= 4
        && line.chars().all(|c| c == first))
    .then_some(line)
}

/// A line that gives the next block metadata: `[attributes]`, `[[anchor]]` or `.Title`.
fn is_metadata(line: &str) -> bool {
    let line = line.trim_end();
    (line.starts_with('[') && line.ends_with(']'))
        || (line.starts_with('.')
            && line.len() > 1
            && !line.starts_with("..")
            && !line[1..].starts_with(char::is_whitespace))
}

impl<'a> Builder<'a> {
    fn text(&self, range: SourceRange) -> &'a str {
        self.source.get(range.start..range.end).unwrap_or("")
    }

    fn lines(&self, range: SourceRange) -> Vec<Line> {
        let mut out = Vec::new();
        let mut start = range.start;
        while start < range.end {
            let end = self.source[start..range.end]
                .find('\n')
                .map_or(range.end, |offset| start + offset);
            out.push(Line { start, end });
            start = end + 1;
        }
        out
    }

    fn line_text(&self, line: Line) -> &'a str {
        &self.source[line.start..line.end]
    }

    /// `range` without trailing whitespace.
    fn trim(&self, range: SourceRange) -> SourceRange {
        let text = self.text(range);
        let start = range.start + (text.len() - text.trim_start().len());
        let end = range.start + text.trim_end().len();
        SourceRange::new(start.min(end), end)
    }

    fn trim_end(&self, range: SourceRange) -> SourceRange {
        let text = self.text(range);
        SourceRange::new(range.start, range.start + text.trim_end().len())
    }

    fn is_from_page(&self, block: &Block) -> bool {
        block.meta.span.path == self.page && block.meta.span.include_chain.is_empty()
    }

    /// The document title and where the header ends.
    fn title(&self, dom: &Dom) -> Option<(Node, usize)> {
        dom.title.as_ref()?;
        let all = self.lines(SourceRange::new(0, self.source.len()));
        let index = all.iter().position(|line| {
            let text = self.line_text(*line);
            !text.trim().is_empty() && !text.starts_with("//")
        })?;
        let line = all[index];
        let text = self.line_text(line);
        let title = text.strip_prefix("= ")?;
        let start = line.start + 2 + (title.len() - title.trim_start().len());
        let content = self.trim_end(SourceRange::new(start, line.end));
        let header_end = all[index..]
            .iter()
            .find(|line| self.line_text(**line).trim().is_empty())
            .map_or(self.source.len(), |line| line.start);
        let mut title_node = node(NodeKind::Title, SourceRange::new(line.start, line.end));
        self.set_text(
            &mut title_node,
            content,
            TextContext::Line,
            dom.title.as_deref(),
        );
        Some((title_node, header_end))
    }

    /// Reads `content` into the node's text block, made read-only when it disagrees with
    /// `dom`.
    fn set_text(
        &self,
        node: &mut Node,
        content: SourceRange,
        context: TextContext,
        dom: Option<&[Inline]>,
    ) {
        let block = self.text_block(content, context);
        if let Some(dom) = dom {
            if !agrees(self.source, &block.pieces, dom) && node.is_editable() {
                node.access = Access::ReadOnly(ReadOnly::Unchecked);
            }
        }
        node.text = Some(block);
    }

    pub(crate) fn text_block(&self, range: SourceRange, context: TextContext) -> TextBlock {
        let plain = context == TextContext::Verbatim;
        let mut pieces = if plain || range.is_empty() {
            if range.is_empty() {
                Vec::new()
            } else {
                vec![Piece::Text { range }]
            }
        } else {
            Scanner::new(self.source, self.attributes, context == TextContext::Cell).scan(range)
        };
        self.number_footnotes(&mut pieces);
        let mut display = String::new();
        push_plain(self.source, &pieces, &mut display);
        TextBlock {
            range,
            pieces,
            plain,
            display,
            context,
        }
    }

    fn number_footnotes(&self, pieces: &mut [Piece]) {
        for piece in pieces {
            match piece {
                Piece::Atom {
                    atom: Atom::Footnote { .. },
                    shown,
                    ..
                } => {
                    let number = self.footnotes.get() + 1;
                    self.footnotes.set(number);
                    *shown = format!("[{number}]");
                }
                Piece::Mark { children, .. } => self.number_footnotes(children),
                _ => {}
            }
        }
    }

    /// Nodes for sibling DOM blocks lying in `region`, with include groups and the source
    /// between blocks (conditionals, attribute entries) as read-only nodes.
    fn children(&self, blocks: &[(&Block, Vec<usize>)], region: SourceRange) -> Vec<Node> {
        let nodes = self.nodes(blocks, region);
        self.with_gaps(nodes, region)
    }

    /// Nodes for sibling DOM blocks lying in `region`, with include groups.
    fn nodes(&self, blocks: &[(&Block, Vec<usize>)], region: SourceRange) -> Vec<Node> {
        // Page blocks, and runs of included blocks under their outermost include target.
        enum Pending<'b> {
            Page(&'b Block, Vec<usize>),
            Included(String, Vec<(&'b Block, Vec<usize>)>),
        }
        let mut pending: Vec<Pending<'_>> = Vec::new();
        for (block, trail) in blocks {
            if self.is_from_page(block) {
                pending.push(Pending::Page(block, trail.clone()));
                continue;
            }
            let target = block
                .meta
                .span
                .include_chain
                .first()
                .cloned()
                .unwrap_or_default();
            match pending.last_mut() {
                Some(Pending::Included(previous, group)) if *previous == target => {
                    group.push((block, trail.clone()));
                }
                _ => pending.push(Pending::Included(target, vec![(block, trail.clone())])),
            }
        }
        let starts: Vec<Option<usize>> = pending
            .iter()
            .map(|item| match item {
                Pending::Page(block, _) => Some(block.meta.span.range.start),
                Pending::Included(..) => None,
            })
            .collect();
        let mut nodes: Vec<Node> = Vec::new();
        let mut cursor = region.start;
        for (index, item) in pending.iter().enumerate() {
            let limit = starts[index + 1..]
                .iter()
                .flatten()
                .next()
                .copied()
                .unwrap_or(region.end)
                .clamp(cursor, region.end);
            let built = match item {
                Pending::Page(block, trail) => {
                    let start = block.meta.span.range.start.clamp(cursor, limit);
                    self.block(block, trail.clone(), start, limit)
                }
                Pending::Included(target, group) => {
                    self.include(target, group, SourceRange::new(cursor, limit))
                }
            };
            cursor = built.range.end.max(cursor);
            nodes.push(built);
        }
        nodes
    }

    /// An include directive and the blocks it brought in, all read-only.
    fn include(&self, target: &str, group: &[(&Block, Vec<usize>)], search: SourceRange) -> Node {
        let directive = self.lines(search).into_iter().find(|line| {
            self.line_text(*line)
                .strip_prefix("include::")
                .and_then(|rest| rest.split_once('['))
                .is_some_and(|(written, _)| written == target)
        });
        let range = directive.map_or(SourceRange::new(search.start, search.start), |line| {
            SourceRange::new(line.start, line.end)
        });
        let path = group
            .first()
            .map_or_else(PathBuf::new, |(block, _)| block.meta.span.path.clone());
        let why = ReadOnly::Included {
            path,
            target: target.to_owned(),
        };
        let mut include = read_only(
            NodeKind::Include {
                target: target.to_owned(),
            },
            range,
            why.clone(),
        );
        include.children = group
            .iter()
            .map(|(block, trail)| {
                let mut child = read_only(
                    NodeKind::Verbatim {
                        name: block.kind_name().to_owned(),
                    },
                    range,
                    why.clone(),
                );
                child.origin = Some(trail.clone());
                child
            })
            .collect();
        include
    }

    /// `nodes` with read-only nodes for what lies between them in `region`: conditional
    /// directives (and inactive branches) and attribute entries. Comments are left out.
    fn with_gaps(&self, nodes: Vec<Node>, region: SourceRange) -> Vec<Node> {
        let mut out = Vec::new();
        let mut cursor = region.start;
        for node in nodes {
            if node.range.start > cursor {
                out.extend(self.gap(SourceRange::new(cursor, node.range.start)));
            }
            cursor = cursor.max(node.range.end);
            out.push(node);
        }
        if region.end > cursor {
            out.extend(self.gap(SourceRange::new(cursor, region.end)));
        }
        out
    }

    fn gap(&self, range: SourceRange) -> Vec<Node> {
        let mut runs: Vec<Vec<Line>> = Vec::new();
        let mut current: Vec<Line> = Vec::new();
        let mut in_comment = false;
        for line in self.lines(range) {
            let text = self.line_text(line).trim_end();
            if text.starts_with("////") && text.chars().all(|c| c == '/') {
                in_comment = !in_comment;
                continue;
            }
            // `+` joins a list item to the block after it.
            if in_comment || text.starts_with("//") || text.trim().is_empty() || text == "+" {
                if !current.is_empty() {
                    runs.push(std::mem::take(&mut current));
                }
                continue;
            }
            current.push(line);
        }
        if !current.is_empty() {
            runs.push(current);
        }
        runs.into_iter()
            .map(|run| {
                let range = SourceRange::new(run[0].start, run[run.len() - 1].end);
                let texts: Vec<&str> = run.iter().map(|line| self.line_text(*line)).collect();
                let directive = texts.iter().find(|text| {
                    ["ifdef::", "ifndef::", "ifeval::", "endif::"]
                        .iter()
                        .any(|prefix| text.starts_with(prefix))
                });
                let kind = match directive {
                    Some(directive) => NodeKind::Conditional {
                        directive: (*directive).to_owned(),
                    },
                    None if texts.iter().all(|text| is_attribute_entry(text)) => {
                        NodeKind::Verbatim {
                            name: "attribute".to_owned(),
                        }
                    }
                    None => NodeKind::Verbatim {
                        name: "source".to_owned(),
                    },
                };
                let why = if matches!(kind, NodeKind::Conditional { .. }) {
                    ReadOnly::Inactive
                } else {
                    ReadOnly::Verbatim
                };
                read_only(kind, range, why)
            })
            .collect()
    }

    /// The first line of `range` that is not block metadata.
    fn after_metadata(&self, range: SourceRange) -> usize {
        for line in self.lines(range) {
            if !is_metadata(self.line_text(line)) {
                return line.start;
            }
        }
        range.end
    }

    /// `start` moved past whitespace, and `end` to the end of its line: parser spans can be a
    /// byte short at either end.
    fn snap(&self, start: usize, end: usize, limit: usize) -> SourceRange {
        let text = self.text(SourceRange::new(start, limit));
        let start = start + (text.len() - text.trim_start().len());
        let end = end.clamp(start, limit);
        let line_end = self.source[end..limit]
            .find('\n')
            .map_or(limit, |offset| end + offset);
        let end = if end > start && !self.source[end..line_end].trim().is_empty() {
            line_end
        } else {
            end
        };
        self.trim_end(SourceRange::new(start, end))
    }

    /// A DOM block from the page, starting at `start` and lying before `limit`.
    fn block(&self, block: &Block, trail: Vec<usize>, start: usize, limit: usize) -> Node {
        let span = block.meta.span.range;
        let range = self.snap(start, span.end, limit);
        let start = range.start;
        let mut built = match &block.kind {
            BlockKind::Section {
                level,
                title,
                blocks,
            } => self.section(*level, title, blocks, &trail, start, limit),
            BlockKind::Heading { level, title } => {
                let mut heading = node(NodeKind::Heading { level: *level }, range);
                match self.heading_line(range, *level) {
                    Some(content) => {
                        self.set_text(&mut heading, content, TextContext::Line, Some(title));
                    }
                    None => heading.access = Access::ReadOnly(ReadOnly::Unchecked),
                }
                heading
            }
            BlockKind::Paragraph { inlines } => self.paragraph(range, inlines),
            BlockKind::List { kind, items } => self.list(*kind, items, &trail, range),
            BlockKind::DescriptionList { items } => self.description_list(items, &trail, range),
            BlockKind::Admonition { kind, blocks } => {
                let delimited = self.delimiters(range).is_some();
                let mut admonition = node(
                    NodeKind::Admonition {
                        kind: *kind,
                        delimited,
                    },
                    range,
                );
                admonition.children = self.contained(blocks, &trail, range);
                admonition
            }
            BlockKind::Listing { language, .. } => match self.delimiters(range) {
                Some((content, delimiter)) if delimiter.starts_with('-') => {
                    let mut code = node(
                        NodeKind::Code {
                            language: language.clone(),
                        },
                        range,
                    );
                    self.set_text(&mut code, content, TextContext::Verbatim, None);
                    if block.meta.attributes.contains_key("subs") {
                        code.access = Access::ReadOnly(ReadOnly::Verbatim);
                    }
                    code
                }
                _ => verbatim(block, range),
            },
            BlockKind::Literal { .. } => match self.delimiters(range) {
                Some((content, delimiter)) if delimiter.starts_with('.') => {
                    let mut literal = node(NodeKind::Literal, range);
                    self.set_text(&mut literal, content, TextContext::Verbatim, None);
                    literal
                }
                _ => verbatim(block, range),
            },
            BlockKind::Quote { blocks, .. } => {
                let mut quote = node(NodeKind::Quote, range);
                quote.children = self.contained(blocks, &trail, range);
                quote
            }
            BlockKind::Sidebar { blocks } => {
                self.container(NodeKind::Sidebar, blocks, &trail, range)
            }
            BlockKind::Example { blocks } => {
                self.container(NodeKind::Example, blocks, &trail, range)
            }
            BlockKind::Open { blocks } => self.container(NodeKind::Open, blocks, &trail, range),
            BlockKind::Table {
                columns,
                header,
                rows,
                footer,
            } => self.table(
                block,
                columns.len(),
                [header.as_ref(), None, footer.as_ref()],
                rows,
                &trail,
                range,
            ),
            BlockKind::Image { target, alt, .. } => node(
                NodeKind::Image {
                    target: target.clone(),
                    alt: alt.clone(),
                },
                range,
            ),
            _ => verbatim(block, range),
        };
        built.origin = Some(trail);
        built
    }

    fn section(
        &self,
        level: u8,
        title: &[Inline],
        blocks: &[Block],
        trail: &[usize],
        start: usize,
        limit: usize,
    ) -> Node {
        // A section runs to the next block of its parent.
        let range = self.trim_end(SourceRange::new(start, limit));
        let mut section = node(NodeKind::Section { level }, range);
        let content_start = match self.heading_line(range, level) {
            Some(content) => {
                let line_end = self.source[content.end..]
                    .find('\n')
                    .map_or(self.source.len(), |offset| content.end + offset);
                self.set_text(&mut section, content, TextContext::Line, Some(title));
                line_end
            }
            None => {
                section.access = Access::ReadOnly(ReadOnly::Unchecked);
                range.start
            }
        };
        let children: Vec<(&Block, Vec<usize>)> = blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block, child_trail(trail, index)))
            .collect();
        section.children = self.children(
            &children,
            SourceRange::new(content_start.min(range.end), range.end),
        );
        section
    }

    /// The title text of the heading line for `level` in `range`.
    fn heading_line(&self, range: SourceRange, level: u8) -> Option<SourceRange> {
        let marker = "=".repeat(usize::from(level) + 1);
        let alternative = "#".repeat(usize::from(level) + 1);
        self.lines(range).into_iter().find_map(|line| {
            let text = self.line_text(line);
            let rest = text
                .strip_prefix(marker.as_str())
                .or_else(|| text.strip_prefix(alternative.as_str()))?;
            if !rest.starts_with([' ', '\t']) {
                return None;
            }
            let start = line.start + (text.len() - rest.trim_start().len());
            // Trailing symmetric markers (`== Title ==`) are left to the source editor.
            if rest.trim_end().ends_with(" =") {
                return None;
            }
            Some(self.trim_end(SourceRange::new(start, line.end)))
        })
    }

    fn paragraph(&self, range: SourceRange, inlines: &[Inline]) -> Node {
        let mut paragraph = node(NodeKind::Paragraph, range);
        let content = self.trim(SourceRange::new(self.after_metadata(range), range.end));
        self.set_text(&mut paragraph, content, TextContext::Flow, Some(inlines));
        paragraph
    }

    /// The content of a delimited block in `range` and its delimiter, when it has one.
    fn delimiters(&self, range: SourceRange) -> Option<(SourceRange, &'a str)> {
        let lines = self.lines(range);
        let open_index = lines
            .iter()
            .position(|line| !is_metadata(self.line_text(*line)))?;
        let open = lines[open_index];
        let delimiter = delimiter_of(self.line_text(open))?;
        let close = lines[open_index + 1..]
            .iter()
            .rev()
            .find(|line| self.line_text(**line).trim_end() == delimiter)?;
        let start = (open.end + 1).min(close.start);
        let end = close.start.saturating_sub(1).max(start);
        Some((SourceRange::new(start, end), delimiter))
    }

    fn container(
        &self,
        kind: NodeKind,
        blocks: &[Block],
        trail: &[usize],
        range: SourceRange,
    ) -> Node {
        let mut container = node(kind, range);
        container.children = self.contained(blocks, trail, range);
        container
    }

    /// The children of a container: inside its delimiters, or (for a styled paragraph such as
    /// `NOTE:` or `[quote]`) in its whole range, where the prefix is not a gap.
    fn contained(&self, blocks: &[Block], trail: &[usize], range: SourceRange) -> Vec<Node> {
        let children: Vec<(&Block, Vec<usize>)> = blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block, child_trail(trail, index)))
            .collect();
        match self.delimiters(range) {
            Some((content, _)) => self.children(&children, content),
            None => self.nodes(&children, range),
        }
    }

    fn list(
        &self,
        kind: ListKind,
        items: &[adoc_dom::ListItem],
        trail: &[usize],
        range: SourceRange,
    ) -> Node {
        let mut list = node(NodeKind::List { kind }, range);
        let mut counter = 0;
        for (index, item) in items.iter().enumerate() {
            let next = items
                .get(index + 1)
                .map_or(range.end, |next| next.span.range.start);
            let start = item.span.range.start.clamp(range.start, range.end);
            let item_range = self.snap(start, item.span.range.end, next.max(start));
            let blocks: Vec<(&Block, Vec<usize>)> = item
                .blocks
                .iter()
                .map(|block| {
                    let trail = child_trail(trail, counter);
                    counter += 1;
                    (block, trail)
                })
                .collect();
            list.children
                .push(self.list_item(item, &blocks, item_range));
        }
        list
    }

    fn list_item(
        &self,
        item: &adoc_dom::ListItem,
        blocks: &[(&Block, Vec<usize>)],
        range: SourceRange,
    ) -> Node {
        let line = self.lines(range).first().copied().unwrap_or(Line {
            start: range.start,
            end: range.start,
        });
        let text = self.line_text(line);
        let Some((marker, checked, offset)) = list_marker(text) else {
            let mut unreadable = read_only(
                NodeKind::ListItem {
                    marker: String::new(),
                    checked: item.checked,
                },
                range,
                ReadOnly::Unchecked,
            );
            unreadable.children = self.children(blocks, range);
            return unreadable;
        };
        let content_start = line.start + offset;
        let first_child = blocks
            .iter()
            .find(|(block, _)| self.is_from_page(block))
            .map(|(block, _)| block.meta.span.range.start)
            .filter(|start| *start > content_start)
            .unwrap_or(range.end);
        let content = self.principal(SourceRange::new(content_start, first_child.min(range.end)));
        let mut list_item = node(
            NodeKind::ListItem {
                marker: marker.to_owned(),
                checked,
            },
            range,
        );
        self.set_text(
            &mut list_item,
            content,
            TextContext::Flow,
            Some(&item.content),
        );
        list_item.children = self.children(blocks, SourceRange::new(content.end, range.end));
        list_item
    }

    /// Principal text: `range` trimmed, without a trailing list continuation (`+`).
    fn principal(&self, range: SourceRange) -> SourceRange {
        let mut range = self.trim(range);
        let text = self.text(range);
        if let Some(rest) = text.strip_suffix('+') {
            if rest.is_empty() || rest.ends_with('\n') {
                range = self.trim(SourceRange::new(range.start, range.start + rest.len()));
            }
        }
        range
    }

    fn description_list(
        &self,
        items: &[adoc_dom::DescriptionItem],
        trail: &[usize],
        range: SourceRange,
    ) -> Node {
        let mut list = node(NodeKind::DescriptionList, range);
        let mut counter = 0;
        for (index, item) in items.iter().enumerate() {
            let next = items
                .get(index + 1)
                .map_or(range.end, |next| next.span.range.start);
            let start = item.span.range.start.clamp(range.start, range.end);
            let item_range = self.snap(start, item.span.range.end, next.max(start));
            let blocks: Vec<(&Block, Vec<usize>)> = item
                .blocks
                .iter()
                .map(|block| {
                    let trail = child_trail(trail, counter);
                    counter += 1;
                    (block, trail)
                })
                .collect();
            list.children
                .push(self.description_item(item, &blocks, item_range));
        }
        list
    }

    fn description_item(
        &self,
        item: &adoc_dom::DescriptionItem,
        blocks: &[(&Block, Vec<usize>)],
        range: SourceRange,
    ) -> Node {
        let mut described = node(NodeKind::DescriptionItem, range);
        let text = self.text(range);
        let found = ["::::", ":::", "::", ";;"].iter().find_map(|delimiter| {
            let at = text.find(delimiter)?;
            let after = text[at + delimiter.len()..].chars().next();
            after
                .is_none_or(char::is_whitespace)
                .then_some((at, delimiter.len()))
        });
        let Some((at, length)) = found else {
            described.access = Access::ReadOnly(ReadOnly::Unchecked);
            described.children = self.children(blocks, range);
            return described;
        };
        let term_range = self.trim(SourceRange::new(range.start, range.start + at));
        let mut term = node(NodeKind::Term, term_range);
        self.set_text(&mut term, term_range, TextContext::Line, Some(&item.term));
        described.children.push(term);
        let after = range.start + at + length;
        let first_child = blocks
            .iter()
            .find(|(block, _)| self.is_from_page(block))
            .map(|(block, _)| block.meta.span.range.start)
            .filter(|start| *start > after)
            .unwrap_or(range.end);
        let content = self.principal(SourceRange::new(after, first_child.min(range.end)));
        let mut rest_start = after;
        if !content.is_empty() {
            let mut description = node(NodeKind::Description, content);
            self.set_text(
                &mut description,
                content,
                TextContext::Flow,
                Some(&item.content),
            );
            described.children.push(description);
            rest_start = content.end;
        }
        let rest = self.children(blocks, SourceRange::new(rest_start, range.end));
        described.children.extend(rest);
        described
    }

    #[allow(clippy::too_many_arguments)]
    fn table(
        &self,
        block: &Block,
        columns: usize,
        edges: [Option<&adoc_dom::Row>; 3],
        rows: &[adoc_dom::Row],
        trail: &[usize],
        range: SourceRange,
    ) -> Node {
        let [header, _, footer] = edges;
        let Some((body, delimiter)) = self.delimiters(range) else {
            return verbatim(block, range);
        };
        if delimiter != "|===" {
            return verbatim(block, range);
        }
        let cells = self.cells(body);
        let dom_rows: Vec<(RowSection, &adoc_dom::Row)> = header
            .map(|row| (RowSection::Header, row))
            .into_iter()
            .chain(rows.iter().map(|row| (RowSection::Body, row)))
            .chain(footer.map(|row| (RowSection::Footer, row)))
            .collect();
        let expected: usize = dom_rows.iter().map(|(_, row)| row.cells.len()).sum();
        if expected != cells.len() {
            return verbatim(block, range);
        }
        let column_styles = match &block.kind {
            BlockKind::Table { columns, .. } => columns
                .iter()
                .map(|column| column.style)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let regular = columns > 0
            && cells.iter().all(|cell| cell.spec.is_empty())
            && dom_rows.iter().all(|(_, row)| {
                row.cells.len() == columns
                    && row
                        .cells
                        .iter()
                        .all(|cell| cell.colspan == 1 && cell.rowspan == 1)
            });
        let mut table = node(NodeKind::Table { columns, regular }, range);
        let mut cells = cells.into_iter();
        let mut counter = 0;
        for (section, dom_row) in dom_rows {
            let mut row_cells = Vec::new();
            for (column, dom_cell) in dom_row.cells.iter().enumerate() {
                let Some(cell) = cells.next() else {
                    break;
                };
                let cell_trails: Vec<Vec<usize>> = dom_cell
                    .blocks
                    .iter()
                    .map(|_| {
                        let trail = child_trail(trail, counter);
                        counter += 1;
                        trail
                    })
                    .collect();
                let style = dom_cell
                    .style
                    .or_else(|| column_styles.get(column).copied())
                    .unwrap_or(CellStyle::Default);
                row_cells.push(self.cell(&cell, dom_cell, style, cell_trails));
            }
            let row_range = match (row_cells.first(), row_cells.last()) {
                (Some(first), Some(last)) => SourceRange::new(first.range.start, last.range.end),
                _ => SourceRange::new(body.start, body.start),
            };
            let mut row = node(NodeKind::Row { section }, row_range);
            row.children = row_cells;
            table.children.push(row);
        }
        table
    }

    fn cell(
        &self,
        cell: &CellSource,
        dom: &adoc_dom::Cell,
        style: CellStyle,
        trails: Vec<Vec<usize>>,
    ) -> Node {
        let mut built = node(NodeKind::Cell, cell.range);
        let simple_style = matches!(
            style,
            CellStyle::Default
                | CellStyle::Emphasis
                | CellStyle::Header
                | CellStyle::Monospace
                | CellStyle::Strong
        );
        let one_paragraph = match dom.blocks.as_slice() {
            [] => Some(&[][..]),
            [Block {
                kind: BlockKind::Paragraph { inlines },
                ..
            }] => Some(inlines.as_slice()),
            _ => None,
        };
        match one_paragraph.filter(|_| simple_style) {
            Some(inlines) => {
                self.set_text(&mut built, cell.content, TextContext::Cell, Some(inlines));
            }
            None => built.access = Access::ReadOnly(ReadOnly::Verbatim),
        }
        built.origin = trails.into_iter().next();
        built
    }

    /// The cells of a PSV table body, in order.
    fn cells(&self, body: SourceRange) -> Vec<CellSource> {
        let text = self.text(body);
        let bytes = text.as_bytes();
        let mut separators = Vec::new();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'|' && (index == 0 || bytes[index - 1] != b'\\') {
                separators.push(index);
            }
        }
        let spec_start = |bar: usize| {
            let mut start = bar;
            while start > 0
                && matches!(
                    bytes[start - 1],
                    b'0'..=b'9'
                        | b'.'
                        | b'+'
                        | b'*'
                        | b'<'
                        | b'^'
                        | b'>'
                        | b'a'
                        | b'd'
                        | b'e'
                        | b'h'
                        | b'l'
                        | b'm'
                        | b's'
                        | b'v'
                )
            {
                start -= 1;
            }
            // A spec begins a line or follows whitespace.
            if start == bar || start == 0 || bytes[start - 1].is_ascii_whitespace() {
                start
            } else {
                bar
            }
        };
        let mut cells = Vec::new();
        for (index, bar) in separators.iter().enumerate() {
            let start = spec_start(*bar);
            let next = separators
                .get(index + 1)
                .map_or(text.len(), |next| spec_start(*next));
            let content = self.trim(SourceRange::new(
                body.start + bar + 1,
                body.start + next.max(bar + 1),
            ));
            let content = if content.is_empty() {
                SourceRange::new(body.start + bar + 1, body.start + bar + 1)
            } else {
                content
            };
            cells.push(CellSource {
                range: SourceRange::new(body.start + start, content.end),
                content,
                spec: text[start..*bar].to_owned(),
            });
        }
        cells
    }
}

/// A table cell as written.
struct CellSource {
    /// From the cell's spec or bar to the end of its content.
    range: SourceRange,
    content: SourceRange,
    spec: String,
}

fn child_trail(trail: &[usize], index: usize) -> Vec<usize> {
    let mut child = trail.to_vec();
    child.push(index);
    child
}

fn verbatim(block: &Block, range: SourceRange) -> Node {
    read_only(
        NodeKind::Verbatim {
            name: block.kind_name().to_owned(),
        },
        range,
        ReadOnly::Verbatim,
    )
}

fn is_attribute_entry(line: &str) -> bool {
    let Some(rest) = line.strip_prefix(':') else {
        return false;
    };
    rest.find(':').is_some_and(|end| {
        let name = rest[..end].trim_start_matches('!').trim_end_matches('!');
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// A list item's marker, checkbox, and the offset of its text in `line`.
pub(crate) fn list_marker(line: &str) -> Option<(&str, Option<bool>, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let marker_len = if rest.starts_with('*') {
        rest.chars().take_while(|c| *c == '*').count()
    } else if rest.starts_with('.') {
        rest.chars().take_while(|c| *c == '.').count()
    } else if rest.starts_with('-') {
        1
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let letters = rest.chars().take_while(char::is_ascii_alphabetic).count();
        if digits > 0 && rest[digits..].starts_with('.') {
            digits + 1
        } else if letters == 1 && rest[1..].starts_with('.') {
            2
        } else if letters > 0 && rest[letters..].starts_with(')') {
            letters + 1
        } else {
            return None;
        }
    };
    let marker = &rest[..marker_len];
    let after = &rest[marker_len..];
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let text_start = after.len() - after.trim_start().len();
    let mut offset = indent + marker_len + text_start;
    let body = &line[offset..];
    let mut checked = None;
    for (box_, state) in [("[ ]", false), ("[x]", true), ("[X]", true), ("[*]", true)] {
        if let Some(after_box) = body.strip_prefix(box_) {
            if after_box.starts_with([' ', '\t']) {
                checked = Some(state);
                offset += box_.len() + (after_box.len() - after_box.trim_start().len());
            }
            break;
        }
    }
    Some((marker, checked, offset))
}

/// Whether the scanner's reading of a text block shows what the parser's does: the same
/// characters (ignoring whitespace) with the same formatting.
pub(crate) fn agrees(source: &str, pieces: &[Piece], dom: &[Inline]) -> bool {
    let mut ours = Vec::new();
    model_signature(source, pieces, MarkSet::EMPTY, &mut ours);
    let mut theirs = Vec::new();
    dom_signature(dom, MarkSet::EMPTY, &mut theirs);
    ours == theirs
}

fn push_chars(text: &str, marks: MarkSet, out: &mut Vec<(char, MarkSet)>) {
    out.extend(
        text.chars()
            .filter(|character| !character.is_whitespace() && *character != '\u{200b}')
            .map(|character| (character, marks)),
    );
}

fn model_signature(source: &str, pieces: &[Piece], marks: MarkSet, out: &mut Vec<(char, MarkSet)>) {
    for piece in pieces {
        match piece {
            Piece::Text { range } => push_chars(&source[range.start..range.end], marks, out),
            Piece::Mark { mark, children, .. } => {
                let inner = if *mark == Mark::Span {
                    marks
                } else {
                    marks.with(*mark)
                };
                model_signature(source, children, inner, out);
            }
            Piece::Atom { atom, shown, .. } => push_chars(atom.dom_text(shown), marks, out),
        }
    }
}

fn dom_signature(inlines: &[Inline], marks: MarkSet, out: &mut Vec<(char, MarkSet)>) {
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Passthrough(text) | Inline::Button(text) => {
                push_chars(text, marks, out);
            }
            Inline::Bold(content) => dom_signature(content, marks.with(Mark::Strong), out),
            Inline::Italic(content) => dom_signature(content, marks.with(Mark::Emphasis), out),
            Inline::Monospace(content) => dom_signature(content, marks.with(Mark::Code), out),
            Inline::Highlight(content) => dom_signature(content, marks.with(Mark::Highlight), out),
            Inline::Superscript(content) => {
                dom_signature(content, marks.with(Mark::Superscript), out);
            }
            Inline::Subscript(content) => dom_signature(content, marks.with(Mark::Subscript), out),
            Inline::Span { content, .. } => dom_signature(content, marks, out),
            Inline::Link { text, .. } | Inline::Xref { text, .. } => {
                push_chars(&Inline::plain_text(text), marks, out);
            }
            Inline::Image { alt, .. } => push_chars(alt, marks, out),
            Inline::Icon { name } => push_chars(name, marks, out),
            Inline::Keyboard(keys) => push_chars(&keys.join("+"), marks, out),
            Inline::Menu(items) => push_chars(&items.join(" > "), marks, out),
            Inline::Stem { source, .. } => push_chars(source, marks, out),
            Inline::Anchor { .. } | Inline::Footnote { .. } | Inline::LineBreak => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_list_markers() {
        assert_eq!(list_marker("* One"), Some(("*", None, 2)));
        assert_eq!(list_marker("** Nested"), Some(("**", None, 3)));
        assert_eq!(list_marker(". First"), Some((".", None, 2)));
        assert_eq!(list_marker("- [x] Done"), Some(("-", Some(true), 6)));
        assert_eq!(list_marker("* [ ] Not done"), Some(("*", Some(false), 6)));
        assert_eq!(list_marker("10. Tenth"), Some(("10.", None, 4)));
        assert_eq!(list_marker("  * Indented"), Some(("*", None, 4)));
        assert_eq!(list_marker("*bold* text"), None);
        assert_eq!(list_marker("Plain"), None);
    }

    #[test]
    fn recognises_attribute_entries() {
        assert!(is_attribute_entry(":toc:"));
        assert!(is_attribute_entry(":product: Foundry"));
        assert!(is_attribute_entry(":!sectnums:"));
        assert!(!is_attribute_entry(":: not one"));
        assert!(!is_attribute_entry("NOTE: text"));
    }
}
