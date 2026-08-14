use std::{
    collections::BTreeMap,
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
    pub fn open(&mut self, uri: &str, text: &str, version: i32) -> &OpenDocument {
        self.update(uri, text, version)
    }

    pub fn change(&mut self, uri: &str, text: &str, version: i32) -> &OpenDocument {
        self.update(uri, text, version)
    }

    pub fn close(&mut self, uri: &str) -> Option<OpenDocument> {
        let closed = self.documents.remove(uri);
        self.index.remove(&uri_to_path(uri));
        closed
    }

    fn update(&mut self, uri: &str, text: &str, version: i32) -> &OpenDocument {
        let document = parse(uri, text).document;
        self.index.replace(uri_to_path(uri), document.clone());
        self.documents
            .insert(uri.to_owned(), OpenDocument { version, document });
        self.documents
            .get(uri)
            .expect("document was inserted immediately before lookup")
    }
}

fn uri_to_path(uri: &str) -> PathBuf {
    Path::new(uri.strip_prefix("file://").unwrap_or(uri)).to_path_buf()
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

        state.close(uri);
        assert!(state.documents.is_empty());
        assert_eq!(state.index.files().count(), 0);
    }
}
