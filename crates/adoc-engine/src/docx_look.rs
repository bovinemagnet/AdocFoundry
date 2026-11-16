//! A Word export's look: the PDF theme, a named Word template or none (the Word looks PRD,
//! `docs/prd/PRD-AdocFoundry_DOCX_Themes.md`, §5.1).
//!
//! The look is the first given of the export's own choice (a profile's becomes the export's),
//! then `[docx] look`. With `[docx] look = "pdf-theme"`, the default, Word follows a PDF theme
//! only when one is **named**: the page's `pdf-theme`, or `[pdf] theme` written in a settings
//! file. Unlike PDF it never falls back to the bundled `default`, so a workspace that names no
//! theme keeps the export's own look. Which theme a name means is worked out exactly as for PDF
//! (`Engine::theme_source`); Word templates are looked for beside the PDF themes, as
//! `<name>.dotx`, else `<name>.docx`. Whatever `[docx]` writes applies over the look
//! (`Engine::docx_options`).

use std::path::PathBuf;

/// The look an export or a profile asks for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LookChoice {
    /// A PDF theme by name or path, as `pdf-theme` names one.
    PdfTheme(String),
    /// A Word template by name or path (`.dotx` or `.docx`), beside the PDF themes.
    Template(String),
    /// Neither: the export's own look and the `[docx]` settings.
    None,
}

/// `[docx] look`: the look an export takes when neither it nor its profile chooses one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DocxLook {
    /// The PDF theme, when one is named.
    #[default]
    PdfTheme,
    /// The template `[docx] template` names.
    Template,
    None,
}

/// The kind of look an export used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookKind {
    PdfTheme,
    Template,
    None,
}

impl LookKind {
    /// How the MCP server and front ends name it: `pdf-theme`, `template` or `none`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PdfTheme => "pdf-theme",
            Self::Template => "template",
            Self::None => "none",
        }
    }
}

/// The look an export used: its kind, its name and the file it came from (none for a bundled
/// theme or for no theme).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportLook {
    pub kind: LookKind,
    pub name: String,
    pub path: Option<PathBuf>,
}

#[cfg(feature = "docx")]
mod resolve {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };

    use adoc_theme::{LoadedTheme, PageGeometry, ThemeSource};

    use super::{DocxLook, ExportLook, LookChoice, LookKind};
    use crate::{DocxExportArgs, Engine};

    /// A look worked out for one export.
    pub(crate) enum ResolvedLook {
        None,
        PdfTheme {
            name: String,
            source: ThemeSource,
            theme: Box<LoadedTheme>,
        },
        Template {
            name: String,
            path: PathBuf,
        },
    }

    impl ResolvedLook {
        pub(crate) fn report(&self) -> ExportLook {
            match self {
                Self::None => ExportLook {
                    kind: LookKind::None,
                    name: "none".to_owned(),
                    path: None,
                },
                Self::PdfTheme { name, source, .. } => ExportLook {
                    kind: LookKind::PdfTheme,
                    name: name.clone(),
                    path: match source {
                        ThemeSource::File(path) => Some(path.clone()),
                        ThemeSource::Bundled(_) => None,
                    },
                },
                Self::Template { name, path } => ExportLook {
                    kind: LookKind::Template,
                    name: name.clone(),
                    path: Some(path.clone()),
                },
            }
        }

        /// What the look gives the writer's styles, title page and covers.
        pub(crate) fn word_look(&self) -> adoc_docx::WordLook {
            match self {
                Self::PdfTheme { theme, .. } => adoc_docx::WordLook::from_theme(theme),
                Self::None | Self::Template { .. } => adoc_docx::WordLook::default(),
            }
        }

        /// The page the look gives: a PDF theme's, with the page's `pdf-page-size`,
        /// `pdf-page-layout` and `pdf-page-margin` over it, as PDF export reads them.
        pub(crate) fn page(&self, attributes: &BTreeMap<String, String>) -> Option<PageGeometry> {
            match self {
                Self::PdfTheme { theme, .. } => Some(adoc_theme::page_geometry(theme, attributes)),
                Self::None | Self::Template { .. } => None,
            }
        }
    }

    impl Engine {
        /// The look of a Word export of `args`, whose first page (or book) has `attributes`
        /// and is at `page`, or why it cannot be had.
        pub(crate) fn docx_look(
            &self,
            args: &DocxExportArgs,
            attributes: &BTreeMap<String, String>,
            page: Option<&Path>,
        ) -> Result<ResolvedLook, String> {
            let choice = match &args.look {
                Some(choice) => choice.clone(),
                None => match self.settings.docx.look {
                    DocxLook::None => LookChoice::None,
                    DocxLook::Template => match &self.settings.docx.template {
                        Some(name) => LookChoice::Template(name.clone()),
                        None => {
                            return Err("`[docx] look` is \"template\" but no template is named; \
                                 set `[docx] template`"
                                .to_owned())
                        }
                    },
                    // Only a named theme: the page's, or one a settings file wrote.
                    DocxLook::PdfTheme => match attributes
                        .get("pdf-theme")
                        .map(|name| name.trim())
                        .filter(|name| !name.is_empty())
                    {
                        Some(name) => LookChoice::PdfTheme(name.to_owned()),
                        None if self.settings.pdf_theme_named() => {
                            LookChoice::PdfTheme(self.settings.pdf.theme.clone())
                        }
                        None => LookChoice::None,
                    },
                },
            };
            match choice {
                LookChoice::None => Ok(ResolvedLook::None),
                LookChoice::PdfTheme(name) => {
                    let source = self.theme_source(Some(&name), attributes, page);
                    let theme = adoc_theme::load_theme_within(&source, &self.file_access)
                        .map_err(|error| error.to_string())?;
                    Ok(ResolvedLook::PdfTheme {
                        name,
                        source,
                        theme: Box::new(theme),
                    })
                }
                LookChoice::Template(name) => {
                    let path = self.template_path(&name, attributes, page)?;
                    Ok(ResolvedLook::Template { name, path })
                }
            }
        }

        /// The Word template `name` means: a path when it ends in `.dotx` or `.docx` (relative
        /// to the themes folder, else to the first root), else `<name>.dotx`, then
        /// `<name>.docx`, in the themes folder.
        fn template_path(
            &self,
            name: &str,
            attributes: &BTreeMap<String, String>,
            page: Option<&Path>,
        ) -> Result<PathBuf, String> {
            let folder = self.themes_dir(attributes, page);
            let lower = name.to_ascii_lowercase();
            let candidates: Vec<PathBuf> = if lower.ends_with(".dotx") || lower.ends_with(".docx") {
                let path = Path::new(name);
                let base = folder.clone().or_else(|| self.roots.first().cloned());
                vec![match base {
                    Some(base) if path.is_relative() => base.join(path),
                    _ => path.to_path_buf(),
                }]
            } else {
                let Some(folder) = folder else {
                    return Err(format!(
                        "no Word template `{name}`: there is no themes folder to look in; set \
                         `[pdf] themes_dir` or give the template's path"
                    ));
                };
                vec![
                    folder.join(format!("{name}.dotx")),
                    folder.join(format!("{name}.docx")),
                ]
            };
            let candidates: Vec<PathBuf> = candidates
                .iter()
                .map(|path| crate::normalize_path(path))
                .collect();
            candidates
                .iter()
                .find(|path| self.file_access.permits(path) && path.is_file())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "no Word template `{name}`: {} is not there",
                        candidates[0].display()
                    )
                })
        }
    }
}

#[cfg(feature = "docx")]
pub(crate) use resolve::ResolvedLook;
