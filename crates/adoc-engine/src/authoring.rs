//! Authoring profiles (#102): named sets of attributes, `[authoring.profiles.<name>]`, to check
//! conditional content with. While one is chosen, pages are read with its attributes after the
//! playbook's, the component version's and Antora's: in the preview, the visual editor and
//! every export, so what is exported is what the preview shows.

use crate::{
    commands::{ids, register, spec},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, EngineEvent,
};

/// One authoring profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoringProfile {
    pub name: String,
    /// The attributes it sets, in the order written; `true` sets one empty.
    pub attributes: Vec<(String, String)>,
}

/// A profile's table, whose `attributes` are read as an export profile's are.
pub(crate) fn parse_profile(name: &str, table: &toml::Table) -> Result<AuthoringProfile, String> {
    if let Some(key) = table.keys().find(|key| key.as_str() != "attributes") {
        return Err(format!(
            "`{key}` is not a profile setting; expected `attributes`"
        ));
    }
    Ok(AuthoringProfile {
        name: name.to_owned(),
        attributes: crate::profiles::profile_attributes(table)?,
    })
}

pub(crate) fn register_authoring(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::PREVIEW_AUTHORING_PROFILE,
            "Set Authoring Profile",
            CommandCategory::View,
            None,
            CommandContext::Always,
        ),
        |engine, args| {
            let CommandArgs::AuthoringProfile(name) = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::PREVIEW_AUTHORING_PROFILE,
                    expected: "an authoring profile's name, or none",
                });
            };
            engine
                .set_authoring_profile(name.as_deref())
                .map(|()| CommandOutcome::Done)
        },
    );
}

impl Engine {
    /// The authoring profiles the settings give, by name.
    #[must_use]
    pub fn authoring_profiles(&self) -> &[AuthoringProfile] {
        &self.settings.authoring_profiles
    }

    /// The profile pages are read with, if one is chosen and the settings still give it.
    #[must_use]
    pub fn authoring_profile(&self) -> Option<&AuthoringProfile> {
        let chosen = self.authoring_profile.as_deref()?;
        self.authoring_profiles()
            .iter()
            .find(|profile| profile.name == chosen)
    }

    /// Chooses the profile `name`, or none, and reads every previewed and visually shown page
    /// again with it. Refuses a name the settings do not give, changing nothing.
    pub fn set_authoring_profile(&mut self, name: Option<&str>) -> Result<(), EngineError> {
        if let Some(name) = name {
            if !self
                .authoring_profiles()
                .iter()
                .any(|profile| profile.name == name)
            {
                return Err(EngineError::Refused(format!(
                    "no authoring profile is named \"{name}\""
                )));
            }
        }
        let before = self.authoring_attributes().to_vec();
        self.authoring_profile = name.map(str::to_owned);
        self.emit(EngineEvent::AuthoringProfileChanged);
        if self.authoring_attributes() != before {
            self.authoring_inputs_changed();
        }
        Ok(())
    }

    /// The chosen profile's attributes, or none.
    pub(crate) fn authoring_attributes(&self) -> &[(String, String)] {
        self.authoring_profile()
            .map_or(&[], |profile| profile.attributes.as_slice())
    }

    /// After the settings are read again: a chosen profile they no longer give is chosen no
    /// more, and pages are read again when its attributes changed.
    pub(crate) fn authoring_settings_changed(&mut self, before: &[(String, String)]) {
        if self.authoring_profile.is_some() && self.authoring_profile().is_none() {
            self.authoring_profile = None;
            self.emit(EngineEvent::AuthoringProfileChanged);
        }
        if self.authoring_attributes() != before {
            self.authoring_inputs_changed();
        }
    }

    fn authoring_inputs_changed(&mut self) {
        self.preview_workspace_changed();
        self.visual_workspace_changed();
    }
}
