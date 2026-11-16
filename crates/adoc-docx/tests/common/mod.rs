//! Reading a written package back: a ZIP reader of its own, so the tests check the bytes the
//! crate writes rather than trusting a library to forgive them.

#![allow(dead_code)]

use std::{collections::BTreeMap, io::Read, path::Path};

use adoc_docx::{write_docx, DocxDocument, DocxOptions, DocxPage, DocxReport};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};

/// The view of `text` as a page at `path`.
pub fn view(path: &str, text: &str) -> adoc_render::ViewTree {
    let path = Path::new(path);
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

/// `text` as an article.
pub fn article(text: &str) -> DocxDocument {
    DocxDocument::Article(DocxPage {
        key: "p0".to_owned(),
        view: view("/docs/page.adoc", text),
        depth: 0,
    })
}

/// `document` written with the default options.
pub fn written(document: &DocxDocument) -> (Vec<u8>, DocxReport) {
    let mut bytes = Vec::new();
    let report = write_docx(document, &DocxOptions::default(), &mut bytes).unwrap();
    (bytes, report)
}

fn u16_at(bytes: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

/// The package's entries by name, read through the central directory, each checked against its
/// local header and its CRC-32. Panics on anything a ZIP reader could not read.
pub fn entries(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let end = bytes.len() - 22;
    assert_eq!(
        &bytes[end..end + 4],
        b"PK\x05\x06",
        "end of central directory"
    );
    let count = u16_at(bytes, end + 10);
    let directory_size = u32_at(bytes, end + 12);
    let mut at = u32_at(bytes, end + 16);
    assert_eq!(
        at + directory_size,
        end,
        "the directory ends where the end record starts"
    );
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        assert_eq!(
            &bytes[at..at + 4],
            b"PK\x01\x02",
            "central directory header"
        );
        let method = u16_at(bytes, at + 10);
        let crc = u32_at(bytes, at + 16);
        let compressed = u32_at(bytes, at + 20);
        let size = u32_at(bytes, at + 24);
        let name_length = u16_at(bytes, at + 28);
        let extra = u16_at(bytes, at + 30);
        let comment = u16_at(bytes, at + 32);
        let local = u32_at(bytes, at + 42);
        let name = String::from_utf8(bytes[at + 46..at + 46 + name_length].to_vec()).unwrap();
        at += 46 + name_length + extra + comment;

        assert_eq!(
            &bytes[local..local + 4],
            b"PK\x03\x04",
            "local header of {name}"
        );
        assert_eq!(u32_at(bytes, local + 14), crc, "{name}: local CRC");
        let local_name = u16_at(bytes, local + 26);
        let local_extra = u16_at(bytes, local + 28);
        assert_eq!(&bytes[local + 30..local + 30 + local_name], name.as_bytes());
        let start = local + 30 + local_name + local_extra;
        let data = &bytes[start..start + compressed];
        let content = match method {
            0 => data.to_vec(),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(data)
                    .read_to_end(&mut out)
                    .unwrap();
                out
            }
            other => panic!("{name}: compression method {other}"),
        };
        assert_eq!(content.len(), size, "{name}: size");
        assert_eq!(crc32fast::hash(&content) as usize, crc, "{name}: CRC");
        assert!(
            entries.insert(name.clone(), content).is_none(),
            "{name} is in the package twice"
        );
    }
    entries
}

/// An entry as text, which every XML part must be.
pub fn text<'a>(entries: &'a BTreeMap<String, Vec<u8>>, name: &str) -> &'a str {
    std::str::from_utf8(entries.get(name).unwrap_or_else(|| panic!("no {name}"))).unwrap()
}

pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const PACKAGE_RELATIONSHIPS: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships";

/// The paragraphs of `word/document.xml`: each one's style and its text, `<w:tab/>` read as a
/// tab and `<w:br/>` as a new line.
pub fn paragraphs(document: &str) -> Vec<(Option<String>, String)> {
    let xml = roxmltree::Document::parse(document).unwrap();
    xml.descendants()
        .filter(|node| node.has_tag_name((W, "p")))
        .map(|paragraph| {
            let style = paragraph
                .descendants()
                .find(|node| node.has_tag_name((W, "pStyle")))
                .and_then(|node| node.attribute((W, "val")))
                .map(str::to_owned);
            let mut text = String::new();
            for node in paragraph.descendants() {
                if node.has_tag_name((W, "t")) {
                    text.push_str(node.text().unwrap_or_default());
                } else if node.has_tag_name((W, "tab")) {
                    text.push('\t');
                } else if node.has_tag_name((W, "br")) {
                    text.push('\n');
                }
            }
            (style, text)
        })
        .collect()
}
