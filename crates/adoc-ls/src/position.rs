use std::borrow::Cow;

use adoc_core::SourceRange;
use lsp_types::{ClientCapabilities, Position, PositionEncodingKind, Range};

/// A text with where its lines start, built once for a request or notification, so that a
/// position and an offset map to each other by a search and the line's own length rather than
/// by counting lines from the start of the text.
pub(crate) struct Lines<'a> {
    text: Cow<'a, str>,
    /// Where each line starts: 0, and after each `\n`.
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self::of(Cow::Borrowed(text))
    }

    /// Lines over a text of their own, to be changed through [`Lines::replace`].
    pub(crate) fn owned(text: String) -> Lines<'static> {
        Lines::of(Cow::Owned(text))
    }

    fn of(text: Cow<'a, str>) -> Self {
        let starts = std::iter::once(0)
            .chain(starts_after_newlines(&text, 0))
            .collect();
        Self { text, starts }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Replaces `range`, which lies on character boundaries, with `replacement`. Lines starting
    /// before the range keep their starts, those starting inside it are gone, those after it
    /// move by the change in length, and the replacement's own newlines start new ones; only
    /// the replacement is scanned.
    pub(crate) fn replace(&mut self, range: SourceRange, replacement: &str) {
        self.text
            .to_mut()
            .replace_range(range.start..range.end, replacement);
        let first = self.starts.partition_point(|start| *start <= range.start);
        let after = self.starts.partition_point(|start| *start <= range.end);
        for start in &mut self.starts[after..] {
            *start = *start - range.end + range.start + replacement.len();
        }
        self.starts.splice(
            first..after,
            starts_after_newlines(replacement, range.start),
        );
    }

    /// The 0-based line of `offset` and its column in bytes; `offset` is within the text.
    fn line_column(&self, offset: usize) -> (usize, usize) {
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        (line, offset - self.starts[line])
    }

    /// Where `line` starts and ends, its end before its `\n` and one `\r` before that; None
    /// past the last line.
    fn bounds(&self, line: usize) -> Option<(usize, usize)> {
        let start = *self.starts.get(line)?;
        let mut end = self
            .starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next - 1);
        if end > start && self.text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        Some((start, end))
    }
}

/// The offsets after each `\n` in `text`, which starts at `base`.
fn starts_after_newlines(text: &str, base: usize) -> impl Iterator<Item = usize> + '_ {
    text.match_indices('\n')
        .map(move |(offset, _)| base + offset + 1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PositionEncoding {
    Utf8,
    Utf16,
}

impl PositionEncoding {
    pub(crate) fn negotiate(capabilities: &ClientCapabilities) -> Self {
        let supports_utf8 = capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
            .is_some_and(|encodings| encodings.contains(&PositionEncodingKind::UTF8));
        if supports_utf8 {
            Self::Utf8
        } else {
            Self::Utf16
        }
    }

    pub(crate) fn lsp_kind(self) -> PositionEncodingKind {
        match self {
            Self::Utf8 => PositionEncodingKind::UTF8,
            Self::Utf16 => PositionEncodingKind::UTF16,
        }
    }

    /// The byte offset of `position` in the text of `lines`. As the specification says, a
    /// character past the end of its line means the line's end, and a line past the last means
    /// the end of the text. None only for a position inside a character (or, in UTF-16, a surrogate pair).
    pub(crate) fn offset(self, lines: &Lines<'_>, position: Position) -> Option<usize> {
        let text = lines.text();
        let Some((start, end)) = lines.bounds(usize::try_from(position.line).ok()?) else {
            return Some(text.len());
        };
        let line = &text[start..end];
        let character = usize::try_from(position.character).ok()?;

        match self {
            Self::Utf8 => {
                let offset = start.saturating_add(character).min(end);
                text.is_char_boundary(offset).then_some(offset)
            }
            Self::Utf16 => utf16_offset(line, character).map(|offset| start + offset),
        }
    }

    pub(crate) fn position(self, lines: &Lines<'_>, offset: usize) -> Option<Position> {
        let text = lines.text();
        if offset > text.len() || !text.is_char_boundary(offset) {
            return None;
        }
        let (line, column) = lines.line_column(offset);
        let line_prefix = &text[offset - column..offset];
        let character = match self {
            Self::Utf8 => line_prefix.len(),
            Self::Utf16 => line_prefix.encode_utf16().count(),
        };
        Some(Position::new(
            u32::try_from(line).ok()?,
            u32::try_from(character).ok()?,
        ))
    }

    pub(crate) fn range(self, lines: &Lines<'_>, range: SourceRange) -> Option<Range> {
        Some(Range::new(
            self.position(lines, range.start)?,
            self.position(lines, range.end)?,
        ))
    }
}

fn utf16_offset(line: &str, target_units: usize) -> Option<usize> {
    let mut units = 0;
    for (offset, character) in line.char_indices() {
        if units == target_units {
            return Some(offset);
        }
        units += character.len_utf16();
        if units > target_units {
            return None;
        }
    }
    // Past the end of the line: its end.
    Some(line.len())
}

#[cfg(test)]
mod tests {
    use adoc_core::SourceRange;
    use lsp_types::Position;

    use super::{Lines, PositionEncoding};

    #[test]
    fn converts_utf16_positions_with_non_bmp_characters() {
        let text = "a😀b\nnext";
        let encoding = PositionEncoding::Utf16;

        assert_eq!(
            encoding.offset(&Lines::new(text), Position::new(0, 3)),
            Some(5)
        );
        assert_eq!(
            encoding.position(&Lines::new(text), 5),
            Some(Position::new(0, 3))
        );
        assert_eq!(
            encoding.offset(&Lines::new(text), Position::new(0, 2)),
            None
        );
    }

    #[test]
    fn clamps_positions_past_a_line_or_the_text_in_both_encodings() {
        let text = "a😀b\nnext";
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            // Past the end of a line: the line's end, before its newline.
            assert_eq!(
                encoding.offset(&Lines::new(text), Position::new(0, 999)),
                Some(6)
            );
            // Past the last line: the end of the text.
            assert_eq!(
                encoding.offset(&Lines::new(text), Position::new(1, 999)),
                Some(text.len())
            );
            assert_eq!(
                encoding.offset(&Lines::new(text), Position::new(2, 0)),
                Some(text.len())
            );
            assert_eq!(
                encoding.offset(&Lines::new(text), Position::new(9, 3)),
                Some(text.len())
            );
        }
        assert_eq!(
            PositionEncoding::Utf16.offset(&Lines::new("ab\r\ncd"), Position::new(0, 5)),
            Some(2),
            "a line's end is before its carriage return"
        );
    }

    #[test]
    fn converts_utf8_positions_as_byte_columns() {
        let text = "éx";
        let encoding = PositionEncoding::Utf8;

        assert_eq!(
            encoding.offset(&Lines::new(text), Position::new(0, 2)),
            Some(2)
        );
        assert_eq!(
            encoding.position(&Lines::new(text), 2),
            Some(Position::new(0, 2))
        );
    }

    /// The offset of `position` in `text`, as a request maps it.
    fn offset(encoding: PositionEncoding, text: &str, position: Position) -> Option<usize> {
        encoding.offset(&Lines::new(text), position)
    }

    #[test]
    fn positions_map_to_the_same_offsets_at_every_edge() {
        /// A text, a line and character, and the offsets in UTF-8 and UTF-16.
        type Case = (&'static str, u32, u32, Option<usize>, Option<usize>);
        const MAX: u32 = u32::MAX;
        let cases: &[Case] = &[
            // Inside and after a surrogate pair, and past the line's end.
            ("a😀b\nnext", 0, 0, Some(0), Some(0)),
            ("a😀b\nnext", 0, 1, Some(1), Some(1)),
            ("a😀b\nnext", 0, 2, None, None),
            ("a😀b\nnext", 0, 3, None, Some(5)),
            ("a😀b\nnext", 0, 4, None, Some(6)),
            ("a😀b\nnext", 0, 5, Some(5), Some(6)),
            ("a😀b\nnext", 0, 6, Some(6), Some(6)),
            ("a😀b\nnext", 0, 7, Some(6), Some(6)),
            ("a😀b\nnext", 1, 2, Some(9), Some(9)),
            ("a😀b\nnext", 1, 4, Some(11), Some(11)),
            ("a😀b\nnext", 1, MAX, Some(11), Some(11)),
            ("a😀b\nnext", 2, 0, Some(11), Some(11)),
            ("a😀b\nnext", MAX, MAX, Some(11), Some(11)),
            // Multi-byte characters, and line ends before a carriage return.
            ("éx\r\nyz", 0, 1, None, Some(2)),
            ("éx\r\nyz", 0, 2, Some(2), Some(3)),
            ("éx\r\nyz", 0, 3, Some(3), Some(3)),
            ("éx\r\nyz", 0, 4, Some(3), Some(3)),
            ("éx\r\nyz", 0, MAX, Some(3), Some(3)),
            ("éx\r\nyz", 1, 0, Some(5), Some(5)),
            ("éx\r\nyz", 1, 3, Some(7), Some(7)),
            ("éx\r\nyz", 2, 0, Some(7), Some(7)),
            ("中文\nz", 0, 1, None, Some(3)),
            ("中文\nz", 0, 3, Some(3), Some(6)),
            ("😀\r\nz", 0, 1, None, None),
            ("😀\r\nz", 0, 2, None, Some(4)),
            ("😀\r\nz", 0, 9, Some(4), Some(4)),
            // `\r\n` lines, and the empty line after the last newline.
            ("ab\r\ncd\r\n", 0, 2, Some(2), Some(2)),
            ("ab\r\ncd\r\n", 0, 3, Some(2), Some(2)),
            ("ab\r\ncd\r\n", 1, 5, Some(6), Some(6)),
            ("ab\r\ncd\r\n", 2, 0, Some(8), Some(8)),
            ("ab\r\ncd\r\n", 2, 5, Some(8), Some(8)),
            ("ab\r\ncd\r\n", 3, 0, Some(8), Some(8)),
            ("\r\nx", 0, 1, Some(0), Some(0)),
            ("\r\nx", 1, 1, Some(3), Some(3)),
            // Only one carriage return is before a line's end; a last line's is too.
            ("a\r\r\nb", 0, 9, Some(2), Some(2)),
            ("a\r\r\nb", 1, 0, Some(4), Some(4)),
            ("ab\r", 0, 2, Some(2), Some(2)),
            ("ab\r", 0, 9, Some(2), Some(2)),
            ("ab\r", 1, 0, Some(3), Some(3)),
            // An empty text.
            ("", 0, 0, Some(0), Some(0)),
            ("", 0, 5, Some(0), Some(0)),
            ("", 1, 0, Some(0), Some(0)),
        ];
        for &(text, line, character, utf8, utf16) in cases {
            let position = Position::new(line, character);
            assert_eq!(
                offset(PositionEncoding::Utf8, text, position),
                utf8,
                "UTF-8 {text:?} {line}:{character}"
            );
            assert_eq!(
                offset(PositionEncoding::Utf16, text, position),
                utf16,
                "UTF-16 {text:?} {line}:{character}"
            );
        }
    }

    #[test]
    fn maps_offsets_on_later_lines_and_refuses_ones_inside_a_character() {
        let text = "😀\nab\n\n😀c";
        let lines = Lines::new(text);
        let encoding = PositionEncoding::Utf16;
        let c = text.find('c').unwrap();
        assert_eq!(encoding.position(&lines, c), Some(Position::new(3, 2)));
        assert_eq!(
            encoding.position(&lines, text.len()),
            Some(Position::new(3, 3))
        );
        assert_eq!(encoding.position(&lines, 1), None, "inside the first 😀");
        assert_eq!(encoding.position(&lines, text.len() + 1), None);
        assert_eq!(
            PositionEncoding::Utf8.position(&lines, c),
            Some(Position::new(3, 4))
        );
    }

    #[test]
    fn replacing_text_keeps_the_lines_a_fresh_scan_finds() {
        let replacements = [
            "",
            "\n",
            "\r\n",
            "\r",
            "x",
            "😀\n\n",
            "é\r",
            "\nab\n",
            "中\r\n\r\n",
        ];
        let mut lines = Lines::owned("a😀b\r\n\nné\rx\n".to_owned());
        // A fixed pseudo-random sequence, so a failure always repeats.
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % u64::try_from(bound).unwrap()).unwrap()
        };
        for step in 0..2000 {
            let text = lines.text();
            let mut start = text.floor_char_boundary(next(text.len() + 1));
            let mut end = text.floor_char_boundary(next(text.len() + 1));
            if start > end {
                std::mem::swap(&mut start, &mut end);
            }
            // Keep the text short, so ranges often meet line ends and carriage returns.
            if text.len() > 60 {
                end = text.len();
            }
            let replacement = replacements[next(replacements.len())];
            lines.replace(SourceRange::new(start, end), replacement);

            let fresh = Lines::new(lines.text());
            assert_eq!(
                lines.starts,
                fresh.starts,
                "step {step}: {:?}",
                lines.text()
            );
            let line_count = u32::try_from(fresh.starts.len()).unwrap();
            for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
                for line in 0..=line_count {
                    for character in 0..8 {
                        let position = Position::new(line, character);
                        assert_eq!(
                            encoding.offset(&lines, position),
                            encoding.offset(&fresh, position),
                            "step {step}: {:?} {line}:{character}",
                            lines.text()
                        );
                    }
                }
            }
        }
    }
}
