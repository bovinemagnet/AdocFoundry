//! The OCF container: a ZIP archive written in order into memory, each entry's local header and
//! data, then the central directory. No seeking, no data descriptors, no extra fields and no
//! 64-bit extensions. The first entry, `mimetype`, is stored; the rest are deflated. Every entry
//! is dated with the book's modification time, so the same book gives the same bytes.

use std::io::Write;

use flate2::{write::DeflateEncoder, Compression};

use crate::{EpubError, Modified};

/// The version needed to extract a stored entry (1.0) and a deflated one (2.0).
const STORED_VERSION: u16 = 10;
const DEFLATED_VERSION: u16 = 20;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
/// The longest entry name written.
const NAME_LIMIT: usize = 240;

struct Entry {
    name: String,
    method: u16,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
}

pub(crate) struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<Entry>,
    time: u16,
    date: u16,
}

fn small<T: TryFrom<u64>>(value: u64) -> Result<T, EpubError> {
    T::try_from(value)
        .map_err(|_| EpubError::TooLarge("the book is past what a ZIP archive holds".to_owned()))
}

/// Whether `name` is a safe entry name: a relative path of ASCII letters, digits, `.`, `-` and
/// `_` in segments separated by `/`, none empty and none `.` or `..`.
pub(crate) fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_LIMIT
        && name.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
}

impl ZipWriter {
    pub(crate) fn new(modified: Modified) -> Self {
        let (time, date) = modified.dos();
        Self {
            out: Vec::new(),
            entries: Vec::new(),
            time,
            date,
        }
    }

    /// Adds `name` holding `data` as it is, uncompressed.
    pub(crate) fn add_stored(&mut self, name: &str, data: &[u8]) -> Result<(), EpubError> {
        self.add(name, data, data.to_vec(), STORED)
    }

    /// Adds `name` holding `data`, deflated.
    pub(crate) fn add_deflated(&mut self, name: &str, data: &[u8]) -> Result<(), EpubError> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(data)
            .and_then(|()| encoder.flush())
            .map_err(|error| {
                EpubError::Invalid(format!("{name} could not be compressed: {error}"))
            })?;
        let compressed = encoder.finish().map_err(|error| {
            EpubError::Invalid(format!("{name} could not be compressed: {error}"))
        })?;
        self.add(name, data, compressed, DEFLATED)
    }

    fn add(
        &mut self,
        name: &str,
        data: &[u8],
        stored: Vec<u8>,
        method: u16,
    ) -> Result<(), EpubError> {
        if !safe_name(name) {
            return Err(EpubError::Invalid(format!(
                "{name:?} is not a safe entry name"
            )));
        }
        if self.entries.iter().any(|entry| entry.name == name) {
            return Err(EpubError::Invalid(format!("{name} is in the book twice")));
        }
        let entry = Entry {
            name: name.to_owned(),
            method,
            crc: crc32fast::hash(data),
            compressed: small(stored.len() as u64)?,
            size: small(data.len() as u64)?,
            offset: small(self.out.len() as u64)?,
        };
        let name_length: u16 = small(name.len() as u64)?;
        let version = if method == STORED {
            STORED_VERSION
        } else {
            DEFLATED_VERSION
        };
        let out = &mut self.out;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // flags: no encryption, no descriptor
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&self.time.to_le_bytes());
        out.extend_from_slice(&self.date.to_le_bytes());
        out.extend_from_slice(&entry.crc.to_le_bytes());
        out.extend_from_slice(&entry.compressed.to_le_bytes());
        out.extend_from_slice(&entry.size.to_le_bytes());
        out.extend_from_slice(&name_length.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // no extra field
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&stored);
        self.entries.push(entry);
        if self.entries.len() > usize::from(u16::MAX) {
            return Err(EpubError::TooLarge(
                "the book has more files than a ZIP archive holds".to_owned(),
            ));
        }
        small::<u32>(self.out.len() as u64)?;
        Ok(())
    }

    /// Writes the central directory and returns the archive.
    pub(crate) fn finish(mut self) -> Result<Vec<u8>, EpubError> {
        let start: u32 = small(self.out.len() as u64)?;
        let mut directory = Vec::new();
        for entry in &self.entries {
            let version = if entry.method == STORED {
                STORED_VERSION
            } else {
                DEFLATED_VERSION
            };
            directory.extend_from_slice(b"PK\x01\x02");
            directory.extend_from_slice(&DEFLATED_VERSION.to_le_bytes()); // made by
            directory.extend_from_slice(&version.to_le_bytes()); // needed
            directory.extend_from_slice(&0u16.to_le_bytes()); // flags
            directory.extend_from_slice(&entry.method.to_le_bytes());
            directory.extend_from_slice(&self.time.to_le_bytes());
            directory.extend_from_slice(&self.date.to_le_bytes());
            directory.extend_from_slice(&entry.crc.to_le_bytes());
            directory.extend_from_slice(&entry.compressed.to_le_bytes());
            directory.extend_from_slice(&entry.size.to_le_bytes());
            directory.extend_from_slice(&small::<u16>(entry.name.len() as u64)?.to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes()); // extra field
            directory.extend_from_slice(&0u16.to_le_bytes()); // comment
            directory.extend_from_slice(&0u16.to_le_bytes()); // disk
            directory.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            directory.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            directory.extend_from_slice(&entry.offset.to_le_bytes());
            directory.extend_from_slice(entry.name.as_bytes());
        }
        let count: u16 = small(self.entries.len() as u64)?;
        let size: u32 = small(directory.len() as u64)?;
        directory.extend_from_slice(b"PK\x05\x06");
        directory.extend_from_slice(&0u16.to_le_bytes()); // this disk
        directory.extend_from_slice(&0u16.to_le_bytes()); // the directory's disk
        directory.extend_from_slice(&count.to_le_bytes());
        directory.extend_from_slice(&count.to_le_bytes());
        directory.extend_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&start.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes()); // comment
        self.out.extend_from_slice(&directory);
        small::<u32>(self.out.len() as u64)?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_names_are_safe() {
        for name in ["mimetype", "META-INF/container.xml", "EPUB/text/c1.xhtml"] {
            assert!(safe_name(name), "{name}");
        }
        for name in [
            "", "/abs", "a//b", "../up", "a/../b", "./a", "a\\b", "a b", "é", "a:b", "a/",
        ] {
            assert!(!safe_name(name), "{name:?}");
        }
        assert!(!safe_name(&"a".repeat(NAME_LIMIT + 1)));
    }

    #[test]
    fn an_unsafe_or_repeated_name_is_refused() {
        let modified = Modified::from_unix_seconds(1_700_000_000).unwrap();
        let mut zip = ZipWriter::new(modified);
        assert!(matches!(
            zip.add_deflated("../evil", b"x"),
            Err(EpubError::Invalid(_))
        ));
        zip.add_deflated("a", b"x").unwrap();
        assert!(matches!(
            zip.add_stored("a", b"y"),
            Err(EpubError::Invalid(_))
        ));
    }
}
