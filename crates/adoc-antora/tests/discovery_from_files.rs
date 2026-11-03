//! Discovery from a revision's file list (Impact PRD §§6.1 and 8) follows the rules disk
//! discovery follows: on each fixture, the catalog found from the files and their text equals
//! the catalog found on disk.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use adoc_antora::{discover_antora_files, discover_antora_workspace, AntoraCatalog};

/// The fixture's path with `..` taken out, as the workspace's roots are given.
fn fixture(name: &str) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("tests/fixtures")
        .join(name)
}

fn files_of(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut files = BTreeMap::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let bytes = fs::read(&path).unwrap();
                files.insert(path, String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }
    files
}

/// What a catalog holds, in a form two catalogs compare by.
fn described(catalog: &AntoraCatalog) -> Vec<String> {
    let mut lines: Vec<String> = catalog
        .components()
        .map(|component| format!("component {component:?}"))
        .chain(catalog.modules().map(|module| format!("module {module:?}")))
        .chain(
            catalog
                .resources()
                .map(|resource| format!("resource {resource:?}")),
        )
        .collect();
    lines.sort();
    lines
}

#[test]
fn a_file_list_is_catalogued_as_the_disk_is() {
    for name in [
        "antora-single-component",
        "antora-multi-component",
        "antora-versions",
        "impact",
        "discovery",
    ] {
        let root = fixture(name);
        let from_disk = discover_antora_workspace(std::slice::from_ref(&root)).unwrap();
        let from_files = discover_antora_files(
            std::slice::from_ref(&root),
            &files_of(&root),
            adoc_core::DEFAULT_IGNORED_DIRECTORIES,
        );
        assert_eq!(
            described(&from_files.catalog),
            described(&from_disk.catalog),
            "{name}"
        );
        assert!(
            from_files.issues.is_empty(),
            "{name}: {:?}",
            from_files.issues
        );
    }
}

#[test]
fn page_aliases_and_ignored_folders_follow_the_same_rules() {
    let root = PathBuf::from("/revision");
    let files = BTreeMap::from([
        (
            root.join("docs/antora.yml"),
            "name: docs\nversion: ~\n".to_owned(),
        ),
        (
            root.join("docs/modules/ROOT/pages/new.adoc"),
            "= New\n:page-aliases: old.adoc\n\nText.\n".to_owned(),
        ),
        (
            root.join("node_modules/x/antora.yml"),
            "name: x\nversion: ~\n".to_owned(),
        ),
        (
            root.join("node_modules/x/modules/ROOT/pages/a.adoc"),
            "= A\n".to_owned(),
        ),
        // A descriptor with no files under `modules/` is no component.
        (
            root.join("empty/antora.yml"),
            "name: empty\nversion: ~\n".to_owned(),
        ),
    ]);
    let found = discover_antora_files(
        std::slice::from_ref(&root),
        &files,
        adoc_core::DEFAULT_IGNORED_DIRECTORIES,
    );
    let names: Vec<&str> = found
        .catalog
        .components()
        .map(|component| component.name.as_str())
        .collect();
    assert_eq!(names, ["docs"]);
    let page = found
        .catalog
        .resource_for_path(&root.join("docs/modules/ROOT/pages/new.adoc"))
        .unwrap();
    assert_eq!(page.aliases.len(), 1);
    assert_eq!(page.aliases[0].path, "old.adoc");
}
