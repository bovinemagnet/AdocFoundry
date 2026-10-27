//! Fonts for a theme: each family in its catalogue is registered under the theme's own name,
//! one face per style, so a theme picks faces exactly as asciidoctor-pdf's catalogue does,
//! whatever the font files call themselves.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use adoc_core::FileAccess;

use typst::{
    foundations::Bytes,
    text::{Font, FontInfo, FontStyle, FontVariant, FontWeight},
};

use crate::theme::{FontFile, LoadedTheme};

/// The fonts asciidoctor-pdf bundles, by file name.
const BUNDLED: &[(&str, &[u8])] = &[
    // Not asciidoctor-pdf's: the maths font STEM is typeset in (OFL-1.1, `fonts/LICENSE-stix`).
    (
        "STIXTwoMath-Regular.otf",
        include_bytes!("../fonts/STIXTwoMath-Regular.otf"),
    ),
    (
        "notoserif-regular-subset.ttf",
        include_bytes!("../fonts/notoserif-regular-subset.ttf"),
    ),
    (
        "notoserif-bold-subset.ttf",
        include_bytes!("../fonts/notoserif-bold-subset.ttf"),
    ),
    (
        "notoserif-italic-subset.ttf",
        include_bytes!("../fonts/notoserif-italic-subset.ttf"),
    ),
    (
        "notoserif-bold_italic-subset.ttf",
        include_bytes!("../fonts/notoserif-bold_italic-subset.ttf"),
    ),
    (
        "notosans-regular-subset.ttf",
        include_bytes!("../fonts/notosans-regular-subset.ttf"),
    ),
    (
        "notosans-bold-subset.ttf",
        include_bytes!("../fonts/notosans-bold-subset.ttf"),
    ),
    (
        "notosans-italic-subset.ttf",
        include_bytes!("../fonts/notosans-italic-subset.ttf"),
    ),
    (
        "notosans-bold_italic-subset.ttf",
        include_bytes!("../fonts/notosans-bold_italic-subset.ttf"),
    ),
    (
        "mplus1mn-regular-subset.ttf",
        include_bytes!("../fonts/mplus1mn-regular-subset.ttf"),
    ),
    (
        "mplus1mn-bold-subset.ttf",
        include_bytes!("../fonts/mplus1mn-bold-subset.ttf"),
    ),
    (
        "mplus1mn-italic-subset.ttf",
        include_bytes!("../fonts/mplus1mn-italic-subset.ttf"),
    ),
    (
        "mplus1mn-bold_italic-subset.ttf",
        include_bytes!("../fonts/mplus1mn-bold_italic-subset.ttf"),
    ),
];

/// The PDF core fonts asciidoctor-pdf's `base` theme names, which are not bundled: each stands
/// in as the bundled family closest to it.
const CORE_FONTS: &[(&str, &str)] = &[
    ("Helvetica", "notosans"),
    ("Times-Roman", "notoserif"),
    ("Courier", "mplus1mn"),
];

/// One bundled font, parsed once per process.
fn bundled(name: &str) -> Option<Font> {
    static FONTS: OnceLock<Vec<(&'static str, Font)>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        BUNDLED
            .iter()
            .filter_map(|(name, data)| Font::new(Bytes::new(*data), 0).map(|font| (*name, font)))
            .collect()
    });
    fonts
        .iter()
        .find(|(bundled, _)| *bundled == name)
        .map(|(_, font)| font.clone())
}

/// The bundled fonts, each under its own family name: enough for a formula typeset on its own.
pub(crate) fn bundled_set() -> FontSet {
    let mut set = FontSet::default();
    for (name, _) in BUNDLED {
        if let Some(font) = bundled(name) {
            let info = font.info().clone();
            set.fonts.push(font);
            set.infos.push(info);
        }
    }
    set
}

/// Fonts and the book entries they are selected by, index for index.
#[derive(Clone, Default)]
pub(crate) struct FontSet {
    pub fonts: Vec<Font>,
    pub infos: Vec<FontInfo>,
}

impl FontSet {
    fn push(&mut self, family: &str, style: Style, font: Font) {
        let mut info = font.info().clone();
        info.family = family.to_owned();
        info.variant = FontVariant {
            style: if style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            weight: if style.bold {
                FontWeight::BOLD
            } else {
                FontWeight::REGULAR
            },
            stretch: info.variant.stretch,
        };
        self.fonts.push(font);
        self.infos.push(info);
    }
}

#[derive(Clone, Copy)]
struct Style {
    bold: bool,
    italic: bool,
}

/// The fonts `theme` names. Relative paths resolve against `dirs` in order (asciidoctor-pdf's
/// `pdf-fontsdir`), then the theme's directory; a file that cannot be read is a warning.
pub(crate) fn font_set(
    theme: &LoadedTheme,
    dirs: &[PathBuf],
    access: &FileAccess,
) -> (FontSet, Vec<String>) {
    let mut set = FontSet::default();
    let mut warnings = Vec::new();
    for (family, faces) in &theme.fonts {
        for (file, style) in [
            (
                &faces.normal,
                Style {
                    bold: false,
                    italic: false,
                },
            ),
            (
                &faces.bold,
                Style {
                    bold: true,
                    italic: false,
                },
            ),
            (
                &faces.italic,
                Style {
                    bold: false,
                    italic: true,
                },
            ),
            (
                &faces.bold_italic,
                Style {
                    bold: true,
                    italic: true,
                },
            ),
        ] {
            let Some(file) = file else { continue };
            match load(file, dirs, theme.dir.as_deref(), access) {
                Some(font) => set.push(family, style, font),
                None => warnings.push(format!(
                    "font {} for {family} not found",
                    match file {
                        FontFile::Bundled(name) => format!("GEM_FONTS_DIR/{name}"),
                        FontFile::Path(path) => path.display().to_string(),
                    }
                )),
            }
        }
    }
    // The core fonts, for themes built on `base`.
    for (family, stem) in CORE_FONTS {
        if theme.fonts.iter().any(|(name, _)| name == family) {
            continue;
        }
        for (suffix, style) in [
            (
                "regular",
                Style {
                    bold: false,
                    italic: false,
                },
            ),
            (
                "bold",
                Style {
                    bold: true,
                    italic: false,
                },
            ),
            (
                "italic",
                Style {
                    bold: false,
                    italic: true,
                },
            ),
            (
                "bold_italic",
                Style {
                    bold: true,
                    italic: true,
                },
            ),
        ] {
            if let Some(font) = bundled(&format!("{stem}-{suffix}-subset.ttf")) {
                set.push(family, style, font);
            }
        }
    }
    // Every bundled font under its own name, last, for glyphs nothing else has.
    for (name, _) in BUNDLED {
        if let Some(font) = bundled(name) {
            let info = font.info().clone();
            set.fonts.push(font);
            set.infos.push(info);
        }
    }
    (set, warnings)
}

/// The fonts `theme` names that cannot be found in `dirs` or the theme's directory, each as the
/// warning an export gives for it. Empty when every font is found.
#[must_use]
pub fn missing_fonts(theme: &LoadedTheme, dirs: &[PathBuf], access: &FileAccess) -> Vec<String> {
    font_set(theme, dirs, access).1
}

/// Each catalogue family's line gap (`hhea.lineGap`) as a fraction of the font size, from its
/// normal face. Prawn's line height includes it; Typst's does not, so the template adds it.
pub(crate) fn line_gaps(
    theme: &LoadedTheme,
    dirs: &[PathBuf],
    access: &FileAccess,
) -> Vec<(String, f64)> {
    let mut gaps = Vec::new();
    for (family, faces) in &theme.fonts {
        let Some(file) = faces.normal.as_ref().or(faces.bold.as_ref()) else {
            continue;
        };
        let gap = load(file, dirs, theme.dir.as_deref(), access)
            .and_then(|font| line_gap(font.data()))
            .unwrap_or(0.0);
        if gap > 0.0 {
            gaps.push((family.clone(), gap));
        }
    }
    gaps
}

/// `hhea.lineGap / head.unitsPerEm` of the first font in `data`.
fn line_gap(data: &[u8]) -> Option<f64> {
    let u16_at = |offset: usize| -> Option<u16> {
        Some(u16::from_be_bytes([
            *data.get(offset)?,
            *data.get(offset + 1)?,
        ]))
    };
    let u32_at = |offset: usize| -> Option<u32> {
        Some(u32::from_be_bytes(
            data.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let table = |tag: &[u8; 4]| -> Option<usize> {
        let count = usize::from(u16_at(4)?);
        (0..count).find_map(|index| {
            let record = 12 + 16 * index;
            (data.get(record..record + 4)? == tag)
                .then(|| u32_at(record + 8).map(|offset| offset as usize))?
        })
    };
    let units = u16_at(table(b"head")? + 18)?;
    let gap = u16_at(table(b"hhea")? + 8)? as i16;
    (units > 0).then(|| f64::from(gap) / f64::from(units))
}

fn load(
    file: &FontFile,
    dirs: &[PathBuf],
    theme_dir: Option<&Path>,
    access: &FileAccess,
) -> Option<Font> {
    match file {
        FontFile::Bundled(name) => bundled(name),
        FontFile::Path(path) if path.is_absolute() => read(path, access),
        FontFile::Path(path) => {
            for dir in dirs {
                if dir.as_os_str() == "GEM_FONTS_DIR" {
                    if let Some(font) = path.to_str().and_then(bundled) {
                        return Some(font);
                    }
                    continue;
                }
                if let Some(font) = read(&dir.join(path), access) {
                    return Some(font);
                }
            }
            if let Some(font) = theme_dir.and_then(|dir| read(&dir.join(path), access)) {
                return Some(font);
            }
            path.to_str().and_then(bundled)
        }
    }
}

fn read(path: &Path, access: &FileAccess) -> Option<Font> {
    let data = access.read(path).ok()?;
    Font::new(Bytes::new(data), 0)
}
