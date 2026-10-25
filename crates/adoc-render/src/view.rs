//! The view tree: the document model with references, images, diagrams, numbering and the table
//! of contents resolved, ready for any renderer. It knows nothing of a UI framework.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use adoc_core::{Location, SourceRange};
use adoc_dom::{
    AdmonitionKind, Align, Block, BlockKind, CellStyle, Column, Dom, Inline, ListKind, MediaKind,
    SourceSpan,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewTree {
    /// The page the tree was rendered from.
    pub source: PathBuf,
    pub title: Option<Vec<ViewInline>>,
    pub blocks: Vec<ViewBlock>,
    /// Present when the document asks for a table of contents (`:toc:`).
    pub toc: Vec<TocEntry>,
    pub footnotes: Vec<ViewFootnote>,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TocEntry {
    pub id: String,
    pub level: u8,
    pub number: Option<String>,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewFootnote {
    pub number: u32,
    pub content: Vec<ViewInline>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewBlock {
    pub span: SourceSpan,
    pub id: Option<String>,
    pub roles: Vec<String>,
    /// The block style, where it changes presentation: `qanda` and `horizontal` lists.
    pub style: Option<String>,
    pub title: Vec<ViewInline>,
    /// A numbered caption such as `Figure 1.` or `Table 2.`, for titled images, tables,
    /// examples and listings.
    pub caption: Option<String>,
    pub kind: ViewKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewListItem {
    pub span: SourceSpan,
    pub checked: Option<bool>,
    pub content: Vec<ViewInline>,
    pub blocks: Vec<ViewBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewDescription {
    pub span: SourceSpan,
    pub term: Vec<ViewInline>,
    pub content: Vec<ViewInline>,
    pub blocks: Vec<ViewBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewCallout {
    pub span: SourceSpan,
    pub number: usize,
    pub content: Vec<ViewInline>,
    pub blocks: Vec<ViewBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewCell {
    pub blocks: Vec<ViewBlock>,
    pub colspan: usize,
    pub rowspan: usize,
    pub align: Align,
    pub style: CellStyle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewTable {
    pub columns: Vec<Column>,
    pub header: Option<Vec<ViewCell>>,
    pub rows: Vec<Vec<ViewCell>>,
    pub footer: Option<Vec<ViewCell>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewImage {
    /// The target as written.
    pub target: String,
    /// The file it resolves to, when it is a local image that exists.
    pub path: Option<PathBuf>,
    /// A remote image URL.
    pub url: Option<String>,
    pub alt: String,
    pub width: Option<String>,
    pub height: Option<String>,
}

impl ViewImage {
    /// A local image that could not be found.
    #[must_use]
    pub fn is_missing(&self) -> bool {
        self.path.is_none() && self.url.is_none()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiagramView {
    Svg(PathBuf),
    /// Rendering is under way; show the source meanwhile.
    Pending {
        kind: String,
        source: String,
    },
    Failed {
        kind: String,
        message: String,
        source: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewKind {
    Section {
        level: u8,
        number: Option<String>,
        title: Vec<ViewInline>,
        blocks: Vec<ViewBlock>,
    },
    Heading {
        level: u8,
        title: Vec<ViewInline>,
    },
    Paragraph(Vec<ViewInline>),
    List {
        kind: ListKind,
        items: Vec<ViewListItem>,
    },
    DescriptionList(Vec<ViewDescription>),
    CalloutList(Vec<ViewCallout>),
    Admonition {
        kind: AdmonitionKind,
        blocks: Vec<ViewBlock>,
    },
    Code {
        language: Option<String>,
        text: String,
        callouts: Vec<adoc_dom::Callout>,
    },
    Literal(String),
    Quote {
        attribution: Option<String>,
        citation: Option<String>,
        blocks: Vec<ViewBlock>,
    },
    Verse {
        attribution: Option<String>,
        citation: Option<String>,
        text: String,
    },
    Sidebar(Vec<ViewBlock>),
    Example(Vec<ViewBlock>),
    Open(Vec<ViewBlock>),
    Table(ViewTable),
    Image(ViewImage),
    Media {
        kind: MediaKind,
        targets: Vec<String>,
    },
    Diagram(DiagramView),
    Stem {
        latex: bool,
        source: String,
        /// The formula typeset as an image, rendered like a diagram, for the preview and HTML.
        rendered: DiagramView,
    },
    Passthrough(String),
    ThematicBreak,
    PageBreak,
    Toc,
    Unsupported {
        name: String,
        source: String,
    },
}

/// Where a cross reference leads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedLink {
    /// The target page and range, when it resolves inside the workspace.
    pub target: Option<Location>,
    /// The anchor in the current page, for a local reference.
    pub fragment: Option<String>,
    /// Text to show when the reference has none of its own: the target's title.
    pub title: Option<String>,
    pub unresolved: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewInline {
    Text(String),
    Strong(Vec<ViewInline>),
    Emphasis(Vec<ViewInline>),
    Code(Vec<ViewInline>),
    Mark(Vec<ViewInline>),
    Span {
        roles: Vec<String>,
        content: Vec<ViewInline>,
    },
    Superscript(Vec<ViewInline>),
    Subscript(Vec<ViewInline>),
    Link {
        url: String,
        content: Vec<ViewInline>,
    },
    Xref {
        target: String,
        link: ResolvedLink,
        content: Vec<ViewInline>,
        span: SourceSpan,
    },
    Anchor(String),
    Image(ViewImage),
    Icon(String),
    FootnoteRef(u32),
    LineBreak,
    Keyboard(Vec<String>),
    Button(String),
    Menu(Vec<String>),
    Stem {
        latex: bool,
        source: String,
        /// The formula typeset as an image, with its measures, once it has been rendered.
        rendered: Option<crate::FormulaImage>,
    },
    Raw(String),
    /// An index term: its terms, primary first, and what the text shows (nothing for a
    /// concealed term).
    IndexTerm {
        terms: Vec<String>,
        content: Vec<ViewInline>,
    },
}

impl ViewInline {
    #[must_use]
    pub fn plain_text(inlines: &[ViewInline]) -> String {
        let mut out = String::new();
        for inline in inlines {
            inline.push_text(&mut out);
        }
        out
    }

    fn push_text(&self, out: &mut String) {
        match self {
            Self::Text(text) | Self::Raw(text) | Self::Button(text) | Self::Icon(text) => {
                out.push_str(text);
            }
            Self::Strong(content)
            | Self::Emphasis(content)
            | Self::Code(content)
            | Self::Mark(content)
            | Self::Superscript(content)
            | Self::Subscript(content)
            | Self::Span { content, .. }
            | Self::Link { content, .. }
            | Self::Xref { content, .. }
            | Self::IndexTerm { content, .. } => {
                for inline in content {
                    inline.push_text(out);
                }
            }
            Self::Image(image) => out.push_str(&image.alt),
            Self::FootnoteRef(number) => out.push_str(&format!("[{number}]")),
            Self::LineBreak => out.push('\n'),
            Self::Keyboard(keys) => out.push_str(&keys.join("+")),
            Self::Menu(items) => out.push_str(&items.join(" › ")),
            Self::Stem { source, .. } => out.push_str(source),
            Self::Anchor(_) => {}
        }
    }
}

/// Resolution the view needs from the workspace: cross-file references, images and diagrams.
pub trait ViewResolver {
    /// A cross reference to another file, written in `from`.
    fn xref(&self, from: &Path, target: &str) -> ResolvedLink;
    /// The image file a target names, given the document's `imagesdir`.
    fn image(&self, from: &Path, target: &str, imagesdir: Option<&str>) -> Option<PathBuf>;
    /// A diagram block; `attributes` are the block's own (format, theme and the like).
    fn diagram(
        &self,
        kind: &str,
        source: &str,
        attributes: &BTreeMap<String, String>,
    ) -> DiagramView;
}

/// The diagram kind a formula is rendered as: `stem-latex` or `stem-asciimath`, with
/// [`inline_stem_attributes`] for an inline formula.
#[must_use]
pub fn stem_kind(latex: bool) -> &'static str {
    if latex {
        "stem-latex"
    } else {
        "stem-asciimath"
    }
}

/// The attributes an inline formula is requested with: `inline`, and `baseline`, which asks
/// for the baseline recorded in the image. Both are part of the cache key, so an image cached
/// before baselines were recorded is typeset again.
#[must_use]
pub fn inline_stem_attributes() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("baseline".to_owned(), "true".to_owned()),
        ("inline".to_owned(), "true".to_owned()),
    ])
}

/// Resolves only what needs no workspace: images relative to the page, nothing else.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalResolver;

impl ViewResolver for LocalResolver {
    fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
        let file = target.split('#').next().unwrap_or("");
        let path = from.parent().unwrap_or_else(|| Path::new("")).join(file);
        ResolvedLink {
            unresolved: !path.is_file(),
            target: path.is_file().then(|| Location {
                path,
                range: SourceRange::default(),
            }),
            ..ResolvedLink::default()
        }
    }

    fn image(&self, from: &Path, target: &str, imagesdir: Option<&str>) -> Option<PathBuf> {
        let base = from.parent().unwrap_or_else(|| Path::new(""));
        let path = match imagesdir {
            Some(dir) => base.join(dir).join(target),
            None => base.join(target),
        };
        path.is_file().then_some(path)
    }

    fn diagram(
        &self,
        kind: &str,
        source: &str,
        _attributes: &BTreeMap<String, String>,
    ) -> DiagramView {
        DiagramView::Pending {
            kind: kind.to_owned(),
            source: source.to_owned(),
        }
    }
}

/// Builds the view tree for `dom`, rendered from the page at `source`.
#[must_use]
pub fn build_view(source: &Path, dom: &Dom, resolver: &dyn ViewResolver) -> ViewTree {
    let mut builder = Builder {
        source: source.to_path_buf(),
        resolver,
        ids: collect_ids(dom),
        imagesdir: dom.attributes.get("imagesdir").cloned(),
        numbered: dom.attributes.contains_key("sectnums"),
        counters: Vec::new(),
        numbering: Numbering::Numbered,
        saved_counters: None,
        appendices: 0,
        appendix_caption: dom
            .attributes
            .get("appendix-caption")
            .cloned()
            .unwrap_or_else(|| "Appendix".to_owned()),
        figures: 0,
        tables: 0,
        examples: 0,
        listings: 0,
        footnotes: Vec::new(),
        toc: Vec::new(),
        toc_levels: dom
            .attributes
            .get("toclevels")
            .and_then(|levels| levels.parse().ok())
            .unwrap_or(2),
    };
    let blocks = builder.blocks(&dom.blocks);
    let title = dom.title.as_ref().map(|title| builder.inlines(title));
    let toc = if dom.attributes.contains_key("toc") {
        std::mem::take(&mut builder.toc)
    } else {
        Vec::new()
    };
    ViewTree {
        source: source.to_path_buf(),
        title,
        blocks,
        toc,
        footnotes: builder.footnotes,
        attributes: dom.attributes.clone(),
    }
}

/// Every id in the document — sections, blocks and inline anchors — with the span and title of
/// what it identifies, for local references.
fn collect_ids(dom: &Dom) -> HashMap<String, (SourceSpan, Option<String>)> {
    fn inline_anchors(
        inlines: &[Inline],
        span: &SourceSpan,
        out: &mut HashMap<String, (SourceSpan, Option<String>)>,
    ) {
        for inline in inlines {
            if let Inline::Anchor { id } = inline {
                out.entry(id.clone()).or_insert((span.clone(), None));
            }
        }
    }
    let mut ids = HashMap::new();
    for block in dom.walk() {
        let title = match &block.kind {
            BlockKind::Section { title, .. } | BlockKind::Heading { title, .. } => {
                Some(Inline::plain_text(title))
            }
            _ if !block.meta.title.is_empty() => Some(Inline::plain_text(&block.meta.title)),
            _ => None,
        };
        if let Some(id) = &block.meta.id {
            ids.entry(id.clone())
                .or_insert((block.meta.span.clone(), title));
        }
        if let BlockKind::Paragraph { inlines } = &block.kind {
            inline_anchors(inlines, &block.meta.span, &mut ids);
        }
    }
    ids
}

struct Builder<'r> {
    source: PathBuf,
    resolver: &'r dyn ViewResolver,
    ids: HashMap<String, (SourceSpan, Option<String>)>,
    imagesdir: Option<String>,
    numbered: bool,
    counters: Vec<u32>,
    /// How the sections being built are numbered, from the special section they are in.
    numbering: Numbering,
    /// The other sections' count while an appendix counts its own.
    saved_counters: Option<Vec<u32>>,
    appendices: u32,
    /// `appendix-caption`: `Appendix` unless the page sets another.
    appendix_caption: String,
    figures: u32,
    tables: u32,
    examples: u32,
    listings: u32,
    footnotes: Vec<ViewFootnote>,
    toc: Vec<TocEntry>,
    toc_levels: u8,
}

/// The section styles Asciidoctor does not number, besides `appendix`, which it letters.
const SPECIAL_SECTIONS: &[&str] = &[
    "abstract",
    "acknowledgments",
    "bibliography",
    "colophon",
    "dedication",
    "glossary",
    "index",
    "partintro",
    "preface",
];

/// How sections are numbered where the builder is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Numbering {
    Numbered,
    /// Inside a special section.
    Unnumbered,
    /// Inside appendix `letter`, a section at `level`.
    Appendix {
        letter: char,
        level: u8,
    },
}

impl Builder<'_> {
    fn blocks(&mut self, blocks: &[Block]) -> Vec<ViewBlock> {
        blocks.iter().map(|block| self.block(block)).collect()
    }

    fn caption(&mut self, block: &Block) -> Option<String> {
        if block.meta.title.is_empty() {
            return None;
        }
        let (counter, label) = match &block.kind {
            BlockKind::Image { .. } => (&mut self.figures, "Figure"),
            BlockKind::Table { .. } => (&mut self.tables, "Table"),
            BlockKind::Example { .. } => (&mut self.examples, "Example"),
            BlockKind::Listing { .. } if block.meta.style.as_deref() == Some("source") => {
                return None;
            }
            _ => return None,
        };
        *counter += 1;
        Some(format!("{label} {counter}."))
    }

    /// The number of a section at `level` with block style `style`, and how the sections inside
    /// it are numbered, as Asciidoctor numbers them: special sections (a preface, glossary,
    /// bibliography and the like) and what they hold are not numbered, and an appendix is
    /// lettered (`Appendix A:`, then `A.1.`) outside the count of the other sections.
    fn section_number(&mut self, level: u8, style: Option<&str>) -> (Option<String>, Numbering) {
        if !self.numbered || level == 0 {
            return (None, self.numbering);
        }
        match self.numbering {
            Numbering::Unnumbered => return (None, Numbering::Unnumbered),
            Numbering::Appendix { letter, level: top } => {
                let depth = usize::from(level.saturating_sub(top));
                self.counters.resize(depth.max(1), 0);
                self.counters[depth.max(1) - 1] += 1;
                let parts: Vec<String> = self.counters.iter().map(ToString::to_string).collect();
                return (
                    Some(format!("{letter}.{}.", parts.join("."))),
                    self.numbering,
                );
            }
            Numbering::Numbered => {}
        }
        match style {
            Some("appendix") => {
                let letter = char::from(b'A' + (self.appendices % 26) as u8);
                self.appendices += 1;
                // The appendix's sections count from zero; the chapters' count resumes after.
                let saved = std::mem::take(&mut self.counters);
                self.saved_counters = Some(saved);
                let number = if self.appendix_caption.is_empty() {
                    format!("{letter}.")
                } else {
                    format!("{} {letter}:", self.appendix_caption)
                };
                (Some(number), Numbering::Appendix { letter, level })
            }
            Some(style) if SPECIAL_SECTIONS.contains(&style) => (None, Numbering::Unnumbered),
            _ => {
                let depth = usize::from(level);
                self.counters.resize(depth, 0);
                self.counters[depth - 1] += 1;
                let number = self
                    .counters
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(".")
                    + ".";
                (Some(number), Numbering::Numbered)
            }
        }
    }

    fn block(&mut self, block: &Block) -> ViewBlock {
        let caption = self.caption(block);
        let title = self.inlines(&block.meta.title);
        let kind = match &block.kind {
            BlockKind::Section {
                level,
                title,
                blocks,
            } => {
                let (number, inside) = self.section_number(*level, block.meta.style.as_deref());
                let title = self.inlines(title);
                if *level <= self.toc_levels {
                    if let Some(id) = &block.meta.id {
                        self.toc.push(TocEntry {
                            id: id.clone(),
                            level: *level,
                            number: number.clone(),
                            title: ViewInline::plain_text(&title),
                        });
                    }
                }
                let outside = std::mem::replace(&mut self.numbering, inside);
                let blocks = self.blocks(blocks);
                self.numbering = outside;
                // Leaving an appendix: the other sections' count resumes where it was.
                if matches!(inside, Numbering::Appendix { level: top, .. } if top == *level) {
                    if let Some(saved) = self.saved_counters.take() {
                        self.counters = saved;
                    }
                }
                ViewKind::Section {
                    level: *level,
                    number,
                    title,
                    blocks,
                }
            }
            BlockKind::Heading { level, title } => ViewKind::Heading {
                level: *level,
                title: self.inlines(title),
            },
            BlockKind::Paragraph { inlines } => ViewKind::Paragraph(self.inlines(inlines)),
            BlockKind::List { kind, items } => ViewKind::List {
                kind: *kind,
                items: items
                    .iter()
                    .map(|item| ViewListItem {
                        span: item.span.clone(),
                        checked: item.checked,
                        content: self.inlines(&item.content),
                        blocks: self.blocks(&item.blocks),
                    })
                    .collect(),
            },
            BlockKind::DescriptionList { items } => ViewKind::DescriptionList(
                items
                    .iter()
                    .map(|item| ViewDescription {
                        span: item.span.clone(),
                        term: self.inlines(&item.term),
                        content: self.inlines(&item.content),
                        blocks: self.blocks(&item.blocks),
                    })
                    .collect(),
            ),
            BlockKind::CalloutList { items } => ViewKind::CalloutList(
                items
                    .iter()
                    .map(|item| ViewCallout {
                        span: item.span.clone(),
                        number: item.number,
                        content: self.inlines(&item.content),
                        blocks: self.blocks(&item.blocks),
                    })
                    .collect(),
            ),
            BlockKind::Admonition { kind, blocks } => ViewKind::Admonition {
                kind: *kind,
                blocks: self.blocks(blocks),
            },
            BlockKind::Listing {
                language,
                text,
                callouts,
            } => {
                if !block.meta.title.is_empty() {
                    self.listings += 1;
                }
                ViewKind::Code {
                    language: language.clone(),
                    text: text.clone(),
                    callouts: callouts.clone(),
                }
            }
            BlockKind::Literal { text } => ViewKind::Literal(text.clone()),
            BlockKind::Quote {
                attribution,
                citation,
                blocks,
            } => ViewKind::Quote {
                attribution: attribution.clone(),
                citation: citation.clone(),
                blocks: self.blocks(blocks),
            },
            BlockKind::Verse {
                attribution,
                citation,
                text,
            } => ViewKind::Verse {
                attribution: attribution.clone(),
                citation: citation.clone(),
                text: text.clone(),
            },
            BlockKind::Sidebar { blocks } => ViewKind::Sidebar(self.blocks(blocks)),
            BlockKind::Example { blocks } => ViewKind::Example(self.blocks(blocks)),
            BlockKind::Open { blocks } => ViewKind::Open(self.blocks(blocks)),
            BlockKind::Table {
                columns,
                header,
                rows,
                footer,
            } => {
                let mut row = |row: &adoc_dom::Row| -> Vec<ViewCell> {
                    row.cells
                        .iter()
                        .enumerate()
                        .map(|(index, cell)| {
                            let column = columns.get(index);
                            ViewCell {
                                blocks: self.blocks(&cell.blocks),
                                colspan: cell.colspan,
                                rowspan: cell.rowspan,
                                align: cell
                                    .align
                                    .or(column.map(|column| column.align))
                                    .unwrap_or(Align::Start),
                                style: cell
                                    .style
                                    .or(column.map(|column| column.style))
                                    .unwrap_or(CellStyle::Default),
                            }
                        })
                        .collect()
                };
                let header = header.as_ref().map(&mut row);
                let rows = rows.iter().map(&mut row).collect();
                let footer = footer.as_ref().map(&mut row);
                ViewKind::Table(ViewTable {
                    columns: columns.clone(),
                    header,
                    rows,
                    footer,
                })
            }
            BlockKind::Image {
                target,
                alt,
                width,
                height,
            } => ViewKind::Image(self.image(target, alt, width.clone(), height.clone())),
            BlockKind::Media { kind, targets } => ViewKind::Media {
                kind: *kind,
                targets: targets.clone(),
            },
            BlockKind::Diagram { kind, source } => {
                ViewKind::Diagram(self.resolver.diagram(kind, source, &block.meta.attributes))
            }
            BlockKind::Stem { latex, source } => ViewKind::Stem {
                latex: *latex,
                source: source.clone(),
                rendered: self
                    .resolver
                    .diagram(stem_kind(*latex), source, &BTreeMap::new()),
            },
            BlockKind::Passthrough { content } => ViewKind::Passthrough(content.clone()),
            BlockKind::ThematicBreak => ViewKind::ThematicBreak,
            BlockKind::PageBreak => ViewKind::PageBreak,
            BlockKind::Toc => ViewKind::Toc,
            BlockKind::Unsupported { name, source } => ViewKind::Unsupported {
                name: name.clone(),
                source: source.clone(),
            },
        };
        ViewBlock {
            span: block.meta.span.clone(),
            id: block.meta.id.clone(),
            roles: block.meta.roles.clone(),
            style: block.meta.style.clone(),
            title,
            caption,
            kind,
        }
    }

    fn image(
        &self,
        target: &str,
        alt: &str,
        width: Option<String>,
        height: Option<String>,
    ) -> ViewImage {
        let remote = target.contains("://") || target.starts_with("data:");
        ViewImage {
            target: target.to_owned(),
            path: (!remote)
                .then(|| {
                    self.resolver
                        .image(&self.source, target, self.imagesdir.as_deref())
                })
                .flatten(),
            url: remote.then(|| target.to_owned()),
            alt: if alt.is_empty() {
                image_alt(target)
            } else {
                alt.to_owned()
            },
            width,
            height,
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) -> Vec<ViewInline> {
        inlines.iter().map(|inline| self.inline(inline)).collect()
    }

    fn inline(&mut self, inline: &Inline) -> ViewInline {
        match inline {
            Inline::Text(text) => ViewInline::Text(text.clone()),
            Inline::Bold(content) => ViewInline::Strong(self.inlines(content)),
            Inline::Italic(content) => ViewInline::Emphasis(self.inlines(content)),
            Inline::Monospace(content) => ViewInline::Code(self.inlines(content)),
            Inline::Highlight(content) => ViewInline::Mark(self.inlines(content)),
            Inline::Span { roles, content } => ViewInline::Span {
                roles: roles.clone(),
                content: self.inlines(content),
            },
            Inline::Superscript(content) => ViewInline::Superscript(self.inlines(content)),
            Inline::Subscript(content) => ViewInline::Subscript(self.inlines(content)),
            Inline::Link { target, text } => ViewInline::Link {
                url: target.clone(),
                content: if text.is_empty() {
                    vec![ViewInline::Text(target.clone())]
                } else {
                    self.inlines(text)
                },
            },
            Inline::Xref { target, text, span } => {
                let link = self.xref(&span.path, target);
                let content = if text.is_empty() {
                    let title = link.title.clone().unwrap_or_else(|| {
                        if link.unresolved {
                            target.clone()
                        } else {
                            target
                                .rsplit(['/', ':', '$', '#'])
                                .next()
                                .unwrap_or(target)
                                .trim_end_matches(".adoc")
                                .to_owned()
                        }
                    });
                    vec![ViewInline::Text(title)]
                } else {
                    self.inlines(text)
                };
                ViewInline::Xref {
                    target: target.clone(),
                    link,
                    content,
                    span: span.clone(),
                }
            }
            Inline::Anchor { id } => ViewInline::Anchor(id.clone()),
            Inline::Image { target, alt } => ViewInline::Image(self.image(target, alt, None, None)),
            Inline::Icon { name } => ViewInline::Icon(name.clone()),
            Inline::Footnote {
                number, content, ..
            } => {
                if !self.footnotes.iter().any(|note| note.number == *number) {
                    let content = self.inlines(content);
                    self.footnotes.push(ViewFootnote {
                        number: *number,
                        content,
                    });
                }
                ViewInline::FootnoteRef(*number)
            }
            Inline::LineBreak => ViewInline::LineBreak,
            Inline::Keyboard(keys) => ViewInline::Keyboard(keys.clone()),
            Inline::Button(label) => ViewInline::Button(label.clone()),
            Inline::Menu(items) => ViewInline::Menu(items.clone()),
            Inline::Stem { latex, source } => ViewInline::Stem {
                latex: *latex,
                source: source.clone(),
                rendered: match self.resolver.diagram(
                    stem_kind(*latex),
                    source,
                    &inline_stem_attributes(),
                ) {
                    DiagramView::Svg(path) => crate::formula_image(&path),
                    _ => None,
                },
            },
            Inline::Passthrough(text) => ViewInline::Raw(text.clone()),
            Inline::IndexTerm { terms, visible } => ViewInline::IndexTerm {
                terms: terms.clone(),
                content: self.inlines(visible),
            },
        }
    }

    /// A local reference (`<<id>>`, `xref:#id[]`, or an xref naming the page itself) resolves
    /// against the document; anything else goes to the resolver.
    fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
        let (file, fragment) = target
            .split_once('#')
            .map_or((target, None), |(file, fragment)| (file, Some(fragment)));
        let local_id = if file.is_empty() {
            fragment
        } else if !file.contains('.') && !file.contains('$') && !file.contains(':') {
            // `<<id>>` arrives as a bare id.
            Some(file)
        } else {
            None
        };
        if let Some(id) = local_id {
            return match self.ids.get(id) {
                Some((span, title)) => ResolvedLink {
                    target: Some(Location {
                        path: span.path.clone(),
                        range: span.range,
                    }),
                    fragment: Some(id.to_owned()),
                    title: title.clone(),
                    unresolved: false,
                },
                None => ResolvedLink {
                    fragment: Some(id.to_owned()),
                    unresolved: true,
                    ..ResolvedLink::default()
                },
            };
        }
        self.resolver.xref(from, target)
    }
}

/// Asciidoctor's default alt text: the file name without extension, separators as spaces.
fn image_alt(target: &str) -> String {
    let name = target.rsplit(['/', '$']).next().unwrap_or(target);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.replace(['-', '_'], " ")
}

/// The path, as child indices from the root, of the innermost block whose span in `path`
/// contains `offset`. Used to keep the preview level with the caret.
#[must_use]
pub fn block_at(view: &ViewTree, path: &Path, offset: usize) -> Option<Vec<usize>> {
    fn search(blocks: &[&ViewBlock], path: &Path, offset: usize, trail: &mut Vec<usize>) -> bool {
        for (index, block) in blocks.iter().enumerate() {
            trail.push(index);
            if search(&block.child_refs(), path, offset, trail) {
                return true;
            }
            let range = block.span.range;
            if block.span.path == path
                && range.start <= offset
                && offset < range.end.max(range.start + 1)
            {
                return true;
            }
            trail.pop();
        }
        false
    }
    let roots: Vec<&ViewBlock> = view.blocks.iter().collect();
    let mut trail = Vec::new();
    search(&roots, path, offset, &mut trail).then_some(trail)
}

/// The block at `trail`, as returned by [`block_at`].
#[must_use]
pub fn block_by_trail<'v>(view: &'v ViewTree, trail: &[usize]) -> Option<&'v ViewBlock> {
    let (first, rest) = trail.split_first()?;
    let mut block = view.blocks.get(*first)?;
    for index in rest {
        block = block.child_refs().into_iter().nth(*index)?;
    }
    Some(block)
}

impl ViewBlock {
    /// The blocks directly inside this one, in order (list items' and cells' blocks included).
    #[must_use]
    pub fn child_refs(&self) -> Vec<&ViewBlock> {
        match &self.kind {
            ViewKind::Section { blocks, .. }
            | ViewKind::Admonition { blocks, .. }
            | ViewKind::Quote { blocks, .. }
            | ViewKind::Sidebar(blocks)
            | ViewKind::Example(blocks)
            | ViewKind::Open(blocks) => blocks.iter().collect(),
            ViewKind::List { items, .. } => items.iter().flat_map(|item| &item.blocks).collect(),
            ViewKind::DescriptionList(items) => {
                items.iter().flat_map(|item| &item.blocks).collect()
            }
            ViewKind::CalloutList(items) => items.iter().flat_map(|item| &item.blocks).collect(),
            ViewKind::Table(table) => table
                .header
                .iter()
                .chain(&table.rows)
                .chain(&table.footer)
                .flatten()
                .flat_map(|cell| &cell.blocks)
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// The range of a block in its file, for click-to-source.
#[must_use]
pub fn source_location(block: &ViewBlock) -> Location {
    Location {
        path: block.span.path.clone(),
        range: block.span.range,
    }
}
