//! PDF export: asciidoctor-pdf themes, the view tree written as Typst, and Typst compiled in
//! process with bundled fonts.

mod document;
mod emit;
mod export;
mod fonts;
mod running;
pub mod theme;
mod typst;
mod world;

pub use document::{to_typst, Book, BookItem, Document, PdfOptions, PdfStandard, Typst};
pub use emit::{EmitOptions, PageView, SourceMap};
pub use export::{
    compile, export, render_math_svg, typeset_pages, typeset_text, PdfDiagnostic, PdfError,
    PdfOutput, PdfSeverity, TextRun, TypesetPage,
};

pub use theme::{
    load_theme, load_theme_within, Color, FontCatalog, FontFaces, FontFile, LoadedTheme,
    ThemeError, ThemeKeys, ThemeSource, ThemeValue, ThemeWarning,
};
