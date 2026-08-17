use std::{
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
};

use crate::{
    read_component_descriptor, AntoraCatalog, AntoraCoordinate, AntoraResource,
    ComponentDescriptor, DescriptorError, Module, ResourceFamily,
};

const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".zed",
    ".idea",
    "node_modules",
    "target",
    "build",
    "dist",
];

#[derive(Clone, Debug, Default)]
pub struct DiscoveryResult {
    pub catalog: AntoraCatalog,
    pub issues: Vec<DescriptorError>,
}

pub fn discover_antora_workspace(roots: &[PathBuf]) -> io::Result<DiscoveryResult> {
    let mut component_roots = Vec::new();
    for root in roots {
        collect_component_roots(root, &mut component_roots)?;
    }
    component_roots.sort();
    component_roots.dedup();

    let mut result = DiscoveryResult::default();
    for root in component_roots {
        match read_component_descriptor(&root) {
            Ok(descriptor) => discover_component(&mut result.catalog, descriptor)?,
            Err(error) => result.issues.push(error),
        }
    }
    Ok(result)
}

fn collect_component_roots(path: &Path, roots: &mut Vec<PathBuf>) -> io::Result<()> {
    if path.is_file() {
        if path.file_name() == Some(OsStr::new("antora.yml")) {
            if let Some(root) = path.parent().filter(|root| root.join("modules").is_dir()) {
                roots.push(normalize_path(root));
            }
        }
        return Ok(());
    }

    if path.join("antora.yml").is_file() && path.join("modules").is_dir() {
        roots.push(normalize_path(path));
    }
    for entry in sorted_entries(path)? {
        if !entry.file_type()?.is_dir() || is_ignored(&entry.file_name()) {
            continue;
        }
        collect_component_roots(&entry.path(), roots)?;
    }
    Ok(())
}

fn discover_component(
    catalog: &mut AntoraCatalog,
    descriptor: ComponentDescriptor,
) -> io::Result<()> {
    let component = descriptor.name.clone();
    let version = descriptor.version.clone();
    let modules_root = descriptor.root.join("modules");
    catalog.insert_component(descriptor);

    for entry in sorted_entries(&modules_root)? {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let root = normalize_path(&entry.path());
        let nav_path = root.join("nav.adoc");
        catalog.insert_module(Module {
            component: component.clone(),
            version: version.clone(),
            name: name.clone(),
            root: root.clone(),
            nav: nav_path.is_file().then_some(nav_path),
        });

        for family in ResourceFamily::ALL {
            let family_root = root.join(family.directory());
            if !family_root.is_dir() {
                continue;
            }
            let mut files = Vec::new();
            collect_files(&family_root, &mut files)?;
            files.sort();
            for source_path in files {
                let relative_path = source_path
                    .strip_prefix(&family_root)
                    .expect("collected resource remains beneath its family root")
                    .to_path_buf();
                catalog.insert(AntoraResource {
                    coordinate: AntoraCoordinate {
                        component: component.clone(),
                        version: version.clone(),
                        module: name.clone(),
                        family,
                        relative_path,
                    },
                    source_path,
                });
            }
        }
    }
    Ok(())
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in sorted_entries(path)? {
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(normalize_path(&entry.path()));
        }
    }
    Ok(())
}

fn sorted_entries(path: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn is_ignored(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| IGNORED_DIRECTORIES.contains(&name))
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{
        discover_antora_workspace, parse_resource_id, AntoraContext, AntoraResolver, ResourceFamily,
    };

    #[test]
    fn discovers_components_modules_and_resources() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let discovery = discover_antora_workspace(std::slice::from_ref(&root)).unwrap();

        assert!(discovery.issues.is_empty());
        let component = discovery.catalog.component("demo", Some("latest")).unwrap();
        assert_eq!(component.title.as_deref(), Some("Demo Documentation"));
        assert_eq!(component.display_version.as_deref(), Some("Latest"));
        assert_eq!(component.start_page.as_deref(), Some("ROOT:index.adoc"));
        assert_eq!(component.asciidoc_attributes["sectanchors"], "");
        assert!(discovery
            .catalog
            .module("demo", Some("latest"), "ROOT")
            .is_some_and(|module| module.nav.is_some()));
        assert!(discovery
            .catalog
            .module("demo", Some("latest"), "security")
            .is_some());
        assert_eq!(discovery.catalog.len(), 7);

        let resource = AntoraResolver::resolve(
            &discovery.catalog,
            &parse_resource_id("security:authentication.adoc").unwrap(),
            &AntoraContext {
                component: "demo".to_owned(),
                version: Some("latest".to_owned()),
                module: "ROOT".to_owned(),
                family: ResourceFamily::Page,
            },
        )
        .unwrap();
        assert!(resource
            .source_path
            .ends_with("modules/security/pages/authentication.adoc"));

        for id in [
            "example$sample.json",
            "image$architecture.svg",
            "attachment$guide.txt",
        ] {
            assert!(AntoraResolver::resolve(
                &discovery.catalog,
                &parse_resource_id(id).unwrap(),
                &AntoraContext {
                    component: "demo".to_owned(),
                    version: Some("latest".to_owned()),
                    module: "ROOT".to_owned(),
                    family: ResourceFamily::Page,
                },
            )
            .is_ok());
        }
    }
}
