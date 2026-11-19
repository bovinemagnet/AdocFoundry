//! Reading a written book back, for tests here and in the crates that export through this one:
//! a ZIP reader of its own, so tests check the bytes written rather than trusting a library to
//! forgive them, and [`assert_valid_ocf`], the container and package checks a reading system
//! makes first. Behind the `test-support` feature; nothing in an export uses it.

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

/// One file of the archive as its local header gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    /// 0 stored, 8 deflated.
    pub method: u16,
    pub flags: u16,
    /// The length of its local and central extra fields.
    pub extra: (usize, usize),
    /// Its MS-DOS time and date.
    pub dos: (u16, u16),
    pub data: Vec<u8>,
}

/// A book's files in archive order (the central directory's, which the writer keeps in the
/// order it wrote the files), read through the central directory and each checked
/// against its local header and its CRC-32. Panics on anything a ZIP reader could not read.
pub fn entries(bytes: &[u8]) -> Vec<Entry> {
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
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    for _ in 0..count {
        assert_eq!(
            &bytes[at..at + 4],
            b"PK\x01\x02",
            "central directory header"
        );
        let flags = u16_at(bytes, at + 8);
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
        assert_eq!(u16_at(bytes, local + 6), flags, "{name}: local flags");
        assert_eq!(u16_at(bytes, local + 8), method, "{name}: local method");
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
        assert!(names.insert(name.clone()), "{name} is in the archive twice");
        let dos = (
            u16::from_le_bytes([bytes[local + 10], bytes[local + 11]]),
            u16::from_le_bytes([bytes[local + 12], bytes[local + 13]]),
        );
        entries.push(Entry {
            name,
            method: u16::try_from(method).unwrap(),
            flags: u16::try_from(flags).unwrap(),
            extra: (local_extra, extra),
            dos,
            data: content,
        });
    }
    entries
}

/// The files by name.
pub fn files(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    entries(bytes)
        .into_iter()
        .map(|entry| (entry.name, entry.data))
        .collect()
}

/// A file as text, which every XML file must be.
pub fn text<'a>(files: &'a BTreeMap<String, Vec<u8>>, name: &str) -> &'a str {
    std::str::from_utf8(files.get(name).unwrap_or_else(|| panic!("no {name}"))).unwrap()
}

/// `text` parsed as XML, a doctype allowed. Panics when it is not well-formed.
pub fn parse<'a>(name: &str, text: &'a str) -> roxmltree::Document<'a> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    roxmltree::Document::parse_with_options(text, options)
        .unwrap_or_else(|error| panic!("{name} is not well-formed: {error}\n{text}"))
}

pub const OPF: &str = "http://www.idpf.org/2007/opf";
pub const DC: &str = "http://purl.org/dc/elements/1.1/";
pub const XHTML: &str = "http://www.w3.org/1999/xhtml";
pub const OPS: &str = "http://www.idpf.org/2007/ops";
pub const CONTAINER: &str = "urn:oasis:names:tc:opendocument:xmlns:container";

/// What a book's package document says.
#[derive(Debug)]
pub struct Package {
    /// Where the package document is.
    pub path: String,
    pub identifier: String,
    pub title: String,
    pub language: String,
    pub modified: String,
    /// The manifest: id to (archive path, media type, properties).
    pub manifest: BTreeMap<String, (String, String, Option<String>)>,
    /// The spine's idrefs in order.
    pub spine: Vec<String>,
    /// The navigation document's archive path.
    pub nav: String,
}

/// `href`, relative to the folder of `base`, as an archive path.
pub fn resolve(base: &str, href: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop();
    for segment in href.split('#').next().unwrap_or_default().split('/') {
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

/// Every check of EPUB 3.3's container and package a reader makes before it opens a chapter
/// (PRD §11 test 1), returning the files and the package. Panics, saying why, on the first that
/// fails.
pub fn assert_valid_ocf(bytes: &[u8]) -> (BTreeMap<String, Vec<u8>>, Package) {
    // `mimetype` is the first local header, stored, with no extra field: its bytes sit at fixed
    // offsets, which is what lets a reader recognise the file.
    assert_eq!(&bytes[0..4], b"PK\x03\x04");
    assert_eq!(&bytes[8..10], &[0, 0], "mimetype is stored");
    assert_eq!(&bytes[28..30], &[0, 0], "mimetype has no extra field");
    assert_eq!(&bytes[30..38], b"mimetype");
    assert_eq!(&bytes[38..58], b"application/epub+zip");
    let entries = entries(bytes);
    let first = &entries[0];
    assert_eq!(first.name, "mimetype");
    assert_eq!(first.method, 0);
    assert_eq!(first.extra, (0, 0));
    assert_eq!(first.data, b"application/epub+zip");
    for entry in &entries {
        assert_eq!(entry.flags & 1, 0, "{} is not encrypted", entry.name);
        assert!(
            !entry.name.starts_with('/')
                && !entry
                    .name
                    .split('/')
                    .any(|segment| segment == ".." || segment.is_empty())
                && !entry.name.contains('\\'),
            "{} is a safe name",
            entry.name
        );
    }
    let files: BTreeMap<String, Vec<u8>> = entries
        .into_iter()
        .map(|entry| (entry.name, entry.data))
        .collect();
    for (name, data) in &files {
        if name.ends_with(".xml") || name.ends_with(".opf") || name.ends_with(".xhtml") {
            let text = std::str::from_utf8(data).unwrap_or_else(|_| panic!("{name} is UTF-8"));
            parse(name, text);
        }
    }

    let container = parse("container.xml", text(&files, "META-INF/container.xml"));
    let root = container.root_element();
    assert_eq!(root.tag_name().namespace(), Some(CONTAINER));
    let rootfile = root
        .descendants()
        .find(|node| node.has_tag_name((CONTAINER, "rootfile")))
        .expect("a rootfile");
    assert_eq!(
        rootfile.attribute("media-type"),
        Some("application/oebps-package+xml")
    );
    let path = rootfile
        .attribute("full-path")
        .expect("full-path")
        .to_owned();

    let opf_text = text(&files, &path);
    let opf = parse(&path, opf_text);
    let package = opf.root_element();
    assert!(package.has_tag_name((OPF, "package")));
    assert_eq!(package.attribute("version"), Some("3.0"));
    let unique = package
        .attribute("unique-identifier")
        .expect("unique-identifier");
    let metadata = package
        .children()
        .find(|node| node.has_tag_name((OPF, "metadata")))
        .expect("metadata");
    let dc = |name: &str| {
        metadata
            .children()
            .find(|node| node.has_tag_name((DC, name)))
            .and_then(|node| node.text())
            .map(str::to_owned)
    };
    let identifier_node = metadata
        .children()
        .find(|node| node.has_tag_name((DC, "identifier")) && node.attribute("id") == Some(unique))
        .expect("the unique identifier");
    let identifier = identifier_node.text().unwrap_or_default().to_owned();
    assert!(!identifier.trim().is_empty(), "the identifier is not empty");
    let title = dc("title").expect("dc:title");
    let language = dc("language").expect("dc:language");
    let modified: Vec<String> = metadata
        .children()
        .filter(|node| {
            node.has_tag_name((OPF, "meta"))
                && node.attribute("property") == Some("dcterms:modified")
        })
        .filter_map(|node| node.text().map(str::to_owned))
        .collect();
    assert_eq!(modified.len(), 1, "one dcterms:modified");
    let modified = modified[0].clone();
    assert!(
        modified.len() == 20
            && modified.ends_with('Z')
            && modified.as_bytes()[10] == b'T'
            && modified.as_bytes()[4] == b'-',
        "dcterms:modified is CCYY-MM-DDThh:mm:ssZ: {modified}"
    );

    let manifest_node = package
        .children()
        .find(|node| node.has_tag_name((OPF, "manifest")))
        .expect("manifest");
    let mut manifest = BTreeMap::new();
    let mut navs = Vec::new();
    for item in manifest_node
        .children()
        .filter(|node| node.has_tag_name((OPF, "item")))
    {
        let id = item.attribute("id").expect("an item's id").to_owned();
        let href = item.attribute("href").expect("an item's href");
        assert!(!href.contains(':'), "{href} is inside the book");
        let target = resolve(&path, href);
        assert!(
            files.contains_key(&target),
            "the manifest names {target}, which is missing"
        );
        let media = item
            .attribute("media-type")
            .expect("a media type")
            .to_owned();
        let properties = item.attribute("properties").map(str::to_owned);
        if properties
            .as_deref()
            .is_some_and(|properties| properties.split_whitespace().any(|p| p == "nav"))
        {
            navs.push(target.clone());
        }
        assert!(
            manifest
                .insert(id.clone(), (target, media, properties))
                .is_none(),
            "{id} is in the manifest twice"
        );
    }
    assert_eq!(navs.len(), 1, "exactly one navigation document");
    let spine: Vec<String> = package
        .children()
        .find(|node| node.has_tag_name((OPF, "spine")))
        .expect("spine")
        .children()
        .filter(|node| node.has_tag_name((OPF, "itemref")))
        .map(|node| node.attribute("idref").expect("an idref").to_owned())
        .collect();
    assert!(!spine.is_empty(), "the spine names a chapter");
    for idref in &spine {
        assert!(
            manifest.contains_key(idref),
            "the spine names {idref}, not in the manifest"
        );
    }
    // Every file but `mimetype`, the container and the package document is in the manifest.
    let listed: BTreeSet<&String> = manifest.values().map(|(target, _, _)| target).collect();
    for name in files.keys() {
        if name != "mimetype" && !name.starts_with("META-INF/") && *name != path {
            assert!(listed.contains(name), "{name} is not in the manifest");
        }
    }
    let nav = navs.remove(0);
    let nav_document = parse(&nav, text(&files, &nav));
    let tocs = nav_document
        .descendants()
        .filter(|node| {
            node.has_tag_name((XHTML, "nav")) && node.attribute((OPS, "type")) == Some("toc")
        })
        .count();
    assert_eq!(tocs, 1, "exactly one toc navigation");
    let package = Package {
        path,
        identifier,
        title,
        language,
        modified,
        manifest,
        spine,
        nav,
    };
    (files, package)
}

/// Whether the writer refuses a file `name` holding `data`: an unsafe name, or XML that is not
/// well-formed, either of which would make a broken archive.
pub fn refused(name: &str, data: &[u8]) -> bool {
    !crate::zip::safe_name(name) || crate::package::check_xml(name, data).is_err()
}
