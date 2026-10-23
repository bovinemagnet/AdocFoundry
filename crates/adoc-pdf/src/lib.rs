//! PDF export: asciidoctor-pdf themes, the view tree written as Typst, and Typst compiled in
//! process with bundled fonts.

mod document;
mod emit;
mod export;
mod fonts;
pub mod theme;
mod typst;
mod world;

pub use document::{to_typst, PdfOptions, PdfStandard, Typst};
pub use emit::{EmitOptions, PageView, SourceMap};
pub use export::{compile, export, typeset_text, PdfDiagnostic, PdfError, PdfOutput, PdfSeverity};

pub use theme::{
    load_theme, Color, FontCatalog, FontFaces, FontFile, LoadedTheme, ThemeError, ThemeKeys,
    ThemeSource, ThemeValue, ThemeWarning,
};
