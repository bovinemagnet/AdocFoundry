//! The navigation tree of a component version, built from the `nav` files its `antora.yml`
//! lists.

use std::{
    fmt, io,
    path::{Path, PathBuf},
};

use adoc_core::{Diagnostic, DiagnosticCode, DiagnosticSeverity, Location, SourceRange};

use crate::{
    parse_nav, parse_resource_id, AntoraCatalog, AntoraResourceId, ResolutionError, ResourceFamily,
};

/// The navigation of one nav file of a component version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavTree {
    pub component: String,
    pub version: Option<String>,
    /// The nav file this tree was read from.
    pub path: PathBuf,
    pub items: Vec<NavItem>,
    /// `adoc.antora.nav-unresolved` for each entry whose target does not resolve.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavItem {
    pub title: String,
    /// The fully qualified page the entry links to, or `None` for a title, plain text or a
    /// target that does not resolve.
    pub target: Option<AntoraResourceId>,
    /// The fragment the entry's link names after `#`, if any.
    pub fragment: Option<String>,
    /// The entry's line in the nav file.
    pub source: Location,
    /// Whether the entry is a `.Title` line, heading the list beneath it.
    pub list_title: bool,
    pub children: Vec<NavItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NavError {
    UnknownComponent {
        component: String,
        version: Option<String>,
    },
    Read {
        path: PathBuf,
        message: String,
    },
    /// A navigation file named outside every component's modules.
    OutsideModules {
        path: PathBuf,
    },
}

impl fmt::Display for NavError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownComponent { component, version } => match version {
                Some(version) => write!(
                    formatter,
                    "unknown Antora component `{version}@{component}`"
                ),
                None => write!(formatter, "unknown Antora component `{component}`"),
            },
            Self::Read { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::OutsideModules { path } => write!(
                formatter,
                "{} is not in a module of an Antora component",
                path.display()
            ),
        }
    }
}

impl std::error::Error for NavError {}

/// The navigation trees of `component` at `version`, one per file listed under `nav` in its
/// `antora.yml`, in that order. An empty list means the component declares no navigation.
///
/// `read` supplies file contents, so that unsaved buffers can stand in for files on disk.
pub fn nav_tree(
    catalog: &AntoraCatalog,
    component: &str,
    version: Option<&str>,
    read: &dyn Fn(&Path) -> io::Result<String>,
) -> Result<Vec<NavTree>, NavError> {
    let descriptor =
        catalog
            .component(component, version)
            .ok_or_else(|| NavError::UnknownComponent {
                component: component.to_owned(),
                version: version.map(str::to_owned),
            })?;
    descriptor
        .nav
        .iter()
        .map(|entry| {
            let path = normalise(&descriptor.root.join(entry));
            let text = read(&path).map_err(|error| NavError::Read {
                path: path.clone(),
                message: error.to_string(),
            })?;
            Ok(build_tree(catalog, component, version, path, &text))
        })
        .collect()
}

/// The navigation trees of `files`, in the order given, each read in the component version and
/// module it sits in, for a publication that chooses its own navigation.
pub fn nav_trees_of(
    catalog: &AntoraCatalog,
    files: &[PathBuf],
    read: &dyn Fn(&Path) -> io::Result<String>,
) -> Result<Vec<NavTree>, NavError> {
    files
        .iter()
        .map(|path| {
            let path = normalise(path);
            let context = catalog
                .context_for_path(&path)
                .ok_or_else(|| NavError::OutsideModules { path: path.clone() })?;
            let text = read(&path).map_err(|error| NavError::Read {
                path: path.clone(),
                message: error.to_string(),
            })?;
            Ok(build_tree(
                catalog,
                &context.component,
                context.version.as_deref(),
                path,
                &text,
            ))
        })
        .collect()
}

fn build_tree(
    catalog: &AntoraCatalog,
    component: &str,
    version: Option<&str>,
    path: PathBuf,
    text: &str,
) -> NavTree {
    let context = catalog.context_for_path(&path);
    let mut diagnostics = Vec::new();
    let flat: Vec<(u8, NavItem)> = parse_nav(text)
        .into_iter()
        .map(|entry| {
            let range = SourceRange::new(entry.start, entry.end);
            let fragment = entry
                .target
                .as_deref()
                .and_then(|target| target.split_once('#'))
                .map(|(_, fragment)| fragment.to_owned());
            let target = entry.target.as_deref().and_then(|target| {
                let page = target.split_once('#').map_or(target, |(page, _)| page);
                let resolved = parse_resource_id(page)
                    .map_err(|error| error.to_string())
                    .and_then(|id| {
                        let context = context.as_ref().ok_or_else(|| {
                            "the navigation file is not inside a module".to_owned()
                        })?;
                        catalog
                            .resolve_detailed(&id, context)
                            .map_err(|error| match error {
                                // Published from elsewhere in the playbook: not a problem here.
                                ResolutionError::UnknownComponent { .. }
                                | ResolutionError::UnknownVersion { .. } => String::new(),
                                other => other.to_string(),
                            })
                    });
                match resolved {
                    Ok(resolved) => {
                        let coordinate = &resolved.resource.coordinate;
                        Some(AntoraResourceId {
                            version: coordinate.version.clone(),
                            component: Some(coordinate.component.clone()),
                            module: Some(coordinate.module.clone()),
                            family: Some(ResourceFamily::Page),
                            path: slash_path(&coordinate.relative_path),
                        })
                    }
                    Err(message) => {
                        if !message.is_empty() {
                            diagnostics.push(Diagnostic {
                                code: DiagnosticCode::AntoraNavUnresolved,
                                severity: DiagnosticSeverity::Warning,
                                message: format!(
                                    "Navigation entry `{target}` does not resolve: {message}"
                                ),
                                range,
                            });
                        }
                        None
                    }
                }
            });
            (
                entry.level,
                NavItem {
                    title: entry.text,
                    target,
                    fragment,
                    source: Location {
                        path: path.clone(),
                        range,
                    },
                    list_title: entry.level == 0,
                    children: Vec::new(),
                },
            )
        })
        .collect();
    let mut position = 0;
    NavTree {
        component: component.to_owned(),
        version: version.map(str::to_owned),
        path,
        items: nest(&flat, &mut position, None),
        diagnostics,
    }
}

/// Nests entries by level: an entry owns the deeper entries that follow it. A `.Title`
/// (level 0) owns the list beneath it.
fn nest(flat: &[(u8, NavItem)], position: &mut usize, parent: Option<u8>) -> Vec<NavItem> {
    let mut items = Vec::new();
    while let Some((level, item)) = flat.get(*position) {
        if parent.is_some_and(|parent| *level <= parent) {
            break;
        }
        *position += 1;
        let mut item = item.clone();
        item.children = nest(flat, position, Some(*level));
        items.push(item);
    }
    items
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalise(path: &Path) -> PathBuf {
    let mut normalised = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalised.pop() {
                    normalised.push("..");
                }
            }
            other => normalised.push(other),
        }
    }
    normalised
}
