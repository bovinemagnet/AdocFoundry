use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RenderSafeMode {
    Secure,
    #[default]
    Safe,
    Server,
    Unsafe,
}

impl RenderSafeMode {
    #[must_use]
    pub const fn as_cli_value(self) -> &'static str {
        match self {
            Self::Secure => "secure",
            Self::Safe => "safe",
            Self::Server => "server",
            Self::Unsafe => "unsafe",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderRequest {
    pub source_file: PathBuf,
    pub source_text: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub safe_mode: RenderSafeMode,
    pub stylesheet: Option<PathBuf>,
    /// Root of the safe-mode jail. Includes outside it are refused, so a document that
    /// pulls in files from a sibling directory needs this set to a common ancestor.
    pub base_dir: Option<PathBuf>,
}

impl RenderRequest {
    #[must_use]
    pub fn from_file(source_file: impl Into<PathBuf>) -> Self {
        Self {
            source_file: source_file.into(),
            source_text: None,
            attributes: BTreeMap::new(),
            safe_mode: RenderSafeMode::default(),
            stylesheet: None,
            base_dir: None,
        }
    }

    #[must_use]
    pub fn from_source(source_file: impl Into<PathBuf>, source_text: impl Into<String>) -> Self {
        Self {
            source_text: Some(source_text.into()),
            ..Self::from_file(source_file)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RenderRequest, RenderSafeMode};

    #[test]
    fn defaults_to_no_base_dir_override() {
        assert!(RenderRequest::from_file("guide.adoc").base_dir.is_none());
    }

    #[test]
    fn defaults_to_safe_file_rendering() {
        let request = RenderRequest::from_file("guide.adoc");

        assert_eq!(request.source_file, std::path::Path::new("guide.adoc"));
        assert_eq!(request.safe_mode, RenderSafeMode::Safe);
        assert!(request.source_text.is_none());
    }
}
