use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    ops::Bound,
    path::{Path, PathBuf},
    sync::Arc,
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

/// Where an anchor is declared, borrowed from the index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnchorLocation<'a> {
    pub path: &'a Path,
    pub range: SourceRange,
}

/// A reference and the file it is in, borrowed from the indexed document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReferenceLocation<'a> {
    pub path: &'a Path,
    pub reference: &'a Reference,
}

/// An include directive and the file it is in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncludeLocation {
    pub path: Arc<Path>,
    pub include: IncludeDirective,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub uri: String,
    pub document: Document,
}

/// A reference by the file it is in and its place among that file's references.
type ReferenceAt = (Arc<Path>, usize);

/// Every table is keyed by file, or holds the file's one shared path, so a file is removed by
/// visiting only what it contributed.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceIndex {
    files: BTreeMap<Arc<Path>, FileEntry>,
    /// Each file's anchors by id, where each is first declared: its declared anchors and its
    /// sections' generated ids.
    anchors: BTreeMap<Arc<Path>, BTreeMap<String, SourceRange>>,
    /// The file name an xref's target ends with (`setup.adoc`) → the xrefs naming it, so the
    /// references to a page are found without visiting every reference.
    xrefs_by_name: BTreeMap<String, Vec<ReferenceAt>>,
    /// An anchor id → the references with it as their fragment.
    by_fragment: BTreeMap<String, Vec<ReferenceAt>>,
    /// The file name an include's target ends with → the includes naming it, whether or not
    /// the index can resolve them (Antora resource ids need the catalog).
    includes_by_name: BTreeMap<String, Vec<IncludeLocation>>,
    /// Include target → the files that include it. Only targets the index can resolve on its
    /// own are recorded: relative paths and attributes the including document declares.
    includers: BTreeMap<PathBuf, BTreeSet<Arc<Path>>>,
    /// Each file's symbols in document order.
    symbols: BTreeMap<Arc<Path>, Vec<WorkspaceSymbol>>,
    /// Each word of a symbol's name → the files with a symbol holding it, so a query reaches
    /// the files that could match by an ordered range scan rather than by visiting every
    /// symbol. A file is listed once per word, however many of its symbols hold it.
    symbol_words: BTreeMap<String, Vec<Arc<Path>>>,
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
        let shared: Arc<Path> = Arc::from(path.as_path());

        let anchors = self.anchors.entry(shared.clone()).or_default();
        let mut anchor = |id: String, range: SourceRange| {
            anchors.entry(id).or_insert(range);
        };
        for declared in &document.anchors {
            anchor(declared.id.clone(), declared.range);
        }
        // Sections are reachable without an explicit anchor, so register the ids Asciidoctor
        // generates for them. They are deliberately not added to `document.anchors`, which
        // stays the record of explicitly declared anchors for duplicate detection.
        for section in &document.sections {
            for id in section.implicit_ids() {
                anchor(id, section.selection_range);
            }
        }

        for (position, reference) in document.references.iter().enumerate() {
            if let Some(name) = target_file_name(reference) {
                self.xrefs_by_name
                    .entry(name.to_owned())
                    .or_default()
                    .push((shared.clone(), position));
            }
            if let Some(fragment) = target_fragment(reference) {
                self.by_fragment
                    .entry(fragment.to_owned())
                    .or_default()
                    .push((shared.clone(), position));
            }
        }

        for include in &document.includes {
            if let Some(name) = file_name(&include.target) {
                self.includes_by_name
                    .entry(name.to_owned())
                    .or_default()
                    .push(IncludeLocation {
                        path: shared.clone(),
                        include: include.clone(),
                    });
            }
        }

        for target in include_targets(&path, &document) {
            self.includers
                .entry(target)
                .or_default()
                .insert(shared.clone());
        }

        let symbols = document_symbols(&shared, &document);
        for word in symbol_words(&symbols) {
            self.symbol_words
                .entry(word)
                .or_default()
                .push(shared.clone());
        }
        self.symbols.insert(shared.clone(), symbols);

        let uri = document.uri.clone();
        self.files.entry(shared).or_insert(FileEntry {
            path,
            uri,
            document,
        })
    }

    pub fn remove(&mut self, path: &Path) -> Option<FileEntry> {
        self.revision += 1;
        let path = normalize_path(path);
        let path = path.as_path();
        self.anchors.remove(path);
        if let Some((file, symbols)) = self.symbols.remove_entry(path) {
            for word in symbol_words(&symbols) {
                if let Some(files) = self.symbol_words.get_mut(&word) {
                    // Every table shares the file's one path, so it is told apart by address.
                    files.retain(|listed| !Arc::ptr_eq(listed, &file));
                    if files.is_empty() {
                        self.symbol_words.remove(&word);
                    }
                }
            }
        }
        let removed = self.files.remove(path)?;
        for reference in &removed.document.references {
            for (key, map) in [
                (target_file_name(reference), &mut self.xrefs_by_name),
                (target_fragment(reference), &mut self.by_fragment),
            ] {
                let Some(key) = key else {
                    continue;
                };
                if let Some(locations) = map.get_mut(key) {
                    locations.retain(|(file, _)| **file != *path);
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
                locations.retain(|location| *location.path != *path);
                if locations.is_empty() {
                    self.includes_by_name.remove(name);
                }
            }
        }
        // Resolution is a pure function of the text and path, so this yields exactly the
        // targets `replace` recorded.
        for target in include_targets(path, &removed.document) {
            if let Some(includers) = self.includers.get_mut(&target) {
                includers.remove(path);
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
        self.files.get(normalize_path(path).as_path())
    }

    pub fn files(&self) -> impl Iterator<Item = &FileEntry> {
        self.files.values()
    }

    /// Every reference, file by file in path order.
    pub fn references(&self) -> impl Iterator<Item = ReferenceLocation<'_>> {
        self.files.iter().flat_map(|(path, file)| {
            file.document
                .references
                .iter()
                .map(|reference| ReferenceLocation { path, reference })
        })
    }

    /// The xrefs whose target names a file called `file_name` (`setup.adoc`), whatever its
    /// directory or Antora coordinates. Resolving each decides which reach a given file.
    pub fn xrefs_naming(&self, file_name: &str) -> impl Iterator<Item = ReferenceLocation<'_>> {
        self.references_at(self.xrefs_by_name.get(file_name))
    }

    /// The references with `id` as their fragment: `<<id>>`, `xref:#id[]` and
    /// `xref:page.adoc#id[]`, in file order.
    pub fn references_with_fragment(
        &self,
        id: &str,
    ) -> impl Iterator<Item = ReferenceLocation<'_>> {
        self.references_at(self.by_fragment.get(id))
    }

    fn references_at<'a>(
        &'a self,
        places: Option<&'a Vec<ReferenceAt>>,
    ) -> impl Iterator<Item = ReferenceLocation<'a>> {
        places.into_iter().flatten().filter_map(|(path, position)| {
            let reference = self.files.get(path)?.document.references.get(*position)?;
            Some(ReferenceLocation { path, reference })
        })
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
            .map(AsRef::as_ref)
    }

    /// Symbols with a word starting with each word of `query`, in file and document order.
    ///
    /// An empty query finds nothing rather than every symbol in the workspace.
    #[must_use]
    pub fn symbols_matching(&self, query: &str) -> Vec<&WorkspaceSymbol> {
        let wanted: Vec<String> = words(query).collect();
        // The files with a word starting with each word of the query, fewest first: only the
        // files in all of them can hold a match.
        let mut candidates: Vec<BTreeSet<&Path>> = wanted
            .iter()
            .map(|wanted| {
                self.symbol_words
                    .range::<str, _>((Bound::Included(wanted.as_str()), Bound::Unbounded))
                    .take_while(|(word, _)| word.starts_with(wanted.as_str()))
                    .flat_map(|(_, files)| files.iter().map(|file| &**file))
                    .collect()
            })
            .collect();
        candidates.sort_by_key(BTreeSet::len);
        let Some((fewest, others)) = candidates.split_first() else {
            return Vec::new();
        };
        fewest
            .iter()
            .filter(|file| others.iter().all(|files| files.contains(*file)))
            .filter_map(|file| self.symbols.get(*file))
            .flatten()
            .filter(|symbol| {
                wanted
                    .iter()
                    .all(|wanted| words(&symbol.name).any(|word| word.starts_with(wanted.as_str())))
            })
            .collect()
    }

    #[must_use]
    pub fn resolve_anchor(&self, path: &Path, id: &str) -> Option<AnchorLocation<'_>> {
        let (path, anchors) = self.anchors.get_key_value(normalize_path(path).as_path())?;
        let range = anchors
            .get(id)
            .or_else(|| anchors.get(&canonical_id(id)))
            .or_else(|| anchors.get(&alphanumeric_id(id)))?;
        Some(AnchorLocation {
            path,
            range: *range,
        })
    }
}

/// The distinct words of the names of `symbols`.
fn symbol_words(symbols: &[WorkspaceSymbol]) -> BTreeSet<String> {
    symbols
        .iter()
        .flat_map(|symbol| words(&symbol.name))
        .collect()
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
        assert_eq!(index.references().count(), 1);

        index.index_source(&path, "[[new]]\n== New\n");

        assert!(index.resolve_anchor(&path, "old").is_none());
        assert!(index.resolve_anchor(&path, "new").is_some());
        assert_eq!(index.references().count(), 0);
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
            .map(|location| (location.path, location.reference.target.as_str()))
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
            .map(|location| (&*location.path, location.include.target.as_str()))
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

    #[test]
    fn every_query_word_must_match_within_one_symbol() {
        let mut index = WorkspaceIndex::new();
        index.index_source("docs/b.adoc", "== Alpha Two\n\n== Beta\n");
        index.index_source("docs/a.adoc", "== Alpha One\n\n== Alpha Beta\n");

        // `b.adoc` has both words, but in different sections.
        assert_eq!(names(index.symbols_matching("alpha beta")), ["Alpha Beta"]);
        // In file order, then document order.
        assert_eq!(
            names(index.symbols_matching("alpha")),
            ["Alpha One", "Alpha Beta", "Alpha Two"]
        );
    }

    #[test]
    fn removing_a_file_leaves_what_the_others_share_with_it() {
        let mut index = WorkspaceIndex::new();
        let text =
            "[[shared]]\n== Common Ground\n\nSee xref:target.adoc#shared[] and <<shared>>.\n";
        index.index_source("docs/one.adoc", text);
        index.index_source("docs/two.adoc", text);

        index.remove(Path::new("docs/one.adoc"));
        let files = |found: Vec<&Path>| -> Vec<PathBuf> {
            found.into_iter().map(Path::to_path_buf).collect()
        };
        assert_eq!(
            files(
                index
                    .symbols_matching("common")
                    .iter()
                    .map(|symbol| &*symbol.path)
                    .collect()
            ),
            [PathBuf::from("docs/two.adoc")]
        );
        assert_eq!(
            files(
                index
                    .xrefs_naming("target.adoc")
                    .map(|location| location.path)
                    .collect()
            ),
            [PathBuf::from("docs/two.adoc")]
        );
        assert_eq!(index.references_with_fragment("shared").count(), 2);
        assert!(index
            .resolve_anchor(Path::new("docs/one.adoc"), "shared")
            .is_none());
        assert!(index
            .resolve_anchor(Path::new("docs/two.adoc"), "shared")
            .is_some());
    }

    #[test]
    fn an_anchor_declared_twice_resolves_to_its_first_declaration() {
        let mut index = WorkspaceIndex::new();
        let text = "[[twice]]\nFirst.\n\n[[twice]]\nSecond.\n";
        index.index_source("docs/page.adoc", text);
        let location = index
            .resolve_anchor(Path::new("docs/page.adoc"), "twice")
            .unwrap();
        assert_eq!(location.range.start, 0);
    }
}
