//! `cargo xtask layers`: enforces the crate ladder described in `CLAUDE.md`.
//!
//! Every crate under `crates/` must appear in [`LAYERS`]. A crate may depend only on crates in a
//! lower layer, except for the same-layer pairs listed in [`SAME_LAYER_ALLOWED`]. UI framework
//! crates are confined to the app crate and LSP crates to the language server.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// The layer of every workspace crate. A new crate needs a line here.
pub const LAYERS: &[(&str, u8)] = &[
    ("adoc-core", 0),
    ("adoc-parser", 1),
    ("adoc-dom", 1),
    ("adoc-antora", 1),
    ("adoc-index", 1),
    ("adoc-lang", 1),
    ("adoc-render", 2),
    ("adoc-diagrams", 2),
    ("adoc-model", 2),
    ("adoc-git", 2),
    ("adoc-pdf", 2),
    ("adoc-engine", 3),
    ("adocfoundry", 4),
    ("adocfoundry-cli", 4),
    ("adoc-ls", 4),
];

/// Same-layer dependencies that are permitted, as `(dependent, dependency)`.
pub const SAME_LAYER_ALLOWED: &[(&str, &str)] = &[
    ("adoc-index", "adoc-parser"),
    ("adoc-lang", "adoc-parser"),
    ("adoc-lang", "adoc-index"),
    ("adoc-lang", "adoc-antora"),
];

/// UI framework crates, permitted only in `adocfoundry`.
pub const UI_CRATES: &[&str] = &[
    "gpui",
    "gpui-pre",
    "gpui-pre-platform",
    "gpui-component",
    "gpui-base",
    "gpui-kit-assets",
    "egui",
    "eframe",
];

/// LSP crates, permitted only in `adoc-ls`.
pub const LSP_CRATES: &[&str] = &["lsp-types", "lsp-server"];

/// Exceptions to the LSP confinement, as `(crate, dependency)`. gpui-component's editor exposes
/// definition, hover and completion only through provider traits typed with `lsp-types`, so the
/// app uses those types in one translation module; it never speaks the protocol.
pub const LSP_ALLOWED: &[(&str, &str)] = &[("adocfoundry", "lsp-types")];

/// External AsciiDoc parser crates, permitted only in `adoc-dom` so the parser stays swappable.
pub const PARSER_CRATES: &[&str] = &["acdc-parser", "asciidoc-parser"];

/// Source-level imports forbidden in `adoc-lang`.
const ADOC_LANG_FORBIDDEN_PATHS: &[&str] = &["lsp_types", "lsp_server", "url::"];

#[derive(Debug, Eq, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub dependencies: Vec<String>,
}

/// Parses the dependency names out of a crate manifest, across normal, build and dev
/// dependencies, including target-specific tables.
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let value: toml::Table = text.parse().map_err(|error| format!("{error}"))?;
    let name = value
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or("manifest has no package.name")?
        .to_owned();

    let mut dependencies = Vec::new();
    collect_dependency_tables(&value, &mut dependencies);
    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        for target in targets.values().filter_map(toml::Value::as_table) {
            collect_dependency_tables(target, &mut dependencies);
        }
    }
    dependencies.sort();
    dependencies.dedup();
    Ok(Manifest { name, dependencies })
}

fn collect_dependency_tables(table: &toml::Table, out: &mut Vec<String>) {
    for key in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if let Some(dependencies) = table.get(key).and_then(toml::Value::as_table) {
            for (name, spec) in dependencies {
                // `foo = { package = "bar" }` depends on `bar`.
                let package = spec
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(name);
                out.push(package.to_owned());
            }
        }
    }
}

fn layer_of(name: &str) -> Option<u8> {
    LAYERS
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, layer)| *layer)
}

/// Checks one manifest against the ladder and confinement rules, returning every violation.
pub fn check_manifest(manifest: &Manifest) -> Vec<String> {
    let mut violations = Vec::new();
    let name = manifest.name.as_str();
    let Some(layer) = layer_of(name) else {
        return vec![format!(
            "{name}: not in the layer table (xtask/src/layers.rs)"
        )];
    };

    for dependency in &manifest.dependencies {
        let dependency = dependency.as_str();
        // A dev-dependency on itself (to turn on a feature for its own tests) is no dependency.
        if dependency == name {
            continue;
        }
        if let Some(dependency_layer) = layer_of(dependency) {
            let allowed = dependency_layer < layer
                || (dependency_layer == layer && SAME_LAYER_ALLOWED.contains(&(name, dependency)));
            if !allowed {
                violations.push(format!(
                    "{name} (L{layer}) must not depend on {dependency} (L{dependency_layer})"
                ));
            }
        }
        if UI_CRATES.contains(&dependency) && name != "adocfoundry" {
            violations.push(format!(
                "{name} depends on UI crate {dependency}; only adocfoundry may"
            ));
        }
        if LSP_CRATES.contains(&dependency)
            && name != "adoc-ls"
            && !LSP_ALLOWED.contains(&(name, dependency))
        {
            violations.push(format!(
                "{name} depends on LSP crate {dependency}; only adoc-ls may"
            ));
        }
    }

    for dependency in &manifest.dependencies {
        if PARSER_CRATES.contains(&dependency.as_str()) && name != "adoc-dom" {
            violations.push(format!(
                "{name} depends on parser crate {dependency}; only adoc-dom may"
            ));
        }
    }

    if name == "adoc-core" && !manifest.dependencies.is_empty() {
        violations.push(format!(
            "adoc-core must have no dependencies, found: {}",
            manifest.dependencies.join(", ")
        ));
    }
    if name == "adoc-lang" && manifest.dependencies.iter().any(|d| d == "url") {
        violations.push("adoc-lang must not depend on url".to_owned());
    }
    violations
}

/// Checks Rust sources for imports that a crate must not use.
pub fn check_sources(name: &str, files: &BTreeMap<PathBuf, String>) -> Vec<String> {
    if name != "adoc-lang" {
        return Vec::new();
    }
    let mut violations = Vec::new();
    for (path, text) in files {
        for forbidden in ADOC_LANG_FORBIDDEN_PATHS {
            if text.contains(forbidden) {
                violations.push(format!(
                    "{}: adoc-lang must not reference `{forbidden}`",
                    path.display()
                ));
            }
        }
    }
    violations
}

/// Runs every check over the workspace rooted at `root`.
pub fn run(root: &Path) -> Result<Vec<String>, String> {
    let crates = root.join("crates");
    let mut entries: Vec<PathBuf> = fs::read_dir(&crates)
        .map_err(|error| format!("{}: {error}", crates.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    entries.sort();

    let mut violations = Vec::new();
    for directory in entries {
        let manifest_path = directory.join("Cargo.toml");
        let text = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        let manifest = parse_manifest(&text)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        violations.extend(check_manifest(&manifest));

        let mut sources = BTreeMap::new();
        collect_sources(&directory.join("src"), &mut sources);
        violations.extend(check_sources(&manifest.name, &sources));
    }
    Ok(violations)
}

fn collect_sources(directory: &Path, out: &mut BTreeMap<PathBuf, String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            if let Ok(text) = fs::read_to_string(&path) {
                out.insert(path, text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, dependencies: &[&str]) -> Manifest {
        Manifest {
            name: name.to_owned(),
            dependencies: dependencies.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    #[test]
    fn parses_all_dependency_tables_and_renames() {
        let parsed = parse_manifest(
            r#"
[package]
name = "adoc-engine"

[dependencies]
adoc-lang.workspace = true
renamed = { package = "adoc-index", path = "../adoc-index" }

[dev-dependencies]
tempfile = "=3.0.0"

[target.'cfg(unix)'.dependencies]
libc = "=0.2.0"
"#,
        )
        .unwrap();
        assert_eq!(parsed.name, "adoc-engine");
        assert_eq!(
            parsed.dependencies,
            ["adoc-index", "adoc-lang", "libc", "tempfile"]
        );
    }

    #[test]
    fn accepts_downward_and_listed_same_layer_dependencies() {
        assert!(check_manifest(&manifest("adoc-engine", &["adoc-lang", "adoc-core"])).is_empty());
        assert!(check_manifest(&manifest("adoc-lang", &["adoc-index", "adoc-antora"])).is_empty());
    }

    #[test]
    fn rejects_upward_and_unlisted_same_layer_dependencies() {
        assert_eq!(
            check_manifest(&manifest("adoc-index", &["adoc-engine"])).len(),
            1
        );
        assert_eq!(
            check_manifest(&manifest("adoc-antora", &["adoc-index"])).len(),
            1
        );
        assert_eq!(
            check_manifest(&manifest("adoc-ls", &["adocfoundry-cli"])).len(),
            1
        );
    }

    #[test]
    fn accepts_a_crate_depending_on_itself() {
        // A dev-dependency on the crate itself turns on a feature for its own tests.
        assert!(check_manifest(&manifest("adoc-git", &["adoc-git", "adoc-core"])).is_empty());
    }

    #[test]
    fn confines_ui_and_lsp_crates() {
        assert!(check_manifest(&manifest("adocfoundry", &["gpui", "gpui-component"])).is_empty());
        assert_eq!(check_manifest(&manifest("adoc-engine", &["gpui"])).len(), 1);
        assert_eq!(
            check_manifest(&manifest("adoc-lang", &["lsp-types"])).len(),
            1
        );
        assert!(check_manifest(&manifest("adoc-ls", &["lsp-types", "lsp-server"])).is_empty());
        assert!(check_manifest(&manifest("adocfoundry", &["lsp-types"])).is_empty());
        assert_eq!(
            check_manifest(&manifest("adocfoundry", &["lsp-server"])).len(),
            1
        );
        assert_eq!(
            check_manifest(&manifest("adoc-engine", &["gpui-pre-platform"])).len(),
            1
        );
    }

    #[test]
    fn rejects_unknown_crates_and_core_dependencies() {
        assert_eq!(check_manifest(&manifest("adoc-new", &[])).len(), 1);
        assert_eq!(check_manifest(&manifest("adoc-core", &["serde"])).len(), 1);
    }

    #[test]
    fn forbids_lsp_and_url_imports_in_adoc_lang_sources() {
        let mut files = BTreeMap::new();
        files.insert(PathBuf::from("a.rs"), "use lsp_types::Position;".to_owned());
        files.insert(
            PathBuf::from("b.rs"),
            "let u = url::Url::parse(x);".to_owned(),
        );
        files.insert(PathBuf::from("c.rs"), "fn fine() {}".to_owned());
        assert_eq!(check_sources("adoc-lang", &files).len(), 2);
        assert!(check_sources("adoc-ls", &files).is_empty());
    }

    #[test]
    fn current_workspace_passes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(run(&root).unwrap(), Vec::<String>::new());
    }
}
