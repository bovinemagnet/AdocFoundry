use std::{
    collections::{hash_map::DefaultHasher, BTreeMap},
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

use adoc_core::{alphanumeric_id, canonical_id, Document, Reference, SourceRange};
use adoc_parser::parse;

use crate::workspace::{collect_asciidoc_files, normalize_path};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AnchorKey {
    pub path: PathBuf,
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorLocation {
    pub path: PathBuf,
    pub range: SourceRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceLocation {
    pub path: PathBuf,
    pub reference: Reference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub uri: String,
    pub content_hash: u64,
    pub document: Document,
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceIndex {
    files: BTreeMap<PathBuf, FileEntry>,
    anchors: BTreeMap<AnchorKey, Vec<AnchorLocation>>,
    references: Vec<ReferenceLocation>,
}

impl WorkspaceIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn index_source(&mut self, path: impl AsRef<Path>, text: &str) -> &FileEntry {
        let path = normalize_path(path.as_ref());
        let uri = path_to_uri(&path);
        let document = parse(&uri, text).document;
        self.replace(path, document)
    }

    pub fn replace(&mut self, path: PathBuf, document: Document) -> &FileEntry {
        let path = normalize_path(&path);
        self.remove(&path);

        for anchor in &document.anchors {
            let key = AnchorKey {
                path: path.clone(),
                id: anchor.id.clone(),
            };
            self.anchors.entry(key).or_default().push(AnchorLocation {
                path: path.clone(),
                range: anchor.range,
            });
        }

        // Sections are reachable without an explicit anchor, so register the ids Asciidoctor
        // generates for them. They are deliberately not added to `document.anchors`, which
        // stays the record of explicitly declared anchors for duplicate detection.
        for section in &document.sections {
            for id in section.implicit_ids() {
                let key = AnchorKey {
                    path: path.clone(),
                    id,
                };
                self.anchors.entry(key).or_default().push(AnchorLocation {
                    path: path.clone(),
                    range: section.selection_range,
                });
            }
        }

        self.references
            .extend(
                document
                    .references
                    .iter()
                    .cloned()
                    .map(|reference| ReferenceLocation {
                        path: path.clone(),
                        reference,
                    }),
            );

        let uri = document.uri.clone();
        let content_hash = hash_text(&document.text);
        self.files.entry(path.clone()).or_insert(FileEntry {
            path,
            uri,
            content_hash,
            document,
        })
    }

    pub fn remove(&mut self, path: &Path) -> Option<FileEntry> {
        let path = normalize_path(path);
        self.anchors.retain(|key, _| key.path != path);
        self.references.retain(|reference| reference.path != path);
        self.files.remove(&path)
    }

    pub fn index_roots(&mut self, roots: &[PathBuf]) -> io::Result<usize> {
        let mut paths = Vec::new();
        for root in roots {
            collect_asciidoc_files(root, &mut paths)?;
        }
        paths.sort();
        paths.dedup();

        for path in &paths {
            let text = fs::read_to_string(path)?;
            self.index_source(path, &text);
        }
        Ok(paths.len())
    }

    #[must_use]
    pub fn file(&self, path: &Path) -> Option<&FileEntry> {
        self.files.get(&normalize_path(path))
    }

    pub fn files(&self) -> impl Iterator<Item = &FileEntry> {
        self.files.values()
    }

    #[must_use]
    pub fn references(&self) -> &[ReferenceLocation] {
        &self.references
    }

    #[must_use]
    pub fn resolve_anchor(&self, path: &Path, id: &str) -> Option<&AnchorLocation> {
        let path = normalize_path(path);
        let exact = self.anchors.get(&AnchorKey {
            path: path.clone(),
            id: id.to_owned(),
        });
        exact
            .or_else(|| {
                self.anchors.get(&AnchorKey {
                    path: path.clone(),
                    id: canonical_id(id),
                })
            })
            .or_else(|| {
                self.anchors.get(&AnchorKey {
                    path,
                    id: alphanumeric_id(id),
                })
            })
            .and_then(|locations| locations.first())
    }
}

fn path_to_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::WorkspaceIndex;

    #[test]
    fn replacing_a_document_removes_stale_contributions() {
        let path = PathBuf::from("docs/guide.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(&path, "[[old]]\n== Old\n\nSee <<old>>.\n");

        assert!(index.resolve_anchor(&path, "old").is_some());
        assert_eq!(index.references().len(), 1);

        index.index_source(&path, "[[new]]\n== New\n");

        assert!(index.resolve_anchor(&path, "old").is_none());
        assert!(index.resolve_anchor(&path, "new").is_some());
        assert!(index.references().is_empty());
        assert_eq!(index.files().count(), 1);
    }

    #[test]
    fn scans_repository_fixtures_deterministically() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/simple");
        let mut index = WorkspaceIndex::new();

        let count = index.index_roots(&[root]).unwrap();

        assert_eq!(count, 1);
        assert_eq!(index.files().count(), 1);
    }
}
