//! The full AsciiDoc document model, behind [`DocumentParser`].
//!
//! `adoc-parser` gives an instant, partial view of a page for the outline and diagnostics; this
//! crate gives the whole document — every block and inline, with includes expanded — for the
//! preview, export and later WYSIWYG editing. Parser crates are only ever named inside this crate,
//! so the parser behind the trait can change without touching anything above it.

mod acdc;
mod model;
mod replacements;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub use acdc::AcdcParser;
pub use model::*;

/// Supplies included files: open buffers, files on disk, and Antora resources.
pub trait IncludeResolver: Send + Sync {
    /// The file and text for an include, given the path the parser requested: the including
    /// file's directory joined with the target as written. A target such as `partial$note.adoc`
    /// therefore arrives as `<dir>/partial$note.adoc` and is the resolver's to interpret.
    fn read(&self, requested: &Path) -> Option<(PathBuf, String)>;
}

/// Resolves nothing: includes are left unresolved.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoIncludes;

impl IncludeResolver for NoIncludes {
    fn read(&self, _: &Path) -> Option<(PathBuf, String)> {
        None
    }
}

/// Reads included files from disk, relative to the requesting file.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileIncludes;

impl IncludeResolver for FileIncludes {
    fn read(&self, requested: &Path) -> Option<(PathBuf, String)> {
        let text = std::fs::read_to_string(requested).ok()?;
        Some((requested.to_path_buf(), text))
    }
}

pub struct ParseInput<'a> {
    pub path: &'a Path,
    pub text: &'a str,
    /// Attributes the caller sets, such as Antora's page and component attributes.
    pub attributes: Vec<(String, String)>,
    pub includes: Arc<dyn IncludeResolver>,
}

impl<'a> ParseInput<'a> {
    /// A page with no caller attributes whose includes are read from disk.
    #[must_use]
    pub fn new(path: &'a Path, text: &'a str) -> Self {
        Self {
            path,
            text,
            attributes: Vec::new(),
            includes: Arc::new(FileIncludes),
        }
    }
}

pub trait DocumentParser: Send + Sync {
    fn name(&self) -> &'static str;
    /// Parses a whole document. Never panics: half-typed input is normal, and anything the
    /// parser cannot place is kept as [`BlockKind::Unsupported`] rather than dropped.
    fn parse(&self, input: &ParseInput<'_>) -> Dom;
}
