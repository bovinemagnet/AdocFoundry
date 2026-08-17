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

    validate_path(path)?;
    let scope = parse_scope(scope)?;
    Ok(AntoraResourceId {
        version: scope.version,
        component: scope.component,
        module: scope.module,
        family,
        path: path.to_owned(),
    })
}

struct ParsedScope {
    version: Option<String>,
    component: Option<String>,
    module: Option<String>,
}

fn parse_scope(scope: &str) -> Result<ParsedScope, ResourceIdParseError> {
    if scope.is_empty() {
        return Ok(ParsedScope {
            version: None,
            component: None,
            module: None,
        });
    }
    let segments = scope.split(':').collect::<Vec<_>>();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(ResourceIdParseError::EmptyCoordinate);
    }

    match segments.as_slice() {
        [module] if !module.contains('@') => Ok(ParsedScope {
            version: None,
            component: None,
            module: Some((*module).to_owned()),
        }),
        [component] => {
            let (version, component) = parse_component(component)?;
            Ok(ParsedScope {
                version,
                component: Some(component),
                module: None,
            })
        }
        [component, module] => {
            let (version, component) = parse_component(component)?;
            Ok(ParsedScope {
                version,
                component: Some(component),
                module: Some((*module).to_owned()),
            })
        }
        _ => Err(ResourceIdParseError::TooManyCoordinates),
    }
}

fn parse_component(coordinate: &str) -> Result<(Option<String>, String), ResourceIdParseError> {
    if coordinate.matches('@').count() > 1 {
        return Err(ResourceIdParseError::InvalidVersionCoordinate);
    }
    if let Some((version, component)) = coordinate.split_once('@') {
        if version.is_empty() || component.is_empty() {
            return Err(ResourceIdParseError::InvalidVersionCoordinate);
        }
        Ok((Some(version.to_owned()), component.to_owned()))
    } else {
        Ok((None, coordinate.to_owned()))
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
    fn rejects_ambiguous_or_unsafe_ids() {
        assert_eq!(
            parse_resource_id("demo:module:extra:index.adoc"),
            Err(ResourceIdParseError::TooManyCoordinates)
        );
        assert!(matches!(
            parse_resource_id("partial$../secret.adoc"),
            Err(ResourceIdParseError::InvalidPath(_))
        ));
    }
}
