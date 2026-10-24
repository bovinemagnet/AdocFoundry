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
    document::{to_typst, Document, PdfOptions, PdfStandard, Typst},
    fonts::font_set,
    theme::LoadedTheme,
    world::PdfWorld,
};

#[cfg(test)]
mod tests {
    #[test]
    fn days_become_dates() {
        assert_eq!(super::civil_from_days(0), (1970, 1, 1));
        assert_eq!(super::civil_from_days(20_385), (2025, 10, 24));
        assert_eq!(super::civil_from_days(11_016), (2000, 2, 29));
    }
}

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

/// Exports `document` as a PDF styled by `theme`.
pub fn export(
    document: &Document,
    theme: &LoadedTheme,
    options: &PdfOptions,
) -> Result<PdfOutput, PdfError> {
    let typst = to_typst(document, theme, options);
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
    // A standard needs a date; an ordinary PDF has none unless one is given, so that the same
    // input gives the same bytes.
    let date = options
        .date
        .or_else(|| (!options.standards.is_empty()).then(today_utc));
    let today = date.and_then(|(year, month, day)| Datetime::from_ymd(year, month, day));
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

/// One formula typeset on its own, as SVG, for the preview and the HTML backend: `math` in
/// `size`-point type, displayed (`block`) or inline, on a transparent page that fits it.
///
/// # Errors
///
/// When Typst cannot typeset it.
pub fn render_math_svg(
    math: &adoc_math::TypstMath,
    block: bool,
    size: f64,
) -> Result<String, PdfError> {
    let size = if size.is_finite() && size > 0.0 {
        size
    } else {
        12.0
    };
    let source = format!(
        "{}\n#set page(width: auto, height: auto, margin: 1pt, fill: none)\n#set text(size: {size}pt, font: \"Noto Serif\")\n#adoc-stem({}, \"\", block: {block})\n",
        crate::document::MATH,
        crate::typst::string(math.as_str()),
    );
    let world = PdfWorld::new(source, crate::fonts::bundled_set(), &[], None);
    let Warned { output, .. } = typst::compile::<PagedDocument>(&world);
    // Formulas render one at a time for as long as the app runs: let old entries go.
    typst::comemo::evict(30);
    let document = output.map_err(|errors| {
        PdfError::Compile(
            errors
                .iter()
                .map(|error| PdfDiagnostic {
                    severity: PdfSeverity::Error,
                    message: error.message.to_string(),
                    hints: Vec::new(),
                    source: None,
                })
                .collect(),
        )
    })?;
    let page = document.pages().first().ok_or_else(|| {
        PdfError::Compile(vec![PdfDiagnostic {
            severity: PdfSeverity::Error,
            message: "the formula typeset to nothing".to_owned(),
            hints: Vec::new(),
            source: None,
        }])
    })?;
    Ok(typst_svg::svg(page, &typst_svg::SvgOptions::default()))
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
    diagnostics.extend(typst.missing.iter().map(|(message, span)| PdfDiagnostic {
        severity: PdfSeverity::Warning,
        message: message.clone(),
        hints: Vec::new(),
        source: Some(span.clone()),
    }));
    diagnostics.extend(
        typst
            .unconverted
            .iter()
            .map(|(reason, span)| PdfDiagnostic {
                severity: PdfSeverity::Warning,
                message: format!("STEM not typeset: {reason}"),
                hints: Vec::new(),
                source: Some(span.clone()),
            }),
    );

    let today = options
        .date
        .and_then(|(year, month, day)| Datetime::from_ymd(year, month, day));
    let world = PdfWorld::new(typst.source.clone(), fonts, &typst.files, today);
    let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
    // Typst memoises layout in a process-wide cache that nothing else empties; a book's is
    // gigabytes, and the app would keep it after the export.
    typst::comemo::evict(0);
    let map = |list: &[SourceDiagnostic]| -> Vec<PdfDiagnostic> {
        list.iter()
            .map(|diagnostic| diagnostic_of(&world, typst, diagnostic))
            .collect()
    };
    diagnostics.extend(map(&warnings));
    let document = output.map_err(|errors| PdfError::Compile(map(&errors)))?;
    Ok((document, diagnostics, world))
}

/// Today's date in UTC, from the system clock.
fn today_utc() -> (i32, u8, u8) {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() / 86_400);
    civil_from_days(i64::try_from(days).unwrap_or(0))
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(1970),
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
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
