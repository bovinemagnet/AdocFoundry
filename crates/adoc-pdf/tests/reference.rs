//! Writes the reference article's PDF and Typst for inspection:
//! `ADOC_PDF_OUT=<dir> cargo test -p adoc-pdf --test reference -- --ignored`.

mod common;

use adoc_pdf::{export, to_typst, PdfOptions};

#[test]
#[ignore = "writes files for inspection; needs ADOC_PDF_OUT"]
fn writes_the_reference_article() {
    let out = std::path::PathBuf::from(std::env::var("ADOC_PDF_OUT").expect("ADOC_PDF_OUT"));
    let path = common::fixtures().join("pdf/reference/article.adoc");
    let pages = common::article(common::view_of(&path));
    let theme = common::default_theme();
    let options = PdfOptions::default();
    std::fs::write(
        out.join("article.typ"),
        to_typst(&pages, &theme, &options).source,
    )
    .unwrap();
    match export(&pages, &theme, &options) {
        Ok(output) => {
            for diagnostic in &output.diagnostics {
                eprintln!("{diagnostic}");
            }
            std::fs::write(out.join("article.pdf"), output.bytes).unwrap();
            eprintln!("{} pages", output.pages);
        }
        Err(error) => panic!("{error}"),
    }
}

/// The reference article with the custom theme and a title page, for inspection.
#[test]
#[ignore = "writes files for inspection; needs ADOC_PDF_OUT"]
fn writes_the_reference_article_with_the_custom_theme() {
    let out = std::path::PathBuf::from(std::env::var("ADOC_PDF_OUT").expect("ADOC_PDF_OUT"));
    let path = common::fixtures().join("pdf/reference/article.adoc");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replacen(":toc:", ":toc:\n:title-page:", 1);
    let pages = common::article(common::view_of_text(&path, &text));
    let theme = adoc_pdf::load_theme(&adoc_pdf::ThemeSource::File(
        common::fixtures().join("pdf/themes/acme-theme.yml"),
    ))
    .unwrap();
    let options = PdfOptions {
        fonts_dirs: vec![
            common::fixtures().join("pdf/themes"),
            "GEM_FONTS_DIR".into(),
        ],
        ..PdfOptions::default()
    };
    std::fs::write(
        out.join("acme.typ"),
        to_typst(&pages, &theme, &options).source,
    )
    .unwrap();
    let output = export(&pages, &theme, &options).unwrap_or_else(|error| panic!("{error}"));
    for diagnostic in &output.diagnostics {
        eprintln!("{diagnostic}");
    }
    std::fs::write(out.join("acme.pdf"), output.bytes).unwrap();
}

/// Compiles a hand-edited Typst file (`ADOC_PDF_TYP`) with the default theme's fonts, for
/// experiments with the template: writes `<file>.pdf` beside it.
#[test]
#[ignore = "an experiment; needs ADOC_PDF_TYP"]
fn compiles_an_edited_typst_file() {
    let path = std::path::PathBuf::from(std::env::var("ADOC_PDF_TYP").expect("ADOC_PDF_TYP"));
    let typst = adoc_pdf::Typst {
        source: std::fs::read_to_string(&path).unwrap(),
        ..adoc_pdf::Typst::default()
    };
    let theme = common::default_theme();
    let output = adoc_pdf::compile(&typst, &theme, &PdfOptions::default())
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(path.with_extension("pdf"), output.bytes).unwrap();
}

/// Any page (`ADOC_PDF_PAGE`) written as Typst and PDF to `ADOC_PDF_OUT`, for inspection.
#[test]
#[ignore = "writes files for inspection; needs ADOC_PDF_PAGE and ADOC_PDF_OUT"]
fn writes_a_page() {
    let out = std::path::PathBuf::from(std::env::var("ADOC_PDF_OUT").expect("ADOC_PDF_OUT"));
    let path = std::path::PathBuf::from(std::env::var("ADOC_PDF_PAGE").expect("ADOC_PDF_PAGE"));
    let pages = common::article(common::view_of(&path));
    let theme = common::default_theme();
    let options = PdfOptions::default();
    std::fs::write(
        out.join("page.typ"),
        to_typst(&pages, &theme, &options).source,
    )
    .unwrap();
    let output = export(&pages, &theme, &options).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(out.join("page.pdf"), output.bytes).unwrap();
}
