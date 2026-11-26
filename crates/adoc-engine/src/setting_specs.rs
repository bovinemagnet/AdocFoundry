//! The settings a front end offers to change (#80): what each is called and may be
//! ([`setting_specs`]), what it is now and which file says so ([`Engine::setting`]), and saving
//! one to the user settings file (`settings.save`), which keeps everything else in the file.

use crate::{
    commands::{ids, register, spec},
    settings::Layer,
    AutoSave, CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry,
    Engine, EngineError, Settings, ThemePreference,
};

/// A setting a front end offers.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingSpec {
    /// As the documentation names it, `editor.font_size`: its table and key in the file.
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// The page of settings it belongs to.
    pub page: &'static str,
    pub kind: SettingKind,
}

/// What a setting may be.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingKind {
    Switch,
    /// A whole number.
    Number {
        min: u32,
        max: u32,
    },
    /// A multiplier, whole or not.
    Scale {
        min: f64,
        max: f64,
    },
    /// Text, which empty is the default.
    Text,
    /// Names, or the like, in order.
    List,
    /// One of these values, each with its name.
    Choice(&'static [(&'static str, &'static str)]),
    /// The name of a light theme, or empty for the native one: the front end knows them.
    LightTheme,
    DarkTheme,
    /// The id of one of [`crate::AppearanceSettings::font_sets`].
    FontSet,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<String>),
}

/// Where a setting's value comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingSource {
    Default,
    User,
    /// The workspace's file, which wins over the user's.
    Workspace,
}

/// What `settings.save` saves.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingChange {
    /// The [`SettingSpec::id`].
    pub id: String,
    /// `None` takes the setting out of the file, which gives its default back.
    pub value: Option<SettingValue>,
}

const fn setting(
    page: &'static str,
    id: &'static str,
    title: &'static str,
    kind: SettingKind,
    description: &'static str,
) -> SettingSpec {
    SettingSpec {
        id,
        title,
        description,
        page,
        kind,
    }
}

const SECONDS: SettingKind = SettingKind::Number {
    min: 1,
    max: 86_400,
};

const SPECS: &[SettingSpec] = {
    use SettingKind::{Choice, DarkTheme, FontSet, LightTheme, List, Number, Scale, Switch, Text};
    &[
        setting(
            "Editor",
            "editor.font_family",
            "Font",
            Text,
            "The source editor's font, in place of the font set's. Empty uses the font set's.",
        ),
        setting(
            "Editor",
            "editor.font_size",
            "Font size",
            Number { min: 6, max: 72 },
            "The source editor's font size, in place of the font set's.",
        ),
        setting(
            "Editor",
            "editor.soft_wrap",
            "Wrap long lines",
            Switch,
            "Show a long line on as many rows as it needs.",
        ),
        setting(
            "Editor",
            "editor.tab_width",
            "Tab width",
            Number { min: 1, max: 16 },
            "The width of a tab, in spaces.",
        ),
        setting(
            "Editor",
            "editor.preview_tabs",
            "Preview tabs",
            Switch,
            "Open pages you browse to in one tab, which the next page replaces.",
        ),
        setting(
            "Editor",
            "editor.auto_save",
            "Auto-save",
            Choice(&[
                ("off", "Off"),
                ("on_focus_change", "When leaving the page"),
                ("after_delay", "A second after typing"),
            ]),
            "Save pages without asking.",
        ),
        setting(
            "Editor",
            "editor.attribute_values",
            "Attribute values",
            Switch,
            "Show the value of each attribute reference at the end of its line.",
        ),
        setting(
            "Appearance",
            "appearance.mode",
            "Mode",
            Choice(&[
                ("auto", "Follow the system"),
                ("light", "Light"),
                ("dark", "Dark"),
            ]),
            "Whether the app follows the system's appearance or stays light or dark.",
        ),
        setting(
            "Appearance",
            "appearance.light_theme",
            "Light theme",
            LightTheme,
            "The theme shown while the app is light.",
        ),
        setting(
            "Appearance",
            "appearance.dark_theme",
            "Dark theme",
            DarkTheme,
            "The theme shown while the app is dark.",
        ),
        setting(
            "Appearance",
            "appearance.font_set",
            "Font set",
            FontSet,
            "The fonts of the interface, the source editor and prose.",
        ),
        setting(
            "Appearance",
            "appearance.font_scale",
            "Text size",
            Scale { min: 0.5, max: 3. },
            "Multiplies the size of all text: 1.5 is half as large again.",
        ),
        setting(
            "Workspace",
            "workspace.ignore",
            "Ignored folders",
            List,
            "Names of folders that are not indexed, watched or shown, separated by commas. \
             Re-index Workspace applies a change to a workspace that is open.",
        ),
        setting(
            "Tools",
            "tools.asciidoctor",
            "Asciidoctor",
            Text,
            "The path to asciidoctor, for the language server's preview. Empty looks on PATH.",
        ),
        setting(
            "Tools",
            "tools.git",
            "Git",
            Text,
            "The path to git, for pushing and signed commits. Empty looks on PATH.",
        ),
        setting(
            "Tools",
            "tools.dot",
            "Graphviz",
            Text,
            "The path to dot, for Graphviz diagrams. Empty looks on PATH.",
        ),
        setting(
            "Tools",
            "tools.plantuml",
            "PlantUML",
            Text,
            "The path to plantuml, for PlantUML diagrams. Empty looks on PATH.",
        ),
        setting(
            "Tools",
            "tools.d2",
            "D2",
            Text,
            "The path to d2, for D2 diagrams. Empty looks on PATH.",
        ),
        setting(
            "Tools",
            "tools.node",
            "Node.js",
            Text,
            "The path to node, for Build Site. Empty looks on PATH.",
        ),
        setting(
            "Tools",
            "tools.diagram_timeout",
            "Diagram timeout",
            SECONDS,
            "Seconds a diagram may take to draw before it is stopped.",
        ),
        setting(
            "Tools",
            "tools.build_timeout",
            "Site build timeout",
            SECONDS,
            "Seconds a site build may take before it is stopped.",
        ),
        setting(
            "Tools",
            "tools.commit_timeout",
            "Signed commit timeout",
            SECONDS,
            "Seconds a signed commit may wait, for a passphrase or a security key.",
        ),
        setting(
            "Git",
            "git.enabled",
            "Use Git",
            Switch,
            "Find each workspace folder's repository, and show its branch and changes.",
        ),
        setting(
            "Git",
            "git.show_markers",
            "Change markers",
            Switch,
            "Mark changed lines in both editors.",
        ),
        setting(
            "Git",
            "git.blame_on_hover",
            "Blame on hover",
            Switch,
            "Show the commit that last changed a line when the pointer rests on its number.",
        ),
        setting(
            "Git",
            "git.remote",
            "Remote",
            Text,
            "The remote that pushes and links to the hosting site use.",
        ),
        setting(
            "PDF",
            "pdf.theme",
            "Theme",
            Text,
            "The theme of PDF exports: a bundled name, a name in the themes folder, or the path \
             of a .yml file.",
        ),
        setting(
            "PDF",
            "pdf.themes_dir",
            "Themes folder",
            Text,
            "The folder of PDF themes, relative to the first workspace folder.",
        ),
        setting(
            "PDF",
            "pdf.fonts_dir",
            "Fonts folder",
            Text,
            "The folder of PDF themes' fonts, relative to the first workspace folder.",
        ),
        setting(
            "PDF",
            "pdf.site_url",
            "Site address",
            Text,
            "The address of the published site, which links to pages a PDF does not hold use.",
        ),
        setting(
            "PDF",
            "pdf.standards",
            "Standards",
            List,
            "Standards every PDF conforms to, separated by commas: pdf-a-1b, pdf-a-1a, pdf-a-2a \
             and pdf-ua-1.",
        ),
    ]
};

/// The settings a front end offers, page by page.
#[must_use]
pub fn setting_specs() -> &'static [SettingSpec] {
    SPECS
}

impl Settings {
    /// What setting `id` of [`setting_specs`] is now.
    fn value_of(&self, id: &str) -> Option<SettingValue> {
        use SettingValue::{Bool, List, Number, Text};
        let text = |text: &str| Text(text.to_owned());
        let optional = |value: &Option<String>| Text(value.clone().unwrap_or_default());
        // Seconds far below what a float holds exactly.
        let seconds = |seconds: u64| Number(seconds as f64);
        Some(match id {
            "editor.font_family" => text(&self.editor.font_family),
            "editor.font_size" => Number(self.editor.font_size.into()),
            "editor.soft_wrap" => Bool(self.editor.soft_wrap),
            "editor.tab_width" => Number(self.editor.tab_width.into()),
            "editor.preview_tabs" => Bool(self.editor.preview_tabs),
            "editor.attribute_values" => Bool(self.editor.attribute_values),
            "editor.auto_save" => text(match self.editor.auto_save {
                AutoSave::Off => "off",
                AutoSave::OnFocusChange => "on_focus_change",
                AutoSave::AfterDelay => "after_delay",
            }),
            "appearance.mode" => text(match self.appearance.mode {
                ThemePreference::Auto => "auto",
                ThemePreference::Light => "light",
                ThemePreference::Dark => "dark",
            }),
            "appearance.light_theme" => optional(&self.appearance.light_theme),
            "appearance.dark_theme" => optional(&self.appearance.dark_theme),
            "appearance.font_set" => Text(self.appearance.chosen_font_set().id),
            "appearance.font_scale" => {
                // To the hundredth, which a scale is kept to.
                Number((f64::from(self.appearance.font_scale.factor()) * 100.).round() / 100.)
            }
            "workspace.ignore" => List(self.workspace.ignore.clone()),
            "tools.asciidoctor" => text(&self.tools.asciidoctor),
            "tools.git" => text(&self.tools.git),
            "tools.dot" => text(&self.tools.dot),
            "tools.plantuml" => text(&self.tools.plantuml),
            "tools.d2" => text(&self.tools.d2),
            "tools.node" => text(&self.tools.node),
            "tools.diagram_timeout" => seconds(self.tools.diagram_timeout),
            "tools.build_timeout" => seconds(self.tools.build_timeout),
            "tools.commit_timeout" => seconds(self.tools.commit_timeout),
            "git.enabled" => Bool(self.git.enabled),
            "git.show_markers" => Bool(self.git.show_markers),
            "git.blame_on_hover" => Bool(self.git.blame_on_hover),
            "git.remote" => text(&self.git.remote),
            "pdf.theme" => text(&self.pdf.theme),
            "pdf.themes_dir" => optional(&self.pdf.themes_dir),
            "pdf.fonts_dir" => optional(&self.pdf.fonts_dir),
            "pdf.site_url" => optional(&self.pdf.site_url),
            "pdf.standards" => List(
                self.pdf
                    .standards
                    .iter()
                    .filter_map(|standard| {
                        ["pdf-a-1b", "pdf-a-1a", "pdf-a-2a", "pdf-ua-1"]
                            .into_iter()
                            .find(|name| crate::pdf_standard(name) == Some(*standard))
                    })
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => return None,
        })
    }
}

impl SettingValue {
    /// As TOML has it; `None` for a number it has no whole or finite form of.
    fn to_toml(&self) -> Option<toml::Value> {
        Some(match self {
            Self::Bool(on) => (*on).into(),
            Self::Number(number) if !number.is_finite() => return None,
            // Whole, and small enough, once in a setting's range, to be exact.
            Self::Number(number) if number.fract() == 0. && number.abs() < 1e9 => {
                (*number as i64).into()
            }
            Self::Number(number) => ((number * 100.).round() / 100.).into(),
            Self::Text(text) => text.trim().to_owned().into(),
            Self::List(items) => items.clone().into(),
        })
    }
}

fn to_edit(value: &toml::Value) -> toml_edit::Value {
    match value {
        toml::Value::Boolean(on) => (*on).into(),
        toml::Value::Integer(number) => (*number).into(),
        toml::Value::Float(number) => (*number).into(),
        toml::Value::Array(items) => items
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<toml_edit::Array>()
            .into(),
        other => other.as_str().unwrap_or_default().into(),
    }
}

impl Engine {
    /// What setting `id` of [`setting_specs`] is now, and which file says so.
    #[must_use]
    pub fn setting(&self, id: &str) -> Option<(SettingValue, SettingSource)> {
        let value = self.settings.value_of(id)?;
        let source = match self.settings.written_in(id) {
            None => SettingSource::Default,
            Some(Layer::User) => SettingSource::User,
            Some(Layer::Workspace) => SettingSource::Workspace,
        };
        Some((value, source))
    }

    /// Writes `change` to the user settings file, changing nothing else in it, and reads the
    /// settings again. Refuses, the file left as it was, a value the settings could not use, a
    /// setting the workspace's file gives, and to write under unsaved edits of the file.
    pub fn save_setting(&mut self, change: &SettingChange) -> Result<(), EngineError> {
        let spec = SPECS
            .iter()
            .find(|spec| spec.id == change.id)
            .ok_or_else(|| EngineError::NotFound(format!("the setting `{}`", change.id)))?;
        let refused = |problem: &str| EngineError::Refused(format!("`{}`: {problem}", spec.id));
        if self.settings.written_in(spec.id) == Some(Layer::Workspace) {
            return Err(refused(&format!(
                "the workspace's {} gives it",
                crate::WORKSPACE_SETTINGS_FILE
            )));
        }
        let user = self
            .settings
            .user_file()
            .ok_or_else(|| refused("there is no user settings file to save to"))?
            .to_path_buf();
        let open = self.document_for_path(&user);
        if open
            .and_then(|id| self.document(id))
            .is_some_and(|document| document.dirty)
        {
            return Err(refused(
                "the settings file has unsaved changes in a tab; save or close it first",
            ));
        }
        let (section, key) = spec.id.split_once('.').unwrap_or((spec.id, ""));
        let value = match &change.value {
            // Empty is the default of every text.
            Some(SettingValue::Text(text)) if text.trim().is_empty() => None,
            Some(value) => {
                let value = value
                    .to_toml()
                    .ok_or_else(|| refused("expected a number"))?;
                self.settings
                    .clone()
                    .set(section, key, &value)
                    .map_err(refused)?;
                Some(to_edit(&value))
            }
            None => None,
        };
        // The mode takes the place of the older setting.
        let replaces = (spec.id == "appearance.mode").then_some(("editor", "theme"));
        self.settings
            .write_keys(section, replaces, &[(key, value)])?;
        if let (Some(id), Ok(text)) = (open, std::fs::read_to_string(&user)) {
            self.open_document_changed_on_disk(id, Some(text));
        }
        self.reload_settings()
    }
}

pub(crate) fn register_setting_specs(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::SETTINGS_SAVE,
            "Save Setting",
            CommandCategory::View,
            None,
            CommandContext::Always,
        ),
        |engine, args| {
            let CommandArgs::Setting(change) = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::SETTINGS_SAVE,
                    expected: "a setting and its value",
                });
            };
            engine.save_setting(&change).map(|()| CommandOutcome::Done)
        },
    );
}
