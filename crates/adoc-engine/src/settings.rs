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

/// The Navigation and Modules trees: how far each opens, and whether the branches the user
/// opens and closes are remembered for the workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidebarSettings {
    pub remember_expansion: bool,
    pub navigation: ExpansionPolicy,
    pub modules: ExpansionPolicy,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            remember_expansion: true,
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
            ignore: ["node_modules", ".git", "build", "target", ".cache"]
                .map(str::to_owned)
                .to_vec(),
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
    /// Standards every export conforms to: `pdf-a-2a`, `pdf-ua-1`.
    pub standards: Vec<crate::PdfStandard>,
}

impl Default for PdfSettings {
    fn default() -> Self {
        Self {
            theme: "default".to_owned(),
            themes_dir: None,
            fonts_dir: None,
            site_url: None,
            standards: Vec::new(),
        }
    }
}

/// A PDF standard by its settings and command-line name.
#[must_use]
pub fn pdf_standard(name: &str) -> Option<crate::PdfStandard> {
    match name {
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
    /// Problems found while loading, such as unknown keys. Shown once by the front end.
    pub warnings: Vec<String>,
    user_file: Option<PathBuf>,
    workspace_file: Option<PathBuf>,
    /// Whether a settings file set `[git] enabled`, rather than leaving the default.
    git_enabled_set: bool,
    /// Git is off unless a settings file turns it on, as in the language server.
    git_opt_in: bool,
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
            || section.starts_with("sidebar.")
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
            ("sidebar", "remember_expansion") => {
                self.sidebar.remember_expansion = boolean(value)?;
            }
            ("sidebar.navigation", "default_expansion") => {
                self.sidebar.navigation = expansion(value)?;
            }
            ("sidebar.modules", "default_expansion") => self.sidebar.modules = expansion(value)?,
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
                    .ok_or("expected \"pdf-a-2a\" or \"pdf-ua-1\"")?;
            }
            _ => return Err("unknown setting"),
        }
        Ok(())
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
        ("sidebar", "remember_expansion")
            | (
                "sidebar.navigation" | "sidebar.modules",
                "default_expansion"
            )
            | (
                "editor",
                "font_family" | "font_size" | "soft_wrap" | "tab_width" | "theme" | "preview_tabs"
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
        assert_eq!(settings.workspace.ignore.len(), 5);
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
            "[pdf]\ntheme = \"acme\"\nthemes_dir = \"resources/themes\"\nstandards = [\"pdf-a-2a\"]\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.pdf.theme, "acme");
        assert_eq!(settings.pdf.themes_dir.as_deref(), Some("resources/themes"));
        assert_eq!(settings.pdf.standards, vec![crate::PdfStandard::A2a]);
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
}
