//! The book model the reflowable ebook exporters share: the view trees of an export, an article
//! or a book of pages, mapped to one [`Book`] ([`Book::build`]) of chapters in reading order,
//! blocks and inlines, a nested table of contents, metadata and embedded pictures. Kindle
//! export (`adoc-kindle`) writes its formats from it, so the reflowable books AdocFoundry
//! writes read the same.
//!
//! Every anchor is generated, so nothing in a book depends on a file path; the same input
//! always gives the same book, with no clock and no process-wide state. The crate does no IO
//! but reading the pictures it embeds, through the caller's [`adoc_core::FileAccess`]; it
//! starts no process and opens no connection (`cargo xtask layers` holds it to that).

use std::{collections::BTreeMap, collections::HashMap, fmt, path::PathBuf};

use adoc_dom::SourceSpan;
use adoc_render::ViewTree;

pub use book::{
    inline_content, plain_text, AsideKind, Asset, AssetKind, Block, Book, Cell, Chapter, Inline,
    InlineStyle, Link, Metadata, Note, ParagraphClass, Row, Target, TocEntry,
};

mod book;
mod map;
mod media;
mod metadata;

/// One page of a book: its view tree, and how deep in the book's navigation it is.
#[derive(Clone, Debug)]
pub struct Page {
    /// A short key unique in the book.
    pub key: String,
    pub view: ViewTree,
    /// Its navigation depth in a book, from 1; 0 or 1 for an article.
    pub depth: u8,
}

/// A book: its title, then parts, headings and pages in order.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub title: String,
    pub subtitle: Option<String>,
    /// The book's attributes, as a document header would set them (`author`, `description`,
    /// `lang`, `revnumber`, …).
    pub attributes: BTreeMap<String, String>,
    pub items: Vec<BookItem>,
}

/// One item of a book, in order.
#[derive(Clone, Debug)]
pub enum BookItem {
    /// A part's title, a chapter of its own.
    Part(String),
    /// A heading of the navigation with no page of its own, at a depth (1 for a chapter).
    Heading {
        title: String,
        depth: u8,
    },
    Page(Page),
}

/// What is exported: one page as an article, or pages in order as a book.
#[derive(Clone, Debug)]
pub enum Document {
    Article(Page),
    Book(Plan),
}

/// The format a book is made for, where the formats differ in what they can show. The mapping
/// keeps what the format can show and lowers the rest, as it always did for Kindle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Format {
    /// A Kindle book: a table inside a table's cell becomes lines of text, and keys, menus and
    /// buttons are written as code and strong text.
    #[default]
    Kindle,
    /// An EPUB: tables nest, and keys, menus and buttons are [`Inline::Styled`].
    Epub,
}

impl Format {
    /// The format's name, as a diagnostic says it: "a Kindle book", "an EPUB".
    #[must_use]
    pub fn book(self) -> &'static str {
        match self {
            Self::Kindle => "a Kindle book",
            Self::Epub => "an EPUB",
        }
    }

    /// What reads the format, as a diagnostic says it: "a Kindle", "an EPUB reader".
    #[must_use]
    pub fn reader(self) -> &'static str {
        match self {
            Self::Kindle => "a Kindle",
            Self::Epub => "an EPUB reader",
        }
    }
}

/// How a book is made. The metadata given here wins over the document's attributes.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The format the book is made for.
    pub format: Format,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    /// The address of each page a reference out of the book may lead to; a reference to any
    /// other page outside the book is left as text.
    pub page_urls: HashMap<PathBuf, String>,
    /// The cover picture, in place of the document's `front-cover-image`.
    pub cover: Option<PathBuf>,
    /// Which files the export may read: its pictures.
    pub access: adoc_core::FileAccess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

/// A problem found while making a book, with the AsciiDoc it concerns when known.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub source: Option<SourceSpan>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source) = &self.source {
            write!(formatter, "{}:{}: ", source.path.display(), source.line)?;
        }
        formatter.write_str(&self.message)
    }
}

impl Book {
    /// Maps `document` to a book, with what could not be kept as it was.
    #[must_use]
    pub fn build(document: &Document, options: &Options) -> (Self, Vec<Diagnostic>) {
        map::build(document, options)
    }
}
