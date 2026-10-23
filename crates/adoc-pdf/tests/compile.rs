//! WP 4.3: compiling in process with the bundled fonts, a world that reaches only the files the
//! document uses, and Typst's errors mapped back to AsciiDoc lines.

mod common;

use std::path::Path;

use adoc_pdf::{compile, export, to_typst, PdfError, PdfOptions, PdfSeverity, Typst};

fn reference() -> adoc_pdf::Document {
    let path = common::fixtures().join("pdf/reference/article.adoc");
    common::article(common::view_of(&path))
}

#[test]
fn compiles_with_bundled_fonts_only() {
    let output = export(
        &reference(),
        &common::default_theme(),
        &PdfOptions::default(),
    )
    .unwrap();
    assert!(output.bytes.starts_with(b"%PDF-"));
    assert!(output.pages >= 3, "{} pages", output.pages);
    let bytes = String::from_utf8_lossy(&output.bytes);
    assert!(bytes.contains("NotoSerif"), "Noto Serif is not embedded");
    assert!(bytes.contains("Mplus1mn") || bytes.contains("mplus1mn") || bytes.contains("M+1mn"));
    assert!(
        output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == PdfSeverity::Warning),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn world_reaches_only_listed_files() {
    let theme = common::default_theme();
    let mut typst: Typst = to_typst(&reference(), &theme, &PdfOptions::default());
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    typst
        .source
        .push_str(&format!("\n#read({:?})\n", manifest.display().to_string()));
    let error = compile(&typst, &theme, &PdfOptions::default()).unwrap_err();
    let PdfError::Compile(diagnostics) = error else {
        panic!("{error}")
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("not found")),
        "{diagnostics:?}"
    );
}

#[test]
fn errors_map_to_asciidoc_lines() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("broken.png"), "this is not a PNG").unwrap();
    let page = dir.path().join("page.adoc");
    let text = "= Page\n\nA paragraph.\n\nimage::broken.png[Broken]\n";
    std::fs::write(&page, text).unwrap();
    let pages = common::article(common::view_of(&page));
    let error = export(&pages, &common::default_theme(), &PdfOptions::default()).unwrap_err();
    let diagnostic = error
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.severity == PdfSeverity::Error)
        .unwrap_or_else(|| panic!("{error}"));
    let source = diagnostic
        .source
        .as_ref()
        .unwrap_or_else(|| panic!("{error}"));
    assert_eq!(source.path, page);
    assert_eq!(source.line, 5, "{error}");
    assert!(error.to_string().contains("page.adoc:5:"), "{error}");
}

#[test]
fn missing_image_is_a_diagnostic_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.adoc");
    std::fs::write(&page, "= Page\n\nimage::nowhere.png[Gone]\n").unwrap();
    let pages = common::article(common::view_of(&page));
    let output = export(&pages, &common::default_theme(), &PdfOptions::default()).unwrap();
    let diagnostic = output
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("nowhere.png"))
        .unwrap_or_else(|| panic!("{:?}", output.diagnostics));
    assert_eq!(diagnostic.severity, PdfSeverity::Warning);
    assert_eq!(
        diagnostic.source.as_ref().map(|source| source.line),
        Some(3)
    );
}

#[test]
fn compile_is_deterministic() {
    let options = PdfOptions {
        date: Some((2026, 10, 24)),
        ..PdfOptions::default()
    };
    let theme = common::default_theme();
    let first = export(&reference(), &theme, &options).unwrap();
    let second = export(&reference(), &theme, &options).unwrap();
    assert_eq!(first.bytes, second.bytes);
}

/// Every page under `ADOC_PDF_CORPUS` exported as an article: none may fail to typeset.
#[test]
#[ignore = "needs ADOC_PDF_CORPUS"]
fn corpus_pages_typeset() {
    fn pages(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                    && !path.ends_with("node_modules")
                    && !path.ends_with("build")
                {
                    pages(&path, out);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("adoc") {
                out.push(path);
            }
        }
    }
    let corpus = std::env::var("ADOC_PDF_CORPUS").expect("ADOC_PDF_CORPUS");
    let mut paths = Vec::new();
    pages(Path::new(&corpus), &mut paths);
    paths.sort();
    let theme = common::default_theme();
    let mut failures = Vec::new();
    let started = std::time::Instant::now();
    for path in &paths {
        let pages = common::article(common::view_of(path));
        if let Err(error) = export(&pages, &theme, &PdfOptions::default()) {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    eprintln!("{} pages in {:?}", paths.len(), started.elapsed());
    assert!(
        failures.is_empty(),
        "{} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
