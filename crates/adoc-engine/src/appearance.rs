//! The app's appearance as the user chose it: `appearance.save` writes the mode and the named
//! light and dark themes to the user settings file (`[appearance]`, see
//! [`crate::AppearanceSettings`]) and reads the settings again. The engine knows themes only by
//! name; a front end loads and applies them.

use crate::{
    commands::{ids, register, spec},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, ThemePreference,
};

/// What `appearance.save` saves: `CommandArgs::Appearance`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppearanceChoice {
    pub mode: ThemePreference,
    /// The light theme's name, `None` for the native default.
    pub light_theme: Option<String>,
    /// The dark theme's name, `None` for the native default.
    pub dark_theme: Option<String>,
}

pub(crate) fn register_appearance(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::APPEARANCE_SAVE,
            "Save Appearance",
            CommandCategory::View,
            None,
            CommandContext::Always,
        ),
        |engine, args| {
            let CommandArgs::Appearance(choice) = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::APPEARANCE_SAVE,
                    expected: "a mode and the light and dark themes' names",
                });
            };
            engine
                .save_appearance(&choice)
                .map(|()| CommandOutcome::Done)
        },
    );
}

impl Engine {
    /// Writes `choice` to the user settings file and reads the settings again, announcing
    /// `SettingsChanged`. Refuses without a user settings file, changing nothing.
    pub fn save_appearance(&mut self, choice: &AppearanceChoice) -> Result<(), EngineError> {
        self.settings.write_appearance(
            choice.mode,
            choice.light_theme.as_deref(),
            choice.dark_theme.as_deref(),
        )?;
        self.reload_settings()
    }
}
