//! The whole Typst document: the theme's keys, the document's facts, the template, the front
//! matter and the body, for an article or a book.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    path::PathBuf,
};

use adoc_render::ViewInline;

use crate::{
    emit::{EmitOptions, Emitted, Emitter, PageView, SourceMap},
    running,
    theme::{LoadedTheme, ThemeValue},
    typst::{self, string, text},
};

/// The template the generated body is typeset with.
const TEMPLATE: &str = include_str!("adoc.typ");
/// STEM's helpers and `adoc-stem`, ahead of the template and in a formula's own document.
pub(crate) const MATH: &str = include_str!("math.typ");

/// Where running content and page numbering start: the first page of the body, numbered 1.
const RUNNING_START: &str = "#metadata(none) <adoc-running-start>#counter(page).update(1)\n";

/// A PDF standard the output must conform to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PdfStandard {
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
    /// The site cross references out of a book link to.
    pub site_url: Option<String>,
    /// The address of each page outside the document a reference may lead to, for a book;
    /// references to any other page are left as text.
    pub page_urls: HashMap<PathBuf, String>,
    /// The date written into the PDF (year, month, day), for reproducible output; none by
    /// default.
    pub date: Option<(i32, u8, u8)>,
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
    fn pages(&self) -> Vec<PageView> {
        match self {
            Self::Article(page) => vec![page.clone()],
            Self::Book(book) => book
                .items
                .iter()
                .filter_map(|item| match item {
                    BookItem::Page(page) => Some(page.clone()),
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
    /// Images that could not be found, with where they were written.
    pub missing: Vec<(String, adoc_render::SourceSpan)>,
    /// Formulas that could not be typeset, why, and where they were written.
    pub unconverted: Vec<(String, adoc_render::SourceSpan)>,
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
    let emit_options = EmitOptions {
        highlight: facts.has("source-highlighter"),
        site_url: options.site_url.clone(),
        page_urls: options.page_urls.clone(),
        toc_levels: facts.toc_levels(),
    };
    let mut emitter = Emitter::new(&pages, &emit_options);

    // Front matter.
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
            };
            let markup = title_page.markup(theme, &mut |path| emitter.file(path));
            emitter.push(&markup);
        }
        if facts.toc_at_top() {
            let min_level = if book && has_parts(document) { 1 } else { 2 };
            emitter.push(&format!(
                "#adoc-toc(min-level: {min_level})\n\n#pagebreak(weak: true)\n\n"
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
                        heading(&mut emitter, *depth, &text(title), None);
                    }
                    BookItem::Page(page) => {
                        emitter.begin_page(page);
                        let anchor = emitter.label("");
                        let title = page
                            .view
                            .title
                            .as_ref()
                            .map(|title| emitter.inlines(title))
                            .unwrap_or_default();
                        heading(&mut emitter, page.depth.max(1), &title, Some(&anchor));
                        emitter.page_body(&page.view);
                        emitter.footnotes(&page.view);
                    }
                }
            }
        }
    }

    let mut attributes = attributes.clone();
    if let Some(title) = &title {
        attributes
            .entry("doctitle".to_owned())
            .or_insert_with(|| title.clone());
    }
    let running = running::running_dictionary(theme, &attributes, &mut |path| emitter.file(path));
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
        &crate::fonts::line_gaps(theme, &options.fonts_dirs),
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
    }
}

/// A book heading at `depth`: a chapter (depth 1) starts a new page.
fn heading(emitter: &mut Emitter<'_>, depth: u8, title: &str, label: Option<&str>) {
    if depth <= 1 {
        emitter.push("#pagebreak(weak: true)\n\n");
    }
    let label = label.map(|label| format!("#{label}")).unwrap_or_default();
    emitter.push(&format!(
        "#heading(level: {}, outlined: true, bookmarked: true)[{title}]{label}\n\n",
        u32::from(depth) + 1
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
    let page = page_geometry(theme);
    let fallbacks: Vec<String> = theme.fallbacks.iter().map(|name| string(name)).collect();
    let toc_title = facts
        .attribute("toc-title")
        .unwrap_or("Table of Contents")
        .to_owned();
    let _ = writeln!(
        out,
        "#let doc = (\n  line-gaps: ({}),\n  lang: {},\n  region: {},\n  hyphenate: {},\n  fallbacks: ({}),\n  page: (width: {}, height: {}, margin: (top: {}, right: {}, bottom: {}, left: {})),\n  toc-title: {},\n  toc-levels: {},\n  running: {},\n  running-from: {},\n)",
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
        running_from(theme),
    );
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
}

/// Page sizes asciidoctor-pdf (through Prawn) knows by name, in points.
const PAPER: &[(&str, f64, f64)] = &[
    ("A0", 2383.94, 3370.39),
    ("A1", 1683.78, 2383.94),
    ("A2", 1190.55, 1683.78),
    ("A3", 841.89, 1190.55),
    ("A4", 595.28, 841.89),
    ("A5", 419.53, 595.28),
    ("A6", 297.64, 419.53),
    ("A7", 209.76, 297.64),
    ("A8", 147.40, 209.76),
    ("B4", 708.66, 1000.63),
    ("B5", 498.90, 708.66),
    ("EXECUTIVE", 521.86, 756.00),
    ("FOLIO", 612.00, 936.00),
    ("LEGAL", 612.00, 1008.00),
    ("LETTER", 612.00, 792.00),
    ("TABLOID", 792.00, 1224.00),
];

fn page_geometry(theme: &LoadedTheme) -> PageGeometry {
    let keys = &theme.keys;
    let (mut width, mut height) = match keys.get("page_size") {
        Some(ThemeValue::Text(name)) => PAPER
            .iter()
            .find(|(paper, _, _)| paper.eq_ignore_ascii_case(name))
            .map_or((595.28, 841.89), |(_, width, height)| (*width, *height)),
        Some(ThemeValue::List(items)) if items.len() == 2 => (
            items[0].as_number().unwrap_or(595.28),
            items[1].as_number().unwrap_or(841.89),
        ),
        _ => (595.28, 841.89),
    };
    if matches!(keys.get("page_layout"), Some(ThemeValue::Text(layout)) if layout == "landscape") {
        std::mem::swap(&mut width, &mut height);
    }
    let margin = sides(keys.get("page_margin"), 36.0);
    PageGeometry {
        width,
        height,
        margin,
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

/// The body page running content starts on: `running_content_start_at` when it is a number.
fn running_from(theme: &LoadedTheme) -> u32 {
    theme
        .keys
        .get("running_content_start_at")
        .and_then(ThemeValue::as_number)
        .map_or(1, |page| page.max(1.0) as u32)
}
