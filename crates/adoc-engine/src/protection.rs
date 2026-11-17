//! Read-only exports, READONLY 2/5 (`docs/prd/PRD-AdocFoundry_READONLY.md` §5.2): one
//! decision per export, from the policy each planned page's own header sets (read from the text
//! the export parses), checked against what the format can do before any writer starts. Every
//! refusal fails the export before anything is written, so existing output stays as it was.

use std::path::PathBuf;

use crate::{read_only::ReadOnlySource, ExportReadOnly, PageReadOnly, ReadOnlyHeaderError};

/// A format an export writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Pdf,
    /// A PDF claiming a PDF/A standard, which forbids encryption.
    PdfA,
    Docx,
    Html,
    /// A Kindle book, AZW3 or KFX, which carries no editing restriction.
    Kindle,
}

impl ExportFormat {
    fn name(self) -> &'static str {
        match self {
            Self::Pdf | Self::PdfA => "PDF",
            Self::Docx => "Word",
            Self::Html => "HTML",
            Self::Kindle => "Kindle",
        }
    }

    /// Whether the format can carry a restriction over the whole document: Word's enforced
    /// document protection (READONLY 3/5). The PDF's arrives with READONLY 4/5 once its
    /// feasibility gate passes; HTML has none.
    fn protects_whole_document(self) -> bool {
        match self {
            Self::Docx => true,
            Self::Pdf | Self::PdfA | Self::Html | Self::Kindle => false,
        }
    }
}

/// What the pages of an export ask for, before what the format can do is considered.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ProtectionRequest {
    /// No page asks: the export is as it always was.
    #[default]
    Off,
    /// Every page asks (or the article's one page does).
    WholeDocument { requested_by: Vec<ReadOnlySource> },
    /// Some pages ask and some do not.
    Mixed {
        requested_by: Vec<ReadOnlySource>,
        editable: Vec<PathBuf>,
    },
}

/// What an export carries once it may be written: nothing, or a restriction over the whole
/// document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ExportProtection {
    #[default]
    Off,
    WholeDocument {
        requested_by: Vec<ReadOnlySource>,
    },
}

/// Why an export whose pages ask for protection cannot be written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtectionRefusal {
    /// A page's header gives no policy.
    InvalidHeader(ReadOnlyHeaderError),
    /// The format has no editing restriction to give (yet).
    UnsupportedFormat {
        format: ExportFormat,
        requested_by: Vec<ReadOnlySource>,
    },
    /// The pages ask for different things and the format restricts only whole files.
    MixedDocument {
        format: ExportFormat,
        requested_by: Vec<ReadOnlySource>,
        editable: Vec<PathBuf>,
    },
    /// A standard the export claims forbids the encryption a restriction needs.
    IncompatibleStandard {
        standard: String,
        requested_by: Vec<ReadOnlySource>,
    },
}

fn sources(sources: &[ReadOnlySource]) -> String {
    sources
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

impl std::fmt::Display for ProtectionRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHeader(error) => error.fmt(formatter),
            Self::UnsupportedFormat {
                format: ExportFormat::Html,
                requested_by,
            } => write!(
                formatter,
                "the page asks for a read-only export (`:export-read-only:` at {}), and HTML \
                 has no editing restriction; export it to Word instead",
                sources(requested_by)
            ),
            Self::UnsupportedFormat {
                format,
                requested_by,
            } => write!(
                formatter,
                "the pages ask for a read-only export (`:export-read-only:` at {}), which a {} \
                 export cannot carry yet; nothing was written",
                sources(requested_by),
                format.name()
            ),
            Self::MixedDocument {
                format,
                requested_by,
                editable,
            } => write!(
                formatter,
                "some pages ask for a read-only export (`:export-read-only:` at {}) and others \
                 do not ({}); a {} export restricts the whole document, so export them \
                 separately",
                sources(requested_by),
                paths(editable),
                format.name()
            ),
            Self::IncompatibleStandard {
                standard,
                requested_by,
            } => write!(
                formatter,
                "{standard} forbids the encryption a read-only PDF needs, and the pages ask for \
                 one (`:export-read-only:` at {}); drop the standard or the attribute",
                sources(requested_by)
            ),
        }
    }
}

impl std::error::Error for ProtectionRefusal {}

/// What the pages ask for, from each page's policy in the export's order; the first page whose
/// header gives no policy refuses the export.
pub(crate) fn request<'a>(
    pages: impl IntoIterator<Item = (PathBuf, Result<PageReadOnly, ReadOnlyHeaderError>)> + 'a,
) -> Result<ProtectionRequest, ProtectionRefusal> {
    let mut requested_by = Vec::new();
    let mut editable = Vec::new();
    for (path, page) in pages {
        let page = page.map_err(ProtectionRefusal::InvalidHeader)?;
        match (page.policy, page.source) {
            (ExportReadOnly::Enabled, Some(source)) => requested_by.push(source),
            _ => editable.push(path),
        }
    }
    Ok(match (requested_by.is_empty(), editable.is_empty()) {
        (true, _) => ProtectionRequest::Off,
        (false, true) => ProtectionRequest::WholeDocument { requested_by },
        (false, false) => ProtectionRequest::Mixed {
            requested_by,
            editable,
        },
    })
}

/// What an export in `format` carries for `request`, or why it cannot be written. `standard`
/// is a PDF/A standard the export claims, if any.
pub(crate) fn resolve(
    request: ProtectionRequest,
    format: ExportFormat,
    standard: Option<String>,
) -> Result<ExportProtection, ProtectionRefusal> {
    let (requested_by, editable) = match request {
        ProtectionRequest::Off => return Ok(ExportProtection::Off),
        ProtectionRequest::WholeDocument { requested_by } => (requested_by, Vec::new()),
        ProtectionRequest::Mixed {
            requested_by,
            editable,
        } => (requested_by, editable),
    };
    if let Some(standard) = standard {
        return Err(ProtectionRefusal::IncompatibleStandard {
            standard,
            requested_by,
        });
    }
    if !editable.is_empty() {
        // No format leaves some pages of one document editable yet; Word's ranges arrive,
        // once verified, with READONLY 3/5.
        return Err(ProtectionRefusal::MixedDocument {
            format,
            requested_by,
            editable,
        });
    }
    if !format.protects_whole_document() {
        return Err(ProtectionRefusal::UnsupportedFormat {
            format,
            requested_by,
        });
    }
    Ok(ExportProtection::WholeDocument { requested_by })
}

/// The name of the first PDF/A standard in `standards`, which forbid encryption; PDF/UA does
/// not, and is decided with the PDF's protection (READONLY 4/5).
#[cfg(feature = "typst")]
pub(crate) fn pdfa(standards: &[crate::PdfStandard]) -> Option<String> {
    standards.iter().find_map(|standard| match standard {
        crate::PdfStandard::A1b => Some("PDF/A-1b".to_owned()),
        crate::PdfStandard::A1a => Some("PDF/A-1a".to_owned()),
        crate::PdfStandard::A2a => Some("PDF/A-2a".to_owned()),
        crate::PdfStandard::Ua1 => None,
    })
}
