//! Word boundaries in shown text, by Unicode's word segmentation (UAX #29), for moving the caret
//! by word and selecting a word with a double click.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation as _;

/// A segment holding a letter or digit: a word, not spacing or punctuation.
fn is_word(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}

/// The end of the word at or after `offset`, skipping the spacing and punctuation before it, or
/// the end of `text` when no word follows.
#[must_use]
pub fn next_word_end(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .map(|(start, segment)| (start, start + segment.len(), segment))
        .find(|(_, end, segment)| *end > offset && is_word(segment))
        .map_or(text.len(), |(_, end, _)| end)
}

/// The start of the word before `offset`, skipping the spacing and punctuation after it, or the
/// start of `text` when no word precedes.
#[must_use]
pub fn previous_word_start(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .rev()
        .find(|(start, segment)| *start < offset && is_word(segment))
        .map_or(0, |(start, _)| start)
}

/// The segment at `offset`: the word it is in, or the run of spacing or punctuation. At the end
/// of `text`, the last segment.
#[must_use]
pub fn word_at(text: &str, offset: usize) -> Range<usize> {
    let mut last = 0..0;
    for (start, segment) in text.split_word_bound_indices() {
        let range = start..start + segment.len();
        if range.contains(&offset) {
            return range;
        }
        last = range;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundaries_follow_uax29() {
        let text = "Hello, wörld — it's 3.5 apples";
        // Forward: to the end of this word, then of the next, past punctuation and spaces.
        assert_eq!(&text[..next_word_end(text, 0)], "Hello");
        let after_hello = next_word_end(text, 0);
        assert_eq!(
            &text[after_hello..next_word_end(text, after_hello)],
            ", wörld"
        );
        // An apostrophe and a decimal point stay inside their words.
        let its = text.find("it's").unwrap();
        assert_eq!(&text[its..next_word_end(text, its)], "it's");
        let number = text.find("3.5").unwrap();
        assert_eq!(&text[number..next_word_end(text, number)], "3.5");
        assert_eq!(next_word_end(text, text.len()), text.len());
        // Backward: to the start of this word, or of the one before.
        assert_eq!(&text[previous_word_start(text, text.len())..], "apples");
        let dash = text.find('—').unwrap();
        assert_eq!(&text[previous_word_start(text, dash)..dash], "wörld ");
        assert_eq!(previous_word_start(text, 0), 0);
        // A word, or the spacing at the offset.
        let w = text.find('w').unwrap() + 2;
        assert_eq!(&text[word_at(text, w)], "wörld");
        assert_eq!(&text[word_at(text, after_hello + 1)], " ");
        assert_eq!(&text[word_at(text, text.len())], "apples");
        assert_eq!(word_at("", 0), 0..0);
    }
}
