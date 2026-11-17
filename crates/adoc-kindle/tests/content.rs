//! AsciiDoc's meaning kept in both formats, which agree with each other (Kindle PRD §12 tests
//! 9, 10 and 11): one book written as AZW3 and as KFX, each read back by its independent
//! reader.

mod common;

use std::path::Path;

use adoc_kindle::{
    testing::{read_azw3, read_kfx, Azw3, Kfx, KfxBlock},
    write_book, write_kindle, BookItem, KindleBook, KindleDocument, KindleFormat, KindleOptions,
    KindlePage, KindlePlan,
};
use common::view;

/// Two components' pages, the second in another folder, with references between them.
const GUIDE: &str = "= Guide\n\nRead xref:../admin/backup.adoc#keep[how backups are kept] first.\n\n[[steps]]\n== Steps\n\n. Install it.\n. Start it.\n.. Check the log.\n\nA term:: Its meaning.\n\n[source,rust]\n----\nfn main() {}\n----\n\nNOTE: Keep a copy.footnote:[Off site.]\n\n|===\n|Name |Size\n\n|alpha\n|1\n|===\n";
const BACKUP: &str = "= Backup\n\n[[keep]]\n== Keeping backups\n\nBack to <<../guide/start.adoc#steps,the steps>>.\n\n____\nA quotation.\n____\n";

fn document(folder: &Path) -> KindleDocument {
    let guide = folder.join("guide");
    let admin = folder.join("admin");
    std::fs::create_dir_all(&guide).unwrap();
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::write(guide.join("start.adoc"), GUIDE).unwrap();
    std::fs::write(admin.join("backup.adoc"), BACKUP).unwrap();
    let page = |path: &Path, text: &str, key: &str, depth| {
        BookItem::Page(KindlePage {
            key: key.to_owned(),
            view: view(path, text),
            depth,
        })
    };
    KindleDocument::Book(KindlePlan {
        title: "Handbook".to_owned(),
        items: vec![
            BookItem::Part("User guide".to_owned()),
            page(&guide.join("start.adoc"), GUIDE, "p1", 1),
            BookItem::Part("Administration".to_owned()),
            page(&admin.join("backup.adoc"), BACKUP, "p2", 1),
        ],
        ..KindlePlan::default()
    })
}

fn both(folder: &Path) -> (KindleBook, Azw3, Kfx) {
    let (book, _) = KindleBook::build(&document(folder), &KindleOptions::default());
    let azw3 = read_azw3(&write_book(&book, KindleFormat::Azw3).unwrap()).unwrap();
    let kfx = read_kfx(&write_book(&book, KindleFormat::Kfx).unwrap()).unwrap();
    (book, azw3, kfx)
}

/// The text an AZW3 link lands on: its element's text up to the next tag.
fn azw3_target_text(rest: &str) -> String {
    let after = rest.split_once('>').map_or("", |(_, after)| after);
    after.split('<').next().unwrap_or("").to_owned()
}

/// PRD §12 test 9: one book gives the same chapters, table of contents and links in both
/// formats, with no link that leads nowhere.
#[test]
fn kindle_formats_agree() {
    let folder = tempfile::tempdir().unwrap();
    let (book, azw3, kfx) = both(folder.path());
    let chapters = kfx.chapters().unwrap();

    // The same chapters, in the same order, each opening with the same title.
    assert_eq!(azw3.files.len(), book.chapters.len());
    assert_eq!(chapters.len(), book.chapters.len());
    for (index, chapter) in book.chapters.iter().enumerate() {
        assert!(
            azw3.files[index].contains(&format!(">{}</h", chapter.title)),
            "{}",
            chapter.title
        );
        assert_eq!(chapters[index][0].text, chapter.title);
    }

    // The same table of contents: labels and depths.
    let azw3_toc: Vec<(String, usize)> = azw3
        .ncx
        .iter()
        .map(|entry| (entry.label.clone(), entry.depth as usize))
        .collect();
    let kfx_toc: Vec<(String, usize)> = kfx
        .toc()
        .into_iter()
        .map(|(label, depth, _, _)| (label, depth))
        .collect();
    assert_eq!(azw3_toc, kfx_toc);
    assert_eq!(
        kfx_toc
            .iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>(),
        [
            "User guide",
            "Guide",
            "Steps",
            "Administration",
            "Backup",
            "Keeping backups"
        ]
    );

    // The same links, in reading order, to the same chapters; none broken.
    let azw3_links: Vec<(usize, usize, String)> = azw3
        .hrefs()
        .into_iter()
        .filter(|(_, href)| href.starts_with("kindle:pos"))
        .map(|(from, href)| {
            let (to, rest) = azw3
                .follow(&href)
                .unwrap_or_else(|| panic!("{href} is broken"));
            (from, to, azw3_target_text(rest))
        })
        .collect();
    let block_of = |eid: i64| -> Option<(usize, &KfxBlock)> {
        chapters.iter().enumerate().find_map(|(index, blocks)| {
            blocks
                .iter()
                .find(|block| block.eid == eid)
                .map(|block| (index, block))
        })
    };
    let mut kfx_links = Vec::new();
    for (from, blocks) in chapters.iter().enumerate() {
        for block in blocks {
            for (_, _, anchor) in &block.links {
                if let Ok((eid, _)) = kfx.anchor(*anchor).expect("an anchor") {
                    let (to, target) = block_of(eid).expect("a link to a block");
                    kfx_links.push((from, to, target.text.clone()));
                }
            }
        }
    }
    let chapters_of = |links: &[(usize, usize, String)]| -> Vec<(usize, usize)> {
        links.iter().map(|(from, to, _)| (*from, *to)).collect()
    };
    assert_eq!(chapters_of(&azw3_links), chapters_of(&kfx_links));
    assert!(azw3_links.len() >= 4, "{azw3_links:?}");
    // A reference to a heading lands on that heading in both.
    for ((_, _, azw3_text), (_, _, kfx_text)) in azw3_links.iter().zip(&kfx_links) {
        if ["Keeping backups", "Steps"].contains(&azw3_text.as_str()) {
            assert_eq!(azw3_text, kfx_text);
        }
    }
    assert!(azw3_links
        .iter()
        .any(|(from, to, text)| (*from, *to) == (1, 3) && text == "Keeping backups"));
    assert!(azw3_links
        .iter()
        .any(|(from, to, text)| (*from, *to) == (3, 1) && text == "Steps"));
}

/// PRD §12 test 10: lists, code, tables and footnotes are text in both formats, never
/// pictures.
#[test]
fn kindle_content_stays_text() {
    let folder = tempfile::tempdir().unwrap();
    let (_, azw3, kfx) = both(folder.path());
    let guide = &azw3.files[1];
    assert!(guide.contains("<ol><li><p>Install it.</p></li>"), "{guide}");
    assert!(guide.contains("<ol><li><p>Check the log.</p></li></ol>"));
    assert!(guide.contains("<dt>A term</dt>"));
    assert!(guide.contains("<pre class=\"code\">fn main() {}</pre>"));
    assert!(guide.contains("<td><p>alpha</p></td>"));
    assert!(guide.contains("Off site."));
    assert!(azw3.images.is_empty(), "no pictures in place of text");

    let chapters = kfx.chapters().unwrap();
    let texts: Vec<&str> = chapters[1]
        .iter()
        .map(|block| block.text.as_str())
        .collect();
    for wanted in [
        "1. Install it.",
        "    1. Check the log.",
        "A term",
        "fn main() {}",
        "1. Off site.",
    ] {
        assert!(texts.contains(&wanted), "{wanted} in {texts:?}");
    }
    let table = chapters[1]
        .iter()
        .find(|block| block.kind == "table")
        .unwrap();
    assert_eq!(
        table.rows[1]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "1"]
    );
    assert!(chapters.iter().flatten().all(|block| block.kind != "image"));
    assert!(
        kfx.of_kind(417).next().is_none(),
        "no pictures in place of text"
    );
}

/// PRD §12 test 11: a reference out of the book, media a Kindle cannot play and a table too
/// complex to lay out each warn where they are written, and stay readable.
#[test]
fn kindle_unsupported_content_warns() {
    let text = "= Page\n\nSee xref:elsewhere.adoc[another page].\n\nvideo::intro.mp4[]\n\n|===\na|\n[cols=2]\n!===\n!inner a !inner b\n!===\n|===\n";
    let document = KindleDocument::Article(KindlePage {
        key: "p0".to_owned(),
        view: view(Path::new("/docs/page.adoc"), text),
        depth: 0,
    });
    for format in [KindleFormat::Azw3, KindleFormat::Kfx] {
        let output = write_kindle(&document, format, &KindleOptions::default()).unwrap();
        let lines: Vec<(u32, &str)> = output
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.source.as_ref().map_or(0, |span| span.line),
                    diagnostic.message.as_str(),
                )
            })
            .collect();
        let at = |needle: &str| {
            lines
                .iter()
                .find(|(_, message)| message.contains(needle))
                .map(|(line, _)| *line)
                .unwrap_or_else(|| panic!("no warning about {needle} in {lines:?}"))
        };
        assert_eq!(at("elsewhere.adoc"), 3);
        assert_eq!(at("video"), 5);
        assert!(at("a table inside a table cell") >= 9, "{lines:?}");

        // Readable all the same.
        let text = match format {
            KindleFormat::Azw3 => read_azw3(&output.bytes).unwrap().plain_text(),
            KindleFormat::Kfx => read_kfx(&output.bytes)
                .unwrap()
                .chapters()
                .unwrap()
                .iter()
                .flatten()
                .flat_map(|block| {
                    std::iter::once(block.text.clone())
                        .chain(block.rows.iter().flatten().map(|cell| cell.text.clone()))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        assert!(text.contains("another page"), "{text}");
        assert!(text.contains("intro.mp4"), "{text}");
        assert!(
            text.contains("inner a") && text.contains("inner b"),
            "{format}: {text}"
        );
    }
}

/// An id on a block that shows nothing of its own (a page break, the table of contents macro)
/// is still a place a reference may lead to, in both formats. (The parser gives a thematic
/// break no id, so a reference to one leads to nothing and is text with a warning.)
#[test]
fn ids_on_breaks_stay_places_to_link_to() {
    let text = "= Page\n\nSee <<turn>> and <<contents>>.\n\n[[turn]]\n<<<\n\n[[contents]]\ntoc::[]\n\nThe end.\n";
    let document = KindleDocument::Article(KindlePage {
        key: "p0".to_owned(),
        view: view(Path::new("/docs/page.adoc"), text),
        depth: 0,
    });
    for format in [KindleFormat::Azw3, KindleFormat::Kfx] {
        let output = write_kindle(&document, format, &KindleOptions::default())
            .unwrap_or_else(|error| panic!("{format}: {error}"));
        match format {
            KindleFormat::Azw3 => {
                let read = read_azw3(&output.bytes).unwrap();
                let links: Vec<_> = read
                    .hrefs()
                    .into_iter()
                    .filter(|(_, href)| href.starts_with("kindle:pos"))
                    .collect();
                assert_eq!(links.len(), 2, "{links:?}");
                for (_, href) in links {
                    assert!(read.follow(&href).is_some(), "{href}");
                }
            }
            KindleFormat::Kfx => {
                let read = read_kfx(&output.bytes).unwrap();
                let chapters = read.chapters().unwrap();
                let see = chapters[0]
                    .iter()
                    .find(|block| block.text.starts_with("See "))
                    .unwrap();
                assert_eq!(see.links.len(), 2);
                for (_, _, anchor) in &see.links {
                    assert!(read.anchor(*anchor).unwrap().is_ok());
                }
            }
        }
    }
}
