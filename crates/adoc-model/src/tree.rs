//! The editable tree: blocks with their exact source, and text blocks whose pieces tile the
//! source of their inline content.

use std::{collections::BTreeMap, path::PathBuf};

use adoc_core::SourceRange;
use adoc_dom::{AdmonitionKind, ListKind};

/// The child indices from the model's root to a node.
pub type BlockPath = Vec<usize>;

/// A page as the visual editor edits it.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// The page.
    pub path: PathBuf,
    /// The text the model describes.
    pub source: String,
    /// Document attributes, for the values of attribute references.
    pub attributes: BTreeMap<String, String>,
    pub blocks: Vec<Node>,
    /// The text blocks by where they start, for finding the one at an offset.
    pub(crate) text_index: crate::text::TextIndex,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    /// The node's whole source in the page, metadata lines included.
    pub range: SourceRange,
    pub access: Access,
    /// A title, a paragraph's text, a list item's principal text, a cell's text or code.
    pub text: Option<TextBlock>,
    pub children: Vec<Node>,
    /// The trail of the DOM block it was built from (as `adoc_dom::Block::children` counts),
    /// which is also its trail in a view tree built from the same DOM.
    pub origin: Option<Vec<usize>>,
    /// Where its text is in a view tree built from the same DOM, as the parser read it.
    pub view_text: Option<ViewText>,
}

/// Where a node's text is in a view tree built from the same DOM, for drawing it as the
/// parser read it when the model reads it otherwise ([`Node::drawn_from_view`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewText {
    /// The block at the node's `origin`: a paragraph, or a cell's paragraph.
    Block,
    /// The title of the section or discrete heading at the node's `origin`.
    Title,
    /// The principal text of item `index` of the list at `list`.
    Item { list: Vec<usize>, index: usize },
    /// The term of item `index` of the description list at `list`.
    Term { list: Vec<usize>, index: usize },
    /// The description of item `index` of the description list at `list`.
    Description { list: Vec<usize>, index: usize },
}

/// Whether a node may be edited in place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Access {
    Editable,
    ReadOnly(ReadOnly),
}

/// Why a node is shown but not edited.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadOnly {
    /// A construct the model keeps as source: diagrams, STEM, passthroughs and the like.
    Verbatim,
    /// Content from an included file.
    Included { path: PathBuf, target: String },
    /// A conditional directive, or a branch that does not apply.
    Inactive,
    /// Text the model's reading of disagrees with the parser's.
    Unchecked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowSection {
    Header,
    Body,
    Footer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeKind {
    /// The document title, `= Title`.
    Title,
    Section {
        level: u8,
    },
    /// A discrete heading.
    Heading {
        level: u8,
    },
    Paragraph,
    List {
        kind: ListKind,
    },
    ListItem {
        /// The marker as written, such as `*`, `**`, `-` or `.`.
        marker: String,
        checked: Option<bool>,
    },
    DescriptionList,
    DescriptionItem,
    /// A description list term.
    Term,
    /// A description list item's principal text.
    Description,
    Admonition {
        kind: AdmonitionKind,
        /// `[NOTE]` with a delimited block, rather than `NOTE:` before a paragraph.
        delimited: bool,
    },
    Code {
        language: Option<String>,
    },
    Literal,
    Quote,
    Sidebar,
    Example,
    Open,
    Table {
        columns: usize,
        /// Rows and columns can be inserted and deleted: no spans, one cell per column.
        regular: bool,
    },
    Row {
        section: RowSection,
    },
    Cell,
    Image {
        target: String,
        alt: String,
    },
    Include {
        target: String,
    },
    Conditional {
        directive: String,
    },
    /// Anything else, kept as source: the DOM's kind name.
    Verbatim {
        name: String,
    },
}

/// Inline content whose pieces tile its source exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// The inline content only: `Title` of `== Title`.
    pub range: SourceRange,
    pub pieces: Vec<Piece>,
    /// Code and literal text: no marks or atoms.
    pub plain: bool,
    /// The text as shown.
    pub display: String,
    /// What else the source around the text constrains.
    pub context: TextContext,
}

/// Where a text block sits, for what typed text may contain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextContext {
    /// Paragraphs, list items, descriptions: lines may break.
    Flow,
    /// Titles, headings and terms: one line.
    Line,
    /// A table cell: one line, and `|` must be escaped.
    Cell,
    /// Code and literal blocks: anything but the closing delimiter line.
    Verbatim,
}

/// A stretch of a text block.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// Shown exactly as written.
    Text { range: SourceRange },
    /// Formatting around other pieces; `open` and `close` are its delimiters (with any role
    /// prefix in `open`).
    Mark {
        mark: Mark,
        open: SourceRange,
        close: SourceRange,
        children: Vec<Piece>,
    },
    /// Shown as a unit and never split.
    Atom {
        atom: Atom,
        range: SourceRange,
        shown: String,
    },
}

impl Piece {
    /// The source the piece covers, delimiters included.
    #[must_use]
    pub fn range(&self) -> SourceRange {
        match self {
            Self::Text { range } | Self::Atom { range, .. } => *range,
            Self::Mark { open, close, .. } => SourceRange::new(open.start, close.end),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mark {
    Strong,
    Emphasis,
    Code,
    Highlight,
    Superscript,
    Subscript,
    /// Text carrying a role, `[.role]#text#`: no formatting of its own.
    Span,
}

impl Mark {
    pub const ALL: [Mark; 7] = [
        Mark::Strong,
        Mark::Emphasis,
        Mark::Code,
        Mark::Highlight,
        Mark::Superscript,
        Mark::Subscript,
        Mark::Span,
    ];

    const fn bit(self) -> u8 {
        1 << self as u8
    }

    /// The delimiter character.
    #[must_use]
    pub const fn delimiter(self) -> char {
        match self {
            Self::Strong => '*',
            Self::Emphasis => '_',
            Self::Code => '`',
            Self::Highlight | Self::Span => '#',
            Self::Superscript => '^',
            Self::Subscript => '~',
        }
    }

    /// Whether the mark has constrained and unconstrained forms.
    #[must_use]
    pub const fn has_forms(self) -> bool {
        !matches!(self, Self::Superscript | Self::Subscript)
    }
}

/// A set of marks.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MarkSet(u8);

impl MarkSet {
    pub const EMPTY: MarkSet = MarkSet(0);

    #[must_use]
    pub const fn contains(self, mark: Mark) -> bool {
        self.0 & mark.bit() != 0
    }

    #[must_use]
    pub const fn with(self, mark: Mark) -> Self {
        Self(self.0 | mark.bit())
    }

    #[must_use]
    pub const fn without(self, mark: Mark) -> Self {
        Self(self.0 & !mark.bit())
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Mark> {
        Mark::ALL
            .into_iter()
            .filter(move |mark| self.contains(*mark))
    }
}

impl FromIterator<Mark> for MarkSet {
    fn from_iter<I: IntoIterator<Item = Mark>>(marks: I) -> Self {
        marks.into_iter().fold(Self::EMPTY, Self::with)
    }
}

/// An inline the model shows but does not edit inside.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Atom {
    Xref {
        target: String,
        text: Option<String>,
    },
    Link {
        target: String,
        text: Option<String>,
    },
    Image {
        target: String,
        alt: String,
    },
    Footnote {
        id: Option<String>,
    },
    Anchor {
        id: String,
    },
    Attribute {
        name: String,
    },
    /// A typographic replacement: `(C)`, `--`, `...`, an apostrophe or an arrow.
    Replacement,
    /// A backslash and the character it keeps literal.
    Escape,
    Passthrough,
    /// A hard line break, ` +` at the end of a line.
    LineBreak,
    /// `kbd:`, `btn:`, `menu:`, `icon:` and STEM macros.
    Macro {
        name: String,
    },
    /// Curved quotes, ``"`text`"``.
    Quoted,
    IndexTerm,
}

impl Atom {
    /// Whether the atom is shown as text the parser also counts as text (and not, say, a
    /// footnote marker or a link label the model chose).
    pub(crate) fn dom_text<'s>(&'s self, shown: &'s str) -> &'s str {
        match self {
            Self::Xref { text, .. } | Self::Link { text, .. } => text.as_deref().unwrap_or(""),
            Self::Image { alt, .. } => alt,
            Self::Footnote { .. } | Self::Anchor { .. } | Self::LineBreak => "",
            Self::Attribute { .. }
            | Self::Replacement
            | Self::Escape
            | Self::Passthrough
            | Self::Macro { .. }
            | Self::Quoted
            | Self::IndexTerm => shown,
        }
    }
}

impl Node {
    #[must_use]
    pub fn is_editable(&self) -> bool {
        self.access == Access::Editable
    }

    /// Where to draw this node's text from instead of its own reading: set when that reading
    /// disagrees with the parser's (`ReadOnly::Unchecked`), so the text is shown as the page
    /// renders, not as the model misreads it. `None` draws the node's own text.
    #[must_use]
    pub fn drawn_from_view(&self) -> Option<&ViewText> {
        if self.access == Access::ReadOnly(ReadOnly::Unchecked) {
            self.view_text.as_ref()
        } else {
            None
        }
    }

    /// The node at `path` below this one.
    #[must_use]
    pub fn descendant(&self, path: &[usize]) -> Option<&Node> {
        let mut node = self;
        for index in path {
            node = node.children.get(*index)?;
        }
        Some(node)
    }
}

impl Model {
    /// The node at `path`.
    #[must_use]
    pub fn node(&self, path: &[usize]) -> Option<&Node> {
        let (first, rest) = path.split_first()?;
        self.blocks.get(*first)?.descendant(rest)
    }

    /// Every node in document order, depth first, with its path.
    #[must_use]
    pub fn walk(&self) -> Vec<(BlockPath, &Node)> {
        adoc_core::probe::hit(adoc_core::probe::Probe::Walk);
        fn visit<'m>(node: &'m Node, path: &mut BlockPath, out: &mut Vec<(BlockPath, &'m Node)>) {
            out.push((path.clone(), node));
            for (index, child) in node.children.iter().enumerate() {
                path.push(index);
                visit(child, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        let mut path = Vec::new();
        for (index, block) in self.blocks.iter().enumerate() {
            path.push(index);
            visit(block, &mut path, &mut out);
            path.pop();
        }
        out
    }

    /// The text blocks in document order, with the paths of the nodes holding them.
    pub fn text_blocks(&self) -> impl Iterator<Item = (BlockPath, &Node, &TextBlock)> {
        self.walk()
            .into_iter()
            .filter_map(|(path, node)| node.text.as_ref().map(|text| (path, node, text)))
    }
}
