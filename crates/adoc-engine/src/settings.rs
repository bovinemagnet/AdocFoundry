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
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".to_owned(),
            font_size: 13,
            soft_wrap: true,
            tab_width: 2,
            theme: ThemePreference::Auto,
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolSettings {
    /// An absolute path to `asciidoctor`; discovered on `PATH` when empty.
    pub asciidoctor: String,
    /// An absolute path to `git`, which pushes (and signed commits) use; discovered on `PATH`
    /// when empty.
    pub git: String,
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub editor: EditorSettings,
    pub workspace: WorkspaceSettings,
    pub tools: ToolSettings,
    pub git: GitSettings,
    /// Problems found while loading, such as unknown keys. Shown once by the front end.
    pub warnings: Vec<String>,
    user_file: Option<PathBuf>,
    workspace_file: Option<PathBuf>,
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
                let name = format!("{section}.{key}");
                let allowed_here = layer == Layer::User
                    || section == "editor"
                    || (section == "workspace" && key == "ignore");
                if !allowed_here && is_known(section, key) {
                    self.warn(&source, &name, "can only be set in the user settings file");
                    continue;
                }
                if let Err(problem) = self.set(section, key, value) {
                    self.warn(&source, &name, problem);
                }
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
            ("git", "enabled") => self.git.enabled = boolean(value)?,
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
                        .filter(|kind| adoc_git::HostKind::from_name(kind).is_some())
                        .ok_or("expected \"github\", \"gitlab\", \"bitbucket\" or \"azure\"")?;
                    hosts.insert(host.to_lowercase(), kind.to_owned());
                }
                self.git.hosts = hosts;
            }
            _ => return Err("unknown setting"),
        }
        Ok(())
    }

    fn warn(&mut self, source: &str, name: &str, problem: &str) {
        self.warnings.push(format!("{source}: `{name}`: {problem}"));
    }
}

fn is_known(section: &str, key: &str) -> bool {
    matches!(
        (section, key),
        (
            "editor",
            "font_family" | "font_size" | "soft_wrap" | "tab_width" | "theme"
        ) | ("workspace", "ignore")
            | ("tools", "asciidoctor" | "git")
            | (
                "git",
                "enabled" | "show_markers" | "blame_on_hover" | "remote" | "hosts"
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
    fn workspace_layer_cannot_set_tools() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[tools]\nasciidoctor = \"/bin/evil\"\n",
        )
        .unwrap();
        let settings = Settings::load(None, Some(directory.path())).unwrap();
        assert_eq!(settings.tools.asciidoctor, "");
        assert_eq!(settings.warnings.len(), 1);
    }
}
