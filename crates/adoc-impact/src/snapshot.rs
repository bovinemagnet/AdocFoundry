//! One side of a comparison: the workspace's index and catalog at one revision, and the
//! dependency questions the rules ask of it, each answered through the index's reverse maps.

use std::{
    borrow::Cow,
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use adoc_antora::{AntoraResource, ResourceFamily};
use adoc_core::SourceRange;
use adoc_index::normalize_path;
use adoc_lang::{graph::LinkKind, WorkspaceState};

/// The dependency evidence of one revision: its index and catalog, borrowed from the live
/// workspace or owned for a revision built from Git objects, and the roots its paths are shown
/// relative to.
#[derive(Clone, Debug)]
pub struct DependencySnapshot<'a> {
    state: Cow<'a, WorkspaceState>,
    roots: Vec<PathBuf>,
}

/// A reference that leads to a file: who writes it, where, and its target as written.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Referrer {
    pub(crate) path: PathBuf,
    pub(crate) range: SourceRange,
    pub(crate) written: String,
}

impl<'a> DependencySnapshot<'a> {
    /// The live workspace's evidence, borrowed.
    #[must_use]
    pub fn borrowed(state: &'a WorkspaceState, roots: &[PathBuf]) -> Self {
        Self {
            state: Cow::Borrowed(state),
            roots: roots.iter().map(|root| normalize_path(root)).collect(),
        }
    }

    /// A revision's evidence, built elsewhere and owned.
    #[must_use]
    pub fn owned(state: WorkspaceState, roots: &[PathBuf]) -> DependencySnapshot<'static> {
        DependencySnapshot {
            state: Cow::Owned(state),
            roots: roots.iter().map(|root| normalize_path(root)).collect(),
        }
    }

    #[must_use]
    pub fn state(&self) -> &WorkspaceState {
        &self.state
    }

    /// `path` relative to the root holding it, with `/`; never absolute.
    pub(crate) fn display(&self, path: &Path) -> String {
        self.roots
            .iter()
            .filter_map(|root| path.strip_prefix(root).ok())
            .min_by_key(|relative| relative.components().count())
            .map(|relative| {
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
    }

    pub(crate) fn inside_roots(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    /// Whether the revision has the file: indexed, or a resource of the catalog.
    pub(crate) fn has(&self, path: &Path) -> bool {
        self.state.index.file(path).is_some() || self.state.antora.resource_for_path(path).is_some()
    }

    pub(crate) fn resource(&self, path: &Path) -> Option<&AntoraResource> {
        self.state.antora.resource_for_path(path)
    }

    pub(crate) fn is_page(&self, path: &Path) -> bool {
        self.resource(path)
            .is_some_and(|resource| resource.coordinate.family == ResourceFamily::Page)
    }

    pub(crate) fn is_asset(&self, path: &Path) -> bool {
        self.resource(path).is_some_and(|resource| {
            matches!(
                resource.coordinate.family,
                ResourceFamily::Image | ResourceFamily::Attachment
            )
        })
    }

    /// `version@component:module:family$path` of the resource at `path`.
    pub(crate) fn resource_id(&self, path: &Path) -> Option<String> {
        let coordinate = &self.resource(path)?.coordinate;
        let version = coordinate
            .version
            .as_ref()
            .map(|version| format!("{version}@"))
            .unwrap_or_default();
        let relative = coordinate
            .relative_path
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Some(format!(
            "{version}{}:{}:{}${relative}",
            coordinate.component, coordinate.module, coordinate.family
        ))
    }

    /// The component version of the resource at `path`, or of the navigation file at `path`.
    pub(crate) fn component_version(&self, path: &Path) -> Option<(String, Option<String>)> {
        if let Some(resource) = self.resource(path) {
            return Some((
                resource.coordinate.component.clone(),
                resource.coordinate.version.clone(),
            ));
        }
        self.state
            .antora
            .components()
            .find(|component| {
                path == component.root.join("antora.yml")
                    || component
                        .nav
                        .iter()
                        .any(|nav| normalize_path(&component.root.join(nav)) == path)
            })
            .map(|component| (component.name.clone(), component.version.clone()))
    }

    /// Whether `path` is a navigation file a component version's `antora.yml` lists.
    pub(crate) fn is_listed_nav(&self, path: &Path) -> bool {
        self.state.antora.components().any(|component| {
            component
                .nav
                .iter()
                .any(|nav| normalize_path(&component.root.join(nav)) == path)
        })
    }

    pub(crate) fn is_descriptor(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| name == "antora.yml")
            && self
                .state
                .antora
                .components()
                .any(|component| component.root.join("antora.yml") == path)
    }

    /// The navigation files a component version's `antora.yml` lists.
    pub(crate) fn version_navs(&self, component: &str, version: Option<&str>) -> Vec<PathBuf> {
        self.state
            .antora
            .component(component, version)
            .map(|descriptor| {
                descriptor
                    .nav
                    .iter()
                    .map(|nav| normalize_path(&descriptor.root.join(nav)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The files that include the file at `path`, each with its include directive.
    pub(crate) fn includers(&self, path: &Path) -> Vec<Referrer> {
        let mut found: Vec<Referrer> = adoc_lang::references::includes_of(&self.state, path)
            .into_iter()
            .map(|location| Referrer {
                path: location.path.to_path_buf(),
                range: location.include.range,
                written: location.include.target.clone(),
            })
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// The xrefs, navigation entries included, that lead to the file at `path`.
    pub(crate) fn referrers(&self, path: &Path) -> Vec<Referrer> {
        let mut found: Vec<Referrer> = adoc_lang::references_to(&self.state, path)
            .into_iter()
            .map(|location| Referrer {
                written: self.written(&location.path, location.range),
                path: location.path,
                range: location.range,
            })
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// The references to anchor `id` of the page at `path`.
    pub(crate) fn anchor_referrers(&self, path: &Path, id: &str) -> Vec<Referrer> {
        let mut found: Vec<Referrer> =
            adoc_lang::references::references_to_anchor(&self.state, path, id)
                .into_iter()
                .map(|location| Referrer {
                    path: location.path.to_path_buf(),
                    range: location.reference.range,
                    written: location.reference.target.clone(),
                })
                .collect();
        found.sort();
        found.dedup();
        found
    }

    fn written(&self, path: &Path, range: SourceRange) -> String {
        self.state
            .index
            .file(path)
            .and_then(|file| {
                file.document
                    .references
                    .iter()
                    .find(|reference| reference.range == range)
                    .map(|reference| reference.target.clone())
            })
            .unwrap_or_default()
    }

    /// The ids the page at `path` can be reached by: its anchors and its sections' ids.
    pub(crate) fn anchors(&self, path: &Path) -> BTreeSet<String> {
        let Some(file) = self.state.index.file(path) else {
            return BTreeSet::new();
        };
        file.document
            .anchors
            .iter()
            .map(|anchor| anchor.id.clone())
            .chain(adoc_lang::xref::section_ids(
                &self.state.antora,
                path,
                &file.document,
            ))
            .collect()
    }

    /// Whether the file at `from` still writes a reference with the target `written`, and where.
    pub(crate) fn still_writes(&self, from: &Path, written: &str) -> Option<SourceRange> {
        self.state.index.file(from).and_then(|file| {
            file.document
                .references
                .iter()
                .find(|reference| reference.target == written)
                .map(|reference| reference.range)
        })
    }

    /// Where the xref target `written`, from the file at `from`, leads in this revision.
    pub(crate) fn resolves(&self, from: &Path, written: &str) -> Option<PathBuf> {
        let file_part = written.split_once('#').map_or(written, |(file, _)| file);
        if file_part.is_empty() {
            return Some(from.to_path_buf());
        }
        adoc_lang::definition::reference_target_path(
            &self.state.index,
            &self.state.antora,
            from,
            file_part,
        )
        .map(|path| normalize_path(&path))
        // Only to a file this side has: resolution falls back to the disk, which is not the
        // revision's, nor a workspace with the file taken out.
        .filter(|path| self.has(path))
    }

    /// The pages the navigation file at `nav` lists, as its entries resolve.
    pub(crate) fn listed_by(&self, nav: &Path) -> BTreeSet<PathBuf> {
        adoc_lang::graph::links_from(&self.state, nav)
            .into_iter()
            .filter(|link| link.kind == LinkKind::Xref)
            .filter_map(|link| match link.target {
                adoc_lang::graph::LinkTarget::File(path) => Some(path),
                adoc_lang::graph::LinkTarget::Unresolved(_) => None,
            })
            .collect()
    }
}
