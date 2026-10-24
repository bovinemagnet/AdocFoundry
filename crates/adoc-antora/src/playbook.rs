//! The Antora playbook: its AsciiDoc attributes, which apply to every page of the site, and its
//! content sources, which say which branches of which repositories the site is built from.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{descriptor::RawAsciidoc, DescriptorError, DescriptorErrorKind};

/// The file name a playbook is discovered by at a workspace root.
pub const PLAYBOOK_FILE: &str = "antora-playbook.yml";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Playbook {
    pub path: PathBuf,
    /// `asciidoc.attributes`, read as Antora reads them: `true` sets an empty value, `false`
    /// and `~` unset. A value ending in `@` is soft-set, so a component or page may override it.
    pub asciidoc_attributes: BTreeMap<String, String>,
    /// `content.sources`, in order.
    pub sources: Vec<ContentSource>,
    /// `site.url`: where the site is published, which references out of an exported PDF lead
    /// to.
    pub site_url: Option<String>,
}

/// One of the playbook's content sources: a repository and the branches Antora takes from it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContentSource {
    /// As written: `.` (or a path) for a local repository, else a remote URL.
    pub url: String,
    /// Branch patterns: globs, `!` to exclude, `HEAD` for the checked-out branch. Antora's
    /// default when none are given is `HEAD` and `v{0..9}*`.
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    /// Where the component descriptors are, relative to the repository root; `""` for the root.
    pub start_paths: Vec<String>,
}

impl ContentSource {
    /// Whether the source names a repository on this machine rather than a remote one.
    #[must_use]
    pub fn is_local(&self) -> bool {
        !(self.url.contains("://") || self.url.starts_with("git@"))
    }

    /// Whether Antora takes branch `name` from this source, `current` being the checked-out
    /// branch that `HEAD` names.
    #[must_use]
    pub fn takes_branch(&self, name: &str, current: Option<&str>) -> bool {
        let mut taken = false;
        for pattern in &self.branches {
            let (excluded, pattern) = match pattern.strip_prefix('!') {
                Some(pattern) => (true, pattern),
                None => (false, pattern.as_str()),
            };
            let matched = if pattern == "HEAD" || pattern == "." {
                current == Some(name)
            } else {
                glob_matches(pattern, name)
            };
            if matched {
                taken = !excluded;
            }
        }
        taken
    }
}

/// Which branches publish each component version, as read from each branch's descriptor.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PublishedVersions {
    /// By component and version (`None` for an unversioned component).
    branches: BTreeMap<(String, Option<String>), BTreeSet<String>>,
}

impl PublishedVersions {
    pub fn record(&mut self, component: &str, version: Option<&str>, branch: &str) {
        self.branches
            .entry((component.to_owned(), version.map(str::to_owned)))
            .or_default()
            .insert(branch.to_owned());
    }

    /// The branches publishing `version` of `component`, in name order.
    #[must_use]
    pub fn branches_for_version(&self, component: &str, version: Option<&str>) -> Vec<String> {
        self.branches
            .get(&(component.to_owned(), version.map(str::to_owned)))
            .map(|branches| branches.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.branches.is_empty()
    }
}

/// The branch the workspace is on and the branches that publish each component version, known
/// only when the workspace has both a playbook and a repository.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BranchVersions {
    pub current: String,
    pub published: PublishedVersions,
}

/// Matches a branch name against an Antora (picomatch) glob: `*`, `?`, `{a,b}` and numeric
/// ranges `{0..9}`, which `globset` has no syntax for and are expanded first.
fn glob_matches(pattern: &str, name: &str) -> bool {
    globset::GlobBuilder::new(&expand_ranges(pattern))
        .literal_separator(true)
        .build()
        .is_ok_and(|glob| glob.compile_matcher().is_match(name))
}

/// `v{1..3}*` as `v{1,2,3}*`.
fn expand_ranges(pattern: &str) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}').map(|close| open + close) else {
            break;
        };
        let inner = &rest[open + 1..close];
        let range = inner.split_once("..").and_then(|(from, to)| {
            let (from, to) = (from.parse::<i64>().ok()?, to.parse::<i64>().ok()?);
            (from <= to && to - from <= 1000).then_some((from, to))
        });
        match range {
            Some((from, to)) => {
                let items: Vec<String> = (from..=to).map(|number| number.to_string()).collect();
                out.push('{');
                out.push_str(&items.join(","));
                out.push('}');
            }
            None => out.push_str(&rest[open..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

impl Playbook {
    /// Attributes no component or page may override.
    pub fn hard_attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.asciidoc_attributes
            .iter()
            .filter(|(_, value)| !value.ends_with('@'))
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Soft-set attributes, with the trailing `@` removed.
    pub fn soft_attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.asciidoc_attributes
            .iter()
            .filter_map(|(name, value)| value.strip_suffix('@').map(|value| (name.as_str(), value)))
    }
}

#[derive(Debug, Deserialize)]
struct RawPlaybook {
    site: Option<RawSite>,
    asciidoc: Option<RawAsciidoc>,
    content: Option<RawContent>,
}

#[derive(Debug, Deserialize)]
struct RawSite {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawContent {
    branches: Option<Patterns>,
    tags: Option<Patterns>,
    #[serde(default)]
    sources: Vec<RawSource>,
}

#[derive(Debug, Deserialize)]
struct RawSource {
    url: Option<String>,
    branches: Option<Patterns>,
    tags: Option<Patterns>,
    start_path: Option<String>,
    start_paths: Option<Patterns>,
}

/// A pattern list, written as a list or as one comma-separated string.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum Patterns {
    One(String),
    Many(Vec<String>),
}

impl Patterns {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(text) => text
                .split(',')
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_owned)
                .collect(),
            Self::Many(patterns) => patterns,
        }
    }
}

pub fn parse_playbook(path: &Path, source: &str) -> Result<Playbook, DescriptorError> {
    let raw: RawPlaybook = serde_saphyr::from_str(source).map_err(|error| DescriptorError {
        kind: DescriptorErrorKind::InvalidYaml,
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let asciidoc_attributes = raw
        .asciidoc
        .map(|asciidoc| {
            asciidoc
                .attributes
                .into_iter()
                .filter_map(|(name, value)| Some((name, value.into_attribute_value()?)))
                .collect()
        })
        .unwrap_or_default();
    let sources = raw
        .content
        .map(|content| {
            let default_branches = content.branches.map(Patterns::into_vec);
            let default_tags = content.tags.map(Patterns::into_vec);
            content
                .sources
                .into_iter()
                .map(|source| ContentSource {
                    url: source.url.unwrap_or_else(|| ".".to_owned()),
                    branches: source
                        .branches
                        .map(Patterns::into_vec)
                        .or_else(|| default_branches.clone())
                        .unwrap_or_else(|| vec!["HEAD".to_owned(), "v{0..9}*".to_owned()]),
                    tags: source
                        .tags
                        .map(Patterns::into_vec)
                        .or_else(|| default_tags.clone())
                        .unwrap_or_default(),
                    start_paths: source
                        .start_paths
                        .map(Patterns::into_vec)
                        .or_else(|| source.start_path.map(|path| vec![path]))
                        .unwrap_or_else(|| vec![String::new()]),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Playbook {
        path: path.to_path_buf(),
        asciidoc_attributes,
        sources,
        site_url: raw
            .site
            .and_then(|site| site.url)
            .filter(|url| !url.trim().is_empty()),
    })
}

pub fn read_playbook(path: &Path) -> Result<Playbook, DescriptorError> {
    let source = fs::read_to_string(path).map_err(|error| DescriptorError {
        kind: DescriptorErrorKind::Read,
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    parse_playbook(path, &source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hard_and_soft_attributes() {
        let playbook = parse_playbook(
            Path::new("antora-playbook.yml"),
            "site:\n  title: Docs\nasciidoc:\n  attributes:\n    company: Foundry Ltd\n    release: One@\n    draft: true\n    hidden: false\n",
        )
        .unwrap();
        assert_eq!(
            playbook.hard_attributes().collect::<Vec<_>>(),
            [("company", "Foundry Ltd"), ("draft", "")]
        );
        assert_eq!(
            playbook.soft_attributes().collect::<Vec<_>>(),
            [("release", "One")]
        );
    }

    #[test]
    fn reads_the_site_url() {
        let playbook = parse_playbook(
            Path::new("antora-playbook.yml"),
            "site:\n  title: Docs\n  url: https://docs.example.com/\n",
        )
        .unwrap();
        assert_eq!(
            playbook.site_url.as_deref(),
            Some("https://docs.example.com/")
        );
        let without = parse_playbook(Path::new("p.yml"), "site:\n  title: Docs\n").unwrap();
        assert_eq!(without.site_url, None);
    }

    fn versions_fixture() -> Playbook {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-versions/antora-playbook.yml");
        read_playbook(&path).unwrap()
    }

    #[test]
    fn playbook_parses_sources_branches_and_start_path() {
        let playbook = versions_fixture();
        assert_eq!(playbook.sources.len(), 2);
        let local = &playbook.sources[0];
        assert_eq!(local.url, ".");
        assert!(local.is_local());
        assert_eq!(local.branches, ["main", "v{1..9}*", "!v1.0-draft"]);
        assert_eq!(local.start_paths, ["docs"]);
        let other = &playbook.sources[1];
        assert!(!other.is_local());
        assert_eq!(other.branches, ["HEAD"]);
        assert_eq!(other.start_paths, ["guide", "reference"]);

        // Antora's defaults: the current branch and the version branches, from the root.
        let defaults =
            parse_playbook(Path::new("p.yml"), "content:\n  sources:\n  - url: .\n").unwrap();
        assert_eq!(defaults.sources[0].branches, ["HEAD", "v{0..9}*"]);
        assert_eq!(defaults.sources[0].start_paths, [""]);
        // A comma-separated string lists several patterns.
        let listed = parse_playbook(
            Path::new("p.yml"),
            "content:\n  sources:\n  - url: .\n    branches: main, release/*\n",
        )
        .unwrap();
        assert_eq!(listed.sources[0].branches, ["main", "release/*"]);
    }

    #[test]
    fn branches_for_version_matches_globs() {
        let playbook = versions_fixture();
        let local = &playbook.sources[0];
        assert!(local.takes_branch("main", Some("main")));
        assert!(local.takes_branch("v1.0", Some("main")));
        assert!(local.takes_branch("v2.0", Some("main")));
        assert!(!local.takes_branch("v1.0-draft", Some("main")), "negated");
        assert!(
            !local.takes_branch("v0.9", Some("main")),
            "outside the range"
        );
        assert!(!local.takes_branch("feature/x", Some("feature/x")));
        assert!(
            playbook.sources[1].takes_branch("feature/x", Some("feature/x")),
            "HEAD"
        );

        let mut published = PublishedVersions::default();
        published.record("product", None, "main");
        published.record("product", Some("1.0"), "v1.0");
        published.record("product", Some("2.0"), "v2.0");
        published.record("product", Some("2.0"), "v2.0-maintenance");
        assert_eq!(
            published.branches_for_version("product", Some("1.0")),
            ["v1.0"]
        );
        assert_eq!(
            published.branches_for_version("product", Some("2.0")),
            ["v2.0", "v2.0-maintenance"]
        );
        assert_eq!(published.branches_for_version("product", None), ["main"]);
        assert!(published
            .branches_for_version("product", Some("3.0"))
            .is_empty());
        assert!(published
            .branches_for_version("other", Some("1.0"))
            .is_empty());
    }

    #[test]
    fn a_playbook_without_attributes_is_empty() {
        let playbook = parse_playbook(Path::new("p.yml"), "site:\n  title: Docs\n").unwrap();
        assert!(playbook.asciidoc_attributes.is_empty());
        assert!(parse_playbook(Path::new("p.yml"), "site: [").is_err());
    }
}
