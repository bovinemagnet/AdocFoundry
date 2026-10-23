//! Keeping the editor widget's display buffer and the engine rope identical.
//!
//! The widget reports only that its text changed, so the edit is recovered as the single
//! replacement between the common prefix and suffix of the two texts. That is exact for typing,
//! deletion and paste, and still correct (if wider) for anything else.

use std::ops::Range;

/// The replacement that turns `old` into `new`: the byte range in `old` and the text to put
/// there. `None` when the texts are equal.
pub fn diff<'a>(old: &str, new: &'a str) -> Option<(Range<usize>, &'a str)> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let longest_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(longest_suffix)
        .take_while(|(left, right)| left == right)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some((prefix..old.len() - suffix, &new[prefix..new.len() - suffix]))
}

#[cfg(test)]
mod tests {
    use super::diff;

    fn apply(old: &str, new: &str) -> String {
        let Some((range, text)) = diff(old, new) else {
            return old.to_owned();
        };
        format!("{}{}{}", &old[..range.start], text, &old[range.end..])
    }

    #[test]
    fn recovers_typing_deletion_and_replacement() {
        assert_eq!(diff("abc", "abXc"), Some((2..2, "X")));
        assert_eq!(diff("abc", "ac"), Some((1..2, "")));
        assert_eq!(diff("hello world", "hello there"), Some((6..11, "there")));
        assert_eq!(diff("same", "same"), None);
    }

    #[test]
    fn repeated_characters_resolve_to_an_insertion() {
        assert_eq!(diff("aaa", "aaaa"), Some((3..3, "a")));
        assert_eq!(diff("\n\n", "\n\n\n"), Some((2..2, "\n")));
    }

    #[test]
    fn respects_multibyte_characters() {
        for (old, new) in [
            ("héllo", "hèllo"),
            ("日本", "日本語"),
            ("a→b", "a←b"),
            ("é", ""),
        ] {
            let (range, _) = diff(old, new).unwrap();
            assert!(old.is_char_boundary(range.start) && old.is_char_boundary(range.end));
            assert_eq!(apply(old, new), new);
        }
    }
}
