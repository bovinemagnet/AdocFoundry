//! Reading a written package back: a ZIP reader of its own, so the tests check the bytes the
//! crate writes rather than trusting a library to forgive them.

#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

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

const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const CORE_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const STYLES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";

/// The part a relationship in `source`'s relationships part points at, as a package name.
fn target_of(source: &str, target: &str) -> String {
    let base = source.rsplit_once('/').map_or("", |(folder, _)| folder);
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for segment in target.split('/') {
        match segment {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            segment => parts.push(segment),
        }
    }
    parts.join("/")
}

/// The relationships part of `part` (`word/_rels/document.xml.rels` for `word/document.xml`;
/// `_rels/.rels` for the package).
fn relationships_of(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((folder, name)) => format!("{folder}/_rels/{name}.rels"),
        None if part.is_empty() => "_rels/.rels".to_owned(),
        None => format!("_rels/{part}.rels"),
    }
}

/// Every check a reader makes of the package, against its bytes.
pub fn assert_valid_package(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let entries = entries(bytes);
    let names: BTreeSet<&str> = entries.keys().map(String::as_str).collect();
    for required in [
        "[Content_Types].xml",
        "_rels/.rels",
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "word/styles.xml",
        "docProps/core.xml",
    ] {
        assert!(names.contains(required), "{required} is missing: {names:?}");
    }

    // Every part is well-formed XML.
    for name in &names {
        roxmltree::Document::parse(text(&entries, name))
            .unwrap_or_else(|error| panic!("{name} is not well formed: {error}"));
    }

    // Every part has a content type, by extension or by name, and every override names a part.
    let types_text = text(&entries, "[Content_Types].xml");
    let types = roxmltree::Document::parse(types_text).unwrap();
    assert_eq!(
        types.root_element().tag_name().namespace(),
        Some(CONTENT_TYPES)
    );
    let defaults: BTreeMap<&str, &str> = types
        .descendants()
        .filter(|node| node.has_tag_name((CONTENT_TYPES, "Default")))
        .map(|node| {
            (
                node.attribute("Extension").unwrap(),
                node.attribute("ContentType").unwrap(),
            )
        })
        .collect();
    let overrides: BTreeMap<&str, &str> = types
        .descendants()
        .filter(|node| node.has_tag_name((CONTENT_TYPES, "Override")))
        .map(|node| {
            (
                node.attribute("PartName").unwrap(),
                node.attribute("ContentType").unwrap(),
            )
        })
        .collect();
    for part in overrides.keys() {
        let name = part.strip_prefix('/').expect("a part name starts with /");
        assert!(
            names.contains(name),
            "an override names {part}, not in the package"
        );
    }
    for name in names.iter().filter(|name| **name != "[Content_Types].xml") {
        let by_name = overrides.get(format!("/{name}").as_str());
        let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
        assert!(
            by_name.is_some() || defaults.contains_key(extension),
            "{name} has no content type"
        );
    }
    assert_eq!(
        overrides["/word/document.xml"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
    );
    assert_eq!(
        overrides["/word/styles.xml"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"
    );
    assert_eq!(
        overrides["/docProps/core.xml"],
        "application/vnd.openxmlformats-package.core-properties+xml"
    );
    assert_eq!(
        defaults["rels"],
        "application/vnd.openxmlformats-package.relationships+xml"
    );

    // Every relationship has a unique id and a target in the package.
    let mut by_type = BTreeMap::new();
    for rels in names.iter().filter(|name| name.ends_with(".rels")) {
        let source = match *rels {
            "_rels/.rels" => String::new(),
            rels => {
                let (folder, file) = rels.rsplit_once("/_rels/").unwrap();
                format!("{folder}/{}", file.strip_suffix(".rels").unwrap())
            }
        };
        assert_eq!(relationships_of(&source), *rels);
        assert!(
            source.is_empty() || names.contains(source.as_str()),
            "{rels} has no part"
        );
        let xml = roxmltree::Document::parse(text(&entries, rels)).unwrap();
        let mut ids = BTreeSet::new();
        for relationship in xml
            .descendants()
            .filter(|node| node.has_tag_name((PACKAGE_RELATIONSHIPS, "Relationship")))
        {
            let id = relationship.attribute("Id").unwrap();
            assert!(ids.insert(id.to_owned()), "{rels}: {id} twice");
            let target = relationship.attribute("Target").unwrap();
            if relationship.attribute("TargetMode") != Some("External") {
                let part = target_of(&source, target);
                assert!(
                    names.contains(part.as_str()),
                    "{rels}: {id} leads to {part}, not in the package"
                );
                by_type.insert(
                    (
                        source.clone(),
                        relationship.attribute("Type").unwrap().to_owned(),
                    ),
                    part,
                );
            }
        }
    }
    assert_eq!(
        by_type[&(String::new(), OFFICE_DOCUMENT.to_owned())],
        "word/document.xml"
    );
    assert_eq!(
        by_type[&(String::new(), CORE_PROPERTIES.to_owned())],
        "docProps/core.xml"
    );
    assert_eq!(
        by_type[&("word/document.xml".to_owned(), STYLES.to_owned())],
        "word/styles.xml"
    );

    // The document's root and body, and every `r:id` it uses has a relationship.
    let document = roxmltree::Document::parse(text(&entries, "word/document.xml")).unwrap();
    assert!(document.root_element().has_tag_name((W, "document")));
    let body = document
        .root_element()
        .children()
        .find(|node| node.has_tag_name((W, "body")))
        .expect("a body");
    assert!(
        body.last_element_child()
            .unwrap()
            .has_tag_name((W, "sectPr")),
        "the body ends with its section's properties"
    );
    // Every `r:` id in a part has a relationship in that part's own relationships part.
    for name in names
        .iter()
        .filter(|name| name.ends_with(".xml") && !name.contains("_rels/"))
    {
        let xml = roxmltree::Document::parse(text(&entries, name)).unwrap();
        let rels_name = relationships_of(name);
        let ids: BTreeSet<String> = entries
            .get(&rels_name)
            .map(|_| {
                roxmltree::Document::parse(text(&entries, &rels_name))
                    .unwrap()
                    .descendants()
                    .filter_map(|node| node.attribute("Id").map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        for node in xml.descendants() {
            for attribute in node.attributes().filter(|a| a.namespace() == Some(R)) {
                assert!(
                    ids.contains(attribute.value()),
                    "{name}: r:{} {} has no relationship in {rels_name}",
                    attribute.name(),
                    attribute.value()
                );
            }
        }
    }

    // Every style used is defined, as the kind of style it is used as.
    let styles = roxmltree::Document::parse(text(&entries, "word/styles.xml")).unwrap();
    let defined: BTreeSet<(&str, &str)> = styles
        .descendants()
        .filter(|node| node.has_tag_name((W, "style")))
        .filter_map(|node| {
            Some((
                node.attribute((W, "type"))?,
                node.attribute((W, "styleId"))?,
            ))
        })
        .collect();
    let body_parts: Vec<roxmltree::Document> = ["word/document.xml", "word/footnotes.xml"]
        .iter()
        .filter(|name| names.contains(**name))
        .map(|name| roxmltree::Document::parse(text(&entries, name)).unwrap())
        .collect();
    for part in &body_parts {
        for (element, kind) in [
            ("pStyle", "paragraph"),
            ("rStyle", "character"),
            ("tblStyle", "table"),
        ] {
            for node in part
                .descendants()
                .filter(|node| node.has_tag_name((W, element)))
            {
                let style = node.attribute((W, "val")).unwrap();
                assert!(
                    defined.contains(&(kind, style)),
                    "the {kind} style {style} is used and not defined"
                );
            }
        }
    }

    // Every list's numbering is defined, and each numbering's abstract definition.
    let used_lists: BTreeSet<&str> = body_parts
        .iter()
        .flat_map(|part| part.descendants())
        .filter(|node| node.has_tag_name((W, "numId")))
        .filter_map(|node| node.attribute((W, "val")))
        .collect();
    if !used_lists.is_empty() {
        let numbering = roxmltree::Document::parse(text(&entries, "word/numbering.xml")).unwrap();
        let abstracts: BTreeSet<&str> = numbering
            .descendants()
            .filter(|node| node.has_tag_name((W, "abstractNum")))
            .filter_map(|node| node.attribute((W, "abstractNumId")))
            .collect();
        let nums: BTreeMap<&str, &str> = numbering
            .descendants()
            .filter(|node| node.has_tag_name((W, "num")))
            .filter_map(|node| {
                let id = node.attribute((W, "numId"))?;
                let abstract_id = node
                    .children()
                    .find(|child| child.has_tag_name((W, "abstractNumId")))?
                    .attribute((W, "val"))?;
                Some((id, abstract_id))
            })
            .collect();
        for id in &used_lists {
            let abstract_id = nums.get(id).unwrap_or_else(|| panic!("no numbering {id}"));
            assert!(
                abstracts.contains(abstract_id),
                "no abstract numbering {abstract_id}"
            );
        }
    }

    // Every footnote referred to exists.
    let footnotes: BTreeSet<String> = if names.contains("word/footnotes.xml") {
        roxmltree::Document::parse(text(&entries, "word/footnotes.xml"))
            .unwrap()
            .descendants()
            .filter(|node| node.has_tag_name((W, "footnote")))
            .filter_map(|node| node.attribute((W, "id")).map(str::to_owned))
            .collect()
    } else {
        BTreeSet::new()
    };
    for node in body_parts
        .iter()
        .flat_map(|part| part.descendants())
        .filter(|node| node.has_tag_name((W, "footnoteReference")))
    {
        let id = node.attribute((W, "id")).unwrap();
        assert!(
            footnotes.contains(id),
            "footnote {id} is referred to and not written"
        );
    }

    // Bookmarks have unique ids and names, and every internal link leads to one.
    let mut bookmark_ids = BTreeSet::new();
    let mut bookmarks = BTreeSet::new();
    for node in body_parts
        .iter()
        .flat_map(|part| part.descendants())
        .filter(|node| node.has_tag_name((W, "bookmarkStart")))
    {
        assert!(bookmark_ids.insert(node.attribute((W, "id")).unwrap().to_owned()));
        let name = node.attribute((W, "name")).unwrap();
        assert!(
            name.len() <= 40,
            "bookmark name {name} is longer than Word allows"
        );
        assert!(bookmarks.insert(name.to_owned()), "bookmark {name} twice");
    }
    for node in body_parts
        .iter()
        .flat_map(|part| part.descendants())
        .filter(|node| node.has_tag_name((W, "hyperlink")))
    {
        if let Some(anchor) = node.attribute((W, "anchor")) {
            assert!(
                bookmarks.contains(anchor),
                "a link leads to {anchor}, no bookmark"
            );
        }
    }
    entries
}
