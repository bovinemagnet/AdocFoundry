//! Reading a written package back, for tests here and in the crates that export through this
//! one: a ZIP reader of its own, so tests check the bytes written rather than trusting a library
//! to forgive them, and [`assert_valid_package`], every check a reader makes. Behind the
//! `test-support` feature; nothing in an export uses it.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

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

/// Whether `head`, a font's first 32 bytes, is the start of an OpenType font `length` bytes
/// long: its version, a table directory consistent with its count of tables, and a first table
/// named in letters and lying within the font.
fn opens_as_a_font(head: &[u8], length: usize) -> bool {
    let u16_at = |at: usize| usize::from(u16::from_be_bytes([head[at], head[at + 1]]));
    let u32_at = |at: usize| u32::from_be_bytes(head[at..at + 4].try_into().unwrap()) as usize;
    if head.len() < 32 || !matches!(&head[..4], [0, 1, 0, 0] | b"true" | b"OTTO") {
        return false;
    }
    let tables = u16_at(4);
    let power = (usize::BITS - 1 - tables.max(1).leading_zeros()) as usize;
    let search_range = (1 << power) * 16;
    tables > 0
        && u16_at(6) == search_range
        && u16_at(8) == power
        && u16_at(10) == tables * 16 - search_range
        && head[12..16]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'/'))
        && u32_at(20) + u32_at(24) <= length
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

    // Every part but the pictures and fonts is well-formed XML.
    for name in names
        .iter()
        .filter(|name| !name.starts_with("word/media/") && !name.starts_with("word/fonts/"))
    {
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

    // Embedded fonts: each face in the font table is a font part of its own content type, which
    // its key opens into a font; every font part is one of them; and the settings say fonts
    // are embedded exactly when one is.
    let mut embedded = BTreeSet::new();
    if names.contains("word/fontTable.xml") {
        assert_eq!(
            overrides["/word/fontTable.xml"],
            "application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml"
        );
        let rels =
            roxmltree::Document::parse(text(&entries, "word/_rels/document.xml.rels")).unwrap();
        assert!(
            rels.descendants().any(|node| {
                node.attribute("Type")
                    .is_some_and(|kind| kind.ends_with("/fontTable"))
                    && node.attribute("Target") == Some("fontTable.xml")
            }),
            "the font table has no relationship from the document"
        );
        let table = roxmltree::Document::parse(text(&entries, "word/fontTable.xml")).unwrap();
        let font_rels = entries
            .get("word/_rels/fontTable.xml.rels")
            .map(|_| text(&entries, "word/_rels/fontTable.xml.rels"))
            .unwrap_or("<Relationships/>");
        let font_rels = roxmltree::Document::parse(font_rels).unwrap();
        let mut families = BTreeSet::new();
        for font in table
            .descendants()
            .filter(|node| node.has_tag_name((W, "font")))
        {
            let name = font.attribute((W, "name")).expect("a font has a name");
            assert!(families.insert(name), "font {name} twice in the font table");
            for embed in font
                .children()
                .filter(|node| node.is_element() && node.tag_name().name().starts_with("embed"))
            {
                let id = embed.attribute((R, "id")).expect("an embedded face's r:id");
                let relationship = font_rels
                    .descendants()
                    .find(|node| node.attribute("Id") == Some(id))
                    .unwrap_or_else(|| panic!("font {name}: no relationship {id}"));
                assert!(
                    relationship
                        .attribute("Type")
                        .is_some_and(|kind| kind.ends_with("/relationships/font")),
                    "font {name}: {id} is not a font relationship"
                );
                let part = target_of(
                    "word/fontTable.xml",
                    relationship.attribute("Target").unwrap(),
                );
                assert_eq!(
                    overrides.get(format!("/{part}").as_str()).copied(),
                    Some("application/vnd.openxmlformats-officedocument.obfuscatedFont"),
                    "{part}'s content type"
                );
                let key = embed.attribute((W, "fontKey")).unwrap_or_default();
                let groups: Vec<usize> = key
                    .trim_start_matches('{')
                    .trim_end_matches('}')
                    .split('-')
                    .map(str::len)
                    .collect();
                assert!(
                    key.starts_with('{')
                        && key.ends_with('}')
                        && groups == [8, 4, 4, 4, 12]
                        && key[1..key.len() - 1]
                            .chars()
                            .all(|c| c == '-' || c.is_ascii_hexdigit()),
                    "font {name}: the font key {key} is not a GUID"
                );
                let data = entries
                    .get(&part)
                    .unwrap_or_else(|| panic!("font {name}: {part} is not in the package"));
                let key = crate::fonts::key_bytes(key);
                let opened: Vec<u8> = data
                    .iter()
                    .take(32)
                    .enumerate()
                    .map(|(index, byte)| byte ^ key[index % 16])
                    .collect();
                assert!(
                    opens_as_a_font(&opened, data.len()),
                    "font {name}: {part} does not open into a font with its key"
                );
                embedded.insert(part);
            }
        }
    }
    for name in names.iter().filter(|name| name.starts_with("word/fonts/")) {
        assert!(
            embedded.contains(*name),
            "{name} is no face in the font table"
        );
    }
    let settings = entries
        .get("word/settings.xml")
        .map(|_| text(&entries, "word/settings.xml"))
        .unwrap_or_default();
    assert_eq!(
        settings.contains("<w:embedTrueTypeFonts/>"),
        !embedded.is_empty(),
        "the settings say fonts are embedded only when one is"
    );

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
    // The parts holding text: the document, its footnotes, headers and footers.
    let xml_parts: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| {
            *name == "word/document.xml"
                || *name == "word/footnotes.xml"
                || (name.starts_with("word/header") || name.starts_with("word/footer"))
                    && name.ends_with(".xml")
        })
        .collect();
    let body_parts: Vec<roxmltree::Document> = xml_parts
        .iter()
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

    // Fields are whole: within each part, every field that begins separates at most once and
    // ends, in order, and none is left open.
    for name in &xml_parts {
        let xml = roxmltree::Document::parse(text(&entries, name)).unwrap();
        let mut open: Vec<bool> = Vec::new();
        for node in xml
            .descendants()
            .filter(|node| node.has_tag_name((W, "fldChar")))
        {
            match node.attribute((W, "fldCharType")).unwrap() {
                "begin" => open.push(false),
                "separate" => {
                    let separated = open
                        .last_mut()
                        .unwrap_or_else(|| panic!("{name}: a field separates before it begins"));
                    assert!(!*separated, "{name}: a field separates twice");
                    *separated = true;
                }
                "end" => {
                    assert!(
                        open.pop().is_some(),
                        "{name}: a field ends before it begins"
                    );
                }
                other => panic!("{name}: field character {other}"),
            }
        }
        assert!(open.is_empty(), "{name}: {} field(s) left open", open.len());
    }

    // Every section's header and footer references lead to header and footer parts.
    for reference in document.descendants().filter(|node| {
        node.has_tag_name((W, "headerReference")) || node.has_tag_name((W, "footerReference"))
    }) {
        let kind = reference.tag_name().name().trim_end_matches("Reference");
        let id = reference.attribute((R, "id")).unwrap();
        let rels =
            roxmltree::Document::parse(text(&entries, "word/_rels/document.xml.rels")).unwrap();
        let relationship = rels
            .descendants()
            .find(|node| node.attribute("Id") == Some(id))
            .unwrap_or_else(|| panic!("no relationship {id}"));
        assert!(
            relationship
                .attribute("Type")
                .unwrap()
                .ends_with(&format!("/{kind}")),
            "{id} is a {kind} reference to a {:?}",
            relationship.attribute("Type")
        );
        let part = target_of(
            "word/document.xml",
            relationship.attribute("Target").unwrap(),
        );
        let root = roxmltree::Document::parse(text(&entries, &part)).unwrap();
        let expected = if kind == "header" { "hdr" } else { "ftr" };
        assert!(root.root_element().has_tag_name((W, expected)), "{part}");
        assert!(
            matches!(
                reference.attribute((W, "type")),
                Some("default" | "first" | "even")
            ),
            "{part}: {:?}",
            reference.attribute((W, "type"))
        );
    }

    // Bookmarks have unique ids and names, each ends, and every internal link leads to one.
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
    let ended: BTreeSet<String> = body_parts
        .iter()
        .flat_map(|part| part.descendants())
        .filter(|node| node.has_tag_name((W, "bookmarkEnd")))
        .map(|node| node.attribute((W, "id")).unwrap().to_owned())
        .collect();
    assert_eq!(ended, bookmark_ids, "every bookmark ends, once");
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

/// `entries` written back as a package with the crate's own ZIP writer, in the order given: for
/// tests that change one thing in a valid package and check that the change is caught.
pub fn package<'a>(entries: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Vec<u8> {
    let mut zip = crate::zip::ZipWriter::new(Vec::new());
    for (name, data) in entries {
        zip.add(name, data).unwrap();
    }
    zip.finish().unwrap()
}
