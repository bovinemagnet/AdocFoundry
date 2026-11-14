//! The whole Typst document: the theme's keys, the document's facts, the template, the front
//! matter and the body, for an article or a book.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    path::PathBuf,
};

use adoc_core::FileAccess;

use adoc_render::{SourceSpan, ViewImage, ViewInline};

use crate::{
    emit::{EmitOptions, Emitted, Emitter, PageView, SourceMap},
    icons, running,
    theme::{LoadedTheme, ThemeValue},
    typst::{self, string, text},
};

/// The template the generated body is typeset with.
const TEMPLATE: &str = include_str!("adoc.typ");
/// STEM's helpers and `adoc-stem`, ahead of the template and in a formula's own document.
pub(crate) const MATH: &str = include_str!("math.typ");

/// The body's start: running content and the page count begin here. Typst numbers a page by
/// its counter after the updates on it, so this page is 1 in the viewer as in the footer.
const RUNNING_START: &str = "#metadata(none) <adoc-running-start>#counter(page).update(1)\n";

/// A PDF standard the output must conform to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PdfStandard {
    /// PDF/A-1b: archival, PDF 1.4.
    A1b,
    /// PDF/A-1a: archival, PDF 1.4, tagged.
    A1a,
    /// PDF/A-2a: archival, tagged.
    A2a,
    /// PDF/UA-1: accessible.
    Ua1,
}

/// How a document is exported.
#[derive(Clone, Debug, Default)]
pub struct PdfOptions {
    pub standards: Vec<PdfStandard>,
    /// Where relative font paths in the theme are found, in order (asciidoctor-pdf's
    /// `pdf-fontsdir`); `GEM_FONTS_DIR` stands for the bundled fonts.
    pub fonts_dirs: Vec<PathBuf>,
    /// The address of each page a reference out of the document may lead to; a reference to
    /// any other page is left as text.
    pub page_urls: HashMap<PathBuf, String>,
    /// The date written into the PDF (year, month, day), for reproducible output; none by
    /// default.
    pub date: Option<(i32, u8, u8)>,
    /// Which files the export may read: images, fonts and every other file Typst asks for.
    pub access: FileAccess,
}

/// What is exported: one page as an article, or pages in order as a book.
#[derive(Clone, Debug)]
pub enum Document {
    Article(PageView),
    Book(Book),
}

/// A book: a title page, a table of contents, then parts, chapters and sections in order.
#[derive(Clone, Debug, Default)]
pub struct Book {
    pub title: String,
    pub subtitle: Option<String>,
    /// The book's attributes, as a document header would set them (`revnumber`, `toc`, …).
    pub attributes: BTreeMap<String, String>,
    pub items: Vec<BookItem>,
}

#[derive(Clone, Debug)]
pub enum BookItem {
    /// A part title, on a page of its own.
    Part(String),
    /// A heading with no page of its own, at a depth (1 for a chapter).
    Heading { title: String, depth: u8 },
    /// A page; its title becomes a heading at the page's depth, its sections move down by it.
    Page(PageView),
}

impl Document {
    /// The image a document attribute names, resolved as the page's images are: an article's
    /// own, or, for a book, that of its first page with the book's value for the attribute.
    fn attribute_image(&self, name: &str) -> Option<&ViewImage> {
        match self {
            Self::Article(page) => page.view.attribute_images.get(name),
            Self::Book(book) => book.items.iter().find_map(|item| match item {
                BookItem::Page(page)
                    if page.view.attributes.get(name) == book.attributes.get(name) =>
                {
                    page.view.attribute_images.get(name)
                }
                _ => None,
            }),
        }
    }

    fn pages(&self) -> Vec<&PageView> {
        match self {
            Self::Article(page) => vec![page],
            Self::Book(book) => book
                .items
                .iter()
                .filter_map(|item| match item {
                    BookItem::Page(page) => Some(page),
                    _ => None,
                })
                .collect(),
        }
    }
}

/// The generated Typst for a document, with the files it uses.
#[derive(Clone, Debug, Default)]
pub struct Typst {
    pub source: String,
    /// Virtual path, real file.
    pub files: Vec<(String, PathBuf)>,
    pub map: SourceMap,
    /// Images that could not be shown, missing or unreadable, and where they were written.
    pub missing: Vec<(String, adoc_render::SourceSpan)>,
    /// Formulas that could not be typeset, why, and where they were written.
    pub unconverted: Vec<(String, adoc_render::SourceSpan)>,
    /// What else was found writing the document that the reader should know: a page size by a
    /// name neither asciidoctor-pdf nor this knows.
    pub warnings: Vec<String>,
}

/// The document's facts the template reads: an article's attributes, or a book's.
struct Facts<'a> {
    attributes: &'a BTreeMap<String, String>,
    title: Option<String>,
    book: bool,
}

impl Facts<'_> {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    fn has(&self, name: &str) -> bool {
        self.attributes.contains_key(name)
    }

    fn toc_levels(&self) -> u8 {
        self.attribute("toclevels")
            .and_then(|levels| levels.parse().ok())
            .unwrap_or(2)
    }

    /// A book, or an article that asks for one, opens with a title page.
    fn title_as_page(&self) -> bool {
        self.title.is_some()
            && (self.book || self.attribute("doctype") == Some("book") || self.has("title-page"))
            && !self.has("notitle")
    }

    /// The table of contents goes at the top unless it is placed by the `toc::[]` macro.
    fn toc_at_top(&self) -> bool {
        self.has("toc")
            && !matches!(self.attribute("toc-placement"), Some("macro" | "preamble"))
            && !matches!(self.attribute("toc"), Some("macro" | "preamble"))
    }
}

/// Writes `document` as one Typst document styled by `theme`.
#[must_use]
pub fn to_typst(document: &Document, theme: &LoadedTheme, options: &PdfOptions) -> Typst {
    let pages = document.pages();
    let (attributes, title) = match document {
        Document::Article(page) => (
            page.view.attributes.clone(),
            page.view
                .title
                .as_ref()
                .map(|title| ViewInline::plain_text(title)),
        ),
        Document::Book(book) => (book.attributes.clone(), Some(book.title.clone())),
    };
    let book = matches!(document, Document::Book(_));
    let facts = Facts {
        attributes: &attributes,
        title: title.clone(),
        book,
    };
    let geometry = page_geometry(theme, &attributes);
    let emit_options = EmitOptions {
        highlight: facts.has("source-highlighter"),
        page_urls: &options.page_urls,
        toc_levels: facts.toc_levels(),
        access: &options.access,
        fits_lines: fits_lines(theme, &geometry),
        // Font Awesome's glyphs are private-use code points, which PDF/A-2a forbids.
        font_icons: facts.attribute("icons") == Some("font")
            && !options.standards.contains(&PdfStandard::A2a),
    };
    let mut emitter = Emitter::new(&pages, &emit_options);
    // Images that cannot be shown are reported against the first page.
    let span = SourceSpan {
        path: pages
            .first()
            .map(|page| page.view.source.clone())
            .unwrap_or_default(),
        ..SourceSpan::default()
    };

    // The front cover, then the front matter.
    if let Some(front) = cover(document, &facts, theme, Face::Front) {
        front.emit(&mut emitter, &span, Face::Front);
    }
    if facts.title_as_page() {
        if let Some(title) = &title {
            let (main, subtitle) = match document {
                Document::Book(book) => (book.title.clone(), book.subtitle.clone()),
                Document::Article(_) => running::partition_title(title),
            };
            let title_page = running::TitlePage {
                title: main,
                subtitle,
                attributes: &attributes,
                logo: document.attribute_image("title-logo-image"),
            };
            let markup = title_page.markup(theme, &mut |image| emitter.image_file(image, &span));
            emitter.push(&markup);
        }
        if facts.toc_at_top() {
            let min_level = if book && has_parts(document) { 1 } else { 2 };
            // Front matter is labelled in roman numerals, as asciidoctor-pdf labels it; the
            // contents are pages of their own, so the body starts on a page of its own too.
            emitter.push(&format!(
                "#page(numbering: \"i\")[#adoc-toc(min-level: {min_level})]\n\n"
            ));
        }
        emitter.push(RUNNING_START);
    } else {
        emitter.push(RUNNING_START);
        if let (Document::Article(page), false) = (document, facts.has("notitle")) {
            if let Some(title) = &page.view.title {
                emitter.begin_page(page);
                let title = emitter.inlines(title);
                emitter.push(&format!("#adoc-doctitle[{title}]\n\n"));
            }
        }
        if facts.toc_at_top() {
            emitter.push("#adoc-toc()\n\n#v(block-bottom)\n\n");
        }
    }

    // The body.
    let toc_levels = facts.toc_levels();
    match document {
        Document::Article(page) => {
            emitter.begin_page(page);
            let anchor = emitter.label("");
            emitter.push(&format!("#metadata(none)#{anchor}\n\n"));
            emitter.page_body(&page.view);
            emitter.footnotes(&page.view);
        }
        Document::Book(book) => {
            for item in &book.items {
                match item {
                    BookItem::Part(title) => {
                        emitter.push(&format!("#adoc-part[{}]\n\n", text(title)));
                    }
                    BookItem::Heading { title, depth } => {
                        heading(&mut emitter, *depth, &text(title), None, "", toc_levels);
                    }
                    BookItem::Page(page) => {
                        emitter.begin_page(page);
                        let anchor = emitter.label("");
                        let (title, markers) = page
                            .view
                            .title
                            .as_ref()
                            .map(|title| emitter.heading_title(title))
                            .unwrap_or_default();
                        heading(
                            &mut emitter,
                            page.depth.max(1),
                            &title,
                            Some(&anchor),
                            &markers,
                            toc_levels,
                        );
                        emitter.page_body(&page.view);
                        emitter.footnotes(&page.view);
                    }
                }
            }
        }
    }

    if let Some(back) = cover(document, &facts, theme, Face::Back) {
        back.emit(&mut emitter, &span, Face::Back);
    }

    let mut attributes = attributes.clone();
    if let Some(title) = &title {
        attributes
            .entry("doctitle".to_owned())
            .or_insert_with(|| title.clone());
    }
    attributes
        .entry("page-layout".to_owned())
        .or_insert_with(|| {
            if geometry.landscape {
                "landscape"
            } else {
                "portrait"
            }
            .to_owned()
        });
    let running = running::running_dictionary(theme, &attributes, &mut |image| {
        emitter.image_file(image, &span)
    });
    let Emitted {
        source: body,
        files,
        mut map,
        missing,
        unconverted,
    } = emitter.finish();

    let mut source = String::new();
    source
        .push_str("// Generated by AdocFoundry from AsciiDoc; edit the AsciiDoc, not this file.\n");
    theme_dictionary(&mut source, theme);
    doc_dictionary(
        &mut source,
        &facts,
        theme,
        &crate::fonts::line_gaps(theme, &options.fonts_dirs, &options.access),
    );
    source.push_str(MATH);
    source.push_str(TEMPLATE);
    source.push('\n');
    source.push_str(&running);
    document_metadata(&mut source, &facts);
    source.push_str("#show: adoc-page-setup.with(running: running)\n\n");
    map.shift(source.len());
    source.push_str(&body);
    Typst {
        source,
        files,
        map,
        missing,
        unconverted,
        warnings: geometry.warnings,
    }
}

/// The side of the document a cover is on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Face {
    Front,
    Back,
}

impl Face {
    fn name(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
        }
    }
}

/// A cover page.
#[derive(Clone, Debug)]
enum Cover {
    /// A blank page, for an empty attribute.
    Blank,
    /// An image fitted to the page as `fit` says, and the page of a PDF to draw.
    Image {
        image: ViewImage,
        fit: CoverFit,
        page: u32,
    },
}

/// How a cover image fills its page, from the image macro's `fit`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoverFit {
    /// Inside the page, the default.
    Contain,
    /// Over the whole page, cut at its edges.
    Cover,
    /// Stretched to the page.
    Fill,
    /// At its own size, no larger than the page (`none`, `scale-down`).
    Natural,
}

impl Cover {
    fn emit(&self, emitter: &mut Emitter<'_>, span: &SourceSpan, face: Face) {
        match self {
            Self::Blank => emitter.blank_cover(face),
            Self::Image { image, fit, page } => emitter.cover(image, span, *fit, *page, face),
        }
    }
}

/// The cover on `face`, as asciidoctor-pdf finds it: the document's `front-cover-image` or
/// `back-cover-image`, else the theme's `cover_front_image` or `cover_back_image`; an image
/// macro or a path, `none` or `~` for no cover, empty for a blank page. The document's image is
/// found as its other images are; the theme's beside the theme.
fn cover(document: &Document, facts: &Facts<'_>, theme: &LoadedTheme, face: Face) -> Option<Cover> {
    let name = format!("{}-cover-image", face.name());
    let (value, from_theme) = match facts.attribute(&name) {
        Some(value) => (value.to_owned(), false),
        None => match theme.keys.get(&format!("cover_{}_image", face.name())) {
            Some(ThemeValue::Text(value)) => (value.clone(), true),
            _ => return None,
        },
    };
    if value.trim().is_empty() {
        return Some(Cover::Blank);
    }
    let named = adoc_render::attribute_image(&value)?;
    let image = if from_theme {
        let path = theme
            .dir
            .as_ref()
            .map_or_else(|| PathBuf::from(named.target), |dir| dir.join(named.target));
        ViewImage {
            target: named.target.to_owned(),
            path: path.is_file().then_some(path),
            url: None,
            alt: String::new(),
            width: None,
            height: None,
        }
    } else {
        document
            .attribute_image(&name)
            .cloned()
            .unwrap_or_else(|| ViewImage {
                target: named.target.to_owned(),
                path: None,
                url: None,
                alt: String::new(),
                width: None,
                height: None,
            })
    };
    let attribute = |key: &str| {
        named.attributes.split(',').find_map(|attribute| {
            let (name, value) = attribute.split_once('=')?;
            (name.trim() == key).then(|| value.trim().trim_matches(['"', '\'']).to_owned())
        })
    };
    let fit = match attribute("fit").as_deref() {
        Some("cover") => CoverFit::Cover,
        Some("fill") => CoverFit::Fill,
        Some("none" | "scale-down") => CoverFit::Natural,
        _ => CoverFit::Contain,
    };
    let page = attribute("page")
        .and_then(|page| page.parse().ok())
        .filter(|page| *page >= 1)
        .unwrap_or(1);
    Some(Cover::Image { image, fit, page })
}

/// A book heading at `depth`: a chapter (depth 1) starts a new page. `markers`, the title's
/// anchors and index terms, follow it. It is in the contents as a section at its level would
/// be: down to `toc_levels`.
fn heading(
    emitter: &mut Emitter<'_>,
    depth: u8,
    title: &str,
    label: Option<&str>,
    markers: &str,
    toc_levels: u8,
) {
    if depth <= 1 {
        emitter.push("#pagebreak(weak: true)\n\n");
    }
    let label = label.map(|label| format!("#{label}")).unwrap_or_default();
    emitter.push(&format!(
        "#heading(level: {}, outlined: {}, bookmarked: true)[{title}]{label}{markers}\n\n",
        u32::from(depth) + 1,
        depth <= toc_levels
    ));
}

fn has_parts(document: &Document) -> bool {
    matches!(document, Document::Book(book) if book.items.iter().any(|item| matches!(item, BookItem::Part(_))))
}

fn theme_dictionary(out: &mut String, theme: &LoadedTheme) {
    out.push_str("#let t = (\n");
    for (key, value) in theme.keys.iter() {
        let _ = writeln!(
            out,
            "  {}: {},",
            string(key),
            typst::theme_value(key, value)
        );
    }
    out.push_str(")\n");
}

fn doc_dictionary(
    out: &mut String,
    facts: &Facts<'_>,
    theme: &LoadedTheme,
    gaps: &[(String, f64)],
) {
    let page = page_geometry(theme, facts.attributes);
    let (running_at, running_from) = running_start(theme, facts);
    let fallbacks: Vec<String> = theme.fallbacks.iter().map(|name| string(name)).collect();
    let toc_title = facts
        .attribute("toc-title")
        .unwrap_or("Table of Contents")
        .to_owned();
    let _ = writeln!(
        out,
        "#let doc = (\n  line-gaps: ({}),\n  lang: {},\n  region: {},\n  hyphenate: {},\n  fallbacks: ({}),\n  page: (width: {}, height: {}, margin: (top: {}, right: {}, bottom: {}, left: {})),\n  toc-title: {},\n  toc-levels: {},\n  running: {},\n  running-at: {},\n  running-from: {},\n  title: {},\n  admonition-icons: ({}),\n)",
        if gaps.is_empty() {
            ":".to_owned()
        } else {
            gaps.iter()
                .map(|(family, gap)| format!("{}: {}, ", string(&family.to_lowercase()), typst::number(*gap)))
                .collect::<String>()
        },
        string(&language(facts.attribute("lang")).0),
        language(facts.attribute("lang"))
            .1
            .map_or_else(|| "none".to_owned(), |region| string(&region)),
        facts.has("hyphens"),
        fallbacks.iter().map(|name| format!("{name}, ")).collect::<String>(),
        typst::points(page.width),
        typst::points(page.height),
        typst::points(page.margin[0]),
        typst::points(page.margin[1]),
        typst::points(page.margin[2]),
        typst::points(page.margin[3]),
        string(&toc_title),
        facts.toc_levels(),
        if facts.book {
            format!("(part: (1, 1), chapter: (2, 2), section: (3, {}))", section_levels(theme) + 1)
        } else {
            format!("(part: none, chapter: none, section: (2, {}))", section_levels(theme) + 1)
        },
        string(running_at.name()),
        running_from,
        facts.title.as_deref().map_or_else(|| "none".to_owned(), string),
        admonition_icons(theme),
    );
}

/// Each admonition kind's icon for `icons=font`: its font, glyph, size and colour, the glyph a
/// string literal and the rest numbers, as the theme gives them.
fn admonition_icons(theme: &LoadedTheme) -> String {
    let mut out = String::new();
    for kind in icons::KINDS {
        let icon = icons::admonition_icon(theme, kind);
        let _ = write!(
            out,
            "{kind}: (font: {}, glyph: {}, size: {}, color: {}), ",
            string(icon.family),
            string(&icon.glyph.to_string()),
            typst::points(icon.size),
            icon.color
                .as_ref()
                .map_or_else(|| "none".to_owned(), typst::color),
        );
    }
    out
}

fn document_metadata(out: &mut String, facts: &Facts<'_>) {
    let title = facts.title.clone();
    let mut fields = Vec::new();
    if let Some(title) = &title {
        fields.push(format!("title: {}", string(title)));
    }
    let authors: Vec<String> = facts
        .attribute("authors")
        .or_else(|| facts.attribute("author"))
        .map(|authors| {
            authors
                .split(',')
                .map(str::trim)
                .filter(|author| !author.is_empty())
                .map(string)
                .collect()
        })
        .unwrap_or_default();
    if !authors.is_empty() {
        fields.push(format!("author: ({},)", authors.join(", ")));
    }
    if let Some(description) = facts.attribute("description") {
        fields.push(format!("description: {}", string(description)));
    }
    if let Some(keywords) = facts.attribute("keywords") {
        let keywords: Vec<String> = keywords
            .split(',')
            .map(str::trim)
            .filter(|keyword| !keyword.is_empty())
            .map(string)
            .collect();
        if !keywords.is_empty() {
            fields.push(format!("keywords: ({},)", keywords.join(", ")));
        }
    }
    if !fields.is_empty() {
        let _ = writeln!(out, "#set document({})", fields.join(", "));
    }
}

struct PageGeometry {
    width: f64,
    height: f64,
    /// Top, right, bottom, left.
    margin: [f64; 4],
    landscape: bool,
    /// A size by a name that is not known, given as A4.
    warnings: Vec<String>,
}

/// Page sizes asciidoctor-pdf knows by name, in points: Prawn's `PageGeometry::SIZES`.
const PAPER: &[(&str, f64, f64)] = &[
    ("4A0", 4767.87, 6740.79),
    ("2A0", 3370.39, 4767.87),
    ("A0", 2383.94, 3370.39),
    ("A1", 1683.78, 2383.94),
    ("A2", 1190.55, 1683.78),
    ("A3", 841.89, 1190.55),
    ("A4", 595.28, 841.89),
    ("A5", 419.53, 595.28),
    ("A6", 297.64, 419.53),
    ("A7", 209.76, 297.64),
    ("A8", 147.40, 209.76),
    ("A9", 104.88, 147.40),
    ("A10", 73.70, 104.88),
    ("B0", 2834.65, 4008.19),
    ("B1", 2004.09, 2834.65),
    ("B2", 1417.32, 2004.09),
    ("B3", 1000.63, 1417.32),
    ("B4", 708.66, 1000.63),
    ("B5", 498.90, 708.66),
    ("B6", 354.33, 498.90),
    ("B7", 249.45, 354.33),
    ("B8", 175.75, 249.45),
    ("B9", 124.72, 175.75),
    ("B10", 87.87, 124.72),
    ("C0", 2599.37, 3676.54),
    ("C1", 1836.85, 2599.37),
    ("C2", 1298.27, 1836.85),
    ("C3", 918.43, 1298.27),
    ("C4", 649.13, 918.43),
    ("C5", 459.21, 649.13),
    ("C6", 323.15, 459.21),
    ("C7", 229.61, 323.15),
    ("C8", 161.57, 229.61),
    ("C9", 113.39, 161.57),
    ("C10", 79.37, 113.39),
    ("RA0", 2437.80, 3458.27),
    ("RA1", 1729.13, 2437.80),
    ("RA2", 1218.90, 1729.13),
    ("RA3", 864.57, 1218.90),
    ("RA4", 609.45, 864.57),
    ("SRA0", 2551.18, 3628.35),
    ("SRA1", 1814.17, 2551.18),
    ("SRA2", 1275.59, 1814.17),
    ("SRA3", 907.09, 1275.59),
    ("SRA4", 637.80, 907.09),
    ("EXECUTIVE", 521.86, 756.00),
    ("FOLIO", 612.00, 936.00),
    ("LEGAL", 612.00, 1008.00),
    ("LETTER", 612.00, 792.00),
    ("TABLOID", 792.00, 1224.00),
];

/// The page's size and margins: the document's `pdf-page-size`, `pdf-page-layout` and
/// `pdf-page-margin`, else the theme's `page_size`, `page_layout` and `page_margin`, as
/// asciidoctor-pdf reads them. A size by a name neither knows is A4, with a warning.
fn page_geometry(theme: &LoadedTheme, attributes: &BTreeMap<String, String>) -> PageGeometry {
    let keys = &theme.keys;
    let attribute = |name: &str| {
        attributes
            .get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let mut warnings = Vec::new();
    let mut unknown = |size: &str| {
        warnings.push(format!("unknown page size {size}; A4 is used"));
        A4
    };
    let (mut width, mut height) = match attribute("pdf-page-size") {
        Some(size) => page_size(size).unwrap_or_else(|| unknown(size)),
        None => match keys.get("page_size") {
            Some(ThemeValue::Text(name)) => paper(name).unwrap_or_else(|| unknown(name)),
            Some(ThemeValue::List(items)) if items.len() == 2 => (
                items[0].as_number().unwrap_or(A4.0),
                items[1].as_number().unwrap_or(A4.1),
            ),
            _ => A4,
        },
    };
    let landscape = match attribute("pdf-page-layout") {
        Some(layout) => layout == "landscape",
        None => {
            matches!(keys.get("page_layout"), Some(ThemeValue::Text(layout)) if layout == "landscape")
        }
    };
    if landscape {
        std::mem::swap(&mut width, &mut height);
    }
    let margin = match attribute("pdf-page-margin") {
        Some(margin) => sides(Some(&page_margin(margin)), 36.0),
        None => sides(keys.get("page_margin"), 36.0),
    };
    PageGeometry {
        width,
        height,
        margin,
        landscape,
        warnings,
    }
}

const A4: (f64, f64) = (595.28, 841.89);

/// A page size by name, in either case.
fn paper(name: &str) -> Option<(f64, f64)> {
    PAPER
        .iter()
        .find(|(paper, _, _)| paper.eq_ignore_ascii_case(name.trim()))
        .map(|(_, width, height)| (*width, *height))
}

/// A `pdf-page-size`: a name (`Letter`), `[8.5in, 11in]` or `8.5in x 11in`; `None` for a
/// size asciidoctor-pdf would not read.
fn page_size(value: &str) -> Option<(f64, f64)> {
    let pair = value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .and_then(|inner| inner.split_once(','))
        .or_else(|| value.split_once('x'))
        .and_then(|(width, height)| Some((measurement(width)?, measurement(height)?)));
    match pair {
        Some((width, height)) => (width > 0.0 && height > 0.0).then_some((width, height)),
        // A name, `Executive` among them.
        None => paper(value),
    }
}

/// A length with an optional unit (`in`, `mm`, `cm`, `pt`, `px`, `pc`), in points.
fn measurement(value: &str) -> Option<f64> {
    let (number, unit) = crate::typst::number_and_unit(value, &[])?;
    crate::theme::to_pt(number, unit).filter(|points| points.is_finite())
}

/// A `pdf-page-margin`: one length, or up to four in brackets (`[0.5in, 0.67in]`), as
/// asciidoctor-pdf reads it; a value it cannot read counts as 0, as there.
fn page_margin(value: &str) -> ThemeValue {
    let points = |part: &str| ThemeValue::Number(measurement(part).unwrap_or(0.0));
    match value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        Some(inner) => ThemeValue::List(inner.split(',').take(4).map(points).collect()),
        None => points(value),
    }
}

/// Lines a block's estimate may reach and still be kept whole without measuring it on paper
/// of a common size.
const FITS_LINES: usize = 20;

/// The most lines `emit::estimated_lines` may give a block for it to fit on a page for
/// certain: half the lines of the page's body at the larger of the prose and code line heights,
/// fewer where a line holds fewer than the 60 characters the estimate counts to one, and never
/// more than `FITS_LINES`. A block estimated at more is measured.
fn fits_lines(theme: &LoadedTheme, page: &PageGeometry) -> usize {
    const CHARACTERS_PER_LINE: f64 = 60.0;
    // Wider than most glyphs, so as to count fewer characters to a line than fit.
    const GLYPH_WIDTH_EM: f64 = 0.6;
    let positive = |key: &str| {
        theme
            .keys
            .get(key)
            .and_then(ThemeValue::as_number)
            .filter(|value| *value > 0.0)
    };
    let base_size = positive("base_font_size").unwrap_or(12.0);
    let base_height = positive("base_line_height").unwrap_or(1.0);
    let code_size = positive("code_font_size").unwrap_or(base_size);
    let code_height = positive("code_line_height").unwrap_or(base_height);
    let line = (base_size * base_height).max(code_size * code_height);
    let [top, right, bottom, left] = page.margin;
    let per_line = (page.width - left - right) / (base_size.max(code_size) * GLYPH_WIDTH_EM);
    let lines =
        (page.height - top - bottom) / line * (per_line / CHARACTERS_PER_LINE).min(1.0) / 2.0;
    if lines.is_finite() && lines > 0.0 {
        (lines as usize).min(FITS_LINES)
    } else {
        0
    }
}

/// A CSS-style shorthand of one to four numbers as top, right, bottom, left.
pub(crate) fn sides(value: Option<&ThemeValue>, default: f64) -> [f64; 4] {
    let number = |value: &ThemeValue| value.as_number().unwrap_or(default);
    match value {
        Some(ThemeValue::Number(value)) => [*value; 4],
        Some(ThemeValue::List(items)) => match items.as_slice() {
            [all] => [number(all); 4],
            [vertical, horizontal] => {
                let (v, h) = (number(vertical), number(horizontal));
                [v, h, v, h]
            }
            [top, horizontal, bottom] => {
                let h = number(horizontal);
                [number(top), h, number(bottom), h]
            }
            [top, right, bottom, left, ..] => {
                [number(top), number(right), number(bottom), number(left)]
            }
            [] => [default; 4],
        },
        _ => [default; 4],
    }
}

/// A `lang` attribute (`en`, `en-GB`, `pt_BR`) as Typst's language and region; a value
/// Typst would reject falls back to English.
fn language(lang: Option<&str>) -> (String, Option<String>) {
    let lang = lang.unwrap_or("en").trim();
    let (language, region) = lang
        .split_once(['-', '_'])
        .map_or((lang, None), |(language, region)| (language, Some(region)));
    let language = language.to_ascii_lowercase();
    if !(2..=3).contains(&language.len()) || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return ("en".to_owned(), None);
    }
    let region = region
        .map(str::to_ascii_uppercase)
        .filter(|region| region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic()));
    (language, region)
}

/// The deepest section level a running title names: `header_sectlevels` or
/// `footer_sectlevels`, 2 by default.
fn section_levels(theme: &LoadedTheme) -> u8 {
    ["header_sectlevels", "footer_sectlevels"]
        .iter()
        .find_map(|key| theme.keys.get(key).and_then(ThemeValue::as_number))
        .map_or(2, |levels| levels.clamp(1.0, 5.0) as u8)
}

/// Where running content starts, from `running_content_start_at`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunningAt {
    /// The title page.
    Title,
    /// The first page of the table of contents.
    Toc,
    /// The page after the table of contents.
    AfterToc,
    /// A page of the body, counted from 1.
    Body,
}

impl RunningAt {
    /// The name the template reads.
    fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Toc => "toc",
            Self::AfterToc => "after-toc",
            Self::Body => "body",
        }
    }
}

/// Where running content starts: `running_content_start_at` as asciidoctor-pdf reads it, a
/// page of the body by number, or by name the title page (`title`), the contents (`toc`) or the
/// page after them (`after-toc`), each the body's first page where the document has none. The
/// names only have a meaning where the title is a page of its own; otherwise they mean the
/// body.
fn running_start(theme: &LoadedTheme, facts: &Facts<'_>) -> (RunningAt, u32) {
    match theme.keys.get("running_content_start_at") {
        Some(ThemeValue::Number(page)) => (RunningAt::Body, page.max(1.0) as u32),
        Some(ThemeValue::Text(name)) if facts.title_as_page() => (
            match name.trim() {
                "title" => RunningAt::Title,
                "toc" => RunningAt::Toc,
                "after-toc" => RunningAt::AfterToc,
                _ => RunningAt::Body,
            },
            1,
        ),
        _ => (RunningAt::Body, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::measurement;

    #[test]
    fn measurements_are_an_unsigned_number_and_a_theme_unit() {
        assert_eq!(measurement("1in"), Some(72.0));
        assert_eq!(measurement("8.5in"), Some(612.0));
        assert_eq!(measurement(".5in"), Some(36.0));
        assert_eq!(measurement("2pc"), Some(24.0));
        assert_eq!(measurement("4px"), Some(3.0));
        assert_eq!(measurement("10pt"), Some(10.0));
        assert!((measurement("25.4mm").unwrap() - 72.0).abs() < 1e-9);
        assert!((measurement("2.54cm").unwrap() - 72.0).abs() < 1e-9);
        // No unit is points.
        assert_eq!(measurement("10"), Some(10.0));
        assert_eq!(measurement("0"), Some(0.0));
        // Space around the whole is trimmed; space before the unit is not.
        assert_eq!(measurement(" 1in "), Some(72.0));
        assert_eq!(measurement("1 in"), None);
        // An unknown unit, a sign, no number or two points.
        assert_eq!(measurement("10em"), None);
        assert_eq!(measurement("10%"), None);
        assert_eq!(measurement("-1in"), None);
        assert_eq!(measurement("+1in"), None);
        assert_eq!(measurement("in"), None);
        assert_eq!(measurement(""), None);
        assert_eq!(measurement("1.2.3in"), None);
        assert_eq!(measurement("1e2"), None);
    }
}
