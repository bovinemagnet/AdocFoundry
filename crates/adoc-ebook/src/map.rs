//! The view trees of an export mapped to a [`Book`]: chapters in the plan's order,
//! headings moved down by each page's depth, cross references made links inside the book
//! where their target is in it, and every id the pages define given a generated anchor.
//!
//! Anchors are named before anything is mapped, so a link may lead forward. A chapter is
//! `c<n>` in reading order; an id a page defines is `c<n>-a<m>`, numbered in the page's order;
//! a footnote is `c<n>-n<number>` and its first mark `c<n>-r<number>`. None depends on a path,
//! and no two pages' anchors can meet, whatever ids the pages chose.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use adoc_dom::SourceSpan;
use adoc_render::{DiagramView, ResolvedLink, ViewBlock, ViewInline, ViewKind, ViewListItem};

use crate::book::{
    Block, Book, Cell, Chapter, Inline, Link, Note, ParagraphClass, Row, Target, TocEntry,
};
use crate::{metadata, BookItem, Diagnostic, Document, Options, Page, Severity};

/// How many levels of a page's sections the table of contents lists below the page.
const TOC_SECTION_LEVELS: u8 = 2;

/// Maps `document` to a book, with what could not be kept as it was.
pub(crate) fn build(document: &Document, options: &Options) -> (Book, Vec<Diagnostic>) {
    let units = units(document);
    let labels = Labels::collect(&units);
    let pages: HashMap<PathBuf, usize> = units
        .iter()
        .enumerate()
        .filter_map(|(index, unit)| match unit {
            Unit::Page(page) => Some((page.view.source.clone(), index)),
            Unit::Title { .. } => None,
        })
        .collect();
    let mut mapper = Mapper {
        labels: &labels,
        pages: &pages,
        options,
        chapter: 0,
        source: PathBuf::new(),
        span: None,
        footnotes: BTreeMap::new(),
        notes_used: Vec::new(),
        diagnostics: Vec::new(),
        in_table: false,
        media: crate::media::Media::default(),
    };
    let mut chapters = Vec::new();
    let mut toc = Toc::default();
    for (index, unit) in units.iter().enumerate() {
        mapper.chapter = index;
        let id = chapter_id(index);
        match unit {
            Unit::Title { title, level } => {
                chapters.push(Chapter {
                    id,
                    title: title.clone(),
                    blocks: vec![Block::Heading {
                        level: (*level).clamp(1, 6),
                        anchor: None,
                        content: vec![Inline::Text(title.clone())],
                    }],
                    source: None,
                });
                toc.add(
                    *level,
                    title.clone(),
                    Target {
                        chapter: index,
                        anchor: None,
                    },
                );
            }
            Unit::Page(page) => {
                let chapter = mapper.page(page, index, &mut toc);
                chapters.push(chapter);
            }
        }
    }
    if chapters.is_empty() {
        chapters.push(Chapter {
            id: chapter_id(0),
            ..Chapter::default()
        });
    }
    let metadata = metadata::metadata(document, options);
    let cover = mapper.cover(document);
    let book = Book {
        metadata,
        chapters,
        toc: toc.finish(),
        assets: std::mem::take(&mut mapper.media.assets),
        cover,
    };
    (book, mapper.diagnostics)
}

fn chapter_id(index: usize) -> String {
    format!("c{}", index + 1)
}

/// One unit of the reading order, before mapping.
enum Unit<'a> {
    Page(&'a Page),
    /// A part's title (level 0 in the table of contents, a top heading in the text) or a
    /// heading of the navigation with no page of its own.
    Title {
        title: String,
        level: u8,
    },
}

impl Unit<'_> {
    fn toc_level(&self) -> u8 {
        match self {
            Self::Page(page) => page.depth.max(1),
            Self::Title { level, .. } => *level,
        }
    }
}

fn units(document: &Document) -> Vec<Unit<'_>> {
    match document {
        Document::Article(page) => vec![Unit::Page(page)],
        Document::Book(book) => book
            .items
            .iter()
            .map(|item| match item {
                BookItem::Part(title) => Unit::Title {
                    title: title.clone(),
                    level: 0,
                },
                BookItem::Heading { title, depth } => Unit::Title {
                    title: title.clone(),
                    level: (*depth).max(1),
                },
                BookItem::Page(page) => Unit::Page(page),
            })
            .collect(),
    }
}

/// The anchor of every id each page defines.
#[derive(Default)]
struct Labels {
    /// `(chapter, id)` to anchor.
    names: HashMap<(usize, String), String>,
    /// How many each chapter has named.
    counts: HashMap<usize, usize>,
}

impl Labels {
    fn collect(units: &[Unit<'_>]) -> Self {
        let mut labels = Self::default();
        for (index, unit) in units.iter().enumerate() {
            if let Unit::Page(page) = unit {
                labels.blocks(index, &page.view.blocks);
            }
        }
        labels
    }

    fn add(&mut self, chapter: usize, id: &str) {
        if self.names.contains_key(&(chapter, id.to_owned())) {
            return;
        }
        let count = self.counts.entry(chapter).or_default();
        *count += 1;
        let anchor = format!("{}-a{count}", chapter_id(chapter));
        self.names.insert((chapter, id.to_owned()), anchor);
    }

    fn get(&self, chapter: usize, id: &str) -> Option<&str> {
        self.names
            .get(&(chapter, id.to_owned()))
            .map(String::as_str)
    }

    fn inlines(&mut self, chapter: usize, inlines: &[ViewInline]) {
        for inline in inlines {
            match inline {
                ViewInline::Anchor(id) => self.add(chapter, id),
                ViewInline::Strong(content)
                | ViewInline::Emphasis(content)
                | ViewInline::Code(content)
                | ViewInline::Mark(content)
                | ViewInline::Superscript(content)
                | ViewInline::Subscript(content)
                | ViewInline::Span { content, .. }
                | ViewInline::Link { content, .. }
                | ViewInline::Xref { content, .. }
                | ViewInline::IndexTerm { content, .. } => self.inlines(chapter, content),
                _ => {}
            }
        }
    }

    fn blocks(&mut self, chapter: usize, blocks: &[ViewBlock]) {
        for block in blocks {
            if let Some(id) = &block.id {
                self.add(chapter, id);
            }
            self.inlines(chapter, &block.title);
            match &block.kind {
                ViewKind::Section { title, blocks, .. } => {
                    self.inlines(chapter, title);
                    self.blocks(chapter, blocks);
                }
                ViewKind::Heading { title, .. } => self.inlines(chapter, title),
                ViewKind::Paragraph(content) => self.inlines(chapter, content),
                ViewKind::List { items, .. } => {
                    for item in items {
                        self.inlines(chapter, &item.content);
                        self.blocks(chapter, &item.blocks);
                    }
                }
                ViewKind::DescriptionList(items) => {
                    for item in items {
                        self.inlines(chapter, &item.term);
                        self.inlines(chapter, &item.content);
                        self.blocks(chapter, &item.blocks);
                    }
                }
                ViewKind::CalloutList(items) => {
                    for item in items {
                        self.inlines(chapter, &item.content);
                        self.blocks(chapter, &item.blocks);
                    }
                }
                ViewKind::Admonition { blocks, .. }
                | ViewKind::Quote { blocks, .. }
                | ViewKind::Sidebar(blocks)
                | ViewKind::Example(blocks)
                | ViewKind::Open(blocks) => self.blocks(chapter, blocks),
                ViewKind::Table(table) => {
                    for cell in table
                        .header
                        .iter()
                        .flatten()
                        .chain(table.rows.iter().flatten())
                        .chain(table.footer.iter().flatten())
                    {
                        self.blocks(chapter, &cell.blocks);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The table of contents, built in reading order from each entry's level.
#[derive(Default)]
struct Toc {
    /// Entries and their levels, flat.
    flat: Vec<(u8, TocEntry)>,
}

impl Toc {
    fn add(&mut self, level: u8, title: String, target: Target) {
        self.flat.push((
            level,
            TocEntry {
                title,
                target,
                children: Vec::new(),
            },
        ));
    }

    /// Nests each entry under the nearest earlier one of a lower level.
    fn finish(self) -> Vec<TocEntry> {
        let mut roots: Vec<TocEntry> = Vec::new();
        // The open entries, as (level, path of child indexes from the roots).
        let mut open: Vec<(u8, Vec<usize>)> = Vec::new();
        for (level, entry) in self.flat {
            while open
                .last()
                .is_some_and(|(open_level, _)| *open_level >= level)
            {
                open.pop();
            }
            let path = match open.last() {
                Some((_, parent_path)) => {
                    let parent = entry_at(&mut roots, parent_path);
                    parent.children.push(entry);
                    let mut path = parent_path.clone();
                    path.push(parent.children.len() - 1);
                    path
                }
                None => {
                    roots.push(entry);
                    vec![roots.len() - 1]
                }
            };
            open.push((level, path));
        }
        roots
    }
}

fn entry_at<'a>(roots: &'a mut [TocEntry], path: &[usize]) -> &'a mut TocEntry {
    let (first, rest) = path.split_first().expect("a path is never empty");
    let mut entry = &mut roots[*first];
    for index in rest {
        entry = &mut entry.children[*index];
    }
    entry
}

struct Mapper<'a> {
    labels: &'a Labels,
    /// The chapter each page of the book is.
    pages: &'a HashMap<PathBuf, usize>,
    options: &'a Options,
    chapter: usize,
    /// The page being mapped.
    source: PathBuf,
    /// The span of the block being mapped, for diagnostics about its inlines.
    span: Option<SourceSpan>,
    /// The page's footnotes, by number.
    footnotes: BTreeMap<u32, Vec<ViewInline>>,
    /// The footnotes the page's marks used, in order, with the first mark's anchor.
    notes_used: Vec<(u32, Option<String>)>,
    diagnostics: Vec<Diagnostic>,
    /// Whether the blocks being mapped are in a table's cell.
    in_table: bool,
    media: crate::media::Media,
}

impl Mapper<'_> {
    /// The cover: the export's own picture, else the document's `front-cover-image` (a book's
    /// from its first page, which is read with the book's attributes).
    fn cover(&mut self, document: &Document) -> Option<usize> {
        self.span = None;
        if let Some(path) = self.options.cover.clone() {
            return match self.load(&path) {
                Ok(asset) => Some(asset),
                Err(reason) => {
                    self.warn(None, format!("the cover {} {reason}", path.display()));
                    None
                }
            };
        }
        let first = match document {
            Document::Article(page) => Some(page),
            Document::Book(book) => book.items.iter().find_map(|item| match item {
                BookItem::Page(page) => Some(page),
                _ => None,
            }),
        }?;
        let image = first
            .view
            .attribute_images
            .get("front-cover-image")?
            .clone();
        match &image.path {
            Some(path) => match self.load(path) {
                Ok(asset) => Some(asset),
                Err(reason) => {
                    self.warn(None, format!("the cover {} {reason}", image.target));
                    None
                }
            },
            None => {
                self.warn(
                    None,
                    format!(
                        "the cover {} cannot be found or is remote, and is left out",
                        image.target
                    ),
                );
                None
            }
        }
    }

    fn warn(&mut self, span: Option<&SourceSpan>, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message,
            source: span.or(self.span.as_ref()).cloned(),
        });
    }

    fn page(&mut self, page: &Page, index: usize, toc: &mut Toc) -> Chapter {
        self.source = page.view.source.clone();
        self.span = None;
        self.footnotes = page
            .view
            .footnotes
            .iter()
            .map(|note| (note.number, note.content.clone()))
            .collect();
        self.notes_used.clear();
        let depth = Unit::Page(page).toc_level();
        let title_inlines = page.view.title.clone().unwrap_or_default();
        let mut blocks = Vec::new();
        let title = ViewInline::plain_text(&title_inlines);
        let title = if title.trim().is_empty() {
            page.view
                .source
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            title
        };
        if !title_inlines.is_empty() {
            let content = self.inlines(&title_inlines);
            blocks.push(Block::Heading {
                level: depth.clamp(1, 6),
                anchor: None,
                content,
            });
        }
        toc.add(
            depth,
            title.clone(),
            Target {
                chapter: index,
                anchor: None,
            },
        );
        let mut sections = Vec::new();
        self.blocks(&page.view.blocks, depth, &mut blocks, &mut sections);
        for (level, title, anchor) in sections {
            toc.add(
                depth.saturating_add(level),
                title,
                Target {
                    chapter: index,
                    anchor: Some(anchor),
                },
            );
        }
        let notes = self.notes();
        if !notes.is_empty() {
            blocks.push(Block::Notes(notes));
        }
        Chapter {
            id: chapter_id(index),
            title,
            blocks,
            source: Some(page.view.source.clone()),
        }
    }

    /// The notes the page's marks used, in the order first used.
    fn notes(&mut self) -> Vec<Note> {
        let used = std::mem::take(&mut self.notes_used);
        let mut notes = Vec::new();
        for (number, back) in used {
            let content = self.footnotes.get(&number).cloned().unwrap_or_default();
            let content = self.inlines(&content);
            notes.push(Note {
                anchor: self.note_anchor(number),
                number,
                content,
                back,
            });
        }
        notes
    }

    fn note_anchor(&self, number: u32) -> String {
        format!("{}-n{number}", chapter_id(self.chapter))
    }

    fn anchor(&self, id: Option<&String>) -> Option<String> {
        id.and_then(|id| self.labels.get(self.chapter, id))
            .map(str::to_owned)
    }

    /// Maps `blocks`, collecting the sections the table of contents lists as
    /// `(level, title, anchor)`.
    fn blocks(
        &mut self,
        blocks: &[ViewBlock],
        depth: u8,
        out: &mut Vec<Block>,
        sections: &mut Vec<(u8, String, String)>,
    ) {
        for block in blocks {
            self.block(block, depth, out, sections);
        }
    }

    fn nested(&mut self, blocks: &[ViewBlock], depth: u8) -> Vec<Block> {
        let mut out = Vec::new();
        let mut sections = Vec::new();
        self.blocks(blocks, depth, &mut out, &mut sections);
        out
    }

    #[allow(clippy::too_many_lines)]
    fn block(
        &mut self,
        block: &ViewBlock,
        depth: u8,
        out: &mut Vec<Block>,
        sections: &mut Vec<(u8, String, String)>,
    ) {
        self.span = Some(block.span.clone());
        let anchor = self.anchor(block.id.as_ref());
        // A block's title reads as a caption before it, except a section's, which is its
        // heading.
        if !block.title.is_empty() && !matches!(block.kind, ViewKind::Section { .. }) {
            let mut content = Vec::new();
            if let Some(caption) = &block.caption {
                content.push(Inline::Text(format!("{caption} ")));
            }
            content.extend(self.inlines(&block.title));
            out.push(Block::Paragraph {
                anchor: None,
                class: ParagraphClass::Caption,
                content,
            });
        }
        match &block.kind {
            ViewKind::Section {
                level,
                number,
                title,
                blocks,
            } => {
                let mut content = Vec::new();
                if let Some(number) = number {
                    content.push(Inline::Text(format!("{number} ")));
                }
                content.extend(self.inlines(title));
                let text = crate::book::plain_text(&content);
                if let (Some(anchor), true) = (&anchor, *level <= TOC_SECTION_LEVELS) {
                    sections.push((*level, text, anchor.clone()));
                }
                out.push(Block::Heading {
                    level: depth.saturating_add(*level).clamp(1, 6),
                    anchor,
                    content,
                });
                self.blocks(blocks, depth, out, sections);
            }
            ViewKind::Heading { level, title } => {
                let content = self.inlines(title);
                out.push(Block::Heading {
                    level: depth.saturating_add(*level).clamp(1, 6),
                    anchor,
                    content,
                });
            }
            ViewKind::Paragraph(content) => {
                let content = self.inlines(content);
                out.push(Block::Paragraph {
                    anchor,
                    class: ParagraphClass::Normal,
                    content,
                });
            }
            ViewKind::List { kind, items } => {
                let ordered = matches!(kind, adoc_dom::ListKind::Ordered);
                let items = items
                    .iter()
                    .map(|item| self.list_item(item, depth))
                    .collect();
                out.push(Block::List {
                    anchor,
                    ordered,
                    items,
                });
            }
            ViewKind::DescriptionList(items) => {
                let items = items
                    .iter()
                    .map(|item| {
                        let term = self.inlines(&item.term);
                        let mut description = Vec::new();
                        if !item.content.is_empty() {
                            description.push(Block::Paragraph {
                                anchor: None,
                                class: ParagraphClass::Normal,
                                content: self.inlines(&item.content),
                            });
                        }
                        description.extend(self.nested(&item.blocks, depth));
                        (term, description)
                    })
                    .collect();
                out.push(Block::Definitions { anchor, items });
            }
            ViewKind::CalloutList(items) => {
                let items = items
                    .iter()
                    .map(|item| {
                        let mut blocks = vec![Block::Paragraph {
                            anchor: None,
                            class: ParagraphClass::Normal,
                            content: self.inlines(&item.content),
                        }];
                        blocks.extend(self.nested(&item.blocks, depth));
                        blocks
                    })
                    .collect();
                out.push(Block::List {
                    anchor,
                    ordered: true,
                    items,
                });
            }
            ViewKind::Admonition { kind, blocks } => {
                let mut inner = vec![Block::Paragraph {
                    anchor: None,
                    class: ParagraphClass::Label,
                    content: vec![Inline::Strong(vec![Inline::Text(
                        admonition_label(*kind).to_owned(),
                    )])],
                }];
                inner.extend(self.nested(blocks, depth));
                out.push(Block::Aside {
                    anchor,
                    blocks: inner,
                });
            }
            ViewKind::Code { text, .. } => out.push(Block::Preformatted {
                anchor,
                text: text.clone(),
                code: true,
            }),
            ViewKind::Literal(text) => out.push(Block::Preformatted {
                anchor,
                text: text.clone(),
                code: true,
            }),
            ViewKind::Quote {
                attribution,
                citation,
                blocks,
            } => {
                let mut inner = self.nested(blocks, depth);
                inner.extend(attribution_paragraph(
                    attribution.as_deref(),
                    citation.as_deref(),
                ));
                out.push(Block::Quote {
                    anchor,
                    blocks: inner,
                });
            }
            ViewKind::Verse {
                attribution,
                citation,
                text,
            } => {
                let mut inner = vec![Block::Preformatted {
                    anchor: None,
                    text: text.clone(),
                    code: false,
                }];
                inner.extend(attribution_paragraph(
                    attribution.as_deref(),
                    citation.as_deref(),
                ));
                out.push(Block::Quote {
                    anchor,
                    blocks: inner,
                });
            }
            ViewKind::Sidebar(blocks) | ViewKind::Example(blocks) => {
                let blocks = self.nested(blocks, depth);
                out.push(Block::Aside { anchor, blocks });
            }
            ViewKind::Open(blocks) => {
                if let Some(anchor) = anchor {
                    out.push(Block::Paragraph {
                        anchor: Some(anchor),
                        class: ParagraphClass::Normal,
                        content: Vec::new(),
                    });
                }
                self.blocks(blocks, depth, out, sections);
            }
            ViewKind::Table(table) if self.in_table => {
                // A table in a table's cell: few readers lay one out, so each row is a line of
                // text, its cells separated by bars (Kindle PRD §15, "Tables").
                self.warn(
                    None,
                    "a table inside a table cell is exported as lines of text".to_owned(),
                );
                let rows = table
                    .header
                    .iter()
                    .chain(table.rows.iter())
                    .chain(table.footer.iter());
                let mut first = true;
                for row in rows {
                    let mut content = Vec::new();
                    for (index, cell) in row.iter().enumerate() {
                        if index > 0 {
                            content.push(Inline::Text(" | ".to_owned()));
                        }
                        let blocks = self.nested(&cell.blocks, depth);
                        content.extend(crate::book::inline_content(&blocks));
                    }
                    out.push(Block::Paragraph {
                        anchor: if first { anchor.clone() } else { None },
                        class: ParagraphClass::Normal,
                        content,
                    });
                    first = false;
                }
                if first {
                    keep_anchor(out, anchor);
                }
            }
            ViewKind::Table(table) => {
                let mut rows = Vec::new();
                let was_in_table = std::mem::replace(&mut self.in_table, true);
                let mut push = |mapper: &mut Self, cells: &[adoc_render::ViewCell], header| {
                    let cells = cells
                        .iter()
                        .map(|cell| Cell {
                            blocks: mapper.nested(&cell.blocks, depth),
                            colspan: u32::try_from(cell.colspan.max(1)).unwrap_or(u32::MAX),
                            rowspan: u32::try_from(cell.rowspan.max(1)).unwrap_or(u32::MAX),
                        })
                        .collect();
                    rows.push(Row { header, cells });
                };
                if let Some(header) = &table.header {
                    push(self, header, true);
                }
                for row in &table.rows {
                    push(self, row, false);
                }
                if let Some(footer) = &table.footer {
                    push(self, footer, false);
                }
                self.in_table = was_in_table;
                out.push(Block::Table { anchor, rows });
            }
            ViewKind::Image(image) => {
                let alt = alt_of(image);
                match self.picture(image) {
                    Some(asset) => out.push(Block::Figure { anchor, asset, alt }),
                    None => out.push(Block::Paragraph {
                        anchor,
                        class: ParagraphClass::Caption,
                        content: self.picture_fallback(image, alt),
                    }),
                }
            }
            ViewKind::Media { kind, targets } => {
                let kind = match kind {
                    adoc_dom::MediaKind::Audio => "audio",
                    adoc_dom::MediaKind::Video => "video",
                };
                self.warn(
                    None,
                    format!("a Kindle book cannot play {kind}; it is exported as its address"),
                );
                let content = targets
                    .iter()
                    .map(|target| self.external_or_text(target, &[]))
                    .collect();
                out.push(Block::Paragraph {
                    anchor,
                    class: ParagraphClass::Normal,
                    content,
                });
            }
            ViewKind::Diagram(diagram) => {
                match diagram {
                    DiagramView::Svg(path) => {
                        let alt = ViewInline::plain_text(&block.title);
                        let alt = if alt.trim().is_empty() {
                            "Diagram".to_owned()
                        } else {
                            alt
                        };
                        match self.load(path) {
                            Ok(asset) => out.push(Block::Figure { anchor, asset, alt }),
                            Err(reason) => {
                                self.warn(None, format!("the diagram {reason} and is left out"));
                                out.push(Block::Paragraph {
                                    anchor,
                                    class: ParagraphClass::Caption,
                                    content: vec![Inline::Text(alt)],
                                });
                            }
                        }
                    }
                    // A failed diagram was reported with the export's other diagrams; one still
                    // pending when the book is written is reported here.
                    DiagramView::Pending { kind, source }
                    | DiagramView::Failed { kind, source, .. } => {
                        if matches!(diagram, DiagramView::Pending { .. }) {
                            self.warn(
                            None,
                            format!("the {kind} diagram was not drawn and is exported as its source"),
                        );
                        }
                        out.push(Block::Preformatted {
                            anchor,
                            text: source.clone(),
                            code: true,
                        });
                    }
                }
            }
            ViewKind::Stem {
                source, rendered, ..
            } => {
                let drawn = match rendered {
                    DiagramView::Svg(path) => self.load(path).ok(),
                    _ => None,
                };
                match drawn {
                    Some(asset) => out.push(Block::Figure {
                        anchor,
                        asset,
                        alt: source.clone(),
                    }),
                    None => {
                        self.warn(
                            None,
                            "the formula is not drawn and is exported as its source".to_owned(),
                        );
                        out.push(Block::Preformatted {
                            anchor,
                            text: source.clone(),
                            code: true,
                        });
                    }
                }
            }
            ViewKind::Passthrough(text) => {
                self.warn(
                    None,
                    "passthrough content is exported as text, not as markup".to_owned(),
                );
                out.push(Block::Preformatted {
                    anchor,
                    text: text.clone(),
                    code: false,
                });
            }
            ViewKind::ThematicBreak => {
                keep_anchor(out, anchor);
                out.push(Block::Rule);
            }
            ViewKind::PageBreak => {
                keep_anchor(out, anchor);
                out.push(Block::PageBreak);
            }
            // The reader shows its own table of contents.
            ViewKind::Toc => keep_anchor(out, anchor),
            ViewKind::Unsupported { name, source } => {
                self.warn(
                    None,
                    format!("the {name} block is not supported and is exported as its source"),
                );
                out.push(Block::Preformatted {
                    anchor,
                    text: source.clone(),
                    code: true,
                });
            }
        }
    }

    /// The picture a local image shows, embedded; `None`, with a warning, where it cannot be.
    fn picture(&mut self, image: &adoc_render::ViewImage) -> Option<usize> {
        let path = image.path.as_ref()?;
        match self.load(path) {
            Ok(asset) => Some(asset),
            Err(reason) => {
                self.warn(
                    None,
                    format!(
                        "the image {} {reason} and is exported as its description",
                        image.target
                    ),
                );
                None
            }
        }
    }

    fn load(&mut self, path: &std::path::Path) -> Result<usize, String> {
        self.media.load(path, &self.options.access)
    }

    /// What an image that is not embedded becomes: its description, and a link to a remote
    /// one, which is never fetched; each with a warning.
    fn picture_fallback(&mut self, image: &adoc_render::ViewImage, alt: String) -> Vec<Inline> {
        if let Some(url) = &image.url {
            self.warn(
                None,
                format!("the remote image {url} is not fetched and is exported as a link to it"),
            );
            return vec![self.external_or_text(url, &[ViewInline::Text(alt)])];
        }
        if image.path.is_none() {
            self.warn(
                None,
                format!(
                    "the image {} cannot be found and is exported as its description",
                    image.target
                ),
            );
        }
        vec![Inline::Text(alt)]
    }

    fn list_item(&mut self, item: &ViewListItem, depth: u8) -> Vec<Block> {
        self.span = Some(item.span.clone());
        let mut content = Vec::new();
        if let Some(checked) = item.checked {
            content.push(Inline::Text(if checked { "☑ " } else { "☐ " }.to_owned()));
        }
        content.extend(self.inlines(&item.content));
        let mut blocks = Vec::new();
        if !content.is_empty() {
            blocks.push(Block::Paragraph {
                anchor: None,
                class: ParagraphClass::Normal,
                content,
            });
        }
        blocks.extend(self.nested(&item.blocks, depth));
        blocks
    }

    fn inlines(&mut self, inlines: &[ViewInline]) -> Vec<Inline> {
        let mut out = Vec::new();
        for inline in inlines {
            self.inline(inline, &mut out);
        }
        out
    }

    fn inline(&mut self, inline: &ViewInline, out: &mut Vec<Inline>) {
        match inline {
            ViewInline::Text(text) | ViewInline::Raw(text) => out.push(Inline::Text(text.clone())),
            ViewInline::Strong(content) => out.push(Inline::Strong(self.inlines(content))),
            ViewInline::Emphasis(content) => out.push(Inline::Emphasis(self.inlines(content))),
            ViewInline::Code(content) => out.push(Inline::Code(self.inlines(content))),
            ViewInline::Mark(content) => out.push(Inline::Mark(self.inlines(content))),
            ViewInline::Superscript(content) => {
                out.push(Inline::Superscript(self.inlines(content)));
            }
            ViewInline::Subscript(content) => out.push(Inline::Subscript(self.inlines(content))),
            ViewInline::Span { content, .. } | ViewInline::IndexTerm { content, .. } => {
                out.extend(self.inlines(content));
            }
            ViewInline::Link { url, content } => out.push(self.external_or_text(url, content)),
            ViewInline::Xref {
                target,
                link,
                content,
                span,
            } => out.push(self.xref(target, link, content, span)),
            ViewInline::Anchor(id) => {
                if let Some(anchor) = self.labels.get(self.chapter, id) {
                    out.push(Inline::Anchor(anchor.to_owned()));
                }
            }
            ViewInline::Image(image) => {
                let alt = alt_of(image);
                match self.picture(image) {
                    Some(asset) => out.push(Inline::Image { asset, alt }),
                    None => out.extend(self.picture_fallback(image, alt)),
                }
            }
            ViewInline::Icon(name) => out.push(Inline::Text(format!("[{name}]"))),
            ViewInline::FootnoteRef { number, repeat, .. } => {
                let first = !self.notes_used.iter().any(|(used, _)| used == number);
                let anchor =
                    (first && !repeat).then(|| format!("{}-r{number}", chapter_id(self.chapter)));
                if first {
                    self.notes_used.push((*number, anchor.clone()));
                }
                out.push(Inline::NoteRef {
                    number: *number,
                    note: self.note_anchor(*number),
                    anchor,
                });
            }
            ViewInline::LineBreak => out.push(Inline::LineBreak),
            ViewInline::Keyboard(keys) => {
                out.push(Inline::Code(vec![Inline::Text(keys.join("+"))]));
            }
            ViewInline::Button(text) => {
                out.push(Inline::Strong(vec![Inline::Text(format!("[{text}]"))]));
            }
            ViewInline::Menu(items) => {
                out.push(Inline::Strong(vec![Inline::Text(items.join(" › "))]));
            }
            ViewInline::Stem {
                source, rendered, ..
            } => {
                let drawn = rendered
                    .as_ref()
                    .and_then(|formula| self.load(&formula.path).ok());
                match drawn {
                    Some(asset) => out.push(Inline::Image {
                        asset,
                        alt: source.clone(),
                    }),
                    None => {
                        self.warn(
                            None,
                            "the formula is not drawn and is exported as its source".to_owned(),
                        );
                        out.push(Inline::Code(vec![Inline::Text(source.clone())]));
                    }
                }
            }
        }
    }

    /// A link out of the book, or its text where the address is not one a reader may follow.
    fn external_or_text(&mut self, url: &str, content: &[ViewInline]) -> Inline {
        let shown = if content.is_empty() {
            vec![Inline::Text(url.to_owned())]
        } else {
            self.inlines(content)
        };
        if safe_url(url) {
            Inline::Link {
                target: Link::External(url.trim().to_owned()),
                content: shown,
            }
        } else {
            self.warn(
                None,
                format!("the address {url} is not a web address and is exported as text"),
            );
            Inline::Emphasis(shown)
        }
    }

    /// A cross reference: a link inside the book where its target is in it, else the target's
    /// published address where one is configured, else its text and a warning.
    fn xref(
        &mut self,
        target: &str,
        link: &ResolvedLink,
        content: &[ViewInline],
        span: &SourceSpan,
    ) -> Inline {
        let shown = if content.is_empty() {
            vec![Inline::Text(
                link.title.clone().unwrap_or_else(|| target.to_owned()),
            )]
        } else {
            self.inlines(content)
        };
        if link.unresolved {
            self.warn(
                Some(span),
                format!("the cross reference {target} leads to nothing and is exported as text"),
            );
            return Inline::Emphasis(shown);
        }
        let fragment = link.fragment.clone().or_else(|| {
            target
                .split_once('#')
                .map(|(_, fragment)| fragment.to_owned())
        });
        let chapter = match &link.target {
            Some(location) if location.path != self.source => self
                .pages
                .get(&location.path)
                .or_else(|| self.pages.get(&adoc_core::normalize_path(&location.path)))
                .copied(),
            _ => Some(self.chapter),
        };
        if let Some(chapter) = chapter {
            let anchor = match fragment.as_deref() {
                Some(fragment) => match self.labels.get(chapter, fragment) {
                    Some(anchor) => Some(anchor.to_owned()),
                    None => {
                        self.warn(
                            Some(span),
                            format!(
                                "the cross reference {target} names nothing in the book and is exported as text"
                            ),
                        );
                        return Inline::Emphasis(shown);
                    }
                },
                None => None,
            };
            return Inline::Link {
                target: Link::Internal(Target { chapter, anchor }),
                content: shown,
            };
        }
        let url = link
            .target
            .as_ref()
            .and_then(|location| self.options.page_urls.get(&location.path));
        match url {
            Some(url) if safe_url(url) => {
                let mut url = url.clone();
                if let Some(fragment) = &fragment {
                    url.push('#');
                    url.push_str(fragment);
                }
                Inline::Link {
                    target: Link::External(url),
                    content: shown,
                }
            }
            _ => {
                self.warn(
                    Some(span),
                    format!(
                        "the cross reference {target} leads outside the book and is exported as text"
                    ),
                );
                Inline::Emphasis(shown)
            }
        }
    }
}

/// A place for `anchor` where its block shows nothing an anchor can sit on: an empty
/// paragraph, as an open block's id gets, so a reference to it still lands.
fn keep_anchor(out: &mut Vec<Block>, anchor: Option<String>) {
    if let Some(anchor) = anchor {
        out.push(Block::Paragraph {
            anchor: Some(anchor),
            class: ParagraphClass::Normal,
            content: Vec::new(),
        });
    }
}

/// An image's description, else its file's name.
fn alt_of(image: &adoc_render::ViewImage) -> String {
    if image.alt.trim().is_empty() {
        image.target.clone()
    } else {
        image.alt.clone()
    }
}

fn attribution_paragraph(attribution: Option<&str>, citation: Option<&str>) -> Option<Block> {
    let text = match (attribution, citation) {
        (Some(attribution), Some(citation)) => format!("— {attribution}, {citation}"),
        (Some(text), None) | (None, Some(text)) => format!("— {text}"),
        (None, None) => return None,
    };
    Some(Block::Paragraph {
        anchor: None,
        class: ParagraphClass::Attribution,
        content: vec![Inline::Text(text)],
    })
}

fn admonition_label(kind: adoc_dom::AdmonitionKind) -> &'static str {
    match kind {
        adoc_dom::AdmonitionKind::Note => "Note",
        adoc_dom::AdmonitionKind::Tip => "Tip",
        adoc_dom::AdmonitionKind::Important => "Important",
        adoc_dom::AdmonitionKind::Caution => "Caution",
        adoc_dom::AdmonitionKind::Warning => "Warning",
    }
}

/// Whether `url` is an address a reader may follow: a web, mail, file transfer or telephone
/// address, never a script or a local file.
pub(crate) fn safe_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.len() > 2000 || url.chars().any(char::is_control) {
        return false;
    }
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto" | "ftp" | "ftps" | "tel"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_urls_are_web_mail_and_telephone_addresses() {
        assert!(safe_url("https://example.com/a"));
        assert!(safe_url("mailto:someone@example.com"));
        assert!(!safe_url("javascript:alert(1)"));
        assert!(!safe_url("file:///etc/passwd"));
        assert!(!safe_url("relative/page.html"));
        assert!(!safe_url("https://example.com/\u{7}"));
    }

    #[test]
    fn the_table_of_contents_nests_by_level() {
        let mut toc = Toc::default();
        let target = |chapter| Target {
            chapter,
            anchor: None,
        };
        toc.add(0, "Part".into(), target(0));
        toc.add(1, "One".into(), target(1));
        toc.add(2, "One a".into(), target(1));
        toc.add(1, "Two".into(), target(2));
        toc.add(0, "Second part".into(), target(3));
        toc.add(2, "Deep".into(), target(4));
        let toc = toc.finish();
        assert_eq!(toc.len(), 2);
        assert_eq!(toc[0].children.len(), 2);
        assert_eq!(toc[0].children[0].children[0].title, "One a");
        assert_eq!(toc[1].children[0].title, "Deep");
    }
}
