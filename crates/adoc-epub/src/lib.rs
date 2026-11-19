//! EPUB export: an article or a book of pages written as a reflowable EPUB 3.3 publication,
//! entirely in Rust, with no converter, validator or other process
//! (`docs/prd/PRD-AdocFoundry_EPUB_Export.md`).
//!
//! [`write_epub`] maps a [`Document`] to one [`Book`] through `adoc-ebook`, the book model
//! Kindle export writes too, then writes that book as an OCF container in memory: `mimetype`
//! first and stored, `META-INF/container.xml`, the package document `EPUB/package.opf`, the
//! navigation document and a content document per chapter, semantic XHTML (`xhtml.rs`). Every
//! XML file is checked to be well-formed before the bytes are returned.
//!
//! The crate reads no clock and no environment: the modification time is given ([`Modified`]),
//! so the same input always gives the same bytes. It does no IO but reading the pictures it
//! embeds, through the caller's `FileAccess`; it starts no process and opens no connection
//! (`cargo xtask layers` holds it to that).

use std::fmt;

pub use adoc_ebook::{
    Book, BookItem, Diagnostic, Document, Format, Options as BookOptions, Page, Plan, Severity,
};

mod package;
#[cfg(feature = "test-support")]
pub mod testing;
mod xhtml;
mod xml;
mod zip;

/// When a book was last modified, to the second, in UTC: its `dcterms:modified` and the date of
/// every file in the archive. Only times a ZIP archive can date are held: 1980 to 2107.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Modified(i64);

impl Modified {
    /// 1980-01-01T00:00:00Z, the earliest time a ZIP archive dates.
    const EARLIEST: i64 = 315_532_800;
    /// 2107-12-31T23:59:59Z, the latest.
    const LATEST: i64 = 4_354_819_199;

    /// The time `seconds` after the Unix epoch, if a book can be dated with it.
    #[must_use]
    pub fn from_unix_seconds(seconds: i64) -> Option<Self> {
        (Self::EARLIEST..=Self::LATEST)
            .contains(&seconds)
            .then_some(Self(seconds))
    }

    #[must_use]
    pub fn unix_seconds(self) -> i64 {
        self.0
    }

    /// Year, month, day, hour, minute and second.
    fn civil(self) -> (i64, u8, u8, u8, u8, u8) {
        let (year, month, day) = adoc_core::civil_from_days(self.0.div_euclid(86_400));
        let second = self.0.rem_euclid(86_400);
        let hms = |value: i64| u8::try_from(value).unwrap_or(0);
        (
            year,
            month,
            day,
            hms(second / 3600),
            hms(second / 60 % 60),
            hms(second % 60),
        )
    }

    /// As `dcterms:modified` writes it: `CCYY-MM-DDThh:mm:ssZ`.
    #[must_use]
    pub fn to_w3c(self) -> String {
        let (year, month, day, hour, minute, second) = self.civil();
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    }

    /// The MS-DOS time and date of a ZIP entry, to the even second below.
    fn dos(self) -> (u16, u16) {
        let (year, month, day, hour, minute, second) = self.civil();
        let year = u16::try_from(year - 1980).unwrap_or(0);
        let time = (u16::from(hour) << 11) | (u16::from(minute) << 5) | u16::from(second / 2);
        let date = (year << 9) | (u16::from(month) << 5) | u16::from(day);
        (time, date)
    }
}

impl fmt::Display for Modified {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_w3c())
    }
}

/// How a book is exported. There is no default: the modification time is always given.
#[derive(Clone, Debug)]
pub struct EpubOptions {
    /// What the book is made from and how: metadata, the cover, links out of the book and the
    /// files the export may read.
    pub book: BookOptions,
    pub modified: Modified,
    /// A stylesheet of the user's, linked after the default one so it overrides it. It may
    /// change only how the book looks: one that imports or names anything is refused.
    pub stylesheet: Option<String>,
}

impl EpubOptions {
    #[must_use]
    pub fn new(modified: Modified) -> Self {
        Self {
            book: BookOptions::default(),
            modified,
            stylesheet: None,
        }
    }
}

/// A written book: its bytes, checked, and what the export reports.
#[derive(Clone, Debug)]
pub struct EpubOutput {
    pub bytes: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    /// How many content documents the book holds, in its reading order.
    pub chapters: usize,
}

/// Why a book could not be written. Nothing is written in its place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EpubError {
    /// What was to be written failed the crate's own checks, so it is not handed out.
    Invalid(String),
    /// The book is past what an EPUB archive, or the export's limits, hold.
    TooLarge(String),
}

impl fmt::Display for EpubError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(formatter, "the EPUB written is not valid: {reason}"),
            Self::TooLarge(reason) => write!(formatter, "too large for an EPUB: {reason}"),
        }
    }
}

impl std::error::Error for EpubError {}

/// Writes `document` as an EPUB, in memory. Every XML file is checked before the bytes are
/// returned, so whoever owns the output writes them somewhere temporary and moves them into
/// place only once this returns `Ok`.
pub fn write_epub(document: &Document, options: &EpubOptions) -> Result<EpubOutput, EpubError> {
    if let Some(stylesheet) = &options.stylesheet {
        check_stylesheet(stylesheet).map_err(EpubError::Invalid)?;
    }
    // Whatever format the options name, the book is made for EPUB.
    let book_options = BookOptions {
        format: Format::Epub,
        ..options.book.clone()
    };
    let (book, diagnostics) = Book::build(document, &book_options);
    let bytes = package::write(&book, options.modified, options.stylesheet.as_deref())?;
    Ok(EpubOutput {
        bytes,
        diagnostics,
        chapters: book.chapters.len(),
    })
}

/// Writes `book` as an EPUB dated `modified`, with the default stylesheet, every XML file
/// checked.
pub fn write_book(book: &Book, modified: Modified) -> Result<Vec<u8>, EpubError> {
    package::write(book, modified, None)
}

/// The most a user stylesheet may hold.
const STYLESHEET_LIMIT: usize = 256 * 1024;

/// Refuses a user stylesheet that could make the book depend on anything outside it or run
/// anything (PRD §5.6): an `@import`, a `url(` (no file of the book is the user's to name), a
/// font face, a binding, an expression or a script address, in any letter case or escaped.
/// A conservative check, not a CSS parser: what it cannot be sure of, it refuses.
pub fn check_stylesheet(stylesheet: &str) -> Result<(), String> {
    if stylesheet.len() > STYLESHEET_LIMIT {
        return Err(format!(
            "the stylesheet is larger than {} KB",
            STYLESHEET_LIMIT / 1024
        ));
    }
    if stylesheet.contains('\\') {
        return Err(
            "the stylesheet holds a backslash escape, which could hide what it names".to_owned(),
        );
    }
    let lower = stylesheet.to_ascii_lowercase();
    for (needle, what) in [
        ("@import", "imports another stylesheet"),
        ("url(", "names a file or address"),
        ("@font-face", "brings in a font"),
        ("@namespace", "declares a namespace"),
        ("expression(", "holds a script expression"),
        ("javascript:", "holds a script address"),
        ("-moz-binding", "binds a script"),
        ("behavior", "binds a script"),
        ("</", "holds markup"),
    ] {
        if lower.contains(needle) {
            return Err(format!(
                "the stylesheet {what} ({needle}), so it is refused"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modification_time_is_dated_both_ways() {
        let time = Modified::from_unix_seconds(1_763_510_400 + 3_661).unwrap();
        assert_eq!(time.to_w3c(), "2025-11-19T01:01:01Z");
        let (dos_time, dos_date) = time.dos();
        assert_eq!(dos_time, (1 << 11) | (1 << 5));
        assert_eq!(dos_date, (45 << 9) | (11 << 5) | 19);
        assert_eq!(
            Modified::from_unix_seconds(Modified::EARLIEST)
                .unwrap()
                .to_w3c(),
            "1980-01-01T00:00:00Z"
        );
        assert_eq!(
            Modified::from_unix_seconds(Modified::LATEST)
                .unwrap()
                .to_w3c(),
            "2107-12-31T23:59:59Z"
        );
        assert!(Modified::from_unix_seconds(Modified::EARLIEST - 1).is_none());
        assert!(Modified::from_unix_seconds(Modified::LATEST + 1).is_none());
    }
}
