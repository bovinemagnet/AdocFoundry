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
    media::Media,
    numbering::{self, ListStyle, Numbering, DEEPEST_LEVEL},
    pages::{RunningPart, Section},
    parts::DEEPEST_HEADING,
    xml::Xml,
    BookItem, DocxDiagnostic, DocxDocument, DocxOptions, DocxPage, DocxSeverity,
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

/// Which part is being written: the document, its footnotes, or a header or footer, each with
/// its own relationships.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Part(pub(crate) usize);

impl Part {
    pub(crate) const DOCUMENT: Self = Self(0);
    pub(crate) const FOOTNOTES: Self = Self(1);
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
    pub(crate) hyperlinks: Vec<Vec<Hyperlink>>,
    /// The pictures, and each part's relationships to them: id and media index.
    pub(crate) media: Media,
    pub(crate) pictures: Vec<Vec<(String, usize)>>,
    /// The drawings written, for their ids, which are unique in the document.
    pub(crate) drawings: u32,
    pub(crate) numbering: Numbering,
    /// The ids a template gives the export's styles, where they differ.
    style_ids: BTreeMap<String, String>,
    /// A template's pictures its kept headers and footers show.
    pub(crate) template_media: std::collections::BTreeSet<usize>,
    /// Odd and even pages have different headers and footers.
    pub(crate) odd_even: bool,
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
    /// The header and footer parts, and the document's relationships to them.
    pub(crate) running_parts: Vec<RunningPart>,
    pub(crate) document_links: Vec<(String, &'static str, String)>,
    /// Each heading written, for the table of contents: its Word level, text and bookmark.
    pub(crate) headings: Vec<(u8, String, String)>,
    /// Where in the body the table of contents goes, and whether one was written.
    toc_at: Option<usize>,
    pub(crate) wrote_toc: bool,
    /// The last section's properties, which end the body.
    pub(crate) final_section: Xml,
    /// The page asks for its table of contents at its `toc::[]` block.
    toc_macro: bool,
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
        let template = options.look.template.as_deref();
        let width = options
            .page_size()
            .0
            .saturating_sub(options.margins.left + options.margins.right)
            .max(1440);
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
            part: Part::DOCUMENT,
            footnotes: Xml::fragment(),
            footnote_count: 0,
            hyperlinks: vec![Vec::new(), Vec::new()],
            media: Media::default(),
            pictures: vec![Vec::new(), Vec::new()],
            drawings: 0,
            numbering: template.map_or_else(Numbering::default, |template| {
                Numbering::after(
                    template
                        .numbering
                        .as_deref()
                        .map_or((0, 0), crate::merge::numbering_ids),
                )
            }),
            style_ids: template.map_or_else(BTreeMap::new, |template| {
                let ours = crate::parts::styles(
                    width,
                    &crate::StyleOverrides::default(),
                    &crate::WordLook::default(),
                    &crate::fonts::Fonts::default(),
                );
                crate::merge::style_ids(&template.styles, &String::from_utf8_lossy(&ours))
            }),
            template_media: std::collections::BTreeSet::new(),
            odd_even: false,
            bookmark_count: 0,
            placed: HashSet::new(),
            pending: Vec::new(),
            indent: 0,
            quote: false,
            base: Format::default(),
            align: None,
            width,
            after_table: false,
            running_parts: Vec::new(),
            document_links: Vec::new(),
            headings: Vec::new(),
            toc_at: None,
            wrote_toc: false,
            final_section: Xml::fragment(),
            toc_macro: false,
        }
    }

    pub(crate) fn document(&mut self, document: &DocxDocument) {
        let margins = self.options.margins;
        // The first page's view: its images for the covers and the title page's logo.
        let first: Option<&ViewTree> = match document {
            DocxDocument::Article(page) => Some(&page.view),
            DocxDocument::Book(book) => book.items.iter().find_map(|item| match item {
                BookItem::Page(page) => Some(&page.view),
                _ => None,
            }),
        };
        // The page's images, else the look's covers.
        let look_image = |path: &Option<PathBuf>, alt: &str| {
            path.clone().map(|path| adoc_render::ViewImage {
                target: path.to_string_lossy().into_owned(),
                path: Some(path),
                url: None,
                alt: alt.to_owned(),
                width: None,
                height: None,
            })
        };
        let image = |name: &str| {
            first
                .and_then(|view| view.attribute_images.get(name))
                .cloned()
                .or_else(|| match name {
                    "front-cover-image" => {
                        look_image(&self.options.look.front_cover, "Front cover")
                    }
                    "back-cover-image" => look_image(&self.options.look.back_cover, "Back cover"),
                    _ => None,
                })
        };
        let (front, back, logo) = (
            image("front-cover-image"),
            image("back-cover-image"),
            image("title-logo-image"),
        );
        let (title, attributes): (Vec<ViewInline>, BTreeMap<String, String>) = match document {
            DocxDocument::Article(page) => (
                page.view.title.clone().unwrap_or_default(),
                page.view.attributes.clone(),
            ),
            DocxDocument::Book(book) => (
                vec![ViewInline::Text(book.title.clone())],
                book.attributes.clone(),
            ),
        };
        let plain_title = ViewInline::plain_text(&title);

        if let Some(front) = &front {
            if self.cover(front, "Front cover") {
                self.end_section(&Section::cover(Vec::new()));
            }
        }
        // A book has a title page; an article only when it asks for one.
        let title_page =
            matches!(document, DocxDocument::Book(_)) || attributes.contains_key("title-page");
        if title_page && !plain_title.trim().is_empty() {
            let subtitle = match document {
                DocxDocument::Book(book) => book.subtitle.clone(),
                DocxDocument::Article(_) => None,
            };
            self.title_page(logo.as_ref(), &title, subtitle.as_deref(), &attributes);
            self.end_section(&Section::plain(margins));
        }

        // A part is a level above the chapters, so where there are parts everything else moves
        // down one, and a chapter's heading is a level lower.
        let shift = match document {
            DocxDocument::Book(book) => u8::from(
                book.items
                    .iter()
                    .any(|item| matches!(item, BookItem::Part(_))),
            ),
            DocxDocument::Article(_) => 0,
        };
        let toc = match document {
            DocxDocument::Book(_) => attributes.contains_key("toc"),
            DocxDocument::Article(page) => !page.view.toc.is_empty(),
        };
        let toc_macro = attributes.get("toc").is_some_and(|toc| toc == "macro");
        match document {
            DocxDocument::Article(page) => {
                self.begin_page(page);
                if !title_page && !plain_title.trim().is_empty() {
                    self.paragraph(
                        Para {
                            style: Some("Title"),
                            ..Para::default()
                        },
                        |writer| writer.inlines(&title, Format::default()),
                    );
                }
                if toc && !toc_macro {
                    self.toc_at = Some(self.xml.len());
                }
                self.toc_macro = toc && toc_macro;
                self.blocks(&page.view.blocks, page.depth);
            }
            DocxDocument::Book(book) => {
                if toc {
                    self.toc_at = Some(self.xml.len());
                }
                // Parts and chapters start a page, unless a part has just started one.
                let mut after_part = false;
                for item in &book.items {
                    match item {
                        BookItem::Part(title) => {
                            self.heading_paragraph(1, true, title.clone(), |writer| {
                                writer.text(title, Format::default());
                            });
                            after_part = true;
                        }
                        BookItem::Heading { title, depth } => {
                            self.heading_paragraph(
                                depth.saturating_add(shift),
                                *depth == 1 && !after_part,
                                title.clone(),
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
                                self.heading_paragraph(
                                    depth,
                                    page.depth <= 1 && !after_part,
                                    ViewInline::plain_text(title),
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

        // The table of contents, of the headings now written, where it was asked for.
        if let Some(at) = self.toc_at {
            let levels = attributes
                .get("toclevels")
                .and_then(|levels| levels.trim().parse::<u8>().ok())
                .unwrap_or(2)
                .clamp(1, 5)
                .saturating_add(shift)
                .min(DEEPEST_HEADING);
            let title = attributes
                .get("toc-title")
                .cloned()
                .unwrap_or_else(|| "Table of Contents".to_owned());
            let contents = self.table_of_contents(levels, &title);
            self.xml.insert(at, contents);
        }

        // The body's section: its running content and page numbers from 1.
        let chapter = format!("heading {}", 1 + shift);
        let (references, first_page) = self.running_references(&plain_title, &chapter);
        let body = Section {
            margins,
            references,
            title_page: first_page,
            restart: true,
        };
        let last = match &back {
            Some(back) => {
                self.end_section(&body);
                if self.cover(back, "Back cover") {
                    let blank = self.blank_references();
                    Section::cover(blank)
                } else {
                    Section::plain(margins)
                }
            }
            None => body,
        };
        let content = std::mem::take(&mut self.xml);
        self.section_properties(&last);
        self.final_section = std::mem::replace(&mut self.xml, content);
    }

    /// A heading that the table of contents lists: its paragraph, with a bookmark the contents
    /// links to.
    fn heading_paragraph(
        &mut self,
        level: u8,
        page_break_before: bool,
        text: String,
        content: impl FnOnce(&mut Self),
    ) {
        let level = level.clamp(1, DEEPEST_HEADING);
        let name = format!("_Toc{}", self.headings.len() + 1);
        self.headings.push((level, text, name.clone()));
        self.pending.push(name);
        self.paragraph(
            Para {
                style: Some(&heading(level)),
                page_break_before,
                ..Para::default()
            },
            content,
        );
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

    /// The id style `id` has in the document: a template's own id for it, where it has one.
    pub(crate) fn style_id(&self, id: &str) -> String {
        self.style_ids
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned())
    }

    /// The paragraph that ends a table before another, or before a cell's or the body's end.
    pub(crate) fn close_tables(&mut self) {
        if self.after_table {
            self.xml.open("w:p", &[]);
            self.xml.open("w:pPr", &[]);
            let style = self.style_id("TableSpacer");
            self.xml.empty("w:pStyle", &[("w:val", &style)]);
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
                let style = self.style_id(style);
                self.xml.empty("w:pStyle", &[("w:val", &style)]);
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
                let text = match number {
                    Some(number) => format!("{number} {}", ViewInline::plain_text(title)),
                    None => ViewInline::plain_text(title),
                };
                self.heading_paragraph(level.saturating_add(depth), false, text, |writer| {
                    if let Some(number) = number {
                        writer.text(&format!("{number} "), Format::default());
                    }
                    writer.inlines(title, Format::default());
                });
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
            // `toc::[]`: where the page asks for its table of contents.
            ViewKind::Toc => {
                if self.toc_macro && self.toc_at.is_none() && self.part == Part::DOCUMENT {
                    self.close_tables();
                    self.toc_at = Some(self.xml.len());
                }
            }
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
            ViewKind::Image(image) => {
                let link = block.attributes.get("link").cloned();
                self.figure(block, "left", |writer| {
                    writer.image(image, Format::default(), link.as_deref());
                });
            }
            ViewKind::Diagram(diagram) => {
                let alt = block
                    .attributes
                    .get("alt")
                    .cloned()
                    .unwrap_or_else(|| "diagram".to_owned());
                let size = (
                    block.attributes.get("width").cloned(),
                    block.attributes.get("height").cloned(),
                );
                let drawn = self.rendered(
                    diagram,
                    (size.0.as_deref(), size.1.as_deref()),
                    "a diagram's",
                );
                if let Some(drawn) = &drawn {
                    self.figure(block, "left", |writer| writer.picture(drawn, &alt));
                } else {
                    if let adoc_render::DiagramView::Pending { source, .. }
                    | adoc_render::DiagramView::Failed { source, .. } = diagram
                    {
                        self.preformatted("LiteralBlock", source);
                    }
                    // A diagram that failed is reported by whoever rendered it.
                    if let adoc_render::DiagramView::Pending { .. } = diagram {
                        self.warn(
                            Some(&block.span),
                            "a diagram that is not rendered yet is exported as its source"
                                .to_owned(),
                        );
                    }
                }
            }
            ViewKind::Stem {
                source, rendered, ..
            } => {
                let drawn = self.rendered(rendered, (None, None), "a formula's");
                if let Some(drawn) = &drawn {
                    self.figure(block, "center", |writer| writer.picture(drawn, source));
                } else {
                    self.preformatted("LiteralBlock", source);
                    if !matches!(rendered, adoc_render::DiagramView::Svg(_)) {
                        self.warn(
                            Some(&block.span),
                            "a formula that could not be typeset is exported as its source"
                                .to_owned(),
                        );
                    }
                }
            }
            kind @ ViewKind::Passthrough(text) => {
                self.title(block);
                self.preformatted("LiteralBlock", text);
                self.warn(
                    Some(&block.span),
                    format!("{} is exported as text", describe(kind)),
                );
            }
        }
    }

    /// A figure: a paragraph aligned as the block's `align` says (else `default`), holding what
    /// `content` writes, then the block's title below it, as Asciidoctor places a figure's
    /// caption.
    fn figure(
        &mut self,
        block: &ViewBlock,
        default: &'static str,
        content: impl FnOnce(&mut Self),
    ) {
        let align = match block
            .attributes
            .get("align")
            .map(String::as_str)
            .unwrap_or(default)
        {
            "center" | "centre" => Some("center"),
            "right" => Some("right"),
            _ => None,
        };
        let align = align.or(self.align);
        let outer = std::mem::replace(&mut self.align, align);
        self.paragraph(Para::default(), content);
        // The caption is aligned with its picture.
        self.title(block);
        self.align = outer;
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
