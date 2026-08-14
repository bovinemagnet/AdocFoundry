use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderRequest {
    pub source: String,
    pub source_path: Option<PathBuf>,
    pub attributes: BTreeMap<String, String>,
}

impl RenderRequest {
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            ..Self::default()
        }
    }
}
