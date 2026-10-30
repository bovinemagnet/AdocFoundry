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
