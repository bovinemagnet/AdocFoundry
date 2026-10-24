//! WP 4.4: running content, the title page, the table of contents and recto/verso parity.

mod common;

use std::path::{Path, PathBuf};

use adoc_pdf::{
    load_theme, to_typst, typeset_pages, LoadedTheme, PdfOptions, ThemeSource, TypesetPage,
};

/// A three-page article: a long section per page.
fn article(header: &str) -> String {
    let filler = "Filler text that runs on long enough to fill the page with lines. ".repeat(60);
    format!(
        "= The Handbook\nAda Writer\nv2.0, 2026-10-24: Second edition\n{header}\n\n== First\n\n{filler}\n\n== Second\n\n{filler}\n\n== Third\n\n{filler}\n"
    )
}

fn pages_of(text: &str, theme: &LoadedTheme) -> Vec<TypesetPage> {
    let view = common::view_of_text(Path::new("/doc/handbook.adoc"), text);
    let typst = to_typst(&common::article(view), theme, &PdfOptions::default());
    typeset_pages(&typst, theme, &PdfOptions::default()).unwrap_or_else(|error| panic!("{error}"))
}

/// A theme file in a temporary directory, extending `default`.
fn theme_with(yaml: &str) -> (tempfile::TempDir, LoadedTheme) {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("test-theme.yml");
    std::fs::write(&path, format!("extends: default\n{yaml}")).unwrap();
    let theme = load_theme(&ThemeSource::File(path)).unwrap();
    (dir, theme)
}

/// The runs in the bottom margin of a page: its footer.
fn footer(page: &TypesetPage) -> Vec<&adoc_pdf::TextRun> {
    page.runs
        .iter()
        .filter(|run| run.y > page.height - 48.0)
        .collect()
}

fn header(page: &TypesetPage) -> Vec<&adoc_pdf::TextRun> {
    page.runs.iter().filter(|run| run.y < 36.0).collect()
}

#[test]
fn header_and_footer_follow_parity() {
    let (_dir, theme) = theme_with(
        "header:\n  height: 0.5in\n  recto:\n    center:\n      content: '{document-title}'\n  verso:\n    center:\n      content: '{document-title}'\nfooter:\n  recto:\n    right:\n      content: '{section-title} | {page-number}'\n  verso:\n    left:\n      content: '{page-number} | {section-title}'\n",
    );
    let pages = pages_of(&article(""), &theme);
    assert!(pages.len() >= 3, "{} pages", pages.len());
    let (recto, verso) = (&pages[0], &pages[1]);
    let text = |runs: Vec<&adoc_pdf::TextRun>| {
        runs.iter().map(|run| run.text.as_str()).collect::<String>()
    };
    assert!(
        text(header(recto)).contains("The Handbook"),
        "{:?}",
        header(recto)
    );
    assert!(text(footer(recto)).contains("| 1"), "{:?}", footer(recto));
    assert!(text(footer(verso)).contains("2 |"), "{:?}", footer(verso));
    // Recto content is on the right of the page, verso content on the left.
    let x = |runs: Vec<&adoc_pdf::TextRun>| runs.iter().map(|run| run.x).fold(f64::NAN, f64::min);
    assert!(x(footer(recto)) > recto.width / 2.0);
    assert!(x(footer(verso)) < verso.width / 2.0);
}

#[test]
fn running_content_starts_where_the_theme_says() {
    let (_dir, theme) = theme_with("running_content:\n  start_at: 2\n");
    let pages = pages_of(&article(""), &theme);
    assert!(footer(&pages[0]).is_empty(), "{:?}", footer(&pages[0]));
    assert!(!footer(&pages[1]).is_empty());
    // With a title page, running content and page numbers start after it.
    let pages = pages_of(&article(":title-page:"), &common::default_theme());
    assert!(footer(&pages[0]).is_empty());
    let first_body = &pages[1];
    assert_eq!(
        footer(first_body)
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
        "1"
    );
}

#[test]
fn title_page_shows_title_authors_and_revision() {
    let pages = pages_of(&article(":title-page:"), &common::default_theme());
    let title_page = pages[0].text();
    for expected in [
        "The Handbook",
        "Ada Writer",
        "Version 2.0, 2026-10-24: Second edition",
    ] {
        assert!(
            title_page.contains(expected),
            "{expected:?} not in {title_page:?}"
        );
    }
    assert!(!title_page.contains("Filler"));
}

#[test]
fn toc_lists_sections_to_toclevels() {
    let text = "= Guide\n:toc:\n:toclevels: 1\n\n== Top\n\n=== Nested\n\nText.\n";
    let pages = pages_of(text, &common::default_theme());
    let first = pages[0].text();
    assert!(first.contains("Table of Contents"));
    assert_eq!(first.matches("Top").count(), 2, "{first}");
    assert_eq!(first.matches("Nested").count(), 1, "{first}");

    let pages = pages_of(
        &text.replace(":toclevels: 1", ":toclevels: 2"),
        &common::default_theme(),
    );
    assert_eq!(pages[0].text().matches("Nested").count(), 2);
}

#[test]
fn page_size_and_margins_follow_the_theme() {
    let (_dir, theme) = theme_with(
        "page:\n  size: Letter\n  layout: landscape\n  margin: [1in, 2in, 1in, 1.5in]\n",
    );
    let pages = pages_of(&article(""), &theme);
    let page = &pages[0];
    assert!(
        (page.width - 792.0).abs() < 0.01 && (page.height - 612.0).abs() < 0.01,
        "{}x{}",
        page.width,
        page.height
    );
    let left = page
        .runs
        .iter()
        .map(|run| run.x)
        .fold(f64::INFINITY, f64::min);
    assert!((left - 108.0).abs() < 1.0, "text starts at {left}");
    let top = page
        .runs
        .iter()
        .map(|run| run.y)
        .fold(f64::INFINITY, f64::min);
    assert!(top > 72.0, "text starts at {top}");
}

#[test]
fn attribute_references_in_running_content_resolve() {
    let (_dir, theme) = theme_with(
        "footer:\n  recto:\n    left:\n      content: '{product} v{revnumber} *{page-number}* of {page-count}'\n    right:\n      content: '{undefined-attribute} dropped'\n",
    );
    let pages = pages_of(&article(":product: Foundry"), &theme);
    let footer_text: String = footer(&pages[0])
        .iter()
        .map(|run| run.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(footer_text.contains("Foundry v2.0"), "{footer_text:?}");
    assert!(
        footer_text.contains(&format!("of {}", pages.len())),
        "{footer_text:?}"
    );
    assert!(!footer_text.contains("dropped"), "{footer_text:?}");
}

#[test]
fn footnotes_sit_where_asciidoctor_pdf_puts_them() {
    // asciidoctor-pdf 2.3.15 tops the reference article's footnote line at 779.20 pt on page 4;
    // at the footnote size (8 pt), Noto Serif's line top is its ascent, 8.55 pt, above its
    // baseline. Before the last item kept its spacing below it, the footnote sat 3.6 pt low.
    const ASCIIDOCTOR_PDF_TOP: f64 = 779.20;
    const BASELINE_TO_TOP: f64 = 8.55;
    let path = common::fixtures().join("pdf/reference/article.adoc");
    let document = common::article(common::view_of(&path));
    let typst = to_typst(&document, &common::default_theme(), &PdfOptions::default());
    let pages = typeset_pages(&typst, &common::default_theme(), &PdfOptions::default())
        .unwrap_or_else(|error| panic!("{error}"));
    let footnote = pages
        .last()
        .unwrap()
        .runs
        .iter()
        .find(|run| run.text.contains("Footnotes"))
        .expect("the footnote is on the last page");
    let top = footnote.y - BASELINE_TO_TOP;
    assert!(
        (top - ASCIIDOCTOR_PDF_TOP).abs() <= 1.0,
        "the footnote's top is {top:.2} pt, asciidoctor-pdf's {ASCIIDOCTOR_PDF_TOP} pt"
    );
}
