use std::{collections::BTreeMap, path::PathBuf};

use adoc_core::SourceRange;

/// One key under `asciidoc.attributes` in a playbook or a component descriptor, as Antora reads
/// it: a value ending in `@` is soft set, `false` soft unsets, `~` hard unsets, anything else
/// hard sets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeEntry {
    /// The value, `@` removed, `true` read as an empty value; `None` when the entry unsets the
    /// attribute.
    pub value: Option<String>,
    /// A later scope (a component, or a page) may change it.
    pub soft: bool,
    /// Where the attribute's name is written.
    pub range: SourceRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentDescriptor {
    pub root: PathBuf,
    pub name: String,
    pub title: Option<String>,
    pub version: Option<String>,
    pub display_version: Option<String>,
    pub start_page: Option<String>,
    pub nav: Vec<String>,
    pub asciidoc_attributes: BTreeMap<String, String>,
    /// Every key under `asciidoc.attributes`, unsets included, with its strength and where it
    /// is written.
    pub attribute_entries: BTreeMap<String, AttributeEntry>,
    /// `prerelease` in `antora.yml`: the version sorts below every released version.
    pub prerelease: bool,
}
