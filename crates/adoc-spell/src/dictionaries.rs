//! The dictionaries available, bundled and from a folder, each parsed when first used.

use std::{
    collections::{BTreeMap, HashMap},
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use crate::apostrophes;

/// The most suggestions offered for a word.
pub const MAX_SUGGESTIONS: usize = 5;

/// The dictionaries built in: SCOWL's, described in `dictionaries/README.md`.
const BUNDLED: &[(&str, &str, &str)] = &[
    (
        "en_GB",
        include_str!("../dictionaries/en_GB.aff"),
        include_str!("../dictionaries/en_GB.dic"),
    ),
    (
        "en_US",
        include_str!("../dictionaries/en_US.aff"),
        include_str!("../dictionaries/en_US.dic"),
    ),
];

/// Where a dictionary comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DictionarySource {
    Bundled,
    /// Its `.aff` file; the `.dic` file is beside it.
    File(PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryInfo {
    pub name: String,
    pub source: DictionarySource,
}

#[derive(Debug)]
pub enum SpellError {
    Io { path: PathBuf, source: io::Error },
    Parse { name: String, message: String },
    Unknown(String),
}

impl fmt::Display for SpellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { name, message } => write!(f, "dictionary {name}: {message}"),
            Self::Unknown(name) => write!(f, "no dictionary named {name}"),
        }
    }
}

impl std::error::Error for SpellError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A parsed dictionary.
pub struct Checker {
    dictionary: spellbook::Dictionary,
}

impl Checker {
    fn parse(name: &str, aff: &str, dic: &str) -> Result<Self, SpellError> {
        spellbook::Dictionary::new(aff, dic)
            .map(|dictionary| Self { dictionary })
            .map_err(|error| SpellError::Parse {
                name: name.to_owned(),
                message: error.to_string(),
            })
    }

    /// Whether the dictionary knows `word`, by Hunspell's rules (a lower-case entry is also
    /// accepted capitalised and in capitals).
    #[must_use]
    pub fn check(&self, word: &str) -> bool {
        self.dictionary.check(&apostrophes(word))
    }

    /// At most [`MAX_SUGGESTIONS`] replacements for `word`, best first.
    #[must_use]
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        self.dictionary
            .suggest(&apostrophes(word), &mut suggestions);
        suggestions.truncate(MAX_SUGGESTIONS);
        suggestions
    }
}

/// The dictionaries available, by name, each parsed when first used and then kept.
pub struct Dictionaries {
    available: BTreeMap<String, DictionarySource>,
    loaded: Mutex<HashMap<String, Arc<Checker>>>,
}

impl Dictionaries {
    /// The bundled dictionaries, and each `name.aff` in `folder` with a `name.dic` beside it,
    /// which replaces a bundled one of the same name. An unreadable folder adds nothing.
    #[must_use]
    pub fn new(folder: Option<&Path>) -> Self {
        let mut available: BTreeMap<String, DictionarySource> = BUNDLED
            .iter()
            .map(|(name, _, _)| ((*name).to_owned(), DictionarySource::Bundled))
            .collect();
        if let Some(entries) = folder.and_then(|folder| fs::read_dir(folder).ok()) {
            for path in entries.flatten().map(|entry| entry.path()) {
                let is_aff = path.extension().is_some_and(|extension| extension == "aff");
                if !is_aff || !path.with_extension("dic").is_file() {
                    continue;
                }
                if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) {
                    available.insert(name.to_owned(), DictionarySource::File(path.clone()));
                }
            }
        }
        Self {
            available,
            loaded: Mutex::default(),
        }
    }

    /// Every dictionary available, by name.
    #[must_use]
    pub fn list(&self) -> Vec<DictionaryInfo> {
        self.available
            .iter()
            .map(|(name, source)| DictionaryInfo {
                name: name.clone(),
                source: source.clone(),
            })
            .collect()
    }

    /// The dictionary for a page whose language is `lang`, else `configured`. Tags compare
    /// without regard to case or to `-` and `_`. A bare language (`en`) takes `configured` when
    /// it is of that language, else the first dictionary of the language by name. `None` when
    /// no dictionary fits.
    #[must_use]
    pub fn resolve(&self, lang: Option<&str>, configured: &str) -> Option<String> {
        let wanted = normalise(lang.unwrap_or(configured));
        let named = |tag: &str| {
            self.available
                .keys()
                .find(|name| normalise(name) == tag)
                .cloned()
        };
        if let Some(name) = named(&wanted) {
            return Some(name);
        }
        if wanted.contains('_') {
            return None;
        }
        let configured = normalise(configured);
        if language(&configured) == wanted {
            if let Some(name) = named(&configured) {
                return Some(name);
            }
        }
        self.available
            .keys()
            .find(|name| language(&normalise(name)) == wanted)
            .cloned()
    }

    /// The dictionary `name`, parsed on first use.
    pub fn load(&self, name: &str) -> Result<Arc<Checker>, SpellError> {
        if let Some(checker) = self.cache().get(name) {
            return Ok(checker.clone());
        }
        let checker = match self.available.get(name) {
            None => return Err(SpellError::Unknown(name.to_owned())),
            Some(DictionarySource::Bundled) => {
                let (_, aff, dic) = BUNDLED
                    .iter()
                    .find(|(bundled, _, _)| *bundled == name)
                    .ok_or_else(|| SpellError::Unknown(name.to_owned()))?;
                Checker::parse(name, aff, dic)?
            }
            Some(DictionarySource::File(aff)) => {
                let dic = aff.with_extension("dic");
                Checker::parse(name, &read(aff)?, &read(&dic)?)?
            }
        };
        let checker = Arc::new(checker);
        self.cache().insert(name.to_owned(), checker.clone());
        Ok(checker)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Checker>>> {
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn read(path: &Path) -> Result<String, SpellError> {
    fs::read_to_string(path).map_err(|source| SpellError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// A language tag compared without regard to case or separator: `en-GB` is `en_gb`.
fn normalise(tag: &str) -> String {
    tag.trim().replace('-', "_").to_lowercase()
}

fn language(tag: &str) -> &str {
    tag.split('_').next().unwrap_or(tag)
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::*;

    /// The bundled dictionaries, parsed once for every test.
    fn bundled() -> &'static Dictionaries {
        static BUNDLED: OnceLock<Dictionaries> = OnceLock::new();
        BUNDLED.get_or_init(|| Dictionaries::new(None))
    }

    #[test]
    fn british_and_american_spellings_differ() {
        let gb = bundled().load("en_GB").unwrap();
        let us = bundled().load("en_US").unwrap();
        assert!(gb.check("colour") && !gb.check("color"));
        assert!(us.check("color") && !us.check("colour"));
        assert!(gb.check("Paris") && gb.check("The"));
        assert!(gb.check("don’t") && gb.check("don't"), "both apostrophes");
    }

    #[test]
    fn suggestions_are_few_and_relevant() {
        let gb = bundled().load("en_GB").unwrap();
        let suggestions = gb.suggest("recieve");
        assert!(
            suggestions.iter().any(|word| word == "receive"),
            "{suggestions:?}"
        );
        assert!(suggestions.len() <= MAX_SUGGESTIONS);
    }

    #[test]
    fn languages_resolve_to_dictionaries() {
        let all = bundled();
        assert_eq!(all.resolve(None, "en_GB").as_deref(), Some("en_GB"));
        assert_eq!(
            all.resolve(Some("en-US"), "en_GB").as_deref(),
            Some("en_US")
        );
        assert_eq!(
            all.resolve(Some("en_gb"), "en_US").as_deref(),
            Some("en_GB")
        );
        assert_eq!(all.resolve(Some("en"), "en_US").as_deref(), Some("en_US"));
        assert_eq!(
            all.resolve(Some("en"), "de_DE").as_deref(),
            Some("en_GB"),
            "first by name"
        );
        assert_eq!(all.resolve(Some("de"), "en_GB"), None);
        assert_eq!(all.resolve(Some("en-AU"), "en_GB"), None, "no such region");
        assert!(matches!(all.load("de_DE"), Err(SpellError::Unknown(_))));
    }

    #[test]
    fn a_folder_adds_and_replaces_dictionaries() {
        let folder = tempfile::tempdir().unwrap();
        for name in ["en_GB", "xx_YY"] {
            fs::write(folder.path().join(format!("{name}.aff")), "SET UTF-8\n").unwrap();
            fs::write(folder.path().join(format!("{name}.dic")), "1\nqwzxy\n").unwrap();
        }
        fs::write(folder.path().join("lonely.aff"), "SET UTF-8\n").unwrap();
        let all = Dictionaries::new(Some(folder.path()));
        let names: Vec<(String, bool)> = all
            .list()
            .into_iter()
            .map(|info| (info.name, info.source == DictionarySource::Bundled))
            .collect();
        assert_eq!(
            names,
            [
                ("en_GB".to_owned(), false),
                ("en_US".to_owned(), true),
                ("xx_YY".to_owned(), false)
            ],
            "an .aff without its .dic is not a dictionary"
        );
        let replaced = all.load("en_GB").unwrap();
        assert!(replaced.check("qwzxy") && !replaced.check("colour"));
        assert!(
            Arc::ptr_eq(&replaced, &all.load("en_GB").unwrap()),
            "parsed once"
        );
    }
}
