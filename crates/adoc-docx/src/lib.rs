//! DOCX export: the view tree written as an editable Microsoft Word document
//! (WordprocessingML in an OPC package), with no Word runtime and no PDF or Typst in between.
//!
//! [`write_docx`] turns a [`DocxDocument`], an article or a book of pages, into a `.docx`
//! package written to any [`Write`] sink. The package is written in-house rather than through
//! an OOXML library: every string goes through one escaping writer (`xml.rs`), so a document can
//! never break a part's markup, and the same input always gives the same bytes, with no
//! timestamps and no process-wide counters (PRD §9; the choice is recorded in the PRD's §14).

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    io::Write,
    path::PathBuf,
};

use adoc_dom::SourceSpan;
use adoc_render::ViewTree;
pub use fonts::{FontFamily, FontSource};
pub use look::{
    Border, BorderSides, BoxLook, RunningLook, StyleLook, TitlePageLook, WordLook, MAPPED_KEYS,
};
pub use template::{Template, TemplateError, TemplatePage, MAX_TEMPLATE_BYTES};
pub use unzip::UnzipError;

mod body;
mod drawing;
mod fonts;
mod inline;
mod labels;
mod look;
mod media;
mod merge;
mod numbering;
mod package;
mod pages;
mod parts;
mod table;
mod template;
#[cfg(feature = "test-support")]
pub mod testing;
mod unzip;
mod xml;
mod zip;

/// One page of a document: its view tree, and how far its headings move down.
#[derive(Clone, Debug)]
pub struct DocxPage {
    /// A short key unique in the document, for the page's bookmarks.
    pub key: String,
    pub view: ViewTree,
    /// How far the page's sections move down: 0 for an article, the page's navigation depth in
    /// a book, where its title becomes a heading at that depth.
    pub depth: u8,
}

/// A book: its title, then parts, chapters and pages in order.
#[derive(Clone, Debug, Default)]
pub struct DocxBook {
    pub title: String,
    pub subtitle: Option<String>,
    /// The book's attributes, as a document header would set them (`author`, `description`,
    /// `keywords`, …).
    pub attributes: BTreeMap<String, String>,
    pub items: Vec<BookItem>,
}

/// One item of a book, in order.
#[derive(Clone, Debug)]
pub enum BookItem {
    /// A part title, starting a new page.
    Part(String),
    /// A heading with no page of its own, at a depth (1 for a chapter).
    Heading { title: String, depth: u8 },
    /// A page; its title becomes a heading at the page's depth.
    Page(DocxPage),
}

/// What is exported: one page as an article, or pages in order as a book.
#[derive(Clone, Debug)]
pub enum DocxDocument {
    Article(DocxPage),
    Book(DocxBook),
}

/// The paper the document is laid out on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Paper {
    #[default]
    A4,
    Letter,
    /// Any other size, in portrait, in twentieths of a point: a theme's.
    Custom {
        width: u32,
        height: u32,
    },
}

impl Paper {
    /// Its width and height in portrait, in twentieths of a point.
    #[must_use]
    pub fn size(self) -> (u32, u32) {
        match self {
            Self::A4 => (11906, 16838),
            Self::Letter => (12240, 15840),
            Self::Custom { width, height } => (width.max(1440), height.max(1440)),
        }
    }
}

/// A page's margins and how far its header and footer are from its edges, in twentieths of a
/// point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Margins {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
    pub header: u32,
    pub footer: u32,
}

impl Default for Margins {
    /// An inch all round, the header and footer half an inch from the edge.
    fn default() -> Self {
        Self {
            top: 1440,
            right: 1440,
            bottom: 1440,
            left: 1440,
            header: 708,
            footer: 708,
        }
    }
}

/// One line of a header or footer: what is at its left, centre and right. Each may hold
/// `{page-number}` and `{page-count}` (live Word fields), `{document-title}`,
/// `{chapter-title}` (a field showing the current chapter's heading) and `{logo}`
/// ([`Running::logo`]).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Line {
    pub left: String,
    pub centre: String,
    pub right: String,
}

impl Line {
    pub(crate) fn is_empty(&self) -> bool {
        self.left.trim().is_empty() && self.centre.trim().is_empty() && self.right.trim().is_empty()
    }
}

/// A header's or footer's lines: on every page (on odd pages, when they differ), and, when
/// given, on even pages and on the first page of the body.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Slots {
    pub odd: Line,
    pub even: Option<Line>,
    pub first: Option<Line>,
}

/// The running content of the body's pages: never on covers or the title page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Running {
    pub header: Slots,
    pub footer: Slots,
    /// Odd and even pages have different headers and footers: [`Slots::even`], or the odd
    /// pages' lines mirrored when none is given.
    pub odd_even: bool,
    /// The image `{logo}` shows.
    pub logo: Option<PathBuf>,
}

impl Default for Running {
    /// As asciidoctor-pdf's default theme: the page number in the footer, at the right of odd
    /// pages and the left of even ones.
    fn default() -> Self {
        Self {
            header: Slots::default(),
            footer: Slots {
                odd: Line {
                    right: "{page-number}".to_owned(),
                    ..Line::default()
                },
                even: Some(Line {
                    left: "{page-number}".to_owned(),
                    ..Line::default()
                }),
                first: None,
            },
            odd_even: true,
            logo: None,
        }
    }
}

/// Changes to the built-in styles. A font is a family name Word looks up on the reader's
/// machine; a colour is six hexadecimal digits (`1F3864`); the size is in half-points (22 is
/// 11 pt). Whoever fills these checks them; the writer only escapes them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StyleOverrides {
    /// Body text, and every style based on it.
    pub body_font: Option<String>,
    /// The title, subtitle, headings and the table of contents' heading.
    pub heading_font: Option<String>,
    /// Code, keys, source and literal blocks.
    pub code_font: Option<String>,
    pub body_size: Option<u32>,
    pub heading_colour: Option<String>,
    pub link_colour: Option<String>,
}

/// How a document is exported.
#[derive(Clone, Debug, Default)]
pub struct DocxOptions {
    /// A PDF theme's (or a template's) look, under `styles`.
    pub look: WordLook,
    pub styles: StyleOverrides,
    pub paper: Paper,
    pub landscape: bool,
    pub margins: Margins,
    pub running: Running,
    /// The address of each page a reference out of the document may lead to; a reference to
    /// any other page outside the document is left as text.
    pub page_urls: HashMap<PathBuf, String>,
    /// Which files the export may read: images, diagrams, formulas and fonts.
    pub access: adoc_core::FileAccess,
    /// Name the look's fonts without embedding them (`[docx] embed_fonts = false`).
    pub fonts_named_only: bool,
}

impl DocxOptions {
    /// The page's width and height, as laid out, in twentieths of a point.
    #[must_use]
    pub fn page_size(&self) -> (u32, u32) {
        let (width, height) = self.paper.size();
        if self.landscape {
            (height, width)
        } else {
            (width, height)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocxSeverity {
    Error,
    Warning,
}

/// A problem found while exporting, with the AsciiDoc it concerns when known.
#[derive(Clone, Debug, PartialEq)]
pub struct DocxDiagnostic {
    pub severity: DocxSeverity,
    pub message: String,
    pub source: Option<SourceSpan>,
}

impl fmt::Display for DocxDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source) = &self.source {
            write!(formatter, "{}:{}: ", source.path.display(), source.line)?;
        }
        formatter.write_str(&self.message)
    }
}

/// What an export reports: the diagnostics of a document that was written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocxReport {
    pub diagnostics: Vec<DocxDiagnostic>,
}

/// Why a document could not be written.
#[derive(Debug)]
pub enum DocxError {
    /// The sink refused the bytes.
    Io(std::io::Error),
    /// The package is past what a ZIP file without its 64-bit extensions holds: 65,535 parts,
    /// or 4 GiB in a part or in all.
    TooLarge,
}

impl fmt::Display for DocxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "writing the document: {error}"),
            Self::TooLarge => formatter.write_str("the document is too large for a DOCX package"),
        }
    }
}

impl std::error::Error for DocxError {}

impl From<std::io::Error> for DocxError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Writes `document` to `sink` as a `.docx` package. The sink receives the package as it is
/// made, a part at a time; whoever owns the file writes it somewhere temporary and moves it into
/// place only once this returns `Ok`.
pub fn write_docx(
    document: &DocxDocument,
    options: &DocxOptions,
    sink: impl Write,
) -> Result<DocxReport, DocxError> {
    let mut writer = body::Writer::new(document, options);
    writer.document(document);
    // The look's fonts the styles name, under the names the fonts give themselves.
    let fonts = fonts::Fonts::new(
        &options.look.fonts,
        &parts::fonts_named(&options.styles, &options.look),
        !options.fonts_named_only,
        &options.access,
    );
    for warning in &fonts.warnings {
        writer.warn(None, warning.clone());
    }
    // What a template held that the export leaves out.
    for warning in options
        .look
        .template
        .iter()
        .flat_map(|template| &template.warnings)
    {
        writer.warn(None, warning.clone());
    }
    let relationship = |id: &str, kind, target: &str| package::Relationship {
        id: id.to_owned(),
        kind,
        target: target.to_owned(),
        external: false,
    };
    // A part's hyperlinks and pictures: relationships of that part.
    let media = std::mem::take(&mut writer.media.parts);
    let links = |part: body::Part| {
        let hyperlinks = writer.hyperlinks[part.0]
            .iter()
            .map(|link| package::Relationship {
                id: link.id.clone(),
                kind: parts::HYPERLINK,
                target: link.url.clone(),
                external: true,
            });
        let pictures = writer.pictures[part.0]
            .iter()
            .map(|(id, index)| package::Relationship {
                id: id.clone(),
                kind: parts::IMAGE,
                target: media[*index]
                    .name
                    .strip_prefix("word/")
                    .unwrap_or(&media[*index].name)
                    .to_owned(),
                external: false,
            });
        hyperlinks.chain(pictures).collect::<Vec<_>>()
    };
    let mut document_relationships = vec![
        relationship("rId1", parts::STYLES, "styles.xml"),
        relationship("rId2", parts::SETTINGS, "settings.xml"),
    ];
    let mut parts = Vec::new();
    let footnotes = writer.footnote_count > 0;
    let template = options.look.template.as_deref();
    // A template's numbering is kept whether or not the export writes a list: its styles may
    // name it.
    let numbering = match template.and_then(|template| template.numbering.as_deref()) {
        Some(theirs) => Some(merge::numbering_part(
            theirs,
            writer.numbering.part().as_deref(),
        )),
        None => writer.numbering.part(),
    };
    if let Some(numbering) = numbering {
        document_relationships.push(relationship("rId3", parts::NUMBERING, "numbering.xml"));
        // A template's picture bullets.
        let mut relationships = Vec::new();
        for (id, target) in template
            .iter()
            .flat_map(|template| &template.numbering_relationships)
        {
            if let template::TemplateTarget::Picture(index) = target {
                writer.template_media.insert(*index);
                relationships.push(package::Relationship {
                    id: id.clone(),
                    kind: parts::IMAGE,
                    target: template.map_or_else(String::new, |template| {
                        template.media[*index]
                            .name
                            .trim_start_matches("word/")
                            .to_owned()
                    }),
                    external: false,
                });
            }
        }
        parts.push(package::Part {
            name: "word/numbering.xml".to_owned(),
            content_type: parts::NUMBERING_TYPE,
            data: numbering,
            relationships,
        });
    }
    if footnotes {
        document_relationships.push(relationship("rId4", parts::FOOTNOTES, "footnotes.xml"));
        parts.push(package::Part {
            name: "word/footnotes.xml".to_owned(),
            content_type: parts::FOOTNOTES_TYPE,
            data: inline::footnotes_part(std::mem::take(&mut writer.footnotes)),
            relationships: links(body::Part::FOOTNOTES),
        });
    }
    // A template's own font table, with the fonts it embeds, when the look names none.
    let template_fonts = template
        .and_then(|template| template.fonts.as_ref())
        .filter(|_| fonts.is_empty());
    if !fonts.is_empty() || template_fonts.is_some() {
        document_relationships.push(relationship("rId5", parts::FONT_TABLE, "fontTable.xml"));
    }
    if let Some(theme) = template.and_then(|template| template.theme.as_ref()) {
        document_relationships.push(relationship("rId6", template::THEME, "theme/theme1.xml"));
        parts.push(package::Part {
            name: "word/theme/theme1.xml".to_owned(),
            content_type: template::THEME_TYPE,
            data: theme.clone(),
            relationships: Vec::new(),
        });
    }
    document_relationships.extend(links(body::Part::DOCUMENT));
    // Odd and even pages differ only when the body has running content of its own for them.
    let odd_even = if template.is_some() {
        writer.odd_even
    } else {
        options.running.odd_even && !writer.running_parts.is_empty()
    };
    for (id, kind, target) in std::mem::take(&mut writer.document_links) {
        document_relationships.push(package::Relationship {
            id,
            kind,
            target,
            external: false,
        });
    }
    let template_media = |index: usize| {
        template
            .and_then(|template| template.media.get(index))
            .map_or_else(String::new, |media| {
                media
                    .name
                    .strip_prefix("word/")
                    .unwrap_or(&media.name)
                    .to_owned()
            })
    };
    for running in std::mem::take(&mut writer.running_parts) {
        let mut relationships = links(running.part);
        relationships.extend(
            running
                .template
                .into_iter()
                .map(|(id, target)| match target {
                    template::TemplateTarget::Picture(index) => package::Relationship {
                        id,
                        kind: parts::IMAGE,
                        target: template_media(index),
                        external: false,
                    },
                    template::TemplateTarget::Link(url) => package::Relationship {
                        id,
                        kind: parts::HYPERLINK,
                        target: url,
                        external: true,
                    },
                }),
        );
        parts.push(package::Part {
            name: running.name,
            content_type: running.content_type,
            data: running.data,
            relationships,
        });
    }
    if let Some(template) = template {
        for index in std::mem::take(&mut writer.template_media) {
            if let Some(media) = template.media.get(index) {
                parts.push(package::Part {
                    name: media.name.clone(),
                    content_type: media.content_type,
                    data: media.data.clone(),
                    relationships: Vec::new(),
                });
            }
        }
    }
    parts.extend(media.into_iter().map(|part| package::Part {
        name: part.name,
        content_type: part.content_type,
        data: part.data,
        relationships: Vec::new(),
    }));
    let body = std::mem::take(&mut writer.xml);
    parts.splice(
        0..0,
        [
            package::Part {
                name: "word/document.xml".to_owned(),
                content_type: parts::DOCUMENT_TYPE,
                data: parts::document(body, std::mem::take(&mut writer.final_section)),
                relationships: document_relationships,
            },
            package::Part {
                name: "word/styles.xml".to_owned(),
                content_type: parts::STYLES_TYPE,
                data: {
                    let ours = parts::styles(writer.width, &options.styles, &options.look, &fonts);
                    match template {
                        Some(template) => merge::styles_part(
                            &template.styles,
                            &String::from_utf8_lossy(&ours),
                            |id| parts::written_look(&options.styles, id),
                            &StyleLook {
                                font: options.styles.body_font.clone(),
                                size: options.styles.body_size,
                                ..StyleLook::default()
                            },
                        ),
                        None => ours,
                    }
                },
                relationships: Vec::new(),
            },
            package::Part {
                name: "word/settings.xml".to_owned(),
                content_type: parts::SETTINGS_TYPE,
                data: parts::settings(
                    footnotes,
                    odd_even,
                    writer.wrote_toc,
                    fonts.embeds()
                        || template_fonts
                            .is_some_and(|fonts| fonts.embeds && !options.fonts_named_only),
                ),
                relationships: Vec::new(),
            },
        ],
    );
    if !fonts.is_empty() {
        let (table, targets) = fonts.table_part();
        parts.push(package::Part {
            name: "word/fontTable.xml".to_owned(),
            content_type: parts::FONT_TABLE_TYPE,
            data: table,
            relationships: targets
                .into_iter()
                .map(|(id, target)| package::Relationship {
                    id,
                    kind: parts::FONT,
                    target,
                    external: false,
                })
                .collect(),
        });
        parts.extend(fonts.into_parts().map(|(name, data)| package::Part {
            name,
            content_type: parts::FONT_TYPE,
            data,
            relationships: Vec::new(),
        }));
    } else if let Some(theirs) = template_fonts.filter(|_| options.fonts_named_only) {
        // `[docx] embed_fonts = false`: the template's families named, none embedded.
        parts.push(package::Part {
            name: "word/fontTable.xml".to_owned(),
            content_type: parts::FONT_TABLE_TYPE,
            data: merge::without_embeds(&theirs.table),
            relationships: Vec::new(),
        });
    } else if let Some(theirs) = template_fonts {
        parts.push(package::Part {
            name: "word/fontTable.xml".to_owned(),
            content_type: parts::FONT_TABLE_TYPE,
            data: theirs.table.clone(),
            relationships: theirs
                .fonts
                .iter()
                .enumerate()
                .map(|(index, (id, _))| package::Relationship {
                    id: id.clone(),
                    kind: parts::FONT,
                    target: format!("fonts/template-font{}.odttf", index + 1),
                    external: false,
                })
                .collect(),
        });
        parts.extend(
            theirs
                .fonts
                .iter()
                .enumerate()
                .map(|(index, (_, data))| package::Part {
                    name: format!("word/fonts/template-font{}.odttf", index + 1),
                    content_type: parts::FONT_TYPE,
                    data: data.clone(),
                    relationships: Vec::new(),
                }),
        );
    }
    parts.push(package::Part {
        name: "docProps/core.xml".to_owned(),
        content_type: parts::CORE_TYPE,
        data: parts::core(&facts(document)),
        relationships: Vec::new(),
    });
    package::write(
        &[
            relationship("rId1", parts::OFFICE_DOCUMENT, "word/document.xml"),
            relationship("rId2", parts::CORE_PROPERTIES, "docProps/core.xml"),
        ],
        &parts,
        sink,
    )?;
    Ok(DocxReport {
        diagnostics: writer.diagnostics,
    })
}

/// The document's title, author, description and keywords, for its core properties.
fn facts(document: &DocxDocument) -> parts::Facts {
    let (title, attributes) = match document {
        DocxDocument::Article(page) => (
            page.view
                .title
                .as_deref()
                .map(adoc_render::ViewInline::plain_text),
            &page.view.attributes,
        ),
        DocxDocument::Book(book) => (Some(book.title.clone()), &book.attributes),
    };
    let attribute = |name: &str| attributes.get(name).cloned();
    parts::Facts {
        title,
        creator: attribute("author").or_else(|| attribute("authors")),
        description: attribute("description"),
        keywords: attribute("keywords"),
    }
}
