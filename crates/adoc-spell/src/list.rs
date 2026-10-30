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
        assert!(
            WordList::read(&path).unwrap().is_empty(),
            "a missing file is empty"
        );
        let mut written = list.clone();
        assert!(written.insert("Qwzxy"));
        assert!(!written.insert("Qwzxy"));
        written.write(&path).unwrap();
        assert_eq!(WordList::read(&path).unwrap(), written);
    }

    #[test]
    fn a_listed_word_accepts_its_capitals_and_possessive() {
        let list = WordList::parse("antora\nAsciidoctor\n");
        for word in [
            "antora",
            "Antora",
            "ANTORA",
            "Antora's",
            "Antora’s",
            "Asciidoctor",
            "ASCIIDOCTOR",
        ] {
            assert!(list.contains(word), "{word}");
        }
        for word in ["AnTora", "asciidoctor", "antoras"] {
            assert!(!list.contains(word), "{word}");
        }
    }
}
