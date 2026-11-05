//! The fonts the app draws with: the files of the font sets that ship with it (their licences in
//! `fonts/ABOUT-fonts`), loaded once, and the roles of the chosen set (see
//! `adoc_engine::Fonts`). The interface and code roles are drawn through the theme, whose
//! `font_family` and `mono_font_family` every view already reads; the source role by the source
//! editor; the reading role by the preview and the visual editor, through [`reading`].
//!
//! Nothing below this crate knows font files exist. A family a set names that is neither shipped
//! nor installed is reported and left to the theme, since GPUI's fallback for a missing family
//! is paid for on every text run.

use std::borrow::Cow;

use adoc_engine::{Fonts, Settings};
use gpui::{px, AbsoluteLength, App, Font, Global, SharedString, Styled, TextStyleRefinement};
use gpui_component::theme::{Theme, ThemeConfig};

/// Every font file built into the app, with the family it declares.
pub const BUNDLED: &[(&str, &[u8])] = &[
    (
        "IBM Plex Sans",
        include_bytes!("../fonts/IBMPlexSans-Bold.ttf"),
    ),
    (
        "IBM Plex Sans",
        include_bytes!("../fonts/IBMPlexSans-BoldItalic.ttf"),
    ),
    (
        "IBM Plex Sans",
        include_bytes!("../fonts/IBMPlexSans-Italic.ttf"),
    ),
    (
        "IBM Plex Sans",
        include_bytes!("../fonts/IBMPlexSans-Regular.ttf"),
    ),
    (
        "IBM Plex Sans",
        include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf"),
    ),
    (
        "IBM Plex Mono",
        include_bytes!("../fonts/IBMPlexMono-Bold.ttf"),
    ),
    (
        "IBM Plex Mono",
        include_bytes!("../fonts/IBMPlexMono-BoldItalic.ttf"),
    ),
    (
        "IBM Plex Mono",
        include_bytes!("../fonts/IBMPlexMono-Italic.ttf"),
    ),
    (
        "IBM Plex Mono",
        include_bytes!("../fonts/IBMPlexMono-Regular.ttf"),
    ),
    (
        "Source Sans 3",
        include_bytes!("../fonts/SourceSans3-Bold.ttf"),
    ),
    (
        "Source Sans 3",
        include_bytes!("../fonts/SourceSans3-BoldIt.ttf"),
    ),
    (
        "Source Sans 3",
        include_bytes!("../fonts/SourceSans3-It.ttf"),
    ),
    (
        "Source Sans 3",
        include_bytes!("../fonts/SourceSans3-Regular.ttf"),
    ),
    (
        "Source Sans 3",
        include_bytes!("../fonts/SourceSans3-Semibold.ttf"),
    ),
    (
        "Source Code Pro",
        include_bytes!("../fonts/SourceCodePro-Bold.ttf"),
    ),
    (
        "Source Code Pro",
        include_bytes!("../fonts/SourceCodePro-BoldIt.ttf"),
    ),
    (
        "Source Code Pro",
        include_bytes!("../fonts/SourceCodePro-It.ttf"),
    ),
    (
        "Source Code Pro",
        include_bytes!("../fonts/SourceCodePro-Regular.ttf"),
    ),
    (
        "Source Serif 4",
        include_bytes!("../fonts/SourceSerif4-Bold.ttf"),
    ),
    (
        "Source Serif 4",
        include_bytes!("../fonts/SourceSerif4-BoldIt.ttf"),
    ),
    (
        "Source Serif 4",
        include_bytes!("../fonts/SourceSerif4-It.ttf"),
    ),
    (
        "Source Serif 4",
        include_bytes!("../fonts/SourceSerif4-Regular.ttf"),
    ),
    (
        "Source Serif 4",
        include_bytes!("../fonts/SourceSerif4-Semibold.ttf"),
    ),
    (
        "Atkinson Hyperlegible Next",
        include_bytes!("../fonts/AtkinsonHyperlegibleNext-Bold.ttf"),
    ),
    (
        "Atkinson Hyperlegible Next",
        include_bytes!("../fonts/AtkinsonHyperlegibleNext-BoldItalic.ttf"),
    ),
    (
        "Atkinson Hyperlegible Next",
        include_bytes!("../fonts/AtkinsonHyperlegibleNext-Italic.ttf"),
    ),
    (
        "Atkinson Hyperlegible Next",
        include_bytes!("../fonts/AtkinsonHyperlegibleNext-Regular.ttf"),
    ),
    (
        "Atkinson Hyperlegible Next",
        include_bytes!("../fonts/AtkinsonHyperlegibleNext-SemiBold.ttf"),
    ),
    (
        "Atkinson Hyperlegible Mono",
        include_bytes!("../fonts/AtkinsonHyperlegibleMono-Bold.ttf"),
    ),
    (
        "Atkinson Hyperlegible Mono",
        include_bytes!("../fonts/AtkinsonHyperlegibleMono-BoldItalic.ttf"),
    ),
    (
        "Atkinson Hyperlegible Mono",
        include_bytes!("../fonts/AtkinsonHyperlegibleMono-Italic.ttf"),
    ),
    (
        "Atkinson Hyperlegible Mono",
        include_bytes!("../fonts/AtkinsonHyperlegibleMono-Regular.ttf"),
    ),
];

/// The theme's families before any set or theme chose others, and the fonts shown.
struct Active {
    native: (SharedString, SharedString),
    fonts: Option<Fonts>,
}

impl Global for Active {}

/// Loads the shipped fonts and notes the families the theme starts with. Before any window, as
/// a window's first frame may draw in them.
pub fn init(cx: &mut App) {
    let theme = Theme::global(cx);
    let native = (theme.font_family.clone(), theme.mono_font_family.clone());
    cx.set_global(Active {
        native,
        fonts: None,
    });
    let files = BUNDLED
        .iter()
        .map(|(_, file)| Cow::Borrowed(*file))
        .collect();
    if let Err(error) = cx.text_system().add_fonts(files) {
        eprintln!("adocfoundry: the shipped fonts could not be loaded: {error}");
    }
}

/// Whether `family` ships with the app.
pub fn is_bundled(family: &str) -> bool {
    BUNDLED.iter().any(|(bundled, _)| *bundled == family)
}

/// Draws `fonts` in every window, returning what could not be honoured. Does nothing while a
/// picker previews, unless `force`.
pub fn show(mut fonts: Fonts, force: bool, cx: &mut App) -> Vec<String> {
    if !force && crate::appearance::previewing(cx) {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let mut installed = None;
    for (role, family) in [
        ("interface", &mut fonts.interface),
        ("code", &mut fonts.code),
        ("reading", &mut fonts.reading),
    ] {
        let Some(name) = family.as_deref().filter(|name| !is_bundled(name)) else {
            continue;
        };
        let names = installed.get_or_insert_with(|| cx.text_system().all_font_names());
        if !names.iter().any(|known| known == name) {
            warnings.push(format!(
                "The {role} font \"{name}\" of the font set {} is not installed; using a fallback",
                fonts.set
            ));
            *family = None;
        }
    }
    if let Some(active) = cx.try_global::<Active>() {
        let native = active.native.clone();
        cx.set_global(Active {
            native,
            fonts: Some(fonts),
        });
    }
    crate::appearance::refresh_text(cx);
    warnings
}

/// The interface and monospaced families to draw with under `config`: the set's, else the
/// theme's, else what the app started with. `None` before [`init`].
pub fn theme_families(config: &ThemeConfig, cx: &App) -> Option<(SharedString, SharedString)> {
    let active = cx.try_global::<Active>()?;
    let fonts = active.fonts.as_ref();
    let interface = fonts
        .and_then(|fonts| fonts.interface.clone())
        .map(SharedString::from)
        .or_else(|| config.font_family.clone())
        .unwrap_or_else(|| active.native.0.clone());
    let mono = fonts
        .and_then(|fonts| fonts.code.clone())
        .map(SharedString::from)
        .or_else(|| config.mono_font_family.clone())
        .unwrap_or_else(|| active.native.1.clone());
    Some((interface, mono))
}

/// The source editor's family and size: those shown, which a picker may be previewing.
pub fn source(settings: &Settings, cx: &App) -> (String, u32) {
    cx.try_global::<Active>()
        .and_then(|active| active.fonts.as_ref())
        .map_or_else(
            || {
                (
                    settings.editor.font_family.clone(),
                    settings.editor.font_size,
                )
            },
            |fonts| (fonts.source.clone(), fonts.source_size),
        )
}

fn shown(cx: &App) -> Option<&Fonts> {
    cx.try_global::<Active>()?.fonts.as_ref()
}

/// The text style of prose and headings: the reading family and size, each only if the set
/// gives one.
pub fn reading_style(cx: &App) -> TextStyleRefinement {
    let fonts = shown(cx);
    TextStyleRefinement {
        font_family: fonts
            .and_then(|fonts| fonts.reading.clone())
            .map(SharedString::from),
        font_size: fonts.and_then(|fonts| fonts.reading_size).map(|size| {
            AbsoluteLength::Pixels(px(size as f32 * crate::appearance::font_scale(cx)))
        }),
        ..TextStyleRefinement::default()
    }
}

/// `element` with the reading role's text style, inherited by the prose inside it.
pub fn reading<E: Styled>(mut element: E, cx: &App) -> E {
    let style = reading_style(cx);
    if let Some(family) = style.font_family {
        element = element.font_family(family);
    }
    if let Some(size) = style.font_size {
        element = element.text_size(size);
    }
    element
}

/// `font` in the reading family, for text laid out outside any element.
pub fn reading_font(font: Font, cx: &App) -> Font {
    match reading_style(cx).font_family {
        Some(family) => Font { family, ..font },
        None => font,
    }
}

/// How the set shown draws each role, for Help › Show Diagnostics: the set's name and the
/// interface, source and reading families.
pub fn described(cx: &App) -> Option<(String, String, String, String)> {
    let fonts = shown(cx)?;
    let interface = Theme::global(cx).font_family.to_string();
    Some((
        fonts.set.clone(),
        interface.clone(),
        fonts.source.clone(),
        fonts.reading.clone().unwrap_or(interface),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use adoc_engine::built_in_font_sets;
    use gpui::{FontStyle, FontWeight};
    use skrifa::{attribute::Style, string::StringId, FontRef, MetadataProvider};

    use super::*;

    /// The family a font file declares (its typographic family, else its family), its weight
    /// and whether it is italic.
    fn declared_family(file: &[u8]) -> (String, u16, bool) {
        let face = FontRef::new(file).unwrap();
        let name = |id| {
            face.localized_strings(id)
                .english_or_first()
                .map(|name| name.to_string())
        };
        let family = name(StringId::TYPOGRAPHIC_FAMILY_NAME)
            .or_else(|| name(StringId::FAMILY_NAME))
            .unwrap();
        let attributes = face.attributes();
        (
            family,
            attributes.weight.value() as u16,
            attributes.style == Style::Italic,
        )
    }

    #[test]
    fn each_font_file_declares_the_family_it_is_listed_under() {
        for (family, file) in BUNDLED {
            assert_eq!(declared_family(file).0, *family);
        }
    }

    #[test]
    fn each_shipped_family_has_the_four_basic_styles() {
        let mut styles: BTreeMap<&str, BTreeSet<(u16, bool)>> = BTreeMap::new();
        for (family, file) in BUNDLED {
            let (_, weight, italic) = declared_family(file);
            styles.entry(family).or_default().insert((weight, italic));
        }
        for (family, styles) in styles {
            for style in [(400, false), (400, true), (700, false), (700, true)] {
                assert!(styles.contains(&style), "{family}: {style:?} in {styles:?}");
            }
        }
    }

    /// Through the platform's own text system, which GPUI's test platform does not have.
    #[test]
    fn the_platform_loads_every_shipped_style() {
        let system = gpui_platform::current_platform(true).text_system();
        let files = BUNDLED
            .iter()
            .map(|(_, file)| Cow::Borrowed(*file))
            .collect();
        system.add_fonts(files).unwrap();
        let names = system.all_font_names();
        let families: BTreeSet<&str> = BUNDLED.iter().map(|(family, _)| *family).collect();
        for family in families {
            assert!(names.iter().any(|name| name == family), "{family}");
            let id = |weight, style| {
                let font = Font {
                    weight,
                    style,
                    ..gpui::font(family)
                };
                system.font_id(&font).unwrap()
            };
            let regular = id(FontWeight::NORMAL, FontStyle::Normal);
            assert_ne!(regular, id(FontWeight::BOLD, FontStyle::Normal), "{family}");
            assert_ne!(
                regular,
                id(FontWeight::NORMAL, FontStyle::Italic),
                "{family}"
            );
            assert_ne!(
                id(FontWeight::BOLD, FontStyle::Normal),
                id(FontWeight::BOLD, FontStyle::Italic),
                "{family}"
            );
        }
    }

    /// The source editor's columns line up only in a monospaced font.
    #[test]
    fn every_built_in_source_font_is_monospaced() {
        let system = gpui_platform::current_platform(true).text_system();
        let files = BUNDLED
            .iter()
            .map(|(_, file)| Cow::Borrowed(*file))
            .collect();
        system.add_fonts(files).unwrap();
        for set in built_in_font_sets() {
            if !is_bundled(&set.source) {
                continue;
            }
            let id = system.font_id(&gpui::font(set.source.clone())).unwrap();
            let advance = |character| {
                let glyph = system.glyph_for_char(id, character).unwrap();
                system.advance(id, glyph).unwrap().width
            };
            assert_eq!(advance('i'), advance('W'), "{}", set.source);
        }
    }

    #[test]
    fn every_built_in_set_but_current_ships_its_fonts() {
        for set in built_in_font_sets() {
            if set.id == "current" {
                assert_eq!(set.interface, None);
                assert!(!is_bundled(&set.source), "current draws as it always did");
                continue;
            }
            for family in [
                set.interface.as_deref().unwrap(),
                &set.source,
                set.reading.as_deref().unwrap(),
            ] {
                assert!(is_bundled(family), "{}: {family}", set.id);
            }
        }
    }

    #[test]
    fn every_shipped_family_is_used_by_a_built_in_set() {
        let sets = built_in_font_sets();
        for (family, _) in BUNDLED {
            assert!(
                sets.iter()
                    .any(|set| set.interface.as_deref() == Some(*family)
                        || set.source == *family
                        || set.reading.as_deref() == Some(*family)),
                "{family} is shipped for nothing"
            );
        }
    }
}
