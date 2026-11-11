use std::{
    borrow::Cow,
    collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet},
    fs,
    hash::{Hash, Hasher},
    io,
    ops::Bound,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use adoc_core::{
    alphanumeric_id, canonical_id, section_ids, Document, IdStyle, IncludeDirective, Reference,
    ReferenceKind, SourceRange,
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

/// An indexed file. A document indexed from disk does not keep its text, which is most of
/// what the index would otherwise hold: [`FileEntry::text`] reads it back. One given to
/// [`WorkspaceIndex::replace`], an open page's, keeps it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub uri: String,
    /// The parse; its `text` is empty for a file indexed from disk.
    pub document: Document,
    /// The length and hash of the text parsed, so text read back is known to be that text.
    text_len: usize,
    text_hash: u64,
}

impl FileEntry {
    /// The text the document was parsed from: the one kept, else the file's, read again.
    /// `None` when the file no longer holds that text, so offsets into the parse would not
    /// fit it, or cannot be read.
    #[must_use]
    pub fn text(&self) -> Option<Cow<'_, str>> {
        if self.document.text.len() == self.text_len {
            return Some(Cow::Borrowed(&self.document.text));
        }
        let text = fs::read_to_string(&self.path).ok()?;
        (text.len() == self.text_len && hash_text(&text) == self.text_hash)
            .then_some(Cow::Owned(text))
    }
}

/// A parse, and the length and hash of its text, which it may or may not keep.
#[derive(Debug)]
struct Parsed {
    document: Document,
    text_len: usize,
    text_hash: u64,
}

impl Parsed {
    fn keeping_text(document: Document) -> Self {
        Self {
            text_len: document.text.len(),
            text_hash: hash_text(&document.text),
            document,
        }
    }

    fn without_text(mut document: Document) -> Self {
        let text = std::mem::take(&mut document.text);
        Self {
            text_len: text.len(),
            text_hash: hash_text(&text),
            document,
        }
    }
}

impl FileEntry {
    fn of(path: PathBuf, parsed: Parsed) -> Self {
        Self {
            uri: parsed.document.uri.clone(),
            path,
            document: parsed.document,
            text_len: parsed.text_len,
            text_hash: parsed.text_hash,
        }
    }
}

/// An open page's entry, made ready away from the index (on a worker, say) for
/// [`WorkspaceIndex::insert_prepared`]: everything in it that needs no other file, the parse,
/// its anchors and generated section ids, its symbols and their words, and its includes'
/// targets, so the index itself only files it.
#[derive(Debug)]
pub struct PreparedFile {
    path: PathBuf,
    shared: Arc<Path>,
    parsed: Parsed,
    anchors: BTreeMap<String, SourceRange>,
    include_targets: Vec<PathBuf>,
    symbols: Vec<WorkspaceSymbol>,
    words: BTreeSet<String>,
}

impl PreparedFile {
    /// Prepares `document`, the parse of an open page, keeping its text as
    /// [`WorkspaceIndex::replace`] does. `shared` is the path the index shares for the file
    /// ([`WorkspaceIndex::shared_path`]): with it, inserting visits only what changed.
    #[must_use]
    pub fn new(path: &Path, shared: Option<Arc<Path>>, document: Document) -> Self {
        Self::from_parsed(normalize_path(path), shared, Parsed::keeping_text(document))
    }

    /// The parse.
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.parsed.document
    }

    fn from_parsed(path: PathBuf, shared: Option<Arc<Path>>, parsed: Parsed) -> Self {
        let shared = shared
            .filter(|shared| **shared == *path)
            .unwrap_or_else(|| Arc::from(path.as_path()));
        let document = &parsed.document;
        let mut anchors = BTreeMap::new();
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
        // A generated id already in use is numbered (`_parameters_2`, `parameters-2`).
        for style in [IdStyle::Antora, IdStyle::Asciidoctor] {
            let ids = section_ids(&document.sections, &document.anchors, style);
            for (section, id) in document.sections.iter().zip(ids) {
                if section.id.is_none() && !id.is_empty() {
                    anchor(id, section.selection_range);
                }
            }
        }
        let include_targets = include_targets_of(&path, document);
        let symbols = document_symbols(&shared, document);
        let words = symbol_words(&symbols);
        Self {
            path,
            shared,
            parsed,
            anchors,
            include_targets,
            symbols,
            words,
        }
    }
}

/// What [`WorkspaceIndex::insert_prepared`] took out of the index. A large page's entry is
/// thousands of small allocations, slow to free: drop it where that costs nothing, on a worker.
#[derive(Debug, Default)]
#[allow(dead_code)] // Held only to be dropped.
pub struct Retired {
    entry: Option<FileEntry>,
    anchors: Option<BTreeMap<String, SourceRange>>,
    symbols: Vec<WorkspaceSymbol>,
    words: BTreeSet<String>,
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

    /// Indexes the text of a file read from disk, keeping its parse but not its text.
    pub fn index_file(&mut self, path: impl AsRef<Path>, text: &str) -> &FileEntry {
        let path = normalize_path(path.as_ref());
        let uri = path_to_uri(&path);
        let document = parse(&uri, text).document;
        self.replace_from_disk(path, document)
    }

    /// Replaces the entry for `path` with `document`, keeping its text: an open page's, whose
    /// text is not the file's.
    pub fn replace(&mut self, path: PathBuf, document: Document) -> &FileEntry {
        self.insert(path, Parsed::keeping_text(document))
    }

    /// Replaces the entry for `path` with `document`, parsed from the file, without keeping
    /// its text: [`FileEntry::text`] reads it back.
    pub fn replace_from_disk(&mut self, path: PathBuf, document: Document) -> &FileEntry {
        self.insert(path, Parsed::without_text(document))
    }

    /// Lets the entry for `path` drop the text it keeps, when its parse is of `text`, the file's
    /// on disk: a page closed unchanged keeps its parse, its text read back like any file's,
    /// without being parsed again. False, changing nothing, when the parse is of other text or
    /// there is no entry.
    pub fn keep_from_disk(&mut self, path: &Path, text: &str) -> bool {
        let Some(entry) = self.files.get_mut(normalize_path(path).as_path()) else {
            return false;
        };
        if entry.text_len != text.len() || entry.text_hash != hash_text(text) {
            return false;
        }
        entry.document.text = Arc::default();
        true
    }

    /// The path the index shares between the tables of the file at `path`, to prepare its next
    /// entry with ([`PreparedFile::new`]).
    #[must_use]
    pub fn shared_path(&self, path: &Path) -> Option<Arc<Path>> {
        self.files
            .get_key_value(normalize_path(path).as_path())
            .map(|(shared, _)| shared.clone())
    }

    /// Replaces a file's entry with one prepared elsewhere, as [`WorkspaceIndex::replace`]
    /// would. Prepared against the path the index still shares for the file, only the parts of
    /// the tables whose keys changed are visited: typing in prose moves every range after it but
    /// changes no target, word or include. Returns what it took out, to be dropped elsewhere.
    pub fn insert_prepared(&mut self, prepared: PreparedFile) -> Retired {
        let current = self
            .files
            .get_key_value(prepared.path.as_path())
            .is_some_and(|(shared, _)| Arc::ptr_eq(shared, &prepared.shared));
        if current {
            self.update(prepared)
        } else {
            let entry = self.remove(&prepared.path);
            self.add(prepared);
            Retired {
                entry,
                ..Retired::default()
            }
        }
    }

    fn insert(&mut self, path: PathBuf, parsed: Parsed) -> &FileEntry {
        let path = normalize_path(&path);
        self.remove(&path);
        self.add(PreparedFile::from_parsed(path, None, parsed))
    }

    /// Adds a file the index does not hold.
    fn add(&mut self, prepared: PreparedFile) -> &FileEntry {
        let PreparedFile {
            path,
            shared,
            parsed,
            anchors,
            include_targets,
            symbols,
            words,
        } = prepared;
        let document = &parsed.document;
        self.anchors.insert(shared.clone(), anchors);
        self.file_references(&shared, &document.references);
        self.file_includes(&shared, &document.includes);
        self.file_includers(&shared, include_targets);
        self.file_words(&shared, words);
        self.symbols.insert(shared.clone(), symbols);
        self.files
            .entry(shared)
            .or_insert(FileEntry::of(path, parsed))
    }

    /// Replaces the entry of a file whose tables share `prepared.shared`, visiting only the keys
    /// that changed.
    fn update(&mut self, prepared: PreparedFile) -> Retired {
        self.revision += 1;
        let PreparedFile {
            path,
            shared,
            parsed,
            anchors,
            include_targets,
            symbols,
            words,
        } = prepared;
        let old = self
            .files
            .remove(path.as_path())
            .expect("an entry prepared against its shared path");
        let document = &parsed.document;
        let old_anchors = self.anchors.insert(shared.clone(), anchors);

        let (before, after) = (&old.document.references, &document.references);
        if before.len() == after.len() {
            // Each reference keeps its place, so only the keys of those whose target changed
            // need their lists rebuilt.
            let mut names = BTreeSet::new();
            let mut fragments = BTreeSet::new();
            for (was, now) in before.iter().zip(after) {
                if was.kind != now.kind || was.target != now.target {
                    names.extend(
                        target_file_name(was)
                            .into_iter()
                            .chain(target_file_name(now)),
                    );
                    fragments.extend(target_fragment(was).into_iter().chain(target_fragment(now)));
                }
            }
            refile(
                &mut self.xrefs_by_name,
                &shared,
                &names,
                after,
                target_file_name,
            );
            refile(
                &mut self.by_fragment,
                &shared,
                &fragments,
                after,
                target_fragment,
            );
        } else {
            self.unfile_references(&shared, before);
            self.file_references(&shared, after);
        }

        // A page has few includes, and their places are kept with them: always filed again.
        self.unfile_includes(&shared, &old.document.includes);
        self.file_includes(&shared, &document.includes);
        self.unfile_includers(&shared, include_targets_of(&path, &old.document));
        self.file_includers(&shared, include_targets);

        let old_symbols = self.symbols.remove(path.as_path()).unwrap_or_default();
        let same_names = old_symbols.len() == symbols.len()
            && old_symbols
                .iter()
                .zip(&symbols)
                .all(|(was, now)| was.name == now.name);
        let words = if same_names {
            words
        } else {
            self.unfile_words(&shared, symbol_words(&old_symbols));
            self.file_words(&shared, words);
            BTreeSet::new()
        };
        self.symbols.insert(shared.clone(), symbols);
        self.files.insert(shared, FileEntry::of(path, parsed));
        Retired {
            entry: Some(old),
            anchors: old_anchors,
            symbols: old_symbols,
            words,
        }
    }

    fn file_references(&mut self, shared: &Arc<Path>, references: &[Reference]) {
        for (position, reference) in references.iter().enumerate() {
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
    }

    fn unfile_references(&mut self, shared: &Arc<Path>, references: &[Reference]) {
        for reference in references {
            for (key, map) in [
                (target_file_name(reference), &mut self.xrefs_by_name),
                (target_fragment(reference), &mut self.by_fragment),
            ] {
                if let Some(key) = key {
                    unlist(map, key, |(file, _)| Arc::ptr_eq(file, shared));
                }
            }
        }
    }

    fn file_includes(&mut self, shared: &Arc<Path>, includes: &[IncludeDirective]) {
        for include in includes {
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
    }

    fn unfile_includes(&mut self, shared: &Arc<Path>, includes: &[IncludeDirective]) {
        for include in includes {
            if let Some(name) = file_name(&include.target) {
                unlist(&mut self.includes_by_name, name, |location| {
                    Arc::ptr_eq(&location.path, shared)
                });
            }
        }
    }

    fn file_includers(&mut self, shared: &Arc<Path>, targets: impl IntoIterator<Item = PathBuf>) {
        for target in targets {
            self.includers
                .entry(target)
                .or_default()
                .insert(shared.clone());
        }
    }

    fn unfile_includers(&mut self, shared: &Path, targets: impl IntoIterator<Item = PathBuf>) {
        for target in targets {
            if let Some(includers) = self.includers.get_mut(&target) {
                includers.remove(shared);
                if includers.is_empty() {
                    self.includers.remove(&target);
                }
            }
        }
    }

    fn file_words(&mut self, shared: &Arc<Path>, words: BTreeSet<String>) {
        for word in words {
            self.symbol_words
                .entry(word)
                .or_default()
                .push(shared.clone());
        }
    }

    fn unfile_words(&mut self, shared: &Arc<Path>, words: BTreeSet<String>) {
        for word in words {
            // Every table shares the file's one path, so it is told apart by address.
            unlist(&mut self.symbol_words, &word, |listed| {
                Arc::ptr_eq(listed, shared)
            });
        }
    }

    pub fn remove(&mut self, path: &Path) -> Option<FileEntry> {
        self.revision += 1;
        let path = normalize_path(path);
        let (shared, removed) = self.files.remove_entry(path.as_path())?;
        self.anchors.remove(path.as_path());
        if let Some(symbols) = self.symbols.remove(path.as_path()) {
            self.unfile_words(&shared, symbol_words(&symbols));
        }
        self.unfile_references(&shared, &removed.document.references);
        self.unfile_includes(&shared, &removed.document.includes);
        // Resolution is a pure function of the text and path, so this yields exactly the
        // targets `replace` recorded.
        self.unfile_includers(&shared, include_targets_of(&path, &removed.document));
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
            collect_asciidoc_files(
                root,
                root,
                ignore,
                &mut on_directory,
                &mut paths,
                &mut unreadable,
            );
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
                                let parsed = fs::read_to_string(path).map(|text| {
                                    Parsed::without_text(parse(&path_to_uri(path), &text).document)
                                });
                                (path, parsed)
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
        for (path, parsed) in parsed {
            match parsed {
                Ok(parsed) => {
                    self.insert(path.clone(), parsed);
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
        self.anchors_in(&normalize_path(path))?.resolve(id)
    }

    /// The anchors of the indexed file at `path`, already normalised, to resolve many ids in.
    /// `None` when the file is not indexed.
    pub(crate) fn anchors_in(&self, path: &Path) -> Option<Anchors<'_>> {
        let (path, anchors) = self.anchors.get_key_value(path)?;
        Some(Anchors { path, anchors })
    }
}

/// One indexed file's anchors, borrowed from the index.
#[derive(Clone, Copy)]
pub(crate) struct Anchors<'a> {
    path: &'a Path,
    anchors: &'a BTreeMap<String, SourceRange>,
}

impl<'a> Anchors<'a> {
    /// Where `id` is declared, as [`WorkspaceIndex::resolve_anchor`] finds it.
    pub(crate) fn resolve(self, id: &str) -> Option<AnchorLocation<'a>> {
        let anchors = self.anchors;
        let range = anchors
            .get(id)
            .or_else(|| anchors.get(&canonical_id(id)))
            .or_else(|| anchors.get(&alphanumeric_id(id)))?;
        Some(AnchorLocation {
            path: self.path,
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

fn include_targets_of(path: &Path, document: &Document) -> Vec<PathBuf> {
    document
        .includes
        .iter()
        .filter_map(|include| resolve_include_target(document, path, &include.target))
        .collect()
}

/// Takes the items `of_file` picks out of the list at `key`, and the list once empty.
fn unlist<T>(map: &mut BTreeMap<String, Vec<T>>, key: &str, of_file: impl Fn(&T) -> bool) {
    if let Some(list) = map.get_mut(key) {
        list.retain(|item| !of_file(item));
        if list.is_empty() {
            map.remove(key);
        }
    }
}

/// Lists again, for each of `keys`, the references of `file` that `key_of` files under it, in
/// their order, after the other files' references.
fn refile(
    map: &mut BTreeMap<String, Vec<ReferenceAt>>,
    file: &Arc<Path>,
    keys: &BTreeSet<&str>,
    references: &[Reference],
    key_of: fn(&Reference) -> Option<&str>,
) {
    if keys.is_empty() {
        return;
    }
    let mut places: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (position, reference) in references.iter().enumerate() {
        if let Some(key) = key_of(reference).filter(|key| keys.contains(key)) {
            places.entry(key).or_default().push(position);
        }
    }
    for key in keys {
        unlist(map, key, |(listed, _)| Arc::ptr_eq(listed, file));
        if let Some(positions) = places.remove(key) {
            map.entry((*key).to_owned()).or_default().extend(
                positions
                    .into_iter()
                    .map(|position| (file.clone(), position)),
            );
        }
    }
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

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn path_to_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };

    use super::{PreparedFile, WorkspaceIndex};

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
    fn a_build_folder_inside_a_module_is_indexed() {
        // Issue #531: `build` is build output beside a component, but a topic folder in a
        // module.
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/ignored-folders");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let page = super::normalize_path(&root.join("docs/modules/ROOT/pages/build/index.adoc"));
        assert!(index.file(&page).is_some(), "the page in pages/build");
        assert!(index.resolve_anchor(&page, "tools").is_some());
        let stray = super::normalize_path(&root.join("build/stray.adoc"));
        assert!(index.file(&stray).is_none(), "build output beside it");
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

    #[test]
    fn a_file_indexed_from_disk_reads_its_text_back_while_it_is_unchanged() {
        let directory =
            std::env::temp_dir().join(format!("adoc-index-text-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("page.adoc");
        std::fs::write(&path, "= Page\n\nOn disk.\n").unwrap();
        let mut index = WorkspaceIndex::new();
        index.index_file(&path, "= Page\n\nOn disk.\n");

        let entry = index.file(&path).unwrap();
        assert!(entry.document.text.is_empty(), "not kept");
        assert_eq!(entry.document.title.as_ref().unwrap().text, "Page");
        assert_eq!(entry.text().as_deref(), Some("= Page\n\nOn disk.\n"));

        // Changed since, the file's text no longer fits the parse's offsets.
        std::fs::write(&path, "= Page\n\nOn disk, changed.\n").unwrap();
        assert_eq!(index.file(&path).unwrap().text(), None);

        // An open page's text is kept, whatever the file holds.
        let open = adoc_parser::parse("file:///page.adoc", "= Open\n").document;
        index.replace(path.clone(), open);
        let entry = index.file(&path).unwrap();
        assert!(matches!(
            entry.text(),
            Some(std::borrow::Cow::Borrowed("= Open\n"))
        ));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// The index's tables as text, each list in file order, so two indexes holding the same
    /// entries compare equal whatever order the files were replaced in.
    fn tables(index: &WorkspaceIndex) -> String {
        fn by_file<T: std::fmt::Debug>(
            lists: &BTreeMap<String, Vec<T>>,
            path: impl Fn(&T) -> &Path,
        ) -> String {
            let lists: BTreeMap<_, _> = lists
                .iter()
                .map(|(key, list)| {
                    let mut list: Vec<_> = list.iter().collect();
                    list.sort_by_key(|item| path(item).to_path_buf());
                    (key, list)
                })
                .collect();
            format!("{lists:?}")
        }
        format!(
            "files {:?}\nanchors {:?}\nxrefs {}\nfragments {}\nincludes {}\nincluders {:?}\nsymbols {:?}\nwords {}",
            index.files,
            index.anchors,
            by_file(&index.xrefs_by_name, |(path, _)| path),
            by_file(&index.by_fragment, |(path, _)| path),
            by_file(&index.includes_by_name, |location| &location.path),
            index.includers,
            index.symbols,
            by_file(&index.symbol_words, |path| path),
        )
    }

    #[test]
    fn a_file_prepared_elsewhere_indexes_as_replacing_it_does() {
        let page = PathBuf::from("docs/page.adoc");
        let other = PathBuf::from("docs/other.adoc");
        let other_text =
            "= Other\n\n[[shared]]\n== Shared\n\nSee xref:page.adoc#intro[] and <<shared>>.\n";
        let texts = [
            // The first, then prose typed: every range after it moves, no target changes.
            "= Page\n:part: part.adoc\n\n[[intro]]\n== Intro\n\nSee xref:other.adoc#shared[], <<intro>> and xref:other.adoc[].\n\ninclude::{part}[]\n\n== Usage\n\nimage::shot.png[]\n",
            "= Page\n:part: part.adoc\n\n[[intro]]\n== Intro\n\nTyped. See xref:other.adoc#shared[], <<intro>> and xref:other.adoc[].\n\ninclude::{part}[]\n\n== Usage\n\nimage::shot.png[]\n",
            // One target changed, the count of references kept.
            "= Page\n:part: part.adoc\n\n[[intro]]\n== Intro\n\nTyped. See xref:other.adoc#gone[], <<intro>> and xref:other.adoc[].\n\ninclude::{part}[]\n\n== Usage\n\nimage::shot.png[]\n",
            // Another file named, the count of references kept.
            "= Page\n:part: part.adoc\n\n[[intro]]\n== Intro\n\nTyped. See xref:other.adoc#gone[], <<intro>> and xref:another.adoc[].\n\ninclude::{part}[]\n\n== Usage\n\nimage::shot.png[]\n",
            // A reference added, so every later one moves along.
            "= Page\n:part: part.adoc\n\n[[intro]]\n== Intro\n\n<<usage>> first. See xref:other.adoc#gone[], <<intro>> and xref:other.adoc[].\n\ninclude::{part}[]\n\n== Usage\n\nimage::shot.png[]\n",
            // A heading renamed, so its symbol's words and generated ids change.
            "= Page\n:part: part.adoc\n\n[[intro]]\n== Introduction\n\n<<usage>> first. See xref:other.adoc#gone[], <<intro>> and xref:other.adoc[].\n\ninclude::{part}[]\n\n== Usage\n\nimage::shot.png[]\n",
            // The include and the attribute it names changed.
            "= Page\n:part: other-part.adoc\n\n[[intro]]\n== Introduction\n\n<<usage>> first. See xref:other.adoc#gone[], <<intro>> and xref:other.adoc[].\n\ninclude::{part}[]\ninclude::partial$extra.adoc[]\n\n== Usage\n\nimage::shot.png[]\n",
            // Everything gone.
            "",
        ];
        let parse = |text: &str| adoc_parser::parse("file:///docs/page.adoc", text).document;

        let mut replaced = WorkspaceIndex::new();
        let mut prepared = WorkspaceIndex::new();
        for index in [&mut replaced, &mut prepared] {
            index.index_source(&other, other_text);
        }
        for text in texts {
            replaced.replace(page.clone(), parse(text));
            let revision = prepared.revision();
            let file = PreparedFile::new(&page, prepared.shared_path(&page), parse(text));
            prepared.insert_prepared(file);
            assert!(prepared.revision() > revision);
            assert_eq!(tables(&prepared), tables(&replaced), "after {text:?}");
        }

        // Prepared against a path the index has since dropped: indexed whole, as replacing is.
        let stale = prepared.shared_path(&page);
        for index in [&mut replaced, &mut prepared] {
            index.remove(&page);
            index.index_source(&page, texts[0]);
        }
        replaced.replace(page.clone(), parse(texts[1]));
        prepared.insert_prepared(PreparedFile::new(&page, stale, parse(texts[1])));
        assert_eq!(tables(&prepared), tables(&replaced));

        for index in [&mut replaced, &mut prepared] {
            index.remove(&page);
            index.remove(&other);
        }
        assert_eq!(tables(&prepared), tables(&WorkspaceIndex::new()));
    }

    #[test]
    fn indexing_roots_keeps_no_text() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/includes");
        let mut index = WorkspaceIndex::new();
        index.index_roots(&[root]).unwrap();
        assert!(index.files().count() > 1);
        for file in index.files() {
            assert!(file.document.text.is_empty(), "{}", file.path.display());
            assert_eq!(
                file.text().as_deref(),
                Some(std::fs::read_to_string(&file.path).unwrap().as_str())
            );
        }
    }
}
