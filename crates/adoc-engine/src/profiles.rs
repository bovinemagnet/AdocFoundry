//! Publication profiles: a PDF export saved under a name in `[pdf.profiles.<name>]`, with its
//! content (a page, a component version, or navigation files in a chosen order), output, theme,
//! fonts, attributes and standards, so a manual is published the same way each time and from
//! every front end. [`Engine::profile_export_args`] turns a profile into the [`ExportArgs`]
//! every front end exports.

use std::path::{Path, PathBuf};

use crate::{Engine, EngineError, ExportArgs, ExportTarget, PdfStandard};

/// A saved publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdfProfile {
    pub name: String,
    pub content: ProfileContent,
    /// Where the PDF is written, relative to the first workspace root; `<name>.pdf` there when
    /// unset.
    pub output: Option<String>,
    /// A bundled theme, a name in the themes directory, or a theme file relative to the first
    /// workspace root; otherwise the page's `pdf-theme`, then `[pdf] theme`.
    pub theme: Option<String>,
    /// Where the theme's fonts are, relative to the first workspace root.
    pub fonts_dir: Option<String>,
    /// Standards the PDF conforms to, besides `[pdf] standards`.
    pub standards: Vec<PdfStandard>,
    /// Attributes every page is read with before its own header, as Antora's are, so the
    /// document's metadata (`pdf-title`, `author`, `description`, `keywords`) can be set here.
    pub attributes: Vec<(String, String)>,
}

/// What a profile publishes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileContent {
    /// One page, as an article: its path relative to the first workspace root.
    Page(String),
    /// A component version, as a book in navigation order: `name` or `version@name`.
    Component(String),
    /// Navigation files, as a book in the order given: paths relative to the first root.
    Navigation(Vec<String>),
}

/// A theme PDF export can use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdfThemeChoice {
    /// The name `[pdf] theme`, a profile or `pdf-theme` gives it by.
    pub name: String,
    /// The theme file, for a workspace theme; `None` for a bundled one.
    pub file: Option<PathBuf>,
}

/// The themes bundled with PDF export.
pub const BUNDLED_THEMES: &[&str] = &["default", "default-sans", "default-for-print", "base"];

const PROFILE_KEYS: &[&str] = &[
    "page",
    "component",
    "nav",
    "output",
    "theme",
    "fonts_dir",
    "standards",
    "attributes",
];

/// A profile from its `[pdf.profiles.<name>]` table.
pub(crate) fn parse_profile(name: &str, table: &toml::Table) -> Result<PdfProfile, String> {
    if let Some(unknown) = table
        .keys()
        .find(|key| !PROFILE_KEYS.contains(&key.as_str()))
    {
        return Err(format!("unknown key `{unknown}`"));
    }
    let text = |key: &str| -> Result<Option<String>, String> {
        table
            .get(key)
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("`{key}`: expected a string"))
            })
            .transpose()
    };
    let nav = table
        .get("nav")
        .map(|value| {
            value
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| "`nav`: expected an array of strings".to_owned())
        })
        .transpose()?
        .filter(|files| !files.is_empty());
    let content = match (text("page")?, text("component")?, nav) {
        (Some(page), None, None) => ProfileContent::Page(page),
        (None, Some(component), None) => ProfileContent::Component(component),
        (None, None, Some(files)) => ProfileContent::Navigation(files),
        _ => return Err("expected one of `page`, `component` or `nav`".to_owned()),
    };
    let standards = match table.get("standards") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().and_then(crate::settings::pdf_standard))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| "`standards`: expected an array of standard names".to_owned())?,
    };
    let attributes = match table.get("attributes") {
        None => Vec::new(),
        Some(value) => value
            .as_table()
            .and_then(|attributes| {
                attributes
                    .iter()
                    .map(|(name, value)| match value {
                        toml::Value::String(text) => Some((name.clone(), text.clone())),
                        toml::Value::Integer(number) => Some((name.clone(), number.to_string())),
                        toml::Value::Float(number) => Some((name.clone(), number.to_string())),
                        // `true` sets an attribute empty, as `:name:` does.
                        toml::Value::Boolean(true) => Some((name.clone(), String::new())),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| {
                "`attributes`: expected a table of strings, numbers or `true`".to_owned()
            })?,
    };
    Ok(PdfProfile {
        name: name.to_owned(),
        content,
        output: text("output")?,
        theme: text("theme")?,
        fonts_dir: text("fonts_dir")?,
        standards,
        attributes,
    })
}

impl Engine {
    /// The export a profile describes, every path resolved against the first workspace root.
    pub fn profile_export_args(&self, name: &str) -> Result<ExportArgs, EngineError> {
        let profile = self
            .settings
            .pdf
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| EngineError::NotFound(format!("no PDF profile `{name}`")))?;
        let root = self
            .roots
            .first()
            .ok_or_else(|| EngineError::Refused("no workspace is open".to_owned()))?;
        let resolve = |path: &str| crate::normalize_path(&root.join(path));
        let target = match &profile.content {
            ProfileContent::Page(page) => ExportTarget::File(resolve(page)),
            ProfileContent::Component(spec) => {
                let (name, version) = match spec.split_once('@') {
                    Some((version, name)) => (name.to_owned(), Some(version.to_owned())),
                    None => (spec.clone(), None),
                };
                ExportTarget::Component { name, version }
            }
            ProfileContent::Navigation(files) => {
                ExportTarget::Navigation(files.iter().map(|file| resolve(file)).collect())
            }
        };
        let output = resolve(
            profile
                .output
                .as_deref()
                .unwrap_or(&format!("{}.pdf", profile.name)),
        );
        // A theme file is the workspace's; a name is looked up as any theme name is.
        let theme = profile.theme.as_ref().map(|theme| {
            if theme.ends_with(".yml") || theme.ends_with(".yaml") {
                resolve(theme).display().to_string()
            } else {
                theme.clone()
            }
        });
        Ok(ExportArgs {
            target,
            output,
            theme,
            standards: profile.standards.clone(),
            fonts_dir: profile.fonts_dir.as_deref().map(resolve),
            typst: None,
            attributes: profile.attributes.clone(),
        })
    }

    /// The themes an export can use: the bundled ones, then each `<name>-theme.yml` in
    /// `[pdf] themes_dir`, by name.
    #[must_use]
    pub fn pdf_themes(&self) -> Vec<PdfThemeChoice> {
        let mut themes: Vec<PdfThemeChoice> = BUNDLED_THEMES
            .iter()
            .map(|name| PdfThemeChoice {
                name: (*name).to_owned(),
                file: None,
            })
            .collect();
        let dir = self
            .settings
            .pdf
            .themes_dir
            .as_deref()
            .zip(self.roots.first())
            .map(|(dir, root)| root.join(dir));
        let mut found: Vec<PdfThemeChoice> = dir
            .as_deref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = theme_name(&path)?;
                Some(PdfThemeChoice {
                    name,
                    file: Some(crate::normalize_path(&path)),
                })
            })
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name));
        themes.extend(found);
        themes
    }
}

/// The name of a theme file, `acme` for `acme-theme.yml`.
fn theme_name(path: &Path) -> Option<String> {
    let file = path.file_name()?.to_str()?;
    let name = file.strip_suffix("-theme.yml")?;
    (!name.is_empty() && path.is_file()).then(|| name.to_owned())
}
