//! The document's body: the view tree written as Word paragraphs, numbering, tables, bookmarks,
//! hyperlinks and footnotes, every one editable. Blocks that have no Word structure of their own
//! (images, diagrams, formulas, media and the like, until DOCX 4/8) are written as text and
//! reported, so nothing is lost silently.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

use adoc_render::{
    AdmonitionKind, ListKind, SourceSpan, ViewBlock, ViewDescription, ViewInline, ViewKind,
    ViewListItem, ViewTree,
};

use crate::{
    labels::Labels,
    numbering::{self, ListStyle, Numbering, DEEPEST_LEVEL},
    parts::DEEPEST_HEADING,
    xml::Xml,
    BookItem, DocxDiagnostic, DocxDocument, DocxOptions, DocxPage, DocxSeverity, Paper,
};

/// How a run of text looks: what its inlines add up to.
#[derive(Clone, Copy, Default)]
pub(crate) struct Format {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) strike: bool,
    pub(crate) underline: bool,
    pub(crate) mark: bool,
    pub(crate) superscript: bool,
    pub(crate) subscript: bool,
    /// A character style.
    pub(crate) style: Option<&'static str>,
}

/// A paragraph's properties.
#[derive(Clone, Copy, Default)]
pub(crate) struct Para<'a> {
    pub(crate) style: Option<&'a str>,
    pub(crate) page_break_before: bool,
    /// Its list: `numId` and level.
    pub(crate) numbering: Option<(u32, u32)>,
    /// Left indent and hanging indent, in twentieths of a point.
    pub(crate) indent: Option<(u32, u32)>,
    pub(crate) rule_below: bool,
}

/// Which part is being written: the document or its footnotes, each with its own relationships.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Document,
    Footnotes,
}

/// A hyperlink's relationship: its id and address.
pub(crate) struct Hyperlink {
    pub(crate) id: String,
    pub(crate) url: String,
}

pub(crate) struct Writer<'a> {
    /// The part being written: the document's body, or a footnote while one is.
    pub(crate) xml: Xml,
    pub(crate) options: &'a DocxOptions,
    pub(crate) diagnostics: Vec<DocxDiagnostic>,
    pub(crate) labels: Labels,
    /// Each page of the document by its file, to its key.
    pub(crate) pages: HashMap<PathBuf, String>,
    /// The page being written.
    pub(crate) key: String,
    pub(crate) source: PathBuf,
    /// The page's attributes.
    pub(crate) attributes: BTreeMap<String, String>,
    /// The page's footnotes by number, and those written so far: their Word ids and bookmarks.
    pub(crate) page_footnotes: HashMap<u32, Vec<ViewInline>>,
    pub(crate) written_footnotes: HashMap<u32, (u32, String)>,
    /// The block being written, for diagnostics.
    pub(crate) span: Option<SourceSpan>,
    pub(crate) part: Part,
    pub(crate) footnotes: Xml,
    pub(crate) footnote_count: u32,
    pub(crate) hyperlinks: [Vec<Hyperlink>; 2],
    pub(crate) numbering: Numbering,
    pub(crate) bookmark_count: u32,
    /// Bookmarks written, so each is written once.
    pub(crate) placed: HashSet<String>,
    /// Bookmarks to open the next paragraph with: a block's id, a page's start.
    pub(crate) pending: Vec<String>,
    /// The context blocks are written in.
    pub(crate) indent: u32,
    pub(crate) quote: bool,
    /// The format every run starts from: a table cell's.
    pub(crate) base: Format,
    /// The alignment of paragraphs: a table cell's.
    pub(crate) align: Option<&'static str>,
    /// The width there is to fill, in twentieths of a point.
    pub(crate) width: u32,
    /// Whether the last thing written was a table, which a paragraph must follow before
    /// another table or the end of a cell.
    pub(crate) after_table: bool,
}

fn heading(level: u8) -> String {
    format!("Heading{}", level.clamp(1, DEEPEST_HEADING))
}

/// What a block is, for a diagnostic.
fn describe(kind: &ViewKind) -> &'static str {
    match kind {
        ViewKind::Image(_) => "an image",
        ViewKind::Diagram(_) => "a diagram",
        ViewKind::Stem { .. } => "a formula",
        ViewKind::Passthrough(_) => "a passthrough block",
        _ => "a block",
    }
}

impl<'a> Writer<'a> {
    pub(crate) fn new(document: &DocxDocument, options: &'a DocxOptions) -> Self {
        let pages: Vec<&DocxPage> = match document {
            DocxDocument::Article(page) => vec![page],
            DocxDocument::Book(book) => book
                .items
                .iter()
                .filter_map(|item| match item {
                    BookItem::Page(page) => Some(page),
                    _ => None,
                })
                .collect(),
        };
        let width = match options.paper {
            Paper::A4 => 11906,
            Paper::Letter => 12240,
        } - 2 * 1440;
        Self {
            xml: Xml::fragment(),
            options,
            diagnostics: Vec::new(),
            labels: Labels::collect(&pages),
            pages: pages
                .iter()
                .map(|page| (page.view.source.clone(), page.key.clone()))
                .collect(),
            key: String::new(),
            source: PathBuf::new(),
            attributes: BTreeMap::new(),
            page_footnotes: HashMap::new(),
            written_footnotes: HashMap::new(),
            span: None,
            part: Part::Document,
            footnotes: Xml::fragment(),
            footnote_count: 0,
            hyperlinks: [Vec::new(), Vec::new()],
            numbering: Numbering::default(),
            bookmark_count: 0,
            placed: HashSet::new(),
            pending: Vec::new(),
            indent: 0,
            quote: false,
            base: Format::default(),
            align: None,
            width,
            after_table: false,
        }
    }

    pub(crate) fn document(&mut self, document: &DocxDocument) {
        match document {
            DocxDocument::Article(page) => {
                self.begin_page(page);
                if let Some(title) = &page.view.title {
                    if !ViewInline::plain_text(title).trim().is_empty() {
                        self.paragraph(
                            Para {
                                style: Some("Title"),
                                ..Para::default()
                            },
                            |writer| writer.inlines(title, Format::default()),
                        );
                    }
                }
                self.blocks(&page.view.blocks, page.depth);
            }
            DocxDocument::Book(book) => {
                if !book.title.trim().is_empty() {
                    self.text_paragraph(Some("Title"), &book.title);
                }
                if let Some(subtitle) = book.subtitle.as_deref().filter(|s| !s.trim().is_empty()) {
                    self.text_paragraph(Some("Subtitle"), subtitle);
                }
                // A part is a level above the chapters, so where there are parts everything
                // else moves down one.
                let shift = u8::from(
                    book.items
                        .iter()
                        .any(|item| matches!(item, BookItem::Part(_))),
                );
                // Parts and chapters start a page, unless a part has just started one.
                let mut after_part = false;
                for item in &book.items {
                    match item {
                        BookItem::Part(title) => {
                            self.paragraph(
                                Para {
                                    style: Some(&heading(1)),
                                    page_break_before: true,
                                    ..Para::default()
                                },
                                |writer| writer.text(title, Format::default()),
                            );
                            after_part = true;
                        }
                        BookItem::Heading { title, depth } => {
                            self.paragraph(
                                Para {
                                    style: Some(&heading(depth.saturating_add(shift))),
                                    page_break_before: *depth == 1 && !after_part,
                                    ..Para::default()
                                },
                                |writer| writer.text(title, Format::default()),
                            );
                            after_part = false;
                        }
                        BookItem::Page(page) => {
                            self.begin_page(page);
                            let depth = page.depth.max(1).saturating_add(shift);
                            if let Some(title) =
                                page.view.title.as_ref().filter(|title| {
                                    !ViewInline::plain_text(title).trim().is_empty()
                                })
                            {
                                self.paragraph(
                                    Para {
                                        style: Some(&heading(depth)),
                                        page_break_before: page.depth <= 1 && !after_part,
                                        ..Para::default()
                                    },
                                    |writer| writer.inlines(title, Format::default()),
                                );
                            }
                            self.blocks(&page.view.blocks, depth);
                            after_part = false;
                        }
                    }
                }
            }
        }
        // A paragraph ends the body after a table, as one ends every cell.
        self.close_tables();
    }

    /// Starts writing `page`: its bookmarks and footnotes are its own.
    fn begin_page(&mut self, page: &DocxPage) {
        self.key.clone_from(&page.key);
        self.source.clone_from(&page.view.source);
        self.attributes.clone_from(&page.view.attributes);
        self.page_footnotes = footnotes_of(&page.view);
        self.written_footnotes.clear();
        if let Some(name) = self.labels.get(&page.key, "") {
            self.pending.push(name.to_owned());
        }
    }

    pub(crate) fn warn(&mut self, span: Option<&SourceSpan>, message: String) {
        let source = span.or(self.span.as_ref()).cloned();
        self.diagnostics.push(DocxDiagnostic {
            severity: DocxSeverity::Warning,
            message,
            source,
        });
    }

    /// A bookmark here, once.
    pub(crate) fn bookmark(&mut self, name: &str) {
        if !self.placed.insert(name.to_owned()) {
            return;
        }
        self.bookmark_count += 1;
        let id = self.bookmark_count.to_string();
        self.xml
            .empty("w:bookmarkStart", &[("w:id", &id), ("w:name", name)]);
        self.xml.empty("w:bookmarkEnd", &[("w:id", &id)]);
    }

    /// The paragraph that ends a table before another, or before a cell's or the body's end.
    pub(crate) fn close_tables(&mut self) {
        if self.after_table {
            self.xml.open("w:p", &[]);
            self.xml.open("w:pPr", &[]);
            self.xml.empty("w:pStyle", &[("w:val", "TableSpacer")]);
            self.xml.close("w:pPr");
            self.xml.close("w:p");
            self.after_table = false;
        }
    }

    /// A paragraph of `para`, its content written by `content`.
    pub(crate) fn paragraph(&mut self, para: Para<'_>, content: impl FnOnce(&mut Self)) {
        self.after_table = false;
        let style = para.style.or(self.quote.then_some("Quote"));
        let indent = para
            .indent
            .or((self.indent > 0).then_some((self.indent, 0)));
        self.xml.open("w:p", &[]);
        let properties = style.is_some()
            || para.page_break_before
            || para.numbering.is_some()
            || para.rule_below
            || indent.is_some()
            || self.align.is_some();
        if properties {
            self.xml.open("w:pPr", &[]);
            if let Some(style) = style {
                self.xml.empty("w:pStyle", &[("w:val", style)]);
            }
            if para.page_break_before {
                self.xml.empty("w:pageBreakBefore", &[]);
            }
            if let Some((number, level)) = para.numbering {
                self.xml.open("w:numPr", &[]);
                self.xml.empty("w:ilvl", &[("w:val", &level.to_string())]);
                self.xml.empty("w:numId", &[("w:val", &number.to_string())]);
                self.xml.close("w:numPr");
            }
            if para.rule_below {
                self.xml.open("w:pBdr", &[]);
                self.xml.empty(
                    "w:bottom",
                    &[
                        ("w:val", "single"),
                        ("w:sz", "6"),
                        ("w:space", "1"),
                        ("w:color", "auto"),
                    ],
                );
                self.xml.close("w:pBdr");
            }
            if let Some((left, hanging)) = indent {
                let left = left.to_string();
                let hanging = hanging.to_string();
                let mut attributes = vec![("w:left", left.as_str())];
                if hanging != "0" {
                    attributes.push(("w:hanging", hanging.as_str()));
                }
                self.xml.empty("w:ind", &attributes);
            }
            if let Some(align) = self.align {
                self.xml.empty("w:jc", &[("w:val", align)]);
            }
            self.xml.close("w:pPr");
        }
        for name in std::mem::take(&mut self.pending) {
            self.bookmark(&name);
        }
        content(self);
        self.xml.close("w:p");
    }

    /// A paragraph of plain text in `style`.
    fn text_paragraph(&mut self, style: Option<&str>, text: &str) {
        self.paragraph(
            Para {
                style,
                ..Para::default()
            },
            |writer| writer.text(text, Format::default()),
        );
    }

    pub(crate) fn blocks(&mut self, blocks: &[ViewBlock], depth: u8) {
        for block in blocks {
            self.block(block, depth);
        }
    }

    /// A block's title, with its caption (`Table 1.`), above it.
    fn title(&mut self, block: &ViewBlock) {
        if ViewInline::plain_text(&block.title).trim().is_empty() {
            return;
        }
        self.paragraph(
            Para {
                style: Some("Caption"),
                ..Para::default()
            },
            |writer| {
                if let Some(caption) = &block.caption {
                    writer.text(&format!("{caption} "), Format::default());
                }
                writer.inlines(&block.title, Format::default());
            },
        );
    }

    fn block(&mut self, block: &ViewBlock, depth: u8) {
        self.span = Some(block.span.clone());
        if let Some(name) = block
            .id
            .as_deref()
            .and_then(|id| self.labels.get(&self.key, id))
        {
            self.pending.push(name.to_owned());
        }
        match &block.kind {
            ViewKind::Section {
                level,
                number,
                title,
                blocks,
            } => {
                self.paragraph(
                    Para {
                        style: Some(&heading(level.saturating_add(depth))),
                        ..Para::default()
                    },
                    |writer| {
                        if let Some(number) = number {
                            writer.text(&format!("{number} "), Format::default());
                        }
                        writer.inlines(title, Format::default());
                    },
                );
                self.blocks(blocks, depth);
            }
            ViewKind::Heading { level, title } => self.paragraph(
                Para {
                    style: Some(&heading(level.saturating_add(depth))),
                    ..Para::default()
                },
                |writer| writer.inlines(title, Format::default()),
            ),
            ViewKind::Paragraph(content) => {
                self.title(block);
                let base = self.base;
                self.paragraph(Para::default(), |writer| writer.inlines(content, base));
            }
            ViewKind::List { kind, items } => {
                self.title(block);
                self.list(block, *kind, items, 0);
            }
            ViewKind::DescriptionList(items) => {
                self.title(block);
                self.descriptions(items, depth);
            }
            ViewKind::CalloutList(items) => {
                self.title(block);
                for item in items {
                    let base = self.base;
                    self.paragraph(
                        Para {
                            indent: Some((self.indent + 720, 720)),
                            ..Para::default()
                        },
                        |writer| {
                            writer.text(&format!("({})\t", item.number), Format::default());
                            writer.inlines(&item.content, base);
                        },
                    );
                    self.nested(&item.blocks, depth, 720);
                }
            }
            ViewKind::Admonition { kind, blocks } => {
                self.title(block);
                let label = self.admonition_label(*kind);
                self.container("AdocAdmonition", Some(&label), blocks, depth);
            }
            ViewKind::Code { text, callouts, .. } => {
                self.title(block);
                let text = with_callouts(text, callouts);
                self.preformatted("SourceCode", &text);
            }
            ViewKind::Literal(text) => {
                self.title(block);
                self.preformatted("LiteralBlock", text);
            }
            ViewKind::Quote {
                attribution,
                citation,
                blocks,
            } => {
                self.title(block);
                let quote = std::mem::replace(&mut self.quote, true);
                self.blocks(blocks, depth);
                self.quote = quote;
                self.attribution(attribution.as_deref(), citation.as_deref());
            }
            ViewKind::Verse {
                attribution,
                citation,
                text,
            } => {
                self.title(block);
                self.preformatted("Verse", text);
                self.attribution(attribution.as_deref(), citation.as_deref());
            }
            ViewKind::Sidebar(blocks) => {
                self.title(block);
                self.container("AdocSidebar", None, blocks, depth);
            }
            ViewKind::Example(blocks) => {
                self.title(block);
                self.container("AdocExample", None, blocks, depth);
            }
            ViewKind::Open(blocks) => {
                self.title(block);
                self.blocks(blocks, depth);
            }
            ViewKind::Table(table) => {
                self.title(block);
                self.table(block, table, depth);
            }
            ViewKind::ThematicBreak => self.paragraph(
                Para {
                    rule_below: true,
                    ..Para::default()
                },
                |_| {},
            ),
            ViewKind::PageBreak => self.paragraph(Para::default(), |writer| {
                writer.xml.open("w:r", &[]);
                writer.xml.empty("w:br", &[("w:type", "page")]);
                writer.xml.close("w:r");
            }),
            ViewKind::Toc => self.warn(
                Some(&block.span),
                "a table of contents is not exported to Word".to_owned(),
            ),
            ViewKind::Media { targets, .. } => {
                self.title(block);
                self.text_paragraph(None, &targets.join(", "));
                self.warn(
                    Some(&block.span),
                    "a video or audio block is exported as its address".to_owned(),
                );
            }
            ViewKind::Unsupported { name, source } => {
                self.preformatted("LiteralBlock", source);
                self.warn(
                    Some(&block.span),
                    format!("an unsupported block ({name}) is exported as its source"),
                );
            }
            kind @ (ViewKind::Image(_)
            | ViewKind::Diagram(_)
            | ViewKind::Stem { .. }
            | ViewKind::Passthrough(_)) => {
                self.title(block);
                match kind {
                    ViewKind::Image(image) => {
                        let text = if image.alt.is_empty() {
                            &image.target
                        } else {
                            &image.alt
                        };
                        self.text_paragraph(None, &format!("[{text}]"));
                    }
                    ViewKind::Diagram(adoc_render::DiagramView::Svg(_)) => {
                        self.text_paragraph(None, "[diagram]");
                    }
                    ViewKind::Diagram(
                        adoc_render::DiagramView::Pending { source, .. }
                        | adoc_render::DiagramView::Failed { source, .. },
                    )
                    | ViewKind::Stem { source, .. } => self.preformatted("LiteralBlock", source),
                    ViewKind::Passthrough(text) => self.preformatted("LiteralBlock", text),
                    _ => {}
                }
                self.warn(
                    Some(&block.span),
                    format!("{} is exported as text", describe(kind)),
                );
            }
        }
    }

    /// `blocks` indented by `by` beyond the current indent.
    fn nested(&mut self, blocks: &[ViewBlock], depth: u8, by: u32) {
        let indent = self.indent;
        self.indent += by;
        self.blocks(blocks, depth);
        self.indent = indent;
    }

    /// One paragraph of `text` in `style`, whitespace kept, each line a line break.
    fn preformatted(&mut self, style: &str, text: &str) {
        self.paragraph(
            Para {
                style: Some(style),
                ..Para::default()
            },
            |writer| writer.text(text, Format::default()),
        );
    }

    fn attribution(&mut self, attribution: Option<&str>, citation: Option<&str>) {
        let text = match (attribution, citation) {
            (Some(attribution), Some(citation)) => format!("— {attribution}, {citation}"),
            (Some(text), None) | (None, Some(text)) => format!("— {text}"),
            (None, None) => return,
        };
        self.text_paragraph(Some("QuoteAttribution"), &text);
    }

    /// An admonition's label: the document's `note-caption` and the like, else Asciidoctor's.
    fn admonition_label(&self, kind: AdmonitionKind) -> String {
        let (attribute, default) = match kind {
            AdmonitionKind::Note => ("note-caption", "Note"),
            AdmonitionKind::Tip => ("tip-caption", "Tip"),
            AdmonitionKind::Important => ("important-caption", "Important"),
            AdmonitionKind::Caution => ("caution-caption", "Caution"),
            AdmonitionKind::Warning => ("warning-caption", "Warning"),
        };
        self.page_attribute(attribute)
            .unwrap_or_else(|| default.to_owned())
    }

    fn page_attribute(&self, name: &str) -> Option<String> {
        self.attributes.get(name).cloned()
    }

    /// A list at `level`: Word numbering for bullets and numbers, a box for a checklist item.
    fn list(&mut self, block: &ViewBlock, kind: ListKind, items: &[ViewListItem], level: u32) {
        let level = level.min(DEEPEST_LEVEL);
        let numbering: Option<u32> = match kind {
            ListKind::Unordered => Some(self.numbering.instance(ListStyle::Bullet, level, 1)),
            ListKind::Ordered => {
                if block.options.contains("reversed") {
                    self.warn(
                        Some(&block.span),
                        "a reversed list is numbered upwards in Word".to_owned(),
                    );
                }
                let start = block
                    .attributes
                    .get("start")
                    .and_then(|start| start.trim().parse().ok())
                    .unwrap_or(1);
                Some(self.numbering.instance(
                    ListStyle::ordered(block.style.as_deref()),
                    level,
                    start,
                ))
            }
            // Only the items written with a box have one; the others are bullets.
            ListKind::Checklist => Some(self.numbering.instance(ListStyle::Bullet, level, 1)),
        };
        for item in items {
            let base = self.base;
            match (item.checked, numbering) {
                (None, Some(numbering)) => self.paragraph(
                    Para {
                        numbering: Some((numbering, level)),
                        ..Para::default()
                    },
                    |writer| writer.inlines(&item.content, base),
                ),
                (checked, _) => self.paragraph(
                    Para {
                        indent: Some((numbering::indent(level), 360)),
                        ..Para::default()
                    },
                    |writer| {
                        let box_ = if checked == Some(true) { "☑" } else { "☐" };
                        writer.text(&format!("{box_}\t"), Format::default());
                        writer.inlines(&item.content, base);
                    },
                ),
            }
            for nested in &item.blocks {
                match &nested.kind {
                    ViewKind::List { kind, items } => {
                        self.span = Some(nested.span.clone());
                        if let Some(name) = nested
                            .id
                            .as_deref()
                            .and_then(|id| self.labels.get(&self.key, id))
                        {
                            self.pending.push(name.to_owned());
                        }
                        self.title(nested);
                        self.list(nested, *kind, items, level + 1);
                    }
                    _ => {
                        let indent = self.indent;
                        self.indent = numbering::indent(level);
                        self.block(nested, 0);
                        self.indent = indent;
                    }
                }
            }
        }
    }

    /// A description list: each term in bold, its description indented below it.
    fn descriptions(&mut self, items: &[ViewDescription], depth: u8) {
        for item in items {
            let base = self.base;
            self.paragraph(
                Para {
                    style: Some("DescriptionTerm"),
                    ..Para::default()
                },
                |writer| writer.inlines(&item.term, base),
            );
            let indent = self.indent;
            self.indent += 720;
            if !item.content.is_empty() {
                self.paragraph(Para::default(), |writer| {
                    writer.inlines(&item.content, base)
                });
            }
            self.blocks(&item.blocks, depth);
            self.indent = indent;
        }
    }
}

/// A page's footnotes by number.
fn footnotes_of(view: &ViewTree) -> HashMap<u32, Vec<ViewInline>> {
    view.footnotes
        .iter()
        .map(|note| (note.number, note.content.clone()))
        .collect()
}

/// Listing text with each callout's number at the end of its line, as the PDF shows them.
fn with_callouts(text: &str, callouts: &[adoc_render::Callout]) -> String {
    if callouts.is_empty() {
        return text.to_owned();
    }
    let mut out = String::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
        for callout in callouts.iter().filter(|callout| callout.line == index) {
            out.push_str(&format!(" ({})", callout.number));
        }
    }
    out
}
