//! DOCX export: the view tree written as an editable Microsoft Word document
//! (WordprocessingML in an OPC package), with no Word runtime and no PDF or Typst in between.
//!
//! [`write_docx`] turns a [`DocxDocument`], an article or a book of pages, into a `.docx`
//! package written to any [`Write`] sink. The package is written in-house rather than through
//! an OOXML library: every string goes through one escaping writer (`xml.rs`), so a document can
//! never break a part's markup, and the same input always gives the same bytes, with no
//! timestamps and no process-wide counters (PRD §9; the choice is recorded in the PRD's §14).

use std::{collections::BTreeMap, fmt, io::Write};

use adoc_dom::SourceSpan;
use adoc_render::ViewTree;

mod body;
mod package;
mod parts;
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
}

/// How a document is exported.
#[derive(Clone, Debug, Default)]
pub struct DocxOptions {
    pub paper: Paper,
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
    let mut body = body::Body::new();
    body.document(document);
    let diagnostics = std::mem::take(&mut body.diagnostics);
    let parts = [
        package::Part {
            name: "word/document.xml".to_owned(),
            content_type: parts::DOCUMENT_TYPE,
            data: parts::document(body.into_xml(), options.paper),
            relationships: vec![package::Relationship {
                id: "rId1".to_owned(),
                kind: parts::STYLES,
                target: "styles.xml".to_owned(),
            }],
        },
        package::Part {
            name: "word/styles.xml".to_owned(),
            content_type: parts::STYLES_TYPE,
            data: parts::styles(),
            relationships: Vec::new(),
        },
        package::Part {
            name: "docProps/core.xml".to_owned(),
            content_type: parts::CORE_TYPE,
            data: parts::core(&facts(document)),
            relationships: Vec::new(),
        },
    ];
    package::write(
        &[
            package::Relationship {
                id: "rId1".to_owned(),
                kind: parts::OFFICE_DOCUMENT,
                target: "word/document.xml".to_owned(),
            },
            package::Relationship {
                id: "rId2".to_owned(),
                kind: parts::CORE_PROPERTIES,
                target: "docProps/core.xml".to_owned(),
            },
        ],
        &parts,
        sink,
    )?;
    Ok(DocxReport { diagnostics })
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
