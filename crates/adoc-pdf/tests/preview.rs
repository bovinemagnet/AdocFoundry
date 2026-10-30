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
