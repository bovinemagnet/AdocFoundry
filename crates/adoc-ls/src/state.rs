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
        // A URI as a client on this platform sends it, with the space percent-encoded.
        let path = std::env::temp_dir().join("docs").join("My Guide.adoc");
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        assert!(uri.contains("My%20Guide.adoc"), "{uri}");
        assert_eq!(super::document_path(&uri), path);
    }
}
