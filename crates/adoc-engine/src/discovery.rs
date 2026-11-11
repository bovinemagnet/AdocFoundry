//! Antora discovery for front ends that find their way around a workspace without reading it
//! all (the MCP server's `list_pages`, `get_outline` and `find_references`): resources listed
//! in coordinate order from where the last listing stopped, a file's outline, and the exact
//! references that reach a page or an anchor. Every answer comes from the catalog, the index
//! and its reverse maps, and the graph's links; none reads a file.

use std::{
    collections::{BTreeMap, HashSet},
    ops::Bound,
    path::{Path, PathBuf},
};

use adoc_antora::{AntoraCoordinate, AntoraResource, ResourceFamily};
use adoc_core::{probe, SourceRange};
use adoc_graph::{EdgeKind, GraphKey};
use adoc_index::normalize_path;
use adoc_lang::DocumentSymbol;

use crate::Engine;

/// Which resources a listing takes. Every field left `None` takes everything.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceQuery {
    /// Only files beneath this folder.
    pub root: Option<PathBuf>,
    pub component: Option<String>,
    /// `Some(None)` is the versionless (`~`) version only, never a numbered one.
    pub version: Option<Option<String>>,
    pub module: Option<String>,
    pub family: Option<ResourceFamily>,
}

impl ResourceQuery {
    fn takes(&self, resource: &AntoraResource) -> bool {
        let coordinate = &resource.coordinate;
        self.component
            .as_ref()
            .is_none_or(|component| *component == coordinate.component)
            && self
                .version
                .as_ref()
                .is_none_or(|version| *version == coordinate.version)
            && self
                .module
                .as_ref()
                .is_none_or(|module| *module == coordinate.module)
            && self.family.is_none_or(|family| family == coordinate.family)
            && self
                .root
                .as_ref()
                .is_none_or(|root| resource.source_path.starts_with(root))
    }

    /// The first coordinate a listing can match. Coordinates order by component, version,
    /// module, family and path, so each of those the query fixes, in that order, narrows where
    /// the listing starts and ends: one component's, one version's, one module's or one
    /// family's resources lie together.
    fn start(&self) -> Option<AntoraCoordinate> {
        let component = self.component.clone()?;
        let (version, module, family) = match (&self.version, &self.module) {
            (Some(version), Some(module)) => (
                version.clone(),
                module.clone(),
                self.family.unwrap_or(ResourceFamily::Page),
            ),
            (Some(version), None) => (version.clone(), String::new(), ResourceFamily::Page),
            (None, _) => (None, String::new(), ResourceFamily::Page),
        };
        Some(AntoraCoordinate {
            component,
            version,
            module,
            family,
            relative_path: PathBuf::new(),
        })
    }

    /// Whether `coordinate`, at or past [`ResourceQuery::start`], is still inside the part of
    /// the catalog the query can match; the first outside ends the listing.
    fn within(&self, coordinate: &AntoraCoordinate) -> bool {
        let Some(component) = &self.component else {
            return true;
        };
        if coordinate.component != *component {
            return false;
        }
        let Some(version) = &self.version else {
            return true;
        };
        if coordinate.version != *version {
            return false;
        }
        let Some(module) = &self.module else {
            return true;
        };
        coordinate.module == *module && self.family.is_none_or(|family| coordinate.family == family)
    }
}

/// A resource as a listing gives it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceEntry {
    pub coordinate: AntoraCoordinate,
    /// Fully qualified: `version@component:module:family$path`.
    pub resource_id: String,
    pub path: PathBuf,
    /// The document title of an indexed page or partial.
    pub title: Option<String>,
    /// For a page: whether a navigation file of its own component version lists it.
    pub in_navigation: Option<bool>,
}

/// What reaches a page or an anchor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReferenceKind {
    /// An xref in a page or partial.
    Xref,
    /// An include directive.
    Include,
    /// An xref in a navigation file: an entry of the navigation.
    Nav,
    /// A reference to one of the page's anchors (`<<id>>`, `xref:page.adoc#id[]`).
    Anchor,
}

impl ReferenceKind {
    pub const ALL: [Self; 4] = [Self::Xref, Self::Include, Self::Nav, Self::Anchor];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Xref => "xref",
            Self::Include => "include",
            Self::Nav => "nav",
            Self::Anchor => "anchor",
        }
    }
}

/// A reference that resolves to a page or an anchor, where it is written.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IncomingReference {
    /// The file the reference is written in.
    pub path: PathBuf,
    /// The whole macro or entry, in that file's bytes.
    pub range: SourceRange,
    pub kind: ReferenceKind,
    /// The target as written: `other.adoc#id`, `partial$note.adoc`.
    pub target: String,
}

impl Engine {
    /// The resources `query` takes after `after` (from the first when `None`), in coordinate
    /// order, at most `limit` of them, and whether more follow. A listing starting after the
    /// last one's final coordinate continues it; what it reads is what it returns, the resources
    /// the query skips on the way, and one more.
    #[must_use]
    pub fn resources_after(
        &self,
        query: &ResourceQuery,
        after: Option<&AntoraCoordinate>,
        limit: usize,
    ) -> (Vec<ResourceEntry>, bool) {
        let catalog = &self.workspace.antora;
        let start = query.start();
        let bound = match (after, &start) {
            (Some(after), _) => Bound::Excluded(after),
            (None, Some(start)) => Bound::Included(start),
            (None, None) => Bound::Unbounded,
        };
        let mut navs: BTreeMap<(String, Option<String>), HashSet<PathBuf>> = BTreeMap::new();
        let mut entries = Vec::new();
        let mut more = false;
        for resource in catalog.resources_from(bound) {
            probe::hit(probe::Probe::ListResource);
            if !query.within(&resource.coordinate) {
                break;
            }
            if !query.takes(resource) {
                continue;
            }
            if entries.len() == limit {
                more = true;
                break;
            }
            entries.push(self.resource_entry(resource, &mut navs));
        }
        (entries, more)
    }

    fn resource_entry(
        &self,
        resource: &AntoraResource,
        navs: &mut BTreeMap<(String, Option<String>), HashSet<PathBuf>>,
    ) -> ResourceEntry {
        let coordinate = &resource.coordinate;
        let path = normalize_path(&resource.source_path);
        let title = self
            .workspace
            .index
            .file(&path)
            .and_then(|file| file.document.title.as_ref())
            .map(|title| title.text.clone());
        let in_navigation = (coordinate.family == ResourceFamily::Page).then(|| {
            let navs = navs
                .entry((coordinate.component.clone(), coordinate.version.clone()))
                .or_insert_with(|| {
                    self.version_nav_files(&coordinate.component, coordinate.version.as_deref())
                });
            self.graph
                .store()
                .links_to(&GraphKey::File(path.clone().into()))
                .any(|link| link.kind == EdgeKind::Nav && navs.contains(&*link.from))
        });
        ResourceEntry {
            coordinate: coordinate.clone(),
            resource_id: self.resource_id(&path).unwrap_or_default(),
            path,
            title,
            in_navigation,
        }
    }

    /// Whether a navigation file of its own component version lists the page at `path`;
    /// `None` when it is not an Antora page.
    #[must_use]
    pub fn in_navigation(&self, path: &Path) -> Option<bool> {
        let resource = self.workspace.antora.resource_for_path(path)?;
        self.resource_entry(resource, &mut BTreeMap::new())
            .in_navigation
    }

    /// The navigation files a component version's `antora.yml` lists.
    fn version_nav_files(&self, component: &str, version: Option<&str>) -> HashSet<PathBuf> {
        self.workspace
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

    /// [`Engine::outline`] of the indexed file at `path`, open or not: a page's title with its
    /// sections nested by level, or a navigation file's entries; `None` when it is not indexed.
    /// Built from the index's parse, which for an open page is its buffer's.
    #[must_use]
    pub fn outline_of(&self, path: &Path) -> Option<Vec<DocumentSymbol>> {
        let path = normalize_path(path);
        let file = self.workspace.index.file(&path)?;
        if self.workspace.antora.is_nav_file(&path) {
            // A navigation file's entries are read from its text, which the index keeps only
            // for an open file; one read from disk is read back.
            let text = file.text()?;
            let document = adoc_core::Document::new(file.document.uri.clone(), &*text);
            return Some(adoc_lang::symbols::nav_symbols(&document));
        }
        Some(adoc_lang::symbols::document_symbols(&file.document))
    }

    /// The ids each section of the indexed page at `path` is reached by, in section order: its
    /// anchor, or the id generated from its title as the component's `idprefix` and
    /// `idseparator` make it; empty where only rendering the title could tell it.
    #[must_use]
    pub fn section_ids(&self, path: &Path) -> Vec<String> {
        let path = normalize_path(path);
        self.workspace
            .index
            .file(&path)
            .map(|file| adoc_lang::xref::section_ids(&self.workspace.antora, &path, &file.document))
            .unwrap_or_default()
    }

    /// The references that resolve to the file at `target`, or with `anchor` to that anchor of
    /// it, sorted by file and place: xrefs and navigation entries naming it and the includes
    /// that include it, or the references to the anchor. Only the index's candidates (the
    /// references naming the file's name, or with the anchor as their fragment) are resolved;
    /// a dynamic target the index cannot resolve is not found, never guessed.
    #[must_use]
    pub fn incoming_references(
        &self,
        target: &Path,
        anchor: Option<&str>,
    ) -> Vec<IncomingReference> {
        let ws = &self.workspace;
        let target = normalize_path(target);
        let mut found = Vec::new();
        let written = |path: &Path, range: SourceRange| {
            ws.index.file(path).and_then(|file| {
                file.document
                    .references
                    .iter()
                    .find(|reference| reference.range == range)
                    .map(|reference| reference.target.clone())
            })
        };
        match anchor {
            Some(anchor) => {
                for location in adoc_lang::references::references_to_anchor(ws, &target, anchor) {
                    found.push(IncomingReference {
                        path: location.path.to_path_buf(),
                        range: location.reference.range,
                        kind: ReferenceKind::Anchor,
                        target: location.reference.target.clone(),
                    });
                }
            }
            None => {
                for location in adoc_lang::references_to(ws, &target) {
                    let kind = if ws.antora.is_nav_file(&location.path) {
                        ReferenceKind::Nav
                    } else {
                        ReferenceKind::Xref
                    };
                    found.push(IncomingReference {
                        target: written(&location.path, location.range).unwrap_or_default(),
                        path: location.path,
                        range: location.range,
                        kind,
                    });
                }
                for location in adoc_lang::references::includes_of(ws, &target) {
                    found.push(IncomingReference {
                        path: location.path.to_path_buf(),
                        range: location.include.range,
                        kind: ReferenceKind::Include,
                        target: location.include.target.clone(),
                    });
                }
            }
        }
        found.sort();
        found.dedup();
        found
    }
}
