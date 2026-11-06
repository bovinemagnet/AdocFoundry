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
    // The first root with a playbook supplies the site-wide attributes, else the nearest one
    // above the first component version that has one, beside its `antora.yml` or in a folder
    // between it and its root (a component in `docs/` of a code repository, or several in
    // `docs/components/`, with their local playbook).
    for folder in playbook_folders(roots, &component_roots) {
        let path = folder.join(crate::PLAYBOOK_FILE);
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

/// Where a playbook is looked for, in order: each root, then each component version's folder
/// and the folders above it up to (not including) its root.
fn playbook_folders<'a>(
    roots: &'a [PathBuf],
    component_roots: &'a [PathBuf],
) -> impl Iterator<Item = PathBuf> + 'a {
    roots
        .iter()
        .cloned()
        .chain(component_roots.iter().flat_map(move |component| {
            let root = roots
                .iter()
                .filter(|root| component.starts_with(root))
                .max_by_key(|root| root.components().count());
            component
                .ancestors()
                .take_while(move |folder| root.is_some_and(|root| *folder != root.as_path()))
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        }))
}

/// The ids a page's `:page-aliases:` header attribute lists, qualified with the page's own
/// coordinates where they omit them. Ids that do not parse are skipped.
pub fn page_aliases(text: &str, page: &AntoraCoordinate) -> Vec<AntoraResourceId> {
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
    let modules_root = descriptor.root.join("modules");
    let mut modules = Vec::new();
    for entry in sorted_entries(&modules_root)? {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let root = normalize_path(&entry.path());
        let mut files = Vec::new();
        for directory in ResourceFamily::ALL
            .into_iter()
            .flat_map(|family| family.directories().iter())
        {
            let family_root = root.join(directory);
            if family_root.is_dir() {
                collect_files(&family_root, &mut files)?;
            }
        }
        if root.join("nav.adoc").is_file() {
            files.push(root.join("nav.adoc"));
        }
        modules.push((name, files));
    }
    catalog_component(catalog, descriptor, modules, &|path| read_header(path).ok());
    Ok(())
}

/// Puts `descriptor`'s component version in `catalog` with its `modules`, each a name and the
/// files beneath it, as disk discovery and discovery from a revision's file list both find them;
/// `header` gives a page's header lines, for its `page-aliases`.
fn catalog_component(
    catalog: &mut AntoraCatalog,
    descriptor: ComponentDescriptor,
    modules: Vec<(String, Vec<PathBuf>)>,
    header: &dyn Fn(&Path) -> Option<String>,
) {
    let component = descriptor.name.clone();
    let version = descriptor.version.clone();
    let modules_root = descriptor.root.join("modules");
    catalog.insert_component(descriptor);

    for (name, mut files) in modules {
        let root = normalize_path(&modules_root.join(&name));
        files.sort();
        let nav_path = root.join("nav.adoc");
        catalog.insert_module(Module {
            component: component.clone(),
            version: version.clone(),
            name: name.clone(),
            root: root.clone(),
            nav: files.contains(&nav_path).then_some(nav_path),
        });

        let family_roots = ResourceFamily::ALL.into_iter().flat_map(|family| {
            family
                .directories()
                .iter()
                .map(move |directory| (family, directory))
        });
        for (family, directory) in family_roots {
            let family_root = root.join(directory);
            for source_path in files.iter().filter(|file| file.starts_with(&family_root)) {
                let relative_path = source_path
                    .strip_prefix(&family_root)
                    .expect("filtered to the family root")
                    .to_path_buf();
                let coordinate = AntoraCoordinate {
                    component: component.clone(),
                    version: version.clone(),
                    module: name.clone(),
                    family,
                    relative_path,
                };
                let aliases = if family == ResourceFamily::Page {
                    header(source_path)
                        .map(|header| page_aliases(&header, &coordinate))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                catalog.insert(AntoraResource {
                    coordinate,
                    source_path: source_path.clone(),
                    aliases,
                });
            }
        }
    }
}

/// Discovers the components among `files`, a revision's files by absolute path with their text,
/// by the rules disk discovery follows: a folder holding `antora.yml` and files under
/// `modules/` is a component version, each folder under `modules/` holding files a module, and
/// the playbook is the first root's, else the first component version's beside its
/// `antora.yml`, as disk discovery finds it. Nothing is read from disk. Folders named in `ignore` are
/// skipped, as disk discovery skips them.
pub fn discover_antora_files(
    roots: &[PathBuf],
    files: &std::collections::BTreeMap<PathBuf, String>,
    ignore: &[impl AsRef<str>],
) -> DiscoveryResult {
    let ignored = |path: &Path| {
        path.components().any(|part| match part {
            Component::Normal(name) => is_ignored(name, ignore),
            _ => false,
        })
    };
    let mut result = DiscoveryResult::default();
    let mut component_roots: Vec<PathBuf> = files
        .keys()
        .filter(|path| path.file_name() == Some(OsStr::new("antora.yml")))
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .filter(|root| {
            roots
                .iter()
                .find(|workspace| root.starts_with(workspace))
                .and_then(|workspace| root.strip_prefix(workspace).ok())
                .is_some_and(|relative| !ignored(relative))
        })
        .filter(|root| {
            let modules = root.join("modules");
            files.keys().any(|file| file.starts_with(&modules))
        })
        .collect();
    component_roots.sort();
    component_roots.dedup();
    for folder in playbook_folders(roots, &component_roots) {
        let path = normalize_path(&folder.join(crate::PLAYBOOK_FILE));
        if let Some(source) = files.get(&path) {
            match crate::parse_playbook(&path, source) {
                Ok(playbook) => result.catalog.set_playbook(Some(playbook)),
                Err(error) => result.issues.push(error),
            }
            break;
        }
    }
    for root in component_roots {
        let descriptor_path = root.join("antora.yml");
        let source = &files[&descriptor_path];
        let descriptor = match crate::parse_component_descriptor(&root, source) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                result.issues.push(error);
                continue;
            }
        };
        let modules_root = root.join("modules");
        let mut modules: std::collections::BTreeMap<String, Vec<PathBuf>> =
            std::collections::BTreeMap::new();
        for file in files.keys() {
            let Ok(relative) = file.strip_prefix(&modules_root) else {
                continue;
            };
            let mut parts = relative.components();
            let (Some(Component::Normal(name)), Some(_)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Some(name) = name.to_str() else {
                continue;
            };
            modules
                .entry(name.to_owned())
                .or_default()
                .push(file.clone());
        }
        let header = |path: &Path| files.get(path).map(|text| header_lines(text));
        catalog_component(
            &mut result.catalog,
            descriptor,
            modules.into_iter().collect(),
            &header,
        );
    }
    result
}

/// The lines of `text` that [`header_attribute`] reads, as [`read_header`] reads them from a file.
fn header_lines(text: &str) -> String {
    let mut header = String::new();
    let mut started = false;
    for line in text.split_inclusive('\n') {
        let blank = line.trim_end().is_empty();
        if blank && started {
            break;
        }
        started |= !blank;
        header.push_str(line);
    }
    header
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

    /// A component in `docs/` of a code repository keeps its local playbook beside
    /// `antora.yml`; with none at the root, that one is the site's.
    #[test]
    fn a_playbook_beside_a_component_is_the_site_s_when_no_root_has_one() {
        let root = tempfile::tempdir().unwrap();
        let docs = root.path().join("docs");
        std::fs::create_dir_all(docs.join("modules/ROOT/pages")).unwrap();
        std::fs::write(docs.join("antora.yml"), "name: demo\nversion: ~\n").unwrap();
        std::fs::write(docs.join("modules/ROOT/pages/index.adoc"), "= Home\n").unwrap();
        std::fs::write(
            docs.join("antora-playbook.yml"),
            "site:\n  title: Demo\n  start_page: demo::index.adoc\n",
        )
        .unwrap();
        let discovery = discover_antora_workspace(&[root.path().to_path_buf()]).unwrap();
        let playbook = discovery
            .catalog
            .playbook()
            .expect("found beside the component");
        assert_eq!(playbook.site_title.as_deref(), Some("Demo"));
        let start = discovery
            .catalog
            .resolve_start_page(&playbook.start_page.as_ref().unwrap().0)
            .unwrap();
        assert!(start.source_path.ends_with("pages/index.adoc"));
        for missing in [
            "demo::missing.adoc",
            "index.adoc",
            "other::index.adoc",
            "demo:ROOT:partial$index.adoc",
        ] {
            assert!(
                discovery.catalog.resolve_start_page(missing).is_none(),
                "{missing}"
            );
        }

        // The same from a file list, as a Git revision is read.
        let files: std::collections::BTreeMap<std::path::PathBuf, String> = [
            ("docs/antora.yml", "name: demo\nversion: ~\n"),
            ("docs/modules/ROOT/pages/index.adoc", "= Home\n"),
            ("docs/antora-playbook.yml", "site:\n  title: Demo\n"),
        ]
        .into_iter()
        .map(|(path, text)| (root.path().join(path), text.to_owned()))
        .collect();
        let listed =
            crate::discover_antora_files(&[root.path().to_path_buf()], &files, &[] as &[&str]);
        assert!(listed.catalog.playbook().is_some());
    }

    #[test]
    fn a_playbook_above_components_is_found_up_to_their_root() {
        let root = tempfile::tempdir().unwrap();
        let docs = root.path().join("docs");
        for component in ["guide", "reference"] {
            let folder = docs.join("components").join(component);
            std::fs::create_dir_all(folder.join("modules/ROOT/pages")).unwrap();
            std::fs::write(
                folder.join("antora.yml"),
                format!("name: {component}\nversion: ~\n"),
            )
            .unwrap();
            std::fs::write(folder.join("modules/ROOT/pages/index.adoc"), "= Home\n").unwrap();
        }
        std::fs::write(docs.join("antora-playbook.yml"), "site:\n  title: Both\n").unwrap();
        // Above the root, a playbook is not the workspace's.
        let outside = tempfile::tempdir().unwrap();
        let nested = outside.path().join("work");
        std::fs::create_dir_all(nested.join("c/modules/ROOT/pages")).unwrap();
        std::fs::write(nested.join("c/antora.yml"), "name: c\nversion: ~\n").unwrap();
        std::fs::write(
            outside.path().join("antora-playbook.yml"),
            "site:\n  title: No\n",
        )
        .unwrap();

        let discovery = discover_antora_workspace(&[root.path().to_path_buf()]).unwrap();
        let playbook = discovery
            .catalog
            .playbook()
            .expect("found above the components");
        assert_eq!(playbook.site_title.as_deref(), Some("Both"));
        let discovery = discover_antora_workspace(&[nested]).unwrap();
        assert!(discovery.catalog.playbook().is_none());
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
