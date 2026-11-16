//! Images, diagrams and formulas embedded in the package: PRD §10, tests 9
//! (`docx_media_is_embedded`) and 10 (`docx_bad_images_are_reported`).

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use adoc_docx::{write_docx, DocxDocument, DocxOptions, DocxPage, DocxReport, DocxSeverity};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_render::{DiagramView, FormulaImage, LocalResolver, ResolvedLink, ViewResolver};
use common::{assert_valid_package, text, PACKAGE_RELATIONSHIPS, R, W};
use roxmltree::Document;

const IMAGES: &str = "../../tests/fixtures/image-viewer/modules/ROOT/images";
const IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
/// English Metric Units in a pixel at 96 to the inch.
const EMU_PER_PIXEL: i64 = 9525;
const EMU_PER_POINT: i64 = 12700;

/// The fixtures' images copied to a folder of their own.
fn images(names: &[&str]) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    for name in names {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(IMAGES)
                .join(name),
            folder.path().join(name),
        )
        .unwrap();
    }
    folder
}

/// Resolves as `LocalResolver` does, with every diagram and formula already rendered to
/// `rendered`, as the engine's are by the time it exports.
struct Rendered {
    svg: PathBuf,
}

impl ViewResolver for Rendered {
    fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
        LocalResolver.xref(from, target)
    }

    fn image(&self, from: &Path, target: &str, imagesdir: Option<&str>) -> Option<PathBuf> {
        LocalResolver.image(from, target, imagesdir)
    }

    fn diagram(&self, _: &str, _: &str, _: &BTreeMap<String, String>) -> DiagramView {
        DiagramView::Svg(self.svg.clone())
    }

    fn formula(&self, _: bool, _: &str) -> Option<FormulaImage> {
        Some(FormulaImage {
            path: self.svg.clone(),
            width: 30.0,
            height: 12.0,
            baseline: 9.0,
        })
    }
}

fn export(folder: &Path, text: &str, access: adoc_core::FileAccess) -> (Vec<u8>, DocxReport) {
    let path = folder.join("page.adoc");
    fs::write(&path, text).unwrap();
    let dom = AcdcParser.parse(&ParseInput::new(&path, text));
    let resolver = Rendered {
        svg: folder.join("architecture.svg"),
    };
    let view = adoc_render::build_view(&path, &dom, &resolver);
    let document = DocxDocument::Article(DocxPage {
        key: "p0".to_owned(),
        view,
        depth: 0,
    });
    let options = DocxOptions {
        access,
        ..DocxOptions::default()
    };
    let mut bytes = Vec::new();
    let report = write_docx(&document, &options, &mut bytes).unwrap();
    (bytes, report)
}

/// Each picture of the document: its alt text, its size and the media part it shows.
fn pictures(entries: &BTreeMap<String, Vec<u8>>) -> Vec<(String, (i64, i64), String)> {
    let document = Document::parse(text(entries, "word/document.xml")).unwrap();
    let relationships = Document::parse(text(entries, "word/_rels/document.xml.rels")).unwrap();
    let targets: BTreeMap<&str, (&str, &str)> = relationships
        .descendants()
        .filter(|node| node.has_tag_name((PACKAGE_RELATIONSHIPS, "Relationship")))
        .map(|node| {
            (
                node.attribute("Id").unwrap(),
                (
                    node.attribute("Type").unwrap(),
                    node.attribute("Target").unwrap(),
                ),
            )
        })
        .collect();
    document
        .descendants()
        .filter(|node| node.tag_name().name() == "inline")
        .map(|inline| {
            let extent = inline
                .children()
                .find(|node| node.tag_name().name() == "extent")
                .unwrap();
            let size = (
                extent.attribute("cx").unwrap().parse().unwrap(),
                extent.attribute("cy").unwrap().parse().unwrap(),
            );
            let alt = inline
                .children()
                .find(|node| node.tag_name().name() == "docPr")
                .and_then(|node| node.attribute("descr"))
                .unwrap()
                .to_owned();
            let blip = inline
                .descendants()
                .find(|node| node.tag_name().name() == "blip")
                .unwrap();
            let (kind, target) = targets[blip.attribute((R, "embed")).unwrap()];
            assert_eq!(kind, IMAGE);
            (alt, size, format!("word/{target}"))
        })
        .collect()
}

/// A PNG's width and height in pixels.
fn png_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    (
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
    )
}

#[test]
fn docx_media_is_embedded() {
    let folder = images(&[
        "logo.png",
        "screenshot.jpg",
        "still.gif",
        "architecture.svg",
    ]);
    let page = "= Pictures\n\n\
                image::logo.png[A logo,width=120]\n\n\
                image::screenshot.jpg[A screenshot]\n\n\
                A dot image:still.gif[a dot] in a line, and the logo again image:logo.png[the logo].\n\n\
                image::architecture.svg[The architecture]\n\n\
                [mermaid]\n----\ngraph TD\n  A --> B\n----\n\n\
                A formula stem:[sqrt(4)] in a line.\n\n\
                [stem]\n++++\nx^2\n++++\n";
    let (bytes, report) = export(folder.path(), page, adoc_core::FileAccess::Any);
    assert_eq!(report.diagnostics, Vec::new());
    let entries = assert_valid_package(&bytes);

    let found = pictures(&entries);
    let alts: Vec<&str> = found.iter().map(|(alt, _, _)| alt.as_str()).collect();
    assert_eq!(
        alts,
        [
            "A logo",
            "A screenshot",
            "a dot",
            "the logo",
            "The architecture",
            "diagram",
            "sqrt(4)",
            "x^2"
        ]
    );
    let by_alt: BTreeMap<&str, ((i64, i64), &str)> = found
        .iter()
        .map(|(alt, size, part)| (alt.as_str(), (*size, part.as_str())))
        .collect();
    // A width in pixels, the height following the image's proportions (24 × 16).
    assert_eq!(
        by_alt["A logo"].0,
        (120 * EMU_PER_PIXEL, 80 * EMU_PER_PIXEL)
    );
    // No size: the image's own, 32 × 20 pixels.
    assert_eq!(
        by_alt["A screenshot"].0,
        (32 * EMU_PER_PIXEL, 20 * EMU_PER_PIXEL)
    );
    // The same file twice is one part.
    assert_eq!(by_alt["A logo"].1, by_alt["the logo"].1);
    // A formula takes its typeset measures, in points.
    assert_eq!(
        by_alt["sqrt(4)"].0,
        (30 * EMU_PER_POINT, 12 * EMU_PER_POINT)
    );

    // Every picture is a part of the package, of its own content type: the SVG drawn as a PNG
    // at twice its size.
    let types = Document::parse(text(&entries, "[Content_Types].xml")).unwrap();
    let type_of = |part: &str| {
        types
            .descendants()
            .find(|node| node.attribute("PartName") == Some(&*format!("/{part}")))
            .and_then(|node| node.attribute("ContentType"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(type_of(by_alt["A logo"].1), "image/png");
    assert_eq!(type_of(by_alt["A screenshot"].1), "image/jpeg");
    assert_eq!(type_of(by_alt["a dot"].1), "image/gif");
    let svg = by_alt["The architecture"].1;
    assert_eq!(type_of(svg), "image/png");
    assert_eq!(png_size(&entries[svg]), (240, 120));
    assert_eq!(
        by_alt["The architecture"].0,
        (120 * EMU_PER_PIXEL, 60 * EMU_PER_PIXEL)
    );
    let media: BTreeSet<&str> = entries
        .keys()
        .map(String::as_str)
        .filter(|name| name.starts_with("word/media/"))
        .collect();
    assert_eq!(media.len(), 4, "{media:?}");
    // The bytes are the files' own.
    assert_eq!(
        entries[by_alt["A screenshot"].1],
        fs::read(folder.path().join("screenshot.jpg")).unwrap()
    );

    // Nothing refers to the files the images came from: removing them changes nothing.
    let folder_name = folder.path().to_string_lossy().into_owned();
    for (name, data) in &entries {
        let text = String::from_utf8_lossy(data);
        assert!(
            !text.contains(&folder_name),
            "{name} names the source folder"
        );
        assert!(
            !text.contains("TargetMode=\"External\""),
            "{name}: an external picture"
        );
    }
    drop(folder);
    assert_valid_package(&bytes);

    // An inline formula sits on the line's baseline: its run is lowered by its depth.
    let document = Document::parse(text(&entries, "word/document.xml")).unwrap();
    let formula = document
        .descendants()
        .find(|node| {
            node.tag_name().name() == "docPr" && node.attribute("descr") == Some("sqrt(4)")
        })
        .unwrap();
    let run = formula
        .ancestors()
        .find(|node| node.has_tag_name((W, "r")))
        .unwrap();
    let position = run
        .descendants()
        .find(|node| node.has_tag_name((W, "position")))
        .and_then(|node| node.attribute((W, "val")));
    // 12 pt tall, its baseline 9 pt down: 3 pt below the line, in half-points.
    assert_eq!(position, Some("-6"));
}

#[test]
fn docx_bad_images_are_reported() {
    let folder = images(&[
        "notes.png",
        "broken.png",
        "still.webp",
        "photo.heic",
        "logo.png",
    ]);
    // A PNG that says it is 30,000 pixels wide.
    let mut huge = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    huge.extend_from_slice(&30_000u32.to_be_bytes());
    huge.extend_from_slice(&30_000u32.to_be_bytes());
    huge.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    huge.extend_from_slice(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82");
    fs::write(folder.path().join("huge.png"), huge).unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::copy(
        folder.path().join("logo.png"),
        outside.path().join("outside.png"),
    )
    .unwrap();
    let outside_path = outside.path().join("outside.png");
    let page = format!(
        "= Bad pictures\n\n\
         image::missing.png[Missing]\n\n\
         image::notes.png[Text named a PNG]\n\n\
         image::broken.png[Cut short]\n\n\
         image::still.webp[A WebP]\n\n\
         image::photo.heic[A HEIC]\n\n\
         image::huge.png[Too big]\n\n\
         image::https://example.com/remote.png[Remote]\n\n\
         image::{}[Outside]\n",
        outside_path.display()
    );
    let access = adoc_core::FileAccess::within([folder.path().to_path_buf()]);
    let (bytes, report) = export(folder.path(), &page, access);
    let entries = assert_valid_package(&bytes);
    assert!(pictures(&entries).is_empty(), "no picture is embedded");
    assert!(!entries.keys().any(|name| name.starts_with("word/media/")));

    // Each is shown by its text, and reported where it is.
    let texts: Vec<String> = common::paragraphs(text(&entries, "word/document.xml"))
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    for alt in [
        "Missing",
        "Text named a PNG",
        "Cut short",
        "A WebP",
        "A HEIC",
        "Too big",
        "Remote",
        "Outside",
    ] {
        assert!(
            texts.iter().any(|text| text.contains(alt)),
            "{alt}: {texts:?}"
        );
    }
    // The remote image is a link to its address, never fetched.
    assert!(text(&entries, "word/_rels/document.xml.rels")
        .contains(r#"Target="https://example.com/remote.png" TargetMode="External""#));
    let warnings: Vec<(DocxSeverity, u32, String)> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.severity,
                diagnostic.source.as_ref().unwrap().line,
                diagnostic.message.clone(),
            )
        })
        .collect();
    let at = |line: u32, message: &str| (DocxSeverity::Warning, line, message.to_owned());
    assert_eq!(
        warnings,
        [
            at(3, "the image missing.png cannot be found and is exported as its alternative text"),
            at(5, "the image notes.png is not an image Word shows (PNG, JPEG, GIF or SVG) and is exported as its alternative text"),
            at(7, "the image broken.png is cut short or damaged and is exported as its alternative text"),
            at(9, "the image still.webp is not an image Word shows (PNG, JPEG, GIF or SVG) and is exported as its alternative text"),
            at(11, "the image photo.heic is not an image Word shows (PNG, JPEG, GIF or SVG) and is exported as its alternative text"),
            at(13, "the image huge.png is 30000 × 30000 pixels where 10000 on a side is the most and is exported as its alternative text"),
            at(15, "the remote image https://example.com/remote.png is not downloaded and is exported as a link to it"),
            at(17, &format!("the image {} cannot be read: it is outside the folders the export may read; it is exported as its alternative text", outside_path.display())),
        ]
    );
}

/// Writes a page of pictures to `$ADOC_DOCX_OUT/media.docx`, for opening in Word or LibreOffice
/// by hand: `ADOC_DOCX_OUT=/tmp cargo test -p adoc-docx --test media -- --ignored`.
#[test]
#[ignore = "writes a file for a manual check"]
fn write_sample() {
    let out = std::env::var_os("ADOC_DOCX_OUT").expect("ADOC_DOCX_OUT names a folder");
    let folder = images(&[
        "logo.png",
        "screenshot.jpg",
        "still.gif",
        "architecture.svg",
    ]);
    let page = "= Pictures\n\nimage::logo.png[A logo,width=120]\n\n\
                .A screenshot\nimage::screenshot.jpg[A screenshot,align=center]\n\n\
                A dot image:still.gif[a dot] in a line.\n\n\
                image::architecture.svg[The architecture]\n\n\
                A formula stem:[sqrt(4)] in a line.\n";
    let (bytes, _) = export(folder.path(), page, adoc_core::FileAccess::Any);
    fs::write(Path::new(&out).join("media.docx"), bytes).unwrap();
}

/// A picture is shrunk to fit the page's text both ways, keeping its proportions: no wider than
/// the text, and no taller than the text's height less a line for its caption. A tall diagram
/// scaled to the text's width alone ran off the page.
#[test]
fn pictures_fit_the_page() {
    let folder = tempfile::tempdir().unwrap();
    let svg = |width: u32, height: u32| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><rect width=\"{width}\" height=\"{height}\" fill=\"#36c\"/></svg>"
        )
    };
    fs::write(folder.path().join("tall.svg"), svg(100, 3000)).unwrap();
    fs::write(folder.path().join("big.svg"), svg(2000, 4000)).unwrap();
    fs::write(folder.path().join("architecture.svg"), svg(10, 10)).unwrap();
    let page = "= Pictures\n\n.Tall\nimage::tall.svg[Tall]\n\nimage::big.svg[Big]\n\nimage::tall.svg[Asked for,width=300]\n";
    let (bytes, report) = export(folder.path(), page, adoc_core::FileAccess::Any);
    assert_eq!(report.diagnostics, Vec::new());
    let entries = assert_valid_package(&bytes);
    let found = pictures(&entries);

    // A4 with an inch of margin: 9,026 by 13,958 twentieths of a point of text, less 720 for a
    // caption's line.
    let (text_width, text_height) = (9026 * 635, (13958 - 720) * 635);
    for (alt, (width, height), _) in &found {
        assert!(*width <= text_width, "{alt} is {width} wide");
        assert!(*height <= text_height, "{alt} is {height} tall");
    }
    let size = |alt: &str| found.iter().find(|(found, _, _)| found == alt).unwrap().1;
    let ratio = |(width, height): (i64, i64)| width as f64 / height as f64;
    for (alt, own) in [
        ("Tall", 100.0 / 3000.0),
        ("Big", 0.5),
        ("Asked for", 100.0 / 3000.0),
    ] {
        let size = size(alt);
        assert_eq!(size.1, text_height, "{alt} fills the page's height");
        assert!(
            (ratio(size) - own).abs() < 0.001,
            "{alt} keeps its proportions"
        );
    }
}
