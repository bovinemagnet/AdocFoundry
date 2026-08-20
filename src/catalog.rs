use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use crate::{
    AntoraContext, AntoraCoordinate, AntoraResolver, AntoraResource, AntoraResourceId,
    ComponentDescriptor, Module, ResolutionError, ResolutionResult, ResourceFamily,
};

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
        self.resources.insert(resource.coordinate.clone(), resource)
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

    #[must_use]
    pub fn resolve(&self, coordinate: &AntoraCoordinate) -> Option<&AntoraResource> {
        self.resources.get(coordinate)
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

    #[must_use]
    pub fn context_for_path(&self, source_path: &Path) -> Option<AntoraContext> {
        let source_path = normalize_path(source_path);
        if let Some(resource) = self
            .resources
            .values()
            .find(|resource| resource.source_path == source_path)
        {
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

    pub fn remove_source(&mut self, source_path: &Path) {
        let source_path = normalize_path(source_path);
        self.resources
            .retain(|_, resource| resource.source_path != source_path);
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

impl AntoraResolver for AntoraCatalog {
    fn resolve<'a>(
        &'a self,
        id: &AntoraResourceId,
        context: &AntoraContext,
    ) -> ResolutionResult<'a> {
        let component = id.component.as_deref().unwrap_or(&context.component);
        let version = id.version.clone().or_else(|| context.version.clone());
        if self.component(component, version.as_deref()).is_none() {
            return Err(ResolutionError::UnknownComponent {
                component: component.to_owned(),
                version,
            });
        }

        let module = id.module.as_deref().unwrap_or(&context.module);
        if self.module(component, version.as_deref(), module).is_none() {
            return Err(ResolutionError::UnknownModule {
                component: component.to_owned(),
                version,
                module: module.to_owned(),
            });
        }

        let family = id.family.unwrap_or(ResourceFamily::Page);
        let coordinate = AntoraCoordinate {
            component: component.to_owned(),
            version: version.clone(),
            module: module.to_owned(),
            family,
            relative_path: id.path.as_str().into(),
        };
        self.resources
            .get(&coordinate)
            .ok_or_else(|| ResolutionError::UnknownResource {
                component: component.to_owned(),
                version,
                module: module.to_owned(),
                family,
                path: coordinate.relative_path,
            })
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
}
