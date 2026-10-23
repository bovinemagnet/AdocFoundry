use std::{fmt, path::PathBuf, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResourceFamily {
    Page,
    Partial,
    Example,
    Image,
    Attachment,
}

impl ResourceFamily {
    pub const ALL: [Self; 5] = [
        Self::Page,
        Self::Partial,
        Self::Example,
        Self::Image,
        Self::Attachment,
    ];

    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::Page => "pages",
            Self::Partial => "partials",
            Self::Example => "examples",
            Self::Image => "images",
            Self::Attachment => "attachments",
        }
    }
}

impl fmt::Display for ResourceFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Page => "page",
            Self::Partial => "partial",
            Self::Example => "example",
            Self::Image => "image",
            Self::Attachment => "attachment",
        };
        formatter.write_str(name)
    }
}

impl FromStr for ResourceFamily {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "page" => Ok(Self::Page),
            "partial" => Ok(Self::Partial),
            "example" => Ok(Self::Example),
            "image" => Ok(Self::Image),
            "attachment" => Ok(Self::Attachment),
            _ => Err(format!("unknown Antora resource family `{value}`")),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AntoraCoordinate {
    pub component: String,
    pub version: Option<String>,
    pub module: String,
    pub family: ResourceFamily,
    pub relative_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AntoraResource {
    pub coordinate: AntoraCoordinate,
    pub source_path: PathBuf,
}
