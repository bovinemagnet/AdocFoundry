//! Part of a page: the text a selection covers, taken from the view tree, for copying.
//!
//! A page's text is a sequence of *leaves* ([`Leaf`]): each stretch of text drawn as one, a
//! paragraph, a list item's text, a block's title, a block of code. A selection is two places
//! in that sequence ([`Place`]), so it can start and end in text that has no place of its own
//! in the page's source, as everything an include brings in has not. [`Flow::cut`] takes what
//! lies between two places as a [`Fragment`], which is written as plain text, Markdown,
//! AsciiDoc or HTML. Nothing here depends on what is drawn, so text out of view is copied too.

mod asciidoc;
mod cut;
mod html;
mod markdown;
mod plain;

use std::{collections::HashMap, ops::Range};

use adoc_dom::{AdmonitionKind, ListKind, SourceSpan};

use crate::{
    DiagramView, MediaKind, ViewBlock, ViewFootnote, ViewImage, ViewInline, ViewKind, ViewTree,
};

pub use cut::list_number;

/// What an inline that is drawn as a picture stands for in a leaf's text: a typeset formula.
pub const OBJECT: char = '\u{fffc}';

/// Which text of a block a leaf is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Part {
    /// The page's title, which belongs to no block: the leaf's trail is empty.
    DocumentTitle,
    /// The table of contents; the one at the top of the page has an empty trail.
    Contents,
    /// An admonition's label.
    Label,
    /// The block's title, with its caption.
    Title,
    /// A section's or discrete heading's title, with its number.
    Heading,
    /// A paragraph's text, or the text of a verbatim block.
    Body,
    /// The principal text of a list item, a term, a description or a callout, by its index.
    Item(usize),
    Term(usize),
    Description(usize),
    Callout(usize),
    /// Who a quotation is from.
    Attribution,
}

/// Names a leaf: the trail of its block, as [`crate::block_by_trail`] follows it, and which of
/// the block's text it is.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LeafKey {
    pub trail: Vec<usize>,
    pub part: Part,
}

impl LeafKey {
    #[must_use]
    pub fn new(trail: &[usize], part: Part) -> Self {
        Self {
            trail: trail.to_vec(),
            part,
        }
    }
}

/// A stretch of a page's text.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub key: LeafKey,
    /// Its text as drawn, with [`OBJECT`] for each typeset formula.
    pub text: String,
    /// The text is an editor's own rather than the view tree's ([`Flow::set_text`]).
    own: bool,
}

/// A place in a page's text: a leaf, by its position in the flow, and a byte offset in its
/// text.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Place {
    pub leaf: usize,
    pub offset: usize,
}

impl Place {
    #[must_use]
    pub const fn new(leaf: usize, offset: usize) -> Self {
        Self { leaf, offset }
    }
}

/// The leaves a block and everything in it hold: from `first`, up to `end`.
#[derive(Clone, Copy, Debug)]
struct Extent {
    first: usize,
    end: usize,
}

/// A page's text in document order.
#[derive(Clone, Debug, Default)]
pub struct Flow {
    leaves: Vec<Leaf>,
    index: HashMap<LeafKey, usize>,
    extents: HashMap<Vec<usize>, Extent>,
}

impl Flow {
    #[must_use]
    pub fn new(view: &ViewTree) -> Self {
        let mut flow = Self::default();
        if let Some(title) = &view.title {
            flow.push(&[], Part::DocumentTitle, shown_text(title));
        }
        if !view.toc.is_empty() && !view.toc_at_macro() {
            flow.push(&[], Part::Contents, contents_text(view));
        }
        let mut trail = Vec::new();
        for (index, block) in view.blocks.iter().enumerate() {
            trail.push(index);
            flow.block(view, block, &mut trail);
            trail.pop();
        }
        flow
    }

    #[must_use]
    pub fn leaves(&self) -> &[Leaf] {
        &self.leaves
    }

    #[must_use]
    pub fn position(&self, key: &LeafKey) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// The place at `offset` of the leaf `key`, which is clamped to its text.
    #[must_use]
    pub fn place(&self, key: &LeafKey, offset: usize) -> Option<Place> {
        let leaf = self.position(key)?;
        let text = &self.leaves[leaf].text;
        Some(Place::new(leaf, text.floor_char_boundary(offset)))
    }

    /// The first and last places of the page, when it has any text.
    #[must_use]
    pub fn ends(&self) -> Option<(Place, Place)> {
        let last = self.leaves.len().checked_sub(1)?;
        Some((
            Place::new(0, 0),
            Place::new(last, self.leaves[last].text.len()),
        ))
    }

    /// Gives the leaf `key` an editor's own text, as it draws it: what is copied from it is
    /// taken from this text, and offsets in it are this text's.
    pub fn set_text(&mut self, key: &LeafKey, text: String) {
        if let Some(index) = self.position(key) {
            self.leaves[index].text = text;
            self.leaves[index].own = true;
        }
    }

    /// The part of leaf `leaf` between two places.
    #[must_use]
    pub fn selected(&self, leaf: usize, from: Place, to: Place) -> Option<Range<usize>> {
        if leaf < from.leaf || leaf > to.leaf {
            return None;
        }
        let text = &self.leaves.get(leaf)?.text;
        let start = if leaf == from.leaf { from.offset } else { 0 };
        let end = if leaf == to.leaf {
            to.offset
        } else {
            text.len()
        };
        let range = text.floor_char_boundary(start)..text.floor_char_boundary(end);
        (range.start < range.end).then_some(range)
    }

    fn push(&mut self, trail: &[usize], part: Part, text: String) {
        if text.is_empty() {
            return;
        }
        let key = LeafKey::new(trail, part);
        self.index.insert(key.clone(), self.leaves.len());
        self.leaves.push(Leaf {
            key,
            text,
            own: false,
        });
    }

    fn block(&mut self, view: &ViewTree, block: &ViewBlock, trail: &mut Vec<usize>) {
        let first = self.leaves.len();
        // A picture's title is drawn below it, as a figure's caption is.
        let title_after = matches!(block.kind, ViewKind::Image(_) | ViewKind::Diagram(_));
        if let ViewKind::Admonition { kind, .. } = &block.kind {
            self.push(trail, Part::Label, kind.label().to_owned());
        }
        if !title_after {
            self.push(trail, Part::Title, title_text(block));
        }
        match &block.kind {
            ViewKind::Section { title, number, .. } => {
                let text = shown_text(&heading(number.as_deref(), title));
                self.push(trail, Part::Heading, text);
            }
            ViewKind::Heading { title, .. } => self.push(trail, Part::Heading, shown_text(title)),
            ViewKind::Paragraph(content) => self.push(trail, Part::Body, shown_text(content)),
            ViewKind::Toc if view.toc_at_macro() && !view.toc.is_empty() => {
                self.push(trail, Part::Contents, contents_text(view));
            }
            _ => {
                if let Some(text) = verbatim_text(&block.kind) {
                    self.push(trail, Part::Body, text.to_owned());
                }
            }
        }
        self.inside(view, block, trail);
        if let Some(line) = attribution(&block.kind) {
            self.push(trail, Part::Attribution, line);
        }
        if title_after {
            self.push(trail, Part::Title, title_text(block));
        }
        self.extents.insert(
            trail.clone(),
            Extent {
                first,
                end: self.leaves.len(),
            },
        );
    }

    /// The items and blocks inside `block`, numbered as [`ViewBlock::child_refs`] numbers them.
    fn inside(&mut self, view: &ViewTree, block: &ViewBlock, trail: &mut Vec<usize>) {
        let mut next = 0;
        let mut children = |flow: &mut Self, blocks: &[ViewBlock], trail: &mut Vec<usize>| {
            for child in blocks {
                trail.push(next);
                flow.block(view, child, trail);
                trail.pop();
                next += 1;
            }
        };
        match &block.kind {
            ViewKind::Section { blocks, .. }
            | ViewKind::Admonition { blocks, .. }
            | ViewKind::Quote { blocks, .. }
            | ViewKind::Sidebar(blocks)
            | ViewKind::Example(blocks)
            | ViewKind::Open(blocks) => children(self, blocks, trail),
            ViewKind::List { items, .. } => {
                for (index, item) in items.iter().enumerate() {
                    self.push(trail, Part::Item(index), shown_text(&item.content));
                    children(self, &item.blocks, trail);
                }
            }
            ViewKind::DescriptionList(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.push(trail, Part::Term(index), shown_text(&item.term));
                    self.push(trail, Part::Description(index), shown_text(&item.content));
                    children(self, &item.blocks, trail);
                }
            }
            ViewKind::CalloutList(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.push(trail, Part::Callout(index), shown_text(&item.content));
                    children(self, &item.blocks, trail);
                }
            }
            ViewKind::Table(table) => {
                for row in table.header.iter().chain(&table.rows).chain(&table.footer) {
                    for cell in row {
                        children(self, &cell.blocks, trail);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A section's title with the number drawn before it.
pub(crate) fn heading(number: Option<&str>, title: &[ViewInline]) -> Vec<ViewInline> {
    let mut content = Vec::new();
    if let Some(number) = number {
        content.push(ViewInline::Text(format!("{number} ")));
    }
    content.extend(title.iter().cloned());
    content
}

/// A block's title with the caption drawn before it; empty for a block with no title.
pub(crate) fn captioned(block: &ViewBlock) -> Vec<ViewInline> {
    if block.title.is_empty() {
        return Vec::new();
    }
    let mut content = Vec::new();
    if let Some(caption) = &block.caption {
        content.push(ViewInline::Text(format!("{caption} ")));
    }
    content.extend(block.title.iter().cloned());
    content
}

fn title_text(block: &ViewBlock) -> String {
    shown_text(&captioned(block))
}

/// The table of contents as drawn: its title, then a line for each entry.
#[must_use]
pub fn contents_text(view: &ViewTree) -> String {
    let mut lines = vec![view.toc_title().to_owned()];
    lines.extend(view.toc.iter().map(|entry| match &entry.number {
        Some(number) => format!("{number} {}", entry.title),
        None => entry.title.clone(),
    }));
    lines.join("\n")
}

/// The text of a block that holds text rather than inlines or blocks: what is drawn for it, or
/// what stands for it where it is drawn as a picture.
#[must_use]
pub fn verbatim_text(kind: &ViewKind) -> Option<&str> {
    match kind {
        ViewKind::Code { text, .. }
        | ViewKind::Literal(text)
        | ViewKind::Verse { text, .. }
        | ViewKind::Passthrough(text) => Some(text),
        ViewKind::Stem { source, .. } | ViewKind::Unsupported { source, .. } => Some(source),
        ViewKind::Image(image) => Some(&image.alt),
        ViewKind::Diagram(
            DiagramView::Pending { source, .. } | DiagramView::Failed { source, .. },
        ) => Some(source),
        _ => None,
    }
}

/// Who a quotation or verse is from, as drawn below it.
#[must_use]
pub fn attribution(kind: &ViewKind) -> Option<String> {
    let (ViewKind::Quote {
        attribution,
        citation,
        ..
    }
    | ViewKind::Verse {
        attribution,
        citation,
        ..
    }) = kind
    else {
        return None;
    };
    match (attribution, citation) {
        (None, None) => None,
        (Some(one), None) | (None, Some(one)) => Some(format!("— {one}")),
        (Some(attribution), Some(citation)) => Some(format!("— {attribution}, {citation}")),
    }
}

/// Inline content as the preview draws it, in one string: a leaf's text.
#[must_use]
pub fn shown_text(inlines: &[ViewInline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        push_shown(inline, &mut out);
    }
    out
}

fn push_shown(inline: &ViewInline, out: &mut String) {
    match content_of(inline) {
        Some(content) => content.iter().for_each(|inline| push_shown(inline, out)),
        None => match inline {
            ViewInline::Text(text) | ViewInline::Raw(text) => out.push_str(text),
            ViewInline::Image(image) => {
                out.push('[');
                out.push_str(&image.alt);
                out.push(']');
            }
            ViewInline::Icon(name) => out.push_str(&format!("[{name}]")),
            ViewInline::FootnoteRef { number, .. } => out.push_str(&format!("[{number}]")),
            ViewInline::LineBreak => out.push('\n'),
            ViewInline::Keyboard(keys) => out.push_str(&keys.join("+")),
            ViewInline::Button(label) => out.push_str(&format!("[{label}]")),
            ViewInline::Menu(items) => out.push_str(&items.join(" › ")),
            ViewInline::Stem {
                rendered: Some(_), ..
            } => out.push(OBJECT),
            ViewInline::Stem { source, .. } => out.push_str(source),
            _ => {}
        },
    }
}

/// The inlines inside `inline`, when it is formatting around others.
pub(crate) fn content_of(inline: &ViewInline) -> Option<&Vec<ViewInline>> {
    match inline {
        ViewInline::Strong(content)
        | ViewInline::Emphasis(content)
        | ViewInline::Code(content)
        | ViewInline::Mark(content)
        | ViewInline::Superscript(content)
        | ViewInline::Subscript(content)
        | ViewInline::Span { content, .. }
        | ViewInline::Link { content, .. }
        | ViewInline::Xref { content, .. }
        | ViewInline::IndexTerm { content, .. } => Some(content),
        _ => None,
    }
}

/// The inlines inside `inline`, to change them.
pub(crate) fn content_of_mut(inline: &mut ViewInline) -> Option<&mut Vec<ViewInline>> {
    match inline {
        ViewInline::Strong(content)
        | ViewInline::Emphasis(content)
        | ViewInline::Code(content)
        | ViewInline::Mark(content)
        | ViewInline::Superscript(content)
        | ViewInline::Subscript(content)
        | ViewInline::Span { content, .. }
        | ViewInline::Link { content, .. }
        | ViewInline::Xref { content, .. }
        | ViewInline::IndexTerm { content, .. } => Some(content),
        _ => None,
    }
}

/// Calls `visit` with every run of inlines of `blocks`.
fn each_inlines(blocks: &mut [FragmentBlock], visit: &mut impl FnMut(&mut Vec<ViewInline>)) {
    for block in blocks {
        visit(&mut block.title);
        match &mut block.kind {
            FragmentKind::Heading { content, .. } | FragmentKind::Paragraph(content) => {
                visit(content);
            }
            FragmentKind::List { items, .. } | FragmentKind::Callouts(items) => {
                for item in items {
                    visit(&mut item.content);
                    each_inlines(&mut item.blocks, visit);
                }
            }
            FragmentKind::Descriptions(items) => {
                for item in items {
                    visit(&mut item.term);
                    visit(&mut item.content);
                    each_inlines(&mut item.blocks, visit);
                }
            }
            FragmentKind::Admonition { blocks, .. }
            | FragmentKind::Quote { blocks, .. }
            | FragmentKind::Sidebar(blocks)
            | FragmentKind::Example(blocks)
            | FragmentKind::Open(blocks) => each_inlines(blocks, visit),
            FragmentKind::Table { rows, .. } => rows
                .iter_mut()
                .flatten()
                .for_each(|cell| each_inlines(&mut cell.blocks, visit)),
            _ => {}
        }
    }
}

impl Fragment {
    /// The number of each footnote the blocks refer to, as often as they do.
    fn references(&self) -> Vec<u32> {
        fn visit(content: &[ViewInline], out: &mut Vec<u32>) {
            for inline in content {
                match inline {
                    ViewInline::FootnoteRef { number, .. } => out.push(*number),
                    other => {
                        if let Some(inside) = content_of(other) {
                            visit(inside, out);
                        }
                    }
                }
            }
        }
        let mut numbers = Vec::new();
        let mut blocks = self.blocks.clone();
        each_inlines(&mut blocks, &mut |content| visit(content, &mut numbers));
        numbers
    }

    /// The footnotes the blocks refer to, each once.
    fn noted(&self) -> Vec<&ViewFootnote> {
        let numbers = self.references();
        self.footnotes
            .iter()
            .filter(|note| numbers.contains(&note.number))
            .collect()
    }
}

/// What a selection covers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fragment {
    pub blocks: Vec<FragmentBlock>,
    /// The page's footnotes, for those the blocks refer to.
    pub footnotes: Vec<ViewFootnote>,
}

/// A block of a fragment, or the part of one a selection covers.
#[derive(Clone, Debug, PartialEq)]
pub struct FragmentBlock {
    /// The caption drawn before its title, such as `Table 2.`
    pub caption: Option<String>,
    pub title: Vec<ViewInline>,
    pub kind: FragmentKind,
    /// Where the block is written, when the selection takes all of it.
    pub whole: Option<SourceSpan>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FragmentKind {
    /// A heading, at level 0 for the page's title; `discrete` starts no section.
    Heading {
        level: u8,
        discrete: bool,
        content: Vec<ViewInline>,
        /// The number drawn before it, which is not part of its source.
        number: Option<String>,
        /// The section's style, such as `appendix`.
        style: Option<String>,
    },
    Paragraph(Vec<ViewInline>),
    List {
        kind: ListKind,
        items: Vec<FragmentItem>,
        reversed: bool,
    },
    Descriptions(Vec<FragmentDescription>),
    Callouts(Vec<FragmentItem>),
    Admonition {
        kind: AdmonitionKind,
        /// The selection takes its label.
        label: bool,
        blocks: Vec<FragmentBlock>,
    },
    Code {
        language: Option<String>,
        text: String,
        /// Each callout's line in `text`, from zero, and its number.
        callouts: Vec<(usize, usize)>,
    },
    Literal(String),
    Quote {
        attribution: Option<String>,
        citation: Option<String>,
        blocks: Vec<FragmentBlock>,
    },
    Verse {
        attribution: Option<String>,
        citation: Option<String>,
        text: String,
    },
    Sidebar(Vec<FragmentBlock>),
    Example(Vec<FragmentBlock>),
    Open(Vec<FragmentBlock>),
    Table {
        header: bool,
        rows: Vec<Vec<FragmentCell>>,
    },
    Image(ViewImage),
    Media {
        kind: MediaKind,
        targets: Vec<String>,
    },
    Stem {
        latex: bool,
        source: String,
    },
    Passthrough(String),
    Rule,
    PageBreak,
    /// A diagram: its kind and source, when it is not drawn as a picture.
    Diagram(Option<(String, String)>),
    /// Text with nothing of its own to say what it is: the contents, an attribution alone.
    Text(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FragmentItem {
    /// Its marker is copied: the selection takes the item from its start, and not only part of
    /// this one item.
    pub marked: bool,
    /// `-` where the page writes the item so, else `*`.
    pub bullet: char,
    /// An ordered item's number on the page, or a callout's.
    pub number: i64,
    pub checked: Option<bool>,
    pub content: Vec<ViewInline>,
    pub blocks: Vec<FragmentBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FragmentDescription {
    pub term: Vec<ViewInline>,
    pub content: Vec<ViewInline>,
    pub blocks: Vec<FragmentBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FragmentCell {
    pub blocks: Vec<FragmentBlock>,
    pub colspan: usize,
    pub rowspan: usize,
}

impl Fragment {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Calls `visit` with every block, each before those inside it.
    pub fn each_block<'a>(&'a self, visit: &mut impl FnMut(&'a FragmentBlock)) {
        each_block(&self.blocks, visit);
    }

    /// Plain text: spaces, tabs, new lines and list markers only.
    #[must_use]
    pub fn to_plain(&self) -> String {
        plain::write(self)
    }

    /// GitHub-flavoured Markdown.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        markdown::write(self)
    }

    /// AsciiDoc. `source` gives the source of a block taken whole, where it can be copied as it
    /// is written; every other block is written from what it holds.
    #[must_use]
    pub fn to_asciidoc(&self, source: &dyn Fn(&SourceSpan) -> Option<String>) -> String {
        asciidoc::write(self, source)
    }

    /// HTML for a word processor: elements with their look in place, and nothing that names a
    /// file. `link` gives the address of a cross reference that has one.
    #[must_use]
    pub fn to_html(&self, link: &dyn Fn(&crate::ResolvedLink) -> Option<String>) -> String {
        html::write(self, link)
    }
}

fn each_block<'a>(blocks: &'a [FragmentBlock], visit: &mut impl FnMut(&'a FragmentBlock)) {
    for block in blocks {
        visit(block);
        match &block.kind {
            FragmentKind::List { items, .. } | FragmentKind::Callouts(items) => {
                items
                    .iter()
                    .for_each(|item| each_block(&item.blocks, visit));
            }
            FragmentKind::Descriptions(items) => items
                .iter()
                .for_each(|item| each_block(&item.blocks, visit)),
            FragmentKind::Admonition { blocks, .. }
            | FragmentKind::Quote { blocks, .. }
            | FragmentKind::Sidebar(blocks)
            | FragmentKind::Example(blocks)
            | FragmentKind::Open(blocks) => each_block(blocks, visit),
            FragmentKind::Table { rows, .. } => rows
                .iter()
                .flatten()
                .for_each(|cell| each_block(&cell.blocks, visit)),
            _ => {}
        }
    }
}

/// The code of a block to copy and run: without its callouts, or the comment left where one
/// was.
#[must_use]
pub fn runnable_code(text: &str, callouts: &[adoc_dom::Callout]) -> String {
    text.split('\n')
        .enumerate()
        .map(|(number, line)| {
            if callouts.iter().any(|callout| callout.line == number) {
                without_callout(line)
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `line` without the comment a callout was written in: `<!---->` is what the parser leaves of
/// `<!--1-->`. Every other comment it takes off itself.
fn without_callout(line: &str) -> &str {
    let mut line = line.trim_end();
    while let Some(rest) = line.strip_suffix("<!---->") {
        line = rest.trim_end();
    }
    line
}
