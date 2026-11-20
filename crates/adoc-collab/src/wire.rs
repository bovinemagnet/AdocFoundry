//! Checks on untrusted bytes before `yrs` reads them: the shapes a replica of this crate
//! produces, and nothing else.
//!
//! `yrs` 0.28.2 decodes an update into memory sized by the counts it claims: a hash table of
//! participants and of map entries, vectors of blocks and of values, each reserved before a
//! single entry is read. A hash table's reservation is touched as it is made, so six bytes can
//! cost gigabytes, and the decoder reaches such tables from content no shared text holds (maps,
//! arrays, JSON). The `collab_update` fuzz target found both. So every update is read here first
//! against the grammar of what a text replica sends (lib0 v1: participants, each a run of
//! blocks of string or deleted text, or garbage-collected or skipped ranges, parented by the one
//! root text; then the deletions), with every count bounded by the bytes left to hold it. Only
//! then does `yrs` decode it (`docs/design/DESIGN-AdocFoundry_Collaboration.md`, §6).

use crate::{CollabError, ReplicaId};

/// The root text's name, as `Replica` creates it.
const ROOT: &str = "text";

const GC: u8 = 0;
const DELETED: u8 = 1;
const STRING: u8 = 4;
const SKIP: u8 = 10;
const HAS_ORIGIN: u8 = 0b1000_0000;
const HAS_RIGHT_ORIGIN: u8 = 0b0100_0000;
const HAS_PARENT_SUB: u8 = 0b0010_0000;
const UNUSED: u8 = 0b0001_0000;

fn malformed(reason: &str) -> CollabError {
    CollabError::MalformedUpdate(reason.to_owned())
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn byte(&mut self) -> Result<u8, CollabError> {
        let byte = *self
            .bytes
            .get(self.at)
            .ok_or_else(|| malformed("ends early"))?;
        self.at += 1;
        Ok(byte)
    }

    /// A lib0 unsigned variable-length integer of at most 64 bits.
    fn var(&mut self) -> Result<u64, CollabError> {
        let mut value: u64 = 0;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            let part = u64::from(byte & 0x7f);
            if shift == 63 && part > 1 {
                return Err(malformed("an integer wider than 64 bits"));
            }
            value |= part << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(malformed("an integer wider than 64 bits"))
    }

    fn u32(&mut self) -> Result<u32, CollabError> {
        u32::try_from(self.var()?).map_err(|_| malformed("a number wider than 32 bits"))
    }

    /// A count of entries each at least `size` bytes long, so never more than the bytes left
    /// could hold.
    fn count(&mut self, size: usize) -> Result<usize, CollabError> {
        let count = self.var()?;
        let fits = count
            .checked_mul(size as u64)
            .is_some_and(|needed| needed <= self.left() as u64);
        if fits {
            Ok(count as usize)
        } else {
            Err(malformed("a count claiming more than follows"))
        }
    }

    fn client(&mut self) -> Result<u64, CollabError> {
        let client = self.var()?;
        ReplicaId::new(client)
            .map(ReplicaId::get)
            .ok_or_else(|| malformed("a participant id outside 1 to 2^53 - 1"))
    }

    fn id(&mut self) -> Result<(), CollabError> {
        self.client()?;
        self.u32()?;
        Ok(())
    }

    fn string(&mut self) -> Result<&'a str, CollabError> {
        let length = self.var()?;
        if length > self.left() as u64 {
            return Err(malformed("a string longer than what follows"));
        }
        let start = self.at;
        self.at += length as usize;
        std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| malformed("text not UTF-8"))
    }

    fn nonzero_length(&mut self) -> Result<(), CollabError> {
        if self.u32()? == 0 {
            Err(malformed("an empty block"))
        } else {
            Ok(())
        }
    }

    fn block(&mut self) -> Result<(), CollabError> {
        let info = self.byte()?;
        match info {
            SKIP | GC => return self.nonzero_length(),
            _ => {}
        }
        if info & (UNUSED | HAS_PARENT_SUB) != 0 {
            return Err(malformed("a block a shared text never holds"));
        }
        if info & HAS_ORIGIN != 0 {
            self.id()?;
        }
        if info & HAS_RIGHT_ORIGIN != 0 {
            self.id()?;
        }
        if info & (HAS_ORIGIN | HAS_RIGHT_ORIGIN) == 0 {
            // No neighbour to take the parent from: it must be the root text, by name.
            if self.var()? != 1 || self.string()? != ROOT {
                return Err(malformed("a block outside the shared text"));
            }
        }
        match info & 0b1111 {
            DELETED => self.nonzero_length(),
            STRING => {
                if self.string()?.is_empty() {
                    Err(malformed("an empty block"))
                } else {
                    Ok(())
                }
            }
            _ => Err(malformed("content a shared text never holds")),
        }
    }

    /// The deletions that end an update.
    fn deletions(&mut self) -> Result<(), CollabError> {
        for _ in 0..self.count(2)? {
            self.client()?;
            for _ in 0..self.count(2)? {
                self.u32()?;
                self.u32()?;
            }
        }
        Ok(())
    }

    fn end(&self) -> Result<(), CollabError> {
        if self.left() == 0 {
            Ok(())
        } else {
            Err(malformed("bytes after the end"))
        }
    }
}

/// Refuses an encoded update unless it has the shape a text replica sends, every count within
/// the bytes that follow it.
pub(crate) fn check_update_shape(bytes: &[u8]) -> Result<(), CollabError> {
    let mut reader = Reader { bytes, at: 0 };
    // Each participant: a count of blocks, an id and a clock.
    for _ in 0..reader.count(3)? {
        // Each block: at least its info byte and a length or a byte of content.
        let blocks = reader.count(2)?;
        reader.client()?;
        reader.u32()?;
        for _ in 0..blocks {
            reader.block()?;
        }
    }
    reader.deletions()?;
    reader.end()
}

/// Refuses an encoded state vector unless every count fits what follows and every id is one a
/// replica can have.
pub(crate) fn check_state_vector_shape(bytes: &[u8]) -> Result<(), CollabError> {
    let mut reader = Reader { bytes, at: 0 };
    for _ in 0..reader.count(2)? {
        reader.client()?;
        reader.u32()?;
    }
    reader.end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_must_fit_what_follows() {
        // No participants and no deletions: the empty update.
        assert!(check_update_shape(&[0x00, 0x00]).is_ok());
        assert!(check_update_shape(&[0x01, 0x00]).is_err());
        assert!(check_update_shape(&[0x97, 0xff, 0xff, 0xff, 0xff, 0x5b]).is_err());
        assert!(check_update_shape(&[]).is_err());
        assert!(check_update_shape(&[0xff; 12]).is_err());
        assert!(check_state_vector_shape(&[0x00]).is_ok());
        assert!(check_state_vector_shape(&[0x01, 0x05, 0x03]).is_ok());
        assert!(check_state_vector_shape(&[0x02, 0x05, 0x03]).is_err());
        // An id wider than 53 bits.
        assert!(check_state_vector_shape(&[
            0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0x00
        ])
        .is_err());
        // Something after the end.
        assert!(check_state_vector_shape(&[0x00, 0x00]).is_err());
    }
}
