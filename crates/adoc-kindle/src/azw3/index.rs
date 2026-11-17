//! KF8 indexes (`INDX`): a main record with the tag table (`TAGX`) and one row per detail
//! record, the detail records holding the entries, and the strings the entries name kept apart
//! in `CNCX` records. The skeleton, fragment and NCX (table of contents) indexes are all of
//! this form; only their tags differ.

use crate::KindleError;

const HEADER_LENGTH: usize = 0xC0;
/// How large a detail record may grow: its row offsets are 16-bit.
const MAX_RECORD: usize = 0xFFFF;
/// The size of each `CNCX` record but the last.
const CNCX_PAGE: usize = 0x1_0000;
const MAX_CNCX_RECORD: usize = 0xFFFF;

/// A tag of an index: its number, how many values each occurrence has, and its bit mask in
/// an entry's control byte.
pub(super) type Tag = (u8, u8, u8);

pub(super) const SKELETON_TAGS: &[Tag] = &[(1, 1, 3), (6, 2, 12)];
pub(super) const FRAGMENT_TAGS: &[Tag] = &[(2, 1, 1), (3, 1, 2), (4, 1, 4), (6, 2, 8)];
pub(super) const NCX_TAGS: &[Tag] = &[
    (1, 1, 0x01),
    (2, 1, 0x02),
    (3, 1, 0x04),
    (4, 1, 0x08),
    (21, 1, 0x10),
    (22, 1, 0x20),
    (23, 1, 0x40),
    (6, 2, 0x80),
];

/// An entry: its name and the values of each tag it has.
pub(super) struct Entry {
    pub(super) name: Vec<u8>,
    pub(super) tags: Vec<(u8, Vec<u32>)>,
}

/// An index's records: the main record, its detail records and its `CNCX` records.
pub(super) struct Encoded {
    pub(super) records: Vec<Vec<u8>>,
}

fn too_large(what: &str) -> KindleError {
    KindleError::TooLarge {
        format: crate::KindleFormat::Azw3,
        reason: what.to_owned(),
    }
}

/// A forward-encoded variable-width integer: seven bits a byte, most significant first, the
/// last byte's high bit set.
pub(super) fn vwi(value: u32) -> Vec<u8> {
    let mut groups = vec![(value & 0x7F) as u8];
    let mut rest = value >> 7;
    while rest != 0 {
        groups.push((rest & 0x7F) as u8);
        rest >>= 7;
    }
    groups.reverse();
    if let Some(last) = groups.last_mut() {
        *last |= 0x80;
    }
    groups
}

/// Strings for `CNCX` records, each its length as a variable-width integer then its bytes,
/// with the offset each entry names it by.
pub(super) fn cncx(strings: &[Vec<u8>]) -> Result<(Vec<u32>, Vec<Vec<u8>>), KindleError> {
    let mut offsets = Vec::with_capacity(strings.len());
    let mut records: Vec<Vec<u8>> = vec![Vec::new()];
    for string in strings {
        let length = u32::try_from(string.len()).map_err(|_| too_large("a CNCX string"))?;
        let mut encoded = vwi(length);
        encoded.extend_from_slice(string);
        if encoded.len() > MAX_CNCX_RECORD {
            return Err(too_large("a table of contents label"));
        }
        if records.last().map_or(0, Vec::len) + encoded.len() > MAX_CNCX_RECORD {
            records.push(Vec::new());
        }
        let page = records.len() - 1;
        let within = records.last().map_or(0, Vec::len);
        let offset = page
            .checked_mul(CNCX_PAGE)
            .and_then(|base| base.checked_add(within))
            .and_then(|offset| u32::try_from(offset).ok())
            .ok_or_else(|| too_large("the CNCX records"))?;
        offsets.push(offset);
        if let Some(record) = records.last_mut() {
            record.extend_from_slice(&encoded);
        }
    }
    let last = records.len() - 1;
    for record in &mut records[..last] {
        record.resize(CNCX_PAGE, 0);
    }
    Ok((offsets, records))
}

/// Encodes `entries` with `tags`, naming `cncx_records` `CNCX` records after the details.
pub(super) fn encode(
    entries: &[Entry],
    tags: &[Tag],
    cncx_records: u32,
) -> Result<Encoded, KindleError> {
    let rows = entries
        .iter()
        .map(|entry| row(entry, tags))
        .collect::<Result<Vec<_>, _>>()?;
    // Detail records, each as many rows as fit with their offsets.
    let mut details: Vec<(usize, usize)> = Vec::new();
    let mut first = 0;
    while first < rows.len() {
        let mut count = 0;
        let mut bytes = 0;
        while first + count < rows.len() {
            let next = bytes + rows[first + count].len();
            if HEADER_LENGTH + next + 4 + 2 * (count + 1) > MAX_RECORD {
                break;
            }
            bytes = next;
            count += 1;
        }
        if count == 0 {
            return Err(too_large("an index entry"));
        }
        details.push((first, count));
        first += count;
    }
    let mut records = Vec::with_capacity(details.len() + 1);
    records.push(main(entries, tags, &details, cncx_records)?);
    for (first, count) in &details {
        records.push(detail(&rows[*first..first + count])?);
    }
    Ok(Encoded { records })
}

fn row(entry: &Entry, tags: &[Tag]) -> Result<Vec<u8>, KindleError> {
    let name_length = u8::try_from(entry.name.len()).map_err(|_| too_large("an index name"))?;
    let mut row = vec![name_length];
    row.extend_from_slice(&entry.name);
    let mut control = 0u8;
    for (tag, per, mask) in tags {
        if let Some((_, values)) = entry.tags.iter().find(|(found, _)| found == tag) {
            let occurrences = u8::try_from(values.len() / usize::from(*per))
                .map_err(|_| too_large("an index tag"))?;
            control |= (occurrences << mask.trailing_zeros()) & mask;
        }
    }
    row.push(control);
    for (tag, _, _) in tags {
        if let Some((_, values)) = entry.tags.iter().find(|(found, _)| found == tag) {
            for value in values {
                row.extend_from_slice(&vwi(*value));
            }
        }
    }
    Ok(row)
}

fn header(words: [u32; 13]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LENGTH);
    bytes.extend_from_slice(b"INDX");
    for word in words {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    bytes.resize(HEADER_LENGTH, 0);
    bytes
}

fn main(
    entries: &[Entry],
    tags: &[Tag],
    details: &[(usize, usize)],
    cncx_records: u32,
) -> Result<Vec<u8>, KindleError> {
    let count = |value: usize| u32::try_from(value).map_err(|_| too_large("an index"));
    let mut bytes = header([
        HEADER_LENGTH as u32,
        0,
        0,
        2,
        0,
        count(details.len())?,
        65001,
        u32::MAX,
        count(entries.len())?,
        0,
        0,
        0,
        cncx_records,
    ]);
    // The tag table follows the header.
    bytes[0xB4..0xB8].copy_from_slice(&(HEADER_LENGTH as u32).to_be_bytes());
    let tagx_length = 12 + 4 * (tags.len() + 1);
    bytes.extend_from_slice(b"TAGX");
    bytes.extend_from_slice(&count(tagx_length)?.to_be_bytes());
    bytes.extend_from_slice(&1u32.to_be_bytes());
    for (tag, per, mask) in tags {
        bytes.extend_from_slice(&[*tag, *per, *mask, 0]);
    }
    bytes.extend_from_slice(&[0, 0, 0, 1]);
    // One row per detail record: the name of its last entry and how many it holds.
    let mut positions = Vec::with_capacity(details.len());
    for (first, length) in details {
        let last = &entries[first + length - 1];
        positions.push(u16::try_from(bytes.len()).map_err(|_| too_large("an index"))?);
        bytes.push(u8::try_from(last.name.len()).map_err(|_| too_large("an index name"))?);
        bytes.extend_from_slice(&last.name);
        bytes.extend_from_slice(
            &u16::try_from(*length)
                .map_err(|_| too_large("an index"))?
                .to_be_bytes(),
        );
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
    }
    let idxt = count(bytes.len())?;
    bytes.extend_from_slice(b"IDXT");
    for position in positions {
        bytes.extend_from_slice(&position.to_be_bytes());
    }
    bytes[20..24].copy_from_slice(&idxt.to_be_bytes());
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    Ok(bytes)
}

fn detail(rows: &[Vec<u8>]) -> Result<Vec<u8>, KindleError> {
    let count = u32::try_from(rows.len()).map_err(|_| too_large("an index"))?;
    let mut bytes = header([
        HEADER_LENGTH as u32,
        0,
        1,
        0,
        0,
        count,
        u32::MAX,
        u32::MAX,
        0,
        0,
        0,
        0,
        0,
    ]);
    let mut positions = Vec::with_capacity(rows.len());
    for row in rows {
        positions.push(u16::try_from(bytes.len()).map_err(|_| too_large("an index"))?);
        bytes.extend_from_slice(row);
    }
    let idxt = u32::try_from(bytes.len()).map_err(|_| too_large("an index"))?;
    bytes.extend_from_slice(b"IDXT");
    for position in positions {
        bytes.extend_from_slice(&position.to_be_bytes());
    }
    bytes[20..24].copy_from_slice(&idxt.to_be_bytes());
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_width_integers_mark_their_last_byte() {
        assert_eq!(vwi(0), [0x80]);
        assert_eq!(vwi(0x7F), [0xFF]);
        assert_eq!(vwi(0x80), [0x01, 0x80]);
        assert_eq!(vwi(0x3FFF), [0x7F, 0xFF]);
    }

    #[test]
    fn cncx_strings_carry_their_lengths() {
        let (offsets, records) = cncx(&[b"One".to_vec(), b"Two".to_vec()]).unwrap();
        assert_eq!(offsets, [0, 4]);
        assert_eq!(records, [b"\x83One\x83Two".to_vec()]);
    }
}
