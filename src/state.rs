use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use adoc_antora::AntoraCatalog;
use adoc_core::Document;
use adoc_index::WorkspaceIndex;
use adoc_parser::parse;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenDocument {
    pub version: i32,
    pub document: Document,
}

#[derive(Clone, Debug, Default)]
pub struct DocumentStore {
    documents: BTreeMap<String, OpenDocument>,
}

impl DocumentStore {
    #[must_use]
    pub fn get(&self, uri: &str) -> Option<&OpenDocument> {
        self.documents.get(uri)
    }

    pub fn insert(&mut self, uri: String, document: OpenDocument) -> Option<OpenDocument> {
        self.documents.insert(uri, document)
    }

    pub fn remove(&mut self, uri: &str) -> Option<OpenDocument> {
        self.documents.remove(uri)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ServerState {
    pub workspace_roots: Vec<PathBuf>,
    pub documents: DocumentStore,
    pub index: WorkspaceIndex,
    pub antora: AntoraCatalog,
}

impl ServerState {
    pub fn index_workspace(&mut self, roots: Vec<PathBuf>) -> io::Result<usize> {
        let indexed = self.index.index_roots(&roots)?;
        self.workspace_roots = roots;
        Ok(indexed)
    }

    pub fn open(&mut self, uri: &str, text: &str, version: i32) -> &OpenDocument {
        self.update(uri, text, version)
    }

    pub fn change(&mut self, uri: &str, text: &str, version: i32) -> &OpenDocument {
        self.update(uri, text, version)
    }

    pub fn close(&mut self, uri: &str) -> io::Result<Option<OpenDocument>> {
        let closed = self.documents.remove(uri);
        let path = document_path(uri);
        if path.is_file() {
            let text = fs::read_to_string(&path)?;
            self.index.index_source(&path, &text);
        } else {
            self.index.remove(&path);
        }
        Ok(closed)
    }

    fn update(&mut self, uri: &str, text: &str, version: i32) -> &OpenDocument {
        let document = parse(uri, text).document;
        self.index.replace(document_path(uri), document.clone());
        self.documents
            .insert(uri.to_owned(), OpenDocument { version, document });
        self.documents
            .get(uri)
            .expect("document was inserted immediately before lookup")
    }
}

#[must_use]
pub fn document_path(uri: &str) -> PathBuf {
    url::Url::parse(uri)
        .ok()
        .and_then(|uri| uri.to_file_path().ok())
        .unwrap_or_else(|| Path::new(uri).to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::ServerState;

    #[test]
    fn open_change_and_close_replace_document_state() {
        let uri = "file:///docs/guide.adoc";
        let mut state = ServerState::default();

        state.open(uri, "[[old]]\n== Old\n", 1);
        assert!(state
            .index
            .resolve_anchor(std::path::Path::new("/docs/guide.adoc"), "old")
            .is_some());

        state.change(uri, "[[new]]\n== New\n", 2);
        assert_eq!(state.documents.get(uri).unwrap().version, 2);
        assert!(state
            .index
            .resolve_anchor(std::path::Path::new("/docs/guide.adoc"), "old")
            .is_none());

        state.close(uri).unwrap();
        assert!(state.documents.is_empty());
        assert_eq!(state.index.files().count(), 0);
    }

    #[test]
    fn converts_percent_encoded_file_uris() {
        assert_eq!(
            super::document_path("file:///docs/My%20Guide.adoc"),
            std::path::Path::new("/docs/My Guide.adoc")
        );
    }
}
