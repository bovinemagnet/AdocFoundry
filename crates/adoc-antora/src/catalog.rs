use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use crate::{
    discovery::discover_component, read_component_descriptor, AntoraContext, AntoraCoordinate,
    AntoraResolver, AntoraResource, AntoraResourceId, ComponentDescriptor, DescriptorError,
    DescriptorErrorKind, Module, ResolutionError, ResolutionResult, ResourceFamily,
};

/// A successful resolution, and whether it went through a `page-aliases` entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resolved<'a> {
    pub resource: &'a AntoraResource,
    pub via_alias: bool,
}

/// The resource ids a descriptor reload added and removed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReloadReport {
    pub added: Vec<AntoraResourceId>,
    pub removed: Vec<AntoraResourceId>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ComponentKey {
    name: String,
    version: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ModuleKey {
    component: String,
    version: Option<String>,
    name: String,
}

#[derive(Clone, Debug, Default)]
pub struct AntoraCatalog {
    components: BTreeMap<ComponentKey, ComponentDescriptor>,
    modules: BTreeMap<ModuleKey, Module>,
    resources: BTreeMap<AntoraCoordinate, AntoraResource>,
    /// Each file → the coordinates of the resources read from it, so a file is looked up
    /// without visiting every resource.
    by_path: BTreeMap<PathBuf, BTreeSet<AntoraCoordinate>>,
    /// Alias coordinate → the coordinate of the page that declares it.
    aliases: BTreeMap<AntoraCoordinate, AntoraCoordinate>,
    playbook: Option<crate::Playbook>,
    /// Which branch publishes each version, when the workspace's repository is known.
    branch_versions: Option<crate::BranchVersions>,
}

impl AntoraCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_component(
        &mut self,
        component: ComponentDescriptor,
    ) -> Option<ComponentDescriptor> {
        self.components.insert(
            ComponentKey {
                name: component.name.clone(),
                version: component.version.clone(),
            },
            component,
        )
    }

    pub fn insert_module(&mut self, module: Module) -> Option<Module> {
        self.modules.insert(
            ModuleKey {
                component: module.component.clone(),
                version: module.version.clone(),
                name: module.name.clone(),
            },
            module,
        )
    }

    pub fn insert(&mut self, resource: AntoraResource) -> Option<AntoraResource> {
        for alias in &resource.aliases {
            if let Some(alias) = qualified_coordinate(alias) {
                self.aliases.insert(alias, resource.coordinate.clone());
            }
        }
        self.by_path
            .entry(resource.source_path.clone())
            .or_default()
            .insert(resource.coordinate.clone());
        let replaced = self.resources.insert(resource.coordinate.clone(), resource);
        if let Some(old) = &replaced {
            if self.resources[&old.coordinate].source_path != old.source_path {
                self.forget_path(&old.source_path, &old.coordinate);
            }
        }
        replaced
    }

    fn forget_path(&mut self, path: &Path, coordinate: &AntoraCoordinate) {
        if let Some(coordinates) = self.by_path.get_mut(path) {
            coordinates.remove(coordinate);
            if coordinates.is_empty() {
                self.by_path.remove(path);
            }
        }
    }

    #[must_use]
    pub fn component(&self, name: &str, version: Option<&str>) -> Option<&ComponentDescriptor> {
        self.components.get(&ComponentKey {
            name: name.to_owned(),
            version: version.map(str::to_owned),
        })
    }

    #[must_use]
    pub fn module(&self, component: &str, version: Option<&str>, name: &str) -> Option<&Module> {
        self.modules.get(&ModuleKey {
            component: component.to_owned(),
            version: version.map(str::to_owned),
            name: name.to_owned(),
        })
    }

    /// The playbook found at a workspace root, whose attributes apply to every page.
    #[must_use]
    pub fn playbook(&self) -> Option<&crate::Playbook> {
        self.playbook.as_ref()
    }

    pub fn set_playbook(&mut self, playbook: Option<crate::Playbook>) {
        self.playbook = playbook;
    }

    #[must_use]
    pub fn branch_versions(&self) -> Option<&crate::BranchVersions> {
        self.branch_versions.as_ref()
    }

    pub fn set_branch_versions(&mut self, branch_versions: Option<crate::BranchVersions>) {
        self.branch_versions = branch_versions;
    }

    /// The resource at `coordinate`, falling back to a page that lists it in `page-aliases`.
    #[must_use]
    pub fn resolve(&self, coordinate: &AntoraCoordinate) -> Option<&AntoraResource> {
        self.resources.get(coordinate).or_else(|| {
            self.aliases
                .get(coordinate)
                .and_then(|target| self.resources.get(target))
        })
    }

    /// The latest version of component `name`: released versions above prereleases, then the
    /// versionless (`~`) version above numbered ones, numbered versions compared as dotted
    /// integers and anything else lexically.
    #[must_use]
    pub fn latest_version(&self, name: &str) -> Option<&ComponentDescriptor> {
        self.components
            .values()
            .filter(|component| component.name == name)
            .max_by(|left, right| compare_component_versions(left, right))
    }

    /// Every version of component `name`, latest first.
    #[must_use]
    pub fn versions_of(&self, name: &str) -> Vec<&ComponentDescriptor> {
        let mut versions: Vec<_> = self
            .components
            .values()
            .filter(|component| component.name == name)
            .collect();
        versions.sort_by(|left, right| compare_component_versions(right, left));
        versions
    }

    /// Resolves `id` relative to `context` following Antora 3.2: an omitted component and
    /// version mean the current component version; a component without a version means its
    /// latest version; a component without a module means its `ROOT` module.
    pub fn resolve_detailed(
        &self,
        id: &AntoraResourceId,
        context: &AntoraContext,
    ) -> Result<Resolved<'_>, ResolutionError> {
        let (component, version) = match (&id.component, &id.version) {
            (None, None) => (context.component.clone(), context.version.clone()),
            (None, Some(version)) => (context.component.clone(), Some(version.clone())),
            (Some(component), Some(version)) => (component.clone(), Some(version.clone())),
            (Some(component), None) => match self.latest_version(component) {
                Some(latest) => (component.clone(), latest.version.clone()),
                None => {
                    return Err(ResolutionError::UnknownComponent {
                        component: component.clone(),
                        version: None,
                    })
                }
            },
        };
        if self.component(&component, version.as_deref()).is_none() {
            if self.latest_version(&component).is_some() {
                return Err(ResolutionError::UnknownVersion {
                    component,
                    version: version.unwrap_or_else(|| "~".to_owned()),
                });
            }
            return Err(ResolutionError::UnknownComponent { component, version });
        }

        let module = match (&id.module, &id.component) {
            (Some(module), _) => module.clone(),
            (None, Some(_)) => "ROOT".to_owned(),
            (None, None) => context.module.clone(),
        };
        if self
            .module(&component, version.as_deref(), &module)
            .is_none()
        {
            return Err(ResolutionError::UnknownModule {
                component,
                version,
                module,
            });
        }

        let family = id.family.unwrap_or(ResourceFamily::Page);
        let coordinate = AntoraCoordinate {
            component: component.clone(),
            version: version.clone(),
            module: module.clone(),
            family,
            relative_path: id.path.as_str().into(),
        };
        if let Some(resource) = self.resources.get(&coordinate) {
            return Ok(Resolved {
                resource,
                via_alias: false,
            });
        }
        if let Some(resource) = self
            .aliases
            .get(&coordinate)
            .and_then(|target| self.resources.get(target))
        {
            return Ok(Resolved {
                resource,
                via_alias: true,
            });
        }
        Err(ResolutionError::UnknownResource {
            component,
            version,
            module,
            family,
            path: coordinate.relative_path,
        })
    }

    /// Re-reads the `antora.yml` at `descriptor_path`, replacing that component version's
    /// modules and resources. On failure the component is removed, as it would be at start-up.
    pub fn reload_component(
        &mut self,
        descriptor_path: &Path,
    ) -> Result<ReloadReport, DescriptorError> {
        let root = normalize_path(descriptor_path.parent().unwrap_or_else(|| Path::new("")));
        let before = self.remove_component_at(&root);
        let descriptor = read_component_descriptor(&root)?;
        discover_component(self, descriptor).map_err(|error| DescriptorError {
            kind: DescriptorErrorKind::Read,
            path: root.join("antora.yml"),
            message: error.to_string(),
        })?;
        let after: BTreeSet<AntoraCoordinate> = self
            .resources
            .values()
            .filter(|resource| resource.source_path.starts_with(&root))
            .map(|resource| resource.coordinate.clone())
            .collect();
        Ok(ReloadReport {
            added: after.difference(&before).map(resource_id).collect(),
            removed: before.difference(&after).map(resource_id).collect(),
        })
    }

    /// Removes every component version rooted at `root` with its modules, resources and
    /// aliases, returning the coordinates of the removed resources.
    fn remove_component_at(&mut self, root: &Path) -> BTreeSet<AntoraCoordinate> {
        let removed_keys: Vec<ComponentKey> = self
            .components
            .iter()
            .filter(|(_, component)| normalize_path(&component.root) == root)
            .map(|(key, _)| key.clone())
            .collect();
        let mut removed = BTreeSet::new();
        for key in removed_keys {
            self.components.remove(&key);
            self.modules.retain(|module, _| {
                !(module.component == key.name && module.version == key.version)
            });
            let mut paths = Vec::new();
            self.resources.retain(|coordinate, resource| {
                let matches = coordinate.component == key.name && coordinate.version == key.version;
                if matches {
                    removed.insert(coordinate.clone());
                    paths.push((resource.source_path.clone(), coordinate.clone()));
                }
                !matches
            });
            for (path, coordinate) in paths {
                self.forget_path(&path, &coordinate);
            }
        }
        self.aliases
            .retain(|_, target| self.resources.contains_key(target));
        removed
    }

    pub fn components(&self) -> impl Iterator<Item = &ComponentDescriptor> {
        self.components.values()
    }

    pub fn modules(&self) -> impl Iterator<Item = &Module> {
        self.modules.values()
    }

    pub fn resources(&self) -> impl Iterator<Item = &AntoraResource> {
        self.resources.values()
    }

    /// Every resource in one module's family, in path order.
    ///
    /// This is a range over the ordered map rather than a filter over every resource:
    /// `AntoraCoordinate` orders component, version, module, family, then path, so the
    /// matching entries are contiguous and the cost is proportional to the answer.
    pub fn resources_in(
        &self,
        component: &str,
        version: Option<&str>,
        module: &str,
        family: ResourceFamily,
    ) -> impl Iterator<Item = &AntoraResource> {
        let start = AntoraCoordinate {
            component: component.to_owned(),
            version: version.map(str::to_owned),
            module: module.to_owned(),
            family,
            relative_path: PathBuf::new(),
        };
        let component = component.to_owned();
        let version = version.map(str::to_owned);
        let module = module.to_owned();
        self.resources
            .range(start..)
            .take_while(move |(coordinate, _)| {
                coordinate.component == component
                    && coordinate.version == version
                    && coordinate.module == module
                    && coordinate.family == family
            })
            .map(|(_, resource)| resource)
    }

    /// Every module of one component version, in name order.
    pub fn modules_of(
        &self,
        component: &str,
        version: Option<&str>,
    ) -> impl Iterator<Item = &Module> {
        let start = ModuleKey {
            component: component.to_owned(),
            version: version.map(str::to_owned),
            name: String::new(),
        };
        let component = component.to_owned();
        let version = version.map(str::to_owned);
        self.modules
            .range(start..)
            .take_while(move |(key, _)| key.component == component && key.version == version)
            .map(|(_, module)| module)
    }

    /// The resource whose file is `source_path`: the first in coordinate order, should two
    /// read the same file.
    #[must_use]
    pub fn resource_for_path(&self, source_path: &Path) -> Option<&AntoraResource> {
        let coordinate = self.by_path.get(&normalize_path(source_path))?.first()?;
        self.resources.get(coordinate)
    }

    #[must_use]
    pub fn context_for_path(&self, source_path: &Path) -> Option<AntoraContext> {
        let source_path = normalize_path(source_path);
        if let Some(resource) = self.resource_for_path(&source_path) {
            return Some(AntoraContext {
                component: resource.coordinate.component.clone(),
                version: resource.coordinate.version.clone(),
                module: resource.coordinate.module.clone(),
                family: resource.coordinate.family,
            });
        }

        // Files that sit in a module but outside any family directory - `nav.adoc`, a shared
        // `_attributes.adoc` - are not catalogued as resources, yet their references still
        // resolve against the module they belong to.
        let module = self
            .modules
            .values()
            .filter(|module| source_path.starts_with(&module.root))
            .max_by_key(|module| module.root.as_os_str().len())?;
        Some(AntoraContext {
            component: module.component.clone(),
            version: module.version.clone(),
            module: module.name.clone(),
            family: ResourceFamily::Page,
        })
    }

    /// Whether `path` is a navigation file: a module's `nav.adoc`, or a file the component's
    /// `antora.yml` lists under `nav`.
    #[must_use]
    pub fn is_nav_file(&self, path: &Path) -> bool {
        let path = normalize_path(path);
        self.modules
            .values()
            .any(|module| module.nav.as_deref() == Some(path.as_path()))
            || self.components.values().any(|component| {
                component
                    .nav
                    .iter()
                    .any(|entry| normalize_path(&component.root.join(entry)) == path)
            })
    }

    pub fn remove_source(&mut self, source_path: &Path) {
        let source_path = normalize_path(source_path);
        for coordinate in self.by_path.remove(&source_path).unwrap_or_default() {
            self.resources.remove(&coordinate);
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

/// Orders component versions so that the greater one is the later: released above prerelease,
/// then by [`compare_versions`].
#[must_use]
pub fn compare_component_versions(
    left: &ComponentDescriptor,
    right: &ComponentDescriptor,
) -> Ordering {
    right
        .prerelease
        .cmp(&left.prerelease)
        .then_with(|| compare_versions(left.version.as_deref(), right.version.as_deref()))
}

/// Orders version strings: versionless highest, then numbered versions as dotted integers
/// (an optional leading `v` ignored), then other names lexically.
#[must_use]
pub fn compare_versions(left: Option<&str>, right: Option<&str>) -> Ordering {
    fn numeric(version: &str) -> Option<Vec<u64>> {
        version
            .strip_prefix('v')
            .unwrap_or(version)
            .split('.')
            .map(|part| part.parse().ok())
            .collect()
    }
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => match (numeric(left), numeric(right)) {
            (Some(mut left), Some(mut right)) => {
                let len = left.len().max(right.len());
                left.resize(len, 0);
                right.resize(len, 0);
                left.cmp(&right)
            }
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => left.cmp(right),
        },
    }
}

fn qualified_coordinate(id: &AntoraResourceId) -> Option<AntoraCoordinate> {
    Some(AntoraCoordinate {
        component: id.component.clone()?,
        version: id.version.clone(),
        module: id.module.clone()?,
        family: id.family.unwrap_or(ResourceFamily::Page),
        relative_path: id.path.as_str().into(),
    })
}

/// The fully qualified resource id of `coordinate`. Paths always use `/`.
fn resource_id(coordinate: &AntoraCoordinate) -> AntoraResourceId {
    AntoraResourceId {
        version: coordinate.version.clone(),
        component: Some(coordinate.component.clone()),
        module: Some(coordinate.module.clone()),
        family: Some(coordinate.family),
        path: coordinate
            .relative_path
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    }
}

impl AntoraResolver for AntoraCatalog {
    fn resolve<'a>(
        &'a self,
        id: &AntoraResourceId,
        context: &AntoraContext,
    ) -> ResolutionResult<'a> {
        self.resolve_detailed(id, context)
            .map(|resolved| resolved.resource)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use crate::{
        AntoraContext, AntoraCoordinate, AntoraResolver, AntoraResource, ComponentDescriptor,
        Module, ResourceFamily,
    };

    use super::AntoraCatalog;

    /// Antora runs on Node, whose `path.join` ignores a leading separator on later
    /// segments, so `partial$/note.adoc` resolves the same as `partial$note.adoc`.
    /// Rust's `PathBuf::join` would treat it as absolute, so it must be trimmed.
    #[test]
    fn resolves_a_resource_id_written_with_a_leading_separator() {
        let mut catalog = AntoraCatalog::new();
        catalog.insert_component(ComponentDescriptor {
            root: PathBuf::from("."),
            name: "demo".to_owned(),
            title: None,
            version: Some("latest".to_owned()),
            display_version: None,
            start_page: None,
            nav: Vec::new(),
            asciidoc_attributes: BTreeMap::new(),
            prerelease: false,
        });
        catalog.insert_module(Module {
            component: "demo".to_owned(),
            version: Some("latest".to_owned()),
            name: "ROOT".to_owned(),
            root: PathBuf::from("modules/ROOT"),
            nav: None,
        });
        catalog.insert(AntoraResource {
            coordinate: AntoraCoordinate {
                component: "demo".to_owned(),
                version: Some("latest".to_owned()),
                module: "ROOT".to_owned(),
                family: ResourceFamily::Partial,
                relative_path: PathBuf::from("api/note.adoc"),
            },
            source_path: PathBuf::from("modules/ROOT/partials/api/note.adoc"),
            aliases: Vec::new(),
        });
        let context = AntoraContext {
            component: "demo".to_owned(),
            version: Some("latest".to_owned()),
            module: "ROOT".to_owned(),
            family: ResourceFamily::Page,
        };

        let resolved = AntoraResolver::resolve(
            &catalog,
            &crate::parse_resource_id("partial$/api/note.adoc").unwrap(),
            &context,
        )
        .expect("a leading separator must not prevent resolution");

        assert_eq!(
            resolved.source_path,
            PathBuf::from("modules/ROOT/partials/api/note.adoc")
        );
    }

    #[test]
    fn catalogs_and_resolves_resources_by_semantic_coordinate() {
        let coordinate = AntoraCoordinate {
            component: "demo".to_owned(),
            version: Some("latest".to_owned()),
            module: "ROOT".to_owned(),
            family: ResourceFamily::Page,
            relative_path: PathBuf::from("index.adoc"),
        };
        let resource = AntoraResource {
            coordinate: coordinate.clone(),
            source_path: PathBuf::from("modules/ROOT/pages/index.adoc"),
            aliases: Vec::new(),
        };
        let mut catalog = AntoraCatalog::new();
        catalog.insert_component(ComponentDescriptor {
            root: PathBuf::from("."),
            name: "demo".to_owned(),
            title: None,
            version: Some("latest".to_owned()),
            display_version: None,
            start_page: None,
            nav: Vec::new(),
            asciidoc_attributes: BTreeMap::new(),
            prerelease: false,
        });
        catalog.insert_module(Module {
            component: "demo".to_owned(),
            version: Some("latest".to_owned()),
            name: "ROOT".to_owned(),
            root: PathBuf::from("modules/ROOT"),
            nav: None,
        });
        catalog.insert(resource);

        assert_eq!(catalog.resolve(&coordinate).unwrap().coordinate, coordinate);
        let id = crate::parse_resource_id("index.adoc").unwrap();
        let resolved = AntoraResolver::resolve(
            &catalog,
            &id,
            &AntoraContext {
                component: "demo".to_owned(),
                version: Some("latest".to_owned()),
                module: "ROOT".to_owned(),
                family: ResourceFamily::Page,
            },
        )
        .unwrap();
        assert_eq!(
            resolved.source_path,
            PathBuf::from("modules/ROOT/pages/index.adoc")
        );
    }

    fn two_module_catalog() -> AntoraCatalog {
        let mut catalog = AntoraCatalog::new();
        catalog.insert_component(ComponentDescriptor {
            root: PathBuf::from("."),
            name: "demo".to_owned(),
            title: None,
            version: Some("latest".to_owned()),
            display_version: None,
            start_page: None,
            nav: Vec::new(),
            asciidoc_attributes: BTreeMap::new(),
            prerelease: false,
        });
        for module in ["ROOT", "security"] {
            catalog.insert_module(Module {
                component: "demo".to_owned(),
                version: Some("latest".to_owned()),
                name: module.to_owned(),
                root: PathBuf::from(format!("modules/{module}")),
                nav: None,
            });
            for (family, file) in [
                (ResourceFamily::Page, "index.adoc"),
                (ResourceFamily::Partial, "note.adoc"),
            ] {
                catalog.insert(AntoraResource {
                    coordinate: AntoraCoordinate {
                        component: "demo".to_owned(),
                        version: Some("latest".to_owned()),
                        module: module.to_owned(),
                        family,
                        relative_path: PathBuf::from(file),
                    },
                    source_path: PathBuf::from(format!(
                        "modules/{module}/{}/{file}",
                        family.directory()
                    )),
                    aliases: Vec::new(),
                });
            }
        }
        catalog
    }

    #[test]
    fn enumerates_only_the_requested_module_and_family() {
        let catalog = two_module_catalog();

        let partials: Vec<_> = catalog
            .resources_in("demo", Some("latest"), "ROOT", ResourceFamily::Partial)
            .map(|resource| resource.source_path.clone())
            .collect();

        assert_eq!(
            partials,
            vec![PathBuf::from("modules/ROOT/partials/note.adoc")],
            "a neighbouring module or family must not leak in"
        );
    }

    #[test]
    fn enumerates_nothing_for_an_unknown_module() {
        let catalog = two_module_catalog();

        assert_eq!(
            catalog
                .resources_in("demo", Some("latest"), "absent", ResourceFamily::Page)
                .count(),
            0
        );
    }

    #[test]
    fn enumerates_the_modules_of_a_component() {
        let catalog = two_module_catalog();

        let names: Vec<_> = catalog
            .modules_of("demo", Some("latest"))
            .map(|module| module.name.clone())
            .collect();

        assert_eq!(names, vec!["ROOT".to_owned(), "security".to_owned()]);
    }

    #[test]
    fn recognises_module_and_descriptor_navigation_files() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut catalog = crate::discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;

        assert!(catalog.is_nav_file(&root.join("modules/ROOT/nav.adoc")));
        assert!(!catalog.is_nav_file(&root.join("modules/ROOT/pages/index.adoc")));

        // A file listed under `nav` counts even where it is not a module's `nav.adoc`.
        let mut component = catalog.components().next().unwrap().clone();
        component.nav = vec!["modules/security/menu.adoc".to_owned()];
        catalog.insert_component(component);
        assert!(catalog.is_nav_file(&root.join("modules/security/menu.adoc")));
    }

    fn page(component: &str, path: &str, file: &str) -> AntoraResource {
        AntoraResource {
            coordinate: AntoraCoordinate {
                component: component.to_owned(),
                version: None,
                module: "ROOT".to_owned(),
                family: ResourceFamily::Page,
                relative_path: PathBuf::from(path),
            },
            source_path: PathBuf::from(file),
            aliases: Vec::new(),
        }
    }

    /// Looking a file up is a map lookup, kept in step with every insert and removal.
    #[test]
    fn resources_are_found_by_path_after_inserts_and_removals() {
        let mut catalog = AntoraCatalog::new();
        for index in 0..1_000 {
            catalog.insert(page(
                "demo",
                &format!("p{index}.adoc"),
                &format!("/docs/modules/ROOT/pages/p{index}.adoc"),
            ));
        }
        let found = catalog
            .resource_for_path(std::path::Path::new("/docs/modules/ROOT/pages/./p7.adoc"))
            .unwrap();
        assert_eq!(found.coordinate.relative_path, PathBuf::from("p7.adoc"));
        assert_eq!(
            catalog
                .context_for_path(std::path::Path::new("/docs/modules/ROOT/pages/p7.adoc"))
                .map(|context| context.component),
            Some("demo".to_owned())
        );

        // The same file read under another coordinate: the first in order answers.
        catalog.insert(page("alpha", "p7.adoc", "/docs/modules/ROOT/pages/p7.adoc"));
        assert_eq!(
            catalog
                .resource_for_path(std::path::Path::new("/docs/modules/ROOT/pages/p7.adoc"))
                .unwrap()
                .coordinate
                .component,
            "alpha"
        );
        // A coordinate given another file moves with it.
        catalog.insert(page("demo", "p8.adoc", "/elsewhere/p8.adoc"));
        assert!(catalog
            .resource_for_path(std::path::Path::new("/docs/modules/ROOT/pages/p8.adoc"))
            .is_none());
        assert!(catalog
            .resource_for_path(std::path::Path::new("/elsewhere/p8.adoc"))
            .is_some());

        catalog.remove_source(std::path::Path::new("/docs/modules/ROOT/pages/p7.adoc"));
        assert!(catalog
            .resource_for_path(std::path::Path::new("/docs/modules/ROOT/pages/p7.adoc"))
            .is_none());
        assert_eq!(catalog.len(), 999);
    }
}
