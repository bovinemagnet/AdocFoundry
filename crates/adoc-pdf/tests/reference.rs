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
