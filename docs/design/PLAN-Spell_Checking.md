# Spell Checking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Underline misspelt prose words in the source editor, the visual editor and `adoc-ls`, with suggestions and Add to Workspace/User Dictionary (issue #65).

**Architecture:** A new L1 crate `adoc-spell` wraps `spellbook` (Hunspell dictionaries), splits plain text into checkable words and holds word lists. `adoc-engine` decides what is prose by walking the visual editor's model (`adoc_model::Model::text_blocks()`), checks open documents on a worker once typing settles, and publishes `Misspelling`s with suggestions. Front ends only draw ranges and send `edit.replace_range` or the two `spelling.*` commands.

**Tech Stack:** Rust 2021, `spellbook` 0.4.2 (MPL-2.0), `unicode-segmentation` 1.13.3 (already in the tree), SCOWL Hunspell dictionaries `rel-2026.02.25`, GPUI / gpui-component 0.8, `lsp-types`.

**Spec:** `docs/design/DESIGN-Spell_Checking.md`

## Global Constraints

- Every cargo command that compiles takes `-j 6` (memory limit on the development machine).
- Dependencies are pinned exactly in `[workspace.dependencies]` (`spellbook = "=0.4.2"`); crates use `.workspace = true`.
- `spellbook` may be named only by `adoc-spell`; `cargo xtask layers` enforces it.
- Layering: `adoc-spell` is L1 and depends on no internal crate; `adoc-engine` (L3) depends on it.
- No `anyhow` in libraries; typed errors. `unsafe_code = "forbid"`. No global mutable state.
- Byte offsets (`SourceRange`) everywhere; line/column conversion only at the edges.
- Conservative: prefer not flagging a word to flagging a correct one.
- Misspellings are never `adoc_core::Diagnostic`s and never reach the Problems panel or `adocfoundry-cli check`.
- British spelling in docs, comments and UI strings. No mention of AI tools anywhere.
- Diagnostic code string for spelling: `adoc.spelling`. Workspace list file: `.adocfoundry-words.txt`. User list file: `spelling-words.txt` beside the user settings file.
- Settings: `[spelling] enabled = true`, `language = "en_GB"` (user and workspace files), `dictionaries` (user file only).
- `SPELLING_SETTLE` = 500 ms; at most 5 suggestions a word; suggestions for at most 100 distinct words a page.
- TDD: write each test, see it fail, then implement.
- A task is done when `cargo fmt --check`, `cargo clippy -j 6 --workspace --all-targets -- -D warnings` and the task's tests pass.

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/adoc-spell/Cargo.toml` | New crate manifest |
| `crates/adoc-spell/src/lib.rs` | Re-exports; apostrophe normalisation |
| `crates/adoc-spell/src/dictionaries.rs` | `Dictionaries` (bundled + folder, language resolution, cache), `Checker`, `SpellError` |
| `crates/adoc-spell/src/words.rs` | `words()` and the skip rules, `OBJECT` |
| `crates/adoc-spell/src/list.rs` | `WordList` |
| `crates/adoc-spell/dictionaries/` | `en_GB.aff/.dic`, `en_US.aff/.dic`, `README.md` (source and licence) |
| `crates/adoc-engine/src/settings.rs` | `SpellingSettings` and the `[spelling]` keys |
| `crates/adoc-engine/src/spelling.rs` | Engine state, prose extraction, worker, edit shifting, word lists, commands |
| `crates/adoc-engine/tests/spelling.rs` | Engine integration tests |
| `crates/adoc-engine/src/doctor.rs` | Spelling section of the report |
| `crates/adoc-ls/src/protocol.rs`, `capabilities.rs` | Hint diagnostics, quick fixes, add commands |
| `crates/adocfoundry/src/spelling.rs` | Menu choices shared by both editors, `ApplySpelling` action |
| `crates/adocfoundry/src/editor/mod.rs` | Source editor hints and context menu |
| `crates/adocfoundry/src/visual/{mod,blocks,text}.rs` | Visual editor underline and right-click menu |
| `xtask/src/layers.rs`, `xtask/src/package.rs` | Layer entry and confinement; licence copied into archives |

---

### Task 1: `adoc-spell` crate with bundled dictionaries and the checker

**Files:**
- Create: `crates/adoc-spell/Cargo.toml`, `crates/adoc-spell/src/lib.rs`, `crates/adoc-spell/src/dictionaries.rs`
- Create: `crates/adoc-spell/dictionaries/{en_GB.aff,en_GB.dic,en_US.aff,en_US.dic,README.md}`
- Modify: `Cargo.toml` (workspace dependencies), `xtask/src/layers.rs`

**Interfaces:**
- Produces: `adoc_spell::{Dictionaries, Checker, DictionaryInfo, DictionarySource, SpellError, MAX_SUGGESTIONS}`; `Dictionaries::new(folder: Option<&Path>) -> Dictionaries`, `Dictionaries::list(&self) -> Vec<DictionaryInfo>`, `Dictionaries::resolve(&self, lang: Option<&str>, configured: &str) -> Option<String>`, `Dictionaries::load(&self, name: &str) -> Result<Arc<Checker>, SpellError>`, `Checker::check(&self, word: &str) -> bool`, `Checker::suggest(&self, word: &str) -> Vec<String>`; crate-private `apostrophes(word: &str) -> Cow<'_, str>`.

- [ ] **Step 1: Fetch the dictionaries**

```bash
D=$(mktemp -d) && cd "$D"
for z in hunspell-en_GB-ise-2026.02.25.zip hunspell-en_US-2026.02.25.zip; do
  curl -sL -o "$z" "https://github.com/en-wl/wordlist/releases/download/rel-2026.02.25/$z"
done
sha256sum *.zip   # d6fbb91a…7247a3 (en_GB-ise), ac8e7331…1179f9 (en_US)
unzip -q hunspell-en_GB-ise-2026.02.25.zip -d gb && unzip -q hunspell-en_US-2026.02.25.zip -d us
R=/run/media/paul/t7_backup/gitHUB/AdocFoundry/crates/adoc-spell/dictionaries
mkdir -p "$R"
cp gb/en_GB-ise.aff "$R/en_GB.aff"; cp gb/en_GB-ise.dic "$R/en_GB.dic"
cp us/en_US.aff "$R/en_US.aff";     cp us/en_US.dic "$R/en_US.dic"
cp gb/README_en_GB-ise.txt "$D/notice.txt"
```

- [ ] **Step 2: Write `crates/adoc-spell/dictionaries/README.md`**

Title "Bundled dictionaries", then: the files and what they were renamed from (`en_GB-ise.aff/.dic` → `en_GB.*`, `en_US.*` unchanged), the release (`rel-2026.02.25`), both download URLs and SHA-256 sums from Step 1, that they are SCOWL size 60, unmodified apart from the file names, and then the whole "COPYRIGHT, SOURCES, and CREDITS" section of `notice.txt` verbatim (from "The English dictionaries come directly from SCOWL" to the end of the affix-file licence). That section's terms require the notice in all copies and in documentation; this file is what `cargo xtask package` copies (Task 11).

- [ ] **Step 3: Add the crate to the workspace and the layer table**

`Cargo.toml`, under "Internal crates" keep alphabetical order:

```toml
adoc-spell = { path = "crates/adoc-spell" }
```

and among the external dependencies (alphabetical):

```toml
spellbook = "=0.4.2"
```

`crates/adoc-spell/Cargo.toml`:

```toml
[package]
name = "adoc-spell"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
spellbook.workspace = true
unicode-segmentation.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true
```

`xtask/src/layers.rs`: add `("adoc-spell", 1),` after `("adoc-math", 1),` in `LAYERS`; add below `PARSER_CRATES`:

```rust
/// The spell checker, permitted only in `adoc-spell`.
pub const SPELL_CRATES: &[&str] = &["spellbook"];
```

and in `check_manifest`, after the parser loop:

```rust
    for dependency in &manifest.dependencies {
        if SPELL_CRATES.contains(&dependency.as_str()) && name != "adoc-spell" {
            violations.push(format!(
                "{name} depends on spell checker {dependency}; only adoc-spell may"
            ));
        }
    }
```

with a test beside `confines_ui_and_lsp_crates`:

```rust
    #[test]
    fn confines_the_spell_checker() {
        assert!(check_manifest(&manifest("adoc-spell", &["spellbook"])).is_empty());
        assert_eq!(
            check_manifest(&manifest("adoc-engine", &["spellbook"])),
            ["adoc-engine depends on spell checker spellbook; only adoc-spell may"]
        );
    }
```

- [ ] **Step 4: Write the failing tests** in `crates/adoc-spell/src/dictionaries.rs` (`#[cfg(test)] mod tests`)

```rust
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
        assert!(suggestions.iter().any(|word| word == "receive"), "{suggestions:?}");
        assert!(suggestions.len() <= MAX_SUGGESTIONS);
    }

    #[test]
    fn languages_resolve_to_dictionaries() {
        let all = bundled();
        assert_eq!(all.resolve(None, "en_GB").as_deref(), Some("en_GB"));
        assert_eq!(all.resolve(Some("en-US"), "en_GB").as_deref(), Some("en_US"));
        assert_eq!(all.resolve(Some("en_gb"), "en_US").as_deref(), Some("en_GB"));
        assert_eq!(all.resolve(Some("en"), "en_US").as_deref(), Some("en_US"));
        assert_eq!(all.resolve(Some("en"), "de_DE").as_deref(), Some("en_GB"), "first by name");
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
        assert!(Arc::ptr_eq(&replaced, &all.load("en_GB").unwrap()), "parsed once");
    }
}
```

- [ ] **Step 5: Run them to see them fail**

Run: `cargo test -j 6 -p adoc-spell`
Expected: FAIL to compile (`Dictionaries` not defined).

- [ ] **Step 6: Implement `lib.rs` and `dictionaries.rs`**

`crates/adoc-spell/src/lib.rs`:

```rust
//! Spell checking for prose: Hunspell dictionaries through `spellbook`, the words worth checking
//! in a run of plain text, and lists of words accepted beyond the dictionary. It knows nothing of
//! AsciiDoc; the engine decides what is prose.

use std::borrow::Cow;

mod dictionaries;

pub use dictionaries::{
    Checker, Dictionaries, DictionaryInfo, DictionarySource, SpellError, MAX_SUGGESTIONS,
};

/// `word` with each right single quotation mark read as an apostrophe, as dictionaries and word
/// lists spell it.
pub(crate) fn apostrophes(word: &str) -> Cow<'_, str> {
    if word.contains('\u{2019}') {
        Cow::Owned(word.replace('\u{2019}', "'"))
    } else {
        Cow::Borrowed(word)
    }
}
```

`crates/adoc-spell/src/dictionaries.rs`:

```rust
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
        self.dictionary.suggest(&apostrophes(word), &mut suggestions);
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
```

- [ ] **Step 7: Run the tests**

Run: `cargo test -j 6 -p adoc-spell && cargo test -j 6 -p xtask confines_the_spell_checker && cargo xtask layers && cargo deny check`
Expected: all pass; `cargo deny` accepts `spellbook` (MPL-2.0) and `foldhash` (Zlib).

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/adoc-spell xtask/src/layers.rs
git commit -m "Add adoc-spell with the bundled SCOWL dictionaries"
```

---

### Task 2: Words worth checking and word lists

**Files:**
- Create: `crates/adoc-spell/src/words.rs`, `crates/adoc-spell/src/list.rs`
- Modify: `crates/adoc-spell/src/lib.rs`

**Interfaces:**
- Consumes: `crate::apostrophes`, `SpellError` (Task 1).
- Produces: `adoc_spell::{words, OBJECT, WordList}`; `words(text: &str) -> impl Iterator<Item = Range<usize>> + '_`; `WordList::{parse(&str) -> WordList, read(&Path) -> Result<WordList, SpellError>, write(&self, &Path) -> Result<(), SpellError>, insert(&mut self, &str) -> bool, contains(&self, &str) -> bool, to_text(&self) -> String, len, is_empty}`.

- [ ] **Step 1: Write the failing tests**

`crates/adoc-spell/src/words.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn checked(text: &str) -> Vec<&str> {
        words(text).map(|range| &text[range]).collect()
    }

    #[test]
    fn words_are_split_by_unicode_boundaries() {
        assert_eq!(checked("The quick brown fox."), ["The", "quick", "brown", "fox"]);
        assert_eq!(checked("well-known"), ["well", "known"]);
        assert_eq!(checked("don’t won't"), ["don’t", "won't"]);
        assert_eq!(checked("naïve café"), ["naïve", "café"]);
    }

    #[test]
    fn names_codes_and_single_letters_are_skipped() {
        let none: [&str; 0] = [];
        assert_eq!(
            checked("GitHub HTML iOS camelCase v2 file.txt e.g. snake_case a I 42"),
            none
        );
    }

    #[test]
    fn words_touching_an_object_are_skipped() {
        let text = format!("{OBJECT}ish word{OBJECT}ing here");
        assert_eq!(checked(&text), ["here"]);
    }
}
```

`crates/adoc-spell/src/list.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_reads_and_writes_sorted_lines() {
        let list = WordList::parse("# Team words\nzebra\n\nantora\nzebra\n  Asciidoctor  \n");
        assert_eq!(list.len(), 3);
        assert_eq!(list.to_text(), "Asciidoctor\nantora\nzebra\n");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("words.txt");
        assert!(WordList::read(&path).unwrap().is_empty(), "a missing file is empty");
        let mut written = list.clone();
        assert!(written.insert("Qwzxy"));
        assert!(!written.insert("Qwzxy"));
        written.write(&path).unwrap();
        assert_eq!(WordList::read(&path).unwrap(), written);
    }

    #[test]
    fn a_listed_word_accepts_its_capitals_and_possessive() {
        let list = WordList::parse("antora\nAsciidoctor\n");
        for word in ["antora", "Antora", "ANTORA", "Antora's", "Antora’s", "Asciidoctor", "ASCIIDOCTOR"] {
            assert!(list.contains(word), "{word}");
        }
        for word in ["AnTora", "asciidoctor", "antoras"] {
            assert!(!list.contains(word), "{word}");
        }
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 6 -p adoc-spell`
Expected: FAIL to compile (`words`, `WordList` not defined; add `mod words; mod list;` to `lib.rs` first so the test modules compile into the crate).

- [ ] **Step 3: Implement**

`crates/adoc-spell/src/words.rs` (above the tests):

```rust
//! The words of a run of plain text worth checking.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation as _;

/// Stands in the text for something that is not prose, such as a macro, a reference or inline
/// code: it breaks words, and a word touching it is never checked, being part of something
/// larger.
pub const OBJECT: char = '\u{FFFC}';

/// The words of `text` worth checking, as byte ranges. Words are split by Unicode's word
/// boundaries (so *don’t* is one word and *well-known* two), and a word is skipped when it holds
/// a digit, `_` or `.`, has fewer than two letters, is all capitals, has a capital after its
/// first letter, or touches an [`OBJECT`].
pub fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    text.split_word_bound_indices()
        .filter_map(move |(start, word)| {
            let end = start + word.len();
            let checked = word.chars().any(char::is_alphabetic)
                && !skipped(word)
                && !text[..start].ends_with(OBJECT)
                && !text[end..].starts_with(OBJECT);
            checked.then_some(start..end)
        })
}

fn skipped(word: &str) -> bool {
    let letters = || word.chars().filter(|c| c.is_alphabetic());
    word.chars().any(|c| c.is_numeric() || c == '_' || c == '.')
        || letters().count() < 2
        || letters().all(char::is_uppercase)
        || word.chars().skip(1).any(char::is_uppercase)
}
```

`crates/adoc-spell/src/list.rs` (above the tests):

```rust
//! Words accepted beyond the dictionary, kept in a plain-text file.

use std::{collections::BTreeSet, fs, io, path::Path};

use crate::{apostrophes, SpellError};

/// A list of words, one a line, kept sorted and without repeats. Blank lines and lines beginning
/// with `#` are ignored.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WordList {
    words: BTreeSet<String>,
}

impl WordList {
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self {
            words: text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(|line| apostrophes(line).into_owned())
                .collect(),
        }
    }

    /// The list in `path`; a missing file is an empty list.
    pub fn read(path: &Path) -> Result<Self, SpellError> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(SpellError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn write(&self, path: &Path) -> Result<(), SpellError> {
        fs::write(path, self.to_text()).map_err(|source| SpellError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// The list as written: each word on its own line, sorted.
    #[must_use]
    pub fn to_text(&self) -> String {
        self.words.iter().map(|word| format!("{word}\n")).collect()
    }

    /// Adds `word`; false when it was listed already.
    pub fn insert(&mut self, word: &str) -> bool {
        self.words.insert(apostrophes(word).into_owned())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.words.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Whether `word` is accepted: listed as written; or listed in lower case and written
    /// capitalised or in capitals; or listed capitalised and written in capitals. A possessive
    /// `'s` is accepted after any of these.
    #[must_use]
    pub fn contains(&self, word: &str) -> bool {
        let word = apostrophes(word);
        let base = word.strip_suffix("'s").unwrap_or(&word);
        self.accepts(&word) || self.accepts(base)
    }

    fn accepts(&self, word: &str) -> bool {
        if self.words.contains(word) {
            return true;
        }
        let lower = word.to_lowercase();
        let capitalised = capitalise(&lower);
        if word == capitalised {
            return self.words.contains(&lower);
        }
        word == word.to_uppercase()
            && (self.words.contains(&lower) || self.words.contains(&capitalised))
    }
}

fn capitalise(word: &str) -> String {
    let mut characters = word.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}
```

`lib.rs`: add `mod list; mod words;` and `pub use list::WordList; pub use words::{words, OBJECT};`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adoc-spell`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adoc-spell
git commit -m "Split prose into words to check and read word lists"
```

---

### Task 3: `[spelling]` settings

**Files:**
- Modify: `crates/adoc-engine/src/settings.rs`, `crates/adoc-engine/src/lib.rs` (re-export)

**Interfaces:**
- Produces: `adoc_engine::SpellingSettings { enabled: bool, language: String, dictionaries: Option<String> }`, field `Settings::spelling`.

- [ ] **Step 1: Write the failing test** in `settings.rs`'s `mod tests`, beside `sidebar_follow_active_loads_per_layer`

```rust
    #[test]
    fn spelling_loads_per_layer() {
        let defaults = Settings::default().spelling;
        assert!(defaults.enabled);
        assert_eq!(defaults.language, "en_GB");
        assert_eq!(defaults.dictionaries, None);

        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.toml");
        fs::write(
            &user,
            "[spelling]\nenabled = false\nlanguage = \"en_US\"\ndictionaries = \"/opt/dicts\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap();
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert!(!settings.spelling.enabled);
        assert_eq!(settings.spelling.language, "en_US");
        assert_eq!(settings.spelling.dictionaries.as_deref(), Some("/opt/dicts"));

        // The workspace overrides the switch and the language, but not the folder.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[spelling]\nenabled = true\nlanguage = \"de-DE\"\ndictionaries = \"/tmp/other\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert!(settings.spelling.enabled);
        assert_eq!(settings.spelling.language, "de-DE");
        assert_eq!(settings.spelling.dictionaries.as_deref(), Some("/opt/dicts"));
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
        assert!(settings.warnings[0].contains("can only be set in the user settings file"));

        // An invalid language warns and keeps the layer beneath.
        fs::write(
            directory.path().join(WORKSPACE_SETTINGS_FILE),
            "[spelling]\nlanguage = \"en GB\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), Some(directory.path())).unwrap();
        assert_eq!(settings.spelling.language, "en_US");
        assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -j 6 -p adoc-engine --lib spelling_loads_per_layer`
Expected: FAIL to compile (no field `spelling`).

- [ ] **Step 3: Implement**

After `HistorySettings` in `settings.rs`:

```rust
/// Spell checking: whether it is on, the language of pages with no `lang` attribute, and a
/// folder of further Hunspell dictionaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpellingSettings {
    pub enabled: bool,
    pub language: String,
    pub dictionaries: Option<String>,
}

impl Default for SpellingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            language: "en_GB".to_owned(),
            dictionaries: None,
        }
    }
}
```

Add `pub spelling: SpellingSettings,` to `Settings` after `history`. In `apply_one`, extend `allowed_here` with `|| (section == "spelling" && key != "dictionaries")`. In `set`, before the `pdf` arms:

```rust
            ("spelling", "enabled") => self.spelling.enabled = boolean(value)?,
            ("spelling", "language") => self.spelling.language = language_tag(value)?,
            ("spelling", "dictionaries") => self.spelling.dictionaries = Some(string(value)?),
```

In `is_known`, add `| ("spelling", "enabled" | "language" | "dictionaries")`. Beside `expansion()`:

```rust
fn language_tag(value: &toml::Value) -> Result<String, &'static str> {
    value
        .as_str()
        .filter(|tag| {
            !tag.is_empty()
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .map(str::to_owned)
        .ok_or("expected a language tag such as \"en_GB\"")
}
```

Re-export `SpellingSettings` from `lib.rs` where the other settings structs are exported (`pub use settings::{…}`).

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adoc-engine --lib settings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adoc-engine/src/settings.rs crates/adoc-engine/src/lib.rs
git commit -m "Add the spelling settings"
```

---

### Task 4: The engine checks the prose of open pages

**Files:**
- Create: `crates/adoc-engine/src/spelling.rs`, `crates/adoc-engine/tests/spelling.rs`
- Modify: `crates/adoc-engine/Cargo.toml` (`adoc-spell.workspace = true`), `src/lib.rs`, `src/engine.rs`, `src/workspace.rs`, `src/event.rs`

**Interfaces:**
- Consumes: `adoc_spell::{Dictionaries, WordList, words, OBJECT}` (Tasks 1–2), `Settings::spelling` (Task 3), `adoc_model::{Model, Piece, Mark, Access}`, `Engine::parse_input`, `OwnedInput::{path, text, parse}`.
- Produces: `Engine::enable_spelling(&mut self)`, `Engine::misspellings(&self, id: DocumentId) -> &[Misspelling]`, `adoc_engine::{Misspelling, SPELLING_SETTLE, SPELLING_CODE}`, `EngineEvent::SpellingChanged(DocumentId)`; crate-private `Spelling` state with `generation: u64`, `user_words: Arc<WordList>`, `workspace_words: BTreeMap<PathBuf, Arc<WordList>>`, `dictionaries: Arc<Dictionaries>` (Tasks 5–7 extend it).

- [ ] **Step 1: Write the failing tests** in `crates/adoc-engine/tests/spelling.rs`

```rust
//! Spell checking of the prose of open pages, once typing settles.

use std::{fs, path::PathBuf, sync::Arc, time::Duration};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, EngineEvent, ManualClock, Settings,
    SPELLING_SETTLE,
};

const WAIT: Duration = Duration::from_secs(30);

fn engine_with(settings: Settings) -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(settings.with_git_opt_in(), Arc::new(clock.clone()));
    engine.enable_spelling();
    (engine, clock)
}

fn engine_with_clock() -> (Engine, ManualClock) {
    engine_with(Settings::load(None, None).unwrap())
}

/// Pumps until the workers are idle, as a front end's timer would.
fn settle(engine: &mut Engine) {
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    engine.pump();
}

fn page(directory: &tempfile::TempDir, name: &str, text: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, text).unwrap();
    path
}

fn misspelt(engine: &Engine, id: DocumentId) -> Vec<String> {
    engine
        .misspellings(id)
        .iter()
        .map(|misspelling| misspelling.word.clone())
        .collect()
}

fn type_text(engine: &mut Engine, id: DocumentId, offset: usize, text: &str) {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(offset, offset),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

#[test]
fn a_misspelt_word_in_a_paragraph_is_found() {
    let directory = tempfile::tempdir().unwrap();
    let text = "= Guide\n\nWe recieve the colour.\n";
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let found = engine.misspellings(id);
    assert_eq!(found.len(), 1, "{found:?}");
    let start = text.find("recieve").unwrap();
    assert_eq!(found[0].word, "recieve");
    assert_eq!(found[0].range, SourceRange::new(start, start + "recieve".len()));
    assert!(found[0].suggestions.iter().any(|word| word == "receive"));
}

#[test]
fn words_in_a_source_block_are_never_flagged() {
    let directory = tempfile::tempdir().unwrap();
    let text = concat!(
        "= Guide\n",
        ":product: Foundary\n\n",
        "[source,rust]\n",
        "----\n",
        "let qwzxa = recieve();\n",
        "----\n\n",
        "....\n",
        "literal qwzxb\n",
        "....\n\n",
        "Use `qwzxc` and xref:other.adoc[qwzxd] and https://example.com/qwzxe[] and {product}.\n\n",
        "Text with qwzxy.\n",
    );
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxy"]);
}

#[test]
fn included_text_is_checked_in_its_own_file() {
    let directory = tempfile::tempdir().unwrap();
    page(&directory, "part.adoc", "Included qwzxa.\n");
    let path = page(
        &directory,
        "page.adoc",
        "= Page\n\ninclude::part.adoc[]\n\nOwn qwzxy.\n",
    );
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxy"]);
}

#[test]
fn a_word_split_by_marks_or_touching_a_reference_is_not_checked() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(
        &directory,
        "page.adoc",
        "= Page\n:x: y\n\n**qw**zxy and {x}qwzxb and *qwzxc* here.\n",
    );
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxc"]);
}

#[test]
fn the_page_language_chooses_the_dictionary() {
    let directory = tempfile::tempdir().unwrap();
    let british = page(&directory, "gb.adoc", "= Page\n\nThe color and the colour.\n");
    let american = page(
        &directory,
        "us.adoc",
        "= Page\n:lang: en-US\n\nThe color and the colour.\n",
    );
    let german = page(&directory, "de.adoc", "= Seite\n:lang: de\n\nThe qwzxy.\n");
    let (mut engine, _clock) = engine_with_clock();
    let british = engine.open(&british).unwrap();
    let american = engine.open(&american).unwrap();
    let german = engine.open(&german).unwrap();
    for _ in 0..3 {
        settle(&mut engine); // one page a job
    }
    assert_eq!(misspelt(&engine, british), ["color"]);
    assert_eq!(misspelt(&engine, american), ["colour"]);
    assert!(misspelt(&engine, german).is_empty(), "no German dictionary");
}

#[test]
fn checking_waits_for_typing_to_settle() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(&directory, "page.adoc", "= Page\n\nOne two.\n");
    let (mut engine, clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert!(misspelt(&engine, id).is_empty());
    assert!(!engine.has_pending());

    type_text(&mut engine, id, "= Page\n\nOne two".len(), " qwzxy");
    clock.advance(SPELLING_SETTLE - Duration::from_millis(1));
    settle(&mut engine);
    assert!(misspelt(&engine, id).is_empty(), "not yet");
    clock.advance(Duration::from_millis(1));
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["qwzxy"]);
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::SpellingChanged(id)));
}

#[test]
fn nothing_is_checked_without_enable_spelling() {
    let directory = tempfile::tempdir().unwrap();
    let path = page(&directory, "page.adoc", "= Page\n\nA qwzxy.\n");
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(
        Settings::load(None, None).unwrap().with_git_opt_in(),
        Arc::new(clock),
    );
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert!(engine.misspellings(id).is_empty());
    assert!(!engine.has_pending());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 6 -p adoc-engine --test spelling`
Expected: FAIL to compile (`enable_spelling`, `misspellings`, `SPELLING_SETTLE` not defined).

- [ ] **Step 3: Implement `crates/adoc-engine/src/spelling.rs`**

```rust
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

use crate::{workspace::WorkerMessage, DocumentId, Engine, EngineEvent, TextEdit};

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
                let document = self.documents.get(&id).filter(|document| document.asciidoc)?;
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
        let folder = self.settings.spelling.dictionaries.as_ref().map(PathBuf::from);
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
        (range.end <= start + length).then(|| {
            SourceRange::new(source + range.start - start, source + range.end - start)
        })
    }
}
```

Note: `TextEdit` is imported for Task 5; if clippy flags it as unused now, add it in Task 5 instead.

- [ ] **Step 4: Wire it into the engine**

- `crates/adoc-engine/Cargo.toml`: `adoc-spell.workspace = true` under `[dependencies]`.
- `src/lib.rs`: `mod spelling;` and `pub use spelling::{Misspelling, MAX_SUGGESTED_WORDS, SPELLING_CODE, SPELLING_SETTLE};`.
- `src/engine.rs`: field `pub(crate) spelling: crate::spelling::Spelling,` beside `words`, initialised `spelling: crate::spelling::Spelling::default(),` beside `words: crate::words::WordCounts::default(),`; in `close`, `self.forget_spelling(id);` after `self.forget_word_count(id);`; in `reload_settings`, `self.spelling_settings_changed();` right after `self.emit(EngineEvent::SettingsChanged);`.
- `src/event.rs`: after `WordCountChanged(DocumentId),` add

```rust
    /// The misspellings of an open document changed: found by a check, moved or dropped by an
    /// edit, or cleared.
    SpellingChanged(DocumentId),
```

- `src/workspace.rs`: add to `WorkerMessage`

```rust
    SpellingChecked {
        id: crate::DocumentId,
        version: u64,
        generation: u64,
        misspellings: Vec<crate::Misspelling>,
    },
```

  in `has_pending` add `|| self.has_due_spelling()` after `|| self.has_due_word_count()`; in `pump` add `+ self.start_due_spelling()` after `+ self.start_due_word_count()`; in `apply_worker_message`:

```rust
            WorkerMessage::SpellingChecked {
                id,
                version,
                generation,
                misspellings,
            } => self.spelling_checked(id, version, generation, misspellings),
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -j 6 -p adoc-engine --test spelling`
Expected: PASS except `checking_waits_for_typing_to_settle`, which needs Task 5's edit hook (it fails at "not yet" because nothing records the change). Leave it failing; Task 5 makes it pass. If any other test fails, fix it before moving on. Also run `cargo test -j 6 -p adoc-engine` to confirm nothing else broke (the `EngineEvent` enum gained a variant; fix any exhaustive `match` it breaks).

- [ ] **Step 6: Commit**

```bash
git add crates/adoc-engine
git commit -m "Check the spelling of open pages on a worker"
```

---

### Task 5: Misspellings follow edits; replacing a word is one undo step

**Files:**
- Modify: `crates/adoc-engine/src/spelling.rs`, `crates/adoc-engine/src/engine.rs`
- Test: `crates/adoc-engine/tests/spelling.rs`

**Interfaces:**
- Consumes: Task 4's `Spelling`, `TextEdit { range, replacement }` (sorted, non-overlapping, in the coordinates of the text before).
- Produces: crate-private `Engine::spelling_edited(&mut self, id: DocumentId, edits: &[TextEdit])`.

- [ ] **Step 1: Write the failing tests** (append to `tests/spelling.rs`)

```rust
#[test]
fn misspellings_move_with_edits_and_go_when_typed_into() {
    let directory = tempfile::tempdir().unwrap();
    let text = "= Page\n\nA qwzxy word.\n";
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let start = text.find("qwzxy").unwrap();
    assert_eq!(engine.misspellings(id)[0].range, SourceRange::new(start, start + 5));

    type_text(&mut engine, id, text.find("A qwzxy").unwrap(), "Big ");
    assert_eq!(
        engine.misspellings(id)[0].range,
        SourceRange::new(start + 4, start + 9),
        "moved before any check"
    );
    type_text(&mut engine, id, start + 9, "s");
    assert!(engine.misspellings(id).is_empty(), "typed into");
}

#[test]
fn a_misspelt_word_is_replaced_in_one_undo_step() {
    let directory = tempfile::tempdir().unwrap();
    let text = "= Page\n\nWe recieve it.\n";
    let path = page(&directory, "page.adoc", text);
    let (mut engine, _clock) = engine_with_clock();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let misspelling = engine.misspellings(id)[0].clone();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: misspelling.range,
                text: "receive".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(engine.document(id).unwrap().text_string(), "= Page\n\nWe receive it.\n");
    assert!(engine.misspellings(id).is_empty());
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(engine.document(id).unwrap().text_string(), text);
}

#[test]
fn nothing_is_checked_when_spelling_is_off() {
    let directory = tempfile::tempdir().unwrap();
    let user = page(&directory, "settings.toml", "[spelling]\nenabled = false\n");
    let path = page(&directory, "page.adoc", "= Page\n\nA qwzxy.\n");
    let (mut engine, _clock) = engine_with(Settings::load(Some(&user), None).unwrap());
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert!(engine.misspellings(id).is_empty());
    assert!(!engine.has_pending());
}
```

Append the timing probe to `crates/adoc-engine/tests/perf.rs`, reusing its `synthetic_document()`:

```rust
#[test]
#[ignore = "timing probe"]
fn spelling_a_5000_line_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.adoc");
    fs::write(&path, synthetic_document()).unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.enable_spelling();
    let id = engine.open(&path).unwrap();
    let started = Instant::now();
    engine.pump();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(60)));
    let elapsed = started.elapsed();
    println!(
        "spelling: {elapsed:?} for {} misspellings (dictionary load included)",
        engine.misspellings(id).len()
    );
    assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
}
```

(Run it with `cargo test -j 6 --release -p adoc-engine --test perf spelling -- --ignored --nocapture`; the first check includes parsing the dictionary. Record the timing in the PR description.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 6 -p adoc-engine --test spelling`
Expected: FAIL in `misspellings_move_with_edits_and_go_when_typed_into` ("moved before any check") and `checking_waits_for_typing_to_settle`.

- [ ] **Step 3: Implement** in `spelling.rs` (inside `impl Engine`)

```rust
    /// Records that document `id` changed, so it is checked only once it settles, and moves its
    /// misspellings through `edits`, dropping those an edit touches.
    pub(crate) fn spelling_edited(&mut self, id: DocumentId, edits: &[TextEdit]) {
        if !self.spelling.enabled {
            return;
        }
        self.spelling.changed.insert(id, self.clock.now());
        let Some(checked) = self.spelling.results.get_mut(&id) else {
            return;
        };
        let before = checked.misspellings.len();
        let mut moved = false;
        checked.misspellings.retain_mut(|misspelling| {
            let mut delta: isize = 0;
            for edit in edits {
                if edit.range.start <= misspelling.range.end
                    && edit.range.end >= misspelling.range.start
                {
                    return false;
                }
                if edit.range.end <= misspelling.range.start {
                    delta += edit.replacement.len() as isize - edit.range.len() as isize;
                }
            }
            if delta != 0 {
                moved = true;
                misspelling.range = SourceRange::new(
                    misspelling.range.start.saturating_add_signed(delta),
                    misspelling.range.end.saturating_add_signed(delta),
                );
            }
            true
        });
        if moved || checked.misspellings.len() != before {
            self.emit(EngineEvent::SpellingChanged(id));
        }
    }
```

In `engine.rs`'s `apply_edits`, after `self.words_changed(id);`, add `self.spelling_edited(id, &forward);`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adoc-engine --test spelling`
Expected: PASS (all tests, including `checking_waits_for_typing_to_settle`).

- [ ] **Step 5: Commit**

```bash
git add crates/adoc-engine
git commit -m "Move misspellings with edits"
```

---

### Task 6: Word lists, the add commands, and the watcher

**Files:**
- Modify: `crates/adoc-engine/src/spelling.rs`, `src/commands.rs`, `src/workspace.rs`, `src/lib.rs`
- Test: `crates/adoc-engine/tests/spelling.rs`

**Interfaces:**
- Consumes: `WordList::{read, write, insert, contains}`, `Engine::text_in(id, range) -> Option<String>`, `self.roots`, `Settings::user_file()`.
- Produces: `commands::ids::{SPELLING_ADD_TO_WORKSPACE, SPELLING_ADD_TO_USER}` (`"spelling.add_to_workspace"`, `"spelling.add_to_user"`), both taking `CommandArgs::Range { id, range }`; `adoc_engine::{WORKSPACE_WORDS_FILE, USER_WORDS_FILE}`; crate-private `Engine::word_lists_changed(&mut self, paths: &[PathBuf])`.

- [ ] **Step 1: Write the failing tests** (append to `tests/spelling.rs`; extend the `use adoc_engine::{…}` list with `WORKSPACE_WORDS_FILE, USER_WORDS_FILE`)

```rust
fn workspace_page(text: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    let path = root.join("page.adoc");
    fs::write(&path, text).unwrap();
    (directory, root, path)
}

#[test]
fn adding_a_word_to_the_workspace_dictionary_writes_the_list_and_clears_it() {
    let (_directory, root, path) = workspace_page("= Page\n\nWe use Qwzxy daily.\n");
    let (mut engine, _clock) = engine_with_clock();
    engine.open_workspace(vec![root.clone()]).unwrap();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let range = engine.misspellings(id)[0].range;
    engine
        .invoke(ids::SPELLING_ADD_TO_WORKSPACE, CommandArgs::Range { id, range })
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join(WORKSPACE_WORDS_FILE)).unwrap(),
        "Qwzxy\n"
    );
    assert!(engine.misspellings(id).is_empty(), "cleared at once");
    settle(&mut engine);
    assert!(engine.misspellings(id).is_empty(), "and after checking again");
}

#[test]
fn adding_a_word_to_the_user_dictionary_needs_a_settings_file() {
    let (directory, root, path) = workspace_page("= Page\n\nA qwzxy.\n");
    let user = directory.path().join("settings.toml");
    fs::write(&user, "").unwrap();
    let (mut engine, _clock) = engine_with(Settings::load(Some(&user), None).unwrap());
    engine.open_workspace(vec![root]).unwrap();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    let range = engine.misspellings(id)[0].range;
    engine
        .invoke(ids::SPELLING_ADD_TO_USER, CommandArgs::Range { id, range })
        .unwrap();
    assert_eq!(
        fs::read_to_string(directory.path().join(USER_WORDS_FILE)).unwrap(),
        "qwzxy\n"
    );
    assert!(engine.misspellings(id).is_empty());

    let (mut bare, _clock) = engine_with_clock();
    let id = bare.open(&path).unwrap();
    assert!(bare
        .invoke(ids::SPELLING_ADD_TO_USER, CommandArgs::Range { id, range })
        .is_err());
}

#[test]
fn an_edited_workspace_list_is_read_again() {
    let (_directory, root, path) = workspace_page("= Page\n\nWe use Qwzxy daily.\n");
    let (mut engine, _clock) = engine_with_clock();
    engine.open_workspace(vec![root.clone()]).unwrap();
    let id = engine.open(&path).unwrap();
    settle(&mut engine);
    assert_eq!(misspelt(&engine, id), ["Qwzxy"]);
    fs::write(root.join(WORKSPACE_WORDS_FILE), "Qwzxy\n").unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while !engine.misspellings(id).is_empty() && std::time::Instant::now() < deadline {
        settle(&mut engine);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(engine.misspellings(id).is_empty());
}
```

Add `adoc-index` to `adoc-engine`'s `[dev-dependencies]` if it is not there already (check `crates/adoc-engine/Cargo.toml`; other engine tests use `adoc_index::normalize_path`).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 6 -p adoc-engine --test spelling`
Expected: FAIL to compile (`SPELLING_ADD_TO_WORKSPACE`, `WORKSPACE_WORDS_FILE` not defined).

- [ ] **Step 3: Implement**

In `spelling.rs`, add the constants and replace the empty `read_word_lists`:

```rust
/// A workspace root's word list, committed with the repository.
pub const WORKSPACE_WORDS_FILE: &str = ".adocfoundry-words.txt";

/// The user's word list, beside the user settings file.
pub const USER_WORDS_FILE: &str = "spelling-words.txt";
```

```rust
    /// The user's word list file, beside the user settings file.
    fn user_words_path(&self) -> Option<PathBuf> {
        Some(self.settings.user_file()?.parent()?.join(USER_WORDS_FILE))
    }

    /// The word list file of the workspace root holding document `id`.
    fn workspace_words_path(&self, id: DocumentId) -> Option<PathBuf> {
        let path = &self.documents.get(&id)?.path;
        let root = self.roots.iter().find(|root| path.starts_with(root))?;
        Some(root.join(WORKSPACE_WORDS_FILE))
    }

    /// Reads the user's and each workspace root's word list; a list that cannot be read is
    /// empty.
    fn read_word_lists(&mut self) {
        let read = |path: &std::path::Path| Arc::new(WordList::read(path).unwrap_or_default());
        self.spelling.user_words = self
            .user_words_path()
            .map_or_else(Arc::default, |path| read(&path));
        self.spelling.workspace_words = self
            .roots
            .iter()
            .map(|root| (root.clone(), read(&root.join(WORKSPACE_WORDS_FILE))))
            .collect();
    }

    /// Reads a workspace root's list again when the watcher reports it changed.
    pub(crate) fn word_lists_changed(&mut self, paths: &[PathBuf]) {
        let mut changed = false;
        for path in paths {
            let Some(root) = path.parent().filter(|_| {
                path.file_name()
                    .is_some_and(|name| name == WORKSPACE_WORDS_FILE)
            }) else {
                continue;
            };
            if let Some(list) = self.spelling.workspace_words.get_mut(root) {
                *list = Arc::new(WordList::read(path).unwrap_or_default());
                changed = true;
            }
        }
        if changed {
            self.spelling.generation += 1;
        }
    }

    /// Adds the word at `range` of document `id` to the user's list, or to its workspace root's,
    /// writes the list and drops the misspellings it now accepts.
    fn add_word(&mut self, id: DocumentId, range: SourceRange, user: bool) -> Result<(), EngineError> {
        let word = self.text_in(id, range).ok_or(EngineError::UnknownDocument(id))?;
        if word.is_empty() || word.chars().any(char::is_whitespace) || word.chars().count() > 100 {
            return Err(EngineError::InvalidRequest(format!("`{word}` is not a word")));
        }
        let path = if user {
            self.user_words_path()
                .ok_or_else(|| EngineError::InvalidRequest("there is no user settings file".to_owned()))?
        } else {
            self.workspace_words_path(id).ok_or_else(|| {
                EngineError::InvalidRequest("the page is in no workspace folder".to_owned())
            })?
        };
        // The file as it is now, so words added elsewhere are kept.
        let mut list = WordList::read(&path).map_err(|error| spell_error(&path, error))?;
        list.insert(&word);
        list.write(&path).map_err(|error| spell_error(&path, error))?;
        let list = Arc::new(list);
        let applies: Vec<DocumentId> = if user {
            self.spelling.user_words = list.clone();
            self.documents.keys().copied().collect()
        } else {
            let root = path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
            self.spelling.workspace_words.insert(root.clone(), list.clone());
            self.documents
                .iter()
                .filter(|(_, document)| document.path.starts_with(&root))
                .map(|(id, _)| *id)
                .collect()
        };
        self.spelling.generation += 1;
        for id in applies {
            let Some(checked) = self.spelling.results.get_mut(&id) else {
                continue;
            };
            let before = checked.misspellings.len();
            checked.misspellings.retain(|misspelling| !list.contains(&misspelling.word));
            if checked.misspellings.len() != before {
                self.emit(EngineEvent::SpellingChanged(id));
            }
        }
        Ok(())
    }
```

and outside the `impl`:

```rust
fn spell_error(path: &std::path::Path, error: adoc_spell::SpellError) -> EngineError {
    match error {
        adoc_spell::SpellError::Io { source, .. } => EngineError::Io {
            path: path.to_path_buf(),
            source,
        },
        other => EngineError::InvalidRequest(other.to_string()),
    }
}

pub(crate) fn register_spelling(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::SPELLING_ADD_TO_WORKSPACE,
            "Add Word to Workspace Dictionary",
            CommandCategory::Edit,
            None,
            CommandContext::ActiveDocument,
        ),
        |engine, args| add_word_command(engine, args, false),
    );
    register(
        registry,
        spec(
            ids::SPELLING_ADD_TO_USER,
            "Add Word to User Dictionary",
            CommandCategory::Edit,
            None,
            CommandContext::ActiveDocument,
        ),
        |engine, args| add_word_command(engine, args, true),
    );
}

/// Takes `CommandArgs::Range`: the word's range in a document.
fn add_word_command(
    engine: &mut Engine,
    args: CommandArgs,
    user: bool,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Range { id, range } = args else {
        return Err(EngineError::InvalidArguments {
            command: if user {
                ids::SPELLING_ADD_TO_USER
            } else {
                ids::SPELLING_ADD_TO_WORKSPACE
            },
            expected: "a document and the range of a word",
        });
    };
    engine.add_word(id, range, user)?;
    Ok(CommandOutcome::Done)
}
```

Extend the `use crate::{…}` of `spelling.rs` with `commands::{ids, register, spec}, CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, EngineError`. If `register`'s closure type will not accept a capturing-free closure, write two `fn`s (`add_to_workspace_command`, `add_to_user_command`) calling `add_word_command`.

In `commands.rs`: in `mod ids`, add

```rust
    pub const SPELLING_ADD_TO_WORKSPACE: CommandId = CommandId::new("spelling.add_to_workspace");
    pub const SPELLING_ADD_TO_USER: CommandId = CommandId::new("spelling.add_to_user");
```

and in `register_builtin`, after `crate::html_export::register_html_export(registry);`, add `crate::spelling::register_spelling(registry);`.

In `workspace.rs`'s `apply_file_changes`, after `self.images_changed(&listed);`, add `self.word_lists_changed(&listed);`.

In `lib.rs`, extend the `pub use spelling::{…}` with `USER_WORDS_FILE, WORKSPACE_WORDS_FILE`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adoc-engine --test spelling && cargo test -j 6 -p adoc-engine`
Expected: PASS. If a test listing every command id (search `tests/` for `commands().all()`) fails, add the two ids where it expects them.

- [ ] **Step 5: Commit**

```bash
git add crates/adoc-engine
git commit -m "Add words to the workspace and user dictionaries"
```

---

### Task 7: The doctor reports spelling

**Files:**
- Modify: `crates/adoc-engine/src/doctor.rs`
- Test: `crates/adoc-engine/tests/doctor.rs`

**Interfaces:**
- Consumes: `Settings::spelling`, `adoc_spell::{Dictionaries, DictionarySource}`.
- Produces: a `Spelling` section in `DoctorReport` with lines `Spelling` (`on`/`off`), `Language`, and one `Dictionary` per dictionary (`en_GB (built in)` or `name (path)`).

- [ ] **Step 1: Write the failing test** (append to `tests/doctor.rs`, reusing its imports)

```rust
#[test]
fn the_report_lists_the_dictionaries() {
    let report = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in())
        .doctor()
        .run();
    let spelling = report.section("Spelling").expect("a spelling section");
    assert_eq!(spelling.value("Spelling"), Some("on"));
    assert_eq!(spelling.value("Language"), Some("en_GB"));
    let dictionaries: Vec<&str> = spelling.values("Dictionary").collect();
    assert_eq!(dictionaries, ["en_GB (built in)", "en_US (built in)"]);
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -j 6 -p adoc-engine --test doctor the_report_lists_the_dictionaries`
Expected: FAIL ("a spelling section").

- [ ] **Step 3: Implement**

Add `spelling: DoctorSection,` to `struct Doctor`. In `Engine::doctor`, before building `Doctor`:

```rust
        let mut spelling = DoctorSection::new("Spelling")
            .line("Spelling", if settings.spelling.enabled { "on" } else { "off" })
            .line("Language", settings.spelling.language.clone());
        let folder = settings.spelling.dictionaries.as_deref().map(Path::new);
        for info in adoc_spell::Dictionaries::new(folder).list() {
            let source = match info.source {
                adoc_spell::DictionarySource::Bundled => "built in".to_owned(),
                adoc_spell::DictionarySource::File(path) => path.display().to_string(),
            };
            spelling = spelling.line("Dictionary", format!("{} ({source})", info.name));
        }
```

and `spelling,` in the `Doctor { … }` literal. In `Doctor::run`, push `self.spelling.clone()` into the report's sections immediately after the Settings section (find where `self.settings` is pushed and add the spelling section on the next line). Import `std::path::Path` if not already.

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adoc-engine --test doctor && cargo test -j 6 -p adocfoundry-cli --test cli`
Expected: PASS. If a CLI `doctor` snapshot changes, review it with `cargo insta review` (or read the `.snap.new`) and accept only the added Spelling section.

- [ ] **Step 5: Commit**

```bash
git add crates/adoc-engine crates/adocfoundry-cli
git commit -m "Report spelling in the doctor"
```

---

### Task 8: `adoc-ls` publishes hints and offers quick fixes

**Files:**
- Modify: `crates/adoc-ls/src/protocol.rs`, `crates/adoc-ls/src/capabilities.rs`

**Interfaces:**
- Consumes: `Engine::{enable_spelling, misspellings}`, `EngineEvent::SpellingChanged`, `SPELLING_CODE`, `ids::{SPELLING_ADD_TO_WORKSPACE, SPELLING_ADD_TO_USER}`.
- Produces: commands `adoc.spelling.addToWorkspace` and `adoc.spelling.addToUser`, arguments `[uri, start, end]` (byte offsets).

- [ ] **Step 1: Write the failing tests** in `protocol.rs`'s `mod tests`

```rust
    fn spelling_server(directory: &Path) -> ProtocolServer {
        let user = directory.join("settings.toml");
        std::fs::write(&user, "").unwrap();
        let settings = Settings::load(Some(&user), None).unwrap().with_git_opt_in();
        let mut engine = Engine::with_clock(
            settings,
            std::sync::Arc::new(adoc_engine::ManualClock::new()),
        );
        engine.enable_spelling();
        ProtocolServer::new(
            PositionEncoding::Utf16,
            engine,
            Box::new(BrowserSink::new(directory.join("previews"), Unopened)),
        )
    }

    /// The spelling diagnostics of each publish, by URI.
    fn spelling_hints(notifications: Vec<Notification>) -> Vec<(String, Vec<Diagnostic>)> {
        notifications
            .into_iter()
            .map(|notification| {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(notification.params).unwrap();
                let hints = params
                    .diagnostics
                    .into_iter()
                    .filter(|diagnostic| {
                        diagnostic.code
                            == Some(NumberOrString::String(adoc_engine::SPELLING_CODE.to_owned()))
                    })
                    .collect();
                (params.uri.as_str().to_owned(), hints)
            })
            .collect()
    }

    #[test]
    fn misspellings_are_published_as_hints_with_quick_fixes() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        let page = root.join("page.adoc");
        let text = "= Page\n\nWe recieve it.\n";
        std::fs::write(&page, text).unwrap();
        let mut server = spelling_server(&root);
        server.open_workspace(vec![root.clone()]);
        let uri = file_uri(&page);
        open(&mut server, uri.as_str(), text);
        server.tick();
        assert!(server.engine.wait_for_workers(std::time::Duration::from_secs(30)));
        let published = spelling_hints(server.published(None));
        let (_, hints) = published
            .iter()
            .find(|(published, _)| published == uri.as_str())
            .expect("a publish for the page");
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].severity, Some(DiagnosticSeverity::HINT));
        assert_eq!(hints[0].message, "Unknown word: recieve");
        assert_eq!(
            hints[0].range,
            Range::new(Position::new(2, 3), Position::new(2, 10))
        );

        let actions = server.code_action_response(&CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: hints[0].range,
            context: CodeActionContext::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        });
        let titles: Vec<String> = actions
            .iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title.clone()),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(titles.contains(&"Change to 'receive'".to_owned()), "{titles:?}");
        assert!(titles.contains(&"Add 'recieve' to the workspace dictionary".to_owned()));
        assert!(titles.contains(&"Add 'recieve' to the user dictionary".to_owned()));
        let receive = actions
            .iter()
            .find_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) if action.title == "Change to 'receive'" => {
                    action.edit.clone()
                }
                _ => None,
            })
            .unwrap();
        let edits = &receive.changes.unwrap()[&uri];
        assert_eq!(edits[0].new_text, "receive");
        assert_eq!(edits[0].range, hints[0].range);
    }

    #[test]
    fn the_add_to_workspace_command_writes_the_list() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        let page = root.join("page.adoc");
        let text = "= Page\n\nWe recieve it.\n";
        std::fs::write(&page, text).unwrap();
        let mut server = spelling_server(&root);
        server.open_workspace(vec![root.clone()]);
        let uri = file_uri(&page);
        open(&mut server, uri.as_str(), text);
        server.tick();
        assert!(server.engine.wait_for_workers(std::time::Duration::from_secs(30)));
        server.published(None);
        let start = text.find("recieve").unwrap();
        let (response, notifications) = server.execute_command_response(Request::new(
            RequestId::from(7),
            ExecuteCommand::METHOD.to_owned(),
            ExecuteCommandParams {
                command: crate::capabilities::ADD_TO_WORKSPACE_COMMAND.to_owned(),
                arguments: vec![
                    serde_json::json!(uri.as_str()),
                    serde_json::json!(start),
                    serde_json::json!(start + "recieve".len()),
                ],
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        ));
        assert!(response.error.is_none(), "{:?}", response.error);
        assert_eq!(
            std::fs::read_to_string(root.join(adoc_engine::WORKSPACE_WORDS_FILE)).unwrap(),
            "recieve\n"
        );
        let published = spelling_hints(notifications);
        assert!(published
            .iter()
            .any(|(published, hints)| published == uri.as_str() && hints.is_empty()));
    }
```

`file_uri` stands for the module's existing path-to-URI helper (the function around line 1380 that builds `Uri::from_str(url::Url::from_file_path(path)…)`); use its real name. Add any missing `lsp_types` imports to the test module (`CodeActionContext`, `CodeActionParams`, `CodeActionOrCommand`, `Diagnostic`, `DiagnosticSeverity`, `ExecuteCommandParams`, `PartialResultParams`, `TextDocumentIdentifier`, `WorkDoneProgressParams`, `request::ExecuteCommand`). Add `tempfile` and `adoc-index` to `adoc-ls`'s `[dev-dependencies]` if missing.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 6 -p adoc-ls --lib spelling`
Expected: FAIL to compile (`ADD_TO_WORKSPACE_COMMAND` not defined).

- [ ] **Step 3: Implement**

`capabilities.rs`, beside `CHECK_WORKSPACE_COMMAND`:

```rust
/// Adds a misspelt word to the workspace dictionary: `[uri, start, end]`, byte offsets.
pub const ADD_TO_WORKSPACE_COMMAND: &str = "adoc.spelling.addToWorkspace";
/// Adds a misspelt word to the user dictionary: `[uri, start, end]`, byte offsets.
pub const ADD_TO_USER_COMMAND: &str = "adoc.spelling.addToUser";
```

and add both to the list `commands(can_export)` returns.

`protocol.rs`:

- In `run_connection`, replace `Engine::new(settings),` with an engine built first: `let mut engine = Engine::new(settings); engine.enable_spelling();` and pass `engine`.
- In `published`, collect ids for both events:

```rust
        for event in self.events.try_iter() {
            let id = match event {
                EngineEvent::DiagnosticsUpdated { path } => self.engine.document_for_path(&path),
                EngineEvent::SpellingChanged(id) => Some(id),
                _ => None,
            };
            if let Some(id) = id.filter(|id| !ids.contains(id)) {
                ids.push(id);
            }
        }
```

- In `diagnostics_notification`, make `diagnostics` a `Vec<Diagnostic>` (add `: Vec<Diagnostic>` and keep `.collect()`), then before building the notification:

```rust
        let text = &document.parsed.text;
        diagnostics.extend(self.engine.misspellings(id).iter().filter_map(|misspelling| {
            Some(Diagnostic {
                range: self.encoding.range(text, misspelling.range)?,
                severity: Some(DiagnosticSeverity::HINT),
                code: Some(NumberOrString::String(adoc_engine::SPELLING_CODE.to_owned())),
                source: Some("adoc-ls".to_owned()),
                message: format!("Unknown word: {}", misspelling.word),
                ..Diagnostic::default()
            })
        }));
```

  (declare `let mut diagnostics`).
- In `code_action_response`, build the spelling actions first and chain the preview actions after them:

```rust
    fn spelling_actions(&self, params: &CodeActionParams) -> Vec<CodeActionOrCommand> {
        let uri = &params.text_document.uri;
        let Some(id) = self.id_for(uri.as_str()) else {
            return Vec::new();
        };
        let Some(text) = self.text_of(id) else {
            return Vec::new();
        };
        let (Some(start), Some(end)) = (
            self.encoding.offset(text, params.range.start),
            self.encoding.offset(text, params.range.end),
        ) else {
            return Vec::new();
        };
        let user = self.engine.settings().user_file().is_some();
        let mut actions = Vec::new();
        for misspelling in self.engine.misspellings(id) {
            if misspelling.range.start > end || misspelling.range.end < start {
                continue;
            }
            let Some(range) = self.encoding.range(text, misspelling.range) else {
                continue;
            };
            for suggestion in &misspelling.suggestions {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Change to '{suggestion}'"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    edit: Some(WorkspaceEdit {
                        changes: Some(
                            [(uri.clone(), vec![TextEdit::new(range, suggestion.clone())])]
                                .into_iter()
                                .collect(),
                        ),
                        ..WorkspaceEdit::default()
                    }),
                    ..CodeAction::default()
                }));
            }
            let mut add = vec![(
                format!("Add '{}' to the workspace dictionary", misspelling.word),
                crate::capabilities::ADD_TO_WORKSPACE_COMMAND,
            )];
            if user {
                add.push((
                    format!("Add '{}' to the user dictionary", misspelling.word),
                    crate::capabilities::ADD_TO_USER_COMMAND,
                ));
            }
            for (title, command) in add {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: title.clone(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    command: Some(Command {
                        title,
                        command: command.to_owned(),
                        arguments: Some(vec![
                            serde_json::Value::String(uri.as_str().to_owned()),
                            serde_json::json!(misspelling.range.start),
                            serde_json::json!(misspelling.range.end),
                        ]),
                    }),
                    ..CodeAction::default()
                }));
            }
        }
        actions
    }
```

  In `code_action_response`, keep its early return, then `let mut actions = self.spelling_actions(params);`, `actions.extend(code_actions_for(uri, live).into_iter().map(…))` and return `actions`. Import `TextEdit` and `WorkspaceEdit` from `lsp_types` (alias `TextEdit` as `LspTextEdit` if the name clashes with an engine import).
- In `execute_command_response`, before the preview `mode` match:

```rust
            ADD_TO_WORKSPACE_COMMAND => return self.add_word(id, &params, false),
            ADD_TO_USER_COMMAND => return self.add_word(id, &params, true),
```

  (in the existing `match command { EXPORT_PDF_COMMAND … }`), and the method:

```rust
    fn add_word(
        &mut self,
        request: RequestId,
        params: &ExecuteCommandParams,
        user: bool,
    ) -> (Response, Vec<Notification>) {
        let arguments = &params.arguments;
        let (Some(uri), Some(start), Some(end)) = (
            arguments.first().and_then(serde_json::Value::as_str),
            arguments.get(1).and_then(serde_json::Value::as_u64),
            arguments.get(2).and_then(serde_json::Value::as_u64),
        ) else {
            return (
                Response::new_err(
                    request,
                    ErrorCode::InvalidParams as i32,
                    "expected a document URI and the start and end of a word".to_owned(),
                ),
                Vec::new(),
            );
        };
        let Some(id) = self.id_for(uri) else {
            return (
                Response::new_err(
                    request,
                    ErrorCode::InvalidParams as i32,
                    format!("{uri} is not open"),
                ),
                Vec::new(),
            );
        };
        let command = if user {
            adoc_engine::commands::ids::SPELLING_ADD_TO_USER
        } else {
            adoc_engine::commands::ids::SPELLING_ADD_TO_WORKSPACE
        };
        let range = SourceRange::new(start as usize, end as usize);
        match self.engine.invoke(command, CommandArgs::Range { id, range }) {
            Ok(_) => (
                Response::new_ok(request, serde_json::Value::Null),
                self.published(None),
            ),
            Err(error) => (
                Response::new_err(request, ErrorCode::RequestFailed as i32, error.to_string()),
                Vec::new(),
            ),
        }
    }
```

  Import the two command constants from `crate::capabilities`, and `CommandArgs`/`SourceRange` if not already imported.

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adoc-ls`
Expected: PASS, including `every_code_action_command_is_advertised` and the stdio tests (the binary now checks spelling; existing tests should only gain hint diagnostics they do not inspect — if one asserts an exact diagnostic list on prose with unknown words, filter out code `adoc.spelling` there).

- [ ] **Step 5: Commit**

```bash
git add crates/adoc-ls
git commit -m "Publish spelling hints and quick fixes from adoc-ls"
```

---

### Task 9: The source editor underlines and offers suggestions

**Files:**
- Create: `crates/adocfoundry/src/spelling.rs`
- Modify: `crates/adocfoundry/src/main.rs` (module), `src/app.rs`, `src/editor/mod.rs`
- Test: `crates/adocfoundry/src/ui_tests.rs`

**Interfaces:**
- Consumes: `Engine::misspellings`, `SPELLING_CODE`, `EngineEvent::SpellingChanged`, the two `spelling.*` command ids, `ids::EDIT_REPLACE_RANGE`.
- Produces: `crate::spelling::{Choice, choices(engine: &Engine, document: DocumentId, offset: usize) -> Vec<(String, Choice)>, apply(app: &Entity<AppState>, document: DocumentId, choice: &Choice, cx: &mut App) -> Result<(), EngineError>, ApplySpelling { document, choice }}` (Task 10 uses `choices`, `apply`, `Choice`).

- [ ] **Step 1: Write the failing test** (append to `ui_tests.rs`)

```rust
#[gpui::test]
fn a_misspelt_word_is_underlined_and_replaced_in_one_undo_step(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    let text = "= Page\n\nWe recieve it.\n";
    std::fs::write(&path, text).unwrap();
    let mut engine = Engine::new(Settings::default());
    let document = engine.open(&path).unwrap();
    let app = app_with(engine, cx);
    let (editor, window) = editor_window(&app, document, cx);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    settle_engine(&app, &mut cx);
    cx.executor().advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| editor.read(cx).spelling_hints_for_test(cx)),
        ["Unknown word: recieve"]
    );

    let start = text.find("recieve").unwrap();
    let choices = cx.update(|_, cx| {
        crate::spelling::choices(&app.read(cx).engine, document, start + 2)
    });
    assert_eq!(
        choices.last().map(|(label, _)| label.as_str()),
        Some("Add to Workspace Dictionary")
    );
    let (first, choice) = choices[0].clone();
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.reveal(SourceRange::new(start, start), window, cx));
    });
    cx.dispatch_action(crate::spelling::ApplySpelling { document, choice });
    cx.run_until_parked();
    let replaced = format!("= Page\n\nWe {first} it.\n");
    assert_eq!(document_text(&app, &mut cx), replaced);
    assert_eq!(widget_text(&editor, &mut cx), replaced);

    cx.simulate_keystrokes("secondary-z");
    cx.run_until_parked();
    assert_eq!(document_text(&app, &mut cx), text);
}
```

With no user settings file in tests, "Add to User Dictionary" is absent, so "Add to Workspace Dictionary" is last.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -j 6 -p adocfoundry a_misspelt_word_is_underlined`
Expected: FAIL to compile (`crate::spelling` not defined).

- [ ] **Step 3: Implement**

`crates/adocfoundry/src/spelling.rs`:

```rust
//! What both editors offer on a misspelt word, and carrying it out through the engine: a
//! suggestion replaces the word in one undo step; a dictionary takes the word.

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, CommandArgs, DocumentId, Engine, EngineError};
use gpui::{App, Entity};

use crate::app::AppState;

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Replace { range: SourceRange, text: String },
    AddToWorkspace(SourceRange),
    AddToUser(SourceRange),
}

/// A spelling choice made in the source editor's menu, whose items are actions.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = adocfoundry, no_json)]
pub struct ApplySpelling {
    pub document: DocumentId,
    pub choice: Choice,
}

/// The choices for the misspelt word at `offset` of `document`: its suggestions, then the
/// dictionaries it may be added to (the user's only with a user settings file). Empty when no
/// misspelt word is there.
pub fn choices(engine: &Engine, document: DocumentId, offset: usize) -> Vec<(String, Choice)> {
    let Some(misspelling) = engine
        .misspellings(document)
        .iter()
        .find(|misspelling| misspelling.range.start <= offset && offset <= misspelling.range.end)
    else {
        return Vec::new();
    };
    let range = misspelling.range;
    let mut choices: Vec<(String, Choice)> = misspelling
        .suggestions
        .iter()
        .map(|text| {
            (
                text.clone(),
                Choice::Replace {
                    range,
                    text: text.clone(),
                },
            )
        })
        .collect();
    choices.push((
        "Add to Workspace Dictionary".to_owned(),
        Choice::AddToWorkspace(range),
    ));
    if engine.settings().user_file().is_some() {
        choices.push(("Add to User Dictionary".to_owned(), Choice::AddToUser(range)));
    }
    choices
}

/// Carries out `choice` in `document`.
pub fn apply(
    app: &Entity<AppState>,
    document: DocumentId,
    choice: &Choice,
    cx: &mut App,
) -> Result<(), EngineError> {
    let (command, args) = match choice.clone() {
        Choice::Replace { range, text } => (
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id: document,
                range,
                text,
            },
        ),
        Choice::AddToWorkspace(range) => (
            ids::SPELLING_ADD_TO_WORKSPACE,
            CommandArgs::Range { id: document, range },
        ),
        Choice::AddToUser(range) => (
            ids::SPELLING_ADD_TO_USER,
            CommandArgs::Range { id: document, range },
        ),
    };
    app.update(cx, |app, cx| app.invoke(command, args, cx))
        .map(|_| ())
}
```

Declare `mod spelling;` in `main.rs` beside the other modules.

`app.rs`: after `engine.enable_word_count();` add

```rust
        // Both editors underline misspelt words.
        engine.enable_spelling();
```

`editor/mod.rs`:

1. In the engine subscription, beside the `DiagnosticsUpdated` arm, redraw on spelling changes with the same delay:

```rust
                EngineEvent::SpellingChanged(id) if *id == this.document => {
                    this._diagnostics = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(DIAGNOSTICS_DELAY).await;
                        let _ = this.update(cx, |this, cx| this.show_diagnostics(cx));
                    }));
                }
```

2. In `show_diagnostics`, read the misspellings with the diagnostics (`let misspellings = engine.misspellings(self.document).to_vec();` — restructure the first `let` to read both from one `self.app.read(cx).engine`), and after the `set.extend(diagnostics…)` call:

```rust
            set.extend(misspellings.into_iter().map(|misspelling| {
                let start = text.offset_to_position(misspelling.range.start.min(text.len()));
                let end = text.offset_to_position(misspelling.range.end.min(text.len()));
                Diagnostic::new(start..end, format!("Unknown word: {}", misspelling.word))
                    .with_severity(WidgetSeverity::Hint)
                    .with_code(adoc_engine::SPELLING_CODE)
            }));
```

3. The test accessor, beside the other `#[cfg(test)]` helpers of `SourceEditorView`:

```rust
    /// The messages of the spelling hints the editor shows.
    #[cfg(test)]
    pub fn spelling_hints_for_test(&self, cx: &App) -> Vec<String> {
        self.state
            .read(cx)
            .diagnostics()
            .map(|set| {
                set.iter()
                    .filter(|entry| {
                        entry.diagnostic.code.as_deref() == Some(adoc_engine::SPELLING_CODE)
                    })
                    .map(|entry| entry.diagnostic.message.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
```

4. The context menu. In `render`, give the `Editor::new(&self.state)` chain a `.context_menu(…)`:

```rust
                    .context_menu({
                        let app = self.app.clone();
                        let state = self.state.clone();
                        let document = self.document;
                        let readonly = self.readonly;
                        move |menu, _, cx| context_menu(menu, &app, &state, document, readonly, cx)
                    }),
```

   and add the function to the module:

```rust
/// The editor's menu: on a misspelt word its suggestions and dictionaries first, then the
/// editor's own items, which a custom menu must list itself.
fn context_menu(
    mut menu: gpui_component::input::NativeMenu,
    app: &Entity<AppState>,
    state: &Entity<EditorState>,
    document: DocumentId,
    readonly: bool,
    cx: &App,
) -> gpui_component::input::NativeMenu {
    use gpui_component::input::{Copy, Cut, GoToDefinition, Paste, SelectAll};
    let editor = state.read(cx);
    if !readonly {
        let choices = crate::spelling::choices(&app.read(cx).engine, document, editor.cursor());
        let mut suggested = false;
        for (label, choice) in choices.iter().cloned() {
            if !matches!(choice, crate::spelling::Choice::Replace { .. }) && suggested {
                menu = menu.separator();
                suggested = false;
            }
            suggested |= matches!(choice, crate::spelling::Choice::Replace { .. });
            menu = menu.menu(label, Box::new(crate::spelling::ApplySpelling { document, choice }));
        }
        if !choices.is_empty() {
            menu = menu.separator();
        }
    }
    let copyable = !editor.selected_range().is_empty();
    menu.menu("Go to Definition", Box::new(GoToDefinition))
        .separator()
        .menu_with_disabled("Cut", readonly || !copyable, Box::new(Cut))
        .menu_with_disabled("Copy", !copyable, Box::new(Copy))
        .menu_with_disabled("Paste", readonly, Box::new(Paste))
        .separator()
        .menu("Select All", Box::new(SelectAll))
}
```

   `NativeMenu` is `gpui_base::input::NativeMenu`; if gpui-component does not re-export it, add `gpui-base` (at the version in `Cargo.lock`, `=0.8.0`) to `[workspace.dependencies]` and `adocfoundry`'s dependencies — it is one of the permitted GPUI crates. If `selected_range()` is not public on `EditorState`, use the accessor it offers for the selection (check `gpui-base`'s `input/editor/mod.rs`). Keep `readonly` as a field of `SourceEditorView` set in `new` (add `readonly: bool,` if the view does not already keep it).

5. Handle the action on the view's root element (the `h_flex()` returned by `render`):

```rust
            .on_action(cx.listener(|this, action: &crate::spelling::ApplySpelling, window, cx| {
                if action.document != this.document {
                    return;
                }
                if let Err(error) =
                    crate::spelling::apply(&this.app, action.document, &action.choice, cx)
                {
                    window.push_notification(error.to_string(), cx);
                }
            }))
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adocfoundry a_misspelt_word && cargo test -j 6 -p adocfoundry`
Expected: PASS. `menus::tests::every_command_is_in_a_menu` passes without listing the spelling commands, because they have no keymap action (`action_for` returns `None`); if it does not, add both ids to `NOT_IN_MENUS`.

- [ ] **Step 5: Commit**

```bash
git add crates/adocfoundry Cargo.toml Cargo.lock
git commit -m "Underline misspelt words in the source editor with suggestions"
```

---

### Task 10: The visual editor underlines and offers suggestions

**Files:**
- Modify: `crates/adocfoundry/src/visual/text.rs`, `src/visual/blocks.rs`, `src/visual/mod.rs`
- Test: `crates/adocfoundry/src/visual/text.rs` (unit), `crates/adocfoundry/src/ui_tests.rs`

**Interfaces:**
- Consumes: `crate::spelling::{choices, apply, Choice}` (Task 9), `TextBlock::display_offset`, `Shown::index`, `VisualView::offset_at`.
- Produces: `text::with_underlines(runs: Vec<TextRun>, ranges: &[Range<usize>], colour: Hsla) -> Vec<TextRun>`; `VisualView::misspelt(&self, document: &VisualDocument, cx: &App) -> Vec<SourceRange>`.

- [ ] **Step 1: Write the failing tests**

In `visual/text.rs`'s `mod tests`:

```rust
    #[test]
    fn underlines_split_the_runs_they_cover() {
        let run = TextRun {
            len: 10,
            font: style().font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let red = gpui::red();
        let runs = with_underlines(vec![run.clone(), TextRun { len: 4, ..run }], &[2..5, 9..12], red);
        let lengths: Vec<usize> = runs.iter().map(|run| run.len).collect();
        assert_eq!(lengths, [2, 3, 4, 1, 2, 2]);
        let wavy: Vec<bool> = runs
            .iter()
            .map(|run| run.underline.is_some_and(|underline| underline.wavy))
            .collect();
        assert_eq!(wavy, [false, true, false, true, true, false]);
        assert_eq!(runs[1].underline.unwrap().color, Some(red));
    }
```

In `ui_tests.rs`:

```rust
#[gpui::test]
fn a_misspelt_word_in_the_visual_editor_is_underlined_and_replaced(cx: &mut TestAppContext) {
    let text = "= Page\n\nWe recieve it.\n";
    let (app, _workspace, visual, id, mut cx) = visual_workspace(text, cx);
    settle_engine(&app, &mut cx);
    let start = text.find("recieve").unwrap();
    let underlined = cx.update(|_, cx| {
        let visual = visual.read(cx);
        visual
            .shown()
            .map(|shown| visual.misspelt(shown, cx))
            .unwrap_or_default()
    });
    assert_eq!(underlined, [SourceRange::new(start, start + "recieve".len())]);

    let first = cx.update(|_, cx| {
        crate::spelling::choices(&app.read(cx).engine, id, start)[0].0.clone()
    });
    // Right-click the word, then take the first item.
    let position = cx.update(|_, cx| {
        let layouts = visual.read(cx).layouts().borrow();
        layouts.get(&vec![1]).expect("the paragraph is laid out")
            .position_for_index("We re".len())
            .expect("laid out")
            + gpui::point(px(1.), px(2.))
    });
    let button = gpui::MouseButton::Right;
    cx.simulate_event(gpui::MouseDownEvent {
        button,
        position,
        modifiers: gpui::Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button,
        position,
        modifiers: gpui::Modifiers::none(),
        click_count: 1,
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(text_of(&app, id, &mut cx), format!("= Page\n\nWe {first} it.\n"));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| app.invoke(ids::EDIT_UNDO, CommandArgs::Document(id), cx))
    })
    .unwrap();
    assert_eq!(text_of(&app, id, &mut cx), text);
}
```

(The title is block `[0]` and the paragraph `[1]`; if `shown_blocks` shows otherwise, use the paragraph's path. Import `ids`/`CommandArgs` in the test module if needed.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 6 -p adocfoundry underlines_split && cargo test -j 6 -p adocfoundry visual_editor_is_underlined`
Expected: FAIL to compile (`with_underlines`, `misspelt` not defined).

- [ ] **Step 3: Implement**

`visual/text.rs`, after `text_runs`:

```rust
/// `runs` with a wavy underline in `colour` under each of `ranges` (indices into the shown
/// text), splitting runs where a range begins or ends.
#[must_use]
pub fn with_underlines(runs: Vec<TextRun>, ranges: &[Range<usize>], colour: Hsla) -> Vec<TextRun> {
    if ranges.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + ranges.len() * 2);
    let mut at = 0;
    for run in runs {
        let end = at + run.len;
        let mut cuts: Vec<usize> = ranges
            .iter()
            .flat_map(|range| [range.start, range.end])
            .filter(|cut| *cut > at && *cut < end)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(end);
        let mut start = at;
        for cut in cuts {
            let mut piece = run.clone();
            piece.len = cut - start;
            if ranges.iter().any(|range| range.start <= start && cut <= range.end) {
                piece.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(colour),
                    wavy: true,
                });
            }
            out.push(piece);
            start = cut;
        }
        at = end;
    }
    out
}
```

`visual/mod.rs`:

- Field on `VisualView`: `spelling_menu: Option<(gpui::Point<gpui::Pixels>, Entity<gpui_component::menu::PopupMenu>, Subscription)>,` initialised `None` in `new`.
- Methods:

```rust
    /// The misspellings of the document, when the model shown is of its current version (their
    /// ranges are the current text's).
    pub fn misspelt(&self, document: &VisualDocument, cx: &App) -> Vec<SourceRange> {
        let engine = &self.app.read(cx).engine;
        let current = engine
            .document(self.document)
            .is_some_and(|open| open.version == document.version);
        if !current {
            return Vec::new();
        }
        engine
            .misspellings(self.document)
            .iter()
            .map(|misspelling| misspelling.range)
            .collect()
    }

    /// Opens the spelling menu for the misspelt word under `position` in the text block at
    /// `path`, if there is one.
    fn spelling_menu(
        &mut self,
        path: &[usize],
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(offset) = self.offset_at(path, position) else {
            return;
        };
        let choices = crate::spelling::choices(&self.app.read(cx).engine, self.document, offset);
        if choices.is_empty() {
            return;
        }
        let app = self.app.clone();
        let document = self.document;
        let menu = gpui_component::menu::PopupMenu::build(window, cx, move |mut menu, _, _| {
            let mut suggested = false;
            for (label, choice) in choices {
                let replace = matches!(choice, crate::spelling::Choice::Replace { .. });
                if !replace && suggested {
                    menu = menu.separator();
                    suggested = false;
                }
                suggested |= replace;
                let app = app.clone();
                menu = menu.item(gpui_component::menu::PopupMenuItem::new(label).on_click(
                    move |_, window, cx| {
                        if let Err(error) = crate::spelling::apply(&app, document, &choice, cx) {
                            window.push_notification(error.to_string(), cx);
                        }
                    },
                ));
            }
            menu
        });
        let closed = cx.subscribe_in(&menu, window, |this, _, _: &gpui::DismissEvent, _, cx| {
            this.spelling_menu = None;
            cx.notify();
        });
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.spelling_menu = Some((position, menu, closed));
        cx.notify();
    }
```

- In `render`, on the root `v_flex()` after `.child(self.toolbar(cx))` chain:

```rust
            .when_some(
                self.spelling_menu
                    .as_ref()
                    .map(|(position, menu, _)| (*position, menu.clone())),
                |view, (position, menu)| {
                    view.child(gpui::deferred(
                        gpui::anchored()
                            .position(position)
                            .snap_to_window_with_margin(px(8.))
                            .child(div().occlude().child(menu)),
                    ))
                },
            )
```

`visual/blocks.rs`:

- Add to `Draw`: `misspelt: Vec<SourceRange>,` and `spelling: Hsla,`, set in `document()` as `misspelt: view.misspelt(document, cx), spelling: cx.theme().danger,`.
- In `text_inner`, after `let mut runs = text::text_runs(…);` (and after the code highlighting), add:

```rust
        let underlined: Vec<Range<usize>> = draw
            .misspelt
            .iter()
            .filter(|range| range.start >= block.range.start && range.end <= block.range.end)
            .filter_map(|range| {
                let start = shown.index(block.display_offset(range.start)?);
                let end = shown.index(block.display_offset(range.end)?);
                Some(start..end)
            })
            .collect();
        runs = text::with_underlines(runs, &underlined, draw.spelling);
```

- On the text block element, beside its `.on_mouse_down(MouseButton::Left, …)`:

```rust
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.spelling_menu(&menu_path, event.position, window, cx);
                }),
            )
```

  with `let menu_path = path.clone();` beside the existing `down_path`/`move_path` clones. Import `SourceRange`, `Range` and `Hsla` where needed; make `spelling_menu` `pub(super)` if `blocks.rs` cannot see it.

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 6 -p adocfoundry`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adocfoundry
git commit -m "Underline misspelt words in the visual editor with suggestions"
```

---

### Task 11: Packaging, documentation and the full gate

**Files:**
- Modify: `xtask/src/package.rs`, `README.md`, `CHANGELOG.md`, `CLAUDE.md` (crate ladder only), `docs/design/DESIGN-AdocFoundry_Architecture.md`, `docs/design/DESIGN-Spell_Checking.md` (implementation notes)
- Modify (through the asciidoc-antora-writer agent): `src/docs/modules/ROOT/pages/settings.adoc`, `src/docs/modules/ROOT/pages/using-the-editor.adoc`, the developer module's crate ladder page

- [ ] **Step 1: Copy the licence into release archives**

In `xtask/src/package.rs`, after `copy(root.join("README.md"), "README.md")?;`:

```rust
    fs::create_dir_all(staging.join("licenses")).map_err(|error| error.to_string())?;
    copy(
        root.join("crates/adoc-spell/dictionaries/README.md"),
        "licenses/SCOWL.txt",
    )?;
```

Update the module comment's list of staged files to mention `licenses/SCOWL.txt`. Run `cargo test -j 6 -p xtask`.

- [ ] **Step 2: README and CHANGELOG**

- `README.md`: remove "spell checking" from the not-implemented line ("Editing included content in place, spell checking, plugins." → "Editing included content in place, plugins."); add a feature bullet describing spelling (prose only, both editors, suggestions, Add to Workspace/User Dictionary, `:lang:`, bundled en_GB/en_US, `adoc-ls` hints); add the `[spelling]` block to the settings example:

```toml
[spelling]
enabled = true                   # underline misspelt words in prose
language = "en_GB"               # for pages with no lang attribute
dictionaries = ""                # folder of extra Hunspell .aff/.dic files (user file only)
```

- `CHANGELOG.md`, Unreleased › Added: "Spell checking of prose in the source and visual editors and in `adoc-ls`: misspelt words are underlined, with suggestions and Add to Workspace Dictionary or Add to User Dictionary in the context menu. Pages are checked in their `lang`, else `[spelling] language` (British English by default), with bundled British and American English dictionaries and any Hunspell dictionaries in `[spelling] dictionaries` (#65)."

- [ ] **Step 3: Antora docs (dispatch the asciidoc-antora-writer agent)**

Brief the agent with the spec's "What is checked", "Language", word-list and settings sections, and ask for: in `settings.adoc`, the `[spelling]` block in the example file, three table rows (`spelling.enabled`, `spelling.language`, `spelling.dictionaries`) in the existing alphabetical table, and `spelling.enabled` and `spelling.language` added to "What a workspace file may set" (with `spelling.dictionaries` user-file only); in `using-the-editor.adoc`, a `[#spelling]` "Spelling" section (what is checked and skipped, languages and dictionaries, suggestions and the two dictionaries, `.adocfoundry-words.txt` committed with the repository, the user list beside the settings file, the setting to switch it off); in the developer module, `adoc-spell` in the crate ladder. Then `gradle25w antora` must build with no errors.

- [ ] **Step 4: Project guide, architecture doc and spec notes**

- `CLAUDE.md`: add `adoc-spell` to the L1 row of the ladder and a bullet: "`adoc-spell` — spell checking with no AsciiDoc knowledge: `spellbook` (only ever named here, checked by `cargo xtask layers`), the bundled SCOWL `en_GB`/`en_US` dictionaries and a folder of others (`Dictionaries`), the words of a run of text worth checking (`words`, `OBJECT`), and word lists (`WordList`). The engine's `src/spelling.rs` reads prose from the visual model and checks open pages on a worker."
- Architecture design doc: add `adoc-spell` to its crate list or ladder.
- `DESIGN-Spell_Checking.md`: add an "Implementation notes" section recording where the build departs from the design: the spelling commands have no keymap action, so they need no `NOT_IN_MENUS` entry; `adoc-spell` depends on `spellbook` and `unicode-segmentation` only (no `adoc-core`; `foldhash` comes through `spellbook`); `adoc-ls` uses source `adoc-ls` for its hints, like its other diagnostics, and is tested in `protocol.rs` unit tests rather than `tests/stdio.rs`; a word list also accepts a listed word's possessive `'s`; the visual editor underlines through its text runs' wavy underline rather than painting.

- [ ] **Step 5: Run the full gate**

```bash
cargo fmt --check
cargo clippy -j 6 --workspace --all-targets -- -D warnings
cargo test -j 6 --workspace
cargo deny check
cargo xtask layers
gradle25w antora
```

Expected: all pass. Fix and re-run anything that does not.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "Document spell checking and ship the dictionary licence"
```
