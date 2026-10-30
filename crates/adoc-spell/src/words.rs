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

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(text: &str) -> Vec<&str> {
        words(text).map(|range| &text[range]).collect()
    }

    #[test]
    fn words_are_split_by_unicode_boundaries() {
        assert_eq!(
            checked("The quick brown fox."),
            ["The", "quick", "brown", "fox"]
        );
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
