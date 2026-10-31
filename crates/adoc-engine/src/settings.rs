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
        }
    }
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub editor: EditorSettings,
    pub workspace: WorkspaceSettings,
    pub tools: ToolSettings,
    pub git: GitSettings,
    pub pdf: PdfSettings,
    pub updates: UpdateSettings,
    pub sidebar: SidebarSettings,
    pub history: HistorySettings,
    pub spelling: SpellingSettings,
    /// Problems found while loading, such as unknown keys. Shown once by the front end.
    pub warnings: Vec<String>,
    user_file: Option<PathBuf>,
    workspace_file: Option<PathBuf>,
    /// Whether a settings file set `[git] enabled`, rather than leaving the default.
    git_enabled_set: bool,
    /// Git is off unless a settings file turns it on, as in the language server.
    git_opt_in: bool,
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
            state_dir: self.state_dir.clone(),
            ..Self::default()
        };
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
        *self = fresh;
        Ok(())
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
            || (section == "spelling" && key != "dictionaries")
            || section.starts_with("sidebar.")
            || (section == "sidebar" && key == "follow_active")
            || (section == "workspace" && key == "ignore");
        if !allowed_here && is_known(section, key) {
            self.warn(source, &name, "can only be set in the user settings file");
            return;
        }
        if let Err(problem) = self.set(section, key, value) {
            self.warn(source, &name, problem);
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
                self.editor.theme = match value.as_str() {
                    Some("auto") => ThemePreference::Auto,
                    Some("light") => ThemePreference::Light,
                    Some("dark") => ThemePreference::Dark,
                    _ => return Err("expected \"auto\", \"light\" or \"dark\""),
                };
            }
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

    fn warn(&mut self, source: &str, name: &str, problem: &str) {
        self.warnings.push(format!("{source}: `{name}`: {problem}"));
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
                "tools",
                "asciidoctor" | "git" | "dot" | "plantuml" | "d2" | "diagram_timeout"
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
                "history",
                "enabled" | "keep_days" | "max_versions" | "max_size_mb"
            )
    )
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
    fn workspace_layer_cannot_set_tools() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[tools]\nasciidoctor = \"/bin/evil\"\ndot = \"/bin/evil\"\nplantuml = \"/bin/evil\"\nd2 = \"/bin/evil\"\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.tools, ToolSettings::default());
        assert_eq!(settings.warnings.len(), 4);
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
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("adocfoundry.toml");
        fs::write(&user, "[tools]\ndiagram_timeout = 5\n").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
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
}
