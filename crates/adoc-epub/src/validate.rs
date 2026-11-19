//! The in-process check every book passes before it is handed out, and that anyone may run on
//! an EPUB's bytes (PRD §11): the container, the package, the navigation, well-formed XML, every
//! file the manifest lists and every file it should, media types, and every link, picture and
//! stylesheet reference. It is not EPUBCheck and certifies no conformance; it catches what would
//! make a reading system refuse or misread the book. Every read is bounds-checked, so a damaged
//! archive gives problems, never a panic.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

const OPF: &str = "http://www.idpf.org/2007/opf";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const XHTML: &str = "http://www.w3.org/1999/xhtml";
const OPS: &str = "http://www.idpf.org/2007/ops";
const CONTAINER: &str = "urn:oasis:names:tc:opendocument:xmlns:container";
/// The schemes a link out of the book may have.
const SCHEMES: &[&str] = &["http", "https", "mailto", "ftp", "ftps", "tel"];
/// The most bytes an entry may inflate to, so a damaged or hostile archive cannot take the
/// memory of the process checking it.
const ENTRY_LIMIT: u64 = 512 * 1024 * 1024;

struct Entry {
    name: String,
    method: u16,
    local_extra: usize,
    data: Vec<u8>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes(
        bytes.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?)).ok()
}

/// The archive's entries in central directory order.
fn entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let truncated = || "the archive is truncated or not a ZIP archive".to_owned();
    let end = bytes.len().checked_sub(22).ok_or_else(truncated)?;
    if bytes.get(end..end + 4) != Some(b"PK\x05\x06") {
        return Err("the archive has no end of central directory where it should".to_owned());
    }
    let count = u16_at(bytes, end + 10).ok_or_else(truncated)?;
    let mut at = u32_at(bytes, end + 16).ok_or_else(truncated)?;
    let mut entries = Vec::new();
    for _ in 0..count {
        if bytes.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err("the central directory is damaged".to_owned());
        }
        let field = |offset: usize| u16_at(bytes, at + offset).ok_or_else(truncated);
        let flags = field(8)?;
        let method = field(10)?;
        let crc = u32_at(bytes, at + 16).ok_or_else(truncated)?;
        let compressed = u32_at(bytes, at + 20).ok_or_else(truncated)?;
        let size = u32_at(bytes, at + 24).ok_or_else(truncated)?;
        let (name_length, extra, comment) = (field(28)?, field(30)?, field(32)?);
        let local = u32_at(bytes, at + 42).ok_or_else(truncated)?;
        let name = bytes
            .get(at + 46..at + 46 + name_length)
            .ok_or_else(truncated)?;
        let name = String::from_utf8(name.to_vec())
            .map_err(|_| "an entry's name is not UTF-8".to_owned())?;
        at += 46 + name_length + extra + comment;
        if flags & 1 != 0 {
            return Err(format!("{name} is encrypted"));
        }
        if bytes.get(local..local + 4) != Some(b"PK\x03\x04") {
            return Err(format!(
                "{name} has no local header where the directory says"
            ));
        }
        let local_name = u16_at(bytes, local + 26).ok_or_else(truncated)?;
        let local_extra = u16_at(bytes, local + 28).ok_or_else(truncated)?;
        let start = local + 30 + local_name + local_extra;
        let stored = bytes
            .get(start..start + compressed)
            .ok_or_else(|| format!("{name} runs past the end of the archive"))?;
        let data = match method {
            0 => stored.to_vec(),
            8 => {
                let mut data = Vec::new();
                flate2::read::DeflateDecoder::new(stored)
                    .take(ENTRY_LIMIT)
                    .read_to_end(&mut data)
                    .map_err(|_| format!("{name} cannot be inflated"))?;
                data
            }
            other => return Err(format!("{name} is compressed with method {other}")),
        };
        if data.len() != size || crc32fast::hash(&data) as usize != crc {
            return Err(format!("{name} does not match its size or checksum"));
        }
        entries.push(Entry {
            name,
            method: u16::try_from(method).unwrap_or(u16::MAX),
            local_extra,
            data,
        });
    }
    Ok(entries)
}

fn parse(text: &str) -> Result<roxmltree::Document<'_>, String> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    roxmltree::Document::parse_with_options(text, options).map_err(|error| error.to_string())
}

/// `href`, relative to the folder of `base`, as an archive path; `None` where it leaves the
/// archive.
fn resolve(base: &str, href: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop();
    for segment in href.split('/') {
        match segment {
            ".." => {
                parts.pop()?;
            }
            "." | "" => {}
            segment => parts.push(segment),
        }
    }
    Some(parts.join("/"))
}

/// The media type a file of the book must be listed with, from its extension.
fn media_type(name: &str) -> Option<&'static str> {
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "xhtml" => "application/xhtml+xml",
        "css" => "text/css",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// Every problem `bytes` has as an EPUB 3 book; empty when it passes.
#[must_use]
pub fn validate(bytes: &[u8]) -> Vec<String> {
    let entries = match entries(bytes) {
        Ok(entries) => entries,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    // The container: `mimetype` first, stored, exact, with no extra field.
    match entries.first() {
        Some(first) if first.name == "mimetype" => {
            if first.method != 0 || first.local_extra != 0 {
                problems.push("mimetype is compressed or has an extra field".to_owned());
            }
            if first.data != b"application/epub+zip" {
                problems.push("mimetype is not exactly application/epub+zip".to_owned());
            }
        }
        _ => problems.push("mimetype is not the first entry".to_owned()),
    }
    let mut files = BTreeMap::new();
    for entry in &entries {
        if entry.name.starts_with('/')
            || entry.name.contains('\\')
            || entry
                .name
                .split('/')
                .any(|segment| segment.is_empty() || segment == "..")
        {
            problems.push(format!("{:?} is not a safe name", entry.name));
        }
        if files.insert(entry.name.as_str(), &entry.data).is_some() {
            problems.push(format!("{} is in the archive twice", entry.name));
        }
    }
    let text = |name: &str| -> Result<&str, String> {
        let data = files
            .get(name)
            .ok_or_else(|| format!("{name} is missing"))?;
        std::str::from_utf8(data).map_err(|_| format!("{name} is not UTF-8"))
    };
    // Every XML file is well-formed.
    for name in files.keys() {
        let xml = name.ends_with(".xml") || name.ends_with(".opf") || name.ends_with(".xhtml");
        if xml {
            if let Err(problem) = text(name).and_then(|text| parse(text).map(drop)) {
                problems.push(format!("{name} is not well-formed: {problem}"));
            }
        }
    }
    let package_path = match text("META-INF/container.xml").and_then(|text| {
        let document = parse(text)?;
        document
            .descendants()
            .find(|node| node.has_tag_name((CONTAINER, "rootfile")))
            .and_then(|node| node.attribute("full-path"))
            .map(str::to_owned)
            .ok_or_else(|| "the container names no package document".to_owned())
    }) {
        Ok(path) => path,
        Err(problem) => {
            problems.push(problem);
            return problems;
        }
    };
    let opf_text = match text(&package_path) {
        Ok(text) => text,
        Err(problem) => {
            problems.push(problem);
            return problems;
        }
    };
    let Ok(opf) = parse(opf_text) else {
        return problems;
    };
    let package = opf.root_element();
    if !package.has_tag_name((OPF, "package")) || package.attribute("version") != Some("3.0") {
        problems.push("the package document is not an EPUB 3 package".to_owned());
    }
    let metadata = package
        .children()
        .find(|node| node.has_tag_name((OPF, "metadata")));
    let dc = |name: &str| {
        metadata.and_then(|metadata| {
            metadata
                .children()
                .find(|node| node.has_tag_name((DC, name)))
                .and_then(|node| node.text())
                .filter(|text| !text.trim().is_empty())
        })
    };
    let unique = package.attribute("unique-identifier");
    let identified = metadata.is_some_and(|metadata| {
        metadata.children().any(|node| {
            node.has_tag_name((DC, "identifier"))
                && node.attribute("id") == unique
                && node.text().is_some_and(|text| !text.trim().is_empty())
        })
    });
    if !identified {
        problems.push("the package has no unique identifier".to_owned());
    }
    for name in ["title", "language"] {
        if dc(name).is_none() {
            problems.push(format!("the package has no dc:{name}"));
        }
    }
    let modified = metadata.map_or(0, |metadata| {
        metadata
            .children()
            .filter(|node| node.attribute("property") == Some("dcterms:modified"))
            .filter(|node| {
                node.text().is_some_and(|text| {
                    text.len() == 20 && text.ends_with('Z') && text.as_bytes()[10] == b'T'
                })
            })
            .count()
    });
    if modified != 1 {
        problems.push(
            "the package needs exactly one dcterms:modified, CCYY-MM-DDThh:mm:ssZ".to_owned(),
        );
    }
    // The manifest: every item a file of the archive with the right media type; one navigation.
    let mut manifest: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut navs = Vec::new();
    for item in package
        .descendants()
        .filter(|node| node.has_tag_name((OPF, "item")))
    {
        let (Some(id), Some(href), Some(media)) = (
            item.attribute("id"),
            item.attribute("href"),
            item.attribute("media-type"),
        ) else {
            problems.push("a manifest item lacks its id, href or media type".to_owned());
            continue;
        };
        if href.contains(':') || href.contains('#') {
            problems.push(format!("the manifest item {id} is not a file of the book"));
            continue;
        }
        let Some(target) = resolve(&package_path, href) else {
            problems.push(format!("the manifest item {id} leaves the book"));
            continue;
        };
        if !files.contains_key(target.as_str()) {
            problems.push(format!("the manifest names {target}, which is missing"));
        }
        if media_type(&target).is_some_and(|expected| expected != media) {
            problems.push(format!("{target} is listed as {media}"));
        }
        if item
            .attribute("properties")
            .is_some_and(|properties| properties.split_whitespace().any(|p| p == "nav"))
        {
            navs.push(target.clone());
        }
        if manifest
            .insert(id.to_owned(), (target, media.to_owned()))
            .is_some()
        {
            problems.push(format!("the manifest lists {id} twice"));
        }
    }
    if navs.len() != 1 {
        problems.push("the manifest needs exactly one navigation document".to_owned());
    }
    let listed: BTreeSet<&str> = manifest.values().map(|(path, _)| path.as_str()).collect();
    for name in files.keys() {
        if *name != "mimetype"
            && !name.starts_with("META-INF/")
            && *name != package_path
            && !listed.contains(name)
        {
            problems.push(format!("{name} is not in the manifest"));
        }
    }
    let spine: Vec<&str> = package
        .descendants()
        .filter(|node| node.has_tag_name((OPF, "itemref")))
        .filter_map(|node| node.attribute("idref"))
        .collect();
    if spine.is_empty() {
        problems.push("the spine is empty".to_owned());
    }
    for idref in &spine {
        match manifest.get(*idref) {
            Some((_, media)) if media == "application/xhtml+xml" => {}
            Some(_) => problems.push(format!("the spine names {idref}, not a content document")),
            None => problems.push(format!("the spine names {idref}, not in the manifest")),
        }
    }
    // A hyperlink may lead only to a document of the spine (EPUBCheck's RSC-011).
    let spine_documents: BTreeSet<&str> = spine
        .iter()
        .filter_map(|idref| manifest.get(*idref))
        .map(|(path, _)| path.as_str())
        .collect();
    // The documents: their ids, one toc navigation, and every reference.
    let documents: Vec<&String> = manifest
        .values()
        .filter(|(_, media)| media == "application/xhtml+xml")
        .map(|(path, _)| path)
        .collect();
    let mut ids: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let parsed: Vec<(&str, &str)> = documents
        .iter()
        .filter_map(|path| Some((path.as_str(), text(path).ok()?)))
        .collect();
    let documents_parsed: Vec<(&str, roxmltree::Document<'_>)> = parsed
        .iter()
        .filter_map(|(path, source)| parse(source).ok().map(|document| (*path, document)))
        .collect();
    for (path, document) in &documents_parsed {
        ids.insert(
            path,
            document
                .descendants()
                .filter_map(|node| node.attribute("id"))
                .collect(),
        );
    }
    for (path, document) in &documents_parsed {
        if Some(*path) == navs.first().map(String::as_str) {
            let tocs = document
                .descendants()
                .filter(|node| {
                    node.has_tag_name((XHTML, "nav"))
                        && node.attribute((OPS, "type")) == Some("toc")
                })
                .count();
            if tocs != 1 {
                problems.push(format!("{path} needs exactly one toc navigation"));
            }
        }
        for node in document.descendants().filter(|node| node.is_element()) {
            let (href, wanted) = match node.tag_name().name() {
                "a" => (node.attribute("href"), None),
                "img" => (node.attribute("src"), Some("image/")),
                "link" => (node.attribute("href"), Some("text/css")),
                _ => continue,
            };
            let Some(href) = href else { continue };
            let file = href.split('#').next().unwrap_or_default();
            if let Some((scheme, _)) = file.split_once(':') {
                let allowed =
                    wanted.is_none() && SCHEMES.contains(&scheme.to_ascii_lowercase().as_str());
                if !allowed {
                    problems.push(format!("{path}: {href} leads out of the book"));
                }
                continue;
            }
            let target = if file.is_empty() {
                Some((*path).to_owned())
            } else {
                resolve(path, file)
            };
            let Some(target) = target else {
                problems.push(format!("{path}: {href} leaves the book"));
                continue;
            };
            match wanted {
                None => match ids.get(target.as_str()) {
                    None => {
                        problems.push(format!("{path}: {href} leads to no document of the book"))
                    }
                    Some(_) if !spine_documents.contains(target.as_str()) => problems.push(
                        format!("{path}: {href} leads to a document that is not in the spine"),
                    ),
                    Some(target_ids) => {
                        if let Some((_, fragment)) = href.split_once('#') {
                            if !target_ids.contains(fragment) {
                                problems.push(format!("{path}: {href} leads to no element"));
                            }
                        }
                    }
                },
                Some(prefix) => {
                    let ok = manifest
                        .values()
                        .any(|(listed, media)| *listed == target && media.starts_with(prefix));
                    if !ok {
                        problems.push(format!(
                            "{path}: {href} is not a file of the book of its kind"
                        ));
                    }
                    if prefix == "image/" && node.attribute("alt").is_none() {
                        problems.push(format!("{path}: the picture {href} has no alt"));
                    }
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damaged_archives_give_problems_never_a_panic() {
        assert!(!validate(b"").is_empty());
        assert!(!validate(b"PK\x05\x06").is_empty());
        assert!(!validate(&[0u8; 64]).is_empty());
        let mut end = vec![0u8; 22];
        end[..4].copy_from_slice(b"PK\x05\x06");
        end[10] = 5; // five entries, none there
        assert!(!validate(&end).is_empty());
    }

    #[test]
    fn hrefs_stay_in_the_book() {
        assert_eq!(
            resolve("EPUB/text/c1.xhtml", "../images/i1.png").as_deref(),
            Some("EPUB/images/i1.png")
        );
        assert_eq!(resolve("EPUB/package.opf", "../../x"), None);
    }
}
