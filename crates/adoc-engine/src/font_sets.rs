//! Font sets: three roles chosen by one setting. Interface draws menus, panels, tabs and dialogs;
//! source the source editor and code wherever it appears; reading the prose and headings of the
//! preview and the visual editor. `[appearance] font_set` chooses a set, built in or one of the
//! user's `[appearance.font_sets.<id>]`, and `[editor] font_family` and `font_size`, when a
//! settings file names them, replace its source role. The engine knows fonts only by family
//! name; a front end ships, loads and draws them.

use crate::{
    commands::{ids, register, spec},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError,
};

/// The set used until another is chosen.
pub const DEFAULT_FONT_SET: &str = "workshop";

/// One choice of fonts for the three roles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontSet {
    /// What `[appearance] font_set` names it by.
    pub id: String,
    /// What it is called in the app.
    pub name: String,
    /// The interface's family; `None` leaves it to the theme, usually the system's.
    pub interface: Option<String>,
    pub source: String,
    pub source_size: u32,
    /// The family of prose and headings; `None` is the interface's.
    pub reading: Option<String>,
    /// The size of body text; `None` is the interface's.
    pub reading_size: Option<u32>,
    /// Whether code outside the source editor is in the source family, not the theme's
    /// monospace.
    pub code_in_source: bool,
}

/// What a set comes to once the settings that replace its source role are applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fonts {
    /// The set's name.
    pub set: String,
    pub interface: Option<String>,
    pub source: String,
    pub source_size: u32,
    /// The family of code outside the source editor; `None` is the theme's monospace.
    pub code: Option<String>,
    pub reading: Option<String>,
    pub reading_size: Option<u32>,
}

fn bundled(
    id: &str,
    name: &str,
    interface: &str,
    (source, source_size): (&str, u32),
    (reading, reading_size): (&str, u32),
) -> FontSet {
    FontSet {
        id: id.to_owned(),
        name: name.to_owned(),
        interface: Some(interface.to_owned()),
        source: source.to_owned(),
        source_size,
        reading: Some(reading.to_owned()),
        reading_size: Some(reading_size),
        code_in_source: true,
    }
}

/// The sets every install has. `current` draws as the app did before there were sets: the
/// theme's fonts, and JetBrains Mono in the source editor if it is installed. The others' fonts
/// ship with the app.
#[must_use]
pub fn built_in_font_sets() -> Vec<FontSet> {
    vec![
        FontSet {
            id: "current".to_owned(),
            name: "Current".to_owned(),
            interface: None,
            source: "JetBrains Mono".to_owned(),
            source_size: 13,
            reading: None,
            reading_size: None,
            code_in_source: false,
        },
        bundled(
            DEFAULT_FONT_SET,
            "Workshop",
            "IBM Plex Sans",
            ("IBM Plex Mono", 13),
            ("IBM Plex Sans", 15),
        ),
        bundled(
            "reading-room",
            "Reading Room",
            "Source Sans 3",
            ("Source Code Pro", 14),
            ("Source Serif 4", 17),
        ),
        bundled(
            "legible",
            "Legible",
            "Atkinson Hyperlegible Next",
            ("Atkinson Hyperlegible Mono", 14),
            ("Atkinson Hyperlegible Next", 16),
        ),
    ]
}

pub(crate) fn register_font_sets(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::APPEARANCE_SAVE_FONT_SET,
            "Save Font Set",
            CommandCategory::View,
            None,
            CommandContext::Always,
        ),
        |engine, args| {
            let CommandArgs::FontSet(id) = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::APPEARANCE_SAVE_FONT_SET,
                    expected: "a font set's id",
                });
            };
            engine.save_font_set(&id).map(|()| CommandOutcome::Done)
        },
    );
}

impl Engine {
    /// Writes `id` to the user settings file as the chosen font set and reads the settings
    /// again, announcing `SettingsChanged`. Refuses an id no set has, and without a user
    /// settings file, changing nothing.
    pub fn save_font_set(&mut self, id: &str) -> Result<(), EngineError> {
        if !self
            .settings
            .appearance
            .font_sets()
            .iter()
            .any(|set| set.id == id)
        {
            return Err(EngineError::Refused(format!(
                "no font set is named \"{id}\""
            )));
        }
        self.settings.write_font_set(id)?;
        self.reload_settings()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::Settings;

    fn load(user: &str) -> Settings {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("adocfoundry.toml");
        fs::write(&file, user).unwrap();
        Settings::load(Some(&file), None).unwrap()
    }

    #[test]
    fn workshop_is_the_default() {
        for settings in [Settings::default(), load("")] {
            let fonts = settings.fonts();
            assert_eq!(fonts.set, "Workshop");
            assert_eq!(fonts.interface.as_deref(), Some("IBM Plex Sans"));
            assert_eq!(fonts.source, "IBM Plex Mono");
            assert_eq!(fonts.source_size, 13);
            assert_eq!(fonts.code.as_deref(), Some("IBM Plex Mono"));
            assert_eq!(fonts.reading.as_deref(), Some("IBM Plex Sans"));
            assert_eq!(fonts.reading_size, Some(15));
            assert_eq!(settings.editor.font_family, "IBM Plex Mono");
            assert_eq!(settings.editor.font_size, 13);
            assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        }
    }

    #[test]
    fn a_built_in_set_gives_the_editor_its_source_role() {
        let settings = load("[appearance]\nfont_set = \"reading-room\"\n");
        let fonts = settings.fonts();
        assert_eq!(fonts.set, "Reading Room");
        assert_eq!(fonts.reading.as_deref(), Some("Source Serif 4"));
        assert_eq!(settings.editor.font_family, "Source Code Pro");
        assert_eq!(settings.editor.font_size, 14);
    }

    #[test]
    fn current_leaves_the_interface_reading_and_code_to_the_theme() {
        let fonts = load("[appearance]\nfont_set = \"current\"\n").fonts();
        assert_eq!(fonts.interface, None);
        assert_eq!(fonts.source, "JetBrains Mono");
        assert_eq!(fonts.source_size, 13);
        assert_eq!(fonts.code, None);
        assert_eq!(fonts.reading, None);
        assert_eq!(fonts.reading_size, None);
    }

    #[test]
    fn editor_settings_replace_the_source_role() {
        let fonts = load(
            "[editor]\nfont_family = \"Fira Code\"\nfont_size = 16\n\
             [appearance]\nfont_set = \"legible\"\n",
        )
        .fonts();
        assert_eq!(fonts.source, "Fira Code");
        assert_eq!(fonts.source_size, 16);
        assert_eq!(fonts.code.as_deref(), Some("Fira Code"));
        assert_eq!(
            fonts.interface.as_deref(),
            Some("Atkinson Hyperlegible Next")
        );
    }

    #[test]
    fn under_current_editor_settings_reach_only_the_editor() {
        let fonts =
            load("[editor]\nfont_family = \"Fira Code\"\n[appearance]\nfont_set = \"current\"\n")
                .fonts();
        assert_eq!(fonts.source, "Fira Code");
        assert_eq!(fonts.code, None);
    }

    #[test]
    fn a_workspace_can_replace_the_source_role_but_not_choose_a_set() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        fs::write(&user, "[appearance]\nfont_set = \"legible\"\n").unwrap();
        fs::write(
            directory.path().join(".adocfoundry.toml"),
            "[editor]\nfont_family = \"Fira Code\"\n\
             [appearance]\nfont_set = \"current\"\n\
             [appearance.font_sets.mine]\nsource = \"Iosevka\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert_eq!(settings.fonts().set, "Legible");
        assert_eq!(settings.fonts().source, "Fira Code");
        for key in ["appearance.font_set", "appearance.font_sets"] {
            assert!(
                settings
                    .warnings
                    .iter()
                    .any(|warning| warning.contains(key) && warning.contains("user settings file")),
                "{key}: {:?}",
                settings.warnings
            );
        }
        assert_eq!(settings.appearance.font_sets().len(), 4);
    }

    #[test]
    fn a_set_of_your_own_takes_what_it_leaves_out_from_its_base() {
        let settings = load(
            "[appearance]\nfont_set = \"house\"\n\
             [appearance.font_sets.house]\nname = \"House\"\nbased_on = \"reading-room\"\n\
             source = \"Fira Code\"\nsource_size = 15\nreading_size = 18\n",
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let fonts = settings.fonts();
        assert_eq!(fonts.set, "House");
        assert_eq!(fonts.interface.as_deref(), Some("Source Sans 3"));
        assert_eq!(fonts.source, "Fira Code");
        assert_eq!(fonts.source_size, 15);
        assert_eq!(fonts.code.as_deref(), Some("Fira Code"));
        assert_eq!(fonts.reading.as_deref(), Some("Source Serif 4"));
        assert_eq!(fonts.reading_size, Some(18));
        assert_eq!(settings.editor.font_family, "Fira Code");
    }

    #[test]
    fn a_set_of_your_own_defaults_to_workshop_and_its_id() {
        let settings = load(
            "[appearance]\nfont_set = \"plain\"\n[appearance.font_sets.plain]\nreading = \"Lora\"\n",
        );
        let fonts = settings.fonts();
        assert_eq!(fonts.set, "plain");
        assert_eq!(fonts.interface.as_deref(), Some("IBM Plex Sans"));
        assert_eq!(fonts.reading.as_deref(), Some("Lora"));
    }

    #[test]
    fn an_unknown_set_warns_and_uses_the_default() {
        let settings = load("[appearance]\nfont_set = \"nope\"\n");
        assert_eq!(settings.fonts().set, "Workshop");
        assert!(
            settings
                .warnings
                .iter()
                .any(|warning| warning.contains("appearance.font_set") && warning.contains("nope")),
            "{:?}",
            settings.warnings
        );
    }

    #[test]
    fn a_bad_set_of_your_own_warns_and_is_left_out() {
        let settings = load(
            "[appearance.font_sets.legible]\nsource = \"Fira Code\"\n\
             [appearance.font_sets.based]\nbased_on = \"nope\"\n\
             [appearance.font_sets.sized]\nsource_size = -1\n\
             [appearance.font_sets.typo]\nsauce = \"x\"\n\
             [appearance.font_sets.scalar]\n",
        );
        let ids: Vec<String> = settings
            .appearance
            .font_sets()
            .into_iter()
            .map(|set| set.id)
            .collect();
        for left_out in ["based", "sized"] {
            assert!(!ids.contains(&left_out.to_owned()), "{ids:?}");
        }
        // A set named like a built-in one does not replace it.
        assert_eq!(
            settings
                .appearance
                .font_sets()
                .iter()
                .find(|set| set.id == "legible")
                .unwrap()
                .source,
            "Atkinson Hyperlegible Mono"
        );
        for key in [
            "font_sets.legible",
            "font_sets.based",
            "font_sets.sized",
            "font_sets.typo",
        ] {
            assert!(
                settings
                    .warnings
                    .iter()
                    .any(|warning| warning.contains(key)),
                "{key}: {:?}",
                settings.warnings
            );
        }
    }

    #[test]
    fn saving_a_set_keeps_the_rest_of_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        let text = "# mine\n[editor]\nsoft_wrap = false\n\n[appearance]\n# dark at night\nmode = \"dark\"\n";
        fs::write(&user, text).unwrap();
        let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
        engine.save_font_set("legible").unwrap();
        let saved = fs::read_to_string(&user).unwrap();
        assert!(saved.starts_with(text), "{saved}");
        assert!(saved.contains("font_set = \"legible\""), "{saved}");
        assert_eq!(engine.settings().fonts().set, "Legible");
        assert_eq!(
            engine.settings().editor.font_family,
            "Atkinson Hyperlegible Mono"
        );
    }

    #[test]
    fn saving_refuses_a_set_that_does_not_exist() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
        assert!(engine.save_font_set("nope").is_err());
        assert!(!user.exists());
    }

    #[test]
    fn the_font_set_is_saved_by_a_command() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
        engine
            .invoke(
                ids::APPEARANCE_SAVE_FONT_SET,
                CommandArgs::FontSet("current".to_owned()),
            )
            .unwrap();
        assert_eq!(engine.settings().fonts().set, "Current");
    }

    #[test]
    fn previewing_a_set_keeps_the_editor_settings() {
        let settings = load("[editor]\nfont_family = \"Fira Code\"\n");
        let legible = built_in_font_sets()
            .into_iter()
            .find(|set| set.id == "legible")
            .unwrap();
        let fonts = settings.fonts_of(&legible);
        assert_eq!(fonts.set, "Legible");
        assert_eq!(fonts.source, "Fira Code");
        assert_eq!(fonts.reading.as_deref(), Some("Atkinson Hyperlegible Next"));
    }
}
