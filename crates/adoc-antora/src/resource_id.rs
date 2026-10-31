use std::{fmt, path::PathBuf, str::FromStr};

use crate::{AntoraResource, ResourceFamily};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AntoraResourceId {
    pub version: Option<String>,
    pub component: Option<String>,
    pub module: Option<String>,
    pub family: Option<ResourceFamily>,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceIdParseError {
    Empty,
    EmptyCoordinate,
    TooManyCoordinates,
    InvalidVersionCoordinate,
    UnknownFamily(String),
    InvalidPath(String),
}

impl fmt::Display for ResourceIdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Antora resource ID is empty"),
            Self::EmptyCoordinate => {
                formatter.write_str("Antora resource ID contains an empty coordinate")
            }
            Self::TooManyCoordinates => {
                formatter.write_str("Antora resource ID has too many coordinate segments")
            }
            Self::InvalidVersionCoordinate => {
                formatter.write_str("Antora resource ID has an invalid version coordinate")
            }
            Self::UnknownFamily(family) => {
                write!(formatter, "unknown Antora resource family `{family}`")
            }
            Self::InvalidPath(path) => write!(formatter, "invalid Antora resource path `{path}`"),
        }
    }
}

impl std::error::Error for ResourceIdParseError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AntoraContext {
    pub component: String,
    pub version: Option<String>,
    pub module: String,
    pub family: ResourceFamily,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolutionError {
    UnknownComponent {
        component: String,
        version: Option<String>,
    },
    /// The component is known but not at this version.
    UnknownVersion { component: String, version: String },
    UnknownModule {
        component: String,
        version: Option<String>,
        module: String,
    },
    UnknownResource {
        component: String,
        version: Option<String>,
        module: String,
        family: ResourceFamily,
        path: PathBuf,
    },
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownComponent { component, version } => write!(
                formatter,
                "unknown Antora component `{}`",
                version.as_ref().map_or_else(
                    || component.clone(),
                    |version| format!("{version}@{component}")
                )
            ),
            Self::UnknownVersion { component, version } => write!(
                formatter,
                "unknown version `{version}` of Antora component `{component}`"
            ),
            Self::UnknownModule { module, .. } => {
                write!(formatter, "unknown Antora module `{module}`")
            }
            Self::UnknownResource { family, path, .. } => write!(
                formatter,
                "unknown Antora {family} resource `{}`",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ResolutionError {}

pub type ResolutionResult<'a> = Result<&'a AntoraResource, ResolutionError>;

pub trait AntoraResolver {
    fn resolve<'a>(
        &'a self,
        id: &AntoraResourceId,
        context: &AntoraContext,
    ) -> ResolutionResult<'a>;
}

pub fn parse_resource_id(input: &str) -> Result<AntoraResourceId, ResourceIdParseError> {
    if input.is_empty() {
        return Err(ResourceIdParseError::Empty);
    }
    if input.matches('$').count() > 1 {
        return Err(ResourceIdParseError::UnknownFamily(input.to_owned()));
    }
    // As in Antora's resource id pattern, a version comes first and belongs to the whole id:
    // `2.0@index.adoc` and `2.0@module:page.adoc` keep the current component.
    let (version, input) = match input.split_once('@') {
        Some((version, rest)) if !version.contains([':', '$']) => {
            if version.is_empty() || rest.contains('@') {
                return Err(ResourceIdParseError::InvalidVersionCoordinate);
            }
            (Some(version.to_owned()), rest)
        }
        _ => (None, input),
    };

    let (scope, family, path) = if let Some((prefix, path)) = input.split_once('$') {
        let (scope, family) = prefix.rsplit_once(':').unwrap_or(("", prefix));
        let family = ResourceFamily::from_str(family)
            .map_err(|_| ResourceIdParseError::UnknownFamily(family.to_owned()))?;
        (scope, Some(family), path)
    } else if let Some((scope, path)) = input.rsplit_once(':') {
        (scope, None, path)
    } else {
        ("", None, input)
    };

    // Antora runs on Node, whose `path.join` ignores a leading separator on later
    // segments, so `partial$/note.adoc` names the same resource as `partial$note.adoc`.
    // Everything else `validate_path` rejects — `..`, backslashes, empty segments —
    // still applies.
    let path = path.strip_prefix('/').unwrap_or(path);
    validate_path(path)?;
    let scope = parse_scope(scope)?;
    Ok(AntoraResourceId {
        version,
        component: scope.component,
        module: scope.module,
        family,
        path: path.to_owned(),
    })
}

struct ParsedScope {
    component: Option<String>,
    module: Option<String>,
}

/// The coordinates before the path, the version already taken off: `module`, or
/// `component:module`, where an empty module is `ROOT`.
fn parse_scope(scope: &str) -> Result<ParsedScope, ResourceIdParseError> {
    if scope.is_empty() {
        return Ok(ParsedScope {
            component: None,
            module: None,
        });
    }
    if scope.contains('@') {
        return Err(ResourceIdParseError::InvalidVersionCoordinate);
    }
    let mut segments = scope.split(':').collect::<Vec<_>>();
    // `component::page.adoc` names the component's ROOT module.
    if segments.len() == 2 && segments[1].is_empty() {
        segments[1] = "ROOT";
    }
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(ResourceIdParseError::EmptyCoordinate);
    }

    match segments.as_slice() {
        [module] => Ok(ParsedScope {
            component: None,
            module: Some((*module).to_owned()),
        }),
        [component, module] => Ok(ParsedScope {
            component: Some((*component).to_owned()),
            module: Some((*module).to_owned()),
        }),
        _ => Err(ResourceIdParseError::TooManyCoordinates),
    }
}

fn validate_path(path: &str) -> Result<(), ResourceIdParseError> {
    let valid = !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('#')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    if valid {
        Ok(())
    } else {
        Err(ResourceIdParseError::InvalidPath(path.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use crate::ResourceFamily;

    use super::{parse_resource_id, ResourceIdParseError};

    #[test]
    fn reads_an_empty_module_coordinate_as_the_root_module() {
        let id = parse_resource_id("other-component::page.adoc").unwrap();

        assert_eq!(id.component.as_deref(), Some("other-component"));
        assert_eq!(id.module.as_deref(), Some("ROOT"));
        assert_eq!(id.path, "page.adoc");
    }

    #[test]
    fn parses_supported_resource_id_forms() {
        let local = parse_resource_id("index.adoc").unwrap();
        assert_eq!(local.path, "index.adoc");

        let module = parse_resource_id("security:authentication.adoc").unwrap();
        assert_eq!(module.module.as_deref(), Some("security"));

        let qualified = parse_resource_id("3.1@demo:security:partial$token-note.adoc").unwrap();
        assert_eq!(qualified.version.as_deref(), Some("3.1"));
        assert_eq!(qualified.component.as_deref(), Some("demo"));
        assert_eq!(qualified.module.as_deref(), Some("security"));
        assert_eq!(qualified.family, Some(ResourceFamily::Partial));
    }

    #[test]
    fn parses_version_with_module_only() {
        // The component coordinate needs a colon of its own: one segment is the module.
        let id = parse_resource_id("2.0@security:index.adoc").unwrap();
        assert_eq!(id.version.as_deref(), Some("2.0"));
        assert_eq!(id.component, None);
        assert_eq!(id.module.as_deref(), Some("security"));
        assert_eq!(id.path, "index.adoc");
    }

    #[test]
    fn parses_version_with_path_only() {
        let id = parse_resource_id("2.0@index.adoc").unwrap();
        assert_eq!(id.version.as_deref(), Some("2.0"));
        assert_eq!(id.component, None);
        assert_eq!(id.module, None);
        assert_eq!(id.path, "index.adoc");

        let partial = parse_resource_id("2.0@partial$note.adoc").unwrap();
        assert_eq!(partial.version.as_deref(), Some("2.0"));
        assert_eq!(partial.module, None);
        assert_eq!(partial.family, Some(ResourceFamily::Partial));
        assert_eq!(partial.path, "note.adoc");

        let both = parse_resource_id("2.0@demo:security:index.adoc").unwrap();
        assert_eq!(both.version.as_deref(), Some("2.0"));
        assert_eq!(both.component.as_deref(), Some("demo"));
        assert_eq!(both.module.as_deref(), Some("security"));
    }

    #[test]
    fn rejects_ambiguous_or_unsafe_ids() {
        assert_eq!(
            parse_resource_id("demo:module:extra:index.adoc"),
            Err(ResourceIdParseError::TooManyCoordinates)
        );
        assert!(matches!(
            parse_resource_id("partial$../secret.adoc"),
            Err(ResourceIdParseError::InvalidPath(_))
        ));
        for id in [
            "@index.adoc",
            "1.0@2.0@index.adoc",
            "demo:1.0@ROOT:index.adoc",
        ] {
            assert_eq!(
                parse_resource_id(id),
                Err(ResourceIdParseError::InvalidVersionCoordinate),
                "{id}"
            );
        }
    }
}
