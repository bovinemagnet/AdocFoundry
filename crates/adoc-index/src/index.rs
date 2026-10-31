use std::{
    collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet},
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    thread,
};

use adoc_core::{
    alphanumeric_id, canonical_id, Document, IncludeDirective, Reference, ReferenceKind,
    SourceRange,
};
use adoc_parser::parse;

use crate::{
    diagnostics::resolve_include_target,
    symbols::{document_symbols, words, WorkspaceSymbol},
    workspace::{collect_asciidoc_files, normalize_path},
    DEFAULT_IGNORED_DIRECTORIES,
};

/// The most threads that read and parse files while a workspace is indexed.
const PARSE_THREADS: usize = 4;

/// What indexing a workspace's roots found.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexReport {
    /// The AsciiDoc files indexed.
    pub files: usize,
    /// The folders and files that could not be read, and were left out.
    pub unreadable: Vec<PathBuf>,
}

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

/// An include directive and the file it is in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncludeLocation {
    pub path: PathBuf,
    pub include: IncludeDirective,
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
    /// The file name an xref's target ends with (`setup.adoc`) → the xrefs naming it, so the
    /// references to a page are found without visiting every reference.
    xrefs_by_name: BTreeMap<String, Vec<ReferenceLocation>>,
    /// An anchor id → the references with it as their fragment.
    by_fragment: BTreeMap<String, Vec<ReferenceLocation>>,
    /// The file name an include's target ends with → the includes naming it, whether or not
    /// the index can resolve them (Antora resource ids need the catalog).
    includes_by_name: BTreeMap<String, Vec<IncludeLocation>>,
    /// Include target → the files that include it. Only targets the index can resolve on its
    /// own are recorded: relative paths and attributes the including document declares.
    includers: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    /// Each file's symbols in document order.
    symbols: BTreeMap<PathBuf, Vec<WorkspaceSymbol>>,
    /// `(word, file, position in that file's symbols)`, so a query reaches its matches by an
    /// ordered range scan rather than by visiting every symbol.
    symbol_words: BTreeSet<(String, PathBuf, usize)>,
    /// Counts the files added, replaced and removed, so a cache built from them can tell it
    /// is stale.
    revision: u64,
}

impl WorkspaceIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A number that changes whenever a file is added, replaced or removed. Only the same
    /// index's revisions compare: another index may have counted to the same number.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
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

        for reference in &document.references {
            let location = || ReferenceLocation {
                path: path.clone(),
                reference: reference.clone(),
            };
            if let Some(name) = target_file_name(reference) {
                self.xrefs_by_name
                    .entry(name.to_owned())
                    .or_default()
                    .push(location());
            }
            if let Some(fragment) = target_fragment(reference) {
                self.by_fragment
                    .entry(fragment.to_owned())
                    .or_default()
                    .push(location());
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

        for include in &document.includes {
            if let Some(name) = file_name(&include.target) {
                self.includes_by_name
                    .entry(name.to_owned())
                    .or_default()
                    .push(IncludeLocation {
                        path: path.clone(),
                        include: include.clone(),
                    });
            }
        }

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
        self.revision += 1;
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
        for reference in &removed.document.references {
            for (key, map) in [
                (target_file_name(reference), &mut self.xrefs_by_name),
                (target_fragment(reference), &mut self.by_fragment),
            ] {
                let Some(key) = key else {
                    continue;
                };
                if let Some(locations) = map.get_mut(key) {
                    locations.retain(|location| location.path != path);
                    if locations.is_empty() {
                        map.remove(key);
                    }
                }
            }
        }
        for include in &removed.document.includes {
            let Some(name) = file_name(&include.target) else {
                continue;
            };
            if let Some(locations) = self.includes_by_name.get_mut(name) {
                locations.retain(|location| location.path != path);
                if locations.is_empty() {
                    self.includes_by_name.remove(name);
                }
            }
        }
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
        let report = self.index_roots_with(roots, DEFAULT_IGNORED_DIRECTORIES, |_| {});
        match report.unreadable.first() {
            Some(path) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} could not be read", path.display()),
            )),
            None => Ok(report.files),
        }
    }

    /// Indexes every AsciiDoc file beneath `roots`, never entering a folder named in `ignore`
    /// or a link, and leaving out whatever cannot be read. `on_directory` sees each folder
    /// entered before it is read, so a watch placed there misses nothing read after. Files are
    /// read and parsed on a few threads.
    pub fn index_roots_with(
        &mut self,
        roots: &[PathBuf],
        ignore: &[impl AsRef<str>],
        mut on_directory: impl FnMut(&Path),
    ) -> IndexReport {
        let mut paths = Vec::new();
        let mut unreadable = Vec::new();
        for root in roots {
            collect_asciidoc_files(root, ignore, &mut on_directory, &mut paths, &mut unreadable);
        }
        paths.sort();
        paths.dedup();

        let threads = thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(PARSE_THREADS);
        let chunk = paths.len().div_ceil(threads).max(1);
        let parsed: Vec<_> = thread::scope(|scope| {
            let workers: Vec<_> = paths
                .chunks(chunk)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|path| {
                                let document = fs::read_to_string(path)
                                    .map(|text| parse(&path_to_uri(path), &text).document);
                                (path, document)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().expect("parsing never panics"))
                .collect()
        });
        let mut files = 0;
        for (path, document) in parsed {
            match document {
                Ok(document) => {
                    self.replace(path.clone(), document);
                    files += 1;
                }
                Err(_) => unreadable.push(path.clone()),
            }
        }
        unreadable.sort();
        IndexReport { files, unreadable }
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

    /// The xrefs whose target names a file called `file_name` (`setup.adoc`), whatever its
    /// directory or Antora coordinates. Resolving each decides which reach a given file.
    pub fn xrefs_naming(&self, file_name: &str) -> impl Iterator<Item = &ReferenceLocation> {
        self.xrefs_by_name.get(file_name).into_iter().flatten()
    }

    /// The references with `id` as their fragment: `<<id>>`, `xref:#id[]` and
    /// `xref:page.adoc#id[]`, in file order.
    pub fn references_with_fragment(&self, id: &str) -> impl Iterator<Item = &ReferenceLocation> {
        self.by_fragment.get(id).into_iter().flatten()
    }

    /// The include directives whose target names a file called `file_name`, resolvable or not.
    pub fn includes_naming(&self, file_name: &str) -> impl Iterator<Item = &IncludeLocation> {
        self.includes_by_name.get(file_name).into_iter().flatten()
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

/// The file name an xref's target ends with: `setup.adoc` for `sub/setup.adoc#ports` or
/// `admin:setup.adoc`.
fn target_file_name(reference: &Reference) -> Option<&str> {
    if reference.kind != ReferenceKind::Xref {
        return None;
    }
    let file = reference
        .target
        .split_once('#')
        .map_or(reference.target.as_str(), |(file, _)| file);
    file_name(file)
}

/// The last segment of a target: `setup.adoc` for `sub/setup.adoc` or `partial$setup.adoc`.
fn file_name(target: &str) -> Option<&str> {
    let name = target.rsplit(['/', ':', '$']).next()?.trim();
    (!name.is_empty()).then_some(name)
}

/// The anchor id a reference points at, if any.
fn target_fragment(reference: &Reference) -> Option<&str> {
    let fragment = match reference.kind {
        ReferenceKind::LocalAnchor => reference.target.as_str(),
        ReferenceKind::Xref => reference.target.split_once('#')?.1,
        ReferenceKind::Link | ReferenceKind::Attribute => return None,
    };
    let fragment = fragment.trim();
    (!fragment.is_empty()).then_some(fragment)
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
    fn the_revision_changes_with_every_file_added_replaced_or_removed() {
        let path = PathBuf::from("docs/guide.adoc");
        let mut index = WorkspaceIndex::new();
        let mut seen = vec![index.revision()];
        index.index_source(&path, "= One\n");
        seen.push(index.revision());
        index.index_source(&path, "= Two\n");
        seen.push(index.revision());
        index.remove(&path);
        seen.push(index.revision());
        seen.dedup();
        assert_eq!(seen.len(), 4);
        let _ = index.file(&path);
        assert_eq!(index.revision(), seen[3]);
    }

    #[test]
    fn indexing_enters_each_folder_kept_before_reading_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut entered = Vec::new();
        let mut index = WorkspaceIndex::new();
        let report = index.index_roots_with(std::slice::from_ref(&root), &["pages"], |folder| {
            entered.push(folder.to_path_buf());
        });

        assert!(report.unreadable.is_empty());
        assert_eq!(report.files, index.files().count());
        assert_eq!(entered.first(), Some(&root), "the root is entered first");
        assert!(entered.contains(&root.join("modules/ROOT/partials")));
        assert!(
            entered.iter().all(|folder| !folder.ends_with("pages")),
            "a folder named in the ignore list is never entered"
        );
        assert!(index.files().all(|file| !file
            .path
            .components()
            .any(|part| part.as_os_str() == "pages")));
    }

    #[test]
    fn finds_references_by_target_file_name_and_fragment() {
        let guide = PathBuf::from("docs/guide.adoc");
        let other = PathBuf::from("docs/other.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            &guide,
            "See xref:sub/setup.adoc[], xref:admin:setup.adoc#ports[] and <<intro>>.\n",
        );
        index.index_source(&other, "See xref:guide.adoc#intro[] and xref:#intro[].\n");

        let naming: Vec<_> = index
            .xrefs_naming("setup.adoc")
            .map(|location| location.reference.target.as_str())
            .collect();
        assert_eq!(naming, ["sub/setup.adoc", "admin:setup.adoc#ports"]);
        let fragment: Vec<_> = index
            .references_with_fragment("intro")
            .map(|location| (location.path.as_path(), location.reference.target.as_str()))
            .collect();
        assert_eq!(
            fragment,
            [
                (guide.as_path(), "intro"),
                (other.as_path(), "guide.adoc#intro"),
                (other.as_path(), "intro"),
            ]
        );

        // Replacing a file takes its references out of both maps.
        index.index_source(&guide, "No links.\n");
        assert_eq!(index.xrefs_naming("setup.adoc").count(), 0);
        assert_eq!(index.references_with_fragment("intro").count(), 2);
        index.remove(&other);
        assert_eq!(index.references_with_fragment("intro").count(), 0);
    }

    #[test]
    fn finds_includes_by_target_file_name() {
        let guide = PathBuf::from("docs/guide.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            &guide,
            "include::partial$notes/token.adoc[]\n\ninclude::{dir}/token.adoc[]\n",
        );

        let targets: Vec<_> = index
            .includes_naming("token.adoc")
            .map(|location| (location.path.as_path(), location.include.target.as_str()))
            .collect();
        assert_eq!(
            targets,
            [
                (guide.as_path(), "partial$notes/token.adoc"),
                (guide.as_path(), "{dir}/token.adoc"),
            ]
        );
        index.remove(&guide);
        assert_eq!(index.includes_naming("token.adoc").count(), 0);
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
