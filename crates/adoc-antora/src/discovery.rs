use std::{
    ffi::OsStr,
    fs,
    io::{self, BufRead as _},
    path::{Component, Path, PathBuf},
};

use crate::{
    parse_resource_id, read_component_descriptor, AntoraCatalog, AntoraCoordinate, AntoraResource,
    AntoraResourceId, ComponentDescriptor, DescriptorError, Module, ResourceFamily,
};

#[derive(Clone, Debug, Default)]
pub struct DiscoveryResult {
    pub catalog: AntoraCatalog,
    pub issues: Vec<DescriptorError>,
}

pub fn discover_antora_workspace(roots: &[PathBuf]) -> io::Result<DiscoveryResult> {
    discover_antora_workspace_ignoring(roots, adoc_core::DEFAULT_IGNORED_DIRECTORIES)
}

/// Discovers the components beneath `roots`, never entering a folder named in `ignore` or one
/// that cannot be read.
pub fn discover_antora_workspace_ignoring(
    roots: &[PathBuf],
    ignore: &[impl AsRef<str>],
) -> io::Result<DiscoveryResult> {
    let mut component_roots = Vec::new();
    for root in roots {
        collect_component_roots(root, ignore, &mut component_roots);
    }
    component_roots.sort();
    component_roots.dedup();

    let mut result = DiscoveryResult::default();
    // The first root with a playbook supplies the site-wide attributes.
    for root in roots {
        let path = root.join(crate::PLAYBOOK_FILE);
        if path.is_file() {
            match crate::read_playbook(&normalize_path(&path)) {
                Ok(playbook) => result.catalog.set_playbook(Some(playbook)),
                Err(error) => result.issues.push(error),
            }
            break;
        }
    }
    for root in component_roots {
        match read_component_descriptor(&root) {
            Ok(descriptor) => discover_component(&mut result.catalog, descriptor)?,
            Err(error) => result.issues.push(error),
        }
    }
    Ok(result)
}

/// The ids a page's `:page-aliases:` header attribute lists, qualified with the page's own
/// coordinates where they omit them. Ids that do not parse are skipped.
pub(crate) fn page_aliases(text: &str, page: &AntoraCoordinate) -> Vec<AntoraResourceId> {
    let Some(value) = header_attribute(text, "page-aliases") else {
        return Vec::new();
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|alias| !alias.is_empty())
        .filter_map(|alias| parse_resource_id(alias).ok())
        .map(|mut id| {
            if id.component.is_none() {
                id.component = Some(page.component.clone());
                if id.version.is_none() {
                    id.version.clone_from(&page.version);
                }
                if id.module.is_none() {
                    id.module = Some(page.module.clone());
                }
            } else if id.module.is_none() {
                id.module = Some("ROOT".to_owned());
            }
            id.family.get_or_insert(ResourceFamily::Page);
            id
        })
        .collect()
}

/// The value of attribute `name` declared in the document header: the lines from the top of
/// the file up to the first blank line after the header starts.
fn header_attribute<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!(":{name}:");
    let mut started = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if started {
                break;
            }
            continue;
        }
        started = true;
        if let Some(value) = line.strip_prefix(&prefix) {
            return Some(value.trim());
        }
    }
    None
}

/// The lines of the file at `path` that [`header_attribute`] reads: from the top up to the
/// first blank line after the header starts. The rest of the page is never read.
fn read_header(path: &Path) -> io::Result<String> {
    let mut reader = io::BufReader::new(fs::File::open(path)?);
    let mut header = String::new();
    let mut started = false;
    loop {
        let start = header.len();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let blank = header[start..].trim_end().is_empty();
        if blank && started {
            break;
        }
        started |= !blank;
    }
    Ok(header)
}

fn collect_component_roots(path: &Path, ignore: &[impl AsRef<str>], roots: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.file_name() == Some(OsStr::new("antora.yml")) {
            if let Some(root) = path.parent().filter(|root| root.join("modules").is_dir()) {
                roots.push(normalize_path(root));
            }
        }
        return;
    }

    if path.join("antora.yml").is_file() && path.join("modules").is_dir() {
        roots.push(normalize_path(path));
    }
    // An unreadable folder holds no components the workspace can use.
    let Ok(entries) = sorted_entries(path) else {
        return;
    };
    for entry in entries {
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_dir && !is_ignored(&entry.file_name(), ignore) {
            collect_component_roots(&entry.path(), ignore, roots);
        }
    }
}

pub(crate) fn discover_component(
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

        let family_roots = ResourceFamily::ALL.into_iter().flat_map(|family| {
            family
                .directories()
                .iter()
                .map(move |directory| (family, directory))
        });
        for (family, directory) in family_roots {
            let family_root = root.join(directory);
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
                let coordinate = AntoraCoordinate {
                    component: component.clone(),
                    version: version.clone(),
                    module: name.clone(),
                    family,
                    relative_path,
                };
                let aliases = if family == ResourceFamily::Page {
                    read_header(&source_path)
                        .map(|header| page_aliases(&header, &coordinate))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                catalog.insert(AntoraResource {
                    coordinate,
                    source_path,
                    aliases,
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

fn is_ignored(name: &OsStr, ignore: &[impl AsRef<str>]) -> bool {
    name.to_str()
        .is_some_and(|name| ignore.iter().any(|ignored| ignored.as_ref() == name))
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

    /// Only the header is read for `:page-aliases:`: a body that is not text, or an attribute
    /// line after the header, makes no difference.
    #[test]
    fn page_aliases_are_read_from_the_header_alone() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("antora.yml"), "name: demo\nversion: ~\n").unwrap();
        let pages = root.path().join("modules/ROOT/pages");
        std::fs::create_dir_all(&pages).unwrap();
        let mut page =
            b"\n= Guide\n:page-aliases: old-guide.adoc\n\n:page-aliases: body.adoc\n".to_vec();
        page.extend_from_slice(&[0xff, 0xfe, b'\n']);
        std::fs::write(pages.join("guide.adoc"), page).unwrap();

        let discovery = discover_antora_workspace(&[root.path().to_path_buf()]).unwrap();
        let guide = discovery
            .catalog
            .resources()
            .find(|resource| resource.source_path.ends_with("guide.adoc"))
            .unwrap();
        let aliases: Vec<&str> = guide
            .aliases
            .iter()
            .map(|alias| alias.path.as_str())
            .collect();
        assert_eq!(aliases, ["old-guide.adoc"]);
    }

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

    #[test]
    fn legacy_assets_directories_hold_images_and_attachments() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-assets");
        let discovery = discover_antora_workspace(std::slice::from_ref(&root)).unwrap();
        let context = AntoraContext {
            component: "assets".to_owned(),
            version: None,
            module: "ROOT".to_owned(),
            family: ResourceFamily::Page,
        };
        for (id, path) in [
            ("image$legacy.svg", "modules/ROOT/assets/images/legacy.svg"),
            ("image$modern.svg", "modules/ROOT/images/modern.svg"),
            (
                "attachment$guide.txt",
                "modules/ROOT/assets/attachments/guide.txt",
            ),
        ] {
            let resource = AntoraResolver::resolve(
                &discovery.catalog,
                &parse_resource_id(id).unwrap(),
                &context,
            )
            .unwrap_or_else(|error| panic!("{id}: {error:?}"));
            assert!(resource.source_path.ends_with(path), "{id}");
        }
    }
}
