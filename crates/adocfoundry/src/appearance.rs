//! Application themes (#357): the native gpui-component themes the app can show, and applying
//! the user's choice from `[appearance]` (see `adoc_engine::AppearanceSettings`).
//!
//! [`ThemeCatalog`] lists every theme that can be chosen: the native defaults from
//! `ThemeRegistry`, then the sets bundled in `themes/` (licences in `themes/ABOUT-themes`), then
//! the user's JSON theme sets from `[appearance] themes_dir`, read again only by Reload Themes or
//! when that folder changes. A name already listed is kept as first found and the later one
//! skipped with a warning. The pinned registry can neither remove nor replace a theme, so the
//! catalog, not the registry, is what a reload rebuilds; the chosen themes are applied by
//! setting `Theme::light_theme` and `dark_theme` and calling `Theme::change`, which repaints
//! every window. Theme files are data only: read with size limits, parsed as JSON, never run.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use adoc_engine::{AppearanceSettings, ThemePreference};
use gpui::{px, App, Global, Window};
use gpui_component::theme::{Theme, ThemeConfig, ThemeMode, ThemeRegistry, ThemeSet};

/// The theme sets built into the app.
pub const BUNDLED: &[(&str, &str)] = &[
    ("catppuccin.json", include_str!("../themes/catppuccin.json")),
    ("tokyonight.json", include_str!("../themes/tokyonight.json")),
    ("solarized.json", include_str!("../themes/solarized.json")),
    ("everforest.json", include_str!("../themes/everforest.json")),
    ("flexoki.json", include_str!("../themes/flexoki.json")),
    ("aurora.json", include_str!("../themes/aurora.json")),
    ("alduin.json", include_str!("../themes/alduin.json")),
    ("asciinema.json", include_str!("../themes/asciinema.json")),
    ("gruvbox.json", include_str!("../themes/gruvbox.json")),
    ("hybrid.json", include_str!("../themes/hybrid.json")),
    ("jellybeans.json", include_str!("../themes/jellybeans.json")),
    (
        "macos-classic.json",
        include_str!("../themes/macos-classic.json"),
    ),
    (
        "mellifluous.json",
        include_str!("../themes/mellifluous.json"),
    ),
    ("spaceduck.json", include_str!("../themes/spaceduck.json")),
];

/// The largest theme file read, and the most files read from the themes folder.
const MAX_THEME_FILE: u64 = 1024 * 1024;
const MAX_THEME_FILES: usize = 64;

/// Every theme the user can choose, and what was wrong with the files they came from.
#[derive(Default)]
pub struct ThemeCatalog {
    themes: Vec<Rc<ThemeConfig>>,
    /// The folder the user's themes were read from.
    dir: Option<PathBuf>,
    pub warnings: Vec<String>,
    /// While the picker previews themes, nothing else changes the theme shown.
    pub previewing: bool,
}

impl Global for ThemeCatalog {}

impl ThemeCatalog {
    /// The native defaults, the bundled sets, then the theme sets in `dir`, by file name.
    pub fn load(defaults: [Rc<ThemeConfig>; 2], dir: Option<&Path>) -> Self {
        let mut catalog = Self {
            dir: dir.map(Path::to_path_buf),
            ..Self::default()
        };
        for theme in defaults {
            catalog.themes.push(theme);
        }
        for (name, json) in BUNDLED {
            catalog.add_set(json, name);
        }
        if let Some(dir) = dir {
            catalog.add_folder(dir);
        }
        catalog
    }

    fn add_folder(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            // No folder is no themes; it is made only when the user puts themes there.
            return;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        files.sort();
        if files.len() > MAX_THEME_FILES {
            self.warnings.push(format!(
                "{}: only the first {MAX_THEME_FILES} theme files are read",
                dir.display()
            ));
            files.truncate(MAX_THEME_FILES);
        }
        for file in files {
            let name = file.display().to_string();
            // A link or anything but a file is not read.
            match std::fs::symlink_metadata(&file) {
                Ok(meta) if meta.is_file() && meta.len() <= MAX_THEME_FILE => {}
                Ok(meta) if meta.is_file() => {
                    self.warnings
                        .push(format!("{name}: larger than 1 MiB, not read"));
                    continue;
                }
                _ => continue,
            }
            match std::fs::read_to_string(&file) {
                Ok(json) => self.add_set(&json, &name),
                Err(error) => self.warnings.push(format!("{name}: {error}")),
            }
        }
    }

    fn add_set(&mut self, json: &str, source: &str) {
        let set = match serde_json::from_str::<ThemeSet>(json) {
            Ok(set) => set,
            Err(error) => {
                self.warnings
                    .push(format!("{source}: not a theme set: {error}"));
                return;
            }
        };
        for theme in set.themes {
            if self.themes.iter().any(|known| known.name == theme.name) {
                self.warnings.push(format!(
                    "{source}: a theme named \"{}\" is listed already; the first is kept",
                    theme.name
                ));
                continue;
            }
            self.themes.push(Rc::new(theme));
        }
    }

    /// Every theme, light ones first, each by name ignoring case.
    pub fn sorted(&self) -> Vec<Rc<ThemeConfig>> {
        let mut themes = self.themes.clone();
        themes.sort_by(|a, b| {
            a.mode
                .is_dark()
                .cmp(&b.mode.is_dark())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        themes
    }

    /// The theme named `name` if it is one of `mode`'s.
    pub fn find(&self, name: &str, mode: ThemeMode) -> Option<Rc<ThemeConfig>> {
        self.themes
            .iter()
            .find(|theme| theme.name == name && theme.mode.is_dark() == mode.is_dark())
            .cloned()
    }
}

fn defaults(cx: &App) -> [Rc<ThemeConfig>; 2] {
    let registry = ThemeRegistry::global(cx);
    [
        registry.default_light_theme().clone(),
        registry.default_dark_theme().clone(),
    ]
}

/// Reads the themes again, from `dir` for the user's, returning what was wrong with them.
pub fn load(dir: Option<&Path>, cx: &mut App) -> Vec<String> {
    let previewing = cx
        .try_global::<ThemeCatalog>()
        .is_some_and(|catalog| catalog.previewing);
    let mut catalog = ThemeCatalog::load(defaults(cx), dir);
    catalog.previewing = previewing;
    let warnings = catalog.warnings.clone();
    cx.set_global(catalog);
    warnings
}

/// The catalog, read from `dir` first if it has not been, or was read from another folder.
fn catalog<'a>(dir: Option<&Path>, cx: &'a mut App) -> &'a ThemeCatalog {
    let current = cx
        .try_global::<ThemeCatalog>()
        .is_some_and(|catalog| catalog.dir.as_deref() == dir);
    if !current {
        load(dir, cx);
    }
    cx.global::<ThemeCatalog>()
}

/// The themes named for each mode, or that mode's native default, with a warning for a name
/// that names no theme of its mode. The names are kept, so a theme that comes back is used.
pub fn resolve(
    appearance: &AppearanceSettings,
    cx: &mut App,
) -> (Rc<ThemeConfig>, Rc<ThemeConfig>, Vec<String>) {
    let [light_default, dark_default] = defaults(cx);
    let catalog = catalog(appearance.themes_dir.as_deref(), cx);
    let mut warnings = Vec::new();
    let mut pick = |name: Option<&String>, mode: ThemeMode, default: Rc<ThemeConfig>| {
        let Some(name) = name else {
            return default;
        };
        catalog.find(name, mode).unwrap_or_else(|| {
            let kind = if mode.is_dark() { "dark" } else { "light" };
            warnings.push(format!(
                "No {kind} theme is named \"{name}\"; using {}",
                default.name
            ));
            default
        })
    };
    let light = pick(
        appearance.light_theme.as_ref(),
        ThemeMode::Light,
        light_default,
    );
    let dark = pick(
        appearance.dark_theme.as_ref(),
        ThemeMode::Dark,
        dark_default,
    );
    (light, dark, warnings)
}

/// Shows `appearance` in every window: its themes, in its mode or, for `auto`, the system's.
/// Does nothing while the picker previews, unless `force`. Returns what could not be honoured.
pub fn apply(
    appearance: &AppearanceSettings,
    window: Option<&mut Window>,
    force: bool,
    cx: &mut App,
) -> Vec<String> {
    if !force
        && cx
            .try_global::<ThemeCatalog>()
            .is_some_and(|catalog| catalog.previewing)
    {
        return Vec::new();
    }
    let (light, dark, warnings) = resolve(appearance, cx);
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    match appearance.mode {
        ThemePreference::Auto => Theme::sync_system_appearance(window, cx),
        ThemePreference::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemePreference::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
    cx.set_global(FontScale(appearance.font_scale.factor()));
    scale_fonts(cx);
    warnings
}

/// gpui-component's text sizes, for a theme that sets none: the rem size, which every length
/// given in rems follows, and the monospaced size.
const FONT_SIZE: f32 = 16.0;
const MONO_FONT_SIZE: f32 = 13.0;

/// `[appearance] font_scale` as last applied.
struct FontScale(f32);

impl Global for FontScale {}

/// The multiplier text is drawn at (#374): 1 unless `[appearance] font_scale` says otherwise.
pub fn font_scale(cx: &App) -> f32 {
    cx.try_global::<FontScale>().map_or(1.0, |scale| scale.0)
}

/// Draws the application's text at its theme's sizes times the font scale. Applying a theme
/// keeps the sizes it does not set, so this follows every change of theme.
fn scale_fonts(cx: &mut App) {
    let scale = font_scale(cx);
    let theme = Theme::global(cx);
    let config = if theme.mode.is_dark() {
        theme.dark_theme.clone()
    } else {
        theme.light_theme.clone()
    };
    let (size, mono) = (
        config.font_size.unwrap_or(FONT_SIZE),
        config.mono_font_size.unwrap_or(MONO_FONT_SIZE),
    );
    let families = crate::fonts::theme_families(&config, cx);
    let theme = Theme::global_mut(cx);
    theme.font_size = px(size * scale);
    theme.mono_font_size = px(mono * scale);
    if let Some((interface, monospaced)) = families {
        theme.font_family = interface;
        theme.mono_font_family = monospaced;
    }
    cx.refresh_windows();
}

/// Draws the application's text again in the font set shown.
pub fn refresh_text(cx: &mut App) {
    scale_fonts(cx);
}

/// Whether a picker is previewing, so that nothing else changes what is shown meanwhile.
pub fn previewing(cx: &App) -> bool {
    cx.try_global::<ThemeCatalog>()
        .is_some_and(|catalog| catalog.previewing)
}

/// Shows `theme` alone, for the picker, without changing what is saved.
pub fn preview(theme: &Rc<ThemeConfig>, cx: &mut App) {
    let mode = theme.mode;
    let current = Theme::global_mut(cx);
    if mode.is_dark() {
        current.dark_theme = theme.clone();
    } else {
        current.light_theme = theme.clone();
    }
    Theme::change(mode, None, cx);
    scale_fonts(cx);
}

/// Whether the picker is previewing, so that nothing else changes the theme meanwhile.
pub fn set_previewing(previewing: bool, cx: &mut App) {
    if cx.has_global::<ThemeCatalog>() {
        cx.global_mut::<ThemeCatalog>().previewing = previewing;
    }
}

/// `appearance` with `theme` chosen for its mode. The mode stays `auto` when it was, the theme
/// then being used whenever the system's appearance is the theme's; an explicit mode becomes
/// the theme's, so the theme chosen is the one shown.
pub fn with_theme(appearance: &AppearanceSettings, theme: &ThemeConfig) -> AppearanceSettings {
    let mut chosen = appearance.clone();
    let name = Some(theme.name.to_string());
    if theme.mode.is_dark() {
        chosen.dark_theme = name;
    } else {
        chosen.light_theme = name;
    }
    if chosen.mode != ThemePreference::Auto {
        chosen.mode = if theme.mode.is_dark() {
            ThemePreference::Dark
        } else {
            ThemePreference::Light
        };
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> [Rc<ThemeConfig>; 2] {
        let theme = |name: &str, mode| {
            let mut theme = ThemeConfig::default();
            theme.name = name.to_owned().into();
            theme.mode = mode;
            Rc::new(theme)
        };
        [
            theme("Default Light", ThemeMode::Light),
            theme("Default Dark", ThemeMode::Dark),
        ]
    }

    fn names(catalog: &ThemeCatalog) -> Vec<String> {
        catalog
            .sorted()
            .iter()
            .map(|theme| theme.name.to_string())
            .collect()
    }

    #[test]
    fn theme_registry_lists_named_entries() {
        let catalog = ThemeCatalog::load(defaults(), None);
        assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
        let names = names(&catalog);
        // Each once; the light ones first, then the dark, each by name.
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique);
        for name in [
            "Catppuccin Latte",
            "Catppuccin Mocha",
            "Tokyo Night",
            "Solarized Light",
            "Solarized Dark",
            "Everforest Dark",
            "Flexoki Light",
            "Aurora Light",
            "Alduin",
            "Asciinema",
            "Gruvbox Light",
            "Gruvbox Dark",
            "Hybrid Light",
            "Hybrid Dark",
            "Jellybeans",
            "macOS Classic Light",
            "macOS Classic Dark",
            "Mellifluous Light",
            "Mellifluous Dark",
            "Spaceduck",
            "Default Light",
            "Default Dark",
        ] {
            assert!(names.contains(&name.to_owned()), "{name} in {names:?}");
        }
        let first_dark = catalog
            .sorted()
            .iter()
            .position(|theme| theme.mode.is_dark())
            .unwrap();
        assert!(catalog.sorted()[first_dark..]
            .iter()
            .all(|theme| theme.mode.is_dark()));
        assert_eq!(
            names,
            super::tests::names(&ThemeCatalog::load(defaults(), None))
        );
    }

    #[test]
    fn theme_invalid_file_does_not_crash() {
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path();
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        std::fs::write(
            dir.join("mine.json"),
            r#"{"name": "Mine", "themes": [
                {"name": "Paper", "mode": "light"},
                {"name": "Tokyo Night", "mode": "dark"}
            ]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        std::fs::write(
            dir.join("huge.json"),
            vec![b' '; MAX_THEME_FILE as usize + 1],
        )
        .unwrap();
        let catalog = ThemeCatalog::load(defaults(), Some(dir));
        let warnings = catalog.warnings.join("\n");
        assert!(warnings.contains("broken.json"), "{warnings}");
        assert!(warnings.contains("huge.json"), "{warnings}");
        assert!(
            warnings.contains("\"Tokyo Night\" is listed already"),
            "{warnings}"
        );
        assert_eq!(catalog.warnings.len(), 3, "{warnings}");
        assert!(catalog.find("Paper", ThemeMode::Light).is_some());
        // The bundled Tokyo Night is kept, not the file's.
        assert!(catalog.find("Paper", ThemeMode::Dark).is_none());
        assert_eq!(
            names(&catalog)
                .iter()
                .filter(|name| *name == "Tokyo Night")
                .count(),
            1
        );
    }

    #[test]
    fn a_chosen_theme_keeps_auto_and_sets_an_explicit_mode() {
        let catalog = ThemeCatalog::load(defaults(), None);
        let mocha = catalog.find("Catppuccin Mocha", ThemeMode::Dark).unwrap();
        let auto = AppearanceSettings::default();
        let chosen = with_theme(&auto, &mocha);
        assert_eq!(chosen.mode, ThemePreference::Auto);
        assert_eq!(chosen.dark_theme.as_deref(), Some("Catppuccin Mocha"));
        assert_eq!(chosen.light_theme, None);

        let light = AppearanceSettings {
            mode: ThemePreference::Light,
            ..AppearanceSettings::default()
        };
        assert_eq!(with_theme(&light, &mocha).mode, ThemePreference::Dark);
    }
}
