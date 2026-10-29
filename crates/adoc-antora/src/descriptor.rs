use std::{collections::BTreeMap, fmt, fs, path::Path};

use adoc_core::{Diagnostic, DiagnosticCode, DiagnosticSeverity, SourceRange};
use serde::{de::IgnoredAny, Deserialize};
use serde_saphyr::Spanned;

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
                .filter_map(|(name, value)| Some((name, value.into_attribute_value()?)))
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
        // Antora accepts `true` or a string such as `-beta`, which also marks a prerelease.
        prerelease: match raw.prerelease {
            Some(ScalarValue::Boolean(prerelease)) => prerelease,
            Some(ScalarValue::String(suffix)) => !suffix.is_empty(),
            _ => false,
        },
    })
}

/// The keys Antora reads from a component descriptor.
const DESCRIPTOR_KEYS: &[&str] = &[
    "name",
    "title",
    "version",
    "display_version",
    "prerelease",
    "start_page",
    "nav",
    "asciidoc",
    "ext",
];

/// Problems inside a component descriptor that can be read, as warnings at the place each is
/// written: a missing `name` or `version`, a key Antora does not use, a `nav` file that does
/// not exist beneath `root`, and an `asciidoc.attributes` value that is a list or mapping. A descriptor that is not valid YAML
/// has none: [`DescriptorError`] reports it.
#[must_use]
pub fn check_component_descriptor(root: &Path, source: &str) -> Vec<Diagnostic> {
    let Ok(keys) = serde_saphyr::from_str::<BTreeMap<String, IgnoredAny>>(source) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let first_line = SourceRange::new(0, source.find('\n').unwrap_or(source.len()));

    let named = serde_saphyr::from_str::<NameCheck>(source)
        .ok()
        .and_then(|check| check.name)
        .is_some_and(|name| {
            !matches!(name, ScalarValue::Null | ScalarValue::Other(_))
                && !name.into_string().trim().is_empty()
        });
    if !named {
        found.push(diagnostic(
            DiagnosticCode::AntoraMissingKey,
            DiagnosticSeverity::Warning,
            "The component descriptor has no `name`, so Antora cannot use it".to_owned(),
            key_range(source, "name").unwrap_or(first_line),
        ));
    }
    if !keys.contains_key("version") {
        found.push(diagnostic(
            DiagnosticCode::AntoraMissingKey,
            DiagnosticSeverity::Warning,
            "The component descriptor has no `version`; write `version: ~` for a component without versions".to_owned(),
            first_line,
        ));
    }
    for key in keys.keys() {
        if !DESCRIPTOR_KEYS.contains(&key.as_str()) {
            found.push(diagnostic(
                DiagnosticCode::AntoraUnknownKey,
                DiagnosticSeverity::Warning,
                format!("Antora does not use `{key}` in a component descriptor"),
                key_range(source, key).unwrap_or(first_line),
            ));
        }
    }

    let nav = serde_saphyr::from_str::<NavList>(source)
        .map(|check| check.nav.unwrap_or_default())
        .or_else(|_| {
            serde_saphyr::from_str::<NavOne>(source).map(|check| check.nav.into_iter().collect())
        })
        .unwrap_or_default();
    for entry in nav {
        if !root.join(&entry.value).is_file() {
            found.push(diagnostic(
                DiagnosticCode::AntoraMissingNavFile,
                DiagnosticSeverity::Warning,
                format!("The navigation file `{}` does not exist", entry.value),
                value_range(source, &entry.referenced),
            ));
        }
    }

    let attributes = serde_saphyr::from_str::<AttributesCheck>(source)
        .ok()
        .and_then(|check| check.asciidoc)
        .map(|asciidoc| asciidoc.attributes)
        .unwrap_or_default();
    for (name, value) in attributes {
        if matches!(value.value, ScalarValue::Other(_)) {
            let at = value_range(source, &value.referenced);
            let key = format!("{name}:");
            let range = source[..at.start]
                .rfind(&key)
                .map_or(at, |start| SourceRange::new(start, start + name.len()));
            found.push(diagnostic(
                DiagnosticCode::AntoraInvalidAttributeValue,
                DiagnosticSeverity::Warning,
                format!(
                    "The attribute `{name}` is a list or mapping; Antora takes a string, number, `true`, `false` or `~`"
                ),
                range,
            ));
        }
    }
    found.sort_by_key(|diagnostic| (diagnostic.range, diagnostic.code));
    found
}

#[derive(Deserialize)]
struct NameCheck {
    name: Option<ScalarValue>,
}

#[derive(Deserialize)]
struct NavList {
    nav: Option<Vec<Spanned<String>>>,
}

#[derive(Deserialize)]
struct NavOne {
    nav: Option<Spanned<String>>,
}

#[derive(Deserialize)]
struct AttributesCheck {
    asciidoc: Option<AsciidocCheck>,
}

#[derive(Deserialize)]
struct AsciidocCheck {
    #[serde(default)]
    attributes: BTreeMap<String, Spanned<ScalarValue>>,
}

fn diagnostic(
    code: DiagnosticCode,
    severity: DiagnosticSeverity,
    message: String,
    range: SourceRange,
) -> Diagnostic {
    Diagnostic {
        code,
        severity,
        message,
        range,
    }
}

/// The range of a top-level key, written at the start of a line.
fn key_range(source: &str, key: &str) -> Option<SourceRange> {
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        let rest = line
            .strip_prefix(key)
            .or_else(|| line.strip_prefix(&format!("\"{key}\"")))
            .or_else(|| line.strip_prefix(&format!("'{key}'")));
        if let Some(rest) = rest {
            if rest.trim_start_matches([' ', '\t']).starts_with(':') {
                let written = line.len() - rest.len();
                return Some(SourceRange::new(start, start + written));
            }
        }
        start += line.len();
    }
    None
}

/// The byte range of a value from its location, or the rest of its line when the parser
/// gives no length.
fn value_range(source: &str, location: &serde_saphyr::Location) -> SourceRange {
    let span = location.span();
    let start = span
        .byte_offset()
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|&offset| offset <= source.len())
        .unwrap_or(0);
    let length = span
        .byte_len()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0);
    let end = if length > 0 {
        (start + length).min(source.len())
    } else {
        source[start..]
            .find('\n')
            .map_or(source.len(), |end| start + end)
    };
    SourceRange::new(start, end)
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
    prerelease: Option<ScalarValue>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawAsciidoc {
    #[serde(default)]
    pub(crate) attributes: BTreeMap<String, ScalarValue>,
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
pub(crate) enum ScalarValue {
    String(String),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Boolean(bool),
    Null,
    /// A list or mapping, which Antora cannot use as a scalar.
    Other(IgnoredAny),
}

impl ScalarValue {
    fn into_optional_string(self) -> Option<String> {
        match self {
            Self::Null | Self::Other(_) => None,
            value => Some(value.into_string()),
        }
    }

    /// Antora's reading of an `asciidoc.attributes` value: `true` sets the attribute with an
    /// empty value, `false` and `~` unset it, and anything else is the value itself.
    pub(crate) fn into_attribute_value(self) -> Option<String> {
        match self {
            Self::Boolean(true) => Some(String::new()),
            Self::Boolean(false) | Self::Null | Self::Other(_) => None,
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
            Self::Null | Self::Other(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{check_component_descriptor, parse_component_descriptor, DescriptorErrorKind};

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
        assert_eq!(descriptor.asciidoc_attributes["experimental"], "");
    }

    #[test]
    fn reads_attribute_values_the_way_antora_does() {
        let descriptor = parse_component_descriptor(
            Path::new("docs"),
            "name: demo\nasciidoc:\n  attributes:\n    toc: true\n    sectids: false\n    hide-uri-scheme: ~\n    product: Widget@\n    release: 2\n",
        )
        .unwrap();
        let attributes = &descriptor.asciidoc_attributes;

        // `true` sets the attribute with an empty value.
        assert_eq!(attributes.get("toc").map(String::as_str), Some(""));
        // `false` and `~` unset it, so nothing is passed on.
        assert!(!attributes.contains_key("sectids"), "{attributes:?}");
        assert!(
            !attributes.contains_key("hide-uri-scheme"),
            "{attributes:?}"
        );
        // A trailing `@` soft-sets; Asciidoctor reads that suffix itself.
        assert_eq!(
            attributes.get("product").map(String::as_str),
            Some("Widget@")
        );
        assert_eq!(attributes.get("release").map(String::as_str), Some("2"));
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

    #[test]
    fn problems_inside_a_descriptor_are_reported_where_they_are_written() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-descriptor-problems");
        let source = std::fs::read_to_string(root.join("antora.yml")).unwrap();
        let found = check_component_descriptor(&root, &source);
        let at = |diagnostic: &adoc_core::Diagnostic| {
            &source[diagnostic.range.start..diagnostic.range.end]
        };
        let summary: Vec<(&str, &str)> = found
            .iter()
            .map(|diagnostic| (diagnostic.code.as_str(), at(diagnostic)))
            .collect();
        assert_eq!(
            summary,
            [
                ("adoc.antora.missing-key", "name: demo"),
                ("adoc.antora.unknown-key", "edition"),
                (
                    "adoc.antora.missing-nav-file",
                    "modules/ROOT/missing-nav.adoc"
                ),
                ("adoc.antora.invalid-attribute-value", "owners"),
            ],
            "{found:#?}"
        );
        assert!(found[0].message.contains("`version`"), "{found:?}");
        assert!(found
            .iter()
            .all(|diagnostic| diagnostic.severity == adoc_core::DiagnosticSeverity::Warning));

        // The descriptor still loads, without the attribute Antora cannot use.
        let descriptor = parse_component_descriptor(&root, &source).unwrap();
        assert_eq!(descriptor.asciidoc_attributes["product-name"], "Foundry");
        assert!(!descriptor.asciidoc_attributes.contains_key("owners"));
    }

    #[test]
    fn a_missing_name_is_reported_on_the_first_line() {
        let found = check_component_descriptor(Path::new("docs"), "title: Untitled\nversion: ~\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code.as_str(), "adoc.antora.missing-key");
        assert!(found[0].message.contains("`name`"));
        assert_eq!((found[0].range.start, found[0].range.end), (0, 15));
    }

    #[test]
    fn a_sound_descriptor_has_no_problems() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let source = std::fs::read_to_string(root.join("antora.yml")).unwrap();
        assert_eq!(check_component_descriptor(&root, &source), Vec::new());
        // A single nav file, versionless components and extension keys are all Antora's.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nav.adoc"), "* xref:a.adoc[]\n").unwrap();
        assert_eq!(
            check_component_descriptor(
                dir.path(),
                "name: demo\nversion: ~\nnav: nav.adoc\next:\n  collector: {}\n"
            ),
            Vec::new()
        );
        // Not YAML: the descriptor error says so instead.
        assert_eq!(
            check_component_descriptor(dir.path(), "name: [\n"),
            Vec::new()
        );
    }
}
