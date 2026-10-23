//! Compiling the generated Typst to PDF, with Typst's errors and warnings mapped back to the
//! AsciiDoc they came from.

use std::fmt;

use adoc_render::SourceSpan;
use typst::{
    diag::{Severity, SourceDiagnostic, Warned},
    foundations::{Datetime, Smart},
    WorldExt as _,
};
use typst_layout::PagedDocument;

use crate::{
    document::{to_typst, PdfOptions, PdfStandard, Typst},
    emit::PageView,
    fonts::font_set,
    theme::LoadedTheme,
    world::PdfWorld,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PdfSeverity {
    Error,
    Warning,
}

/// A problem found while exporting, with the AsciiDoc it concerns when known.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfDiagnostic {
    pub severity: PdfSeverity,
    pub message: String,
    pub hints: Vec<String>,
    pub source: Option<SourceSpan>,
}

impl fmt::Display for PdfDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source) = &self.source {
            write!(formatter, "{}:{}: ", source.path.display(), source.line)?;
        }
        formatter.write_str(&self.message)?;
        for hint in &self.hints {
            write!(formatter, " (hint: {hint})")?;
        }
        Ok(())
    }
}

/// A finished export.
#[derive(Clone, Debug)]
pub struct PdfOutput {
    pub bytes: Vec<u8>,
    pub pages: usize,
    /// Warnings: from the theme, the fonts, missing images and Typst.
    pub diagnostics: Vec<PdfDiagnostic>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PdfError {
    /// Typst could not lay the document out.
    Compile(Vec<PdfDiagnostic>),
    /// The PDF could not be written, for instance because a standard's requirements are not
    /// met.
    Pdf(Vec<PdfDiagnostic>),
}

impl PdfError {
    #[must_use]
    pub fn diagnostics(&self) -> &[PdfDiagnostic] {
        match self {
            Self::Compile(diagnostics) | Self::Pdf(diagnostics) => diagnostics,
        }
    }
}

impl fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self {
            Self::Compile(_) => "the document could not be typeset",
            Self::Pdf(_) => "the PDF could not be written",
        };
        formatter.write_str(what)?;
        for diagnostic in self.diagnostics() {
            write!(formatter, "\n{diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PdfError {}

/// Exports `pages` as a PDF styled by `theme`.
pub fn export(
    pages: &[PageView],
    theme: &LoadedTheme,
    options: &PdfOptions,
) -> Result<PdfOutput, PdfError> {
    let typst = to_typst(pages, theme, options);
    compile(&typst, theme, options)
}

/// Compiles generated Typst to PDF.
pub fn compile(
    typst: &Typst,
    theme: &LoadedTheme,
    options: &PdfOptions,
) -> Result<PdfOutput, PdfError> {
    let (document, diagnostics, world) = typeset(typst, theme, options)?;
    let map = |list: &[SourceDiagnostic]| -> Vec<PdfDiagnostic> {
        list.iter()
            .map(|diagnostic| diagnostic_of(&world, typst, diagnostic))
            .collect()
    };
    let standards: Vec<typst_pdf::PdfStandard> = options
        .standards
        .iter()
        .map(|standard| match standard {
            PdfStandard::A2a => typst_pdf::PdfStandard::A_2a,
            PdfStandard::Ua1 => typst_pdf::PdfStandard::Ua_1,
        })
        .collect();
    let standards = typst_pdf::PdfStandards::new(&standards).map_err(|error| {
        PdfError::Pdf(vec![PdfDiagnostic {
            severity: PdfSeverity::Error,
            message: error.message().to_string(),
            hints: error.hints().iter().map(ToString::to_string).collect(),
            source: None,
        }])
    })?;
    let today = options
        .date
        .and_then(|(year, month, day)| Datetime::from_ymd(year, month, day));
    let pdf_options = typst_pdf::PdfOptions {
        ident: Smart::Auto,
        creator: Smart::Custom(Some(format!("AdocFoundry {}", env!("CARGO_PKG_VERSION")))),
        timestamp: today.map(typst_pdf::Timestamp::new_utc),
        standards,
        ..typst_pdf::PdfOptions::default()
    };
    let bytes =
        typst_pdf::pdf(&document, &pdf_options).map_err(|errors| PdfError::Pdf(map(&errors)))?;
    Ok(PdfOutput {
        bytes,
        pages: document.pages().len(),
        diagnostics,
    })
}

/// A page as Typst laid it out: its size and its runs of text, in points from the top left.
#[derive(Clone, Debug, PartialEq)]
pub struct TypesetPage {
    pub width: f64,
    pub height: f64,
    pub runs: Vec<TextRun>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub x: f64,
    pub y: f64,
    pub text: String,
}

impl TypesetPage {
    /// The page's text, a space between runs.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for run in &self.runs {
            out.push_str(&run.text);
            out.push(' ');
        }
        out
    }
}

/// The pages as Typst laid them out, for checking what a document says and where, without
/// reading the PDF back.
pub fn typeset_pages(
    typst: &Typst,
    theme: &LoadedTheme,
    options: &PdfOptions,
) -> Result<Vec<TypesetPage>, PdfError> {
    fn collect(frame: &typst::layout::Frame, x: f64, y: f64, out: &mut Vec<TextRun>) {
        for (point, item) in frame.items() {
            let (px, py) = (x + point.x.to_pt(), y + point.y.to_pt());
            match item {
                typst::layout::FrameItem::Group(group) => collect(
                    &group.frame,
                    px + group.transform.tx.to_pt(),
                    py + group.transform.ty.to_pt(),
                    out,
                ),
                typst::layout::FrameItem::Text(text) => out.push(TextRun {
                    x: px,
                    y: py,
                    text: text.text.to_string(),
                }),
                _ => {}
            }
        }
    }
    let (document, _, _) = typeset(typst, theme, options)?;
    Ok(document
        .pages()
        .iter()
        .map(|page| {
            let mut runs = Vec::new();
            collect(&page.frame, 0.0, 0.0, &mut runs);
            TypesetPage {
                width: page.frame.width().to_pt(),
                height: page.frame.height().to_pt(),
                runs,
            }
        })
        .collect())
}

/// The text of each page as Typst laid it out, one string per page.
pub fn typeset_text(
    typst: &Typst,
    theme: &LoadedTheme,
    options: &PdfOptions,
) -> Result<Vec<String>, PdfError> {
    Ok(typeset_pages(typst, theme, options)?
        .iter()
        .map(TypesetPage::text)
        .collect())
}

/// Lays the document out, with every warning found on the way.
fn typeset(
    typst: &Typst,
    theme: &LoadedTheme,
    options: &PdfOptions,
) -> Result<(PagedDocument, Vec<PdfDiagnostic>, PdfWorld), PdfError> {
    let mut diagnostics: Vec<PdfDiagnostic> = theme
        .warnings
        .iter()
        .map(|warning| PdfDiagnostic {
            severity: PdfSeverity::Warning,
            message: match &warning.path {
                Some(path) => format!("{}: {}", path.display(), warning.message),
                None => warning.message.clone(),
            },
            hints: Vec::new(),
            source: None,
        })
        .collect();
    let (fonts, font_warnings) = font_set(theme, &options.fonts_dirs);
    diagnostics.extend(font_warnings.into_iter().map(|message| PdfDiagnostic {
        severity: PdfSeverity::Warning,
        message,
        hints: Vec::new(),
        source: None,
    }));
    diagnostics.extend(typst.missing.iter().map(|(target, span)| PdfDiagnostic {
        severity: PdfSeverity::Warning,
        message: format!("image not found: {target}"),
        hints: Vec::new(),
        source: Some(span.clone()),
    }));

    let today = options
        .date
        .and_then(|(year, month, day)| Datetime::from_ymd(year, month, day));
    let world = PdfWorld::new(typst.source.clone(), fonts, &typst.files, today);
    let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
    let map = |list: &[SourceDiagnostic]| -> Vec<PdfDiagnostic> {
        list.iter()
            .map(|diagnostic| diagnostic_of(&world, typst, diagnostic))
            .collect()
    };
    diagnostics.extend(map(&warnings));
    let document = output.map_err(|errors| PdfError::Compile(map(&errors)))?;
    Ok((document, diagnostics, world))
}

fn diagnostic_of(world: &PdfWorld, typst: &Typst, diagnostic: &SourceDiagnostic) -> PdfDiagnostic {
    let source = world
        .range(diagnostic.span)
        .and_then(|range| typst.map.lookup(range.start))
        .cloned()
        .or_else(|| {
            // An error inside the template: the innermost call into it from the body.
            diagnostic.trace.iter().rev().find_map(|point| {
                world
                    .range(point.span)
                    .and_then(|range| typst.map.lookup(range.start))
                    .cloned()
            })
        });
    PdfDiagnostic {
        severity: match diagnostic.severity {
            Severity::Error => PdfSeverity::Error,
            Severity::Warning => PdfSeverity::Warning,
        },
        message: diagnostic.message.to_string(),
        hints: diagnostic
            .hints
            .iter()
            .map(|hint| hint.v.to_string())
            .collect(),
        source,
    }
}
