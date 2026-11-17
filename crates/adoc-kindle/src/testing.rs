//! Readers for tests, written apart from the writers and sharing no code with them (Kindle PRD
//! §15, "Independent readback"): what a test reads back is what a reader would find, not what
//! the writer meant to write.
//!
//! [`read_azw3`] reads a KF8 file as a reader does: the PalmDB record list, record 0's headers
//! and EXTH, the text records with their trailing entries removed (and PalmDOC compression
//! undone), the `FDST` flows, the skeleton, fragment and NCX indexes with their `CNCX` strings,
//! the chapters reassembled from skeletons and fragments, and the pictures.

/// A KF8 book as read.
#[derive(Debug)]
pub struct Azw3 {
    /// The PalmDB type and creator, `BOOKMOBI` for a book.
    pub kind: [u8; 8],
    /// The MOBI header's file version, 8 for KF8.
    pub version: u32,
    /// The full title from record 0.
    pub title: String,
    /// EXTH records, in order.
    pub exth: Vec<(u32, Vec<u8>)>,
    /// The text as stored: skeletons, then fragments, then the other flows.
    pub text: Vec<u8>,
    /// Each flow's range of the text.
    pub flows: Vec<(usize, usize)>,
    /// Each chapter reassembled from its skeleton and fragments.
    pub files: Vec<String>,
    /// Each fragment: its sequence number, file and where it starts in the reassembled file.
    pub fragments: Vec<(u32, usize, usize)>,
    pub ncx: Vec<NcxEntry>,
    /// The pictures, from the first image record to the last.
    pub images: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NcxEntry {
    pub label: String,
    pub depth: u32,
    pub parent: Option<u32>,
    /// The fragment's sequence number and the offset in it.
    pub fid: u32,
    pub offset: u32,
}

impl Azw3 {
    /// The text of every EXTH record of `kind`.
    #[must_use]
    pub fn exth_text(&self, kind: u32) -> Vec<String> {
        self.exth
            .iter()
            .filter(|(found, _)| *found == kind)
            .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
            .collect()
    }

    /// The EXTH record of `kind` as a number.
    #[must_use]
    pub fn exth_number(&self, kind: u32) -> Option<u32> {
        self.exth
            .iter()
            .find(|(found, _)| *found == kind)
            .and_then(|(_, value)| Some(u32::from_be_bytes(value.as_slice().try_into().ok()?)))
    }

    /// Where `kindle:pos:fid:<fid>:off:<offset>` leads: a file and an offset in it.
    #[must_use]
    pub fn position(&self, fid: u32, offset: u32) -> Option<(usize, usize)> {
        let (_, file, start) = self
            .fragments
            .iter()
            .find(|(sequence, _, _)| *sequence == fid)?;
        Some((*file, start + offset as usize))
    }

    /// Every `href` of every file, with the file it is in.
    #[must_use]
    pub fn hrefs(&self) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (index, file) in self.files.iter().enumerate() {
            let mut rest = file.as_str();
            while let Some(at) = rest.find("href=\"") {
                rest = &rest[at + 6..];
                let end = rest.find('"').unwrap_or(rest.len());
                out.push((index, rest[..end].to_owned()));
                rest = &rest[end..];
            }
        }
        out
    }

    /// Where an internal link leads, if it is one and leads somewhere: the file and the text
    /// from there.
    #[must_use]
    pub fn follow(&self, href: &str) -> Option<(usize, &str)> {
        let rest = href.strip_prefix("kindle:pos:fid:")?;
        let (fid, offset) = rest.split_once(":off:")?;
        let (file, at) = self.position(base32(fid)?, base32(offset)?)?;
        Some((file, self.files.get(file)?.get(at..)?))
    }

    /// The text of every file with the markup removed.
    #[must_use]
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for file in &self.files {
            let body = file
                .split_once("<body")
                .map_or(file.as_str(), |(_, body)| body);
            let mut inside = true;
            for character in body.chars() {
                match character {
                    '<' => inside = true,
                    '>' => inside = false,
                    c if !inside => out.push(c),
                    _ => {}
                }
            }
            out.push('\n');
        }
        out.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&amp;", "&")
    }
}

fn base32(text: &str) -> Option<u32> {
    text.chars().try_fold(0u32, |value, character| {
        let digit = character.to_digit(32)?;
        value.checked_mul(32)?.checked_add(digit)
    })
}

fn be32(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|slice| u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
        .ok_or_else(|| format!("no four bytes at {at}"))
}

fn be16(bytes: &[u8], at: usize) -> Result<u16, String> {
    bytes
        .get(at..at + 2)
        .map(|slice| u16::from_be_bytes([slice[0], slice[1]]))
        .ok_or_else(|| format!("no two bytes at {at}"))
}

/// A forward variable-width integer at `at`: its value and length.
fn forward_vwi(bytes: &[u8], at: usize) -> Result<(u32, usize), String> {
    let mut value = 0u32;
    for (length, byte) in bytes.get(at..).unwrap_or_default().iter().enumerate() {
        value = value.checked_shl(7).ok_or("an integer too large")? | u32::from(byte & 0x7F);
        if byte & 0x80 != 0 {
            return Ok((value, length + 1));
        }
        if length >= 4 {
            break;
        }
    }
    Err(format!("an unfinished integer at {at}"))
}

/// The size a trailing entry gives at the end of `data`, read backwards.
fn backward_vwi(data: &[u8]) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    for byte in data.iter().rev().take(4) {
        value |= usize::from(byte & 0x7F) << shift;
        if byte & 0x80 != 0 {
            break;
        }
        shift += 7;
    }
    value
}

fn palmdoc_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(data.len() * 2);
    let mut at = 0;
    while at < data.len() {
        let byte = data[at];
        at += 1;
        match byte {
            0x00 | 0x09..=0x7F => out.push(byte),
            0x01..=0x08 => {
                let end = at + usize::from(byte);
                out.extend_from_slice(data.get(at..end).ok_or("a literal cut short")?);
                at = end;
            }
            0x80..=0xBF => {
                let next = *data.get(at).ok_or("a reference cut short")?;
                at += 1;
                let pair = (u16::from(byte) << 8 | u16::from(next)) & 0x3FFF;
                let distance = usize::from(pair >> 3);
                let length = usize::from(pair & 7) + 3;
                if distance == 0 || distance > out.len() {
                    return Err("a reference before the start".to_owned());
                }
                for _ in 0..length {
                    out.push(out[out.len() - distance]);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(byte ^ 0x80);
            }
        }
    }
    Ok(out)
}

/// An index entry's tags and their values.
type Tags = Vec<(u8, Vec<u32>)>;

/// An index as read: each entry's name and tag values, and its `CNCX` strings by offset.
struct Index {
    entries: Vec<(String, Tags)>,
    cncx: Vec<u8>,
}

impl Index {
    fn string(&self, offset: u32) -> Result<String, String> {
        let page = (offset / 0x1_0000) as usize;
        let within = (offset % 0x1_0000) as usize;
        let at = page * 0x1_0000 + within;
        let (length, used) = forward_vwi(&self.cncx, at)?;
        let start = at + used;
        let bytes = self
            .cncx
            .get(start..start + length as usize)
            .ok_or("a CNCX string cut short")?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn tag(entry: &[(u8, Vec<u32>)], wanted: u8) -> Option<&[u32]> {
    entry
        .iter()
        .find(|(found, _)| *found == wanted)
        .map(|(_, values)| values.as_slice())
}

fn read_index(records: &[&[u8]], first: usize) -> Result<Index, String> {
    let main = records.get(first).ok_or("no index record")?;
    if !main.starts_with(b"INDX") {
        return Err("not an INDX record".to_owned());
    }
    let header_length = be32(main, 4)? as usize;
    let details = be32(main, 24)? as usize;
    let cncx_count = be32(main, 52)? as usize;
    let tagx = main.get(header_length..).ok_or("no TAGX")?;
    if !tagx.starts_with(b"TAGX") {
        return Err("no TAGX".to_owned());
    }
    let tagx_length = be32(tagx, 4)? as usize;
    let control_bytes = be32(tagx, 8)? as usize;
    let mut tags = Vec::new();
    let mut at = 12;
    while at + 4 <= tagx_length {
        tags.push((tagx[at], tagx[at + 1], tagx[at + 2], tagx[at + 3]));
        at += 4;
    }
    let mut entries = Vec::new();
    for detail in records
        .get(first + 1..first + 1 + details)
        .ok_or("detail records missing")?
    {
        let idxt = be32(detail, 20)? as usize;
        let count = be32(detail, 24)? as usize;
        if detail.get(idxt..idxt + 4) != Some(b"IDXT") {
            return Err("no IDXT".to_owned());
        }
        for entry_number in 0..count {
            let position = usize::from(be16(detail, idxt + 4 + 2 * entry_number)?);
            let name_length = usize::from(*detail.get(position).ok_or("an entry past the end")?);
            let name = detail
                .get(position + 1..position + 1 + name_length)
                .ok_or("a name cut short")?;
            let mut cursor = position + 1 + name_length;
            let controls = detail
                .get(cursor..cursor + control_bytes)
                .ok_or("control bytes cut short")?
                .to_vec();
            cursor += control_bytes;
            let mut values = Vec::new();
            let mut control_index = 0;
            for (number, per, mask, end) in &tags {
                if *end == 1 {
                    control_index += 1;
                    continue;
                }
                let control = controls.get(control_index).copied().unwrap_or(0);
                let shifted = (control & mask) >> mask.trailing_zeros();
                let mut occurrences = usize::from(shifted);
                if mask.count_ones() > 1 && shifted == mask >> mask.trailing_zeros() {
                    let (count, used) = forward_vwi(detail, cursor)?;
                    cursor += used;
                    occurrences = count as usize;
                }
                if occurrences == 0 {
                    continue;
                }
                let mut found = Vec::new();
                for _ in 0..occurrences * usize::from(*per) {
                    let (value, used) = forward_vwi(detail, cursor)?;
                    cursor += used;
                    found.push(value);
                }
                values.push((*number, found));
            }
            entries.push((String::from_utf8_lossy(name).into_owned(), values));
        }
    }
    let mut cncx = Vec::new();
    for record in records
        .get(first + 1 + details..first + 1 + details + cncx_count)
        .ok_or("CNCX records missing")?
    {
        cncx.extend_from_slice(record);
    }
    Ok(Index { entries, cncx })
}

/// Reads a KF8 book.
pub fn read_azw3(bytes: &[u8]) -> Result<Azw3, String> {
    let mut kind = [0u8; 8];
    kind.copy_from_slice(bytes.get(60..68).ok_or("no PalmDB header")?);
    let count = usize::from(be16(bytes, 76)?);
    let mut offsets = (0..count)
        .map(|index| be32(bytes, 78 + 8 * index).map(|offset| offset as usize))
        .collect::<Result<Vec<_>, _>>()?;
    offsets.push(bytes.len());
    let records: Vec<&[u8]> = offsets
        .windows(2)
        .map(|pair| {
            bytes
                .get(pair[0]..pair[1])
                .ok_or("a record outside the file")
        })
        .collect::<Result<_, _>>()?;
    let zero = records.first().ok_or("no record 0")?;
    let compression = be16(zero, 0)?;
    let text_length = be32(zero, 4)? as usize;
    let text_records = usize::from(be16(zero, 8)?);
    let mobi = zero.get(16..).ok_or("no MOBI header")?;
    if !mobi.starts_with(b"MOBI") {
        return Err("no MOBI header".to_owned());
    }
    let mobi_length = be32(mobi, 4)? as usize;
    let version = be32(mobi, 0x14)?;
    let title_offset = be32(mobi, 0x44)? as usize;
    let title_length = be32(mobi, 0x48)? as usize;
    let title = String::from_utf8_lossy(
        zero.get(title_offset..title_offset + title_length)
            .ok_or("the title outside record 0")?,
    )
    .into_owned();
    let first_image = be32(mobi, 0x5C)?;
    let last_image = be16(mobi, 0xAA)?;
    let fdst_record = be32(mobi, 0xB0)? as usize;
    let extra_flags = be16(mobi, 0xE2)?;
    let ncx_record = be32(mobi, 0xE4)?;
    let fragment_record = be32(mobi, 0xE8)? as usize;
    let skeleton_record = be32(mobi, 0xEC)? as usize;

    let mut exth = Vec::new();
    if be32(mobi, 0x70)? & 0x40 != 0 {
        let exth_bytes = mobi.get(mobi_length..).ok_or("no EXTH")?;
        if !exth_bytes.starts_with(b"EXTH") {
            return Err("no EXTH".to_owned());
        }
        let records_count = be32(exth_bytes, 8)?;
        let mut at = 12;
        for _ in 0..records_count {
            let kind = be32(exth_bytes, at)?;
            let length = be32(exth_bytes, at + 4)? as usize;
            let value = exth_bytes
                .get(at + 8..at + length)
                .ok_or("an EXTH record cut short")?;
            exth.push((kind, value.to_vec()));
            at += length;
        }
    }

    let mut text = Vec::with_capacity(text_length);
    for record in records
        .get(1..=text_records)
        .ok_or("text records missing")?
    {
        let mut data: &[u8] = record;
        for bit in (1..16).rev() {
            if extra_flags & (1 << bit) != 0 {
                let size = backward_vwi(data);
                data = &data[..data.len().saturating_sub(size)];
            }
        }
        if extra_flags & 1 != 0 {
            let size = usize::from(data.last().copied().unwrap_or(0) & 3) + 1;
            data = &data[..data.len().saturating_sub(size)];
        }
        match compression {
            1 => text.extend_from_slice(data),
            2 => text.extend(palmdoc_decompress(data)?),
            other => return Err(format!("compression {other} is not read here")),
        }
    }
    if text.len() != text_length {
        return Err(format!(
            "the text records hold {} bytes, not the {text_length} the header gives",
            text.len()
        ));
    }

    let fdst = records.get(fdst_record).ok_or("no FDST")?;
    if !fdst.starts_with(b"FDST") {
        return Err("no FDST".to_owned());
    }
    let flows = (0..be32(fdst, 8)? as usize)
        .map(|index| {
            Ok((
                be32(fdst, 12 + 8 * index)? as usize,
                be32(fdst, 16 + 8 * index)? as usize,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;

    // Chapters reassembled as a reader does: each skeleton, with its fragments inserted in turn.
    let skeletons = read_index(&records, skeleton_record)?;
    let fragment_index = read_index(&records, fragment_record)?;
    let mut fragment_entries = fragment_index.entries.iter();
    let mut files = Vec::new();
    let mut fragments = Vec::new();
    for (file, (_, skeleton)) in skeletons.entries.iter().enumerate() {
        let count = tag(skeleton, 1)
            .and_then(|values| values.first())
            .copied()
            .ok_or("a skeleton without a count")? as usize;
        let range = tag(skeleton, 6).ok_or("a skeleton without a range")?;
        let (start, length) = (range[0] as usize, range[1] as usize);
        let mut assembled = text
            .get(start..start + length)
            .ok_or("a skeleton outside the text")?
            .to_vec();
        let mut cursor = start + length;
        for _ in 0..count {
            let (name, fragment) = fragment_entries.next().ok_or("too few fragments")?;
            let insert: usize = name.parse().map_err(|_| "a fragment name not a number")?;
            let sequence = tag(fragment, 4)
                .and_then(|values| values.first())
                .copied()
                .ok_or("a fragment without a sequence")?;
            let geometry = tag(fragment, 6).ok_or("a fragment without a range")?;
            let length = geometry[1] as usize;
            let payload = text
                .get(cursor..cursor + length)
                .ok_or("a fragment outside the text")?;
            let at = insert
                .checked_sub(start)
                .ok_or("a fragment before its skeleton")?;
            if at > assembled.len() {
                return Err("a fragment inserted past its skeleton".to_owned());
            }
            assembled.splice(at..at, payload.iter().copied());
            fragments.push((sequence, file, at));
            cursor += length;
        }
        files.push(String::from_utf8(assembled).map_err(|_| "a chapter that is not UTF-8")?);
    }

    let mut ncx = Vec::new();
    if ncx_record != u32::MAX {
        let index = read_index(&records, ncx_record as usize)?;
        for (_, entry) in &index.entries {
            let label = tag(entry, 3)
                .and_then(|values| values.first())
                .copied()
                .ok_or("an NCX entry without a label")?;
            let position = tag(entry, 6).ok_or("an NCX entry without a place")?;
            ncx.push(NcxEntry {
                label: index.string(label)?,
                depth: tag(entry, 4)
                    .and_then(|values| values.first())
                    .copied()
                    .unwrap_or(0),
                parent: tag(entry, 21).and_then(|values| values.first()).copied(),
                fid: position[0],
                offset: position[1],
            });
        }
    }

    let images = if first_image == u32::MAX {
        Vec::new()
    } else {
        records
            .get(first_image as usize..=usize::from(last_image))
            .ok_or("pictures outside the file")?
            .iter()
            .map(|record| record.to_vec())
            .collect()
    };

    Ok(Azw3 {
        kind,
        version,
        title,
        exth,
        text,
        flows,
        files,
        fragments,
        ncx,
        images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palmdoc_references_copy_earlier_text() {
        // "abcabc": three literals then a reference three back, three long.
        let data = [b'a', b'b', b'c', 0x80, 0x18];
        assert_eq!(palmdoc_decompress(&data).unwrap(), b"abcabc");
        assert_eq!(palmdoc_decompress(&[0xC1]).unwrap(), b" A");
    }

    #[test]
    fn integers_read_both_ways() {
        assert_eq!(forward_vwi(&[0x01, 0x80], 0).unwrap(), (0x80, 2));
        assert_eq!(backward_vwi(&[0x81, 0x05]), 0x05 | (1 << 7));
        assert_eq!(base32("0V"), Some(31));
    }
}
