//! The KFX (KF10) writer: what it writes is a KFX container whose every reference resolves,
//! read back by a reader that shares no code with it (`adoc_kindle::testing::read_kfx`).
//! Kindle PRD §12 tests 6 and 7.

mod common;

use std::collections::HashSet;

use adoc_kindle::{
    testing::{read_kfx, Ion, Kfx},
    write_book, Asset, AssetKind, Block, KindleBook, KindleFormat, KindleOptions,
};
use common::*;

const INSTALL: &str = "= Install\nAnn Author\n:description: Getting going.\n:lang: en-GB\n\nFirst you *install* it, from https://example.com/get[the site]. 日本語のテキスト.\n\n[[setup]]\n== Set up\n\nSee xref:use.adoc#later[later in Use] and <<check>>.\n\n* one\n* two\n\n[[check]]\n== Check\n\n----\ncode here\n----\n";
const USE: &str = "= Use\n\n== Daily use\n\nText.footnote:[A note.]\n\n[[later]]\n== Later\n\n|===\n|a |b\n|===\n";

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
    book.cover = Some(0);
    // A book's metadata is its own, not its first page's.
    book.metadata.authors = vec!["Ann Author".to_owned()];
    book.metadata.language = "en-GB".to_owned();
    book.metadata.description = Some("Getting going.".to_owned());
    book
}

fn written(folder: &std::path::Path) -> (Vec<u8>, Kfx) {
    let bytes = write_book(&guide(folder), KindleFormat::Kfx).unwrap();
    let read = read_kfx(&bytes).unwrap();
    (bytes, read)
}

/// Every symbol a value names in a field that refers to another entity.
fn references(value: &Ion, out: &mut Vec<(u32, u32)>) {
    match value {
        Ion::Struct(fields) => {
            for (name, field) in fields {
                // Field → the type of entity it names.
                let kind = match name {
                    174 => Some(260),
                    176 => Some(259),
                    175 => Some(164),
                    179 => Some(266),
                    157 => Some(157),
                    _ => None,
                };
                if let (Some(kind), Some(symbol)) = (kind, field.symbol()) {
                    out.push((kind, symbol));
                }
                references(field, out);
            }
        }
        Ion::List(items) => items.iter().for_each(|item| references(item, out)),
        Ion::Annotated(_, inner) => references(inner, out),
        _ => {}
    }
}

/// PRD §12 test 6: the file has a KFX signature and container, and every fragment reference
/// resolves.
#[test]
fn kfx_is_a_kfx_container() {
    let folder = tempfile::tempdir().unwrap();
    let (bytes, read) = written(folder.path());
    assert_eq!(&bytes[..4], b"CONT");
    assert!(!bytes.starts_with(b"PK"), "not a ZIP or EPUB");
    assert!(
        bytes.windows(8).all(|window| window != b"BOOKMOBI"),
        "not AZW3"
    );
    assert_eq!(read.version, 2);
    // Local symbols begin where the shared table ends, as Ion counts them: nine system
    // symbols, then the import's.
    assert_eq!(read.shared_max_id, 842);
    assert!(read.symbols.iter().any(|name| name == "c0"));

    // The book's fragments are all there.
    for kind in [
        585, 490, 258, 538, 389, 260, 259, 145, 164, 417, 266, 157, 264, 265, 550, 419,
    ] {
        assert!(read.of_kind(kind).next().is_some(), "no ${kind}");
    }
    // Named entities are local symbols and never repeat.
    let mut seen = HashSet::new();
    for entity in read.entities.iter().filter(|entity| entity.id != 348) {
        assert!(entity.id >= 852, "{}", entity.id);
        assert!(
            seen.insert((entity.kind, entity.id)),
            "{} twice",
            read.name(entity.id)
        );
    }
    // Every reference names an entity of the right type.
    let mut wanted = Vec::new();
    for entity in &read.entities {
        if let Some(value) = &entity.value {
            references(value, &mut wanted);
        }
    }
    assert!(wanted.len() > 10, "{wanted:?}");
    for (kind, symbol) in &wanted {
        assert!(
            read.entity(*kind, *symbol).is_some(),
            "${kind} {} is named but missing",
            read.name(*symbol)
        );
    }
    // Every content reference is in range, every position names a block.
    let chapters = read.chapters().unwrap();
    let mut eids = HashSet::new();
    for block in chapters.iter().flatten() {
        eids.insert(block.eid);
        for cell in block.rows.iter().flatten() {
            eids.insert(cell.eid);
        }
        if block.kind == "text" {
            assert!(!block.text.is_empty() || block.links.is_empty());
        }
    }
    for entity in read.of_kind(266) {
        if let Some(Ok((eid, _))) = read.anchor(entity.id) {
            assert!(
                eids.contains(&eid),
                "anchor {} leads to no block",
                read.name(entity.id)
            );
        }
    }
    for (label, _, eid, _) in read.toc() {
        assert!(eids.contains(&eid), "{label} leads to no block");
    }
    // The container map names every named entity.
    let map = read.of_kind(419).next().unwrap().value.as_ref().unwrap();
    let listed: HashSet<u32> = map.field(252).unwrap().list()[0]
        .field(181)
        .unwrap()
        .list()
        .iter()
        .filter_map(Ion::symbol)
        .collect();
    for entity in read.entities.iter().filter(|entity| entity.id != 348) {
        assert!(
            listed.contains(&entity.id),
            "{} unlisted",
            read.name(entity.id)
        );
    }
}

/// PRD §12 test 7: chapter order, table of contents, metadata, hyperlinks and pictures read
/// back.
#[test]
fn kfx_reads_back_complete() {
    let folder = tempfile::tempdir().unwrap();
    let (_, read) = written(folder.path());

    assert_eq!(read.metadata("title").as_deref(), Some("Guide"));
    assert_eq!(read.metadata("author").as_deref(), Some("Ann Author"));
    assert_eq!(read.metadata("language").as_deref(), Some("en-GB"));
    assert_eq!(
        read.metadata("description").as_deref(),
        Some("Getting going.")
    );
    assert_eq!(read.metadata("cover_image").as_deref(), Some("e0"));

    // Two chapters in order, each opening with its title.
    let order: Vec<String> = read
        .reading_order()
        .into_iter()
        .map(|id| read.name(id))
        .collect();
    assert_eq!(order, ["c0", "c1"]);
    let chapters = read.chapters().unwrap();
    assert_eq!(chapters[0][0].text, "Install");
    assert_eq!(chapters[1][0].text, "Use");
    let text: String = chapters
        .iter()
        .flatten()
        .map(|block| block.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("日本語のテキスト"), "{text}");
    assert!(text.contains("code here"));
    assert!(text.contains("• one"));
    assert!(text.contains("A note."));
    // Bold text has a look of its own.
    let first = chapters[0]
        .iter()
        .find(|block| block.text.starts_with("First you"))
        .unwrap();
    assert!(
        first
            .styles
            .iter()
            .any(|(start, length, _)| (*start, *length) == (10, 7)),
        "{first:?}"
    );

    // The table of contents, nested, each entry on its heading.
    let block_text = |eid: i64| {
        chapters
            .iter()
            .flatten()
            .find(|block| block.eid == eid)
            .map(|block| block.text.clone())
            .unwrap_or_default()
    };
    let toc: Vec<(String, usize)> = read
        .toc()
        .into_iter()
        .map(|(label, depth, _, _)| (label, depth))
        .collect();
    assert_eq!(
        toc,
        [
            ("Install".to_owned(), 0),
            ("Set up".to_owned(), 1),
            ("Check".to_owned(), 1),
            ("Use".to_owned(), 0),
            ("Daily use".to_owned(), 1),
            ("Later".to_owned(), 1)
        ]
    );
    for (label, _, eid, offset) in read.toc() {
        assert_eq!(block_text(eid), label);
        assert_eq!(offset, 0);
    }

    // Links: the address out, and the cross reference into the other chapter's section.
    let links: Vec<_> = first
        .links
        .iter()
        .map(|(_, _, anchor)| read.anchor(*anchor).unwrap())
        .collect();
    assert_eq!(links, [Err("https://example.com/get".to_owned())]);
    let see = chapters[0]
        .iter()
        .find(|block| block.text.starts_with("See "))
        .unwrap();
    let (eid, _) = read.anchor(see.links[0].2).unwrap().unwrap();
    assert_eq!(block_text(eid), "Later");
    let (eid, _) = read.anchor(see.links[1].2).unwrap().unwrap();
    assert_eq!(block_text(eid), "Check");
    // The footnote mark leads to its note.
    let mark = chapters[1]
        .iter()
        .find(|block| block.text.starts_with("Text."))
        .unwrap();
    let (eid, _) = read.anchor(mark.links[0].2).unwrap().unwrap();
    assert!(block_text(eid).ends_with("A note."));

    // The table, and the picture's bytes.
    let table = chapters[1]
        .iter()
        .find(|block| block.kind == "table")
        .unwrap();
    assert_eq!(
        table.rows[0]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    let picture = chapters[0]
        .iter()
        .find(|block| block.kind == "image")
        .unwrap();
    assert_eq!(picture.alt.as_deref(), Some("a red dot"));
    assert_eq!(read.picture(picture.resource.unwrap()), Some(PNG));
}

/// Writes the sample book in both formats to `$ADOC_KINDLE_OUT`, for opening on a Kindle
/// (`docs/qa/kindle.md`): `ADOC_KINDLE_OUT=<dir> cargo test -p adoc-kindle --test kfx -- --ignored`.
#[test]
#[ignore = "writes sample books for manual checks"]
fn write_samples() {
    let out = std::path::PathBuf::from(std::env::var("ADOC_KINDLE_OUT").expect("ADOC_KINDLE_OUT"));
    let folder = tempfile::tempdir().unwrap();
    let book = guide(folder.path());
    for format in [KindleFormat::Azw3, KindleFormat::Kfx] {
        let bytes = write_book(&book, format).unwrap();
        std::fs::write(out.join(format!("guide.{}", format.extension())), bytes).unwrap();
    }
}
