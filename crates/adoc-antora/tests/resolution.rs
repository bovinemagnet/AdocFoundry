//! Cross-component, cross-version, alias, descriptor reload and navigation tree behaviour.

use std::{
    fs,
    path::{Path, PathBuf},
};

use adoc_antora::{
    discover_antora_workspace, nav_tree, parse_resource_id, AntoraCatalog, AntoraContext,
    AntoraResolver, ResolutionError, ResourceFamily,
};

fn fixture(name: &str) -> PathBuf {
    adoc_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name),
    )
}

fn adoc_path(path: &Path) -> PathBuf {
    let mut normalised = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalised.pop();
            }
            std::path::Component::CurDir => {}
            other => normalised.push(other),
        }
    }
    normalised
}

fn catalog(roots: &[PathBuf]) -> AntoraCatalog {
    let discovery = discover_antora_workspace(roots).unwrap();
    assert!(discovery.issues.is_empty(), "{:?}", discovery.issues);
    discovery.catalog
}

fn page_context(component: &str, version: Option<&str>) -> AntoraContext {
    AntoraContext {
        component: component.to_owned(),
        version: version.map(str::to_owned),
        module: "ROOT".to_owned(),
        family: ResourceFamily::Page,
    }
}

fn resolve_title(catalog: &AntoraCatalog, id: &str, context: &AntoraContext) -> String {
    let resource = AntoraResolver::resolve(catalog, &parse_resource_id(id).unwrap(), context)
        .unwrap_or_else(|error| panic!("{id}: {error}"));
    fs::read_to_string(&resource.source_path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn resolves_cross_component() {
    let catalog = catalog(&[fixture("antora-multi-component")]);
    let alpha = page_context("alpha", Some("1.0"));
    assert_eq!(
        resolve_title(&catalog, "beta::index.adoc", &alpha),
        "= Beta"
    );
    assert_eq!(
        resolve_title(&catalog, "beta:api:reference.adoc", &alpha),
        "= API Reference"
    );
    let beta = page_context("beta", None);
    assert_eq!(
        resolve_title(&catalog, "1.0@alpha::index.adoc", &beta),
        "= Alpha"
    );
    // A module given without a component stays in the current component version.
    assert!(matches!(
        AntoraResolver::resolve(
            &catalog,
            &parse_resource_id("api:reference.adoc").unwrap(),
            &alpha
        ),
        Err(ResolutionError::UnknownModule { .. })
    ));
}

#[test]
fn resolves_latest_version_when_omitted() {
    let catalog = catalog(&[fixture("antora-versions")]);
    let v2 = page_context("product", Some("2.0"));
    // The versionless (`~`) component version sorts above every numbered one.
    assert_eq!(
        resolve_title(&catalog, "product::feature.adoc", &v2),
        "= Feature (unversioned)"
    );
    assert_eq!(
        resolve_title(&catalog, "1.0@product::feature.adoc", &v2),
        "= Feature (1.0)"
    );
    // Neither component nor version given: the current component version.
    assert_eq!(
        resolve_title(&catalog, "feature.adoc", &v2),
        "= Feature (2.0)"
    );
    assert_eq!(
        AntoraResolver::resolve(
            &catalog,
            &parse_resource_id("9.9@product::feature.adoc").unwrap(),
            &v2
        ),
        Err(ResolutionError::UnknownVersion {
            component: "product".to_owned(),
            version: "9.9".to_owned(),
        })
    );
}

#[test]
fn prerelease_sorts_below_released() {
    let root = fixture("antora-versions");
    let catalog = catalog(&[root.join("v1"), root.join("v2"), root.join("next")]);
    let latest = catalog.latest_version("product").unwrap();
    assert_eq!(latest.version.as_deref(), Some("2.0"));
    assert!(
        catalog
            .component("product", Some("3.0"))
            .unwrap()
            .prerelease
    );
    assert_eq!(
        resolve_title(
            &catalog,
            "product::feature.adoc",
            &page_context("product", Some("1.0"))
        ),
        "= Feature (2.0)"
    );
}

#[test]
fn resolves_via_page_alias() {
    let catalog = catalog(&[fixture("antora-multi-component")]);
    let alpha = page_context("alpha", Some("1.0"));
    let id = parse_resource_id("old-guide.adoc").unwrap();
    let resolved = catalog.resolve_detailed(&id, &alpha).unwrap();
    assert!(resolved.via_alias);
    assert!(resolved.resource.source_path.ends_with("pages/guide.adoc"));
    assert_eq!(resolve_title(&catalog, "old-guide.adoc", &alpha), "= Guide");

    let direct = catalog
        .resolve_detailed(&parse_resource_id("guide.adoc").unwrap(), &alpha)
        .unwrap();
    assert!(!direct.via_alias);
    let guide = direct.resource;
    assert_eq!(guide.aliases.len(), 1);
    assert_eq!(guide.aliases[0].component.as_deref(), Some("alpha"));
    assert_eq!(guide.aliases[0].path, "old-guide.adoc");
}

#[test]
fn reload_descriptor_reports_added_and_removed() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_path(directory.path());
    fs::create_dir_all(root.join("modules/ROOT/pages")).unwrap();
    fs::create_dir_all(root.join("modules/extra/pages")).unwrap();
    fs::write(root.join("antora.yml"), "name: demo\nversion: '1.0'\n").unwrap();
    fs::write(root.join("modules/ROOT/pages/index.adoc"), "= Index\n").unwrap();
    fs::write(root.join("modules/extra/pages/extra.adoc"), "= Extra\n").unwrap();
    let mut catalog = catalog(std::slice::from_ref(&root));
    assert_eq!(catalog.len(), 2);

    // Renaming the version moves every resource to new coordinates.
    fs::write(root.join("antora.yml"), "name: demo\nversion: '2.0'\n").unwrap();
    fs::remove_file(root.join("modules/extra/pages/extra.adoc")).unwrap();
    let report = catalog.reload_component(&root.join("antora.yml")).unwrap();

    assert!(catalog.component("demo", Some("1.0")).is_none());
    assert!(catalog.component("demo", Some("2.0")).is_some());
    assert_eq!(report.added.len(), 1);
    assert_eq!(report.added[0].version.as_deref(), Some("2.0"));
    assert_eq!(report.added[0].path, "index.adoc");
    assert_eq!(report.removed.len(), 2);
    assert!(report
        .removed
        .iter()
        .all(|id| id.version.as_deref() == Some("1.0")));

    // A broken descriptor removes the component and reports the error.
    fs::write(root.join("antora.yml"), "version: '2.0'\n").unwrap();
    assert!(catalog.reload_component(&root.join("antora.yml")).is_err());
    assert!(catalog.component("demo", Some("2.0")).is_none());
    assert!(catalog.is_empty());
}

fn read(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

#[test]
fn nav_tree_nests_by_list_depth() {
    let catalog = catalog(&[fixture("antora-multi-component")]);
    let trees = nav_tree(&catalog, "alpha", Some("1.0"), &read).unwrap();
    assert_eq!(trees.len(), 1);
    let tree = &trees[0];
    assert_eq!(tree.component, "alpha");
    assert_eq!(tree.version.as_deref(), Some("1.0"));

    let titles = |items: &[adoc_antora::NavItem]| -> Vec<String> {
        items.iter().map(|item| item.title.clone()).collect()
    };
    assert_eq!(titles(&tree.items), ["Alpha"]);
    let alpha = &tree.items[0];
    assert_eq!(
        titles(&alpha.children),
        ["Home", "Beta home", "Missing page"]
    );
    let home = &alpha.children[0];
    assert_eq!(titles(&home.children), ["Guide", "Background"]);
    assert_eq!(titles(&home.children[0].children), ["Setup"]);
    assert!(
        home.children[1].target.is_none(),
        "plain text has no target"
    );

    let guide = home.children[0].target.as_ref().unwrap();
    assert_eq!(guide.component.as_deref(), Some("alpha"));
    assert_eq!(guide.module.as_deref(), Some("ROOT"));
    assert_eq!(guide.path, "guide.adoc");
    assert_eq!(home.children[0].fragment, None);
    let setup = &home.children[0].children[0];
    assert_eq!(setup.target.as_ref().unwrap().path, "guide.adoc");
    assert_eq!(setup.fragment.as_deref(), Some("setup"));
    let beta = alpha.children[1].target.as_ref().unwrap();
    assert_eq!(beta.component.as_deref(), Some("beta"));
    assert_eq!(beta.version, None);

    let source = &home.source;
    assert!(source.path.ends_with("modules/ROOT/nav.adoc"));
    let text = fs::read_to_string(&source.path).unwrap();
    assert_eq!(
        &text[source.range.start..source.range.end],
        "* xref:index.adoc[Home]"
    );
}

#[test]
fn nav_tree_marks_unresolved() {
    let catalog = catalog(&[fixture("antora-multi-component")]);
    let trees = nav_tree(&catalog, "alpha", Some("1.0"), &read).unwrap();
    let missing = &trees[0].items[0].children[2];
    assert_eq!(missing.title, "Missing page");
    assert!(missing.target.is_none());
    assert_eq!(trees[0].diagnostics.len(), 1);
    let diagnostic = &trees[0].diagnostics[0];
    assert_eq!(diagnostic.code.as_str(), "adoc.antora.nav-unresolved");
    assert_eq!(diagnostic.range, missing.source.range);

    assert!(nav_tree(&catalog, "nobody", None, &read).is_err());
}
