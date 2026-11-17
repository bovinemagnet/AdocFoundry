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

/// An Ion value as read: symbols and field names by symbol id.
#[derive(Clone, Debug, PartialEq)]
pub enum Ion {
    Null,
    Bool(bool),
    Int(i64),
    /// The decimal's bytes, undecoded.
    Decimal(Vec<u8>),
    Symbol(u32),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<Ion>),
    Struct(Vec<(u32, Ion)>),
    Annotated(Vec<u32>, Box<Ion>),
}

impl Ion {
    /// The field `name` of a struct, through any annotations.
    #[must_use]
    pub fn field(&self, name: u32) -> Option<&Ion> {
        match self {
            Self::Struct(fields) => fields
                .iter()
                .find(|(found, _)| *found == name)
                .map(|(_, value)| value),
            Self::Annotated(_, inner) => inner.field(name),
            _ => None,
        }
    }

    #[must_use]
    pub fn list(&self) -> &[Ion] {
        match self {
            Self::List(items) => items,
            Self::Annotated(_, inner) => inner.list(),
            _ => &[],
        }
    }

    #[must_use]
    pub fn int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn symbol(&self) -> Option<u32> {
        match self {
            Self::Symbol(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

/// A `VarUInt` at the start of `bytes`: its value and length.
fn ion_var_uint(bytes: &[u8]) -> Result<(u64, usize), String> {
    let mut value = 0u64;
    for (length, byte) in bytes.iter().enumerate().take(10) {
        value = (value << 7) | u64::from(byte & 0x7F);
        if byte & 0x80 != 0 {
            return Ok((value, length + 1));
        }
    }
    Err("an unfinished VarUInt".to_owned())
}

fn ion_magnitude(bytes: &[u8]) -> Result<i64, String> {
    if bytes.len() > 8 {
        return Err("an integer too large".to_owned());
    }
    let mut value = 0u64;
    for byte in bytes {
        value = (value << 8) | u64::from(*byte);
    }
    i64::try_from(value).map_err(|_| "an integer too large".to_owned())
}

/// One Ion value at the start of `bytes`: the value and its length.
fn ion_value(bytes: &[u8], depth: usize) -> Result<(Ion, usize), String> {
    if depth > 100 {
        return Err("Ion nested too deeply".to_owned());
    }
    let descriptor = *bytes.first().ok_or("an Ion value cut short")?;
    let (kind, low) = (descriptor >> 4, descriptor & 0x0F);
    if kind == 1 {
        return Ok((
            if low == 0x0F {
                Ion::Null
            } else {
                Ion::Bool(low == 1)
            },
            1,
        ));
    }
    if low == 0x0F {
        return Ok((Ion::Null, 1));
    }
    let mut at = 1;
    let length = if low == 14 || (kind == 13 && low == 1) {
        let (length, used) = ion_var_uint(&bytes[1..])?;
        at += used;
        usize::try_from(length).map_err(|_| "an Ion length too large")?
    } else {
        usize::from(low)
    };
    let body = bytes.get(at..at + length).ok_or("an Ion value cut short")?;
    let value = match kind {
        0 => Ion::Null,
        2 => Ion::Int(ion_magnitude(body)?),
        3 => Ion::Int(-ion_magnitude(body)?),
        5 => Ion::Decimal(body.to_vec()),
        7 => Ion::Symbol(u32::try_from(ion_magnitude(body)?).map_err(|_| "a symbol too large")?),
        8 => Ion::String(String::from_utf8(body.to_vec()).map_err(|_| "a string not UTF-8")?),
        10 | 9 => Ion::Bytes(body.to_vec()),
        11 | 12 => {
            let mut items = Vec::new();
            let mut cursor = 0;
            while cursor < body.len() {
                let (item, used) = ion_value(&body[cursor..], depth + 1)?;
                cursor += used;
                items.push(item);
            }
            Ion::List(items)
        }
        13 => {
            let mut fields = Vec::new();
            let mut cursor = 0;
            while cursor < body.len() {
                let (name, used) = ion_var_uint(&body[cursor..])?;
                cursor += used;
                let (field, used) = ion_value(&body[cursor..], depth + 1)?;
                cursor += used;
                fields.push((
                    u32::try_from(name).map_err(|_| "a field name too large")?,
                    field,
                ));
            }
            Ion::Struct(fields)
        }
        14 => {
            let (names_length, used) = ion_var_uint(body)?;
            let names_end =
                used + usize::try_from(names_length).map_err(|_| "annotations too long")?;
            let mut names = Vec::new();
            let mut cursor = used;
            while cursor < names_end {
                let (name, used) =
                    ion_var_uint(body.get(cursor..names_end).ok_or("annotations cut short")?)?;
                cursor += used;
                names.push(u32::try_from(name).map_err(|_| "an annotation too large")?);
            }
            let (inner, _) = ion_value(
                body.get(names_end..)
                    .ok_or("an annotated value cut short")?,
                depth + 1,
            )?;
            Ion::Annotated(names, Box::new(inner))
        }
        other => return Err(format!("Ion type {other} is not read here")),
    };
    Ok((value, at + length))
}

/// An Ion stream: the version marker, then values.
pub fn read_ion(bytes: &[u8]) -> Result<Vec<Ion>, String> {
    if bytes.get(..4) != Some(&[0xE0, 0x01, 0x00, 0xEA]) {
        return Err("no Ion version marker".to_owned());
    }
    let mut values = Vec::new();
    let mut at = 4;
    while at < bytes.len() {
        let (value, used) = ion_value(&bytes[at..], 0)?;
        at += used;
        values.push(value);
    }
    Ok(values)
}

/// An entity of a KFX container: its id, its type, and its Ion value, or its bytes for raw
/// media.
#[derive(Debug)]
pub struct Entity {
    pub id: u32,
    pub kind: u32,
    pub value: Option<Ion>,
    pub raw: Vec<u8>,
}

/// A block of a storyline as read: its id, kind and text, and its links and looks as
/// `(start, length, what)`.
#[derive(Debug, Default)]
pub struct KfxBlock {
    pub eid: i64,
    /// `text`, `image` or `table`.
    pub kind: &'static str,
    pub text: String,
    pub links: Vec<(i64, i64, u32)>,
    pub styles: Vec<(i64, i64, u32)>,
    /// A picture's resource and alternative text.
    pub resource: Option<u32>,
    pub alt: Option<String>,
    /// A table's cells, row by row.
    pub rows: Vec<Vec<KfxBlock>>,
}

/// A KFX book as read.
#[derive(Debug)]
pub struct Kfx {
    pub version: u16,
    /// The `YJ_symbols` import's `max_id`.
    pub shared_max_id: u32,
    /// Local symbols' names, the first numbered `shared_max_id + 10`.
    pub symbols: Vec<String>,
    pub entities: Vec<Entity>,
}

impl Kfx {
    /// The name of symbol `id`: a local symbol's own, or `$<id>`.
    #[must_use]
    pub fn name(&self, id: u32) -> String {
        let first = self.shared_max_id + 10;
        match id.checked_sub(first) {
            Some(local) => self
                .symbols
                .get(local as usize)
                .cloned()
                .unwrap_or_else(|| format!("${id}")),
            None => format!("${id}"),
        }
    }

    /// The entities of type `kind`.
    pub fn of_kind(&self, kind: u32) -> impl Iterator<Item = &Entity> {
        self.entities
            .iter()
            .filter(move |entity| entity.kind == kind)
    }

    /// The entity named `id` of type `kind`.
    #[must_use]
    pub fn entity(&self, kind: u32, id: u32) -> Option<&Entity> {
        self.of_kind(kind).find(|entity| entity.id == id)
    }

    /// A title metadata value by key (`title`, `author`, `language`, …).
    #[must_use]
    pub fn metadata(&self, key: &str) -> Option<String> {
        let book = self.of_kind(490).next()?.value.as_ref()?;
        for category in book.field(491)?.list() {
            for pair in category.field(258).map(Ion::list).unwrap_or_default() {
                if pair.field(492).and_then(Ion::text) == Some(key) {
                    return match pair.field(307)? {
                        Ion::String(value) => Some(value.clone()),
                        Ion::Bool(value) => Some(value.to_string()),
                        _ => None,
                    };
                }
            }
        }
        None
    }

    /// The sections of the reading order, by symbol.
    #[must_use]
    pub fn reading_order(&self) -> Vec<u32> {
        self.of_kind(258)
            .next()
            .and_then(|entity| entity.value.as_ref())
            .and_then(|value| value.field(169))
            .and_then(|orders| orders.list().first())
            .and_then(|order| order.field(170))
            .map(|sections| sections.list().iter().filter_map(Ion::symbol).collect())
            .unwrap_or_default()
    }

    fn content(&self, reference: &Ion) -> Option<String> {
        let name = reference.field(4)?.symbol()?;
        let index = usize::try_from(reference.field(403)?.int()?).ok()?;
        let content = self.entity(145, name)?.value.as_ref()?;
        content
            .field(146)?
            .list()
            .get(index)?
            .text()
            .map(str::to_owned)
    }

    fn block(&self, value: &Ion) -> KfxBlock {
        let events = |field: u32| -> Vec<(i64, i64, u32)> {
            value
                .field(142)
                .map(Ion::list)
                .unwrap_or_default()
                .iter()
                .filter_map(|event| {
                    Some((
                        event.field(143)?.int()?,
                        event.field(144)?.int()?,
                        event.field(field)?.symbol()?,
                    ))
                })
                .collect()
        };
        let kind = match value.field(159).and_then(Ion::symbol) {
            Some(269) => "text",
            Some(271) => "image",
            Some(278) => "table",
            Some(279) => "row",
            _ => "other",
        };
        KfxBlock {
            eid: value.field(155).and_then(Ion::int).unwrap_or(-1),
            kind,
            text: value
                .field(145)
                .and_then(|reference| self.content(reference))
                .unwrap_or_default(),
            links: events(179),
            styles: events(157),
            resource: value.field(175).and_then(Ion::symbol),
            alt: value.field(584).and_then(Ion::text).map(str::to_owned),
            rows: value
                .field(146)
                .map(Ion::list)
                .unwrap_or_default()
                .iter()
                .map(|row| {
                    row.field(146)
                        .map(Ion::list)
                        .unwrap_or_default()
                        .iter()
                        .map(|cell| self.block(cell))
                        .collect()
                })
                .collect(),
        }
    }

    /// Each section of the reading order, through its page template's storyline, as blocks.
    pub fn chapters(&self) -> Result<Vec<Vec<KfxBlock>>, String> {
        let mut chapters = Vec::new();
        for section_name in self.reading_order() {
            let section = self
                .entity(260, section_name)
                .and_then(|entity| entity.value.as_ref())
                .ok_or_else(|| format!("no section {}", self.name(section_name)))?;
            let mut blocks = Vec::new();
            for template in section.field(141).map(Ion::list).unwrap_or_default() {
                let story = template
                    .field(176)
                    .and_then(Ion::symbol)
                    .ok_or("a template without a story")?;
                let storyline = self
                    .entity(259, story)
                    .and_then(|entity| entity.value.as_ref())
                    .ok_or_else(|| format!("no storyline {}", self.name(story)))?;
                for block in storyline.field(146).map(Ion::list).unwrap_or_default() {
                    blocks.push(self.block(block));
                }
            }
            chapters.push(blocks);
        }
        Ok(chapters)
    }

    /// Where anchor `name` leads: a block id and offset, or an address.
    #[must_use]
    pub fn anchor(&self, name: u32) -> Option<Result<(i64, i64), String>> {
        let anchor = self.entity(266, name)?.value.as_ref()?;
        if let Some(uri) = anchor.field(186).and_then(Ion::text) {
            return Some(Err(uri.to_owned()));
        }
        let position = anchor.field(183)?;
        Some(Ok((
            position.field(155)?.int()?,
            position.field(143)?.int()?,
        )))
    }

    /// The table of contents, depth first: each entry's label, depth and target.
    #[must_use]
    pub fn toc(&self) -> Vec<(String, usize, i64, i64)> {
        fn visit(units: &[Ion], depth: usize, out: &mut Vec<(String, usize, i64, i64)>) {
            for unit in units {
                let label = unit
                    .field(241)
                    .and_then(|representation| representation.field(244))
                    .and_then(Ion::text)
                    .unwrap_or_default()
                    .to_owned();
                let target = unit.field(246);
                let eid = target
                    .and_then(|target| target.field(155))
                    .and_then(Ion::int)
                    .unwrap_or(-1);
                let offset = target
                    .and_then(|target| target.field(143))
                    .and_then(Ion::int)
                    .unwrap_or(-1);
                out.push((label, depth, eid, offset));
                visit(
                    unit.field(247).map(Ion::list).unwrap_or_default(),
                    depth + 1,
                    out,
                );
            }
        }
        let mut out = Vec::new();
        for navigation in self.of_kind(389).filter_map(|entity| entity.value.as_ref()) {
            for order in navigation.list() {
                for container in order.field(392).map(Ion::list).unwrap_or_default() {
                    visit(
                        container.field(247).map(Ion::list).unwrap_or_default(),
                        0,
                        &mut out,
                    );
                }
            }
        }
        out
    }

    /// A picture's bytes by its resource symbol, through its external resource.
    #[must_use]
    pub fn picture(&self, resource: u32) -> Option<&[u8]> {
        let external = self.entity(164, resource)?.value.as_ref()?;
        let location = external.field(165)?.text()?;
        let raw = self.symbols.iter().position(|name| name == location)?;
        let raw = self.shared_max_id + 10 + u32::try_from(raw).ok()?;
        Some(&self.entity(417, raw)?.raw)
    }
}

fn le32(bytes: &[u8], at: usize) -> Result<usize, String> {
    bytes
        .get(at..at + 4)
        .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as usize)
        .ok_or_else(|| format!("no four bytes at {at}"))
}

fn le64(bytes: &[u8], at: usize) -> Result<usize, String> {
    let slice = bytes
        .get(at..at + 8)
        .ok_or_else(|| format!("no eight bytes at {at}"))?;
    let mut array = [0u8; 8];
    array.copy_from_slice(slice);
    usize::try_from(u64::from_le_bytes(array)).map_err(|_| "an offset too large".to_owned())
}

/// Reads a KFX container: the header, the container's description, the entity index, the
/// document's symbol table, and every entity, each its `ENTY` header then its Ion (or raw
/// media's bytes).
pub fn read_kfx(bytes: &[u8]) -> Result<Kfx, String> {
    if bytes.get(..4) != Some(b"CONT") {
        return Err("no CONT signature".to_owned());
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let header_length = le32(bytes, 6)?;
    let info_offset = le32(bytes, 10)?;
    let info_length = le32(bytes, 14)?;
    let info = read_ion(
        bytes
            .get(info_offset..info_offset + info_length)
            .ok_or("no description")?,
    )?;
    let info = info.first().ok_or("an empty description")?;
    let number = |field: u32| -> Result<usize, String> {
        info.field(field)
            .and_then(Ion::int)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| format!("the description has no ${field}"))
    };
    let (index_offset, index_length) = (number(413)?, number(414)?);
    let (symbols_offset, symbols_length) = (number(415)?, number(416)?);
    let table = read_ion(
        bytes
            .get(symbols_offset..symbols_offset + symbols_length)
            .ok_or("no symbol table")?,
    )?;
    let table = table.first().ok_or("an empty symbol table")?;
    let import = table
        .field(6)
        .and_then(|imports| imports.list().first())
        .ok_or("no import")?;
    if import.field(4).and_then(Ion::text) != Some("YJ_symbols") {
        return Err("the import is not YJ_symbols".to_owned());
    }
    let shared_max_id = import
        .field(8)
        .and_then(Ion::int)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("no max_id")?;
    let symbols = table
        .field(7)
        .map(Ion::list)
        .unwrap_or_default()
        .iter()
        .map(|symbol| {
            symbol
                .text()
                .map(str::to_owned)
                .ok_or("a symbol that is not text")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let payload = bytes.get(header_length..).ok_or("no payload")?;
    let mut entities = Vec::new();
    let index = bytes
        .get(index_offset..index_offset + index_length)
        .ok_or("no index")?;
    for entry in index.chunks(24) {
        let id = u32::try_from(le32(entry, 0)?).unwrap_or(u32::MAX);
        let kind = u32::try_from(le32(entry, 4)?).unwrap_or(u32::MAX);
        let (offset, length) = (le64(entry, 8)?, le64(entry, 16)?);
        let entity = payload
            .get(offset..offset + length)
            .ok_or("an entity outside the file")?;
        if !entity.starts_with(b"ENTY") {
            return Err("an entity without ENTY".to_owned());
        }
        let header = le32(entity, 6)?;
        let body = entity
            .get(header..)
            .ok_or("an entity header past its end")?;
        let (value, raw) = if kind == 417 || kind == 418 {
            (None, body.to_vec())
        } else {
            let mut values = read_ion(body)?;
            if values.len() != 1 {
                return Err(format!("entity ${kind} holds {} values", values.len()));
            }
            (values.pop(), Vec::new())
        };
        entities.push(Entity {
            id,
            kind,
            value,
            raw,
        });
    }
    Ok(Kfx {
        version,
        shared_max_id,
        symbols,
        entities,
    })
}
