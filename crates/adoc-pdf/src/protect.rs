//! Read-only PDFs, READONLY 4/5 (`docs/prd/PRD-AdocFoundry_READONLY.md` §5.4): the finished PDF
//! re-encrypted in memory with the Standard Security Handler, AES-256 (version 5, revision 6),
//! so a reader opens it with no password but refuses to modify it.
//!
//! The file key and the owner password are random and never kept, so nothing secret is asked
//! for, stored or shown: the restriction is a deterrent, as Word's protection without a
//! password is. Reading, copying, printing and the permission assistive technology needs are
//! kept; modifying, annotating, filling forms and assembling pages are not. The PDF's version
//! (1.7) declares Adobe's extension level 8, which brings AES-256. Nothing unencrypted is
//! written: the bytes are encrypted before they reach a file. Every string is then written in
//! hexadecimal, since ciphertext in a literal string can hold line breaks that a reader may
//! read wrongly (`hex_strings`).

use std::{collections::BTreeMap, sync::Arc};

use lopdf::{
    encryption::crypt_filters::{Aes256CryptFilter, CryptFilter},
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat,
};

/// An editing restriction a PDF carries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PdfProtection {
    #[default]
    None,
    /// Opens with no password; modification denied (`/P` -1324).
    RestrictModification,
}

/// What a read-only PDF still allows: reading and copying, printing at full quality, and what
/// assistive technology needs.
fn allowed() -> Permissions {
    Permissions::PRINTABLE
        | Permissions::COPYABLE
        | Permissions::COPYABLE_FOR_ACCESSIBILITY
        | Permissions::PRINTABLE_IN_HIGH_QUALITY
}

/// A password nobody needs or is told, from random bytes: printable ASCII, as revision 6
/// takes it.
fn owner_password(seed: &[u8]) -> String {
    seed.iter()
        .map(|byte| char::from(b'!' + byte % 94))
        .collect()
}

/// `pdf` encrypted so that modification is denied, with randomness from `random`: the file key
/// and the owner password, neither kept.
pub(crate) fn restrict_modification(
    pdf: &[u8],
    random: &mut dyn FnMut(&mut [u8]),
) -> Result<Vec<u8>, String> {
    let mut document =
        Document::load_mem(pdf).map_err(|error| format!("reading the PDF back: {error}"))?;
    let mut key = [0u8; 32];
    random(&mut key);
    let mut seed = [0u8; 32];
    random(&mut seed);
    let owner = owner_password(&seed);
    let mut adbe = Dictionary::new();
    adbe.set("BaseVersion", Object::Name(b"1.7".to_vec()));
    adbe.set("ExtensionLevel", Object::Integer(8));
    let mut extensions = Dictionary::new();
    extensions.set("ADBE", Object::Dictionary(adbe));
    document
        .catalog_mut()
        .map_err(|error| format!("the PDF has no catalog: {error}"))?
        .set("Extensions", Object::Dictionary(extensions));
    let mut filters: BTreeMap<Vec<u8>, Arc<dyn CryptFilter>> = BTreeMap::new();
    filters.insert(b"StdCF".to_vec(), Arc::new(Aes256CryptFilter));
    let state = EncryptionState::try_from(EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: filters,
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: &owner,
        user_password: "",
        permissions: allowed(),
    })
    .map_err(|error| format!("setting up the encryption: {error}"))?;
    document
        .encrypt(&state)
        .map_err(|error| format!("encrypting the PDF: {error}"))?;
    for object in document.objects.values_mut() {
        hex_strings(object);
    }
    for (_, value) in document.trailer.iter_mut() {
        hex_strings(value);
    }
    let mut encrypted = Vec::new();
    document
        .save_to(&mut encrypted)
        .map_err(|error| format!("writing the encrypted PDF: {error}"))?;
    Ok(encrypted)
}

/// Every string in `object`, at any depth, written in hexadecimal. In a literal string an
/// unescaped line feed is a line feed, but readers differ on a run of them (`hayro-syntax`
/// 0.7.2 reads consecutive line feeds as one), and ciphertext holds such runs by chance; a
/// hexadecimal string holds no line break.
fn hex_strings(object: &mut Object) {
    match object {
        Object::String(_, format) => *format = StringFormat::Hexadecimal,
        Object::Array(items) => items.iter_mut().for_each(hex_strings),
        Object::Dictionary(dictionary) => {
            for (_, value) in dictionary.iter_mut() {
                hex_strings(value);
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter_mut() {
                hex_strings(value);
            }
        }
        _ => {}
    }
}

/// Whether `pdf` carries the restriction: read back from its bytes, encrypted with AES-256
/// (version 5, revision 6), opening with no password, modification denied.
#[must_use]
pub fn restricts_modification(pdf: &[u8]) -> bool {
    Document::load_mem(pdf)
        .ok()
        .and_then(|document| document.encryption_state)
        .is_some_and(|state| {
            state.version() == 5
                && state.revision() == 6
                && !state.permissions().contains(Permissions::MODIFIABLE)
                && state
                    .permissions()
                    .contains(Permissions::COPYABLE_FOR_ACCESSIBILITY)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_password_is_printable_ascii() {
        let every: Vec<u8> = (0..=255).collect();
        let owner = owner_password(&every);
        assert_eq!(owner.len(), 256);
        assert!(owner.chars().all(|c| c.is_ascii_graphic()), "{owner}");
        assert_ne!(owner_password(&[1; 32]), owner_password(&[2; 32]));
    }

    #[test]
    fn what_stays_allowed() {
        let p = allowed().bits() as i64 as i32;
        // Print, copy, accessibility and full-quality print, with the reserved bits set, as
        // the dictionary's /P gives it.
        let reserved = 0b11 << 6 | 0b1111 << 12 | (0xffff << 16);
        assert_eq!(p | reserved, -1324);
    }
}
