//! Spell checking: the prose of each open AsciiDoc page, read from the visual editor's model,
//! checked on a worker once typing has paused for [`SPELLING_SETTLE`]. Between checks the
//! misspellings found move with each edit, and those an edit touches are dropped.

use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_model::{Access, Mark, Model, Piece};
use adoc_spell::{Dictionaries, WordList};

use crate::{workspace::WorkerMessage, DocumentId, Engine, EngineEvent};

/// How long a page must go unchanged before it is checked again.
pub const SPELLING_SETTLE: Duration = Duration::from_millis(500);

/// Suggestions are found for at most this many distinct words of a page.
pub const MAX_SUGGESTED_WORDS: usize = 100;

/// The code front ends give spelling hints, apart from the engine's diagnostics.
pub const SPELLING_CODE: &str = "adoc.spelling";

/// A word the dictionary and word lists do not know, where it is, and what might replace it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Misspelling {
    pub range: SourceRange,
    pub word: String,
    pub suggestions: Vec<String>,
}

/// A document's last check: the version and word lists it checked, and what it found.
struct Checked {
    version: u64,
    generation: u64,
    misspellings: Vec<Misspelling>,
}

pub(crate) struct Spelling {
    enabled: bool,
    pub(crate) dictionaries: Arc<Dictionaries>,
    /// The folder `dictionaries` was listed from.
    folder: Option<PathBuf>,
    pub(crate) user_words: Arc<WordList>,
    /// Each workspace root's list.
    pub(crate) workspace_words: BTreeMap<PathBuf, Arc<WordList>>,
    /// Bumped whenever a word list, the dictionaries or the language change, so every page is
    /// checked again.
    pub(crate) generation: u64,
    results: BTreeMap<DocumentId, Checked>,
    /// When each document last changed; one never changed may be checked at once.
    changed: BTreeMap<DocumentId, Instant>,
    /// The document, version and generation being checked on a worker.
    checking: Option<(DocumentId, u64, u64)>,
}

impl Default for Spelling {
    fn default() -> Self {
        Self {
            enabled: false,
            dictionaries: Arc::new(Dictionaries::new(None)),
            folder: None,
            user_words: Arc::default(),
            workspace_words: BTreeMap::new(),
            generation: 0,
            results: BTreeMap::new(),
            changed: BTreeMap::new(),
            checking: None,
        }
    }
}

impl Engine {
    /// Checks the spelling of open pages. Only the app and the language server show it; the
    /// other front ends never parse a page for it.
    pub fn enable_spelling(&mut self) {
        self.spelling.enabled = true;
        self.spelling_settings_changed();
    }

    fn spelling_on(&self) -> bool {
        self.spelling.enabled && self.settings.spelling.enabled
    }

    /// The misspellings last found in document `id`, moved with the edits made since.
    #[must_use]
    pub fn misspellings(&self, id: DocumentId) -> &[Misspelling] {
        if !self.spelling_on() {
            return &[];
        }
        self.spelling
            .results
            .get(&id)
            .map_or(&[], |checked| checked.misspellings.as_slice())
    }

    /// The open AsciiDoc documents not checked at their version and generation, the active
    /// one first; none while a check runs.
    fn unchecked(&self) -> Vec<(DocumentId, u64)> {
        if !self.spelling_on() || self.spelling.checking.is_some() {
            return Vec::new();
        }
        let generation = self.spelling.generation;
        let mut ids: Vec<DocumentId> = self.active.into_iter().collect();
        ids.extend(self.documents.keys().filter(|id| Some(**id) != self.active));
        ids.into_iter()
            .filter_map(|id| {
                let document = self
                    .documents
                    .get(&id)
                    .filter(|document| document.asciidoc)?;
                let current = self.spelling.results.get(&id).is_some_and(|checked| {
                    checked.version == document.version && checked.generation == generation
                });
                (!current).then_some((id, document.version))
            })
            .collect()
    }

    /// True while a page waits to be checked.
    pub(crate) fn has_due_spelling(&self) -> bool {
        !self.unchecked().is_empty()
    }

    /// Starts checking the first page that has gone unchanged for [`SPELLING_SETTLE`]. Returns
    /// the number of checks started.
    pub(crate) fn start_due_spelling(&mut self) -> usize {
        let now = self.clock.now();
        let due = self.unchecked().into_iter().find(|(id, _)| {
            self.spelling
                .changed
                .get(id)
                .is_none_or(|changed| now >= *changed + SPELLING_SETTLE)
        });
        let Some((id, version)) = due else {
            return 0;
        };
        let Some(input) = self.parse_input(id) else {
            return 0;
        };
        let generation = self.spelling.generation;
        self.spelling.checking = Some((id, version, generation));
        let lists = self.word_lists_for(&input.path);
        let dictionaries = self.spelling.dictionaries.clone();
        let configured = self.settings.spelling.language.clone();
        let parser = self.document_parser();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let (path, text) = (input.path.clone(), input.text.clone());
            let (dom, _) = input.parse(parser.as_ref());
            let model = Model::build(&path, &text, &dom);
            let misspellings = check_model(&model, &dictionaries, &configured, &lists);
            let _ = sender.send(WorkerMessage::SpellingChecked {
                id,
                version,
                generation,
                misspellings,
            });
        });
        1
    }

    /// The word lists that apply to the page at `path`: the user's, and its workspace root's.
    pub(crate) fn word_lists_for(&self, path: &std::path::Path) -> Vec<Arc<WordList>> {
        let mut lists = vec![self.spelling.user_words.clone()];
        lists.extend(
            self.spelling
                .workspace_words
                .iter()
                .filter(|(root, _)| path.starts_with(root))
                .map(|(_, list)| list.clone()),
        );
        lists
    }

    pub(crate) fn spelling_checked(
        &mut self,
        id: DocumentId,
        version: u64,
        generation: u64,
        misspellings: Vec<Misspelling>,
    ) {
        if self.spelling.checking == Some((id, version, generation)) {
            self.spelling.checking = None;
        }
        let current = self
            .documents
            .get(&id)
            .is_some_and(|document| document.version == version);
        if !current || generation != self.spelling.generation {
            return; // The page changed, or a list did, since the check began.
        }
        let changed = self.misspellings(id) != misspellings.as_slice();
        self.spelling.results.insert(
            id,
            Checked {
                version,
                generation,
                misspellings,
            },
        );
        if changed {
            self.emit(EngineEvent::SpellingChanged(id));
        }
    }

    pub(crate) fn forget_spelling(&mut self, id: DocumentId) {
        self.spelling.results.remove(&id);
        self.spelling.changed.remove(&id);
    }

    /// Reads the dictionaries again when their folder changed, and the word lists, and has every
    /// page checked again. Called whenever settings are loaded.
    pub(crate) fn spelling_settings_changed(&mut self) {
        let folder = self
            .settings
            .spelling
            .dictionaries
            .as_ref()
            .map(PathBuf::from);
        if folder != self.spelling.folder {
            self.spelling.dictionaries = Arc::new(Dictionaries::new(folder.as_deref()));
            self.spelling.folder = folder;
        }
        self.read_word_lists();
        self.spelling.generation += 1;
        if !self.spelling_on() {
            let cleared: Vec<DocumentId> = self.spelling.results.keys().copied().collect();
            self.spelling.results.clear();
            for id in cleared {
                self.emit(EngineEvent::SpellingChanged(id));
            }
        }
    }

    /// Reads the user's and each workspace root's word list. Task 6 fills this in.
    fn read_word_lists(&mut self) {}
}

/// The misspellings in the prose of `model`: the text of editable text blocks that are not code,
/// outside inline code and atoms, in the page's `lang`, else `configured`.
fn check_model(
    model: &Model,
    dictionaries: &Dictionaries,
    configured: &str,
    lists: &[Arc<WordList>],
) -> Vec<Misspelling> {
    let lang = model.attributes.get("lang").map(String::as_str);
    let Some(name) = dictionaries.resolve(lang, configured) else {
        return Vec::new();
    };
    let Ok(checker) = dictionaries.load(&name) else {
        return Vec::new();
    };
    let mut suggestions: HashMap<String, Vec<String>> = HashMap::new();
    let mut found = Vec::new();
    for (_, node, block) in model.text_blocks() {
        if node.access != Access::Editable || block.plain {
            continue;
        }
        let prose = Prose::of(&block.pieces, &model.source);
        for range in adoc_spell::words(&prose.text) {
            let Some(source) = prose.source_range(range.clone()) else {
                continue; // A word broken by a mark's delimiters.
            };
            let word = &prose.text[range];
            if checker.check(word) || lists.iter().any(|list| list.contains(word)) {
                continue;
            }
            let suggested = match suggestions.get(word) {
                Some(suggested) => suggested.clone(),
                None if suggestions.len() < MAX_SUGGESTED_WORDS => {
                    let suggested = checker.suggest(word);
                    suggestions.insert(word.to_owned(), suggested.clone());
                    suggested
                }
                None => Vec::new(),
            };
            found.push(Misspelling {
                range: source,
                word: word.to_owned(),
                suggestions: suggested,
            });
        }
    }
    found
}

/// A text block's prose as one string: its text pieces as written, mark delimiters left out,
/// and each atom or run of inline code as an [`adoc_spell::OBJECT`].
#[derive(Default)]
struct Prose {
    text: String,
    /// Each text piece: its start in `text`, its start in the page, and its length.
    segments: Vec<(usize, usize, usize)>,
}

impl Prose {
    fn of(pieces: &[Piece], source: &str) -> Self {
        let mut prose = Self::default();
        prose.push(pieces, source);
        prose
    }

    fn push(&mut self, pieces: &[Piece], source: &str) {
        for piece in pieces {
            match piece {
                Piece::Text { range } => {
                    self.segments
                        .push((self.text.len(), range.start, range.len()));
                    self.text.push_str(&source[range.start..range.end]);
                }
                Piece::Mark {
                    mark: Mark::Code, ..
                }
                | Piece::Atom { .. } => self.text.push(adoc_spell::OBJECT),
                Piece::Mark { children, .. } => self.push(children, source),
            }
        }
    }

    /// The page's range for `range` of the prose, when it lies within one text piece.
    fn source_range(&self, range: Range<usize>) -> Option<SourceRange> {
        let (start, source, length) = *self
            .segments
            .iter()
            .find(|(start, _, length)| (*start..*start + *length).contains(&range.start))?;
        (range.end <= start + length)
            .then(|| SourceRange::new(source + range.start - start, source + range.end - start))
    }
}
