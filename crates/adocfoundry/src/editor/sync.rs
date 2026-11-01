//! Keeping the editor widget's display buffer and the engine rope identical.
//!
//! The widget reports only that its text changed, so the edit is recovered as the single
//! replacement between the common prefix and suffix of the two texts. That is exact for typing,
//! deletion and paste, and still correct (if wider) for anything else. Both are ropes, compared
//! chunk against chunk where they start and end, so nothing is copied: only the replacement
//! itself is.

use std::ops::Range;

use ropey::Rope;

/// The replacement that turns `old` into `new`: the byte range in `old` and the text to put
/// there. `None` when the texts are equal.
pub fn diff(old: &Rope, new: &Rope) -> Option<(Range<usize>, String)> {
    adoc_core::probe::hit(adoc_core::probe::Probe::Diff);
    let mut prefix = common_prefix(old, new);
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let longest_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = common_suffix(old, new, longest_suffix);
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    let replacement = new.slice(prefix..new.len() - suffix).to_string();
    Some((prefix..old.len() - suffix, replacement))
}

/// How many bytes `a` and `b` share at their start.
fn common_prefix(a: &Rope, b: &Rope) -> usize {
    let (mut left, mut right) = (a.chunks(), b.chunks());
    let (mut x, mut y) = (left.next().unwrap_or(""), right.next().unwrap_or(""));
    let mut shared = 0;
    loop {
        if x.is_empty() {
            match left.next() {
                Some(chunk) => x = chunk,
                None => return shared,
            }
            continue;
        }
        if y.is_empty() {
            match right.next() {
                Some(chunk) => y = chunk,
                None => return shared,
            }
            continue;
        }
        let length = x.len().min(y.len());
        let (head_x, head_y) = (&x.as_bytes()[..length], &y.as_bytes()[..length]);
        if head_x != head_y {
            return shared
                + head_x
                    .iter()
                    .zip(head_y)
                    .take_while(|(left, right)| left == right)
                    .count();
        }
        shared += length;
        x = &x[length..];
        y = &y[length..];
    }
}

/// How many bytes, at most `limit`, `a` and `b` share at their end.
fn common_suffix(a: &Rope, b: &Rope, limit: usize) -> usize {
    let (mut left, mut right) = (
        a.chunks_at(a.len()).0.reversed(),
        b.chunks_at(b.len()).0.reversed(),
    );
    let (mut x, mut y) = ("", "");
    let mut shared = 0;
    while shared < limit {
        if x.is_empty() {
            match left.next() {
                Some(chunk) => x = chunk,
                None => break,
            }
            continue;
        }
        if y.is_empty() {
            match right.next() {
                Some(chunk) => y = chunk,
                None => break,
            }
            continue;
        }
        let length = x.len().min(y.len()).min(limit - shared);
        let (tail_x, tail_y) = (
            &x.as_bytes()[x.len() - length..],
            &y.as_bytes()[y.len() - length..],
        );
        if tail_x != tail_y {
            return shared
                + tail_x
                    .iter()
                    .rev()
                    .zip(tail_y.iter().rev())
                    .take_while(|(left, right)| left == right)
                    .count();
        }
        shared += length;
        x = &x[..x.len() - length];
        y = &y[..y.len() - length];
    }
    shared
}

/// Where `offset` is once `range` is replaced by `inserted` bytes. Text before or after the
/// range keeps its place; an offset within it, or at either end, goes to the end of the
/// replacement, as a caret does after typing, undo or redo there.
pub fn map_offset(offset: usize, range: &Range<usize>, inserted: usize) -> usize {
    if offset < range.start {
        offset
    } else if offset > range.end {
        offset - (range.end - range.start) + inserted
    } else {
        range.start + inserted
    }
}

#[cfg(test)]
mod tests {
    use ropey::Rope;

    use super::{diff, map_offset};

    fn of(old: &str, new: &str) -> Option<(std::ops::Range<usize>, String)> {
        diff(&Rope::from_str(old), &Rope::from_str(new))
    }

    fn apply(old: &str, new: &str) -> String {
        let Some((range, text)) = of(old, new) else {
            return old.to_owned();
        };
        format!("{}{}{}", &old[..range.start], text, &old[range.end..])
    }

    #[test]
    fn offsets_outside_a_replacement_keep_their_text() {
        // "First." becoming "First, changed." at 13..13.
        let range = 13..13;
        assert_eq!(map_offset(5, &range, 9), 5);
        assert_eq!(map_offset(20, &range, 9), 29);
        // A caret where the text went in ends up after it.
        assert_eq!(map_offset(13, &range, 9), 22);
        // Within or at the end of a deletion, the caret goes to where it was.
        assert_eq!(map_offset(15, &(10..20), 0), 10);
        assert_eq!(map_offset(20, &(10..20), 0), 10);
        assert_eq!(map_offset(25, &(10..20), 0), 15);
    }

    #[test]
    fn recovers_typing_deletion_and_replacement() {
        assert_eq!(of("abc", "abXc"), Some((2..2, "X".to_owned())));
        assert_eq!(of("abc", "ac"), Some((1..2, String::new())));
        assert_eq!(
            of("hello world", "hello there"),
            Some((6..11, "there".to_owned()))
        );
        assert_eq!(of("same", "same"), None);
        assert_eq!(of("", ""), None);
        assert_eq!(of("", "new"), Some((0..0, "new".to_owned())));
        assert_eq!(of("old", ""), Some((0..3, String::new())));
    }

    #[test]
    fn repeated_characters_resolve_to_an_insertion() {
        assert_eq!(of("aaa", "aaaa"), Some((3..3, "a".to_owned())));
        assert_eq!(of("\n\n", "\n\n\n"), Some((2..2, "\n".to_owned())));
    }

    #[test]
    fn respects_multibyte_characters() {
        for (old, new) in [
            ("héllo", "hèllo"),
            ("日本", "日本語"),
            ("a→b", "a←b"),
            ("é", ""),
        ] {
            let (range, _) = of(old, new).unwrap();
            assert!(old.is_char_boundary(range.start) && old.is_char_boundary(range.end));
            assert_eq!(apply(old, new), new);
        }
    }

    /// Ropes long enough to hold several chunks, split differently on the two sides.
    #[test]
    fn compares_across_chunk_boundaries() {
        let base: String = (0..5_000).map(|n| format!("line {n} é\n")).collect();
        for at in [0, 1, 4_096, base.len() / 2, base.len() - 3, base.len()] {
            let at = (0..=at)
                .rev()
                .find(|at| base.is_char_boundary(*at))
                .unwrap();
            let edited = format!("{}XY{}", &base[..at], &base[at..]);
            // Built in pieces, so its chunks do not line up with the other's.
            let mut built = Rope::new();
            let mut start = 0;
            while start < edited.len() {
                let mut end = (start + 999).min(edited.len());
                while !edited.is_char_boundary(end) {
                    end += 1;
                }
                built.insert(built.len(), &edited[start..end]);
                start = end;
            }
            let (range, text) = diff(&Rope::from_str(&base), &built).unwrap();
            assert_eq!(apply(&base, &edited), edited);
            assert_eq!((range.end - range.start, text.len()), (0, 2), "at {at}");
        }
    }
}
