//! The fonts a look names, embedded as Word embeds them (the Word looks PRD,
//! `docs/prd/PRD-AdocFoundry_DOCX_Themes.md`, §5.4): each face obfuscated with a key of its own
//! (ECMA-376 Part 1, §17.8.1) as a part in `word/fonts/`, referenced from `fontTable.xml`.
//!
//! Word and LibreOffice find an embedded font by the family name inside it, not by the name a
//! theme's catalogue gives it, so a family is written under its regular face's own family name
//! (name ID 1, the name Word groups regular, bold, italic and bold italic under), and the styles
//! name it so too. A face is embedded only when its OpenType `fsType` allows it, never when its
//! outlines are CFF (Word embeds TrueType outlines), and within the size a picture may have. The
//! key is worked out from the face's bytes, so the same fonts give the same package.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use adoc_core::FileAccess;
use sha2::{Digest, Sha256};

use crate::{media::MAX_FILE_BYTES, parts, xml::Xml};

/// Where a face of a look's font is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontSource {
    /// A font bundled with the program.
    Bundled(&'static [u8]),
    /// A font file, read through [`DocxOptions::access`](crate::DocxOptions::access).
    File(PathBuf),
    /// A font that was not found: what the theme named, for the warning.
    Missing(String),
}

/// A font family a look names: its name in the look, and its regular, bold, italic and bold
/// italic faces.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FontFamily {
    pub name: String,
    pub faces: [Option<FontSource>; 4],
}

const FACES: [&str; 4] = ["regular", "bold", "italic", "bold italic"];
const EMBED: [&str; 4] = [
    "w:embedRegular",
    "w:embedBold",
    "w:embedItalic",
    "w:embedBoldItalic",
];

/// One family in the font table: its name and its embedded faces, each the index of its font
/// part and its key.
struct TableFont {
    name: String,
    faces: [Option<(usize, String)>; 4],
}

/// The fonts of one export, worked out before its styles are written.
#[derive(Default)]
pub(crate) struct Fonts {
    /// A look's family names to the names their fonts give themselves, where they differ.
    renames: BTreeMap<String, String>,
    table: Vec<TableFont>,
    /// The obfuscated font parts, `word/fonts/font<n>.odttf` for index `n - 1`.
    parts: Vec<Vec<u8>>,
    pub(crate) warnings: Vec<String>,
}

impl Fonts {
    /// The fonts of `families` that `named` holds, in the families' order, embedded when
    /// `embed` is set and the font allows it, else only named.
    pub(crate) fn new(
        families: &[FontFamily],
        named: &BTreeSet<&str>,
        embed: bool,
        access: &FileAccess,
    ) -> Self {
        let mut fonts = Self::default();
        let mut by_bytes: BTreeMap<[u8; 32], usize> = BTreeMap::new();
        for family in families
            .iter()
            .filter(|family| named.contains(family.name.as_str()))
        {
            let mut faces: [Option<Vec<u8>>; 4] = Default::default();
            for (index, source) in family.faces.iter().enumerate() {
                let Some(source) = source else { continue };
                match read(source, access) {
                    Ok(bytes) => faces[index] = Some(bytes),
                    Err(why) => fonts.warnings.push(format!(
                        "the {} face of font `{}` {why}; it is named but not embedded",
                        FACES[index], family.name
                    )),
                }
            }
            // The family's own name, from its regular face, else its first readable one.
            let mut own = None;
            for (index, bytes) in faces.iter().enumerate() {
                let Some(bytes) = bytes else { continue };
                match family_name(bytes) {
                    Some(name) if own.is_none() => own = Some(name),
                    Some(name) if own.as_deref() != Some(name.as_str()) => {
                        fonts.warnings.push(format!(
                            "the {} face of font `{}` is of the family `{name}`, not `{}`; Word \
                             may not use it for that family",
                            FACES[index],
                            family.name,
                            own.as_deref().unwrap_or_default()
                        ));
                    }
                    _ => {}
                }
            }
            let name = own.unwrap_or_else(|| family.name.clone());
            if name != family.name {
                fonts.renames.insert(family.name.clone(), name.clone());
            }
            let row = match fonts.table.iter().position(|font| font.name == name) {
                Some(row) => row,
                None => {
                    fonts.table.push(TableFont {
                        name: name.clone(),
                        faces: Default::default(),
                    });
                    fonts.table.len() - 1
                }
            };
            if !embed {
                continue;
            }
            for (index, bytes) in faces.into_iter().enumerate() {
                let Some(bytes) = bytes else { continue };
                if fonts.table[row].faces[index].is_some() {
                    continue;
                }
                if let Err(why) = embeddable(&bytes) {
                    fonts.warnings.push(format!(
                        "the {} face of font `{}` {why}; it is named but not embedded",
                        FACES[index], family.name
                    ));
                    continue;
                }
                if let Some(warning) = preview_only(&bytes) {
                    fonts.warnings.push(format!(
                        "the {} face of font `{}` {warning}",
                        FACES[index], family.name
                    ));
                }
                let hash: [u8; 32] = Sha256::digest(&bytes).into();
                let key = font_key(&hash);
                let part = *by_bytes.entry(hash).or_insert_with(|| {
                    fonts.parts.push(obfuscate(bytes, &key));
                    fonts.parts.len() - 1
                });
                fonts.table[row].faces[index] = Some((part, key));
            }
        }
        fonts
    }

    /// The name the styles give a font the look names `name`.
    pub(crate) fn name<'a>(&'a self, name: &'a str) -> &'a str {
        self.renames.get(name).map_or(name, String::as_str)
    }

    /// Whether any face is embedded, for `w:embedTrueTypeFonts`.
    pub(crate) fn embeds(&self) -> bool {
        !self.parts.is_empty()
    }

    /// Whether there is a font table to write.
    pub(crate) fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// `word/fontTable.xml` and its relationships to the font parts.
    pub(crate) fn table_part(&self) -> (Vec<u8>, Vec<(String, String)>) {
        let mut xml = Xml::part();
        xml.open("w:fonts", &[("xmlns:w", parts::W), ("xmlns:r", parts::R)]);
        for font in &self.table {
            xml.open("w:font", &[("w:name", &font.name)]);
            for (element, face) in EMBED.iter().zip(&font.faces) {
                if let Some((part, key)) = face {
                    xml.empty(element, &[("r:id", &font_id(*part)), ("w:fontKey", key)]);
                }
            }
            xml.close("w:font");
        }
        xml.close("w:fonts");
        let relationships = (0..self.parts.len())
            .map(|part| (font_id(part), format!("fonts/font{}.odttf", part + 1)))
            .collect();
        (xml.into_bytes(), relationships)
    }

    /// The font parts, by name.
    pub(crate) fn into_parts(self) -> impl Iterator<Item = (String, Vec<u8>)> {
        self.parts
            .into_iter()
            .enumerate()
            .map(|(part, data)| (format!("word/fonts/font{}.odttf", part + 1), data))
    }
}

fn font_id(part: usize) -> String {
    format!("rFont{}", part + 1)
}

fn read(source: &FontSource, access: &FileAccess) -> Result<Vec<u8>, String> {
    match source {
        FontSource::Bundled(bytes) => Ok(bytes.to_vec()),
        FontSource::Missing(named) => Err(format!("({named}) was not found")),
        FontSource::File(path) => {
            if !access.permits(path) {
                return Err(format!(
                    "({}) cannot be read: it is outside the folders the export may read",
                    path.display()
                ));
            }
            if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES as u64)
            {
                return Err(format!(
                    "({}) is more than {} MB",
                    path.display(),
                    MAX_FILE_BYTES / 1_000_000
                ));
            }
            access
                .read(path)
                .map_err(|error| format!("({}) cannot be read: {error}", path.display()))
        }
    }
}

/// The table `tag` of the font `bytes`, from its table directory, when it lies within the font.
/// Font files are a theme's, so nothing here trusts a count, an offset or a length.
fn table<'a>(bytes: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let u16_at = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
    let u32_at = |at: usize| {
        Some(u32::from_be_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
    };
    let tables = usize::from(u16_at(4)?);
    (0..tables).find_map(|index| {
        let record = 12 + index * 16;
        if bytes.get(record..record + 4)? != tag {
            return None;
        }
        let (offset, length) = (u32_at(record + 8)?, u32_at(record + 12)?);
        bytes.get(offset..offset.checked_add(length)?)
    })
}

/// The family name a face gives itself (name ID 1), in US English where it has one: a Unicode
/// or Windows Unicode name, read as UTF-16.
fn family_name(bytes: &[u8]) -> Option<String> {
    let names = table(bytes, b"name")?;
    let u16_at = |at: usize| Some(u16::from_be_bytes([*names.get(at)?, *names.get(at + 1)?]));
    let count = usize::from(u16_at(2)?);
    let storage = usize::from(u16_at(4)?);
    let mut found: Option<String> = None;
    for index in 0..count {
        let record = 6 + index * 12;
        let field = |at: usize| u16_at(record + at);
        let (Some(platform), Some(encoding), Some(language), Some(id), Some(length), Some(offset)) =
            (field(0), field(2), field(4), field(6), field(8), field(10))
        else {
            break;
        };
        let unicode = platform == 0 || (platform == 3 && matches!(encoding, 0 | 1));
        if id != 1 || !unicode || length % 2 != 0 {
            continue;
        }
        let start = storage + usize::from(offset);
        let Some(text) = names.get(start..start + usize::from(length)) else {
            continue;
        };
        let units: Vec<u16> = text
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair))
            .collect();
        let Ok(name) = String::from_utf16(&units) else {
            continue;
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            continue;
        }
        if platform == 3 && language == 0x0409 {
            return Some(name);
        }
        found.get_or_insert(name);
    }
    found
}

/// The face's `fsType`, or 0 (installable) when it has no `OS/2` table.
fn fs_type(bytes: &[u8]) -> Option<u16> {
    if !matches!(bytes.get(..4)?, [0, 1, 0, 0] | b"true" | b"OTTO") {
        return None;
    }
    let Some(os2) = table(bytes, b"OS/2") else {
        return Some(0);
    };
    Some(u16::from_be_bytes([*os2.get(8)?, *os2.get(9)?]))
}

/// Whether Word can take the face whole, or why not, as the end of a sentence naming it.
fn embeddable(bytes: &[u8]) -> Result<(), String> {
    if bytes.starts_with(b"ttcf") {
        return Err("is a font collection, which Word does not embed".to_owned());
    }
    if bytes.starts_with(b"OTTO") {
        return Err("has CFF outlines, and Word embeds only TrueType outlines".to_owned());
    }
    let fs_type = fs_type(bytes).ok_or("is not a font that can be read")?;
    if fs_type & 0x0200 != 0 {
        return Err("allows only its bitmaps to be embedded".to_owned());
    }
    // The least restrictive of the usage bits a font sets.
    let usage = fs_type & 0x000F;
    if usage != 0 && usage & 0x000C == 0 && usage & 0x0002 != 0 {
        return Err("does not allow embedding (its licence restricts it)".to_owned());
    }
    Ok(())
}

/// A warning for a face whose licence lets it be embedded for preview and print only.
fn preview_only(bytes: &[u8]) -> Option<&'static str> {
    let usage = fs_type(bytes)? & 0x000F;
    (usage & 0x0004 != 0 && usage & 0x0008 == 0).then_some(
        "allows embedding for preview and print only; Word may open the document read-only",
    )
}

/// The key a face is obfuscated with, a GUID worked out from its bytes' hash.
fn font_key(hash: &[u8; 32]) -> String {
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|byte| format!("{byte:02X}")).collect() };
    format!(
        "{{{}-{}-{}-{}-{}}}",
        hex(&hash[0..4]),
        hex(&hash[4..6]),
        hex(&hash[6..8]),
        hex(&hash[8..10]),
        hex(&hash[10..16])
    )
}

/// The key's 16 bytes as §17.8.1 takes them: the GUID's hexadecimal digits read as bytes, last
/// first.
pub(crate) fn key_bytes(key: &str) -> [u8; 16] {
    let digits: Vec<u8> = key
        .bytes()
        .filter(u8::is_ascii_hexdigit)
        .map(|digit| match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            _ => digit - b'A' + 10,
        })
        .collect();
    let mut bytes = [0; 16];
    for (index, pair) in digits.chunks(2).take(16).enumerate() {
        bytes[15 - index] = pair[0] << 4 | pair.get(1).copied().unwrap_or(0);
    }
    bytes
}

/// The face with its first 32 bytes each XORed with the key's byte at the same place modulo 16.
fn obfuscate(mut bytes: Vec<u8>, key: &str) -> Vec<u8> {
    let key = key_bytes(key);
    for (index, byte) in bytes.iter_mut().take(32).enumerate() {
        *byte ^= key[index % 16];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled(name: &str) -> &'static [u8] {
        adoc_theme::bundled_font(name).expect("a bundled font")
    }

    /// `bytes` with its `fsType` set to `value`.
    fn with_fs_type(bytes: &[u8], value: u16) -> Vec<u8> {
        let os2 = table(bytes, b"OS/2").unwrap();
        let at = os2.as_ptr() as usize - bytes.as_ptr() as usize + 8;
        let mut bytes = bytes.to_vec();
        bytes[at..at + 2].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    #[test]
    fn fonts_give_their_own_family_names() {
        for (file, family) in [
            ("notoserif-regular-subset.ttf", "Noto Serif"),
            ("notoserif-bold_italic-subset.ttf", "Noto Serif"),
            ("notosans-regular-subset.ttf", "Noto Sans"),
            ("mplus1mn-bold-subset.ttf", "M+ 1mn"),
        ] {
            assert_eq!(
                family_name(bundled(file)).as_deref(),
                Some(family),
                "{file}"
            );
            assert_eq!(fs_type(bundled(file)), Some(0), "{file}");
        }
    }

    #[test]
    fn damaged_fonts_are_read_without_panicking() {
        let font = bundled("notoserif-regular-subset.ttf");
        // Every cut-short font, and every byte of the directory and name table changed.
        for length in 0..4096.min(font.len()) {
            let cut = &font[..length];
            let _ = (family_name(cut), fs_type(cut), embeddable(cut));
        }
        let names = table(font, b"name").unwrap();
        let at = names.as_ptr() as usize - font.as_ptr() as usize;
        for index in (0..12 + 16 * 20).chain(at..at + 64) {
            for value in [0x00, 0x7F, 0xFF] {
                let mut damaged = font.to_vec();
                damaged[index] = value;
                let _ = (
                    family_name(&damaged),
                    fs_type(&damaged),
                    embeddable(&damaged),
                );
            }
        }
        // A huge count of tables, a name past the end, and UTF-16 of an odd length.
        let mut huge = font.to_vec();
        huge[4..6].copy_from_slice(&u16::MAX.to_be_bytes());
        let _ = (family_name(&huge), fs_type(&huge));
        let mut past = font.to_vec();
        // The first name record's length and offset.
        past[at + 6 + 8..at + 6 + 12].copy_from_slice(&[0xFF, 0xFE, 0xFF, 0xFF]);
        let _ = family_name(&past);
        assert_eq!(family_name(b"\0\x01\0\0\0\0"), None);
    }

    #[test]
    fn the_key_is_the_guid_read_backwards() {
        let key = "{00112233-4455-6677-8899-AABBCCDDEEFF}";
        assert_eq!(
            key_bytes(key),
            [
                0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
                0x11, 0x00
            ]
        );
        // Only the first 32 bytes change, and doing it again undoes it.
        let font = (0..40).collect::<Vec<u8>>();
        let hidden = obfuscate(font.clone(), key);
        assert_eq!(hidden[0], 0xFF);
        assert_eq!(hidden[16], 16 ^ 0xFF);
        assert_eq!(hidden[32..], font[32..]);
        assert_eq!(obfuscate(hidden, key), font);
        // The key is a GUID worked out from the bytes.
        let key = font_key(&[0xAB; 32]);
        assert_eq!(key, "{ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB}");
    }

    #[test]
    fn a_face_is_embedded_as_its_licence_allows() {
        let font = bundled("notoserif-regular-subset.ttf");
        assert_eq!(embeddable(&with_fs_type(font, 0)), Ok(()));
        assert_eq!(embeddable(&with_fs_type(font, 0x0008)), Ok(()));
        // No subsetting asks nothing of a whole font.
        assert_eq!(embeddable(&with_fs_type(font, 0x0100)), Ok(()));
        assert!(embeddable(&with_fs_type(font, 0x0002))
            .unwrap_err()
            .contains("licence"));
        assert!(embeddable(&with_fs_type(font, 0x0200))
            .unwrap_err()
            .contains("bitmaps"));
        // The least restrictive bit wins.
        assert_eq!(embeddable(&with_fs_type(font, 0x000A)), Ok(()));
        assert_eq!(preview_only(&with_fs_type(font, 0x000C)), None);
        assert_eq!(embeddable(&with_fs_type(font, 0x0004)), Ok(()));
        assert!(preview_only(&with_fs_type(font, 0x0004)).is_some());
        let mut cff = font.to_vec();
        cff[..4].copy_from_slice(b"OTTO");
        assert!(embeddable(&cff).unwrap_err().contains("CFF"));
        assert!(embeddable(b"not a font").is_err());
    }

    #[test]
    fn families_take_their_own_names_and_each_face_is_embedded_once() {
        let regular = bundled("notoserif-regular-subset.ttf");
        let families = [
            FontFamily {
                name: "Brand Serif".to_owned(),
                faces: [
                    Some(FontSource::Bundled(regular)),
                    Some(FontSource::Bundled(bundled("notoserif-bold-subset.ttf"))),
                    None,
                    Some(FontSource::Missing("brand-bold-italic.ttf".to_owned())),
                ],
            },
            // The same file under another name, and a family no style names.
            FontFamily {
                name: "Other".to_owned(),
                faces: [Some(FontSource::Bundled(regular)), None, None, None],
            },
            FontFamily {
                name: "Unused".to_owned(),
                faces: [Some(FontSource::Bundled(regular)), None, None, None],
            },
        ];
        let named = BTreeSet::from(["Brand Serif", "Other"]);
        let fonts = Fonts::new(&families, &named, true, &FileAccess::default());
        let own = "Noto Serif";
        assert_eq!(fonts.name("Brand Serif"), own);
        assert_eq!(fonts.name("Other"), own);
        assert_eq!(fonts.name("Unnamed"), "Unnamed");
        assert_eq!(fonts.table.len(), 1);
        assert_eq!(fonts.parts.len(), 2);
        assert!(fonts.embeds());
        assert_eq!(fonts.warnings.len(), 1, "{:?}", fonts.warnings);
        assert!(fonts.warnings[0].contains("brand-bold-italic.ttf"));
        let (table, relationships) = fonts.table_part();
        let table = String::from_utf8(table).unwrap();
        assert!(
            table.contains(&format!(r#"<w:font w:name="{own}">"#)),
            "{table}"
        );
        assert!(table.contains(r#"<w:embedRegular r:id="rFont1" w:fontKey="{"#));
        assert!(table.contains(r#"<w:embedBold r:id="rFont2""#));
        assert!(!table.contains("w:embedItalic"));
        assert_eq!(relationships[1].1, "fonts/font2.odttf");
        // Read back, each part is its face.
        let key = fonts.table[0].faces[0].as_ref().unwrap().1.clone();
        let part = fonts.into_parts().next().unwrap();
        assert_eq!(part.0, "word/fonts/font1.odttf");
        assert_eq!(obfuscate(part.1, &key), regular);

        // Not embedding names the family only.
        let fonts = Fonts::new(&families, &named, false, &FileAccess::default());
        assert!(!fonts.embeds());
        assert_eq!(fonts.table.len(), 1);
        assert!(!String::from_utf8(fonts.table_part().0)
            .unwrap()
            .contains("w:embed"));
    }
}
