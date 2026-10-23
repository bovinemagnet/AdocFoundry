use std::{collections::BTreeMap, path::PathBuf};

use adoc_core::SourceRange;

/// Where a node came from: the page itself or a file included into it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceSpan {
    /// The file containing the content: the page, or the partial it was included from.
    pub path: PathBuf,
    /// Byte range in that file.
    pub range: SourceRange,
    /// One-based line of the start in that file.
    pub line: u32,
    /// Include targets as written, outermost first; empty for content of the page itself.
    pub include_chain: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dom {
    pub title: Option<Vec<Inline>>,
    /// Document attributes after the header and the caller's attributes.
    pub attributes: BTreeMap<String, String>,
    pub blocks: Vec<Block>,
    pub warnings: Vec<DomWarning>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DomWarning {
    pub message: String,
    pub span: Option<SourceSpan>,
}

/// What every block carries besides its content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockMeta {
    pub span: SourceSpan,
    pub id: Option<String>,
    pub roles: Vec<String>,
    pub title: Vec<Inline>,
    /// The block style, such as `source`, `quote` or a diagram kind.
    pub style: Option<String>,
    /// Named attributes from the block's attribute list.
    pub attributes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub meta: BlockMeta,
    pub kind: BlockKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKind {
    Unordered,
    Ordered,
    Checklist,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmonitionKind {
    Note,
    Tip,
    Important,
    Caution,
    Warning,
}

impl AdmonitionKind {
    #[must_use]
    pub fn from_style(style: &str) -> Option<Self> {
        Some(match style.to_ascii_uppercase().as_str() {
            "NOTE" => Self::Note,
            "TIP" => Self::Tip,
            "IMPORTANT" => Self::Important,
            "CAUTION" => Self::Caution,
            "WARNING" => Self::Warning,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Caution => "Caution",
            Self::Warning => "Warning",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Audio,
    Video,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub span: SourceSpan,
    /// For a checklist item, whether it is ticked.
    pub checked: Option<bool>,
    pub content: Vec<Inline>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DescriptionItem {
    pub span: SourceSpan,
    pub term: Vec<Inline>,
    pub content: Vec<Inline>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalloutItem {
    pub span: SourceSpan,
    pub number: usize,
    pub content: Vec<Inline>,
    pub blocks: Vec<Block>,
}

/// A callout marker in a listing: which line, which number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Callout {
    /// Zero-based line within the listing text.
    pub line: usize,
    pub number: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Align {
    Start,
    Centre,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellStyle {
    AsciiDoc,
    Default,
    Emphasis,
    Header,
    Literal,
    Monospace,
    Strong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColumnWidth {
    Proportional(u32),
    Percentage(u32),
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub width: ColumnWidth,
    pub align: Align,
    pub style: CellStyle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub colspan: usize,
    pub rowspan: usize,
    pub align: Option<Align>,
    pub style: Option<CellStyle>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Section {
        level: u8,
        title: Vec<Inline>,
        blocks: Vec<Block>,
    },
    /// A discrete heading: looks like a section title, starts no section.
    Heading {
        level: u8,
        title: Vec<Inline>,
    },
    Paragraph {
        inlines: Vec<Inline>,
    },
    List {
        kind: ListKind,
        items: Vec<ListItem>,
    },
    DescriptionList {
        items: Vec<DescriptionItem>,
    },
    CalloutList {
        items: Vec<CalloutItem>,
    },
    Admonition {
        kind: AdmonitionKind,
        blocks: Vec<Block>,
    },
    Listing {
        language: Option<String>,
        text: String,
        callouts: Vec<Callout>,
    },
    Literal {
        text: String,
    },
    Quote {
        attribution: Option<String>,
        citation: Option<String>,
        blocks: Vec<Block>,
    },
    Verse {
        attribution: Option<String>,
        citation: Option<String>,
        text: String,
    },
    Sidebar {
        blocks: Vec<Block>,
    },
    Example {
        blocks: Vec<Block>,
    },
    Open {
        blocks: Vec<Block>,
    },
    Table {
        columns: Vec<Column>,
        header: Option<Row>,
        rows: Vec<Row>,
        footer: Option<Row>,
    },
    Image {
        target: String,
        alt: String,
        width: Option<String>,
        height: Option<String>,
    },
    Media {
        kind: MediaKind,
        targets: Vec<String>,
    },
    /// A diagram block such as `[mermaid]`, kept as source for `adoc-diagrams`.
    Diagram {
        kind: String,
        source: String,
    },
    Stem {
        latex: bool,
        source: String,
    },
    Passthrough {
        content: String,
    },
    ThematicBreak,
    PageBreak,
    Toc,
    /// Something the model does not represent; kept with its source so nothing is lost.
    Unsupported {
        name: String,
        source: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Monospace(Vec<Inline>),
    /// Marked (highlighted) text.
    Highlight(Vec<Inline>),
    /// Text carrying roles, written `[.role]#text#`.
    Span {
        roles: Vec<String>,
        content: Vec<Inline>,
    },
    Superscript(Vec<Inline>),
    Subscript(Vec<Inline>),
    Link {
        target: String,
        text: Vec<Inline>,
    },
    Xref {
        target: String,
        text: Vec<Inline>,
        span: SourceSpan,
    },
    Anchor {
        id: String,
    },
    Image {
        target: String,
        alt: String,
    },
    Icon {
        name: String,
    },
    Footnote {
        id: Option<String>,
        number: u32,
        content: Vec<Inline>,
    },
    LineBreak,
    Keyboard(Vec<String>),
    Button(String),
    Menu(Vec<String>),
    Stem {
        latex: bool,
        source: String,
    },
    Passthrough(String),
    /// An index term: `((term))` is shown and indexed, `(((primary, secondary)))` and
    /// `indexterm:[…]` only indexed.
    IndexTerm {
        /// The primary term, then the secondary and tertiary ones, as plain text.
        terms: Vec<String>,
        /// What the text shows: the term of a flow term, nothing for a concealed one.
        visible: Vec<Inline>,
    },
}

impl Inline {
    /// The plain text of a run of inlines, without formatting.
    #[must_use]
    pub fn plain_text(inlines: &[Inline]) -> String {
        let mut text = String::new();
        for inline in inlines {
            inline.push_text(&mut text);
        }
        text
    }

    fn push_text(&self, out: &mut String) {
        match self {
            Self::Text(text) | Self::Passthrough(text) | Self::Button(text) => out.push_str(text),
            Self::Bold(content)
            | Self::Italic(content)
            | Self::Monospace(content)
            | Self::Highlight(content)
            | Self::Superscript(content)
            | Self::Subscript(content)
            | Self::Span { content, .. }
            | Self::Link { text: content, .. }
            | Self::Xref { text: content, .. }
            | Self::IndexTerm {
                visible: content, ..
            } => {
                for inline in content {
                    inline.push_text(out);
                }
            }
            Self::Image { alt, .. } => out.push_str(alt),
            Self::Icon { name } => out.push_str(name),
            Self::LineBreak => out.push('\n'),
            Self::Keyboard(keys) => out.push_str(&keys.join("+")),
            Self::Menu(items) => out.push_str(&items.join(" > ")),
            Self::Stem { source, .. } => out.push_str(source),
            Self::Anchor { .. } | Self::Footnote { .. } => {}
        }
    }
}

impl Block {
    /// The blocks directly inside this one, in order.
    #[must_use]
    pub fn children(&self) -> Vec<&Block> {
        match &self.kind {
            BlockKind::Section { blocks, .. }
            | BlockKind::Admonition { blocks, .. }
            | BlockKind::Quote { blocks, .. }
            | BlockKind::Sidebar { blocks }
            | BlockKind::Example { blocks }
            | BlockKind::Open { blocks } => blocks.iter().collect(),
            BlockKind::List { items, .. } => items.iter().flat_map(|item| &item.blocks).collect(),
            BlockKind::DescriptionList { items } => {
                items.iter().flat_map(|item| &item.blocks).collect()
            }
            BlockKind::CalloutList { items } => {
                items.iter().flat_map(|item| &item.blocks).collect()
            }
            BlockKind::Table {
                header,
                rows,
                footer,
                ..
            } => header
                .iter()
                .chain(rows)
                .chain(footer)
                .flat_map(|row| &row.cells)
                .flat_map(|cell| &cell.blocks)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A short name for the kind of block, for tests and diagnostics.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match &self.kind {
            BlockKind::Section { .. } => "section",
            BlockKind::Heading { .. } => "heading",
            BlockKind::Paragraph { .. } => "paragraph",
            BlockKind::List { kind, .. } => match kind {
                ListKind::Unordered => "unordered-list",
                ListKind::Ordered => "ordered-list",
                ListKind::Checklist => "checklist",
            },
            BlockKind::DescriptionList { .. } => "description-list",
            BlockKind::CalloutList { .. } => "callout-list",
            BlockKind::Admonition { .. } => "admonition",
            BlockKind::Listing { .. } => "listing",
            BlockKind::Literal { .. } => "literal",
            BlockKind::Quote { .. } => "quote",
            BlockKind::Verse { .. } => "verse",
            BlockKind::Sidebar { .. } => "sidebar",
            BlockKind::Example { .. } => "example",
            BlockKind::Open { .. } => "open",
            BlockKind::Table { .. } => "table",
            BlockKind::Image { .. } => "image",
            BlockKind::Media { .. } => "media",
            BlockKind::Diagram { .. } => "diagram",
            BlockKind::Stem { .. } => "stem",
            BlockKind::Passthrough { .. } => "passthrough",
            BlockKind::ThematicBreak => "thematic-break",
            BlockKind::PageBreak => "page-break",
            BlockKind::Toc => "toc",
            BlockKind::Unsupported { .. } => "unsupported",
        }
    }
}

impl Dom {
    /// Every block in document order, depth first.
    #[must_use]
    pub fn walk(&self) -> Vec<&Block> {
        fn visit<'a>(block: &'a Block, out: &mut Vec<&'a Block>) {
            out.push(block);
            for child in block.children() {
                visit(child, out);
            }
        }
        let mut out = Vec::new();
        for block in &self.blocks {
            visit(block, &mut out);
        }
        out
    }
}
