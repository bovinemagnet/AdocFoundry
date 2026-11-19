//! EPUB 9/9 (#646): the in-process validator finds each thing that would make a reading system
//! refuse or misread a book (PRD §11), on books damaged one way at a time.

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_epub::{testing, validate, write_epub, Document, EpubOptions, Modified, Page};

/// A good book's files, in archive order.
fn good() -> Vec<(String, Vec<u8>)> {
    let path = Path::new("/docs/page.adoc");
    let text = "= A Page\n:lang: en\n\n[[here]]\n== Here\n\nSee <<here>> and https://example.com[the site].\n";
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    let view = adoc_render::build_view(path, &dom, &adoc_render::LocalResolver);
    let document = Document::Article(Page {
        key: "p0".to_owned(),
        view,
        depth: 0,
    });
    let bytes = write_epub(
        &document,
        &EpubOptions::new(Modified::from_unix_seconds(1_795_084_200).unwrap()),
    )
    .unwrap()
    .bytes;
    assert!(validate(&bytes).is_empty(), "{:?}", validate(&bytes));
    testing::entries(&bytes)
        .into_iter()
        .map(|entry| (entry.name, entry.data))
        .collect()
}

/// `files` as a ZIP archive, every entry stored, or deflated where `deflate` names it.
fn archive(files: &[(String, Vec<u8>)], deflate: &[&str]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for (name, data) in files {
        let (method, stored): (u16, Vec<u8>) = if deflate.contains(&name.as_str()) {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data).unwrap();
            (8, encoder.finish().unwrap())
        } else {
            (0, data.clone())
        };
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let header = |signature: &[u8], central: bool| {
            let mut header = signature.to_vec();
            if central {
                header.extend_from_slice(&20u16.to_le_bytes());
            }
            header.extend_from_slice(&20u16.to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(&method.to_le_bytes());
            header.extend_from_slice(&[0, 0, 0x21, 0]);
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            if central {
                header.extend_from_slice(&[0; 10]); // comment, disk, internal and external attributes
                header.extend_from_slice(&offset.to_le_bytes());
            }
            header.extend_from_slice(name.as_bytes());
            header
        };
        out.extend(header(b"PK\x03\x04", false));
        out.extend_from_slice(&stored);
        directory.extend(header(b"PK\x01\x02", true));
    }
    let start = out.len() as u32;
    let count = files.len() as u16;
    let size = directory.len() as u32;
    out.extend(directory);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The problems of the good book changed by `change`.
fn problems(change: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) -> Vec<String> {
    let mut files = good();
    change(&mut files);
    validate(&archive(&files, &[]))
}

fn edit(files: &mut [(String, Vec<u8>)], name: &str, from: &str, to: &str) {
    let file = files.iter_mut().find(|(file, _)| file == name).unwrap();
    let text = String::from_utf8(file.1.clone()).unwrap();
    assert!(text.contains(from), "{from} in {name}");
    file.1 = text.replacen(from, to, 1).into_bytes();
}

fn finds(problems: &[String], needle: &str) {
    assert!(
        problems.iter().any(|problem| problem.contains(needle)),
        "no problem about {needle}: {problems:?}"
    );
}

#[test]
fn the_validator_finds_what_breaks_a_book() {
    // The good book, rebuilt stored, passes: the validator reads what it should.
    assert_eq!(problems(|_| {}), Vec::<String>::new());

    // The container.
    let files = good();
    finds(
        &validate(&archive(&files, &["mimetype"])),
        "mimetype is compressed",
    );
    finds(
        &problems(|files| files.swap(0, 1)),
        "mimetype is not the first entry",
    );
    finds(
        &problems(|files| files[0].1 = b"application/zip".to_vec()),
        "not exactly",
    );

    // The package.
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/package.opf",
                "property=\"dcterms:modified\"",
                "property=\"x\"",
            )
        }),
        "dcterms:modified",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/package.opf",
                "<dc:language>en</dc:language>",
                "",
            )
        }),
        "dc:language",
    );
    finds(
        &problems(|files| files.retain(|(name, _)| name != "EPUB/text/c1.xhtml")),
        "which is missing",
    );
    finds(
        &problems(|files| files.push(("EPUB/extra.xhtml".to_owned(), b"<html/>".to_vec()))),
        "EPUB/extra.xhtml is not in the manifest",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/package.opf",
                "media-type=\"text/css\"",
                "media-type=\"text/plain\"",
            )
        }),
        "is listed as text/plain",
    );
    finds(
        &problems(|files| edit(files, "EPUB/package.opf", " properties=\"nav\"", "")),
        "exactly one navigation document",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/package.opf",
                "<itemref idref=\"c1\"/>",
                "<itemref idref=\"nowhere\"/>",
            )
        }),
        "not in the manifest",
    );

    // The documents.
    finds(
        &problems(|files| edit(files, "EPUB/text/c1.xhtml", "</section>", "</div>")),
        "is not well-formed",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/nav.xhtml",
                "epub:type=\"toc\"",
                "epub:type=\"x\"",
            )
        }),
        "toc navigation",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/text/c1.xhtml",
                "href=\"https://example.com\"",
                "href=\"javascript:alert(1)\"",
            )
        }),
        "leads out of the book",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/text/c1.xhtml",
                "href=\"https://example.com\"",
                "href=\"file:///etc/passwd\"",
            )
        }),
        "leads out of the book",
    );
    finds(
        &problems(|files| edit(files, "EPUB/text/c1.xhtml", "href=\"#", "href=\"#missing-")),
        "leads to no element",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/text/c1.xhtml",
                "href=\"../css/book.css\"",
                "href=\"../css/none.css\"",
            )
        }),
        "is not a file of the book of its kind",
    );
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/nav.xhtml",
                "href=\"text/c1.xhtml\"",
                "href=\"../../outside.xhtml\"",
            )
        }),
        "leaves the book",
    );

    // A hyperlink to a document outside the spine, such as the navigation document itself.
    finds(
        &problems(|files| {
            edit(
                files,
                "EPUB/text/c1.xhtml",
                "href=\"https://example.com\"",
                "href=\"../nav.xhtml\"",
            )
        }),
        "not in the spine",
    );

    // An unsafe name in the archive.
    finds(
        &problems(|files| files.push(("../escape.xhtml".to_owned(), b"<html/>".to_vec()))),
        "is not a safe name",
    );
}
