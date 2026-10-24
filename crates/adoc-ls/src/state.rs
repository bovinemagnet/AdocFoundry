use std::path::{Path, PathBuf};

/// The file path a document URI names; a URI that is not a file URL is taken as a path.
#[must_use]
pub fn document_path(uri: &str) -> PathBuf {
    url::Url::parse(uri)
        .ok()
        .and_then(|uri| uri.to_file_path().ok())
        .unwrap_or_else(|| Path::new(uri).to_path_buf())
}

#[cfg(test)]
mod tests {
    #[test]
    fn converts_percent_encoded_file_uris() {
        assert_eq!(
            super::document_path("file:///docs/My%20Guide.adoc"),
            std::path::Path::new("/docs/My Guide.adoc")
        );
    }
}
