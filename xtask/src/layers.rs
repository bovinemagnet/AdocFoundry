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
    ("adoc-spell", 1),
    ("adoc-graph", 1),
    ("adoc-theme", 1),
    ("adoc-render", 2),
    ("adoc-diagrams", 2),
    ("adoc-model", 2),
    ("adoc-git", 2),
    ("adoc-pdf", 2),
    ("adoc-docx", 2),
    ("adoc-ebook", 2),
    ("adoc-epub", 2),
    ("adoc-docbook", 2),
    ("adoc-kindle", 2),
    ("adoc-audit", 2),
    ("adoc-impact", 2),
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
    ("adoc-docx", "adoc-render"),
    ("adoc-kindle", "adoc-render"),
    ("adoc-ebook", "adoc-render"),
    ("adoc-kindle", "adoc-ebook"),
    ("adoc-epub", "adoc-ebook"),
    ("adoc-epub", "adoc-render"),
    ("adoc-docbook", "adoc-render"),
];

/// Crates whose logic the engine orchestrates: front ends reach them only through
/// `adoc-engine`, never by naming them, so every front end gets the same results.
pub const ENGINE_ONLY: &[&str] = &["adoc-audit", "adoc-impact"];

/// The front ends, the crates of the top layer.
fn is_front_end(name: &str) -> bool {
    layer_of(name) == Some(4)
}

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

/// The spell checker, permitted only in `adoc-spell`.
pub const SPELL_CRATES: &[&str] = &["spellbook"];

/// Source-level imports forbidden in `adoc-lang`.
const ADOC_LANG_FORBIDDEN_PATHS: &[&str] = &["lsp_types", "lsp_server", "url::"];

/// What `adoc-audit` must never name, so the core audit stays offline and pure: no files of its
/// own (it reads only through the caller's reader), no subprocesses, no network, no threads.
/// Optional providers that need any of these run outside it (`adoc_audit::PROVIDERS`).
const ADOC_AUDIT_FORBIDDEN_PATHS: &[&str] = &[
    "std::fs",
    "File::open",
    "File::create",
    "OpenOptions",
    "std::process",
    "Command::new",
    "std::net",
    "TcpStream",
    "UdpSocket",
    "std::thread",
];

/// What `adoc-kindle`, and `adoc-ebook`, whose book model it writes, must never name: every
/// stage of a Kindle export runs in process, with no converter, reader or other program and no
/// network (Kindle PRD §7).
const ADOC_KINDLE_FORBIDDEN_PATHS: &[&str] = &[
    "std::process",
    "Command::new",
    "std::net",
    "TcpStream",
    "UdpSocket",
];

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

    if is_front_end(name) {
        for dependency in &manifest.dependencies {
            if ENGINE_ONLY.contains(&dependency.as_str()) {
                violations.push(format!(
                    "{name} depends on {dependency}; front ends reach it only through adoc-engine"
                ));
            }
        }
    }

    for dependency in &manifest.dependencies {
        if PARSER_CRATES.contains(&dependency.as_str()) && name != "adoc-dom" {
            violations.push(format!(
                "{name} depends on parser crate {dependency}; only adoc-dom may"
            ));
        }
    }

    for dependency in &manifest.dependencies {
        if SPELL_CRATES.contains(&dependency.as_str()) && name != "adoc-spell" {
            violations.push(format!(
                "{name} depends on spell checker {dependency}; only adoc-spell may"
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
    let forbidden_paths = match name {
        "adoc-lang" => ADOC_LANG_FORBIDDEN_PATHS,
        // The impact rules are pure as the audit is: no files, processes, network or threads.
        "adoc-audit" | "adoc-impact" => ADOC_AUDIT_FORBIDDEN_PATHS,
        // Kindle export runs in process: no process and no network (Kindle PRD §7), its shared
        // book model included.
        "adoc-kindle" | "adoc-ebook" => ADOC_KINDLE_FORBIDDEN_PATHS,
        // So does EPUB export (EPUB PRD §7).
        "adoc-epub" => ADOC_KINDLE_FORBIDDEN_PATHS,
        // DocBook export is held as the audit is (DocBook PRD §7): no process and no network,
        // and no files or threads of its own; the engine writes what it hands back.
        "adoc-docbook" => ADOC_AUDIT_FORBIDDEN_PATHS,
        _ => return Vec::new(),
    };
    let mut violations = Vec::new();
    for (path, text) in files {
        for forbidden in forbidden_paths {
            if text.contains(forbidden) {
                violations.push(format!(
                    "{}: {name} must not reference `{forbidden}`",
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
const LEAN_EXCLUDED: &[&str] = &[
    "adoc-pdf",
    "adoc-math",
    "adoc-git",
    "adoc-ebook",
    "adoc-epub",
    "adoc-docbook",
    "adoc-kindle",
    "typst",
    "gix",
];

/// The packages `cargo build -p <package>` compiles with its default features, from
/// `cargo tree`; `cargo metadata` resolves features for the whole workspace at once.
fn default_build_packages(root: &Path, package: &str) -> Result<BTreeSet<String>, String> {
    build_packages(root, package, &[])
}

/// The packages the engine compiles with DOCX and without Typst: `--no-default-features
/// --features docx`.
fn docx_engine_packages(root: &Path) -> Result<BTreeSet<String>, String> {
    build_packages(
        root,
        "adoc-engine",
        &["--no-default-features", "--features", "docx"],
    )
}

/// The packages `cargo build -p <package> <features>` compiles.
fn build_packages(
    root: &Path,
    package: &str,
    features: &[&str],
) -> Result<BTreeSet<String>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["tree", "--locked", "-p", package, "-e", "normal,build"])
        .args(features)
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

/// What DOCX export must never compile, in `adoc-docx` or in the engine built with `docx` and
/// without `typst`: Word documents are written from the view tree, never through the PDF
/// exporter or Typst.
const DOCX_EXCLUDED: &[&str] = &["adoc-pdf", "typst"];

/// A violation for each crate of [`DOCX_EXCLUDED`] in the DOCX writer's build.
fn docx_violations(packages: &BTreeSet<String>) -> Vec<String> {
    DOCX_EXCLUDED
        .iter()
        .filter(|name| packages.contains(**name))
        .map(|name| format!("DOCX export builds {name}; it needs neither PDF nor Typst"))
        .collect()
}

/// The packages the engine compiles with Kindle export alone: `--no-default-features
/// --features kindle`.
fn kindle_engine_packages(root: &Path) -> Result<BTreeSet<String>, String> {
    build_packages(
        root,
        "adoc-engine",
        &["--no-default-features", "--features", "kindle"],
    )
}

/// The packages the engine compiles with EPUB export alone: `--no-default-features --features
/// epub`.
fn epub_engine_packages(root: &Path) -> Result<BTreeSet<String>, String> {
    build_packages(
        root,
        "adoc-engine",
        &["--no-default-features", "--features", "epub"],
    )
}

/// The packages the engine compiles with DocBook export alone: `--no-default-features
/// --features docbook`.
fn docbook_engine_packages(root: &Path) -> Result<BTreeSet<String>, String> {
    build_packages(
        root,
        "adoc-engine",
        &["--no-default-features", "--features", "docbook"],
    )
}

/// What DocBook export must never compile, in `adoc-docbook` or in the engine built with
/// `docbook` alone: the document is written from the view trees, never through another
/// format's exporter or Typst (DocBook PRD §8).
const DOCBOOK_EXCLUDED: &[&str] = &[
    "adoc-pdf",
    "adoc-docx",
    "adoc-ebook",
    "adoc-epub",
    "adoc-kindle",
    "adoc-math",
    "typst",
];

/// A violation for each crate of [`DOCBOOK_EXCLUDED`] in a DocBook build.
fn docbook_violations(packages: &BTreeSet<String>) -> Vec<String> {
    DOCBOOK_EXCLUDED
        .iter()
        .filter(|name| packages.contains(**name))
        .map(|name| format!("DocBook export builds {name}; it needs no other exporter or Typst"))
        .collect()
}

/// What EPUB export must never compile, in `adoc-epub` or in the engine built with `epub`
/// alone: books are written through the shared book model, never through another format's
/// exporter or Typst (EPUB PRD §8).
const EPUB_EXCLUDED: &[&str] = &["adoc-pdf", "adoc-docx", "adoc-kindle", "adoc-math", "typst"];

/// A violation for each crate of [`EPUB_EXCLUDED`] in an EPUB build.
fn epub_violations(packages: &BTreeSet<String>) -> Vec<String> {
    EPUB_EXCLUDED
        .iter()
        .filter(|name| packages.contains(**name))
        .map(|name| format!("EPUB export builds {name}; it needs no other exporter or Typst"))
        .collect()
}

/// What Kindle export must never compile, in `adoc-kindle` or in the engine built with `kindle`
/// alone: books are written from the view tree, never through another format's exporter or
/// Typst (Kindle PRD §9).
const KINDLE_EXCLUDED: &[&str] = &["adoc-pdf", "adoc-docx", "adoc-math", "typst"];

/// A violation for each crate of [`KINDLE_EXCLUDED`] in a Kindle build.
fn kindle_violations(packages: &BTreeSet<String>) -> Vec<String> {
    KINDLE_EXCLUDED
        .iter()
        .filter(|name| packages.contains(**name))
        .map(|name| format!("Kindle export builds {name}; it needs no other exporter or Typst"))
        .collect()
}

/// Each package Kindle export builds, with its licence expression, from `cargo tree`.
fn kindle_licences(root: &Path) -> Result<Vec<(String, String)>, String> {
    licences(root, "adoc-kindle")
}

/// Each package `package` builds, with its licence expression, from `cargo tree`.
fn licences(root: &Path, package: &str) -> Result<Vec<(String, String)>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(root)
        // Every platform's dependencies, whichever the check runs on.
        .args(["tree", "--locked", "--target", "all", "-p", package])
        .args([
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}|{l}",
        ])
        .output()
        .map_err(|error| format!("cargo tree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut licences: Vec<(String, String)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (package, licence) = line.split_once('|')?;
            let name = package.split_whitespace().next()?;
            // `(*)` marks a package `cargo tree` has shown already.
            let licence = licence.trim().trim_end_matches("(*)").trim();
            Some((name.to_owned(), licence.to_owned()))
        })
        .collect();
    licences.sort();
    licences.dedup();
    Ok(licences)
}

/// Whether a licence expression may be used without copyleft: some `OR` alternative whose
/// every `AND` part is permissive. None declared is not.
fn permissive(licence: &str) -> bool {
    let copyleft = |part: &str| {
        let upper = part.to_ascii_uppercase();
        upper.contains("GPL") || upper.contains("EUPL") || upper.contains("CDDL")
    };
    let expression = licence.replace(['(', ')'], " ");
    !expression.trim().is_empty()
        && expression.split(" OR ").any(|alternative| {
            alternative
                .split(" AND ")
                .all(|part| !part.trim().is_empty() && !copyleft(part))
        })
}

/// A violation for each package of Kindle export that may be used only under a copyleft
/// licence, or declares none (Kindle PRD §7: no GPL or other copyleft dependency, `boko` among
/// them).
fn kindle_licence_violations(licences: &[(String, String)]) -> Vec<String> {
    licences
        .iter()
        .filter(|(_, licence)| !permissive(licence))
        .map(|(name, licence)| {
            format!("Kindle export builds {name}, licensed `{licence}`; no copyleft is allowed")
        })
        .collect()
}

/// A violation for each package of EPUB export that may be used only under a copyleft licence,
/// or declares none (EPUB PRD §7, as Kindle's).
fn epub_licence_violations(licences: &[(String, String)]) -> Vec<String> {
    licences
        .iter()
        .filter(|(_, licence)| !permissive(licence))
        .map(|(name, licence)| {
            format!("EPUB export builds {name}, licensed `{licence}`; no copyleft is allowed")
        })
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
    violations.extend(docx_violations(&default_build_packages(root, "adoc-docx")?));
    violations.extend(docx_violations(&docx_engine_packages(root)?));
    violations.extend(kindle_violations(&default_build_packages(
        root,
        "adoc-kindle",
    )?));
    violations.extend(kindle_violations(&kindle_engine_packages(root)?));
    violations.extend(epub_violations(&default_build_packages(root, "adoc-epub")?));
    violations.extend(epub_violations(&epub_engine_packages(root)?));
    violations.extend(docbook_violations(&default_build_packages(
        root,
        "adoc-docbook",
    )?));
    violations.extend(docbook_violations(&docbook_engine_packages(root)?));
    violations.extend(kindle_licence_violations(&kindle_licences(root)?));
    violations.extend(epub_licence_violations(&licences(root, "adoc-epub")?));
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
    fn front_ends_reach_engine_only_crates_through_the_engine() {
        assert!(check_manifest(&manifest("adoc-engine", &["adoc-audit"])).is_empty());
        for front_end in ["adocfoundry", "adocfoundry-cli", "adoc-ls", "adoc-mcp"] {
            assert_eq!(
                check_manifest(&manifest(front_end, &["adoc-engine", "adoc-audit"])),
                [format!(
                    "{front_end} depends on adoc-audit; front ends reach it only through adoc-engine"
                )]
            );
        }
    }

    #[test]
    fn confines_the_spell_checker() {
        assert!(check_manifest(&manifest("adoc-spell", &["spellbook"])).is_empty());
        assert_eq!(
            check_manifest(&manifest("adoc-engine", &["spellbook"])),
            ["adoc-engine depends on spell checker spellbook; only adoc-spell may"]
        );
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

    /// The core audit names no file, process, network or thread API, in a synthetic crate and
    /// in `adoc-audit` itself.
    #[test]
    fn audit_is_offline() {
        let mut files = BTreeMap::new();
        files.insert(
            PathBuf::from("a.rs"),
            "std::process::Command::new(\"vale\")".to_owned(),
        );
        files.insert(
            PathBuf::from("b.rs"),
            "let s = std::net::TcpStream::connect(a);".to_owned(),
        );
        files.insert(
            PathBuf::from("c.rs"),
            "let t = std::fs::read(p);".to_owned(),
        );
        files.insert(PathBuf::from("e.rs"), "let f = File::create(p);".to_owned());
        files.insert(PathBuf::from("d.rs"), "fn fine() {}".to_owned());
        let violations = check_sources("adoc-audit", &files);
        assert_eq!(violations.len(), 6, "{violations:#?}");
        assert!(violations[0].contains("adoc-audit must not reference `std::process`"));

        let mut sources = BTreeMap::new();
        collect_sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/adoc-audit/src"),
            &mut sources,
        );
        assert!(sources.len() > 5, "the crate's sources are read");
        assert_eq!(check_sources("adoc-audit", &sources), Vec::<String>::new());
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

    /// The DOCX export PRD, §10 test 3: the DOCX writer compiles neither the PDF exporter nor
    /// Typst.
    #[test]
    fn docx_builds_without_typst() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let packages = default_build_packages(&root, "adoc-docx").unwrap();
        assert!(packages.contains("adoc-render"), "{packages:?}");
        assert_eq!(docx_violations(&packages), Vec::<String>::new());

        // The engine with DOCX and without Typst.
        let engine = docx_engine_packages(&root).unwrap();
        assert!(engine.contains("adoc-docx"), "{engine:?}");
        assert_eq!(docx_violations(&engine), Vec::<String>::new());

        let with_pdf: BTreeSet<String> = ["adoc-render", "adoc-pdf", "typst"]
            .map(str::to_owned)
            .into();
        assert_eq!(
            docx_violations(&with_pdf),
            [
                "DOCX export builds adoc-pdf; it needs neither PDF nor Typst",
                "DOCX export builds typst; it needs neither PDF nor Typst",
            ]
        );
    }

    /// Kindle PRD §12 test 2: Kindle export alone, in `adoc-kindle` and in the engine built
    /// with `kindle` and no other format, compiles neither Typst nor another exporter.
    #[test]
    fn kindle_builds_alone() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let packages = default_build_packages(&root, "adoc-kindle").unwrap();
        assert!(packages.contains("adoc-render"), "{packages:?}");
        assert_eq!(kindle_violations(&packages), Vec::<String>::new());

        let engine = kindle_engine_packages(&root).unwrap();
        assert!(engine.contains("adoc-kindle"), "{engine:?}");
        assert_eq!(kindle_violations(&engine), Vec::<String>::new());

        let with_others: BTreeSet<String> = ["adoc-kindle", "adoc-docx", "typst"]
            .map(str::to_owned)
            .into();
        assert_eq!(
            kindle_violations(&with_others),
            [
                "Kindle export builds adoc-docx; it needs no other exporter or Typst",
                "Kindle export builds typst; it needs no other exporter or Typst",
            ]
        );
        // And its sources name no process or network.
        let mut sources = BTreeMap::new();
        sources.insert(
            PathBuf::from("x.rs"),
            "std::process::Command::new(\"x\")".to_owned(),
        );
        assert_eq!(check_sources("adoc-kindle", &sources).len(), 2);
    }

    /// EPUB PRD §11 test 5: EPUB export alone, in `adoc-epub` and in the engine built with
    /// `epub` and no other format, compiles neither Typst nor another exporter.
    #[test]
    fn epub_builds_alone() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let packages = default_build_packages(&root, "adoc-epub").unwrap();
        assert!(packages.contains("adoc-ebook"), "{packages:?}");
        assert_eq!(epub_violations(&packages), Vec::<String>::new());

        let engine = epub_engine_packages(&root).unwrap();
        assert!(engine.contains("adoc-epub"), "{engine:?}");
        assert_eq!(epub_violations(&engine), Vec::<String>::new());

        let with_others: BTreeSet<String> = ["adoc-epub", "adoc-kindle", "typst"]
            .map(str::to_owned)
            .into();
        assert_eq!(
            epub_violations(&with_others),
            [
                "EPUB export builds adoc-kindle; it needs no other exporter or Typst",
                "EPUB export builds typst; it needs no other exporter or Typst",
            ]
        );
    }

    /// Kindle PRD §12 test 19: `adoc-kindle`, and `adoc-ebook`, whose book model it writes,
    /// name no process and no network path, and nothing Kindle export builds is under a
    /// copyleft licence (`cargo deny` checks the rest of the tree).
    #[test]
    fn kindle_is_in_process() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for (name, least) in [("adoc-kindle", 5), ("adoc-ebook", 4)] {
            let mut sources = BTreeMap::new();
            collect_sources(&root.join("crates").join(name).join("src"), &mut sources);
            assert!(sources.len() > least, "{name}: {}", sources.len());
            assert_eq!(check_sources(name, &sources), Vec::<String>::new());
        }

        let licences = kindle_licences(&root).unwrap();
        assert!(
            licences.iter().any(|(name, _)| name == "resvg"),
            "{licences:?}"
        );
        assert_eq!(kindle_licence_violations(&licences), Vec::<String>::new());
        let copyleft = [
            ("boko".to_owned(), "GPL-3.0-or-later".to_owned()),
            ("both".to_owned(), "MIT AND LGPL-2.1".to_owned()),
            ("unknown".to_owned(), String::new()),
            // A choice that may be taken without copyleft is allowed.
            ("either".to_owned(), "Apache-2.0 OR GPL-2.0-only".to_owned()),
        ];
        assert_eq!(kindle_licence_violations(&copyleft).len(), 3);
    }

    /// EPUB PRD §11 test 22: `adoc-epub`, and `adoc-ebook`, whose book model it writes, name
    /// no process and no network path, and nothing EPUB export builds is under a copyleft
    /// licence (`cargo deny` checks the rest of the tree).
    #[test]
    fn epub_is_in_process() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for (name, least) in [("adoc-epub", 5), ("adoc-ebook", 4)] {
            let mut sources = BTreeMap::new();
            collect_sources(&root.join("crates").join(name).join("src"), &mut sources);
            assert!(sources.len() > least, "{name}: {}", sources.len());
            assert_eq!(check_sources(name, &sources), Vec::<String>::new());
        }
        // A source naming a process is caught.
        let mut sources = BTreeMap::new();
        sources.insert(
            PathBuf::from("x.rs"),
            "std::process::Command::new(\"epubcheck\")".to_owned(),
        );
        assert_eq!(check_sources("adoc-epub", &sources).len(), 2);

        let licences = licences(&root, "adoc-epub").unwrap();
        assert!(
            licences.iter().any(|(name, _)| name == "roxmltree"),
            "{licences:?}"
        );
        assert_eq!(epub_licence_violations(&licences), Vec::<String>::new());
        let copyleft = [("boko".to_owned(), "GPL-3.0-or-later".to_owned())];
        assert_eq!(epub_licence_violations(&copyleft).len(), 1);
    }

    /// DocBook PRD §11 test 5: DocBook export alone, in `adoc-docbook` and in the engine built
    /// with `docbook` and no other format, compiles neither Typst nor another exporter.
    #[test]
    fn docbook_builds_alone() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let packages = default_build_packages(&root, "adoc-docbook").unwrap();
        assert!(packages.contains("adoc-render"), "{packages:?}");
        assert!(
            !packages.contains("oxixml-relaxng"),
            "the schema validator is for tests only: {packages:?}"
        );
        assert_eq!(docbook_violations(&packages), Vec::<String>::new());

        let engine = docbook_engine_packages(&root).unwrap();
        assert!(engine.contains("adoc-docbook"), "{engine:?}");
        assert_eq!(docbook_violations(&engine), Vec::<String>::new());

        let with_others: BTreeSet<String> = ["adoc-docbook", "adoc-epub", "typst"]
            .map(str::to_owned)
            .into();
        assert_eq!(
            docbook_violations(&with_others),
            [
                "DocBook export builds adoc-epub; it needs no other exporter or Typst",
                "DocBook export builds typst; it needs no other exporter or Typst",
            ]
        );
    }

    /// The Word looks PRD, §9 test 1: themes load without Typst, so Word export can read them
    /// in a build without PDF export; PDF export reads them through the same crate.
    #[test]
    fn theme_crate_builds_without_typst() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let packages = default_build_packages(&root, "adoc-theme").unwrap();
        assert!(packages.contains("adoc-theme"), "{packages:?}");
        assert_eq!(docx_violations(&packages), Vec::<String>::new());
        let pdf = default_build_packages(&root, "adoc-pdf").unwrap();
        assert!(pdf.contains("adoc-theme"), "{pdf:?}");
    }

    #[test]
    fn current_workspace_passes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(run(&root).unwrap(), Vec::<String>::new());
    }

    /// Whether `text` names `name` as a word of its own: `adocfoundry` is not named by
    /// `adocfoundry-cli`.
    fn names(text: &str, name: &str) -> bool {
        let part_of_name = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        text.match_indices(name).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + name.len()..].chars().next();
            !before.is_some_and(part_of_name) && !after.is_some_and(part_of_name)
        })
    }

    #[test]
    fn names_a_crate_only_as_a_word() {
        assert!(names("`adocfoundry` (the app)", "adocfoundry"));
        assert!(names("crates/adocfoundry/", "adocfoundry"));
        assert!(!names("`adocfoundry-cli`", "adocfoundry"));
        assert!(!names("adoc-lang-server", "adoc-lang"));
    }

    /// Issue #197: the guides other than the project's own instructions, and the developer
    /// documentation's crate table and diagram, describe the real ladder, so every crate in
    /// `LAYERS` is named in each.
    #[test]
    fn every_crate_is_documented() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut missing = Vec::new();
        for file in [
            "AGENTS.md",
            "README.md",
            "docs/design/DESIGN-AdocFoundry_Architecture.md",
            "src/docs/modules/developer/pages/architecture.adoc",
            "src/docs/modules/developer/examples/crate-layers.mmd",
        ] {
            let text = fs::read_to_string(root.join(file)).unwrap();
            for (name, _) in LAYERS {
                if !names(&text, name) {
                    missing.push(format!("{file} does not name {name}"));
                }
            }
        }
        assert_eq!(missing, Vec::<String>::new());
    }

    /// Issue #589: the documentation's `crates` module has a page for every crate in
    /// `LAYERS`, named after it and listed in the module's navigation, and the component
    /// lists that navigation.
    #[test]
    fn every_crate_has_a_page() {
        let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/docs");
        let mut missing = Vec::new();
        let component = fs::read_to_string(docs.join("antora.yml")).unwrap();
        if !component.contains("modules/crates/nav.adoc") {
            missing.push("antora.yml does not list modules/crates/nav.adoc".to_owned());
        }
        let nav = fs::read_to_string(docs.join("modules/crates/nav.adoc")).unwrap_or_default();
        for (name, _) in LAYERS {
            let page = format!("{name}.adoc");
            if !docs.join("modules/crates/pages").join(&page).is_file() {
                missing.push(format!("no page crates/pages/{page}"));
            }
            if !nav.contains(&format!("xref:crates:{page}[")) {
                missing.push(format!("crates/nav.adoc does not list {page}"));
            }
        }
        assert_eq!(missing, Vec::<String>::new());
    }
}
