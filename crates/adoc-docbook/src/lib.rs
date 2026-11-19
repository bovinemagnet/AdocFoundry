//! DocBook export: one page as a DocBook `article`, or the pages of an Antora book as a
//! `book`, written as semantic DocBook 5.2 XML in memory, entirely in Rust, with no converter,
//! validator or other process (`docs/prd/PRD-AdocFoundry_DocBook_Export.md`).
//!
//! [`write_docbook`] reads the view trees the engine built (the only input: AsciiDoc is not
//! parsed again and nothing passes through HTML) and writes one XML document in the DocBook
//! namespace. Every document string is escaped as it is written (`xml.rs`), and the document is
//! checked as a whole ([`check`]) before it is handed out.
//!
//! The crate writes no file: the caller puts the document in place. It reads no clock and no
//! environment, so the same input always gives the same bytes; it starts no process and opens
//! no connection (`cargo xtask layers` holds it to that).

use std::{collections::BTreeMap, fmt};

use adoc_dom::SourceSpan;
use adoc_render::ViewTree;

pub use check::check;

mod blocks;
mod check;
mod document;
mod ids;
mod info;
mod inlines;
mod media;
mod table;
#[cfg(feature = "test-support")]
pub mod testing;
mod xml;

/// The DocBook namespace.
pub const NAMESPACE: &str = "http://docbook.org/ns/docbook";

/// The XLink namespace, for links out of the document.
pub const XLINK: &str = "http://www.w3.org/1999/xlink";

/// The `version` the root carries. The 5.2.1 schema accepts any text there and its Schematron
/// asks only that the root has one; `5.2` is what DocBook 5.2.1's own documents carry (its
/// release notes are `version='5.2'`), so a processor reading the version takes the document as
/// DocBook 5.2.
pub const VERSION: &str = "5.2";

/// One page: its view tree, and how deep in the book's navigation it is.
#[derive(Clone, Debug)]
pub struct Page {
    /// A short key unique in the publication.
    pub key: String,
    pub view: ViewTree,
    /// Its navigation depth in a book, from 1; 0 for an article.
    pub depth: u8,
}

/// A book: its title, then parts, headings and pages in the navigation's order.
#[derive(Clone, Debug, Default)]
pub struct Book {
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
    /// The start page, when the navigation does not list it: the book's `preface`. Only a
    /// book's first item is read so; a later one is a chapter.
    Preface(Page),
    /// A part's title.
    Part(String),
    /// A heading of the navigation with no page of its own, at a depth (1 at the top).
    Heading {
        title: String,
        depth: u8,
    },
    Page(Page),
}

/// What is exported: one page as an article, or pages in order as a book.
#[derive(Clone, Debug)]
pub enum DocBookDocument {
    Article(Page),
    Book(Book),
}

/// How a document is written.
#[derive(Clone, Debug, Default)]
pub struct DocBookOptions {
    /// Which files the export may read: its pictures.
    pub access: adoc_core::FileAccess,
    /// Where the pictures go, and how the document refers to them.
    pub assets: AssetPolicy,
    /// The published address of each page a reference out of the publication may lead to; a
    /// reference to any other page outside it is written as its text, with a warning.
    pub page_urls: std::collections::HashMap<std::path::PathBuf, String>,
}

/// How a document refers to its pictures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetPolicy {
    /// Each picture is copied once into a folder beside the document, `folder` its name as the
    /// document refers to it (`guide_assets`): every `fileref` is `folder/i<n>.<ext>`, so the
    /// document and its folder can be moved together. The default.
    Copy { folder: String },
    /// Nothing is copied: each `fileref` is the path from `folder`, the document's own folder,
    /// to the picture where it is. Not portable: the document works only where those files
    /// stay.
    Reference { folder: std::path::PathBuf },
}

impl Default for AssetPolicy {
    fn default() -> Self {
        Self::Copy {
            folder: "assets".to_owned(),
        }
    }
}

/// A picture to write beside the document, in its assets folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    /// Its name in the assets folder: `i<n>.<ext>`, numbered in the order first used.
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

/// A problem found while writing, with the AsciiDoc it concerns when known.
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

/// A written document, checked, and what the export reports.
#[derive(Clone, Debug)]
pub struct DocBookOutput {
    /// The XML, UTF-8, starting with its declaration.
    pub xml: String,
    /// The pictures to write in the assets folder ([`AssetPolicy::Copy`]), each once.
    pub assets: Vec<Asset>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a document could not be written. Nothing is handed out in its place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocBookError {
    /// What was written failed the crate's own check ([`check`]), so it is not handed out.
    Invalid(String),
}

impl fmt::Display for DocBookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => {
                write!(formatter, "the DocBook written is not valid: {reason}")
            }
        }
    }
}

impl std::error::Error for DocBookError {}

/// Writes `document` as DocBook XML, in memory, checked before it is returned, so whoever owns
/// the output writes it somewhere temporary and moves it into place only once this returns
/// `Ok`.
pub fn write_docbook(
    document: &DocBookDocument,
    options: &DocBookOptions,
) -> Result<DocBookOutput, DocBookError> {
    let (xml, assets, diagnostics) = document::write(document, options);
    let referenced = matches!(options.assets, AssetPolicy::Reference { .. });
    let problems = check::check_document(&xml, referenced);
    if problems.is_empty() {
        Ok(DocBookOutput {
            xml,
            assets,
            diagnostics,
        })
    } else {
        Err(DocBookError::Invalid(problems.join("; ")))
    }
}
