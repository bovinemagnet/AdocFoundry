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

/// `text` written as `page.adoc` beside `logo.png` holding `logo`, exported with `theme`.
fn export_with_logo(
    text: &str,
    logo: Option<&[u8]>,
    theme: &LoadedTheme,
) -> (adoc_pdf::PdfOutput, Vec<TypesetPage>) {
    let directory = tempfile::tempdir().unwrap();
    if let Some(logo) = logo {
        std::fs::write(directory.path().join("logo.png"), logo).unwrap();
    }
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, text).unwrap();
    let document = common::article(common::view_of(&page));
    let output = adoc_pdf::export(&document, theme, &PdfOptions::default())
        .unwrap_or_else(|error| panic!("{error}"));
    let typst = to_typst(&document, theme, &PdfOptions::default());
    let pages = typeset_pages(&typst, theme, &PdfOptions::default()).unwrap();
    (output, pages)
}

fn image_warnings(output: &adoc_pdf::PdfOutput) -> Vec<&adoc_pdf::PdfDiagnostic> {
    output
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("logo.png"))
        .collect()
}

#[test]
fn a_title_logo_is_found_beside_the_page_and_checked() {
    let png = std::fs::read(common::fixtures().join("pdf/reference/images/wide.png")).unwrap();
    let text = "= Handbook\n:title-page:\n:title-logo-image: image:logo.png[]\n\nText.\n";
    // Beside the page, as asciidoctor-pdf finds it.
    let (output, pages) = export_with_logo(text, Some(&png), &common::default_theme());
    assert_eq!(pages[0].images.len(), 1, "{:?}", output.diagnostics);
    assert!(
        image_warnings(&output).is_empty(),
        "{:?}",
        output.diagnostics
    );
    // Missing, or not an image: a warning, and the document still exports.
    for logo in [None, Some(&b"not an image"[..])] {
        let (output, pages) = export_with_logo(text, logo, &common::default_theme());
        assert!(pages[0].images.is_empty());
        assert_eq!(image_warnings(&output).len(), 1, "{:?}", output.diagnostics);
    }
    // A width in any unit, as a body image reads it.
    let sized =
        "= Handbook\n:title-page:\n:title-logo-image: image:logo.png[pdfwidth=1in]\n\nText.\n";
    let (_, pages) = export_with_logo(sized, Some(&png), &common::default_theme());
    assert!(
        (pages[0].images[0].width - 72.0).abs() < 0.01,
        "{:?}",
        pages[0].images
    );
}

#[test]
fn a_running_content_image_that_cannot_be_shown_is_a_warning() {
    // Found beside the theme, as asciidoctor-pdf finds it.
    let (dir, theme) = theme_with(
        "header:\n  height: 0.75in\n  recto:\n    center:\n      content: image:logo.png[]\n  verso:\n    center:\n      content: image:logo.png[]\n",
    );
    for logo in [None, Some(&b"not an image"[..])] {
        if let Some(logo) = logo {
            std::fs::write(dir.path().join("logo.png"), logo).unwrap();
        }
        let (output, pages) = export_with_logo("= Page\n\nText.\n", None, &theme);
        assert_eq!(image_warnings(&output).len(), 1, "{:?}", output.diagnostics);
        assert!(!pages[0].text().contains("image:"), "{}", pages[0].text());
    }
}

#[test]
fn footnotes_taller_than_a_page_flow_onto_the_next() {
    // A float cannot break across pages: a page of notes and more was drawn past the bottom
    // edge and lost.
    let notes: String = (1..=140)
        .map(|number| format!("Point.footnote:[MARK{number} a note of one line.] "))
        .collect();
    let pages = pages_of(&format!("= Notes\n\n{notes}\n"), &common::default_theme());
    for page in &pages {
        let below: Vec<_> = page.runs.iter().filter(|run| run.y > page.height).collect();
        assert!(below.is_empty(), "{} runs below the page", below.len());
    }
    let text: String = pages.iter().map(TypesetPage::text).collect();
    for number in 1..=140 {
        assert!(
            text.contains(&format!("MARK{number} ")),
            "note {number} is on no page"
        );
    }
}

/// The label a PDF viewer shows for each page, from the catalogue's `/PageLabels`.
fn page_labels(bytes: &[u8], pages: usize) -> Vec<String> {
    use hayro_syntax::object::{Array, Dict, Name, Object, String as PdfString};

    fn roman(mut number: u32) -> String {
        let mut out = String::new();
        for (value, symbol) in [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ] {
            while number >= value {
                out.push_str(symbol);
                number -= value;
            }
        }
        out
    }

    let pdf = hayro_syntax::Pdf::new(bytes.to_vec()).unwrap();
    let catalog: Dict = pdf.xref().get(pdf.xref().root_id()).unwrap();
    let Some(tree) = catalog.get::<Dict>(b"PageLabels") else {
        return (1..=pages).map(|page| page.to_string()).collect();
    };
    let nums: Array = tree.get(b"Nums").expect("a flat number tree");
    let entries: Vec<Object> = nums.iter::<Object>().collect();
    let ranges: Vec<(usize, Dict)> = entries
        .chunks(2)
        .filter_map(|pair| match (&pair[0], &pair[1]) {
            (Object::Number(start), Object::Dict(label)) => {
                Some((start.as_f64() as usize, label.clone()))
            }
            _ => None,
        })
        .collect();
    (0..pages)
        .map(|page| {
            let Some((start, label)) = ranges.iter().rev().find(|(start, _)| *start <= page) else {
                return String::new();
            };
            let prefix = label
                .get::<PdfString>(b"P")
                .map(|prefix| String::from_utf8_lossy(prefix.as_bytes()).into_owned())
                .unwrap_or_default();
            let first = label
                .get::<Object>(b"St")
                .and_then(|value| match value {
                    Object::Number(number) => Some(number.as_f64() as u32),
                    _ => None,
                })
                .unwrap_or(1);
            let number = first + (page - start) as u32;
            let style = label.get::<Name>(b"S").map(|name| name.as_str().to_owned());
            let shown = match style.as_deref() {
                Some("D") => number.to_string(),
                Some("r") => roman(number),
                Some("R") => roman(number).to_uppercase(),
                _ => String::new(),
            };
            prefix + &shown
        })
        .collect()
}

#[test]
fn page_labels_match_the_printed_numbers() {
    // As asciidoctor-pdf labels pages: the front matter in roman numerals, then the body from
    // 1, the number its footer prints.
    let theme = common::default_theme();
    for (header, front) in [
        (":title-page:\n:toc:", vec!["i", "ii"]),
        (":title-page:", vec!["i"]),
        (":toc:", vec![]),
        ("", vec![]),
    ] {
        let text = article(header);
        let view = common::view_of_text(Path::new("/doc/handbook.adoc"), &text);
        let output = adoc_pdf::export(&common::article(view), &theme, &PdfOptions::default())
            .unwrap_or_else(|error| panic!("{error}"));
        let labels = page_labels(&output.bytes, output.pages);
        assert_eq!(labels[..front.len()], front, "{header:?}: {labels:?}");
        let pages = pages_of(&text, &theme);
        for (index, page) in pages.iter().enumerate().skip(front.len()) {
            let printed: String = footer(page).iter().map(|run| run.text.as_str()).collect();
            assert_eq!(
                labels[index],
                (index - front.len() + 1).to_string(),
                "{header:?}: {labels:?}"
            );
            assert!(
                printed.contains(&labels[index]),
                "{header:?}: page {index} prints {printed:?}, labelled {}",
                labels[index]
            );
        }
    }
}

/// The runs of `page` drawn in the bundled Font Awesome fonts.
fn icon_runs(page: &TypesetPage) -> Vec<&adoc_pdf::TextRun> {
    page.runs
        .iter()
        .filter(|run| run.font.starts_with("Font Awesome 5 Free"))
        .collect()
}

#[test]
fn admonition_icon_replaces_the_label_with_icons_font() {
    let theme = common::default_theme();
    let pages = pages_of("= Doc\n:icons: font\n\nNOTE: Read this first.\n", &theme);
    let text = pages[0].text();
    assert!(!text.contains("NOTE"), "{text:?}");
    assert!(text.contains("Read this first."), "{text:?}");
    let icons = icon_runs(&pages[0]);
    assert_eq!(icons.len(), 1, "{:?}", pages[0].runs);
    assert_eq!(icons[0].text, "\u{f05a}", "fas-info-circle");
    // asciidoctor-pdf shrinks the 24 pt icon to the height of a one-line block.
    assert!(
        icons[0].size > 16.0 && icons[0].size <= 24.0,
        "{}",
        icons[0].size
    );

    // Without `icons=font`, or with an icon image of the block's own, the label is text.
    for source in [
        "= Doc\n\nNOTE: Read this first.\n",
        "= Doc\n:icons: font\n\n[NOTE,icon=custom]\nRead this first.\n",
    ] {
        let pages = pages_of(source, &theme);
        assert!(pages[0].text().contains("NOTE"), "{source:?}");
        assert!(icon_runs(&pages[0]).is_empty(), "{source:?}");
    }
}

#[test]
fn admonition_icons_follow_the_theme() {
    let (_dir, theme) = theme_with(
        "admonition:\n  label:\n    min_width: 100\n  icon:\n    note:\n      name: fa-lightbulb-o\n      size: 12\n      stroke_color: '00AA00'\n    tip:\n      name: fas-rocket\n",
    );
    let pages = pages_of(
        "= Doc\n:icons: font\n\nNOTE: Read this first.\n\nTIP: A tip.\n\nWARNING: Careful.\n",
        &theme,
    );
    let icons: Vec<(&str, f64)> = icon_runs(&pages[0])
        .iter()
        .map(|run| (run.text.as_str(), run.size))
        .collect();
    assert_eq!(icons.len(), 3, "{icons:?}");
    // far-lightbulb, at the theme's size.
    assert_eq!(icons[0], ("\u{f0eb}", 12.0));
    // A glyph that is not bundled falls back to the kind's own, far-lightbulb.
    assert_eq!(icons[1].0, "\u{f0eb}");
    assert_eq!(icons[2].0, "\u{f071}");
    // The label column is the theme's minimum width wide, not one and a half times the icon.
    let body_x = |pages: &[TypesetPage]| {
        pages[0]
            .runs
            .iter()
            .find(|run| run.text.contains("Read this first."))
            .unwrap()
            .x
    };
    let default = pages_of(
        "= Doc\n:icons: font\n\nNOTE: Read this first.\n",
        &common::default_theme(),
    );
    let moved = body_x(&pages) - body_x(&default);
    assert!((moved - (100.0 - 36.0)).abs() < 0.01, "{moved}");
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 0.01
}

fn printed(runs: Vec<&adoc_pdf::TextRun>) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

#[test]
fn page_attributes_override_the_theme() {
    // `pdf-page-size`, `pdf-page-layout` and `pdf-page-margin` are read before the theme's
    // `page_size`, `page_layout` and `page_margin`, in the forms asciidoctor-pdf reads.
    let (_dir, theme) = theme_with("page:\n  size: A5\n  margin: 0.5in\n");
    for (header, width, height) in [
        (":pdf-page-size: Letter", 612.0, 792.0),
        (":pdf-page-size: letter", 612.0, 792.0),
        (":pdf-page-size: Executive", 521.86, 756.0),
        (":pdf-page-size: [8.5in, 11in]", 612.0, 792.0),
        (":pdf-page-size: 8.5in x 11in", 612.0, 792.0),
        (":pdf-page-size: 200mm x 100mm", 566.9291, 283.4646),
        (
            ":pdf-page-size: Letter\n:pdf-page-layout: landscape",
            792.0,
            612.0,
        ),
        (":pdf-page-layout: landscape", 595.28, 419.53),
        // A size asciidoctor-pdf does not know is A4, as it is there.
        (":pdf-page-size: Unknown", 595.28, 841.89),
        ("", 419.53, 595.28),
    ] {
        let pages = pages_of(&article(header), &theme);
        let page = &pages[0];
        assert!(
            close(page.width, width) && close(page.height, height),
            "{header:?}: {}x{}",
            page.width,
            page.height
        );
    }
    let pages = pages_of(
        &article(":pdf-page-size: Letter\n:pdf-page-margin: [1in, 1.5in]"),
        &theme,
    );
    let page = &pages[0];
    assert!(close(page.width, 612.0), "{}", page.width);
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
    let pages = pages_of(&article(":pdf-page-margin: 1in"), &theme);
    let left = pages[0]
        .runs
        .iter()
        .map(|run| run.x)
        .fold(f64::INFINITY, f64::min);
    assert!((left - 72.0).abs() < 1.0, "text starts at {left}");
}

/// `text` written as `handbook.adoc` in `dir` and laid out with `theme` and `options`.
fn pages_in(
    dir: &Path,
    text: &str,
    theme: &LoadedTheme,
    options: &PdfOptions,
) -> (adoc_pdf::Typst, Vec<TypesetPage>) {
    let page = dir.join("handbook.adoc");
    std::fs::write(&page, text).unwrap();
    let typst = to_typst(&common::article(common::view_of(&page)), theme, options);
    let pages = typeset_pages(&typst, theme, options).unwrap_or_else(|error| panic!("{error}"));
    (typst, pages)
}

#[test]
fn front_cover_is_the_first_page() {
    // As asciidoctor-pdf draws covers: the front cover is a page of its own before the title
    // page, the image fitted inside the whole page, with no running content; the back cover
    // follows the body. The body's pages are numbered from 1 all the same.
    let images = common::fixtures().join("pdf/reference/images");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("images")).unwrap();
    std::fs::copy(images.join("wide.png"), dir.path().join("images/cover.png")).unwrap();
    std::fs::copy(images.join("wide.png"), dir.path().join("back.png")).unwrap();
    let (_theme_dir, theme) = theme_with(
        "footer:\n  recto:\n    right:\n      content: '{page-number}'\n  verso:\n    left:\n      content: '{page-number}'\n",
    );
    let (typst, pages) = pages_in(
        dir.path(),
        &article(
            ":title-page:\n:imagesdir: images\n:front-cover-image: image:cover.png[]\n:back-cover-image: back.png",
        ),
        &theme,
        &PdfOptions::default(),
    );
    assert!(typst.missing.is_empty(), "{:?}", typst.missing);
    let cover = &pages[0];
    assert_eq!(cover.images.len(), 1, "{:?}", cover.images);
    assert!(cover.runs.is_empty(), "{:?}", cover.runs);
    let image = &cover.images[0];
    // A 4:1 image: as wide as the page, in the middle of it.
    assert!(
        close(image.x, 0.0) && close(image.width, cover.width),
        "{image:?}"
    );
    assert!(close(image.width / image.height, 4.0), "{image:?}");
    assert!(
        close(image.y, (cover.height - image.height) / 2.0),
        "{image:?}"
    );
    assert!(
        pages[1].text().contains("The Handbook"),
        "{}",
        pages[1].text()
    );
    assert!(pages[1].images.is_empty());
    assert_eq!(printed(footer(&pages[2])), "1");
    let back = pages.last().unwrap();
    assert_eq!(back.images.len(), 1, "{:?}", back.images);
    assert!(back.runs.is_empty(), "{:?}", back.runs);
    assert!(close(back.images[0].width, back.width));
    // The front cover is labelled as front matter, before the title page.
    let output = adoc_pdf::export(
        &common::article(common::view_of(&dir.path().join("handbook.adoc"))),
        &theme,
        &PdfOptions::default(),
    )
    .unwrap();
    let labels = page_labels(&output.bytes, output.pages);
    assert_eq!(labels[..3], ["i", "ii", "1"], "{labels:?}");

    // A book's cover comes from its attributes, found as its first page's images are.
    let book = adoc_pdf::Document::Book(adoc_pdf::Book {
        title: "The Handbook".to_owned(),
        attributes: [
            ("doctype", "book"),
            ("imagesdir", "images"),
            ("front-cover-image", "image:cover.png[]"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect(),
        items: vec![adoc_pdf::BookItem::Page(adoc_pdf::PageView {
            key: "p0".to_owned(),
            view: common::view_of(&dir.path().join("handbook.adoc")),
            depth: 1,
        })],
        ..adoc_pdf::Book::default()
    });
    let typst = to_typst(&book, &theme, &PdfOptions::default());
    assert!(typst.missing.is_empty(), "{:?}", typst.missing);
    let pages = typeset_pages(&typst, &theme, &PdfOptions::default()).unwrap();
    assert_eq!(pages[0].images.len(), 1, "{:?}", pages[0].images);
    assert!(pages[1].text().contains("The Handbook"));

    // Fitted to cover the page instead, and a blank back cover.
    let (_, pages) = pages_in(
        dir.path(),
        &article(
            ":imagesdir: images\n:front-cover-image: image:cover.png[fit=cover]\n:back-cover-image:",
        ),
        &theme,
        &PdfOptions::default(),
    );
    let image = &pages[0].images[0];
    assert!(close(image.height, pages[0].height), "{image:?}");
    let back = pages.last().unwrap();
    assert!(back.images.is_empty() && back.runs.is_empty(), "{back:?}");
    assert!(
        pages[pages.len() - 2].text().contains("Filler"),
        "the page before the back cover is the body's"
    );

    // A PDF cover, its first page drawn as the image.
    let cover = adoc_pdf::export(
        &common::article(common::view_of_text(
            Path::new("/doc/cover.adoc"),
            "= Cover\n\nThe cover.\n",
        )),
        &common::default_theme(),
        &PdfOptions::default(),
    )
    .unwrap();
    std::fs::write(dir.path().join("images/cover.pdf"), &cover.bytes).unwrap();
    let (typst, pages) = pages_in(
        dir.path(),
        &article(":imagesdir: images\n:front-cover-image: image:cover.pdf[]"),
        &theme,
        &PdfOptions::default(),
    );
    assert!(typst.missing.is_empty(), "{:?}", typst.missing);
    assert_eq!(pages[0].images.len(), 1, "{:?}", pages[0].images);
    assert!(pages[0].runs.is_empty());
    assert!(pages[1].text().contains("Filler"));

    // The theme's covers, resolved beside the theme; the document's `none` turns one off.
    let (theme_dir, theme) = theme_with(
        "cover:\n  front:\n    image: image:theme-cover.png[]\n  back:\n    image: theme-cover.png\n",
    );
    std::fs::copy(
        images.join("wide.png"),
        theme_dir.path().join("theme-cover.png"),
    )
    .unwrap();
    let (_, pages) = pages_in(dir.path(), &article(""), &theme, &PdfOptions::default());
    assert_eq!(pages[0].images.len(), 1);
    assert_eq!(pages.last().unwrap().images.len(), 1);
    let (_, pages) = pages_in(
        dir.path(),
        &article(":front-cover-image: none"),
        &theme,
        &PdfOptions::default(),
    );
    assert!(pages[0].images.is_empty());
    assert!(pages[0].text().contains("The Handbook"));
    assert_eq!(pages.last().unwrap().images.len(), 1);

    // A cover outside the workspace roots is left out and reported, as other images are.
    let elsewhere = tempfile::tempdir().unwrap();
    let options = PdfOptions {
        access: adoc_core::FileAccess::within(vec![elsewhere.path().to_path_buf()]),
        ..PdfOptions::default()
    };
    let (typst, pages) = pages_in(
        dir.path(),
        &article(":imagesdir: images\n:front-cover-image: image:cover.png[]"),
        &common::default_theme(),
        &options,
    );
    assert!(pages[0].images.is_empty());
    assert!(
        typst
            .missing
            .iter()
            .any(|(message, _)| message.contains("cover.png")),
        "{:?}",
        typst.missing
    );
}

#[test]
fn running_content_starts_at_the_title_or_the_contents() {
    // `running_content_start_at` names a page as asciidoctor-pdf does: `title`, `toc` or
    // `after-toc`. Front matter pages print their number in roman numerals, counted from the
    // first page.
    let footer_theme = "footer:\n  recto:\n    right:\n      content: '{page-number}'\n  verso:\n    left:\n      content: '{page-number}'\n";
    let header = ":title-page:\n:toc:";
    for (start, expected) in [
        ("title", ["i", "ii", "1"]),
        ("toc", ["", "ii", "1"]),
        ("after-toc", ["", "", "1"]),
        ("body", ["", "", "1"]),
    ] {
        let (_dir, theme) = theme_with(&format!(
            "{footer_theme}running_content:\n  start_at: {start}\n"
        ));
        let pages = pages_of(&article(header), &theme);
        let shown: Vec<String> = pages[..3]
            .iter()
            .map(|page| printed(footer(page)))
            .collect();
        assert_eq!(shown, expected, "{start}");
    }
    // Without a title page there is no front matter, and the names mean the body.
    let (_dir, theme) = theme_with(&format!(
        "{footer_theme}running_content:\n  start_at: title\n"
    ));
    let pages = pages_of(&article(":toc:"), &theme);
    assert_eq!(printed(footer(&pages[0])), "1");
}

#[test]
fn page_sizes_prawn_names_are_known_and_others_are_reported() {
    let export = |size: &str| {
        let view = common::view_of_text(
            Path::new("/doc/page.adoc"),
            &format!("= Page\n:pdf-page-size: {size}\n\nText.\n"),
        );
        adoc_pdf::export(
            &common::article(view),
            &common::default_theme(),
            &PdfOptions::default(),
        )
        .unwrap()
    };
    let pages = |size: &str| {
        let view = common::view_of_text(
            Path::new("/doc/page.adoc"),
            &format!("= Page\n:pdf-page-size: {size}\n\nText.\n"),
        );
        pages_of_view(view)
    };
    for (size, width) in [
        ("C5", 459.21),
        ("B6", 354.33),
        ("SRA4", 637.80),
        ("4A0", 4767.87),
    ] {
        assert!((pages(size)[0].width - width).abs() < 0.01, "{size}");
        assert!(export(size).diagnostics.is_empty(), "{size}");
    }
    let unknown = export("Postcard");
    assert!(
        unknown
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Postcard")),
        "{:?}",
        unknown.diagnostics
    );
}

fn pages_of_view(view: adoc_render::ViewTree) -> Vec<TypesetPage> {
    let typst = to_typst(
        &common::article(view),
        &common::default_theme(),
        &PdfOptions::default(),
    );
    typeset_pages(&typst, &common::default_theme(), &PdfOptions::default()).unwrap()
}

#[test]
fn page_layout_in_running_content_follows_the_page() {
    let (_dir, theme) = theme_with(
        "footer:\n  recto:\n    center:\n      content: 'layout {page-layout}'\n  verso:\n    center:\n      content: 'layout {page-layout}'\n",
    );
    for (layout, expected) in [
        ("landscape", "layout landscape"),
        ("portrait", "layout portrait"),
    ] {
        let pages = pages_of(
            &format!("= Page\n:pdf-page-layout: {layout}\n\nText.\n"),
            &theme,
        );
        let text: String = footer(&pages[0])
            .iter()
            .map(|run| run.text.as_str())
            .collect();
        assert!(text.contains(expected), "{layout}: {text:?}");
    }
}

#[test]
fn only_a_v_before_a_digit_is_taken_from_the_version() {
    for (number, expected) in [
        ("v2.0", "Version 2.0"),
        ("vNext", "Version vNext"),
        ("V3", "Version 3"),
    ] {
        let pages = pages_of(
            &format!("= Handbook\n:title-page:\n:revnumber: {number}\n\nText.\n"),
            &common::default_theme(),
        );
        assert!(
            pages[0].text().contains(expected),
            "{number}: {}",
            pages[0].text()
        );
    }
}
