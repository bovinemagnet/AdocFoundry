//! AZW3: a KF8 book in a PalmDB/MOBI file, written in-house (Kindle PRD §6.5). The format
//! facts are from `epub3-kindle`'s MIT source, whose notice is `LICENSE-epub3-kindle` beside
//! this file, as `docs/qa/spike-kindle.md` records. KF8 only: there is no MOBI7 part.
//!
//! The text is one stream of markup: each chapter's skeleton followed by its fragments, then
//! the stylesheet. Record 0 holds the PalmDOC and MOBI headers and the EXTH metadata; then come
//! the text records, the fragment, skeleton and NCX indexes, the `FDST` that divides the text
//! into flows, the pictures, `RESC`, `FLIS`, `FCIS` and the end-of-file record. Every count and
//! offset is checked, and the finished file is read back ([`check`]) before it is returned.

use crate::{KindleBook, Target, TocEntry};
use crate::{KindleError, KindleFormat};

mod index;
mod xhtml;

/// Uncompressed text per record.
const RECORD_SIZE: usize = 4096;
/// A fragment's size past which the body's next element starts a new fragment.
const FRAGMENT_SIZE: usize = 8192;
/// Record 0's padding after the title, as KindleGen leaves it.
const RECORD_ZERO_PADDING: usize = 8192;
const EOF: &[u8] = b"\xE9\x8E\r\n";

fn invalid(reason: impl Into<String>) -> KindleError {
    KindleError::Invalid {
        format: KindleFormat::Azw3,
        reason: reason.into(),
    }
}

fn too_large(reason: impl Into<String>) -> KindleError {
    KindleError::TooLarge {
        format: KindleFormat::Azw3,
        reason: reason.into(),
    }
}

fn u32_of(value: usize, what: &str) -> Result<u32, KindleError> {
    u32::try_from(value).map_err(|_| too_large(what.to_owned()))
}

/// `value` in Kindle's base 32 (`0`–`9`, `A`–`V`), at least `width` digits.
pub(crate) fn base32(mut value: u32, width: usize) -> String {
    const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut digits = Vec::new();
    while value != 0 {
        digits.push(DIGITS[(value % 32) as usize]);
        value /= 32;
    }
    if digits.is_empty() {
        digits.push(b'0');
    }
    while digits.len() < width {
        digits.push(b'0');
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

/// A fragment as laid out: its chapter, sequence number, where it is inserted, where it
/// starts in its file's fragment stream, and its bytes' range in the chapter's body.
struct Fragment {
    file: usize,
    sequence: u32,
    insert: u32,
    start: u32,
    body: std::ops::Range<usize>,
}

/// A place resolved: the fragment's sequence number, the offset in it, and the offset in the
/// reassembled text.
#[derive(Clone, Copy)]
struct Place {
    fid: u32,
    offset: u32,
    position: u32,
}

pub(crate) fn write(book: &KindleBook) -> Result<Vec<u8>, KindleError> {
    let mut markups: Vec<xhtml::ChapterMarkup> = book
        .chapters
        .iter()
        .enumerate()
        .map(|(index, chapter)| xhtml::chapter(book, index, chapter))
        .collect();

    // Fragments: each chapter's body elements grouped up to FRAGMENT_SIZE.
    let mut fragments = Vec::new();
    let mut file_starts = Vec::with_capacity(markups.len());
    let mut position = 0usize;
    for (file, markup) in markups.iter().enumerate() {
        file_starts.push(position);
        let mut start = 0usize;
        let mut ends = markup.element_ends.clone();
        if ends.is_empty() || *ends.last().unwrap_or(&0) != markup.body.len() {
            ends.push(markup.body.len());
        }
        let mut from = 0usize;
        for (index, end) in ends.iter().enumerate() {
            let last = index + 1 == ends.len();
            let next_too_big = ends
                .get(index + 1)
                .is_some_and(|next| next - from > FRAGMENT_SIZE);
            if last || next_too_big {
                let insert = position + markup.prefix.len() + start;
                fragments.push(Fragment {
                    file,
                    sequence: u32_of(fragments.len(), "fragments")?,
                    insert: u32_of(insert, "the text")?,
                    start: u32_of(start, "a chapter")?,
                    body: from..*end,
                });
                start += end - from;
                from = *end;
            }
        }
        position += markup.prefix.len() + markup.body.len() + markup.suffix.len();
    }

    // Where every anchor, and every chapter's start, is.
    let prefixes: Vec<usize> = markups.iter().map(|markup| markup.prefix.len()).collect();
    let anchors: Vec<Vec<(String, usize)>> = markups
        .iter_mut()
        .map(|markup| std::mem::take(&mut markup.anchors))
        .collect();
    let links: Vec<Vec<(usize, Target)>> = markups
        .iter_mut()
        .map(|markup| std::mem::take(&mut markup.links))
        .collect();
    let place_in = |file: usize, body_offset: usize| -> Option<Place> {
        let fragment = fragments
            .iter()
            .find(|fragment| fragment.file == file && fragment.body.contains(&body_offset))
            .or_else(|| {
                fragments
                    .iter()
                    .rev()
                    .find(|fragment| fragment.file == file && fragment.body.end == body_offset)
            })?;
        let offset = body_offset - fragment.body.start;
        Some(Place {
            fid: fragment.sequence,
            offset: u32::try_from(offset).ok()?,
            position: u32::try_from(file_starts[file] + prefixes[file] + body_offset).ok()?,
        })
    };
    let resolve = |target: &Target| -> Option<Place> {
        let chapter_anchors = anchors.get(target.chapter)?;
        let body_offset = match &target.anchor {
            None => 0,
            Some(anchor) => chapter_anchors
                .iter()
                .find(|(name, _)| name == anchor)
                .map(|(_, offset)| *offset)?,
        };
        place_in(target.chapter, body_offset)
    };

    // Internal links, filled in place.
    let mut fills = Vec::new();
    for (file, file_links) in links.iter().enumerate() {
        for (at, target) in file_links {
            let place = resolve(target).ok_or_else(|| {
                invalid(format!(
                    "a link in chapter {} leads to an anchor the book does not have",
                    file + 1
                ))
            })?;
            let link = format!(
                "kindle:pos:fid:{}:off:{}",
                base32(place.fid, 4),
                base32(place.offset, 10)
            );
            if link.len() != xhtml::PLACEHOLDER.len() {
                return Err(too_large("a link's position"));
            }
            fills.push((file, *at, link));
        }
    }
    for (file, at, link) in fills {
        markups[file].body[at..at + link.len()].copy_from_slice(link.as_bytes());
    }

    // The text: skeletons and fragments, then the stylesheet.
    let mut text = Vec::with_capacity(position + xhtml::STYLESHEET.len());
    for (file, markup) in markups.iter().enumerate() {
        text.extend_from_slice(&markup.prefix);
        text.extend_from_slice(&markup.suffix);
        for fragment in fragments.iter().filter(|fragment| fragment.file == file) {
            text.extend_from_slice(&markup.body[fragment.body.clone()]);
        }
    }
    let html_end = text.len();
    text.extend_from_slice(xhtml::STYLESHEET.as_bytes());
    let text_length = u32_of(text.len(), "the text")?;

    // Indexes.
    let skeleton = index::encode(
        &markups
            .iter()
            .enumerate()
            .map(|(file, markup)| {
                let count = fragments
                    .iter()
                    .filter(|fragment| fragment.file == file)
                    .count();
                let count = u32_of(count, "fragments")?;
                let start = u32_of(file_starts[file], "the text")?;
                let length = u32_of(markup.prefix.len() + markup.suffix.len(), "a chapter")?;
                Ok(index::Entry {
                    name: format!("SKEL{file:010}").into_bytes(),
                    tags: vec![
                        (1, vec![count, count]),
                        (6, vec![start, length, start, length]),
                    ],
                })
            })
            .collect::<Result<Vec<_>, KindleError>>()?,
        index::SKELETON_TAGS,
        0,
    )?;
    let selectors: Vec<Vec<u8>> = fragments
        .iter()
        .map(|fragment| {
            format!(
                "P-//*[@aid='{}']",
                base32(u32_of(fragment.file, "files").unwrap_or(0), 0)
            )
            .into_bytes()
        })
        .collect();
    let (selector_offsets, fragment_cncx) = index::cncx(&selectors)?;
    let fragment_index = index::encode(
        &fragments
            .iter()
            .zip(&selector_offsets)
            .map(|(fragment, selector)| {
                Ok(index::Entry {
                    name: format!("{:010}", fragment.insert).into_bytes(),
                    tags: vec![
                        (2, vec![*selector]),
                        (3, vec![u32_of(fragment.file, "files")?]),
                        (4, vec![fragment.sequence]),
                        (
                            6,
                            vec![fragment.start, u32_of(fragment.body.len(), "a fragment")?],
                        ),
                    ],
                })
            })
            .collect::<Result<Vec<_>, KindleError>>()?,
        index::FRAGMENT_TAGS,
        u32_of(fragment_cncx.len(), "the fragment index")?,
    )?;
    let ncx = ncx(book, &resolve, u32_of(html_end, "the text")?)?;

    // Text records, each with its trailing multibyte entry: the bytes of a character the
    // record's end splits, then how many there are.
    let mut text_records = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let end = (at + RECORD_SIZE).min(text.len());
        let mut record = text[at..end].to_vec();
        let mut overlap = Vec::new();
        let mut cursor = end;
        // Continuation bytes after the boundary complete the record's last character.
        while cursor < text.len() && overlap.len() < 3 && text[cursor] & 0xC0 == 0x80 {
            overlap.push(text[cursor]);
            cursor += 1;
        }
        let overlap_length = overlap.len();
        record.extend_from_slice(&overlap);
        record.push(u8::try_from(overlap_length).unwrap_or(0));
        text_records.push(record);
        at = end;
    }
    let text_record_count = u16::try_from(text_records.len())
        .map_err(|_| too_large("more than 65,535 text records"))?;

    // Every record after record 0, in order.
    let mut records: Vec<Vec<u8>> = text_records;
    records.push(vec![0, 0]);
    let first_non_text = u32_of(records.len() + 1, "records")?;
    let fragment_record = first_non_text;
    records.extend(fragment_index.records);
    records.extend(fragment_cncx);
    let skeleton_record = u32_of(records.len() + 1, "records")?;
    records.extend(skeleton.records);
    let (ncx_record, ncx_records) = match ncx {
        Some(ncx) => (u32_of(records.len() + 1, "records")?, ncx),
        None => (u32::MAX, Vec::new()),
    };
    records.extend(ncx_records);
    let fdst_record = u32_of(records.len() + 1, "records")?;
    let mut fdst = Vec::with_capacity(28);
    fdst.extend_from_slice(b"FDST");
    fdst.extend_from_slice(&12u32.to_be_bytes());
    fdst.extend_from_slice(&2u32.to_be_bytes());
    for (start, end) in [(0, html_end), (html_end, text.len())] {
        fdst.extend_from_slice(&u32_of(start, "the text")?.to_be_bytes());
        fdst.extend_from_slice(&u32_of(end, "the text")?.to_be_bytes());
    }
    records.push(fdst);
    let (first_image, last_image) = if book.assets.is_empty() {
        (u32::MAX, u16::MAX)
    } else {
        let first = u32_of(records.len() + 1, "records")?;
        let last = u16::try_from(records.len() + book.assets.len())
            .map_err(|_| too_large("too many records for the pictures"))?;
        (first, last)
    };
    for asset in &book.assets {
        records.push(asset.bytes.clone());
    }
    records.push(resc(book)?);
    let flis_record = u32_of(records.len() + 1, "records")?;
    records.push(
        b"FLIS\x00\x00\x00\x08\x00\x41\x00\x00\x00\x00\x00\x00\xff\xff\xff\xff\x00\x01\x00\x03\x00\x00\x00\x03\x00\x00\x00\x01\xff\xff\xff\xff"
            .to_vec(),
    );
    let fcis_record = u32_of(records.len() + 1, "records")?;
    records.push(fcis(text_length));
    records.push(EOF.to_vec());

    // Record 0.
    let start_reading = resolve(&Target {
        chapter: 0,
        anchor: None,
    })
    .map(|place| place.position);
    let exth = exth(book, start_reading);
    let header = MobiHeader {
        first_non_text,
        first_image,
        last_image,
        fcis_record,
        flis_record,
        fdst_record,
        fragment_record,
        skeleton_record,
        ncx_record,
        language: language_code(&book.metadata.language),
    };
    let record0 = record_zero(
        text_length,
        text_record_count,
        &header,
        &exth,
        &book.metadata.title,
    )?;
    let mut all = Vec::with_capacity(records.len() + 1);
    all.push(record0);
    all.extend(records);
    let bytes = palm_database(&book.metadata.title, &all)?;
    check(&bytes)?;
    Ok(bytes)
}

/// The NCX index from the table of contents, or none for an empty one. Entries are in
/// reading order, depth first, each with its place, its range up to the next entry no deeper,
/// its label, depth, parent and children.
fn ncx(
    book: &KindleBook,
    resolve: &dyn Fn(&Target) -> Option<Place>,
    end: u32,
) -> Result<Option<Vec<Vec<u8>>>, KindleError> {
    struct Node {
        label: Vec<u8>,
        depth: u32,
        parent: Option<usize>,
        first_child: Option<usize>,
        last_child: Option<usize>,
        place: Place,
    }
    fn visit(
        entries: &[TocEntry],
        depth: u32,
        parent: Option<usize>,
        resolve: &dyn Fn(&Target) -> Option<Place>,
        out: &mut Vec<Node>,
    ) -> Result<(), KindleError> {
        for entry in entries {
            let place = resolve(&entry.target)
                .ok_or_else(|| invalid("a table of contents entry leads nowhere"))?;
            let index = out.len();
            let mut label = entry.title.clone();
            if label.trim().is_empty() {
                label = "—".to_owned();
            }
            out.push(Node {
                label: label.into_bytes(),
                depth,
                parent,
                first_child: None,
                last_child: None,
                place,
            });
            let first = out.len();
            visit(&entry.children, depth + 1, Some(index), resolve, out)?;
            if out.len() > first {
                out[index].first_child = Some(first);
                out[index].last_child = Some(out.len() - 1);
            }
        }
        Ok(())
    }
    let mut nodes = Vec::new();
    visit(&book.toc, 0, None, resolve, &mut nodes)?;
    if nodes.is_empty() {
        return Ok(None);
    }
    let labels: Vec<Vec<u8>> = nodes.iter().map(|node| node.label.clone()).collect();
    let (label_offsets, label_records) = index::cncx(&labels)?;
    let mut entries = Vec::with_capacity(nodes.len());
    for (number, node) in nodes.iter().enumerate() {
        let start = node.place.position.min(end);
        let finish = nodes[number + 1..]
            .iter()
            .find(|next| next.depth <= node.depth)
            .map_or(end, |next| next.place.position.min(end))
            .max(start);
        let mut tags = vec![
            (1, vec![start]),
            (2, vec![finish - start]),
            (3, vec![label_offsets[number]]),
            (4, vec![node.depth]),
        ];
        if let Some(parent) = node.parent {
            tags.push((21, vec![u32_of(parent, "the table of contents")?]));
        }
        if let (Some(first), Some(last)) = (node.first_child, node.last_child) {
            tags.push((22, vec![u32_of(first, "the table of contents")?]));
            tags.push((23, vec![u32_of(last, "the table of contents")?]));
        }
        tags.push((6, vec![node.place.fid, node.place.offset]));
        entries.push(index::Entry {
            name: number.to_string().into_bytes(),
            tags,
        });
    }
    let encoded = index::encode(
        &entries,
        index::NCX_TAGS,
        u32_of(label_records.len(), "the table of contents")?,
    )?;
    let mut records = encoded.records;
    records.extend(label_records);
    Ok(Some(records))
}

/// `RESC`: the spine, naming each chapter's skeleton.
fn resc(book: &KindleBook) -> Result<Vec<u8>, KindleError> {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\"><spine>",
    );
    for (index, chapter) in book.chapters.iter().enumerate() {
        xml.push_str("<itemref idref=\"");
        xhtml::escape_into(&mut xml, &chapter.id);
        xml.push_str(&format!("\" skelid=\"{index}\" linear=\"yes\"/>"));
    }
    xml.push_str("</spine></package>");
    let head = format!(
        "size={}&version=1&type=1",
        base32(u32_of(xml.len(), "the spine")?, 0)
    );
    let payload = head.len() + xml.len();
    let capacity = payload.div_ceil(4096) * 4096;
    let mut record = vec![0u8; 16 + capacity];
    record[0..4].copy_from_slice(b"RESC");
    record[4..8].copy_from_slice(&0x10u32.to_be_bytes());
    record[8..12].copy_from_slice(&1u32.to_be_bytes());
    record[12..16].copy_from_slice(&u32_of(head.len(), "the spine")?.to_be_bytes());
    record[16..16 + head.len()].copy_from_slice(head.as_bytes());
    record[16 + head.len()..16 + payload].copy_from_slice(xml.as_bytes());
    Ok(record)
}

fn fcis(text_length: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(52);
    bytes.extend_from_slice(b"FCIS");
    for word in [20u32, 16, 2, 0, text_length, 0, 0x28, 0, 0x28, 8] {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes
}

/// The EXTH metadata records.
fn exth(book: &KindleBook, start_reading: Option<u32>) -> Vec<(u32, Vec<u8>)> {
    let metadata = &book.metadata;
    let mut records: Vec<(u32, Vec<u8>)> = Vec::new();
    for author in &metadata.authors {
        records.push((100, author.as_bytes().to_vec()));
    }
    if let Some(publisher) = &metadata.publisher {
        records.push((101, publisher.as_bytes().to_vec()));
    }
    if let Some(description) = &metadata.description {
        records.push((103, description.as_bytes().to_vec()));
    }
    if let Some(date) = &metadata.date {
        records.push((106, date.as_bytes().to_vec()));
    }
    records.push((112, metadata.identifier.as_bytes().to_vec()));
    if let Some(start) = start_reading {
        records.push((116, start.to_be_bytes().to_vec()));
    }
    let resources = u32::try_from(book.assets.len()).unwrap_or(u32::MAX);
    records.push((125, resources.to_be_bytes().to_vec()));
    if let Some(cover) = book.cover {
        let cover = u32::try_from(cover).unwrap_or(u32::MAX);
        records.push((201, cover.to_be_bytes().to_vec()));
    }
    records.push((503, metadata.title.as_bytes().to_vec()));
    records.push((524, metadata.language.as_bytes().to_vec()));
    records.push((525, b"horizontal-lr".to_vec()));
    records.push((527, b"ltr".to_vec()));
    records
}

struct MobiHeader {
    first_non_text: u32,
    first_image: u32,
    last_image: u16,
    fcis_record: u32,
    flis_record: u32,
    fdst_record: u32,
    fragment_record: u32,
    skeleton_record: u32,
    ncx_record: u32,
    language: u32,
}

/// Record 0: the PalmDOC header, the MOBI header (264 bytes, version 8), EXTH, the full title
/// and KindleGen's padding.
fn record_zero(
    text_length: u32,
    text_records: u16,
    header: &MobiHeader,
    exth: &[(u32, Vec<u8>)],
    title: &str,
) -> Result<Vec<u8>, KindleError> {
    let mut exth_bytes = Vec::new();
    let exth_length = 12 + exth.iter().map(|(_, value)| 8 + value.len()).sum::<usize>();
    exth_bytes.extend_from_slice(b"EXTH");
    exth_bytes.extend_from_slice(&u32_of(exth_length, "the metadata")?.to_be_bytes());
    exth_bytes.extend_from_slice(&u32_of(exth.len(), "the metadata")?.to_be_bytes());
    for (kind, value) in exth {
        exth_bytes.extend_from_slice(&kind.to_be_bytes());
        exth_bytes.extend_from_slice(&u32_of(8 + value.len(), "the metadata")?.to_be_bytes());
        exth_bytes.extend_from_slice(value);
    }
    while !exth_bytes.len().is_multiple_of(4) {
        exth_bytes.push(0);
    }
    let title = title.as_bytes();
    let title_offset = 16 + 0x108 + exth_bytes.len();

    let mut mobi = vec![0u8; 0x108];
    let put32 = |bytes: &mut [u8], at: usize, value: u32| {
        bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
    };
    mobi[0..4].copy_from_slice(b"MOBI");
    put32(&mut mobi, 0x04, 0x108);
    put32(&mut mobi, 0x08, 2);
    put32(&mut mobi, 0x0C, 65001);
    put32(&mut mobi, 0x10, 1);
    put32(&mut mobi, 0x14, 8);
    mobi[0x18..0x40].fill(0xFF);
    put32(&mut mobi, 0x40, header.first_non_text);
    put32(&mut mobi, 0x44, u32_of(title_offset, "record 0")?);
    put32(&mut mobi, 0x48, u32_of(title.len(), "the title")?);
    put32(&mut mobi, 0x4C, header.language);
    put32(&mut mobi, 0x58, 8);
    put32(&mut mobi, 0x5C, header.first_image);
    put32(&mut mobi, 0x70, 0x50);
    put32(&mut mobi, 0x98, u32::MAX);
    mobi[0xAA..0xAC].copy_from_slice(&header.last_image.to_be_bytes());
    put32(&mut mobi, 0xB0, header.fdst_record);
    put32(&mut mobi, 0xB4, 2);
    put32(&mut mobi, 0xB8, header.fcis_record);
    put32(&mut mobi, 0xBC, 1);
    put32(&mut mobi, 0xC0, header.flis_record);
    put32(&mut mobi, 0xC4, 1);
    put32(&mut mobi, 0xD0, u32::MAX);
    put32(&mut mobi, 0xD4, 0);
    for at in [0xD8, 0xDC, 0xF0, 0xF8, 0x100] {
        put32(&mut mobi, at, u32::MAX);
    }
    // Trailing entries: the multibyte one only.
    mobi[0xE2..0xE4].copy_from_slice(&1u16.to_be_bytes());
    put32(&mut mobi, 0xE4, header.ncx_record);
    put32(&mut mobi, 0xE8, header.fragment_record);
    put32(&mut mobi, 0xEC, header.skeleton_record);
    put32(&mut mobi, 0xF4, u32::MAX);

    let mut record = Vec::with_capacity(title_offset + title.len() + 2 + RECORD_ZERO_PADDING + 4);
    // PalmDOC: no compression, the text's length, its records, their size, no encryption.
    record.extend_from_slice(&1u16.to_be_bytes());
    record.extend_from_slice(&0u16.to_be_bytes());
    record.extend_from_slice(&text_length.to_be_bytes());
    record.extend_from_slice(&text_records.to_be_bytes());
    record.extend_from_slice(&(RECORD_SIZE as u16).to_be_bytes());
    record.extend_from_slice(&0u16.to_be_bytes());
    record.extend_from_slice(&0u16.to_be_bytes());
    record.extend_from_slice(&mobi);
    record.extend_from_slice(&exth_bytes);
    record.extend_from_slice(title);
    record.extend_from_slice(&[0, 0]);
    while !record.len().is_multiple_of(4) {
        record.push(0);
    }
    record.resize(record.len() + RECORD_ZERO_PADDING, 0);
    Ok(record)
}

/// The PalmDB file: its 78-byte header, the record list and the records. The name is the
/// title's letters and digits, the dates zero so the same book gives the same bytes.
fn palm_database(title: &str, records: &[Vec<u8>]) -> Result<Vec<u8>, KindleError> {
    let count = u16::try_from(records.len()).map_err(|_| too_large("more than 65,535 records"))?;
    let mut name: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(31)
        .collect();
    if name.is_empty() {
        name.push_str("book");
    }
    let table = 78 + 8 * records.len() + 2;
    let total = table + records.iter().map(Vec::len).sum::<usize>();
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(name.as_bytes());
    bytes.resize(32, 0);
    bytes.resize(60, 0);
    bytes.extend_from_slice(b"BOOKMOBI");
    bytes.extend_from_slice(&(2 * u32::from(count)).saturating_sub(1).to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&count.to_be_bytes());
    let mut offset = table;
    for (index, record) in records.iter().enumerate() {
        bytes.extend_from_slice(&u32_of(offset, "the file")?.to_be_bytes());
        let unique = u32_of(2 * index, "records")?.to_be_bytes();
        bytes.push(0);
        bytes.extend_from_slice(&unique[1..]);
        offset += record.len();
    }
    bytes.extend_from_slice(&[0, 0]);
    for record in records {
        bytes.extend_from_slice(record);
    }
    Ok(bytes)
}

/// The language as a Windows locale identifier, for the MOBI header; 0 where it has none here.
fn language_code(language: &str) -> u32 {
    let mut parts = language.split(['-', '_']);
    let primary = parts.next().unwrap_or("").to_ascii_lowercase();
    let main = match primary.as_str() {
        "en" => 0x09,
        "de" => 0x07,
        "es" => 0x0A,
        "fr" => 0x0C,
        "it" => 0x10,
        "ja" => 0x11,
        "ko" => 0x12,
        "nl" => 0x13,
        "pt" => 0x16,
        "zh" => 0x04,
        _ => return 0,
    };
    let region = parts.next().map(str::to_ascii_lowercase);
    let sub = match (primary.as_str(), region.as_deref()) {
        ("en", Some("us")) => 0x04,
        ("en", Some("gb")) => 0x08,
        _ => 0,
    };
    main | (sub << 8)
}

/// Reads the file back: the PalmDB record list, record 0's headers and every record they
/// name, with the magic each must start with. A file that fails is never returned.
fn check(bytes: &[u8]) -> Result<(), KindleError> {
    let read32 = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    if bytes.get(60..68) != Some(b"BOOKMOBI") {
        return Err(invalid("the file is not a BOOKMOBI database"));
    }
    let count = usize::from(u16::from_be_bytes([bytes[76], bytes[77]]));
    let mut offsets = Vec::with_capacity(count + 1);
    for index in 0..count {
        offsets
            .push(read32(78 + 8 * index).ok_or_else(|| invalid("record list cut short"))? as usize);
    }
    offsets.push(bytes.len());
    if offsets.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err(invalid("records out of order"));
    }
    let record = |index: usize| -> Option<&[u8]> { bytes.get(offsets[index]..offsets[index + 1]) };
    let zero = record(0).ok_or_else(|| invalid("no record 0"))?;
    if zero.get(16..20) != Some(b"MOBI") || zero.get(36..40) != Some(&8u32.to_be_bytes()) {
        return Err(invalid("record 0 is not a version 8 MOBI header"));
    }
    let field = |at: usize| -> u32 {
        zero.get(16 + at..20 + at)
            .and_then(|slice| slice.try_into().ok())
            .map_or(u32::MAX, u32::from_be_bytes)
    };
    for (name, at, magic) in [
        ("fragment index", 0xE8, b"INDX"),
        ("skeleton index", 0xEC, b"INDX"),
        ("table of contents", 0xE4, b"INDX"),
        ("FDST", 0xB0, b"FDST"),
        ("FLIS", 0xC0, b"FLIS"),
        ("FCIS", 0xB8, b"FCIS"),
    ] {
        let pointer = field(at);
        if pointer == u32::MAX && name == "table of contents" {
            continue;
        }
        let found = (pointer as usize) < count
            && record(pointer as usize).is_some_and(|data| data.starts_with(magic));
        if !found {
            return Err(invalid(format!("the {name} pointer names no {name}")));
        }
    }
    let text_length = u32::from_be_bytes(zero[4..8].try_into().unwrap_or_default());
    let fdst = record(field(0xB0) as usize).unwrap_or_default();
    let flows = read_u32(fdst, 8).unwrap_or(0) as usize;
    let last_end = read_u32(fdst, 12 + 8 * flows.saturating_sub(1) + 4).unwrap_or(0);
    if flows == 0 || last_end != text_length {
        return Err(invalid("the FDST does not cover the text"));
    }
    if record(count - 1) != Some(EOF) {
        return Err(invalid("no end-of-file record"));
    }
    Ok(())
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_is_kindle_base32() {
        assert_eq!(base32(0, 4), "0000");
        assert_eq!(base32(31, 4), "000V");
        assert_eq!(base32(32, 0), "10");
        assert_eq!(base32(1, 10), "0000000001");
    }

    #[test]
    fn languages_are_locale_identifiers() {
        assert_eq!(language_code("en"), 0x09);
        assert_eq!(language_code("en-US"), 0x409);
        assert_eq!(language_code("ja"), 0x11);
        assert_eq!(language_code("xx"), 0);
    }
}
