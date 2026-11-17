//! The AZW3 (KF8) writer: what it writes is a KF8 book, read back by readers that share no
//! code with it (`adoc_kindle::testing` and the `mobi` crate). Kindle PRD §12 tests 4, 5 and 8.

mod common;

use adoc_kindle::{
    testing::read_azw3, write_book, write_kindle, Asset, AssetKind, Block, KindleBook, KindleError,
    KindleFormat, KindleOptions,
};
use common::*;

const INSTALL: &str = "= Install\nAnn Author\n:description: Getting going.\n\nFirst you install it. 日本語のテキスト.\n\n[[setup]]\n== Set up\n\nSee xref:use.adoc#later[later in Use] and <<check>>.\n\n* one\n* two\n\n[[check]]\n== Check\n\n----\ncode here\n----\n";
const USE: &str = "= Use\n\n== Daily use\n\nText.footnote:[A note.]\n\n[[later]]\n== Later\n\n|===\n|a |b\n|===\n";

/// A 1×1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x90, 0x77, 0x53, 0xDE, 0, 0, 0, 0x0C, 0x49, 0x44, 0x41, 0x54,
    0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0, 0, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0, 0,
    0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn guide(folder: &std::path::Path) -> KindleBook {
    let document = book(
        folder,
        "Guide",
        &[("install.adoc", INSTALL), ("use.adoc", USE)],
    );
    let (mut book, _) = KindleBook::build(&document, &KindleOptions::default());
    // A picture, as K6 will embed them.
    book.assets.push(Asset {
        kind: AssetKind::Png,
        bytes: PNG.to_vec(),
        width: 1,
        height: 1,
    });
    book.chapters[0].blocks.push(Block::Figure {
        anchor: None,
        asset: 0,
        alt: "a red dot".to_owned(),
    });
    book
}

/// PRD §12 test 4: the file is KF8 in a PalmDB/MOBI file, not an EPUB, a ZIP or MOBI7 under
/// another name.
#[test]
fn azw3_is_a_kf8_container() {
    let folder = tempfile::tempdir().unwrap();
    let bytes = write_book(&guide(folder.path()), KindleFormat::Azw3).unwrap();
    assert!(!bytes.starts_with(b"PK"), "not a ZIP or EPUB");
    assert_eq!(&bytes[60..68], b"BOOKMOBI");
    let read = read_azw3(&bytes).unwrap();
    assert_eq!(read.version, 8, "a KF8 file, not MOBI7");
    // KF8 only: one MOBI header and no boundary record that a dual file has.
    assert_eq!(
        bytes.windows(4).filter(|window| window == b"BOOK").count(),
        1
    );
    assert!(
        read.exth.iter().all(|(kind, _)| *kind != 121),
        "no KF8 boundary"
    );
    assert_eq!(read.flows.len(), 2, "markup and stylesheet");

    // A reader written by someone else agrees.
    let other = mobi::Mobi::new(&bytes).unwrap();
    assert_eq!(other.title(), "Guide");
    let text = other.content_as_string_lossy();
    assert!(text.contains("First you install it."), "{text}");
}

/// PRD §12 test 5: an article and a book of several chapters read back with their metadata,
/// table of contents, anchors, text and a picture.
#[test]
fn azw3_reads_back_complete() {
    let folder = tempfile::tempdir().unwrap();
    let book = guide(folder.path());
    let read = read_azw3(&write_book(&book, KindleFormat::Azw3).unwrap()).unwrap();

    // Metadata.
    assert_eq!(read.title, "Guide");
    assert_eq!(read.exth_text(503), ["Guide"]);
    assert_eq!(read.exth_text(524), ["en"]);
    assert_eq!(
        read.exth_text(112),
        std::slice::from_ref(&book.metadata.identifier)
    );
    assert_eq!(read.exth_number(125), Some(1));

    // Two chapters, in order, with their text, Japanese included.
    assert_eq!(read.files.len(), 2);
    assert!(
        read.files[0].contains("<h1>Install</h1>"),
        "{}",
        read.files[0]
    );
    assert!(read.files[1].contains("<h1>Use</h1>"));
    let text = read.plain_text();
    assert!(text.contains("日本語のテキスト"), "{text}");
    assert!(text.contains("code here"));
    assert!(text.contains("A note."));

    // The table of contents: each chapter and its sections, nested, landing on its heading.
    let labels: Vec<(&str, u32)> = read
        .ncx
        .iter()
        .map(|entry| (entry.label.as_str(), entry.depth))
        .collect();
    assert_eq!(
        labels,
        [
            ("Install", 0),
            ("Set up", 1),
            ("Check", 1),
            ("Use", 0),
            ("Daily use", 1),
            ("Later", 1)
        ]
    );
    assert_eq!(read.ncx[1].parent, Some(0));
    for entry in &read.ncx {
        let (file, at) = read.position(entry.fid, entry.offset).unwrap();
        let rest = &read.files[file][at..];
        assert!(
            rest.starts_with("<h"),
            "{}: {}",
            entry.label,
            &rest[..rest.len().min(40)]
        );
        assert!(rest.contains(&entry.label), "{}", entry.label);
    }
    let (file, rest) = read
        .follow(&format!(
            "kindle:pos:fid:{:0>4}:off:{:0>10}",
            to_base32(read.ncx[2].fid),
            to_base32(read.ncx[2].offset)
        ))
        .unwrap();
    assert_eq!(file, 0);
    assert!(rest.starts_with("<h2 id=\"c1-a2\">Check</h2>"), "{rest}");

    // Every internal link lands on the start of the element it names.
    let internal: Vec<_> = read
        .hrefs()
        .into_iter()
        .filter(|(_, href)| href.starts_with("kindle:pos"))
        .collect();
    assert!(internal.len() >= 4, "{internal:?}");
    for (_, href) in &internal {
        let (_, rest) = read
            .follow(href)
            .unwrap_or_else(|| panic!("{href} leads nowhere"));
        assert!(
            rest.starts_with('<'),
            "{href}: {}",
            &rest[..rest.len().min(40)]
        );
        assert!(rest[..rest.find('>').unwrap()].contains(" id=\""), "{href}");
    }
    // The cross reference to the other chapter's `later` section lands there.
    let (file, rest) = read.follow(&internal[0].1).unwrap();
    assert_eq!(file, 1);
    assert!(rest.starts_with("<h2 id=\"c2-a2\">Later"), "{rest}");

    // The picture, as its bytes, named by the image record.
    assert_eq!(read.images, [PNG.to_vec()]);
    assert!(read.files[0].contains("src=\"kindle:embed:0001?mime=image/png\""));

    // An article reads back too.
    let article = article("= Note\n\nJust a paragraph.\n");
    let output = write_kindle(&article, KindleFormat::Azw3, &KindleOptions::default()).unwrap();
    let read = read_azw3(&output.bytes).unwrap();
    assert_eq!(read.title, "Note");
    assert_eq!(read.files.len(), 1);
    assert!(read.plain_text().contains("Just a paragraph."));
    assert_eq!(read.ncx.len(), 1);
}

fn to_base32(mut value: u32) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(value % 32) as usize]);
        value /= 32;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// PRD §12 test 8: each format is its own or a typed error; one is never written in place of
/// the other.
#[test]
fn kindle_never_substitutes_a_format() {
    let folder = tempfile::tempdir().unwrap();
    let book = guide(folder.path());
    for format in [KindleFormat::Azw3, KindleFormat::Kfx] {
        match write_book(&book, format) {
            Ok(bytes) => match format {
                KindleFormat::Azw3 => assert_eq!(&bytes[60..68], b"BOOKMOBI"),
                KindleFormat::Kfx => assert_eq!(&bytes[..4], b"CONT"),
            },
            Err(KindleError::Unsupported {
                format: refused, ..
            }) => assert_eq!(refused, format),
            Err(error) => panic!("{format}: {error}"),
        }
    }
}

/// A long book splits its text over many records and its chapters over many fragments, and
/// still reads back whole.
#[test]
fn long_chapters_split_into_records_and_fragments() {
    let mut text = String::from("= Long\n\n");
    for number in 0..400 {
        text.push_str(&format!(
            "[[p{number}]]\nParagraph {number} — ünïcödé text that runs on.\n\n"
        ));
    }
    text.push_str("See <<p399>>.\n");
    let output = write_kindle(
        &article(&text),
        KindleFormat::Azw3,
        &KindleOptions::default(),
    )
    .unwrap();
    let read = read_azw3(&output.bytes).unwrap();
    assert!(read.fragments.len() > 2, "{}", read.fragments.len());
    assert!(read.plain_text().contains("Paragraph 399"));
    let link = read
        .hrefs()
        .into_iter()
        .find(|(_, href)| href.starts_with("kindle:pos"))
        .unwrap();
    let (_, rest) = read.follow(&link.1).unwrap();
    assert!(rest.starts_with("<p id="), "{}", &rest[..40]);
    assert!(rest.contains("Paragraph 399"));
}
