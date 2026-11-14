//! Issue #62: the live PDF preview's pages, as SVG, with the source of each line of text so a
//! click on a page can go to it.

mod common;

use std::path::Path;

use adoc_pdf::{preview_pages, to_typst, PdfOptions};

#[test]
fn preview_pages_are_svg_with_their_source() {
    let path = Path::new("/doc/page.adoc");
    let text = "= Preview\n\nFirst paragraph.\n\n<<<\n\n== Second Page\n\nLast paragraph.\n";
    let view = common::view_of_text(path, text);
    let theme = common::default_theme();
    let typst = to_typst(&common::article(view), &theme, &PdfOptions::default());
    let pages = preview_pages(&typst, &theme, &PdfOptions::default())
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(pages.len(), 2);
    for page in &pages {
        assert!(page.svg.starts_with("<svg"), "{}", &page.svg[..40]);
        assert!(page.width > 500.0 && page.height > 700.0);
        // Anchors run down the page.
        assert!(page
            .anchors
            .windows(2)
            .all(|pair| pair[0].top <= pair[1].top));
        assert!(page.anchors.iter().all(|anchor| anchor.top < anchor.bottom));
    }
    let offset_of = |needle: &str| text.find(needle).unwrap();
    let anchor_for = |page: usize, offset: usize| {
        pages[page]
            .anchors
            .iter()
            .find(|anchor| anchor.path == path && anchor.offset == offset)
            .unwrap_or_else(|| {
                panic!(
                    "no anchor at {offset} on page {page}: {:?}",
                    pages[page].anchors
                )
            })
    };
    let first = anchor_for(0, offset_of("First paragraph."));
    let heading = anchor_for(1, offset_of("== Second Page"));
    let last = anchor_for(1, offset_of("Last paragraph."));
    assert!(first.top > 0.0);
    assert!(heading.bottom <= last.top + 1.0, "{heading:?} {last:?}");
}

/// The acme theme, with its font, copied to a folder of its own, so its font's path is one no
/// other test has loaded.
fn acme_theme_copy() -> (tempfile::TempDir, std::path::PathBuf) {
    let themes = common::fixtures().join("pdf/themes");
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("fonts")).unwrap();
    for file in [
        "acme-theme.yml",
        "acme-base-theme.yml",
        "fonts/acme-mono.ttf",
    ] {
        std::fs::copy(themes.join(file), directory.path().join(file)).unwrap();
    }
    let font = directory.path().join("fonts/acme-mono.ttf");
    (directory, font)
}

fn preview_with(theme: &adoc_pdf::LoadedTheme) {
    let path = Path::new("/doc/page.adoc");
    let view = common::view_of_text(path, "= Fonts\n\nSome `code` in a paragraph.\n");
    let typst = to_typst(&common::article(view), theme, &PdfOptions::default());
    preview_pages(&typst, theme, &PdfOptions::default()).unwrap_or_else(|error| panic!("{error}"));
}

/// Issue #188: a theme's own fonts are read from disk once, not on every compile and not
/// again for their line gaps.
#[test]
fn custom_fonts_are_read_once() {
    use adoc_core::probe::{count, reset, Probe};

    let (directory, _) = acme_theme_copy();
    let theme = adoc_pdf::load_theme(&adoc_pdf::ThemeSource::File(
        directory.path().join("acme-theme.yml"),
    ))
    .unwrap();
    reset();
    preview_with(&theme);
    // The catalogue names the one file for all four faces.
    assert_eq!(count(Probe::FontRead), 1, "read once for the first preview");
    preview_with(&theme);
    assert_eq!(count(Probe::FontRead), 1, "and not again for the next");
}

/// A font file that changes on disk is read again.
#[test]
fn a_changed_font_is_read_again() {
    use adoc_core::probe::{count, reset, Probe};

    let (directory, font) = acme_theme_copy();
    let theme = adoc_pdf::load_theme(&adoc_pdf::ThemeSource::File(
        directory.path().join("acme-theme.yml"),
    ))
    .unwrap();
    reset();
    preview_with(&theme);
    assert_eq!(count(Probe::FontRead), 1);
    // The same font with a byte more, so its size differs whatever the clock's resolution.
    let mut data = std::fs::read(&font).unwrap();
    data.push(0);
    std::fs::write(&font, data).unwrap();
    preview_with(&theme);
    assert_eq!(count(Probe::FontRead), 2);
}

/// Issue #188: a kept theme is loaded again only when it is chosen differently or one of its
/// files, the theme or one it extends, changes on disk.
#[test]
fn a_kept_theme_is_current_until_one_of_its_files_changes() {
    use adoc_core::probe::{count, reset, Probe};
    use adoc_core::FileAccess;
    use adoc_pdf::{KeptTheme, ThemeSource};

    let (directory, _) = acme_theme_copy();
    let source = ThemeSource::File(directory.path().join("acme-theme.yml"));
    reset();
    let kept = KeptTheme::load(&source, &FileAccess::Any).unwrap();
    assert_eq!(count(Probe::ThemeLoad), 1);
    assert_eq!(
        kept.theme(),
        &adoc_pdf::load_theme(&source).unwrap(),
        "the same theme as a load"
    );
    assert!(kept.is_current(&source, &FileAccess::Any));
    assert!(!kept.is_current(&ThemeSource::Bundled("default".into()), &FileAccess::Any));
    let elsewhere = FileAccess::within(vec![directory.path().join("fonts")]);
    assert!(
        !kept.is_current(&source, &elsewhere),
        "read with other access"
    );
    // The theme it extends gains a key, so its size differs whatever the clock's resolution.
    let base = directory.path().join("acme-base-theme.yml");
    let mut text = std::fs::read_to_string(&base).unwrap();
    text.push_str("sidebar:\n  font-color: 333333\n");
    std::fs::write(&base, text).unwrap();
    assert!(!kept.is_current(&source, &FileAccess::Any));
    let again = KeptTheme::load(&source, &FileAccess::Any).unwrap();
    assert!(again.is_current(&source, &FileAccess::Any));
    assert_ne!(again.theme(), kept.theme());
    assert_eq!(count(Probe::ThemeLoad), 2);
}

/// A bundled theme never changes.
#[test]
fn a_kept_bundled_theme_stays_current() {
    use adoc_core::FileAccess;
    use adoc_pdf::{KeptTheme, ThemeSource};

    let source = ThemeSource::Bundled("default".into());
    let kept = KeptTheme::load(&source, &FileAccess::Any).unwrap();
    assert!(kept.is_current(&source, &FileAccess::Any));
}

/// Issue #188: the preview keeps what Typst memoised, so compiling the same page again reuses
/// it. A timing probe, run with `--ignored --release`.
#[test]
#[ignore = "timing probe"]
fn preview_recompile_reuses_the_cache() {
    let path = common::fixtures().join("pdf/reference/article.adoc");
    let theme = common::default_theme();
    let typst = to_typst(
        &common::article(common::view_of(&path)),
        &theme,
        &PdfOptions::default(),
    );
    let time = || {
        let started = std::time::Instant::now();
        preview_pages(&typst, &theme, &PdfOptions::default())
            .unwrap_or_else(|error| panic!("{error}"));
        started.elapsed()
    };
    // Once for what is loaded once per process, then the cache emptied as an export empties it.
    time();
    adoc_pdf::typeset_text(&typst, &theme, &PdfOptions::default()).unwrap();
    let (cold, warm) = (time(), time());
    eprintln!("cold {cold:?}, warm {warm:?}");
    assert!(warm * 2 < cold, "cold {cold:?}, warm {warm:?}");
}
