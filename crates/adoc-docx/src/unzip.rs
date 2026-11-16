//! Reading a ZIP package someone else wrote: a Word template. Unlike `testing`'s reader, which
//! checks what this crate wrote, this one is for untrusted input: every offset is checked, the
//! sizes and CRC come from the central directory, and what an entry inflates to is capped, so a
//! package can neither panic the reader nor make it allocate past its limits.

use std::{collections::BTreeMap, fmt, io::Read};

/// The most entries a package may hold.
pub(crate) const MAX_ENTRIES: usize = 2_000;
/// The most all entries together may inflate to.
pub(crate) const MAX_INFLATED: u64 = 128 * 1024 * 1024;

/// Why a package could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnzipError {
    /// Not a ZIP file, or one whose structure is damaged.
    Damaged(String),
    /// Past a limit: too many entries, or too large inflated.
    TooLarge(String),
    /// Encrypted, ZIP64 or compressed other than by deflate.
    Unsupported(String),
}

impl fmt::Display for UnzipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Damaged(why) => write!(formatter, "it is not a readable ZIP package: {why}"),
            Self::TooLarge(why) => write!(formatter, "it is too large: {why}"),
            Self::Unsupported(why) => write!(formatter, "it uses {why}, which is not supported"),
        }
    }
}

fn damaged(why: impl Into<String>) -> UnzipError {
    UnzipError::Damaged(why.into())
}

fn slice(bytes: &[u8], at: usize, length: usize) -> Result<&[u8], UnzipError> {
    at.checked_add(length)
        .and_then(|end| bytes.get(at..end))
        .ok_or_else(|| damaged("a record runs past the end of the file"))
}

fn u16_at(bytes: &[u8], at: usize) -> Result<usize, UnzipError> {
    let raw = slice(bytes, at, 2)?;
    Ok(usize::from(u16::from_le_bytes([raw[0], raw[1]])))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, UnzipError> {
    let raw = slice(bytes, at, 4)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn offset(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// The end of central directory record: the last one in the file, which may end with a
/// comment of up to 65,535 bytes.
fn end_record(bytes: &[u8]) -> Result<usize, UnzipError> {
    let last = bytes
        .len()
        .checked_sub(22)
        .ok_or_else(|| damaged("it is shorter than a ZIP file can be"))?;
    let first = last.saturating_sub(65_535);
    (first..=last)
        .rev()
        .find(|&at| bytes[at..at + 4] == *b"PK\x05\x06")
        .ok_or_else(|| damaged("there is no end of central directory"))
}

/// The package's entries by name, in no particular order. Folders are left out; a name given
/// twice, ignoring case (as OPC part names compare), is refused.
pub(crate) fn entries(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, UnzipError> {
    let end = end_record(bytes)?;
    let count = u16_at(bytes, end + 10)?;
    let directory = u32_at(bytes, end + 16)?;
    if count == 0xFFFF || directory == u32::MAX {
        return Err(UnzipError::Unsupported("ZIP64".to_owned()));
    }
    if count > MAX_ENTRIES {
        return Err(UnzipError::TooLarge(format!(
            "{count} entries, past {MAX_ENTRIES}"
        )));
    }
    let mut at = offset(directory);
    let mut entries = BTreeMap::new();
    let mut names = std::collections::BTreeSet::new();
    let mut inflated_total: u64 = 0;
    for _ in 0..count {
        if slice(bytes, at, 4)? != b"PK\x01\x02" {
            return Err(damaged(
                "a central directory record is not where it should be",
            ));
        }
        let flags = u16_at(bytes, at + 8)?;
        let method = u16_at(bytes, at + 10)?;
        let crc = u32_at(bytes, at + 16)?;
        let compressed = u32_at(bytes, at + 20)?;
        let size = u32_at(bytes, at + 24)?;
        let name_length = u16_at(bytes, at + 28)?;
        let extra = u16_at(bytes, at + 30)?;
        let comment = u16_at(bytes, at + 32)?;
        let local = u32_at(bytes, at + 42)?;
        let name = String::from_utf8(slice(bytes, at + 46, name_length)?.to_vec())
            .map_err(|_| damaged("an entry's name is not UTF-8"))?;
        at += 46 + name_length + extra + comment;
        if flags & 1 != 0 {
            return Err(UnzipError::Unsupported("encryption".to_owned()));
        }
        if [compressed, size, local].contains(&u32::MAX) {
            return Err(UnzipError::Unsupported("ZIP64".to_owned()));
        }
        if name.ends_with('/') {
            continue;
        }
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(damaged(format!("{name} is in it twice")));
        }
        inflated_total += u64::from(size);
        if inflated_total > MAX_INFLATED {
            return Err(UnzipError::TooLarge(format!(
                "its entries inflate to more than {} MB",
                MAX_INFLATED / (1024 * 1024)
            )));
        }
        // The local header repeats the name and has an extra field of its own; the data
        // follows them. Its sizes and CRC may be zero (a data descriptor follows the data), so
        // the central directory's are the ones read.
        let local = offset(local);
        if slice(bytes, local, 4)? != b"PK\x03\x04" {
            return Err(damaged(format!("{name}'s local header is missing")));
        }
        let start = local + 30 + u16_at(bytes, local + 26)? + u16_at(bytes, local + 28)?;
        let data = slice(bytes, start, offset(compressed))?;
        let content = match method {
            0 => data.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(offset(size).min(1 << 20));
                // One byte more than declared shows a lie without reading all of it.
                flate2::read::DeflateDecoder::new(data)
                    .take(u64::from(size) + 1)
                    .read_to_end(&mut out)
                    .map_err(|error| damaged(format!("{name} does not inflate: {error}")))?;
                out
            }
            other => {
                return Err(UnzipError::Unsupported(format!(
                    "compression method {other}"
                )))
            }
        };
        if content.len() != offset(size) {
            return Err(damaged(format!("{name} is not the size it says")));
        }
        if crc32fast::hash(&content) != crc {
            return Err(damaged(format!("{name} fails its CRC check")));
        }
        entries.insert(name, content);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut sink = Vec::new();
        let mut writer = crate::zip::ZipWriter::new(&mut sink);
        for (name, data) in entries {
            writer.add(name, data).unwrap();
        }
        writer.finish().unwrap();
        sink
    }

    #[test]
    fn a_package_is_read_whole() {
        let bytes = zip(&[("a.xml", b"<a/>"), ("b/c.bin", &[7; 5000])]);
        let entries = entries(&bytes).unwrap();
        assert_eq!(entries["a.xml"], b"<a/>");
        assert_eq!(entries["b/c.bin"], vec![7; 5000]);
    }

    #[test]
    fn a_comment_after_the_directory_is_allowed() {
        let mut bytes = zip(&[("a.xml", b"<a/>")]);
        let end = bytes.len() - 22;
        bytes[end + 20..end + 22].copy_from_slice(&5u16.to_le_bytes());
        bytes.extend_from_slice(b"hello");
        assert_eq!(entries(&bytes).unwrap().len(), 1);
    }

    #[test]
    fn damage_is_an_error_and_never_a_panic() {
        let good = zip(&[("a.xml", b"<a/>"), ("b.xml", &[b'x'; 3000])]);
        // Every truncation, and every byte changed, reads or fails without panicking.
        for length in 0..good.len() {
            let _ = entries(&good[..length]);
        }
        for at in 0..good.len() {
            let mut bytes = good.clone();
            bytes[at] ^= 0xFF;
            let _ = entries(&bytes);
        }
        assert!(matches!(entries(b"not a zip"), Err(UnzipError::Damaged(_))));
    }

    #[test]
    fn what_it_will_not_read_is_refused() {
        let good = zip(&[("a.xml", b"<a/>")]);
        let directory =
            u32::from_le_bytes(good[good.len() - 6..good.len() - 2].try_into().unwrap()) as usize;
        // Encrypted.
        let mut bytes = good.clone();
        bytes[directory + 8] |= 1;
        assert_eq!(
            entries(&bytes),
            Err(UnzipError::Unsupported("encryption".to_owned()))
        );
        // Declared smaller than it inflates to.
        let mut bytes = good.clone();
        bytes[directory + 24..directory + 28].copy_from_slice(&2u32.to_le_bytes());
        assert!(matches!(entries(&bytes), Err(UnzipError::Damaged(_))));
        // A name twice, in another case.
        let twice = zip(&[("Word/a.xml", b"<a/>"), ("word/A.xml", b"<b/>")]);
        assert!(matches!(entries(&twice), Err(UnzipError::Damaged(_))));
        // Declared past the limit.
        let mut bytes = good;
        bytes[directory + 24..directory + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(entries(&bytes).is_err());
    }
}
