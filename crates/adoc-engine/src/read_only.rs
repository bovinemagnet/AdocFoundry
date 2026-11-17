//! Read-only exports (`docs/prd/PRD-AdocFoundry_READONLY.md`): what a page's own header asks
//! of its exports, read from the text the export renders — the open buffer, unsaved edits
//! included, else the file — and located for diagnostics.

#[cfg(feature = "export")]
use std::path::Path;
use std::path::PathBuf;

use adoc_core::SourceRange;
pub use adoc_parser::ExportReadOnly;

/// Where a page declares `:export-read-only:`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOnlySource {
    pub path: PathBuf,
    /// One-based.
    pub line: u32,
    /// The declaration's line, in bytes of the page's text.
    pub range: SourceRange,
}

impl std::fmt::Display for ReadOnlySource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.path.display(), self.line)
    }
}

/// A page's export policy and the declaration that set it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageReadOnly {
    pub policy: ExportReadOnly,
    pub source: Option<ReadOnlySource>,
}

/// A page whose header gives no policy, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOnlyHeaderError {
    pub error: adoc_parser::HeaderReadOnlyError,
    pub source: ReadOnlySource,
}

impl std::fmt::Display for ReadOnlyHeaderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.source, self.error)
    }
}

impl std::error::Error for ReadOnlyHeaderError {}

#[cfg(feature = "export")]
fn located(path: &Path, text: &str, range: SourceRange) -> ReadOnlySource {
    let (line, _) = adoc_core::LineIndex::new(text).line_column(range.start);
    ReadOnlySource {
        path: path.to_path_buf(),
        line: u32::try_from(line + 1).unwrap_or(u32::MAX),
        range,
    }
}

/// The policy the page at `path` with `text` sets in its own main header.
#[cfg(feature = "export")]
pub(crate) fn page_read_only(path: &Path, text: &str) -> Result<PageReadOnly, ReadOnlyHeaderError> {
    match adoc_parser::export_read_only(text) {
        Ok(header) => Ok(PageReadOnly {
            policy: header.policy,
            source: header.range.map(|range| located(path, text, range)),
        }),
        Err(error) => Err(ReadOnlyHeaderError {
            source: located(path, text, error.range()),
            error,
        }),
    }
}

#[cfg(feature = "export")]
impl crate::Engine {
    /// What the page at `path` asks of its exports, from its own main header, read from the
    /// text an export of it renders: its open buffer, unsaved edits included, else its file.
    /// `None` when the page cannot be read.
    #[must_use]
    pub fn export_read_only(
        &self,
        path: &Path,
    ) -> Option<Result<PageReadOnly, ReadOnlyHeaderError>> {
        let input = self.parse_input_for_path(path)?;
        Some(page_read_only(&input.path, &input.text))
    }
}
