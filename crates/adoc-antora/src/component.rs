use std::{collections::BTreeMap, path::PathBuf};

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
}
