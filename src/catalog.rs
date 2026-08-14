use std::{collections::BTreeMap, path::Path};

use crate::{AntoraCoordinate, AntoraResource};

#[derive(Clone, Debug, Default)]
pub struct AntoraCatalog {
    resources: BTreeMap<AntoraCoordinate, AntoraResource>,
}

impl AntoraCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, resource: AntoraResource) -> Option<AntoraResource> {
        self.resources.insert(resource.coordinate.clone(), resource)
    }

    #[must_use]
    pub fn resolve(&self, coordinate: &AntoraCoordinate) -> Option<&AntoraResource> {
        self.resources.get(coordinate)
    }

    pub fn remove_source(&mut self, source_path: &Path) {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{AntoraCoordinate, AntoraResource, ResourceFamily};

    use super::AntoraCatalog;

    #[test]
    fn catalogs_resources_by_semantic_coordinate() {
        let coordinate = AntoraCoordinate {
            component: "demo".to_owned(),
            version: "latest".to_owned(),
            module: "ROOT".to_owned(),
            family: ResourceFamily::Page,
            relative_path: PathBuf::from("index.adoc"),
        };
        let resource = AntoraResource {
            coordinate: coordinate.clone(),
            source_path: PathBuf::from("modules/ROOT/pages/index.adoc"),
        };
        let mut catalog = AntoraCatalog::new();

        catalog.insert(resource);

        assert_eq!(catalog.resolve(&coordinate).unwrap().coordinate, coordinate);
    }
}
