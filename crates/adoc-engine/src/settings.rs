//! TOML settings with a user layer and a workspace layer.
//!
//! The user file lives at `<config dir>/adocfoundry/adocfoundry.toml`; a workspace root may carry a
//! `.adocfoundry.toml` that overrides `editor.*` and `workspace.ignore`. Unknown or disallowed keys
//! produce warnings, never errors.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::EngineError;

pub const USER_SETTINGS_FILE: &str = "adocfoundry.toml";
pub const WORKSPACE_SETTINGS_FILE: &str = ".adocfoundry.toml";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemePreference {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorSettings {
    pub font_family: String,
    pub font_size: u32,
    pub soft_wrap: bool,
    pub tab_width: u32,
    pub theme: ThemePreference,
    /// Browsing to a page shows it in one reusable preview tab until it is edited or kept open.
    pub preview_tabs: bool,
    /// When pages are saved without Save.
    pub auto_save: AutoSave,
}

/// `[editor] auto_save`: when an edited page is saved without Save.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AutoSave {
    /// Only Save and Save All save.
    #[default]
    Off,
    /// When the page's tab or the window loses focus.
    OnFocusChange,
    /// [`crate::AUTO_SAVE_DELAY`] after typing stops.
    AfterDelay,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".to_owned(),
            font_size: 13,
            soft_wrap: true,
            tab_width: 2,
            theme: ThemePreference::Auto,
            preview_tabs: true,
            auto_save: AutoSave::Off,
        }
    }
}

/// How far a sidebar tree opens when it is first shown or reset: a number of branch levels,
/// counting the component version row as level 1, or every branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpansionPolicy {
    Depth(u32),
    All,
}

impl ExpansionPolicy {
    /// Whether a branch at `level` (1 for the top row) starts expanded.
    #[must_use]
    pub fn expands(self, level: usize) -> bool {
        match self {
            Self::All => true,
            Self::Depth(depth) => level <= depth as usize,
        }
    }
}

/// The Navigation and Modules trees: how far each opens, whether the branches the user opens
/// and closes are remembered for the workspace, and whether the Files, Navigation and Modules
/// trees reveal and select the active page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidebarSettings {
    pub remember_expansion: bool,
    pub follow_active: bool,
    pub navigation: ExpansionPolicy,
    pub modules: ExpansionPolicy,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            remember_expansion: true,
            follow_active: true,
            navigation: ExpansionPolicy::Depth(2),
            modules: ExpansionPolicy::Depth(2),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSettings {
    pub ignore: Vec<String>,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            ignore: adoc_core::DEFAULT_IGNORED_DIRECTORIES
                .iter()
                .map(|&name| name.to_owned())
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolSettings {
    /// An absolute path to `asciidoctor`; discovered on `PATH` when empty.
    pub asciidoctor: String,
    /// An absolute path to `git`, which pushes (and signed commits) use; discovered on `PATH`
    /// when empty.
    pub git: String,
    /// An absolute path to Graphviz's `dot`; discovered on `PATH` when empty.
    pub dot: String,
    /// An absolute path to `plantuml`; discovered on `PATH` when empty.
    pub plantuml: String,
    /// An absolute path to `d2`; discovered on `PATH` when empty.
    pub d2: String,
    /// Seconds a diagram tool may run before it is stopped.
    pub diagram_timeout: u64,
    /// An absolute path to Node.js's `node`, beside which Build Site finds `npm`; `npm` is
    /// discovered on `PATH` when empty.
    pub node: String,
    /// Seconds a site build may run, `npm install` included, before it is stopped.
    pub build_timeout: u64,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            asciidoctor: String::new(),
            git: String::new(),
            dot: String::new(),
            plantuml: String::new(),
            d2: String::new(),
            diagram_timeout: adoc_diagrams::DEFAULT_TIMEOUT.as_secs(),
            node: String::new(),
            build_timeout: crate::site_build::DEFAULT_BUILD_TIMEOUT.as_secs(),
        }
    }
}

/// The `[templates]` table: the folders of New Antora Project's own templates, each a
/// subfolder holding a `template.toml`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateSettings {
    /// The user's, `[templates] dir` in the user settings file: `templates` beside that file
    /// unless it says otherwise, and none without a user settings file.
    pub user_dir: Option<PathBuf>,
    /// A team's, `[templates] dir` in the workspace's `.adocfoundry.toml`, relative to the
    /// workspace and only inside it.
    pub workspace_dir: Option<PathBuf>,
}

/// The `[git]` table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSettings {
    /// When false, the engine looks for no repository and reports none.
    pub enabled: bool,
    /// Change markers in the editors.
    pub show_markers: bool,
    /// Line blame shown when hovering the gutter.
    pub blame_on_hover: bool,
    /// The remote that links to the hosting site and pushes use.
    pub remote: String,
    /// Self-hosted sites by host name: `github`, `gitlab`, `bitbucket` or `azure`.
    pub hosts: std::collections::BTreeMap<String, String>,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            show_markers: true,
            blame_on_hover: false,
            remote: "origin".to_owned(),
            hosts: std::collections::BTreeMap::new(),
        }
    }
}

/// The `[updates]` table: whether the app checks for a newer release at start-up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateSettings {
    /// At most once a day. The command checks whatever this says.
    pub check: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check: true }
    }
}

/// The `[history]` table: the saved versions of each page the app keeps beside the settings
/// file, for comparing and restoring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySettings {
    pub enabled: bool,
    /// Versions older than this many days are removed.
    pub keep_days: u32,
    /// At most this many versions of a page are kept, the oldest removed first.
    pub max_versions: u32,
    /// The history of every page together is kept under this many megabytes, the oldest
    /// versions removed first.
    pub max_size_mb: u32,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_days: 30,
            max_versions: 50,
            max_size_mb: 256,
        }
    }
}

/// Spell checking: whether it is on, the language of pages with no `lang` attribute, and a
/// folder of further Hunspell dictionaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpellingSettings {
    pub enabled: bool,
    pub language: String,
    pub dictionaries: Option<String>,
}

impl Default for SpellingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            language: "en_GB".to_owned(),
            dictionaries: None,
        }
    }
}

/// What colours the link graph's nodes.
pub use adoc_graph::ColourBy as GraphColour;

/// How the graph is laid out: by force, or as a mind map from one root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GraphLayout {
    #[default]
    Force,
    MindMap(adoc_graph::TreeStyle),
}

/// The `[graph]` table: what the link graph shows when it is first opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphSettings {
    /// The local graph's starting depth, 1 to 3.
    pub depth: u8,
    pub show_partials: bool,
    pub show_nav: bool,
    pub show_orphans: bool,
    pub show_unresolved: bool,
    pub colour_by: GraphColour,
    /// What the graph window groups the nodes by, if anything.
    pub group: Option<adoc_graph::GroupBy>,
    /// How the graph window and panel lay the graph out.
    pub layout: GraphLayout,
    /// Pages unchanged for longer than this many days are marked stale.
    pub stale_after_days: u32,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            depth: 1,
            show_partials: true,
            show_nav: true,
            show_orphans: true,
            show_unresolved: false,
            colour_by: GraphColour::Component,
            group: None,
            layout: GraphLayout::Force,
            stale_after_days: 365,
        }
    }
}

impl GraphSettings {
    /// The filter these settings start the graph with.
    #[must_use]
    pub fn filter(&self) -> adoc_graph::GraphFilter {
        adoc_graph::GraphFilter {
            partials: self.show_partials,
            nav: self.show_nav,
            orphans: self.show_orphans,
            unresolved: self.show_unresolved,
            components: None,
        }
    }
}

/// The `[pdf]` table: how PDF export finds its theme and fonts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdfSettings {
    /// A bundled theme (`default`, `default-sans`, `default-for-print`, `base`), a theme name
    /// found as `<name>-theme.yml` in `themes_dir`, or a theme file. A page's `pdf-theme`
    /// attribute wins over it.
    pub theme: String,
    /// Where named themes are found, relative to the first workspace root.
    pub themes_dir: Option<String>,
    /// Where a theme's fonts are found, relative to the first workspace root; the bundled
    /// fonts are always searched after it.
    pub fonts_dir: Option<String>,
    /// The site cross references out of an exported book link to.
    pub site_url: Option<String>,
    /// Standards every export conforms to: `pdf-a-1b`, `pdf-a-1a`, `pdf-a-2a`, `pdf-ua-1`.
    pub standards: Vec<crate::PdfStandard>,
    /// Publication profiles, `[pdf.profiles.<name>]`, by name; a workspace's replaces a user
    /// profile of the same name.
    pub profiles: Vec<crate::PdfProfile>,
}

impl Default for PdfSettings {
    fn default() -> Self {
        Self {
            theme: "default".to_owned(),
            themes_dir: None,
            fonts_dir: None,
            site_url: None,
            standards: Vec::new(),
            profiles: Vec::new(),
        }
    }
}

/// The `[quality]` table: whether the documentation quality audit is kept current, and the
/// policy it is judged by (`fail_on`, `require_complete`, `[quality.rules]`,
/// `[[quality.exclusions]]` and the optional `providers`). Both layers may set it: a rule's
/// severity from the workspace file wins, and the exclusions and providers of both files apply.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QualitySettings {
    /// Keep the audit current while editing. Off, the audit runs only when asked for.
    pub enabled: bool,
    pub policy: adoc_audit::Policy,
    /// Each problem with the `[quality]` settings, as in [`Settings::warnings`]: invalid
    /// configuration for a front end that gates on the policy, such as the command line.
    pub problems: Vec<String>,
}

/// A PDF standard by its settings and command-line name.
#[must_use]
pub fn pdf_standard(name: &str) -> Option<crate::PdfStandard> {
    match name {
        "pdf-a-1b" | "a-1b" => Some(crate::PdfStandard::A1b),
        "pdf-a-1a" | "a-1a" => Some(crate::PdfStandard::A1a),
        "pdf-a-2a" | "a-2a" => Some(crate::PdfStandard::A2a),
        "pdf-ua-1" | "ua-1" => Some(crate::PdfStandard::Ua1),
        _ => None,
    }
}

/// The `[appearance]` table, user file only: whether the app follows the system's light or dark
/// appearance or keeps one, and the named theme for each (`None` for the native default).
/// Without `mode`, `[editor] theme` gives it, as it did before the table existed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppearanceSettings {
    pub mode: ThemePreference,
    pub light_theme: Option<String>,
    pub dark_theme: Option<String>,
    /// Where user theme files are read from: `themes` beside the user settings file unless
    /// `themes_dir` names another folder (relative to that file's folder, `~/` the home folder).
    pub themes_dir: Option<PathBuf>,
    /// How many times larger than the default the application's text is drawn.
    pub font_scale: FontScale,
}

/// How many times larger than the default the application's text is drawn (`[appearance]
/// font_scale`, #374): 1 unless set, from [`FontScale::MIN`] to [`FontScale::MAX`]. Kept in
/// hundredths, so settings compare exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FontScale(u16);

impl FontScale {
    pub const MIN: f64 = 0.5;
    pub const MAX: f64 = 3.0;

    /// The scale `factor` gives, if it is within the range.
    #[must_use]
    pub fn new(factor: f64) -> Option<Self> {
        (Self::MIN..=Self::MAX)
            .contains(&factor)
            // Within the range, so it fits.
            .then(|| Self((factor * 100.0).round() as u16))
    }

    /// The multiplier: 1.5 draws text half as large again as the default.
    #[must_use]
    pub fn factor(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl Default for FontScale {
    fn default() -> Self {
        Self(100)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub editor: EditorSettings,
    pub appearance: AppearanceSettings,
    pub workspace: WorkspaceSettings,
    pub tools: ToolSettings,
    pub git: GitSettings,
    pub pdf: PdfSettings,
    pub updates: UpdateSettings,
    pub sidebar: SidebarSettings,
    pub history: HistorySettings,
    pub spelling: SpellingSettings,
    pub graph: GraphSettings,
    pub quality: QualitySettings,
    pub templates: TemplateSettings,
    /// Problems found while loading, such as unknown keys. Shown once by the front end.
    pub warnings: Vec<String>,
    user_file: Option<PathBuf>,
    workspace_file: Option<PathBuf>,
    /// Whether a settings file set `[git] enabled`, rather than leaving the default.
    git_enabled_set: bool,
    /// Whether the user file set `[appearance] mode`, which `[editor] theme` then gives way to.
    appearance_mode_set: bool,
    /// Whether a settings file set the older `[editor] theme`.
    editor_theme_set: bool,
    /// Git is off unless a settings file turns it on, as in the language server.
    git_opt_in: bool,
    /// Pages are never saved without Save, whatever the files say; see
    /// [`Settings::without_auto_save`].
    auto_save_off: bool,
    /// Where the app keeps unsaved text for recovery and the local history; see
    /// [`Settings::with_state_dir`].
    state_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Layer {
    User,
    Workspace,
}

impl Settings {
    /// The platform location of the user settings file.
    #[must_use]
    pub fn default_user_file() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("adocfoundry").join(USER_SETTINGS_FILE))
    }

    /// The platform location of the app's state: `~/.local/state/adocfoundry` on Linux (or
    /// `$XDG_STATE_HOME`), the local, never roaming, application data folder elsewhere. Kept
    /// apart from the settings file, which is often synchronised or kept in a repository.
    #[must_use]
    pub fn default_state_dir() -> Option<PathBuf> {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .map(|dir| dir.join("adocfoundry"))
    }

    /// Keeps unsaved text for recovery and the local history in `dir`, now and on every
    /// reload. Unset, as it is by default, nothing is kept.
    #[must_use]
    pub fn with_state_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.state_dir = dir;
        self
    }

    #[must_use]
    pub fn state_dir(&self) -> Option<&Path> {
        self.state_dir.as_deref()
    }

    /// Loads the user layer from `user_file` and the workspace layer from
    /// `<workspace_root>/.adocfoundry.toml`. Missing files are not an error.
    pub fn load(
        user_file: Option<&Path>,
        workspace_root: Option<&Path>,
    ) -> Result<Self, EngineError> {
        let mut settings = Self {
            user_file: user_file.map(Path::to_path_buf),
            workspace_file: workspace_root.map(|root| root.join(WORKSPACE_SETTINGS_FILE)),
            ..Self::default()
        };
        settings.reload()?;
        Ok(settings)
    }

    /// Re-reads both layers from the files they were loaded from.
    pub fn reload(&mut self) -> Result<(), EngineError> {
        let mut fresh = Self {
            user_file: self.user_file.clone(),
            workspace_file: self.workspace_file.clone(),
            git_opt_in: self.git_opt_in,
            auto_save_off: self.auto_save_off,
            state_dir: self.state_dir.clone(),
            ..Self::default()
        };
        fresh.templates.user_dir = fresh
            .user_file
            .as_deref()
            .and_then(Path::parent)
            .map(|folder| folder.join("templates"));
        fresh.appearance.themes_dir = fresh
            .user_file
            .as_deref()
            .and_then(Path::parent)
            .map(|folder| folder.join("themes"));
        if let Some(path) = fresh.user_file.clone() {
            if let Some(table) = read_table(&path)? {
                fresh.apply(&table, Layer::User, &path);
            }
        }
        if let Some(path) = fresh.workspace_file.clone() {
            if let Some(table) = read_table(&path)? {
                fresh.apply(&table, Layer::Workspace, &path);
            }
        }
        if fresh.git_opt_in && !fresh.git_enabled_set {
            fresh.git.enabled = false;
        }
        if fresh.auto_save_off {
            fresh.editor.auto_save = AutoSave::Off;
        }
        fresh.resolve_appearance_mode();
        *self = fresh;
        Ok(())
    }

    /// `[appearance] mode` if set, else the older `[editor] theme`; both kept in step.
    fn resolve_appearance_mode(&mut self) {
        if !self.appearance_mode_set {
            self.appearance.mode = self.editor.theme;
        } else if self.editor_theme_set {
            let source = self
                .user_file
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            self.warn(
                &source,
                "editor.theme",
                "is ignored, as `appearance.mode` is set; remove it",
            );
        }
        self.editor.theme = self.appearance.mode;
    }

    /// Writes the appearance to the user settings file, changing only `[appearance]`'s `mode`,
    /// `light_theme` and `dark_theme` (a `None` name is taken out) and taking out the older
    /// `[editor] theme` it replaces, keeping every other line and comment; then reads the
    /// settings again. Refuses without a user file, and leaves a file it cannot read as it is.
    pub fn save_appearance(
        &mut self,
        mode: ThemePreference,
        light_theme: Option<&str>,
        dark_theme: Option<&str>,
    ) -> Result<(), EngineError> {
        self.write_appearance(mode, light_theme, dark_theme)?;
        self.reload()
    }

    /// [`Self::save_appearance`]'s write, without reading the settings again.
    pub(crate) fn write_appearance(
        &self,
        mode: ThemePreference,
        light_theme: Option<&str>,
        dark_theme: Option<&str>,
    ) -> Result<(), EngineError> {
        let path = self.user_file.clone().ok_or_else(|| {
            EngineError::Refused("there is no user settings file to save to".to_owned())
        })?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(EngineError::io(&path, error)),
        };
        let mut document: toml_edit::DocumentMut = text
            .parse()
            .map_err(|error| EngineError::Settings(format!("{}: {error}", path.display())))?;
        if let Some(editor) = document
            .get_mut("editor")
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            editor.remove("theme");
        }
        let appearance = document
            .entry("appearance")
            .or_insert_with(toml_edit::table)
            .as_table_like_mut()
            .ok_or_else(|| {
                EngineError::Settings(format!("{}: `appearance` is not a table", path.display()))
            })?;
        let mode = match mode {
            ThemePreference::Auto => "auto",
            ThemePreference::Light => "light",
            ThemePreference::Dark => "dark",
        };
        for (key, text) in [
            ("mode", Some(mode)),
            ("light_theme", light_theme),
            ("dark_theme", dark_theme),
        ] {
            match (text, appearance.get_mut(key)) {
                // A key already there keeps its place and the comments written above it.
                (Some(text), Some(item)) => *item = toml_edit::value(text),
                (Some(text), None) => {
                    appearance.insert(key, toml_edit::value(text));
                }
                (None, _) => {
                    appearance.remove(key);
                }
            }
        }
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(|error| EngineError::io(folder, error))?;
        }
        // Through a temporary file, so a failed write never leaves half a settings file.
        let temporary = path.with_extension("toml.saving");
        fs::write(&temporary, document.to_string())
            .and_then(|()| fs::rename(&temporary, &path))
            .map_err(|error| {
                let _ = fs::remove_file(&temporary);
                EngineError::io(&path, error)
            })
    }

    #[must_use]
    pub fn user_file(&self) -> Option<&Path> {
        self.user_file.as_deref()
    }

    #[must_use]
    pub fn workspace_file(&self) -> Option<&Path> {
        self.workspace_file.as_deref()
    }

    /// Makes Git opt-in: off unless a settings file sets `[git] enabled = true`, now and on
    /// every reload. For front ends where Git is not expected, such as the language server.
    #[must_use]
    pub fn with_git_opt_in(mut self) -> Self {
        self.git_opt_in = true;
        if !self.git_enabled_set {
            self.git.enabled = false;
        }
        self
    }

    /// Turns `[editor] auto_save` off, now and on every reload, whatever the settings files
    /// say. For front ends whose client owns saving, such as the language server.
    #[must_use]
    pub fn without_auto_save(mut self) -> Self {
        self.auto_save_off = true;
        self.editor.auto_save = AutoSave::Off;
        self
    }

    /// Points the workspace layer at a new root; call [`Settings::reload`] afterwards.
    pub fn set_workspace_root(&mut self, root: Option<&Path>) {
        self.workspace_file = root.map(|root| root.join(WORKSPACE_SETTINGS_FILE));
    }

    fn apply(&mut self, table: &toml::Table, layer: Layer, path: &Path) {
        let source = path.display().to_string();
        for (section, value) in table {
            let Some(entries) = value.as_table() else {
                self.warn(&source, section, "expected a table");
                continue;
            };
            for (key, value) in entries {
                if section == "pdf" && key == "profiles" {
                    self.apply_profiles(&source, value);
                    continue;
                }
                if section == "quality" && key == "rules" {
                    self.apply_quality_rules(&source, value);
                    continue;
                }
                if section == "quality" && key == "exclusions" {
                    self.apply_quality_exclusions(&source, value);
                    continue;
                }
                if section == "quality" && key == "providers" {
                    self.apply_quality_providers(&source, value);
                    continue;
                }
                if section == "templates" && key == "dir" {
                    self.apply_templates_dir(&source, layer, path, value);
                    continue;
                }
                if section == "appearance" && key == "themes_dir" && layer == Layer::User {
                    self.apply_themes_dir(&source, path, value);
                    continue;
                }
                // `[sidebar.navigation]` and `[sidebar.modules]` are tables of their own.
                let panel = value.as_table().filter(|_| {
                    section == "sidebar" && matches!(key.as_str(), "navigation" | "modules")
                });
                match panel {
                    Some(panel) => {
                        let section = format!("sidebar.{key}");
                        for (key, value) in panel {
                            self.apply_one(&source, layer, &section, key, value);
                        }
                    }
                    None => self.apply_one(&source, layer, section, key, value),
                }
            }
        }
    }

    /// `[appearance] themes_dir`: a path relative to the user file's folder, `~/` the home folder.
    fn apply_themes_dir(&mut self, source: &str, file: &Path, value: &toml::Value) {
        let Some(text) = value.as_str().filter(|text| !text.trim().is_empty()) else {
            self.warn(source, "appearance.themes_dir", "expected a folder");
            return;
        };
        let base = file.parent().unwrap_or(Path::new(""));
        let path = match (text.strip_prefix("~/"), dirs::home_dir()) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => base.join(text),
        };
        self.appearance.themes_dir = Some(adoc_index::normalize_path(&path));
    }

    /// `[templates] dir`: from the user file, a path (`~/` the home folder) relative to the
    /// file's folder; from a workspace's, a folder inside the workspace.
    fn apply_templates_dir(
        &mut self,
        source: &str,
        layer: Layer,
        file: &Path,
        value: &toml::Value,
    ) {
        let Some(text) = value.as_str() else {
            self.warn(source, "templates.dir", "expected a string");
            return;
        };
        let base = file.parent().unwrap_or(Path::new(""));
        let path = match (text.strip_prefix("~/"), dirs::home_dir()) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => base.join(text),
        };
        let path = adoc_index::normalize_path(&path);
        match layer {
            Layer::User => self.templates.user_dir = Some(path),
            Layer::Workspace => {
                let base = adoc_index::normalize_path(base);
                // Lexically, and once links are followed, as far as the folder exists.
                let inside = path.starts_with(&base)
                    && fs::canonicalize(&path).ok().is_none_or(|real| {
                        fs::canonicalize(&base).is_ok_and(|real_base| real.starts_with(real_base))
                    });
                if inside {
                    self.templates.workspace_dir = Some(path);
                } else {
                    self.warn(
                        source,
                        "templates.dir",
                        "must be a folder inside the workspace",
                    );
                }
            }
        }
    }

    fn apply_one(
        &mut self,
        source: &str,
        layer: Layer,
        section: &str,
        key: &str,
        value: &toml::Value,
    ) {
        let name = format!("{section}.{key}");
        let allowed_here = layer == Layer::User
            || section == "editor"
            || section == "pdf"
            || section == "graph"
            || section == "quality"
            || (section == "spelling" && key != "dictionaries")
            || section.starts_with("sidebar.")
            || (section == "sidebar" && key == "follow_active")
            || (section == "workspace" && key == "ignore");
        if !allowed_here && is_known(section, key) {
            self.warn(source, &name, "can only be set in the user settings file");
            return;
        }
        if let Err(problem) = self.set(section, key, value) {
            if section == "quality" {
                self.warn_quality(source, &name, problem);
            } else {
                self.warn(source, &name, problem);
            }
        }
    }

    fn set(&mut self, section: &str, key: &str, value: &toml::Value) -> Result<(), &'static str> {
        match (section, key) {
            ("editor", "font_family") => self.editor.font_family = string(value)?,
            ("editor", "font_size") => self.editor.font_size = positive(value)?,
            ("editor", "soft_wrap") => {
                self.editor.soft_wrap = value.as_bool().ok_or("expected true or false")?;
            }
            ("editor", "tab_width") => self.editor.tab_width = positive(value)?,
            ("editor", "preview_tabs") => self.editor.preview_tabs = boolean(value)?,
            ("editor", "auto_save") => {
                self.editor.auto_save = match value.as_str() {
                    Some("off") => AutoSave::Off,
                    Some("on_focus_change") => AutoSave::OnFocusChange,
                    Some("after_delay") => AutoSave::AfterDelay,
                    _ => return Err("expected \"off\", \"on_focus_change\" or \"after_delay\""),
                };
            }
            ("editor", "theme") => {
                self.editor.theme = theme_preference(value)?;
                self.editor_theme_set = true;
            }
            ("appearance", "mode") => {
                self.appearance.mode = theme_preference(value)?;
                self.appearance_mode_set = true;
            }
            ("appearance", "light_theme") => self.appearance.light_theme = Some(theme_name(value)?),
            ("appearance", "dark_theme") => self.appearance.dark_theme = Some(theme_name(value)?),
            ("appearance", "font_scale") => self.appearance.font_scale = font_scale(value)?,
            ("workspace", "ignore") => {
                let items = value.as_array().ok_or("expected an array of strings")?;
                self.workspace.ignore = items.iter().map(string).collect::<Result<_, _>>()?;
            }
            ("tools", "asciidoctor") => self.tools.asciidoctor = string(value)?,
            ("tools", "git") => self.tools.git = string(value)?,
            ("tools", "dot") => self.tools.dot = string(value)?,
            ("tools", "plantuml") => self.tools.plantuml = string(value)?,
            ("tools", "d2") => self.tools.d2 = string(value)?,
            ("tools", "diagram_timeout") => self.tools.diagram_timeout = positive(value)?.into(),
            ("tools", "node") => self.tools.node = string(value)?,
            ("tools", "build_timeout") => self.tools.build_timeout = positive(value)?.into(),
            ("git", "enabled") => {
                self.git.enabled = boolean(value)?;
                self.git_enabled_set = true;
            }
            ("git", "show_markers") => self.git.show_markers = boolean(value)?,
            ("git", "blame_on_hover") => self.git.blame_on_hover = boolean(value)?,
            ("git", "remote") => self.git.remote = string(value)?,
            ("git", "hosts") => {
                let table = value
                    .as_table()
                    .ok_or("expected a table of host names and site kinds")?;
                let mut hosts = std::collections::BTreeMap::new();
                for (host, kind) in table {
                    let kind = kind
                        .as_str()
                        .filter(|kind| is_host_kind(kind))
                        .ok_or("expected \"github\", \"gitlab\", \"bitbucket\" or \"azure\"")?;
                    hosts.insert(host.to_lowercase(), kind.to_owned());
                }
                self.git.hosts = hosts;
            }
            ("updates", "check") => self.updates.check = boolean(value)?,
            ("history", "enabled") => self.history.enabled = boolean(value)?,
            ("history", "keep_days") => self.history.keep_days = positive(value)?,
            ("history", "max_versions") => self.history.max_versions = positive(value)?,
            ("history", "max_size_mb") => self.history.max_size_mb = positive(value)?,
            ("sidebar", "remember_expansion") => {
                self.sidebar.remember_expansion = boolean(value)?;
            }
            ("sidebar", "follow_active") => self.sidebar.follow_active = boolean(value)?,
            ("sidebar.navigation", "default_expansion") => {
                self.sidebar.navigation = expansion(value)?;
            }
            ("sidebar.modules", "default_expansion") => self.sidebar.modules = expansion(value)?,
            ("spelling", "enabled") => self.spelling.enabled = boolean(value)?,
            ("spelling", "language") => self.spelling.language = language_tag(value)?,
            ("spelling", "dictionaries") => self.spelling.dictionaries = Some(string(value)?),
            ("graph", "depth") => {
                self.graph.depth = value
                    .as_integer()
                    .and_then(|depth| u8::try_from(depth).ok())
                    .filter(|depth| (1..=3).contains(depth))
                    .ok_or("expected 1, 2 or 3")?;
            }
            ("graph", "show_partials") => self.graph.show_partials = boolean(value)?,
            ("graph", "show_nav") => self.graph.show_nav = boolean(value)?,
            ("graph", "show_orphans") => self.graph.show_orphans = boolean(value)?,
            ("graph", "show_unresolved") => self.graph.show_unresolved = boolean(value)?,
            ("graph", "colour_by") => {
                self.graph.colour_by = match value.as_str() {
                    Some("component") => GraphColour::Component,
                    Some("module") => GraphColour::Module,
                    Some("family") => GraphColour::Family,
                    Some("age") => GraphColour::Age,
                    _ => return Err("expected \"component\", \"module\", \"family\" or \"age\""),
                };
            }
            ("graph", "group") => {
                self.graph.group = match value.as_str() {
                    Some("off") => None,
                    Some("module") => Some(adoc_graph::GroupBy::Module),
                    Some("component") => Some(adoc_graph::GroupBy::Component),
                    _ => return Err("expected \"off\", \"module\" or \"component\""),
                };
            }
            ("graph", "stale_after_days") => {
                self.graph.stale_after_days = positive(value)?;
            }
            ("graph", "layout") => {
                self.graph.layout = match value.as_str() {
                    Some("force") => GraphLayout::Force,
                    Some("mind-map") => GraphLayout::MindMap(adoc_graph::TreeStyle::Horizontal),
                    Some("radial") => GraphLayout::MindMap(adoc_graph::TreeStyle::Radial),
                    _ => return Err("expected \"force\", \"mind-map\" or \"radial\""),
                };
            }
            ("quality", "enabled") => self.quality.enabled = boolean(value)?,
            ("quality", "fail_on") => {
                self.quality.policy.fail_on = value
                    .as_str()
                    .and_then(adoc_audit::FailOn::from_name)
                    .ok_or("expected \"none\", \"warning\" or \"error\"")?;
            }
            ("quality", "require_complete") => {
                self.quality.policy.require_complete = boolean(value)?;
            }
            ("pdf", "theme") => self.pdf.theme = string(value)?,
            ("pdf", "themes_dir") => self.pdf.themes_dir = Some(string(value)?),
            ("pdf", "fonts_dir") => self.pdf.fonts_dir = Some(string(value)?),
            ("pdf", "site_url") => self.pdf.site_url = Some(string(value)?),
            ("pdf", "standards") => {
                let items = value.as_array().ok_or("expected an array of strings")?;
                self.pdf.standards = items
                    .iter()
                    .map(|item| item.as_str().and_then(pdf_standard))
                    .collect::<Option<_>>()
                    .ok_or("expected \"pdf-a-1b\", \"pdf-a-1a\", \"pdf-a-2a\" or \"pdf-ua-1\"")?;
            }
            _ => return Err("unknown setting"),
        }
        Ok(())
    }

    /// `[pdf.profiles.<name>]`: each profile parsed whole, or left out with a warning.
    fn apply_profiles(&mut self, source: &str, value: &toml::Value) {
        let Some(profiles) = value.as_table() else {
            self.warn(source, "pdf.profiles", "expected a table of profiles");
            return;
        };
        for (name, table) in profiles {
            let parsed = table
                .as_table()
                .ok_or_else(|| "expected a table".to_owned())
                .and_then(|table| crate::profiles::parse_profile(name, table));
            match parsed {
                Ok(profile) => {
                    self.pdf.profiles.retain(|known| known.name != profile.name);
                    self.pdf.profiles.push(profile);
                    self.pdf
                        .profiles
                        .sort_by(|left, right| left.name.cmp(&right.name));
                }
                Err(problem) => self.warn(source, &format!("pdf.profiles.{name}"), &problem),
            }
        }
    }

    /// `[quality.rules]`: each rule's severity, or a warning.
    fn apply_quality_rules(&mut self, source: &str, value: &toml::Value) {
        let Some(rules) = value.as_table() else {
            self.warn_quality(
                source,
                "quality.rules",
                "expected a table of rules and severities",
            );
            return;
        };
        for (rule, level) in rules {
            let name = format!("quality.rules.{rule}");
            let Some(level) = level.as_str() else {
                self.warn_quality(source, &name, "expected a string");
                continue;
            };
            if let Err(problem) = self.quality.policy.set_severity(rule, level) {
                self.warn_quality(source, &name, &problem.to_string());
            }
        }
    }

    /// `[[quality.exclusions]]`: each exclusion added whole, or left out with a warning.
    fn apply_quality_exclusions(&mut self, source: &str, value: &toml::Value) {
        let Some(exclusions) = value.as_array() else {
            self.warn_quality(source, "quality.exclusions", "expected an array of tables");
            return;
        };
        for (index, exclusion) in exclusions.iter().enumerate() {
            let name = format!("quality.exclusions[{}]", index + 1);
            let Some(table) = exclusion.as_table() else {
                self.warn_quality(source, &name, "expected a table");
                continue;
            };
            for key in table.keys() {
                if !matches!(key.as_str(), "rule" | "resource" | "reason") {
                    self.warn_quality(source, &format!("{name}.{key}"), "unknown setting");
                }
            }
            let text = |key: &str| table.get(key).map(|value| value.as_str());
            let (Some(Some(rule)), Some(Some(resource))) = (text("rule"), text("resource")) else {
                self.warn_quality(source, &name, "expected `rule` and `resource` strings");
                continue;
            };
            let reason = match text("reason") {
                Some(Some(reason)) => reason,
                Some(None) => {
                    self.warn_quality(source, &format!("{name}.reason"), "expected a string");
                    continue;
                }
                None => "",
            };
            if let Err(problem) = self.quality.policy.add_exclusion(rule, resource, reason) {
                self.warn_quality(source, &name, &problem.to_string());
            }
        }
    }

    fn warn(&mut self, source: &str, name: &str, problem: &str) {
        self.warnings.push(format!("{source}: `{name}`: {problem}"));
    }

    /// `[quality] providers`: each optional provider named enabled, or a warning. Both layers'
    /// providers are enabled.
    fn apply_quality_providers(&mut self, source: &str, value: &toml::Value) {
        let Some(names) = value.as_array() else {
            self.warn_quality(source, "quality.providers", "expected an array of strings");
            return;
        };
        for (index, name) in names.iter().enumerate() {
            let setting = format!("quality.providers[{}]", index + 1);
            let Some(name) = name.as_str() else {
                self.warn_quality(source, &setting, "expected a string");
                continue;
            };
            if let Err(problem) = self.quality.policy.enable_provider(name) {
                self.warn_quality(source, &setting, &problem.to_string());
            }
        }
    }

    /// A warning about the `[quality]` settings, kept in [`QualitySettings::problems`] too.
    fn warn_quality(&mut self, source: &str, name: &str, problem: &str) {
        self.warn(source, name, problem);
        self.quality
            .problems
            .push(format!("{source}: `{name}`: {problem}"));
    }
}

/// Whether `kind` names a kind of Git hosting site `[git] hosts` may give.
fn is_host_kind(kind: &str) -> bool {
    #[cfg(feature = "git")]
    let known = adoc_git::HostKind::from_name(kind).is_some();
    #[cfg(not(feature = "git"))]
    let known = matches!(kind, "github" | "gitlab" | "bitbucket" | "azure");
    known
}

/// A language tag: letters, digits, `_` and `-`, as `en_GB` or `en-US`.
fn language_tag(value: &toml::Value) -> Result<String, &'static str> {
    value
        .as_str()
        .filter(|tag| {
            !tag.is_empty()
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .map(str::to_owned)
        .ok_or("expected a language tag such as \"en_GB\"")
}

/// A sidebar tree's expansion: levels, 0 or more, or `"all"`.
fn expansion(value: &toml::Value) -> Result<ExpansionPolicy, &'static str> {
    const EXPECTED: &str = "expected a whole number of levels, 0 or more, or \"all\"";
    if value.as_str() == Some("all") {
        return Ok(ExpansionPolicy::All);
    }
    let depth = value.as_integer().ok_or(EXPECTED)?;
    u32::try_from(depth)
        .map(ExpansionPolicy::Depth)
        .map_err(|_| EXPECTED)
}

fn is_known(section: &str, key: &str) -> bool {
    matches!(
        (section, key),
        ("sidebar", "remember_expansion" | "follow_active")
            | ("spelling", "enabled" | "language" | "dictionaries")
            | (
                "sidebar.navigation" | "sidebar.modules",
                "default_expansion"
            )
            | (
                "editor",
                "font_family"
                    | "font_size"
                    | "soft_wrap"
                    | "tab_width"
                    | "theme"
                    | "preview_tabs"
                    | "auto_save"
            )
            | ("workspace", "ignore")
            | (
                "appearance",
                "mode" | "light_theme" | "dark_theme" | "themes_dir" | "font_scale"
            )
            | (
                "tools",
                "asciidoctor"
                    | "git"
                    | "dot"
                    | "plantuml"
                    | "d2"
                    | "diagram_timeout"
                    | "node"
                    | "build_timeout"
            )
            | (
                "git",
                "enabled" | "show_markers" | "blame_on_hover" | "remote" | "hosts"
            )
            | (
                "pdf",
                "theme" | "themes_dir" | "fonts_dir" | "site_url" | "standards"
            )
            | ("updates", "check")
            | (
                "quality",
                "enabled" | "fail_on" | "require_complete" | "rules" | "exclusions" | "providers"
            )
            | (
                "graph",
                "depth"
                    | "show_partials"
                    | "show_nav"
                    | "show_orphans"
                    | "show_unresolved"
                    | "colour_by"
                    | "group"
                    | "layout"
                    | "stale_after_days"
            )
            | (
                "history",
                "enabled" | "keep_days" | "max_versions" | "max_size_mb"
            )
    )
}

fn theme_preference(value: &toml::Value) -> Result<ThemePreference, &'static str> {
    match value.as_str() {
        Some("auto") => Ok(ThemePreference::Auto),
        Some("light") => Ok(ThemePreference::Light),
        Some("dark") => Ok(ThemePreference::Dark),
        _ => Err("expected \"auto\", \"light\" or \"dark\""),
    }
}

/// A theme's name as the theme files give it.
fn theme_name(value: &toml::Value) -> Result<String, &'static str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or("expected a theme's name")
}

/// A multiplier of the default text size, whole or not.
fn font_scale(value: &toml::Value) -> Result<FontScale, &'static str> {
    const EXPECTED: &str = "expected a multiplier of the default size from 0.5 to 3, such as 1.5";
    let factor = match value {
        toml::Value::Float(factor) => *factor,
        // Small enough, once in range, to be exact.
        toml::Value::Integer(factor) => *factor as f64,
        _ => return Err(EXPECTED),
    };
    FontScale::new(factor).ok_or(EXPECTED)
}

fn string(value: &toml::Value) -> Result<String, &'static str> {
    value.as_str().map(str::to_owned).ok_or("expected a string")
}

fn boolean(value: &toml::Value) -> Result<bool, &'static str> {
    value.as_bool().ok_or("expected true or false")
}

fn positive(value: &toml::Value) -> Result<u32, &'static str> {
    value
        .as_integer()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number > 0)
        .ok_or("expected a positive integer")
}

fn read_table(path: &Path) -> Result<Option<toml::Table>, EngineError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(EngineError::io(path, error)),
    };
    text.parse::<toml::Table>()
        .map(Some)
        .map_err(|error| EngineError::Settings(format!("{}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_specification() {
        let settings = Settings::default();
        assert_eq!(settings.editor.font_size, 13);
        assert_eq!(settings.editor.tab_width, 2);
        assert!(settings.editor.soft_wrap);
        assert_eq!(
            settings.workspace.ignore.len(),
            adoc_core::DEFAULT_IGNORED_DIRECTORIES.len()
        );
    }

    #[test]
    fn missing_files_load_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings::load(
            Some(&directory.path().join("absent.toml")),
            Some(directory.path()),
        )
        .unwrap();
        assert_eq!(settings.editor, EditorSettings::default());
        assert!(settings.warnings.is_empty());
    }

    #[test]
    fn invalid_values_warn_and_keep_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(&user, "[editor]\nfont_size = -3\ntheme = \"purple\"\n").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.editor.font_size, 13);
        assert_eq!(settings.warnings.len(), 2);
    }

    #[test]
    fn graph_settings_have_defaults_and_load() {
        let defaults = Settings::default().graph;
        assert_eq!(defaults.depth, 1);
        assert_eq!(defaults.filter(), adoc_graph::GraphFilter::default());

        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join(WORKSPACE_SETTINGS_FILE);
        fs::write(
            &workspace,
            "[graph]\ndepth = 2\nshow_nav = false\nshow_unresolved = true\ncolour_by = \"module\"\n\
             group = \"component\"\nlayout = \"radial\"\nstale_after_days = 90\n[graph.extra]\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.graph.depth, 2);
        assert!(!settings.graph.show_nav && settings.graph.show_unresolved);
        assert_eq!(settings.graph.colour_by, GraphColour::Module);
        assert_eq!(settings.graph.group, Some(adoc_graph::GroupBy::Component));
        assert_eq!(settings.graph.stale_after_days, 90);
        assert_eq!(
            settings.graph.layout,
            GraphLayout::MindMap(adoc_graph::TreeStyle::Radial)
        );
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);

        fs::write(
            &workspace,
            "[graph]\ndepth = 4\ncolour_by = \"size\"\ngroup = \"family\"\nlayout = \"tree\"\n\
             stale_after_days = 0\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.graph, GraphSettings::default());
        assert_eq!(settings.warnings.len(), 5);
        fs::write(
            &workspace,
            "[graph]\ngroup = \"off\"\nlayout = \"mind-map\"\ncolour_by = \"age\"\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!((settings.graph.group, settings.warnings.len()), (None, 0));
        assert_eq!(settings.graph.colour_by, GraphColour::Age);
        assert_eq!(
            settings.graph.layout,
            GraphLayout::MindMap(adoc_graph::TreeStyle::Horizontal)
        );
    }

    #[test]
    fn syntax_errors_fail() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(&user, "[editor\n").unwrap();
        assert!(matches!(
            Settings::load(Some(&user), None),
            Err(EngineError::Settings(_))
        ));
    }

    #[test]
    fn git_settings_have_defaults_and_load() {
        let defaults = Settings::default();
        assert!(defaults.git.enabled && defaults.git.show_markers && !defaults.git.blame_on_hover);
        assert_eq!(defaults.git.remote, "origin");

        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(
            &user,
            "[git]\nenabled = false\nblame_on_hover = true\nremote = \"upstream\"\ncolour = 1\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert!(!settings.git.enabled && settings.git.blame_on_hover);
        assert_eq!(settings.git.remote, "upstream");
        assert_eq!(settings.warnings.len(), 1, "an unknown key warns");

        fs::write(
            &user,
            "[git.hosts]\n\"Git.Example.com\" = \"gitlab\"\n\"bad.example.com\" = \"svn\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.git.hosts.len(), 0, "a bad kind rejects the table");
        assert_eq!(settings.warnings.len(), 1);
        fs::write(&user, "[git.hosts]\n\"Git.Example.com\" = \"gitlab\"\n").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.git.hosts["git.example.com"], "gitlab");
    }

    #[test]
    fn pdf_settings_load_in_either_layer() {
        let defaults = Settings::default();
        assert_eq!(defaults.pdf.theme, "default");
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[pdf]\ntheme = \"acme\"\nthemes_dir = \"resources/themes\"\nstandards = [\"pdf-a-2a\", \"pdf-a-1b\", \"pdf-a-1a\"]\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.pdf.theme, "acme");
        assert_eq!(settings.pdf.themes_dir.as_deref(), Some("resources/themes"));
        assert_eq!(
            settings.pdf.standards,
            vec![
                crate::PdfStandard::A2a,
                crate::PdfStandard::A1b,
                crate::PdfStandard::A1a
            ]
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    }

    #[test]
    fn preview_tabs_default_on_and_follow_the_layers() {
        assert!(Settings::default().editor.preview_tabs);
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(&user, "[editor]\npreview_tabs = false\n").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert!(!settings.editor.preview_tabs);
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);

        // The workspace file overrides the user's.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[editor]\npreview_tabs = true\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(settings.editor.preview_tabs);

        // An invalid value warns and keeps the value beneath it.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[editor]\npreview_tabs = \"sometimes\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(!settings.editor.preview_tabs);
        assert_eq!(settings.warnings.len(), 1);
    }

    #[test]
    fn sidebar_expansion_defaults_to_two_levels() {
        let settings = Settings::default();
        assert_eq!(settings.sidebar.navigation, ExpansionPolicy::Depth(2));
        assert_eq!(settings.sidebar.modules, ExpansionPolicy::Depth(2));
        assert!(settings.sidebar.remember_expansion);
    }

    #[test]
    fn spelling_loads_per_layer() {
        let defaults = Settings::default().spelling;
        assert!(defaults.enabled);
        assert_eq!(defaults.language, "en_GB");
        assert_eq!(defaults.dictionaries, None);

        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(
            &user,
            "[spelling]\nenabled = false\nlanguage = \"en_US\"\ndictionaries = \"/opt/dicts\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert!(!settings.spelling.enabled);
        assert_eq!(settings.spelling.language, "en_US");
        assert_eq!(
            settings.spelling.dictionaries.as_deref(),
            Some("/opt/dicts")
        );

        // The workspace overrides the switch and the language, but not the folder.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[spelling]\nenabled = true\nlanguage = \"de-DE\"\ndictionaries = \"/tmp/other\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(settings.spelling.enabled);
        assert_eq!(settings.spelling.language, "de-DE");
        assert_eq!(
            settings.spelling.dictionaries.as_deref(),
            Some("/opt/dicts")
        );
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
        assert!(settings.warnings[0].contains("can only be set in the user settings file"));

        // An invalid language warns and keeps the layer beneath.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[spelling]\nlanguage = \"en GB\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert_eq!(settings.spelling.language, "en_US");
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
    }

    #[test]
    fn sidebar_follow_active_loads_per_layer() {
        assert!(Settings::default().sidebar.follow_active);
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(&user, "[sidebar]\nfollow_active = false\n").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert!(!settings.sidebar.follow_active);

        // The workspace overrides the user's.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[sidebar]\nfollow_active = true\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert!(settings.sidebar.follow_active);

        // An invalid value warns and keeps the value beneath it.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[sidebar]\nfollow_active = \"yes\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(!settings.sidebar.follow_active);
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
    }

    #[test]
    fn sidebar_expansion_loads_per_panel_and_layer() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(
            &user,
            "[sidebar]\nremember_expansion = false\n\n[sidebar.navigation]\ndefault_expansion = \"all\"\n\n[sidebar.modules]\ndefault_expansion = 0\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert_eq!(settings.sidebar.navigation, ExpansionPolicy::All);
        assert_eq!(settings.sidebar.modules, ExpansionPolicy::Depth(0));
        assert!(!settings.sidebar.remember_expansion);

        // The workspace overrides one panel and keeps the user's other; it may not set
        // `remember_expansion`.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[sidebar]\nremember_expansion = true\n\n[sidebar.modules]\ndefault_expansion = 3\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert_eq!(settings.sidebar.navigation, ExpansionPolicy::All);
        assert_eq!(settings.sidebar.modules, ExpansionPolicy::Depth(3));
        assert!(!settings.sidebar.remember_expansion);
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
        assert!(settings.warnings[0].contains("sidebar.remember_expansion"));
    }

    #[test]
    fn invalid_sidebar_expansion_keeps_the_layer_beneath() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(&user, "[sidebar.navigation]\ndefault_expansion = 4\n").unwrap();
        for bad in ["-1", "1.5", "true", "[1]", "\"some\"", "\"ALL\""] {
            fs::write(
                directory.path().join(WORKSPACE_SETTINGS_FILE),
                format!("[sidebar.navigation]\ndefault_expansion = {bad}\n"),
            )
            .unwrap();
            let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
            assert_eq!(
                settings.sidebar.navigation,
                ExpansionPolicy::Depth(4),
                "{bad}"
            );
            assert_eq!(settings.warnings.len(), 1, "{bad}: {:?}", settings.warnings);
        }
        fs::write(&user, "[sidebar.navigation]\nbreadth = 2\n").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.warnings.len(), 1, "an unknown key warns");
    }

    #[test]
    fn template_folders_are_beside_the_user_file_and_inside_the_workspace() {
        assert_eq!(Settings::default().templates, TemplateSettings::default());
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        let workspace = directory.path().join("docs");
        fs::create_dir(&workspace).unwrap();
        let settings = Settings::load(Some(&user), Some(&workspace)).unwrap();
        assert_eq!(
            settings.templates.user_dir,
            Some(adoc_index::normalize_path(
                &directory.path().join("templates")
            ))
        );
        assert_eq!(settings.templates.workspace_dir, None);

        fs::write(&user, "[templates]\ndir = \"house\"\n").unwrap();
        fs::write(
            workspace.join(WORKSPACE_SETTINGS_FILE),
            "[templates]\ndir = \"tools/templates\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(&workspace)).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert_eq!(
            settings.templates.user_dir,
            Some(adoc_index::normalize_path(&directory.path().join("house")))
        );
        assert_eq!(
            settings.templates.workspace_dir,
            Some(adoc_index::normalize_path(
                &workspace.join("tools/templates")
            ))
        );

        fs::write(
            workspace.join(WORKSPACE_SETTINGS_FILE),
            "[templates]\ndir = \"../elsewhere\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(&workspace)).unwrap();
        assert_eq!(settings.templates.workspace_dir, None);
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
    }

    #[test]
    fn workspace_layer_cannot_set_tools() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[tools]\nasciidoctor = \"/bin/evil\"\ndot = \"/bin/evil\"\nplantuml = \"/bin/evil\"\nd2 = \"/bin/evil\"\nnode = \"/bin/evil\"\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.tools, ToolSettings::default());
        assert_eq!(settings.warnings.len(), 5);
    }

    #[test]
    fn diagram_tool_paths_are_read_from_the_user_file() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        fs::write(
            &user,
            "[tools]\ndot = \"/opt/homebrew/bin/dot\"\nplantuml = \"/opt/plantuml\"\nd2 = \"/usr/local/bin/d2\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.tools.dot, "/opt/homebrew/bin/dot");
        assert_eq!(settings.tools.plantuml, "/opt/plantuml");
        assert_eq!(settings.tools.d2, "/usr/local/bin/d2");
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    }

    #[test]
    fn the_diagram_timeout_is_read_in_seconds() {
        assert_eq!(Settings::default().tools.diagram_timeout, 30);
        assert_eq!(Settings::default().tools.build_timeout, 600);
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        fs::write(
            &user,
            "[tools]\ndiagram_timeout = 5\nbuild_timeout = 90\nnode = \"/opt/node/bin/node\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.tools.build_timeout, 90);
        assert_eq!(settings.tools.node, "/opt/node/bin/node");
        assert_eq!(settings.tools.diagram_timeout, 5);
        fs::write(&user, "[tools]\ndiagram_timeout = 0\n").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.tools.diagram_timeout, 30);
        assert_eq!(settings.warnings.len(), 1);
    }

    #[test]
    fn the_state_folder_is_set_only_when_asked_and_kept_on_reload() {
        assert_eq!(Settings::default().state_dir(), None);
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        let mut settings = Settings::load(Some(&user), None)
            .unwrap()
            .with_state_dir(Some(directory.path().join("state")));
        assert_eq!(
            settings.state_dir(),
            Some(directory.path().join("state").as_path())
        );
        settings.reload().unwrap();
        assert_eq!(
            settings.state_dir(),
            Some(directory.path().join("state").as_path())
        );
        assert!(Settings::default_state_dir().is_some_and(|dir| dir.ends_with("adocfoundry")));
    }

    #[test]
    fn local_history_is_read_from_the_user_file_only() {
        let defaults = HistorySettings::default();
        assert!(defaults.enabled);
        assert_eq!((defaults.keep_days, defaults.max_versions), (30, 50));
        assert_eq!(defaults.max_size_mb, 256);
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        fs::write(
            &user,
            "[history]\nenabled = false\nkeep_days = 7\nmax_versions = 10\nmax_size_mb = 64\n",
        )
        .unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[history]\nkeep_days = 1\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(!settings.history.enabled);
        assert_eq!(settings.history.keep_days, 7);
        assert_eq!(settings.history.max_versions, 10);
        assert_eq!(settings.history.max_size_mb, 64);
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
        assert!(settings.warnings[0].contains("can only be set in the user settings file"));
    }

    #[test]
    fn quality_settings_default_to_off_and_the_default_policy() {
        let quality = Settings::default().quality;
        assert!(!quality.enabled);
        assert_eq!(quality.policy, crate::audit::Policy::default());
    }

    #[test]
    fn quality_settings_load_from_the_workspace_file() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            r#"
[quality]
enabled = true
fail_on = "warning"
require_complete = true

[quality.rules]
"quality.navigation.unlisted-page" = "info"
"quality.page.missing-title" = "off"

[[quality.exclusions]]
rule = "quality.navigation.unlisted-page"
resource = "product:ROOT:internal-notes.adoc"
reason = "Deliberately unlisted staff-only page"
"#,
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let quality = &settings.quality;
        assert!(quality.enabled);
        assert_eq!(quality.policy.fail_on, crate::audit::FailOn::Warning);
        assert!(quality.policy.require_complete);
        assert_eq!(
            quality.policy.severity(crate::audit::RuleId::UNLISTED_PAGE),
            Some(crate::audit::Severity::Info)
        );
        assert_eq!(
            quality.policy.severity(crate::audit::RuleId::MISSING_TITLE),
            None
        );
        let exclusions = quality.policy.exclusions();
        assert_eq!(exclusions.len(), 1);
        assert_eq!(exclusions[0].path, "internal-notes.adoc");
        assert_eq!(
            exclusions[0].reason,
            "Deliberately unlisted staff-only page"
        );
    }

    #[test]
    fn invalid_quality_settings_warn_and_are_left_out() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            r#"
[quality]
fail_on = "info"
require_complete = "yes"
colour = 1

[quality.rules]
"quality.no-such-rule" = "error"
"adoc.unresolved-xref-file" = "info"
"quality.image.missing-alt" = 3

[[quality.exclusions]]
rule = "quality.navigation.unlisted-page"
resource = "a.adoc"
reason = "Not qualified"

[[quality.exclusions]]
rule = "quality.navigation.unlisted-page"
resource = "product:ROOT:a.adoc"

[[quality.exclusions]]
rule = "quality.navigation.unlisted-page"
resource = "product:ROOT:b.adoc"
reason = "Kept"

[[quality.exclusions]]
rule = "quality.navigation.unlisted-page"
resource = "product:ROOT:b.adoc"
reason = "Again"

[[quality.exclusions]]
rule = "quality.navigation.unlisted-page"
resource = "product:ROOT:c.adoc"
reason = "Kept, with a note warned about"
note = "unknown"
"#,
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.warnings.len(), 10, "{:#?}", settings.warnings);
        assert_eq!(
            settings.quality.problems, settings.warnings,
            "every one is a problem with the quality settings"
        );
        for expected in [
            "`quality.fail_on`",
            "`quality.require_complete`",
            "`quality.colour`: unknown setting",
            "`quality.rules.quality.no-such-rule`: unknown rule",
            "keeps its own severity",
            "`quality.rules.quality.image.missing-alt`: expected a string",
            "must name its component and module",
            "needs a reason",
            "is already excluded",
            "`quality.exclusions[5].note`: unknown setting",
        ] {
            assert!(
                settings
                    .warnings
                    .iter()
                    .any(|warning| warning.contains(expected)),
                "no warning containing {expected:?} in {:#?}",
                settings.warnings
            );
        }
        let policy = &settings.quality.policy;
        assert_eq!(policy.fail_on, crate::audit::FailOn::Error);
        assert!(!policy.require_complete);
        let kept: Vec<&str> = policy
            .exclusions()
            .iter()
            .map(|exclusion| exclusion.path.as_str())
            .collect();
        assert_eq!(kept, ["b.adoc", "c.adoc"]);
    }

    #[test]
    fn quality_providers_are_enabled_by_name_from_either_layer() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(&user, "[quality]\nproviders = [\"vale\"]\n").unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[quality]\nproviders = [\"external-links\", \"spellcheck\", 3]\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        let policy = &settings.quality.policy;
        assert!(policy.provider_enabled(crate::audit::ProviderId::VALE));
        assert!(policy.provider_enabled(crate::audit::ProviderId::EXTERNAL_LINKS));
        assert!(!policy.provider_enabled(crate::audit::ProviderId::PAGE_AGE));
        assert_eq!(settings.warnings.len(), 2, "{:#?}", settings.warnings);
        assert!(
            settings.warnings[0].contains("`quality.providers[2]`: unknown provider `spellcheck`")
        );
        assert!(settings.warnings[1].contains("`quality.providers[3]`: expected a string"));
        assert_eq!(settings.quality.problems, settings.warnings);
    }

    #[test]
    fn quality_exclusions_from_both_layers_apply_and_the_workspace_sets_rules_last() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(
            &user,
            "[quality.rules]\n\"quality.navigation.unlisted-page\" = \"error\"\n\
             [[quality.exclusions]]\nrule = \"quality.navigation.unlisted-page\"\n\
             resource = \"product:ROOT:b.adoc\"\nreason = \"From the user file\"\n",
        )
        .unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[quality.rules]\n\"quality.navigation.unlisted-page\" = \"info\"\n\
             [[quality.exclusions]]\nrule = \"quality.navigation.unlisted-page\"\n\
             resource = \"product:ROOT:c.adoc\"\nreason = \"From the workspace\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let policy = &settings.quality.policy;
        assert_eq!(
            policy.severity(crate::audit::RuleId::UNLISTED_PAGE),
            Some(crate::audit::Severity::Info)
        );
        assert_eq!(policy.exclusions().len(), 2);
    }

    #[test]
    fn appearance_legacy_theme_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        for (legacy, mode) in [
            ("auto", ThemePreference::Auto),
            ("light", ThemePreference::Light),
            ("dark", ThemePreference::Dark),
        ] {
            let text = format!("# mine\n[editor]\ntheme = \"{legacy}\"\n");
            fs::write(&user, &text).unwrap();
            let settings = Settings::load(Some(&user), None).unwrap();
            assert_eq!(settings.appearance.mode, mode);
            assert_eq!(settings.editor.theme, mode);
            assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
            assert_eq!(
                fs::read_to_string(&user).unwrap(),
                text,
                "loading writes nothing"
            );
        }
    }

    #[test]
    fn appearance_precedence_and_user_only() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(
            &user,
            "[editor]\ntheme = \"dark\"\n[appearance]\nmode = \"light\"\n\
             light_theme = \"Solarized Light\"\ndark_theme = \"Tokyo Night\"\n",
        )
        .unwrap();
        let root = directory.path().join("docs");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join(WORKSPACE_SETTINGS_FILE),
            "[appearance]\nmode = \"dark\"\ndark_theme = \"Catppuccin Mocha\"\n\
             themes_dir = \"themes\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(&root)).unwrap();
        let appearance = &settings.appearance;
        assert_eq!(appearance.mode, ThemePreference::Light);
        assert_eq!(appearance.light_theme.as_deref(), Some("Solarized Light"));
        assert_eq!(appearance.dark_theme.as_deref(), Some("Tokyo Night"));
        assert_eq!(appearance.themes_dir, Some(directory.path().join("themes")));
        let warnings = settings.warnings.join("\n");
        assert!(warnings.contains("`editor.theme`"), "{warnings}");
        for key in [
            "appearance.mode",
            "appearance.dark_theme",
            "appearance.themes_dir",
        ] {
            assert!(
                warnings.contains(&format!(
                    "`{key}`: can only be set in the user settings file"
                )),
                "{key}: {warnings}"
            );
        }
        assert_eq!(settings.warnings.len(), 4, "{warnings}");
    }

    #[test]
    fn appearance_invalid_values_warn_and_keep_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(
            &user,
            "[appearance]\nmode = \"purple\"\nlight_theme = \"\"\nthemes_dir = \"~/my-themes\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert_eq!(settings.appearance.mode, ThemePreference::Auto);
        assert_eq!(settings.appearance.light_theme, None);
        assert_eq!(settings.warnings.len(), 2, "{:?}", settings.warnings);
        if let Some(home) = dirs::home_dir() {
            assert_eq!(settings.appearance.themes_dir, Some(home.join("my-themes")));
        }
    }

    #[test]
    fn appearance_font_scale_is_a_multiplier_of_the_default() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        let root = directory.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        let scale = |text: &str| {
            fs::write(&user, text).unwrap();
            let settings = Settings::load(Some(&user), None).unwrap();
            (settings.appearance.font_scale.factor(), settings.warnings)
        };
        assert_eq!(Settings::default().appearance.font_scale.factor(), 1.0);
        assert_eq!(scale("[appearance]\nfont_scale = 1.5\n"), (1.5, vec![]));
        assert_eq!(scale("[appearance]\nfont_scale = 2\n"), (2.0, vec![]));
        assert_eq!(scale("[appearance]\nfont_scale = 0.5\n").0, 0.5);
        assert_eq!(scale("[appearance]\nfont_scale = 3\n").0, 3.0);
        for wrong in ["0", "0.25", "3.5", "-1", "\"big\""] {
            let (factor, warnings) = scale(&format!("[appearance]\nfont_scale = {wrong}\n"));
            assert_eq!(factor, 1.0, "{wrong}");
            assert_eq!(warnings.len(), 1, "{wrong}: {warnings:?}");
            assert!(warnings[0].contains("from 0.5 to 3"), "{warnings:?}");
        }
        // A screen is the machine's, not the workspace's.
        fs::write(&user, "").unwrap();
        fs::write(
            root.join(".adocfoundry.toml"),
            "[appearance]\nfont_scale = 2\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(&root)).unwrap();
        assert_eq!(settings.appearance.font_scale.factor(), 1.0);
        assert!(settings.warnings[0].contains("can only be set in the user settings file"));
    }

    #[test]
    fn appearance_saves_only_its_keys() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("config/user.toml");
        let mut settings = Settings::load(Some(&user), None).unwrap();
        // No file yet: one is made.
        settings
            .save_appearance(ThemePreference::Dark, None, Some("Tokyo Night"))
            .unwrap();
        assert_eq!(settings.appearance.mode, ThemePreference::Dark);
        assert_eq!(
            settings.appearance.dark_theme.as_deref(),
            Some("Tokyo Night")
        );

        fs::write(
            &user,
            "# My settings\n[editor]\nfont_size = 15 # bigger\ntheme = \"light\"\n\n\
             [appearance]\n# chosen in the app\nmode = \"dark\"\n",
        )
        .unwrap();
        settings.reload().unwrap();
        settings
            .save_appearance(
                ThemePreference::Auto,
                Some("Solarized Light"),
                Some("Catppuccin Mocha"),
            )
            .unwrap();
        let text = fs::read_to_string(&user).unwrap();
        assert!(
            text.starts_with("# My settings\n[editor]\nfont_size = 15 # bigger\n"),
            "{text}"
        );
        assert!(text.contains("# chosen in the app\n"), "{text}");
        assert!(
            !text.contains("theme = \"light\""),
            "the legacy key goes: {text}"
        );
        assert!(text.contains("mode = \"auto\""), "{text}");
        assert!(text.contains("light_theme = \"Solarized Light\""), "{text}");
        assert_eq!(settings.editor.font_size, 15);
        assert_eq!(settings.appearance.mode, ThemePreference::Auto);
        assert_eq!(
            settings.appearance.light_theme.as_deref(),
            Some("Solarized Light")
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);

        // A name left out is taken out of the file.
        settings
            .save_appearance(ThemePreference::Light, None, Some("Catppuccin Mocha"))
            .unwrap();
        assert!(!fs::read_to_string(&user).unwrap().contains("light_theme"));
    }

    #[test]
    fn appearance_cannot_be_saved_without_a_user_file() {
        let mut settings = Settings::default();
        assert!(settings
            .save_appearance(ThemePreference::Dark, None, None)
            .is_err());
    }
}
