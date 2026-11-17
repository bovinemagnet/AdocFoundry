//! Pictures, diagrams, formulas and the cover, embedded in both formats; bad ones reported
//! where they are written and never fetched (Kindle PRD §12 tests 12 and 13).

mod common;

use std::path::{Path, PathBuf};

use adoc_kindle::{
    testing::{read_azw3, read_kfx},
    write_kindle, KindleDocument, KindleFormat, KindleOptions, KindlePage,
};
use adoc_render::{DiagramView, FormulaImage, ViewInline, ViewKind};
use common::view;

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(path)
}

/// A page in `folder` with a cover, a PNG, a JPEG, a diagram, and a display and an inline
/// formula, the diagram and formulas drawn as the engine would have drawn them.
fn page(folder: &Path) -> KindleDocument {
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
    let text = "= Pictures\n:front-cover-image: image:cover.png[]\n\nimage::logo.png[The logo]\n\nimage::photo.jpg[A photograph]\n\n.The pipeline\n[mermaid]\n....\ngraph LR; a-->b\n....\n\n[stem]\n++++\nx^2\n++++\n\nInline stem:[y_1] and image:logo.png[the logo again] in text.\n";
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
    KindleDocument::Article(KindlePage {
        key: "p0".to_owned(),
        view,
        depth: 0,
    })
}

fn options(folder: &Path) -> KindleOptions {
    KindleOptions {
        access: adoc_core::FileAccess::within([folder.to_path_buf()]),
        ..KindleOptions::default()
    }
}

/// PRD §12 test 12: a cover, a PNG, a JPEG, a diagram and inline and display formulas are in
/// both files' bytes, with valid signatures, references, counts and the cover named.
#[test]
fn kindle_media_is_embedded() {
    let folder = tempfile::tempdir().unwrap();
    let document = page(folder.path());
    let png = std::fs::read(folder.path().join("logo.png")).unwrap();
    let jpeg = std::fs::read(folder.path().join("photo.jpg")).unwrap();
    let cover = std::fs::read(folder.path().join("cover.png")).unwrap();

    let azw3 = write_kindle(&document, KindleFormat::Azw3, &options(folder.path())).unwrap();
    assert!(azw3.diagnostics.is_empty(), "{:?}", azw3.diagnostics);
    let read = read_azw3(&azw3.bytes).unwrap();
    // The logo once, though shown twice; the photo; the drawn diagram and formula (one
    // drawing for both formulas, which are the same file here); the cover.
    assert_eq!(read.images.len(), 5, "{}", read.images.len());
    assert!(
        read.images.contains(&png) && read.images.contains(&jpeg) && read.images.contains(&cover)
    );
    let drawn: Vec<&Vec<u8>> = read
        .images
        .iter()
        .filter(|image| ![&png, &jpeg, &cover].contains(image))
        .collect();
    assert_eq!(drawn.len(), 2);
    assert!(drawn
        .iter()
        .all(|image| image.starts_with(b"\x89PNG\r\n\x1a\n")));
    assert_eq!(read.exth_number(125), Some(5));
    let cover_index = read.exth_number(201).unwrap() as usize;
    assert_eq!(read.images[cover_index], cover);
    // Every picture referenced is one the file holds, and each signature is whole.
    let references: Vec<_> = read
        .files
        .iter()
        .flat_map(|file| {
            file.match_indices("kindle:embed:")
                .map(move |(at, _)| &file[at + 13..at + 17])
        })
        .collect();
    assert_eq!(references.len(), 6, "{references:?}");
    for reference in references {
        let index = u32::from_str_radix(reference, 32).unwrap() as usize;
        assert!(index >= 1 && index <= read.images.len(), "{reference}");
    }
    let text = read.plain_text();
    assert!(
        !text.contains("x^2"),
        "the formula is a picture, not its source"
    );

    let kfx = write_kindle(&document, KindleFormat::Kfx, &options(folder.path())).unwrap();
    let read = read_kfx(&kfx.bytes).unwrap();
    assert_eq!(read.of_kind(417).count(), 5);
    assert_eq!(read.of_kind(164).count(), 5);
    let cover_name = read.metadata("cover_image").unwrap();
    let cover_resource = read
        .of_kind(164)
        .find(|entity| read.name(entity.id) == cover_name)
        .unwrap();
    assert_eq!(read.picture(cover_resource.id), Some(cover.as_slice()));
    let chapters = read.chapters().unwrap();
    let pictures: Vec<_> = chapters[0]
        .iter()
        .filter(|block| block.kind == "image")
        .map(|block| {
            (
                block.alt.clone().unwrap_or_default(),
                read.picture(block.resource.unwrap()).unwrap().to_vec(),
            )
        })
        .collect();
    assert_eq!(pictures.len(), 4, "logo, photo, diagram, formula");
    assert_eq!(pictures[0], ("The logo".to_owned(), png));
    assert_eq!(pictures[1], ("A photograph".to_owned(), jpeg));
    assert_eq!(pictures[2].0, "The pipeline");
    assert_eq!(pictures[3].0, "x^2");
}

/// PRD §12 test 13: oversized, malformed, missing and remote pictures each give a warning
/// where they are written and their description in the text; nothing is fetched and nothing
/// is dropped silently.
#[test]
fn kindle_bad_assets_are_reported() {
    let folder = tempfile::tempdir().unwrap();
    // A PNG whose header claims 20,000 pixels a side, and one cut short.
    let mut huge = std::fs::read(fixture("docx/themes/brand-logo.png")).unwrap();
    huge[16..20].copy_from_slice(&20_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&20_000u32.to_be_bytes());
    std::fs::write(folder.path().join("huge.png"), huge).unwrap();
    let whole = std::fs::read(fixture("docx/themes/brand-logo.png")).unwrap();
    std::fs::write(folder.path().join("broken.png"), &whole[..whole.len() / 2]).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.png"), &whole).unwrap();
    let secret = outside.path().join("secret.png");
    let text = format!(
        "= Bad\n\nimage::huge.png[Huge]\n\nimage::broken.png[Broken]\n\nimage::missing.png[Missing]\n\nimage::https://example.com/remote.png[Remote]\n\nimage::{}[Secret]\n",
        secret.display()
    );
    let document = KindleDocument::Article(KindlePage {
        key: "p0".to_owned(),
        view: view(&folder.path().join("bad.adoc"), &text),
        depth: 0,
    });
    for format in [KindleFormat::Azw3, KindleFormat::Kfx] {
        let output = write_kindle(&document, format, &options(folder.path())).unwrap();
        let warning = |needle: &str| {
            output
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.contains(needle))
                .unwrap_or_else(|| panic!("no warning about {needle}: {:?}", output.diagnostics))
                .source
                .as_ref()
                .map(|span| span.line)
        };
        assert_eq!(warning("huge.png"), Some(3));
        assert_eq!(warning("broken.png"), Some(5));
        assert_eq!(warning("missing.png"), Some(7));
        assert_eq!(warning("https://example.com/remote.png"), Some(9));
        assert_eq!(warning("secret.png"), Some(11));
        let (pictures, text) = match format {
            KindleFormat::Azw3 => {
                let read = read_azw3(&output.bytes).unwrap();
                (read.images.len(), read.plain_text())
            }
            KindleFormat::Kfx => {
                let read = read_kfx(&output.bytes).unwrap();
                let text = read
                    .chapters()
                    .unwrap()
                    .iter()
                    .flatten()
                    .map(|block| block.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                (read.of_kind(417).count(), text)
            }
        };
        assert_eq!(pictures, 0, "{format}: nothing embedded, nothing fetched");
        for alt in ["Huge", "Broken", "Missing", "Remote", "Secret"] {
            assert!(text.contains(alt), "{format}: {alt} in {text}");
        }
    }
}
