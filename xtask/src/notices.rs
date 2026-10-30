//! Third-party notices: every component the release binaries are built from, each with its
//! version, licence and licence texts, for Help › About and the notices file the packages ship.
//!
//! The crates are those of the release build's resolved graph on every platform, as `cargo deny`
//! sees it, read from `cargo metadata`. Their texts are the licence files each one ships, else
//! the standard text of its licence (`xtask/licences/`). The bundled files that are not crates
//! are [`ASSETS`]. Nothing else is kept by hand, and a component without a text is an error.
//!
//! The app's build script includes this module by path and embeds [`Notices::to_json`];
//! `cargo xtask notices` writes [`Notices::to_text`].

use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsStr,
    fs,
    path::Path,
    process::Command,
};

use serde_json::{json, Value};

/// The release binaries, whose dependencies the notices cover.
pub const BINARIES: &[&str] = &["adocfoundry", "adocfoundry-cli", "adoc-ls", "adoc-mcp"];

/// The features the release builds turn on beyond each crate's defaults.
pub const FEATURES: &str = "adoc-ls/typst,adoc-ls/git";

/// Files built into the binaries that do not come from a crate.
pub struct Asset {
    pub name: &'static str,
    pub version: &'static str,
    pub licence: &'static str,
    /// The folder holding it, from the repository root.
    pub folder: &'static str,
    /// How the names of its files in `folder` begin.
    pub files: &'static [&'static str],
    /// Its licence and notice files in `folder`.
    pub notices: &'static [&'static str],
}

pub const ASSETS: &[Asset] = &[
    Asset {
        name: "SCOWL English dictionaries (en_GB, en_US)",
        version: "2026.02.25",
        licence: "SCOWL AND BSD (Ispell)",
        folder: "crates/adoc-spell/dictionaries",
        files: &["en_GB.", "en_US."],
        notices: &["README.md"],
    },
    Asset {
        name: "Noto Sans (subset)",
        version: "20161022",
        licence: "Apache-2.0",
        folder: "crates/adoc-pdf/fonts",
        files: &["notosans-"],
        notices: &["LICENSE-noto", "ABOUT-notosans-subset"],
    },
    Asset {
        name: "Noto Serif (subset)",
        version: "20161022",
        licence: "Apache-2.0",
        folder: "crates/adoc-pdf/fonts",
        files: &["notoserif-"],
        notices: &["LICENSE-noto", "ABOUT-notoserif-subset"],
    },
    Asset {
        name: "M+ 1mn (subset)",
        version: "TESTFLIGHT 63a",
        licence: "mplus",
        folder: "crates/adoc-pdf/fonts",
        files: &["mplus1mn-"],
        notices: &["LICENSE-mplus", "ABOUT-mplus1mn-subset"],
    },
    Asset {
        name: "STIX Two Math",
        version: "2.13",
        licence: "OFL-1.1",
        folder: "crates/adoc-pdf/fonts",
        files: &["STIXTwoMath-"],
        notices: &["LICENSE-stix", "ABOUT-stixtwomath"],
    },
    Asset {
        name: "Asciidoctor PDF themes",
        version: "2.3",
        licence: "MIT",
        folder: "crates/adoc-pdf/themes",
        files: &["base-theme", "default-"],
        notices: &["LICENSE-asciidoctor-pdf"],
    },
];

/// The standard texts of the licences of crates that ship no licence file, by SPDX identifier.
const STANDARD_TEXTS: &[(&str, &str)] = &[
    ("0BSD", include_str!("../licences/0BSD.txt")),
    ("Apache-2.0", include_str!("../licences/Apache-2.0.txt")),
    ("BSD-3-Clause", include_str!("../licences/BSD-3-Clause.txt")),
    ("CC0-1.0", include_str!("../licences/CC0-1.0.txt")),
    ("MIT", include_str!("../licences/MIT.txt")),
    ("MPL-2.0", include_str!("../licences/MPL-2.0.txt")),
    ("Zlib", include_str!("../licences/Zlib.txt")),
];

/// Every component, sorted by name, whatever its case, and version, and the licence texts they share.
#[derive(Debug, Default, PartialEq)]
pub struct Notices {
    pub components: Vec<Component>,
    pub texts: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct Component {
    pub name: String,
    pub version: String,
    pub licence: String,
    /// Why the texts are its licence's standard text, when it ships no licence file.
    pub note: Option<String>,
    /// Each licence file's name, and its text as an index into [`Notices::texts`].
    pub texts: Vec<(String, usize)>,
}

/// The notices of the release build of the workspace at `root`, asking `cargo` for its graph.
pub fn collect(root: &Path, cargo: &OsStr) -> Result<Notices, String> {
    // No --offline: metadata needs every package of the graph, other platforms' too.
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked"])
        .args(["--features", FEATURES])
        .output()
        .map_err(|error| format!("cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata: {error}"))?;
    from_metadata(&metadata, root)
}

/// The notices of the crates `metadata` resolves for [`BINARIES`], and of [`ASSETS`] under
/// `root`.
pub fn from_metadata(metadata: &Value, root: &Path) -> Result<Notices, String> {
    let mut notices = Notices::default();
    let mut missing = Vec::new();
    for package in dependencies(metadata)? {
        let field = |key: &str| package[key].as_str().unwrap_or_default().to_owned();
        let (name, version, licence) = (field("name"), field("version"), field("license"));
        let folder = Path::new(package["manifest_path"].as_str().unwrap_or_default())
            .parent()
            .unwrap_or(Path::new(""));
        let mut files = licence_files(folder);
        // A path dependency may keep its licence in the repository above it, as the grammar
        // does.
        if files.is_empty() && package["source"].is_null() {
            files = folder.parent().map(licence_files).unwrap_or_default();
        }
        let mut note = None;
        if files.is_empty() {
            if let Some((id, text)) = standard_text(&licence) {
                let authors: Vec<&str> = package["authors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let holders = if authors.is_empty() {
                    format!("the authors of {name}")
                } else {
                    authors.join(", ")
                };
                note = Some(format!(
                    "{name} ships no licence file. This is the standard text of {id}, its \
                     licence; the copyright is held by {holders}."
                ));
                files.push((id.to_owned(), text.to_owned()));
            } else {
                missing.push(format!("{name} {version} ({licence})"));
                continue;
            }
        }
        let texts = files
            .into_iter()
            .map(|(file, text)| (file, notices.intern(&text)))
            .collect();
        notices.components.push(Component {
            name,
            version,
            licence,
            note,
            texts,
        });
    }
    for asset in ASSETS {
        let mut texts = Vec::new();
        for file in asset.notices {
            let path = root.join(asset.folder).join(file);
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            texts.push(((*file).to_owned(), notices.intern(&text)));
        }
        notices.components.push(Component {
            name: asset.name.to_owned(),
            version: asset.version.to_owned(),
            licence: asset.licence.to_owned(),
            note: None,
            texts,
        });
    }
    if !missing.is_empty() {
        return Err(format!(
            "no licence file, and no standard text for the licence, of: {}",
            missing.join(", ")
        ));
    }
    notices
        .components
        .sort_by_cached_key(|component| (component.name.to_lowercase(), component.version.clone()));
    Ok(notices)
}

/// The packages outside the workspace that [`BINARIES`] reach through normal and build
/// dependencies, on any platform.
fn dependencies(metadata: &Value) -> Result<Vec<&Value>, String> {
    let malformed = || "cargo metadata: unexpected output".to_owned();
    let packages: HashMap<&str, &Value> = metadata["packages"]
        .as_array()
        .ok_or_else(malformed)?
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect();
    let nodes: HashMap<&str, &Value> = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or_else(malformed)?
        .iter()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .ok_or_else(malformed)?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut queue: Vec<&str> = members
        .iter()
        .copied()
        .filter(|id| {
            packages
                .get(id)
                .and_then(|package| package["name"].as_str())
                .is_some_and(|name| BINARIES.contains(&name))
        })
        .collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        let deps = nodes.get(id).and_then(|node| node["deps"].as_array());
        for dep in deps.into_iter().flatten() {
            let built = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|kind| kind["kind"].is_null() || kind["kind"].as_str() == Some("build"));
            if let Some(pkg) = dep["pkg"].as_str().filter(|_| built) {
                queue.push(pkg);
            }
        }
    }
    Ok(seen
        .into_iter()
        .filter(|id| !members.contains(id))
        .filter_map(|id| packages.get(id).copied())
        .collect())
}

/// The licence and notice files in `folder`, by name, with their texts.
fn licence_files(folder: &Path) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            let licence = lower.contains("licen")
                || ["copying", "copyright", "notice"]
                    .iter()
                    .any(|start| lower.starts_with(start));
            if !licence || lower.ends_with(".rs") {
                return None;
            }
            let text = fs::read(entry.path()).ok()?;
            Some((name, String::from_utf8_lossy(&text).into_owned()))
        })
        .collect();
    files.sort();
    files
}

/// The first of the alternatives of `licence` with a standard text. Only a plain choice of
/// licences (`MIT OR Apache-2.0`, `MIT/Apache-2.0`) is read; a combination needs its files.
fn standard_text(licence: &str) -> Option<(&'static str, &'static str)> {
    let expression = licence.replace('/', " OR ");
    if expression.contains(" AND ") || expression.contains('(') {
        return None;
    }
    expression.split(" OR ").map(str::trim).find_map(|id| {
        STANDARD_TEXTS
            .iter()
            .find(|(standard, _)| *standard == id)
            .copied()
    })
}

impl Notices {
    /// The index of `text` in [`Self::texts`], added if it is new. Line endings and trailing
    /// space are normalised so that copies of one text differ in nothing else.
    fn intern(&mut self, text: &str) -> usize {
        let text = text.replace("\r\n", "\n").trim_end().to_owned();
        if let Some(index) = self.texts.iter().position(|known| *known == text) {
            return index;
        }
        self.texts.push(text);
        self.texts.len() - 1
    }

    /// The notices as the app reads them: `components`, each with its `texts` as pairs of a
    /// file name and an index into `texts`.
    pub fn to_json(&self) -> String {
        let components: Vec<Value> = self
            .components
            .iter()
            .map(|component| {
                json!({
                    "name": component.name,
                    "version": component.version,
                    "licence": component.licence,
                    "note": component.note,
                    "texts": component.texts,
                })
            })
            .collect();
        json!({ "components": components, "texts": self.texts }).to_string()
    }

    /// The notices file the packages ship: the components with their licences and the numbers
    /// of their texts, then each text once, numbered.
    pub fn to_text(&self) -> String {
        let mut text = String::from(
            "Third-party notices for AdocFoundry\n\
             \n\
             AdocFoundry is built from the components below. Each is listed with its version,\n\
             its licence and the numbers of its licence texts, which follow the list.\n\n",
        );
        for component in &self.components {
            let numbers: Vec<String> = component
                .texts
                .iter()
                .map(|(_, index)| (index + 1).to_string())
                .collect();
            text.push_str(&format!(
                "{} {}: {} [{}]\n",
                component.name,
                component.version,
                component.licence,
                numbers.join(", ")
            ));
            if let Some(note) = &component.note {
                text.push_str(&format!("    {note}\n"));
            }
        }
        for (index, body) in self.texts.iter().enumerate() {
            let file = self
                .components
                .iter()
                .flat_map(|component| &component.texts)
                .find(|(_, text)| *text == index)
                .map_or("", |(file, _)| file.as_str());
            text.push_str(&format!(
                "\n{}\n[{}] {file}\n\n{body}\n",
                "=".repeat(80),
                index + 1
            ));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    /// Metadata in which the binary `adocfoundry` depends on each of `crates`, given as a
    /// folder, a licence and its dependency kind.
    fn metadata(crates: &[(&Path, &str, Option<&str>)]) -> Value {
        let mut packages = vec![json!({
            "id": "app", "name": "adocfoundry", "version": "0.0.0", "license": "Apache-2.0",
            "manifest_path": "/app/Cargo.toml", "source": null, "authors": [],
        })];
        let mut deps = Vec::new();
        for (index, (folder, licence, kind)) in crates.iter().enumerate() {
            let id = format!("dep{index}");
            packages.push(json!({
                "id": id, "name": format!("dep{index}"), "version": "1.0.0", "license": licence,
                "manifest_path": folder.join("Cargo.toml"),
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "authors": ["A. Author <a@example.com>"],
            }));
            deps.push(json!({ "pkg": id, "dep_kinds": [{ "kind": kind, "target": null }] }));
        }
        json!({
            "packages": packages,
            "workspace_members": ["app"],
            "resolve": { "nodes": [{ "id": "app", "deps": deps }] },
        })
    }

    #[test]
    fn crates_carry_the_licence_files_they_ship() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join("LICENSE-MIT"), "MIT text\r\n").unwrap();
        fs::write(folder.path().join("LICENSE-APACHE"), "Apache text\n").unwrap();
        fs::write(folder.path().join("license.rs"), "fn main() {}").unwrap();
        let notices = from_metadata(
            &metadata(&[(folder.path(), "MIT OR Apache-2.0", None)]),
            &root(),
        )
        .unwrap();
        let dep = &notices.components[notices
            .components
            .iter()
            .position(|component| component.name == "dep0")
            .unwrap()];
        let files: Vec<(&str, &str)> = dep
            .texts
            .iter()
            .map(|(file, index)| (file.as_str(), notices.texts[*index].as_str()))
            .collect();
        assert_eq!(
            files,
            [
                ("LICENSE-APACHE", "Apache text"),
                ("LICENSE-MIT", "MIT text")
            ]
        );
        assert_eq!(dep.note, None);
    }

    #[test]
    fn a_crate_without_a_licence_file_gets_its_licences_standard_text() {
        let folder = tempfile::tempdir().unwrap();
        let notices = from_metadata(
            &metadata(&[(folder.path(), "Apache-2.0 WITH LLVM-exception/MIT", None)]),
            &root(),
        )
        .unwrap();
        let dep = notices
            .components
            .iter()
            .find(|component| component.name == "dep0")
            .unwrap();
        assert_eq!(dep.texts.len(), 1);
        assert_eq!(dep.texts[0].0, "MIT");
        assert!(notices.texts[dep.texts[0].1].starts_with("Permission is hereby granted"));
        assert!(dep.note.as_ref().unwrap().contains("A. Author"));
    }

    #[test]
    fn a_crate_with_no_text_to_be_found_is_an_error() {
        let folder = tempfile::tempdir().unwrap();
        let error = from_metadata(
            &metadata(&[(folder.path(), "MIT AND BSD-2-Clause", None)]),
            &root(),
        )
        .unwrap_err();
        assert!(
            error.contains("dep0 1.0.0 (MIT AND BSD-2-Clause)"),
            "{error}"
        );
    }

    #[test]
    fn build_dependencies_count_and_dev_dependencies_do_not() {
        let built = tempfile::tempdir().unwrap();
        let tested = tempfile::tempdir().unwrap();
        let notices = from_metadata(
            &metadata(&[
                (built.path(), "MIT", Some("build")),
                (tested.path(), "MIT", Some("dev")),
            ]),
            &root(),
        )
        .unwrap();
        let names: Vec<&str> = notices
            .components
            .iter()
            .map(|component| component.name.as_str())
            .collect();
        assert!(names.contains(&"dep0"), "{names:?}");
        assert!(!names.contains(&"dep1"), "{names:?}");
    }

    #[test]
    fn the_text_numbers_each_shared_text_once() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("LICENSE"), "Same text").unwrap();
        fs::write(b.path().join("COPYING"), "Same text\n").unwrap();
        let notices = from_metadata(
            &metadata(&[(a.path(), "MIT", None), (b.path(), "MIT", None)]),
            &root(),
        )
        .unwrap();
        let text = notices.to_text();
        assert!(text.contains("dep0 1.0.0: MIT [1]\n"), "{text}");
        assert!(text.contains("dep1 1.0.0: MIT [1]\n"), "{text}");
        assert_eq!(text.matches("\nSame text\n").count(), 1);
    }

    #[test]
    fn every_bundled_file_has_its_notice() {
        let folders: BTreeSet<&str> = ASSETS.iter().map(|asset| asset.folder).collect();
        for folder in folders {
            let assets: Vec<&Asset> = ASSETS
                .iter()
                .filter(|asset| asset.folder == folder)
                .collect();
            for entry in fs::read_dir(root().join(folder)).unwrap() {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                let covered = assets.iter().any(|asset| {
                    asset.notices.contains(&name.as_str())
                        || asset.files.iter().any(|start| name.starts_with(start))
                });
                assert!(covered, "{folder}/{name} is in no asset of ASSETS");
            }
        }
    }

    #[test]
    fn every_dependency_of_the_release_build_has_a_notice() {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let notices = collect(&root(), &cargo).unwrap();
        for name in [
            "gpui-pre",
            "gpui-component",
            "typst",
            "acdc-parser",
            "merman",
            "gix",
            "spellbook",
            "evalexpr",
            "comemo",
            "tree-sitter-asciidoc",
            "STIX Two Math",
            "SCOWL English dictionaries (en_GB, en_US)",
        ] {
            assert!(
                notices
                    .components
                    .iter()
                    .any(|component| component.name == name),
                "{name} is not in the notices"
            );
        }
        assert!(notices
            .components
            .iter()
            .all(|component| !component.texts.is_empty()
                && component
                    .texts
                    .iter()
                    .all(|(_, index)| !notices.texts[*index].is_empty())));
        assert!(!notices
            .components
            .iter()
            .any(|component| BINARIES.contains(&component.name.as_str())));
    }
}
