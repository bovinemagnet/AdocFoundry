//! The `Book`: the one semantic representation both writers read, so the two formats
//! agree (PRD §9). Chapters in reading order, each a run of blocks with inline content, a
//! nested table of contents, metadata and embedded assets. Every anchor is generated
//! ([`crate::map`]), so nothing here depends on a file path.

use std::path::PathBuf;

/// A book ready to write in either format.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Book {
    pub metadata: Metadata,
    /// The reading order. Never empty: an export with no content has one empty chapter.
    pub chapters: Vec<Chapter>,
    /// The table of contents, nested.
    pub toc: Vec<TocEntry>,
    /// Embedded pictures, in the order they are first used. [`Inline::Image`] and
    /// [`Block::Figure`] name them by index.
    pub assets: Vec<Asset>,
    /// The cover, an index into `assets`.
    pub cover: Option<usize>,
}

/// What the library shows of a book, and what identifies it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub title: String,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    /// A BCP 47 language tag (`en`, `en-GB`, `ja`).
    pub language: String,
    /// Stable for the same title, authors and version: `urn:uuid:…`.
    pub identifier: String,
    pub publisher: Option<String>,
    pub description: Option<String>,
    /// The date as written (`revdate`); never the time of the export.
    pub date: Option<String>,
    pub version: Option<String>,
}

/// A unit of the reading order: a page, a part's title or a heading of the navigation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chapter {
    /// `c1`, `c2` and so on in reading order.
    pub id: String,
    /// Its title as plain text, for the table of contents.
    pub title: String,
    pub blocks: Vec<Block>,
    /// The page it was made from, for diagnostics; `None` for a part or a heading.
    pub source: Option<PathBuf>,
}

/// One entry of the table of contents.
#[derive(Clone, Debug, PartialEq)]
pub struct TocEntry {
    pub title: String,
    pub target: Target,
    pub children: Vec<TocEntry>,
}

/// A place in the book: a chapter, and an anchor in it or its start.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Target {
    /// An index into [`Book::chapters`].
    pub chapter: usize,
    pub anchor: Option<String>,
}

/// An embedded picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub kind: AssetKind,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetKind {
    Png,
    Jpeg,
    Gif,
}

impl AssetKind {
    #[must_use]
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
        }
    }

    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
        }
    }
}

/// How a paragraph reads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ParagraphClass {
    #[default]
    Normal,
    /// A block's title, or a table's or figure's caption.
    Caption,
    /// Who said a quotation, and where.
    Attribution,
    /// An admonition's label (`Note`, `Warning`).
    Label,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading {
        /// 1 to 6.
        level: u8,
        anchor: Option<String>,
        content: Vec<Inline>,
    },
    Paragraph {
        anchor: Option<String>,
        class: ParagraphClass,
        content: Vec<Inline>,
    },
    /// Text whose lines and spaces are kept: a listing, a literal block or a verse.
    Preformatted {
        anchor: Option<String>,
        text: String,
        /// In a fixed-width font: source and literal blocks, not verses.
        code: bool,
    },
    List {
        anchor: Option<String>,
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    /// A description list: each term and its description.
    Definitions {
        anchor: Option<String>,
        items: Vec<(Vec<Inline>, Vec<Block>)>,
    },
    Quote {
        anchor: Option<String>,
        blocks: Vec<Block>,
    },
    /// Content set apart: an admonition, a sidebar or an example.
    Aside {
        anchor: Option<String>,
        kind: AsideKind,
        blocks: Vec<Block>,
    },
    Table {
        anchor: Option<String>,
        rows: Vec<Row>,
    },
    Figure {
        anchor: Option<String>,
        /// An index into [`Book::assets`].
        asset: usize,
        alt: String,
    },
    Rule,
    PageBreak,
    /// The chapter's footnotes, at its end.
    Notes(Vec<Note>),
}

impl Block {
    /// The block's own anchor, if it has one.
    #[must_use]
    pub fn anchor(&self) -> Option<&str> {
        match self {
            Self::Heading { anchor, .. }
            | Self::Paragraph { anchor, .. }
            | Self::Preformatted { anchor, .. }
            | Self::List { anchor, .. }
            | Self::Definitions { anchor, .. }
            | Self::Quote { anchor, .. }
            | Self::Aside { anchor, .. }
            | Self::Table { anchor, .. }
            | Self::Figure { anchor, .. } => anchor.as_deref(),
            Self::Rule | Self::PageBreak | Self::Notes(_) => None,
        }
    }
}

/// What an [`Block::Aside`] sets apart. A format that cannot tell them apart shows each the
/// same way.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsideKind {
    /// An admonition, its kind's name (`note`, `tip`, `important`, `caution`, `warning`).
    Admonition(&'static str),
    Sidebar,
    Example,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// A header row, whose cells are headings for their columns.
    pub header: bool,
    /// A footer row, at the table's end. A format with no footer group reads it as body.
    pub footer: bool,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub colspan: u32,
    pub rowspan: u32,
}

/// A footnote, with the anchor its references lead to and the anchor of its first reference,
/// to lead back.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub anchor: String,
    pub number: u32,
    pub content: Vec<Inline>,
    pub back: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Code(Vec<Inline>),
    /// Highlighted text.
    Mark(Vec<Inline>),
    Superscript(Vec<Inline>),
    Subscript(Vec<Inline>),
    Link {
        target: Link,
        content: Vec<Inline>,
    },
    /// A place a link may lead to.
    Anchor(String),
    /// A footnote's mark, leading to the note; `anchor` is the mark's own, for the note to lead
    /// back to.
    NoteRef {
        number: u32,
        note: String,
        anchor: Option<String>,
    },
    Image {
        /// An index into [`Book::assets`].
        asset: usize,
        alt: String,
    },
    LineBreak,
    /// Text marked as what it names, for a format that can show it so: only made when the
    /// options' [`crate::Format`] keeps it, and otherwise written as its content.
    Styled {
        style: InlineStyle,
        content: Vec<Inline>,
    },
}

/// What [`Inline::Styled`] text names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineStyle {
    /// A key to press.
    Keyboard,
    /// A menu, and the items to choose in it.
    Menu,
    /// A button to press.
    Button,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Internal(Target),
    External(String),
}

/// The inline content of `blocks` run together, a space between blocks: what a table cell
/// holds, as one line.
#[must_use]
pub fn inline_content(blocks: &[Block]) -> Vec<Inline> {
    let mut out = Vec::new();
    for block in blocks {
        let before = out.len();
        if before > 0 {
            out.push(Inline::Text(" ".to_owned()));
        }
        match block {
            Block::Heading { content, .. } | Block::Paragraph { content, .. } => {
                out.extend(content.iter().cloned());
            }
            Block::Preformatted { text, .. } => out.push(Inline::Text(text.replace('\n', " "))),
            Block::List { items, .. } => {
                for item in items {
                    out.extend(inline_content(item));
                }
            }
            Block::Definitions { items, .. } => {
                for (term, description) in items {
                    out.extend(term.iter().cloned());
                    out.push(Inline::Text(" ".to_owned()));
                    out.extend(inline_content(description));
                }
            }
            Block::Quote { blocks, .. } | Block::Aside { blocks, .. } => {
                out.extend(inline_content(blocks));
            }
            Block::Table { rows, .. } => {
                for cell in rows.iter().flat_map(|row| &row.cells) {
                    out.extend(inline_content(&cell.blocks));
                }
            }
            Block::Figure { alt, .. } => out.push(Inline::Text(alt.clone())),
            Block::Rule | Block::PageBreak | Block::Notes(_) => {}
        }
        if out.len() == before + 1 && before > 0 {
            out.pop();
        }
    }
    out
}

/// The text of `inlines` as a reader would read it aloud: no marks, an image's alternative
/// text, a footnote's number.
#[must_use]
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_text(inlines, &mut out);
    out
}

fn push_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(text),
            Inline::Strong(content)
            | Inline::Emphasis(content)
            | Inline::Code(content)
            | Inline::Mark(content)
            | Inline::Superscript(content)
            | Inline::Subscript(content)
            | Inline::Link { content, .. }
            | Inline::Styled { content, .. } => push_text(content, out),
            Inline::NoteRef { number, .. } => out.push_str(&number.to_string()),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::LineBreak => out.push('\n'),
            Inline::Anchor(_) => {}
        }
    }
}

impl Book {
    /// Every anchor the book defines, as `(chapter, anchor)`, in reading order.
    #[must_use]
    pub fn anchors(&self) -> Vec<(usize, String)> {
        let mut anchors = Vec::new();
        for (index, chapter) in self.chapters.iter().enumerate() {
            visit_blocks(&chapter.blocks, &mut |anchor| {
                anchors.push((index, anchor.to_owned()));
            });
        }
        anchors
    }

    /// Every internal link's target, in reading order.
    #[must_use]
    pub fn internal_links(&self) -> Vec<Target> {
        let mut targets = Vec::new();
        for chapter in &self.chapters {
            links_in_blocks(&chapter.blocks, &mut targets);
        }
        targets
    }
}

/// Calls `found` with every anchor `blocks` define, theirs and their inlines'.
fn visit_blocks(blocks: &[Block], found: &mut dyn FnMut(&str)) {
    for block in blocks {
        if let Some(anchor) = block.anchor() {
            found(anchor);
        }
        match block {
            Block::Heading { content, .. } | Block::Paragraph { content, .. } => {
                visit_inlines(content, found);
            }
            Block::List { items, .. } => {
                for item in items {
                    visit_blocks(item, found);
                }
            }
            Block::Definitions { items, .. } => {
                for (term, description) in items {
                    visit_inlines(term, found);
                    visit_blocks(description, found);
                }
            }
            Block::Quote { blocks, .. } | Block::Aside { blocks, .. } => {
                visit_blocks(blocks, found)
            }
            Block::Table { rows, .. } => {
                for cell in rows.iter().flat_map(|row| &row.cells) {
                    visit_blocks(&cell.blocks, found);
                }
            }
            Block::Notes(notes) => {
                for note in notes {
                    found(&note.anchor);
                    visit_inlines(&note.content, found);
                }
            }
            Block::Preformatted { .. } | Block::Figure { .. } | Block::Rule | Block::PageBreak => {}
        }
    }
}

fn visit_inlines(inlines: &[Inline], found: &mut dyn FnMut(&str)) {
    for inline in inlines {
        match inline {
            Inline::Anchor(anchor) => found(anchor),
            Inline::NoteRef {
                anchor: Some(anchor),
                ..
            } => found(anchor),
            Inline::Strong(content)
            | Inline::Emphasis(content)
            | Inline::Code(content)
            | Inline::Mark(content)
            | Inline::Superscript(content)
            | Inline::Subscript(content)
            | Inline::Link { content, .. }
            | Inline::Styled { content, .. } => visit_inlines(content, found),
            _ => {}
        }
    }
}

fn links_in_blocks(blocks: &[Block], targets: &mut Vec<Target>) {
    for block in blocks {
        match block {
            Block::Heading { content, .. } | Block::Paragraph { content, .. } => {
                links_in_inlines(content, targets);
            }
            Block::List { items, .. } => {
                for item in items {
                    links_in_blocks(item, targets);
                }
            }
            Block::Definitions { items, .. } => {
                for (term, description) in items {
                    links_in_inlines(term, targets);
                    links_in_blocks(description, targets);
                }
            }
            Block::Quote { blocks, .. } | Block::Aside { blocks, .. } => {
                links_in_blocks(blocks, targets);
            }
            Block::Table { rows, .. } => {
                for cell in rows.iter().flat_map(|row| &row.cells) {
                    links_in_blocks(&cell.blocks, targets);
                }
            }
            Block::Notes(notes) => {
                for note in notes {
                    links_in_inlines(&note.content, targets);
                }
            }
            Block::Preformatted { .. } | Block::Figure { .. } | Block::Rule | Block::PageBreak => {}
        }
    }
}

fn links_in_inlines(inlines: &[Inline], targets: &mut Vec<Target>) {
    for inline in inlines {
        match inline {
            Inline::Link {
                target: Link::Internal(target),
                content,
            } => {
                targets.push(target.clone());
                links_in_inlines(content, targets);
            }
            Inline::Strong(content)
            | Inline::Emphasis(content)
            | Inline::Code(content)
            | Inline::Mark(content)
            | Inline::Superscript(content)
            | Inline::Subscript(content)
            | Inline::Link { content, .. }
            | Inline::Styled { content, .. } => links_in_inlines(content, targets),
            _ => {}
        }
    }
}
