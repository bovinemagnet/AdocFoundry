//! Publication profiles: a PDF export saved under a name in `[pdf.profiles.<name>]`, with its
//! content (a page, a component version, or navigation files in a chosen order), output, theme,
//! fonts, attributes and standards, so a manual is published the same way each time and from
//! every front end. [`Engine::profile_export_args`] turns a profile into the [`ExportArgs`]
//! every front end exports.

use std::path::{Path, PathBuf};

use crate::{
    DocxExportArgs, Engine, EngineError, ExportArgs, ExportTarget, FileAccess, PdfStandard,
};

/// A saved publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdfProfile {
    pub name: String,
    pub content: ProfileContent,
    /// Where the PDF is written, relative to the first workspace root and inside it;
    /// `<name>.pdf` there when unset.
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
    let text = |key: &str| profile_text(table, key);
    let content = profile_content(table)?;
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
    let attributes = profile_attributes(table)?;
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

/// A profile's string `key`, when it has one.
pub(crate) fn profile_text(table: &toml::Table, key: &str) -> Result<Option<String>, String> {
    table
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("`{key}`: expected a string"))
        })
        .transpose()
}

/// What a profile publishes: exactly one of `page`, `component` and `nav`.
pub(crate) fn profile_content(table: &toml::Table) -> Result<ProfileContent, String> {
    let text = |key: &str| profile_text(table, key);
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
    match (text("page")?, text("component")?, nav) {
        (Some(page), None, None) => Ok(ProfileContent::Page(page)),
        (None, Some(component), None) => Ok(ProfileContent::Component(component)),
        (None, None, Some(files)) => Ok(ProfileContent::Navigation(files)),
        _ => Err("expected one of `page`, `component` or `nav`".to_owned()),
    }
}

/// A profile's `attributes`: strings, numbers, or `true` for an empty value.
pub(crate) fn profile_attributes(table: &toml::Table) -> Result<Vec<(String, String)>, String> {
    Ok(match table.get("attributes") {
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
        let target = profile_target(&profile.content, root);
        let output = resolve(
            profile
                .output
                .as_deref()
                .unwrap_or(&format!("{}.pdf", profile.name)),
        );
        // A profile comes with the workspace's files, so it writes only inside the workspace,
        // lexically and once links are followed; a front end's own output choice is not limited.
        if !FileAccess::within(vec![root.clone()]).permits(&output) {
            return Err(EngineError::Refused(format!(
                "PDF profile `{name}` writes {}, which is outside the workspace",
                output.display()
            )));
        }
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

    /// What would stop the profile `name` from exporting, found without exporting: an unknown
    /// profile, an output outside the workspace, content that is not in it (a page not indexed,
    /// a component version not in the catalog, a navigation file missing) or a theme that is
    /// neither a bundled or workspace theme nor a file there. Empty when it would start.
    #[must_use]
    pub fn profile_problems(&self, name: &str) -> Vec<String> {
        let args = match self.profile_export_args(name) {
            Ok(args) => args,
            Err(error) => return vec![error.to_string()],
        };
        let mut problems = self.target_problems(&args.target);
        if let Some(theme) = &args.theme {
            let file = theme.ends_with(".yml") || theme.ends_with(".yaml");
            let known = if file {
                Path::new(theme).is_file()
            } else {
                self.pdf_themes().iter().any(|choice| choice.name == *theme)
            };
            if !known {
                problems.push(format!("its theme `{theme}` is not found"));
            }
        }
        problems
    }

    /// The Word export a `[docx.profiles.<name>]` profile describes, every path resolved
    /// against the first workspace root, its output inside the workspace.
    pub fn docx_profile_export_args(&self, name: &str) -> Result<DocxExportArgs, EngineError> {
        let profile = self
            .settings
            .docx
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| EngineError::NotFound(format!("no DOCX profile `{name}`")))?;
        let root = self
            .roots
            .first()
            .ok_or_else(|| EngineError::Refused("no workspace is open".to_owned()))?;
        let output = crate::normalize_path(
            &root.join(
                profile
                    .output
                    .as_deref()
                    .unwrap_or(&format!("{}.docx", profile.name)),
            ),
        );
        // As a PDF profile's: the workspace's own files write only inside the workspace.
        if !FileAccess::within(vec![root.clone()]).permits(&output) {
            return Err(EngineError::Refused(format!(
                "DOCX profile `{name}` writes {}, which is outside the workspace",
                output.display()
            )));
        }
        Ok(DocxExportArgs {
            target: profile_target(&profile.content, root),
            output,
            attributes: profile.attributes.clone(),
            paper: profile.paper,
            landscape: profile.landscape,
            look: profile.look.clone(),
        })
    }

    /// What would stop the Word profile `name` from exporting, found without exporting: an
    /// unknown profile, an output outside the workspace, or content that is not in it.
    #[must_use]
    pub fn docx_profile_problems(&self, name: &str) -> Vec<String> {
        match self.docx_profile_export_args(name) {
            Ok(args) => self.target_problems(&args.target),
            Err(error) => vec![error.to_string()],
        }
    }

    /// What is missing of what a profile publishes: a page not indexed, a component version not
    /// in the catalog, a navigation file not there.
    pub(crate) fn target_problems(&self, target: &ExportTarget) -> Vec<String> {
        let mut problems = Vec::new();
        match target {
            ExportTarget::File(path) => {
                if self.workspace.index.file(path).is_none() {
                    problems.push(format!(
                        "its page {} is not in the workspace",
                        path.display()
                    ));
                }
            }
            ExportTarget::Component { name, version } => {
                let found = match version {
                    Some(version) => self
                        .workspace
                        .antora
                        .component(name, Some(version))
                        .is_some(),
                    None => !self.workspace.antora.versions_of(name).is_empty(),
                };
                if !found {
                    let version = version
                        .as_deref()
                        .map(|version| format!(" {version}"))
                        .unwrap_or_default();
                    problems.push(format!(
                        "its component {name}{version} is not in the workspace"
                    ));
                }
            }
            ExportTarget::Navigation(files) => {
                for file in files {
                    if !file.is_file() {
                        problems.push(format!("its navigation file {} is missing", file.display()));
                    }
                }
            }
            ExportTarget::Page(_) => {}
        }
        problems
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

/// What `content` names, its paths resolved against `root`.
pub(crate) fn profile_target(content: &ProfileContent, root: &Path) -> ExportTarget {
    let resolve = |path: &str| crate::normalize_path(&root.join(path));
    match content {
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
    }
}

/// The name of a theme file, `acme` for `acme-theme.yml`.
fn theme_name(path: &Path) -> Option<String> {
    let file = path.file_name()?.to_str()?;
    let name = file.strip_suffix("-theme.yml")?;
    (!name.is_empty() && path.is_file()).then(|| name.to_owned())
}
