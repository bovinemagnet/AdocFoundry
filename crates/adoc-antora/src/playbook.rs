//! The Antora playbook's AsciiDoc attributes, which apply to every page of the site.

use std::{
    collections::BTreeMap,
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
    asciidoc: Option<RawAsciidoc>,
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
    Ok(Playbook {
        path: path.to_path_buf(),
        asciidoc_attributes,
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
    fn a_playbook_without_attributes_is_empty() {
        let playbook = parse_playbook(Path::new("p.yml"), "site:\n  title: Docs\n").unwrap();
        assert!(playbook.asciidoc_attributes.is_empty());
        assert!(parse_playbook(Path::new("p.yml"), "site: [").is_err());
    }
}
