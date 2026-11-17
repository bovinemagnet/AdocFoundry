//! Kindle export: the view tree written as a reflowable Kindle book, `.azw3` (KF8) or `.kfx`
//! (KF10), entirely in Rust, with no converter, reader or other process
//! (`docs/prd/PRD-AdocFoundry_Kindle_Export.md`).
//!
//! [`write_kindle`] maps a [`KindleDocument`], an article or a book of pages, to one
//! [`KindleBook`] ([`KindleBook::build`]), then writes that book in the format asked for. Both
//! formats are written from the same book, which is what makes them agree; neither is ever
//! written in place of the other. The writers are the crate's own (the K1 spike,
//! `docs/qa/spike-kindle.md`): the same input always gives the same bytes, with no
//! timestamps and no process-wide state.
//!
//! The crate does no IO but reading the pictures it embeds, through the caller's
//! [`adoc_core::FileAccess`]; it starts no process and opens no connection
//! (`cargo xtask layers` holds it to that).

use std::{collections::BTreeMap, collections::HashMap, fmt, path::PathBuf};

use adoc_dom::SourceSpan;
use adoc_render::ViewTree;

pub use book::{
    plain_text, Asset, AssetKind, Block, Cell, Chapter, Inline, KindleBook, Link, Metadata, Note,
    ParagraphClass, Row, Target, TocEntry,
};

mod azw3;
mod book;
mod map;
mod metadata;
#[cfg(feature = "test-support")]
pub mod testing;

/// The formats a Kindle book is written in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KindleFormat {
    /// KF8 in a PalmDB/MOBI file.
    Azw3,
    /// KF10.
    Kfx,
}

impl KindleFormat {
    /// The file name extension the format is written with.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Azw3 => "azw3",
            Self::Kfx => "kfx",
        }
    }

    /// The format a file name extension names, in any letter case.
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "azw3" => Some(Self::Azw3),
            "kfx" => Some(Self::Kfx),
            _ => None,
        }
    }

    /// Its name as settings and command lines write it: `azw3` or `kfx`.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.extension()
    }
}

impl fmt::Display for KindleFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Azw3 => "AZW3 (KF8)",
            Self::Kfx => "KFX (KF10)",
        })
    }
}

/// One page of a book: its view tree, and how deep in the book's navigation it is.
#[derive(Clone, Debug)]
pub struct KindlePage {
    /// A short key unique in the book.
    pub key: String,
    pub view: ViewTree,
    /// Its navigation depth in a book, from 1; 0 or 1 for an article.
    pub depth: u8,
}

/// A book: its title, then parts, headings and pages in order.
#[derive(Clone, Debug, Default)]
pub struct KindlePlan {
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
    Page(KindlePage),
}

/// What is exported: one page as an article, or pages in order as a book.
#[derive(Clone, Debug)]
pub enum KindleDocument {
    Article(KindlePage),
    Book(KindlePlan),
}

/// How a book is exported. The metadata given here wins over the document's attributes.
#[derive(Clone, Debug, Default)]
pub struct KindleOptions {
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    /// The address of each page a reference out of the book may lead to; a reference to any
    /// other page outside the book is left as text.
    pub page_urls: HashMap<PathBuf, String>,
    /// Which files the export may read: its pictures.
    pub access: adoc_core::FileAccess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KindleSeverity {
    Error,
    Warning,
}

/// A problem found while exporting, with the AsciiDoc it concerns when known.
#[derive(Clone, Debug, PartialEq)]
pub struct KindleDiagnostic {
    pub severity: KindleSeverity,
    pub message: String,
    pub source: Option<SourceSpan>,
}

impl fmt::Display for KindleDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source) = &self.source {
            write!(formatter, "{}:{}: ", source.path.display(), source.line)?;
        }
        formatter.write_str(&self.message)
    }
}

/// A written book: its bytes, checked, and what the export reports.
#[derive(Clone, Debug)]
pub struct KindleOutput {
    pub bytes: Vec<u8>,
    pub diagnostics: Vec<KindleDiagnostic>,
}

/// Why a book could not be written. No other format is ever written in its place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KindleError {
    /// The format cannot be written, or not this book in it.
    Unsupported {
        format: KindleFormat,
        reason: String,
    },
    /// The bytes written failed the format's own checks, so they are not handed out.
    Invalid {
        format: KindleFormat,
        reason: String,
    },
    /// The book is past what the format, or the export's limits, hold.
    TooLarge {
        format: KindleFormat,
        reason: String,
    },
}

impl fmt::Display for KindleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { format, reason } => {
                write!(formatter, "{format} cannot be written: {reason}")
            }
            Self::Invalid { format, reason } => {
                write!(formatter, "the {format} written is not valid: {reason}")
            }
            Self::TooLarge { format, reason } => {
                write!(formatter, "too large for {format}: {reason}")
            }
        }
    }
}

impl std::error::Error for KindleError {}

impl KindleBook {
    /// Maps `document` to a book, with what could not be kept as it was.
    #[must_use]
    pub fn build(
        document: &KindleDocument,
        options: &KindleOptions,
    ) -> (Self, Vec<KindleDiagnostic>) {
        map::build(document, options)
    }
}

/// Writes `document` as a Kindle book in `format`, in memory. The bytes are checked against
/// the format before they are returned, so whoever owns the file writes them somewhere
/// temporary and moves them into place only once this returns `Ok`.
pub fn write_kindle(
    document: &KindleDocument,
    format: KindleFormat,
    options: &KindleOptions,
) -> Result<KindleOutput, KindleError> {
    let (book, diagnostics) = KindleBook::build(document, options);
    let bytes = write_book(&book, format)?;
    Ok(KindleOutput { bytes, diagnostics })
}

/// Writes `book` in `format`, the bytes checked against the format.
pub fn write_book(book: &KindleBook, format: KindleFormat) -> Result<Vec<u8>, KindleError> {
    match format {
        KindleFormat::Azw3 => azw3::write(book),
        KindleFormat::Kfx => Err(KindleError::Unsupported {
            format,
            reason: "no writer for this format yet".to_owned(),
        }),
    }
}
