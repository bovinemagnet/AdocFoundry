use std::{collections::BTreeMap, fmt, fs, path::Path};

use serde::Deserialize;

use crate::ComponentDescriptor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescriptorErrorKind {
    Read,
    InvalidYaml,
    MissingName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DescriptorError {
    pub kind: DescriptorErrorKind,
    pub path: std::path::PathBuf,
    pub message: String,
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for DescriptorError {}

pub fn read_component_descriptor(root: &Path) -> Result<ComponentDescriptor, DescriptorError> {
    let path = root.join("antora.yml");
    let source = fs::read_to_string(&path).map_err(|error| DescriptorError {
        kind: DescriptorErrorKind::Read,
        path,
        message: error.to_string(),
    })?;
    parse_component_descriptor(root, &source)
}

pub fn parse_component_descriptor(
    root: &Path,
    source: &str,
) -> Result<ComponentDescriptor, DescriptorError> {
    let path = root.join("antora.yml");
    let raw: RawDescriptor = serde_saphyr::from_str(source).map_err(|error| DescriptorError {
        kind: DescriptorErrorKind::InvalidYaml,
        path: path.clone(),
        message: error.to_string(),
    })?;
    let name = raw
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| DescriptorError {
            kind: DescriptorErrorKind::MissingName,
            path,
            message: "Antora component descriptor is missing `name`".to_owned(),
        })?;
    let asciidoc_attributes = raw
        .asciidoc
        .map(|asciidoc| {
            asciidoc
                .attributes
                .into_iter()
                .map(|(name, value)| (name, value.into_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok(ComponentDescriptor {
        root: root.to_path_buf(),
        name,
        title: raw.title,
        version: raw.version.and_then(ScalarValue::into_optional_string),
        display_version: raw
            .display_version
            .and_then(ScalarValue::into_optional_string),
        start_page: raw.start_page,
        nav: raw.nav.map_or_else(Vec::new, OneOrMany::into_vec),
        asciidoc_attributes,
    })
}

#[derive(Debug, Deserialize)]
struct RawDescriptor {
    name: Option<String>,
    title: Option<String>,
    version: Option<ScalarValue>,
    display_version: Option<ScalarValue>,
    start_page: Option<String>,
    nav: Option<OneOrMany>,
    asciidoc: Option<RawAsciidoc>,
}

#[derive(Debug, Deserialize)]
struct RawAsciidoc {
    #[serde(default)]
    attributes: BTreeMap<String, ScalarValue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ScalarValue {
    String(String),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Boolean(bool),
    Null,
}

impl ScalarValue {
    fn into_optional_string(self) -> Option<String> {
        match self {
            Self::Null => None,
            value => Some(value.into_string()),
        }
    }

    fn into_string(self) -> String {
        match self {
            Self::String(value) => value,
            Self::Signed(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
            Self::Null => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{parse_component_descriptor, DescriptorErrorKind};

    #[test]
    fn parses_supported_descriptor_fields() {
        let descriptor = parse_component_descriptor(
            Path::new("docs"),
            "name: demo\nversion: 3.1\ndisplay_version: '3.1'\nnav: modules/ROOT/nav.adoc\nasciidoc:\n  attributes:\n    sectanchors: ''\n    experimental: true\n",
        )
        .unwrap();

        assert_eq!(descriptor.name, "demo");
        assert_eq!(descriptor.version.as_deref(), Some("3.1"));
        assert_eq!(descriptor.nav, ["modules/ROOT/nav.adoc"]);
        assert_eq!(descriptor.asciidoc_attributes["experimental"], "true");
    }

    #[test]
    fn reports_missing_name_without_requiring_optional_fields() {
        let error = parse_component_descriptor(Path::new("docs"), "title: Demo\n").unwrap_err();

        assert_eq!(error.kind, DescriptorErrorKind::MissingName);
        assert!(error.message.contains("name"));
    }

    #[test]
    fn preserves_yaml_error_details() {
        let error = parse_component_descriptor(Path::new("docs"), "name: [\n").unwrap_err();

        assert_eq!(error.kind, DescriptorErrorKind::InvalidYaml);
        assert!(!error.message.is_empty());
    }
}
