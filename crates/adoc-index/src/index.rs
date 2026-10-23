use std::{
    collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet},
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

use adoc_core::{alphanumeric_id, canonical_id, Document, Reference, SourceRange};
use adoc_parser::parse;

use crate::{
    diagnostics::resolve_include_target,
    symbols::{document_symbols, words, WorkspaceSymbol},
    workspace::{collect_asciidoc_files, normalize_path},
};

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
    /// Include target → the files that include it. Only targets the index can resolve on its
    /// own are recorded: relative paths and attributes the including document declares.
    includers: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    /// Each file's symbols in document order.
    symbols: BTreeMap<PathBuf, Vec<WorkspaceSymbol>>,
    /// `(word, file, position in that file's symbols)`, so a query reaches its matches by an
    /// ordered range scan rather than by visiting every symbol.
    symbol_words: BTreeSet<(String, PathBuf, usize)>,
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

        for target in include_targets(&path, &document) {
            self.includers
                .entry(target)
                .or_default()
                .insert(path.clone());
        }

        let symbols = document_symbols(&path, &document);
        for (position, symbol) in symbols.iter().enumerate() {
            for word in words(&symbol.name) {
                self.symbol_words.insert((word, path.clone(), position));
            }
        }
        self.symbols.insert(path.clone(), symbols);

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
        if let Some(symbols) = self.symbols.remove(&path) {
            for (position, symbol) in symbols.iter().enumerate() {
                for word in words(&symbol.name) {
                    self.symbol_words.remove(&(word, path.clone(), position));
                }
            }
        }
        let removed = self.files.remove(&path)?;
        // Resolution is a pure function of the text and path, so this yields exactly the
        // targets `replace` recorded.
        for target in include_targets(&path, &removed.document) {
            if let Some(includers) = self.includers.get_mut(&target) {
                includers.remove(&path);
                if includers.is_empty() {
                    self.includers.remove(&target);
                }
            }
        }
        Some(removed)
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

    /// The files that include `path`, as far as the index can resolve their includes.
    pub fn includers(&self, path: &Path) -> impl Iterator<Item = &Path> {
        self.includers
            .get(&normalize_path(path))
            .into_iter()
            .flatten()
            .map(PathBuf::as_path)
    }

    /// Symbols with a word starting with each word of `query`, in file and document order.
    ///
    /// An empty query finds nothing rather than every symbol in the workspace.
    #[must_use]
    pub fn symbols_matching(&self, query: &str) -> Vec<&WorkspaceSymbol> {
        let mut query_words = words(query);
        let Some(first) = query_words.next() else {
            return Vec::new();
        };
        let rest: Vec<String> = query_words.collect();

        let hits: BTreeSet<(&Path, usize)> = self
            .symbol_words
            .range((first.clone(), PathBuf::new(), 0)..)
            .take_while(|(word, _, _)| word.starts_with(&first))
            .map(|(_, path, position)| (path.as_path(), *position))
            .collect();

        hits.into_iter()
            .filter_map(|(path, position)| self.symbols.get(path)?.get(position))
            .filter(|symbol| {
                rest.iter()
                    .all(|wanted| words(&symbol.name).any(|word| word.starts_with(wanted.as_str())))
            })
            .collect()
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

fn include_targets<'a>(
    path: &'a Path,
    document: &'a Document,
) -> impl Iterator<Item = PathBuf> + 'a {
    document
        .includes
        .iter()
        .filter_map(move |include| resolve_include_target(document, path, &include.target))
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

    #[test]
    fn records_the_files_that_include_a_document() {
        let guide = PathBuf::from("docs/guide.adoc");
        let setup = PathBuf::from("docs/chapters/setup.adoc");
        let mut index = WorkspaceIndex::new();
        // Indexed before its includer: the map is keyed by target, so order is irrelevant.
        index.index_source(&setup, "== Setup\n");
        index.index_source(&guide, "= Guide\n\ninclude::chapters/setup.adoc[]\n");

        let includers: Vec<_> = index.includers(&setup).collect();

        assert_eq!(includers, vec![guide.as_path()]);
        assert_eq!(index.includers(&guide).count(), 0);
    }

    #[test]
    fn records_an_include_written_with_a_declared_attribute() {
        let guide = PathBuf::from("docs/guide.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            &guide,
            ":chapters: chapters\n\ninclude::{chapters}/setup.adoc[]\n",
        );

        assert_eq!(
            index
                .includers(Path::new("docs/chapters/setup.adoc"))
                .count(),
            1
        );
    }

    #[test]
    fn forgets_an_include_the_includer_drops() {
        let guide = PathBuf::from("docs/guide.adoc");
        let setup = Path::new("docs/chapters/setup.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(&guide, "include::chapters/setup.adoc[]\n");

        index.index_source(&guide, "= Guide\n");

        assert_eq!(index.includers(setup).count(), 0);
    }

    #[test]
    fn forgets_the_includes_of_a_removed_file() {
        let guide = PathBuf::from("docs/guide.adoc");
        let setup = Path::new("docs/chapters/setup.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(&guide, "include::chapters/setup.adoc[]\n");

        index.remove(&guide);

        assert_eq!(index.includers(setup).count(), 0);
    }

    fn names(symbols: Vec<&crate::WorkspaceSymbol>) -> Vec<&str> {
        symbols.iter().map(|symbol| symbol.name.as_str()).collect()
    }

    const GUIDE: &str =
        "= Guide\n\n[[auth-flow]]\n== Authentication Flow\n\n== Authentication Setup\n";

    #[test]
    fn finds_symbols_by_the_prefix_of_any_word() {
        let mut index = WorkspaceIndex::new();
        index.index_source("docs/guide.adoc", GUIDE);

        assert_eq!(
            names(index.symbols_matching("auth")),
            ["Authentication Flow", "Authentication Setup", "auth-flow"]
        );
        assert_eq!(
            names(index.symbols_matching("FLOW")),
            ["Authentication Flow", "auth-flow"]
        );
    }

    #[test]
    fn requires_every_query_word_to_match() {
        let mut index = WorkspaceIndex::new();
        index.index_source("docs/guide.adoc", GUIDE);

        assert_eq!(
            names(index.symbols_matching("auth set")),
            ["Authentication Setup"]
        );
        assert!(index.symbols_matching("auth missing").is_empty());
    }

    #[test]
    fn returns_a_symbol_once_when_several_of_its_words_match() {
        let mut index = WorkspaceIndex::new();
        index.index_source("docs/guide.adoc", "== Flow Flowchart\n");

        assert_eq!(names(index.symbols_matching("flow")), ["Flow Flowchart"]);
    }

    #[test]
    fn forgets_the_symbols_of_replaced_and_removed_files() {
        let path = PathBuf::from("docs/guide.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(&path, GUIDE);

        index.index_source(&path, "== Overview\n");
        assert!(index.symbols_matching("auth").is_empty());
        assert_eq!(names(index.symbols_matching("over")), ["Overview"]);

        index.remove(&path);
        assert!(index.symbols_matching("over").is_empty());
    }

    #[test]
    fn returns_nothing_for_an_empty_query() {
        let mut index = WorkspaceIndex::new();
        index.index_source("docs/guide.adoc", GUIDE);

        assert!(index.symbols_matching("").is_empty());
        assert!(index.symbols_matching(" -- ").is_empty());
    }
}
