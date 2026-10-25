//! `cargo xtask layers`: enforces the crate ladder described in `CLAUDE.md`.
//!
//! Every crate under `crates/` must appear in [`LAYERS`]. A crate may depend only on crates in a
//! lower layer, except for the same-layer pairs listed in [`SAME_LAYER_ALLOWED`]. UI framework
//! crates are confined to the app crate and LSP crates to the language server, and nothing but
//! the app may reach a network client, directly or through its dependencies. The MCP server
//! may listen for HTTP, building only the server side of the HTTP crates.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// The layer of every workspace crate. A new crate needs a line here.
pub const LAYERS: &[(&str, u8)] = &[
    ("adoc-core", 0),
    ("adoc-parser", 1),
    ("adoc-dom", 1),
    ("adoc-antora", 1),
    ("adoc-index", 1),
    ("adoc-lang", 1),
    ("adoc-math", 1),
    ("adoc-render", 2),
    ("adoc-diagrams", 2),
    ("adoc-model", 2),
    ("adoc-git", 2),
    ("adoc-pdf", 2),
    ("adoc-engine", 3),
    ("adocfoundry", 4),
    ("adocfoundry-cli", 4),
    ("adoc-ls", 4),
    ("adoc-mcp", 4),
];

/// Same-layer dependencies that are permitted, as `(dependent, dependency)`.
pub const SAME_LAYER_ALLOWED: &[(&str, &str)] = &[
    ("adoc-index", "adoc-parser"),
    ("adoc-lang", "adoc-parser"),
    ("adoc-lang", "adoc-index"),
    ("adoc-lang", "adoc-antora"),
    ("adoc-pdf", "adoc-render"),
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

/// HTTP client crates. Rendering, diagrams and every engine feature work locally; only the app
/// reaches one, through `gpui-kit-assets`.
pub const NETWORK_CRATES: &[&str] = &[
    "reqwest",
    "gpui-pre-reqwest",
    "ureq",
    "isahc",
    "curl",
    "attohttpc",
    "surf",
];

/// HTTP crates with a server side and a client side. Reaching one is reaching a client, except
/// from a crate in [`NETWORK_SERVER_ALLOWED`] whose own build leaves out the client side.
pub const HTTP_CRATES: &[&str] = &["hyper", "hyper-util"];

/// Workspace crates that may reach a network client.
pub const NETWORK_ALLOWED: &[&str] = &["adocfoundry"];

/// Workspace crates that may listen for HTTP through [`HTTP_CRATES`], without their client
/// features: the MCP server's Streamable HTTP transport.
pub const NETWORK_SERVER_ALLOWED: &[&str] = &["adoc-mcp"];

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

/// Every workspace crate outside [`NETWORK_ALLOWED`] that reaches a network client through
/// normal or build dependencies, from `cargo metadata`'s resolved graph. Dev-dependencies are
/// not shipped and are not followed. A crate in [`NETWORK_SERVER_ALLOWED`] may reach
/// [`HTTP_CRATES`]; [`server_violations`] checks that it builds no client side of them.
pub fn network_violations(metadata: &serde_json::Value) -> Vec<String> {
    let names: BTreeMap<&str, &str> = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|package| Some((package["id"].as_str()?, package["name"].as_str()?)))
        .collect();
    let dependencies: BTreeMap<&str, Vec<&str>> = metadata["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let shipped = node["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|dependency| {
                    dependency["dep_kinds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|kind| kind["kind"].as_str() != Some("dev"))
                })
                .filter_map(|dependency| dependency["pkg"].as_str())
                .collect();
            Some((node["id"].as_str()?, shipped))
        })
        .collect();

    let mut violations = Vec::new();
    for member in metadata["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        let name = names.get(member).copied().unwrap_or(member);
        if NETWORK_ALLOWED.contains(&name) {
            continue;
        }
        let server = NETWORK_SERVER_ALLOWED.contains(&name);
        let mut seen = BTreeSet::new();
        let mut stack = vec![member];
        let mut reached = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(dependency) = names.get(id).filter(|name| {
                NETWORK_CRATES.contains(name) || (!server && HTTP_CRATES.contains(name))
            }) {
                reached.insert(*dependency);
            }
            stack.extend(dependencies.get(id).into_iter().flatten());
        }
        violations.extend(
            reached.into_iter().map(|client| {
                format!("{name} reaches network client {client}; only adocfoundry may")
            }),
        );
    }
    violations
}

/// The feature nodes, such as `hyper feature "server"`, of what `cargo build -p <package>`
/// compiles on any target, from `cargo tree`; `cargo metadata` unifies features over the
/// workspace, where the app's client turns on hyper's client side.
fn build_features(root: &Path, package: &str) -> Result<BTreeSet<String>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["tree", "--locked", "-p", package, "--target", "all"])
        .args([
            "-e",
            "features,normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .output()
        .map_err(|error| format!("cargo tree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim_end_matches(" (*)").to_owned())
        .filter(|line| line.contains(" feature \""))
        .collect())
}

/// A violation for each client feature of [`HTTP_CRATES`] that `package`, a crate allowed to
/// listen for HTTP, builds: `client` and every `client-…` feature.
fn server_violations(package: &str, features: &BTreeSet<String>) -> Vec<String> {
    features
        .iter()
        .filter(|node| {
            node.split_once(" feature \"")
                .is_some_and(|(name, feature)| {
                    HTTP_CRATES.contains(&name) && feature.starts_with("client")
                })
        })
        .map(|node| format!("{package} builds the HTTP client side ({node}); only adocfoundry may"))
        .collect()
}

/// The workspace's resolved dependency graph, from `cargo metadata`.
fn cargo_metadata(root: &Path) -> Result<serde_json::Value, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()
        .map_err(|error| format!("cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("cargo metadata: {error}"))
}

/// What the language server's default build must not compile: Typst and Git come only with its
/// `typst` and `git` features.
const LEAN_EXCLUDED: &[&str] = &["adoc-pdf", "adoc-math", "adoc-git", "typst", "gix"];

/// The packages `cargo build -p <package>` compiles with its default features, from
/// `cargo tree`; `cargo metadata` resolves features for the whole workspace at once.
fn default_build_packages(root: &Path, package: &str) -> Result<BTreeSet<String>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["tree", "--locked", "-p", package, "-e", "normal,build"])
        .args(["--prefix", "none", "--format", "{p}"])
        .output()
        .map_err(|error| format!("cargo tree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect())
}

/// A violation for each crate of [`LEAN_EXCLUDED`] in the language server's default build.
fn lean_violations(packages: &BTreeSet<String>) -> Vec<String> {
    LEAN_EXCLUDED
        .iter()
        .filter(|name| packages.contains(**name))
        .map(|name| format!("adoc-ls builds {name} by default; only its features may bring it"))
        .collect()
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
    violations.extend(network_violations(&cargo_metadata(root)?));
    for package in NETWORK_SERVER_ALLOWED {
        violations.extend(server_violations(package, &build_features(root, package)?));
    }
    violations.extend(lean_violations(&default_build_packages(root, "adoc-ls")?));
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

    /// A package and its dependencies as `(dependency, kind)`, `None` for a normal dependency.
    type Package<'a> = (&'a str, &'a [(&'a str, Option<&'a str>)]);

    /// A `cargo metadata` document with workspace `members` and `packages`, and nothing else.
    fn metadata(members: &[&str], packages: &[Package<'_>]) -> serde_json::Value {
        let id = |name: &str| format!("{name} 0.1.0 (registry)");
        serde_json::json!({
            "packages": packages.iter().map(|(name, _)| serde_json::json!({ "id": id(name), "name": name })).collect::<Vec<_>>(),
            "workspace_members": members.iter().map(|name| id(name)).collect::<Vec<_>>(),
            "resolve": { "nodes": packages.iter().map(|(name, dependencies)| serde_json::json!({
                "id": id(name),
                "deps": dependencies.iter().map(|(dependency, kind)| serde_json::json!({
                    "pkg": id(dependency),
                    "dep_kinds": [{ "kind": kind }],
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>() },
        })
    }

    #[test]
    fn network_clients_are_confined_to_the_app() {
        let graph = metadata(
            &["adocfoundry", "adoc-engine", "adoc-diagrams"],
            &[
                ("adocfoundry", &[("gpui-kit-assets", None)]),
                ("gpui-kit-assets", &[("gpui-pre-reqwest", None)]),
                ("gpui-pre-reqwest", &[("hyper", None)]),
                ("hyper", &[]),
                // Through another crate, and through a build dependency.
                ("adoc-engine", &[("fetcher", None)]),
                ("fetcher", &[("ureq", Some("build"))]),
                ("ureq", &[]),
                // A dev-dependency is not shipped.
                ("adoc-diagrams", &[("mock-server", Some("dev"))]),
                ("mock-server", &[("reqwest", None)]),
                ("reqwest", &[]),
            ],
        );
        assert_eq!(
            network_violations(&graph),
            ["adoc-engine reaches network client ureq; only adocfoundry may"]
        );
    }

    #[test]
    fn http_servers_are_allowed_only_in_adoc_mcp() {
        let graph = metadata(
            &["adoc-mcp", "adoc-engine"],
            &[
                ("adoc-mcp", &[("hyper-util", None)]),
                ("hyper-util", &[("hyper", None)]),
                ("hyper", &[]),
                ("adoc-engine", &[("hyper", None)]),
            ],
        );
        assert_eq!(
            network_violations(&graph),
            ["adoc-engine reaches network client hyper; only adocfoundry may"]
        );

        let server: BTreeSet<String> = [
            r#"hyper feature "server""#,
            r#"hyper feature "http1""#,
            r#"hyper-util feature "tokio""#,
        ]
        .map(str::to_owned)
        .into();
        assert_eq!(server_violations("adoc-mcp", &server), Vec::<String>::new());
        let client: BTreeSet<String> = [
            r#"hyper feature "client""#,
            r#"hyper-util feature "client-legacy""#,
        ]
        .map(str::to_owned)
        .into();
        assert_eq!(server_violations("adoc-mcp", &client).len(), 2);

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let built = build_features(&root, "adoc-mcp").unwrap();
        assert!(built.contains(r#"hyper feature "server""#), "{built:?}");
        assert_eq!(server_violations("adoc-mcp", &built), Vec::<String>::new());
    }

    #[test]
    fn default_build_has_no_typst_or_git() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let packages = default_build_packages(&root, "adoc-ls").unwrap();
        assert!(packages.contains("adoc-engine"), "{packages:?}");
        assert_eq!(lean_violations(&packages), Vec::<String>::new());

        let full: BTreeSet<String> = ["adoc-engine", "typst", "gix"].map(str::to_owned).into();
        assert_eq!(
            lean_violations(&full),
            [
                "adoc-ls builds typst by default; only its features may bring it",
                "adoc-ls builds gix by default; only its features may bring it",
            ]
        );
    }

    #[test]
    fn current_workspace_passes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(run(&root).unwrap(), Vec::<String>::new());
    }
}
