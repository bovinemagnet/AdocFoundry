//! The ZIP container of a package, written in order to any [`Write`] sink: each entry
//! compressed in memory, then its local header and data, and at the end the central directory.
//! No seeking, no data descriptors and no 64-bit extensions; every entry is dated 1 January
//! 1980, the earliest date ZIP holds, so the same parts give the same bytes.

use std::io::Write;

use flate2::{write::DeflateEncoder, Compression};

use crate::DocxError;

/// MS-DOS time 00:00:00 and date 1980-01-01.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = (1 << 5) | 1;
/// The version needed to extract a deflated entry: 2.0.
const VERSION: u16 = 20;
const DEFLATED: u16 = 8;

struct Entry {
    name: String,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
}

pub(crate) struct ZipWriter<W: Write> {
    sink: W,
    written: u64,
    entries: Vec<Entry>,
}

fn small<T: TryFrom<u64>>(value: u64) -> Result<T, DocxError> {
    T::try_from(value).map_err(|_| DocxError::TooLarge)
}

impl<W: Write> ZipWriter<W> {
    pub(crate) fn new(sink: W) -> Self {
        Self {
            sink,
            written: 0,
            entries: Vec::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), DocxError> {
        self.sink.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Adds an entry `name` holding `data`, deflated.
    pub(crate) fn add(&mut self, name: &str, data: &[u8]) -> Result<(), DocxError> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data)?;
        let compressed = encoder.finish()?;
        let entry = Entry {
            name: name.to_owned(),
            crc: crc32fast::hash(data),
            compressed: small(compressed.len() as u64)?,
            size: small(data.len() as u64)?,
            offset: small(self.written)?,
        };
        let name_length: u16 = small(name.len() as u64)?;
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(b"PK\x03\x04");
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // flags
        header.extend_from_slice(&DEFLATED.to_le_bytes());
        header.extend_from_slice(&DOS_TIME.to_le_bytes());
        header.extend_from_slice(&DOS_DATE.to_le_bytes());
        header.extend_from_slice(&entry.crc.to_le_bytes());
        header.extend_from_slice(&entry.compressed.to_le_bytes());
        header.extend_from_slice(&entry.size.to_le_bytes());
        header.extend_from_slice(&name_length.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra field
        header.extend_from_slice(name.as_bytes());
        self.write(&header)?;
        self.write(&compressed)?;
        self.entries.push(entry);
        if self.entries.len() > usize::from(u16::MAX) {
            return Err(DocxError::TooLarge);
        }
        Ok(())
    }

    /// Writes the central directory and returns the sink.
    pub(crate) fn finish(mut self) -> Result<W, DocxError> {
        let start: u32 = small(self.written)?;
        let mut directory = Vec::new();
        for entry in &self.entries {
            directory.extend_from_slice(b"PK\x01\x02");
            directory.extend_from_slice(&VERSION.to_le_bytes()); // made by
            directory.extend_from_slice(&VERSION.to_le_bytes()); // needed
            directory.extend_from_slice(&0u16.to_le_bytes()); // flags
            directory.extend_from_slice(&DEFLATED.to_le_bytes());
            directory.extend_from_slice(&DOS_TIME.to_le_bytes());
            directory.extend_from_slice(&DOS_DATE.to_le_bytes());
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
        self.write(&directory)?;
        // The end of the central directory must fit where a 32-bit offset can say.
        small::<u32>(self.written)?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}
