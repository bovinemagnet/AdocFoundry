//! EPUB 5/9 (#642): pictures, diagrams, formulas and the cover embedded; bad ones reported
//! where they are written and never fetched (PRD §11 tests 12 and 13).

use std::path::{Path, PathBuf};

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_epub::{
    testing, write_epub, BookOptions, Document, EpubOptions, Modified, Page, Severity,
};
use adoc_render::{DiagramView, FormulaImage, ViewInline, ViewKind};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(path)
}

fn view(path: &Path, text: &str) -> adoc_render::ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

fn options(folder: &Path) -> EpubOptions {
    EpubOptions {
        book: BookOptions {
            access: adoc_core::FileAccess::within([folder.to_path_buf()]),
            ..BookOptions::default()
        },
        ..EpubOptions::new(Modified::from_unix_seconds(1_795_084_200).unwrap())
    }
}

/// A page in `folder` with a cover, a PNG shown twice, a JPEG, a diagram, and a display and an
/// inline formula, the diagram and formulas drawn as the engine would have drawn them.
fn page(folder: &Path) -> Document {
    for (from, to) in [
        ("docx/themes/brand-cover.png", "cover.png"),
        ("docx/themes/brand-logo.png", "logo.png"),
        (
            "docx/reference/manual/modules/ROOT/images/photo.jpg",
            "photo.jpg",
        ),
        ("pdf/reference/images/pipeline.svg", "pipeline.svg"),
        ("pdf/themes/images/logo.svg", "formula.svg"),
    ] {
        std::fs::copy(fixture(from), folder.join(to)).unwrap();
    }
    let text = "= Pictures\n:front-cover-image: image:cover.png[]\n:lang: en-GB\n\nimage::logo.png[The logo]\n\n\
                image::photo.jpg[A photograph]\n\n.The pipeline\n[mermaid]\n....\ngraph LR; a-->b\n....\n\n\
                [stem]\n++++\nx^2\n++++\n\nInline stem:[y_1] and image:logo.png[the logo again] in text.\n";
    let mut view = view(&folder.join("page.adoc"), text);
    let svg = folder.join("pipeline.svg");
    let formula = folder.join("formula.svg");
    for block in &mut view.blocks {
        match &mut block.kind {
            ViewKind::Diagram(diagram) => *diagram = DiagramView::Svg(svg.clone()),
            ViewKind::Stem { rendered, .. } => *rendered = DiagramView::Svg(formula.clone()),
            ViewKind::Paragraph(inlines) => {
                for inline in inlines {
                    if let ViewInline::Stem { rendered, .. } = inline {
                        *rendered = Some(FormulaImage {
                            path: formula.clone(),
                            width: 10.0,
                            height: 10.0,
                            baseline: 8.0,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    Document::Article(Page {
        key: "p0".to_owned(),
        view,
        depth: 0,
    })
}

/// PRD §11 test 12.
#[test]
fn epub_media_is_embedded() {
    let folder = tempfile::tempdir().unwrap();
    let output = write_epub(&page(folder.path()), &options(folder.path())).unwrap();
    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    testing::assert_links_resolve(&output.bytes);

    // Each picture once, with its media type; the PNG and JPEG as they were.
    let images: Vec<(&String, &String, &Option<String>)> = package
        .manifest
        .values()
        .filter(|(_, media, _)| media.starts_with("image/"))
        .map(|(path, media, properties)| (path, media, properties))
        .collect();
    let read = |name: &str| std::fs::read(folder.path().join(name)).unwrap();
    let (png, jpeg, cover) = (read("logo.png"), read("photo.jpg"), read("cover.png"));
    let held = |bytes: &[u8]| {
        images
            .iter()
            .filter(|(path, _, _)| files[*path] == bytes)
            .count()
    };
    assert_eq!(held(&png), 1, "the logo shown twice is held once");
    assert_eq!(held(&jpeg), 1);
    assert_eq!(held(&cover), 1);
    for (path, media, _) in &images {
        let expected = if path.ends_with(".png") {
            "image/png"
        } else if path.ends_with(".jpg") {
            "image/jpeg"
        } else {
            panic!("{path} is neither PNG nor JPEG");
        };
        assert_eq!(*media, expected, "{path}");
        assert!(!path.ends_with(".svg"), "SVG is drawn, never packed");
    }
    // The diagram and the formula drawing are pictures of their own: the cover, the logo, the
    // photograph, the diagram and the formula (both formulas being one file here).
    assert_eq!(images.len(), 5, "{images:?}");
    let cover_items: Vec<&&String> = images
        .iter()
        .filter(|(_, _, properties)| properties.as_deref() == Some("cover-image"))
        .map(|(path, _, _)| path)
        .collect();
    assert_eq!(cover_items.len(), 1);
    assert_eq!(files[*cover_items[0]], cover);

    // The cover document comes first, with its landmark.
    assert_eq!(package.spine.first().map(String::as_str), Some("cover"));
    let nav = testing::text(&files, &package.nav);
    assert!(
        nav.contains("epub:type=\"cover\" href=\"cover.xhtml\""),
        "{nav}"
    );
    let cover_page = testing::text(&files, "EPUB/cover.xhtml");
    assert!(cover_page.contains("<img src=\"images/"), "{cover_page}");

    // In the chapter: figures and inline pictures with their descriptions, sized by style,
    // never in pixels; a formula's description is its source.
    let chapter = testing::text(&files, "EPUB/text/c1.xhtml");
    let document = testing::parse("c1", chapter);
    let pictures: Vec<(String, Option<String>, bool)> = document
        .descendants()
        .filter(|node| node.has_tag_name((testing::XHTML, "img")))
        .map(|img| {
            (
                img.attribute("alt").unwrap().to_owned(),
                img.attribute("class").map(str::to_owned),
                img.attribute("width").is_some() || img.attribute("height").is_some(),
            )
        })
        .collect();
    let alts: Vec<&str> = pictures.iter().map(|(alt, _, _)| alt.as_str()).collect();
    for alt in [
        "The logo",
        "A photograph",
        "The pipeline",
        "x^2",
        "y_1",
        "the logo again",
    ] {
        assert!(alts.contains(&alt), "{alt} in {alts:?}");
    }
    assert!(pictures.iter().all(|(_, _, sized)| !sized));
    assert!(pictures
        .iter()
        .any(|(alt, class, _)| alt == "y_1" && class.as_deref() == Some("inline")));
    assert!(
        document
            .descendants()
            .filter(|node| node.has_tag_name((testing::XHTML, "figure")))
            .count()
            >= 4
    );
    // No warning: everything was embedded.
    assert!(
        output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Warning),
        "{:?}",
        output.diagnostics
    );
}

/// PRD §11 test 13: oversized, malformed, missing, remote, outside and scripted pictures each
/// give a warning where they are written, or are drawn safely; nothing is fetched, nothing is
/// dropped silently, and the book depends on no local path.
#[test]
fn epub_bad_assets_are_reported() {
    let folder = tempfile::tempdir().unwrap();
    // A PNG whose header claims 20,000 pixels a side, one cut short, and text named as a PNG.
    let mut huge = std::fs::read(fixture("docx/themes/brand-logo.png")).unwrap();
    huge[16..20].copy_from_slice(&20_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&20_000u32.to_be_bytes());
    std::fs::write(folder.path().join("huge.png"), huge).unwrap();
    let whole = std::fs::read(fixture("docx/themes/brand-logo.png")).unwrap();
    std::fs::write(folder.path().join("broken.png"), &whole[..whole.len() / 2]).unwrap();
    std::fs::write(folder.path().join("text.png"), "not a picture").unwrap();
    // An SVG with a script and a link to a file outside it: drawn, never packed.
    std::fs::write(
        folder.path().join("scripted.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="10"><script>alert(1)</script><image xlink:href="file:///etc/passwd" width="5" height="5"/><rect width="20" height="10" fill="red"/></svg>"#,
    )
    .unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.png"), &whole).unwrap();
    let secret = outside.path().join("secret.png");
    let text = format!(
        "= Bad\n\nimage::huge.png[Huge]\n\nimage::broken.png[Broken]\n\nimage::missing.png[Missing]\n\n\
         image::https://example.com/remote.png[Remote]\n\nimage::{}[Secret]\n\nimage::text.png[Text]\n\n\
         image::scripted.svg[Scripted]\n",
        secret.display()
    );
    let document = Document::Article(Page {
        key: "p0".to_owned(),
        view: view(&folder.path().join("bad.adoc"), &text),
        depth: 0,
    });
    let output = write_epub(&document, &options(folder.path())).unwrap();
    let warning = |needle: &str| {
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains(needle))
            .unwrap_or_else(|| panic!("no warning about {needle}: {:?}", output.diagnostics));
        (
            diagnostic.message.clone(),
            diagnostic.source.as_ref().map(|span| span.line),
        )
    };
    assert_eq!(warning("huge.png").1, Some(3));
    assert_eq!(warning("broken.png").1, Some(5));
    assert_eq!(warning("missing.png").1, Some(7));
    assert_eq!(warning("https://example.com/remote.png").1, Some(9));
    assert_eq!(warning("secret.png").1, Some(11));
    let (message, line) = warning("text.png");
    assert_eq!(line, Some(13));
    assert!(message.contains("an EPUB reader"), "{message}");

    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    let links = testing::assert_links_resolve(&output.bytes);
    // Only the scripted SVG is a picture of the book, drawn as a PNG.
    let images: Vec<&String> = package
        .manifest
        .values()
        .filter(|(_, media, _)| media.starts_with("image/"))
        .map(|(path, _, _)| path)
        .collect();
    assert_eq!(images.len(), 1, "{images:?}");
    assert!(images[0].ends_with(".png"));
    assert!(files[images[0]].starts_with(b"\x89PNG"));
    // The rest are their descriptions; the remote one a link to it, never fetched.
    let chapter = testing::text(&files, "EPUB/text/c1.xhtml");
    for alt in ["Huge", "Broken", "Missing", "Secret", "Text"] {
        assert!(chapter.contains(alt), "{alt} in {chapter}");
    }
    assert!(links
        .iter()
        .any(|link| link.href == "https://example.com/remote.png" && link.text == "Remote"));
    // Nothing in the book names a script, a local file or the folder the pictures are in.
    for (name, data) in &files {
        if name.ends_with(".xhtml") || name.ends_with(".opf") {
            let text = String::from_utf8_lossy(data);
            for needle in ["<script", "file:", folder.path().to_string_lossy().as_ref()] {
                assert!(!text.contains(needle), "{name} holds {needle}");
            }
        }
    }
}
