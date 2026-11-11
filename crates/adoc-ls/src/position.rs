use adoc_core::{LineIndex, SourceRange};
use lsp_types::{ClientCapabilities, Position, PositionEncodingKind, Range};

/// A text with where its lines start, built once for a request or notification, so that each
/// offset maps to a position by a search rather than by counting lines from the start.
pub(crate) struct Lines<'a> {
    text: &'a str,
    index: LineIndex,
}

impl<'a> Lines<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            text,
            index: LineIndex::new(text),
        }
    }
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

    /// The byte offset of `position` in `text`. As the specification says, a character past
    /// the end of its line means the line's end, and a line past the last means the end of the
    /// text. None only for a position inside a character (or, in UTF-16, a surrogate pair).
    pub(crate) fn offset(self, text: &str, position: Position) -> Option<usize> {
        let Some((start, end)) = line_bounds(text, usize::try_from(position.line).ok()?) else {
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
        let text = lines.text;
        if offset > text.len() || !text.is_char_boundary(offset) {
            return None;
        }
        let (line, column) = lines.index.line_column(offset);
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

fn line_bounds(text: &str, target_line: usize) -> Option<(usize, usize)> {
    let mut start = 0;
    for _ in 0..target_line {
        start += text[start..].find('\n')? + 1;
    }
    let mut end = text[start..]
        .find('\n')
        .map_or(text.len(), |newline| start + newline);
    if end > start && text.as_bytes()[end - 1] == b'\r' {
        end -= 1;
    }
    Some((start, end))
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
    use lsp_types::Position;

    use super::{Lines, PositionEncoding};

    #[test]
    fn converts_utf16_positions_with_non_bmp_characters() {
        let text = "a😀b\nnext";
        let encoding = PositionEncoding::Utf16;

        assert_eq!(encoding.offset(text, Position::new(0, 3)), Some(5));
        assert_eq!(
            encoding.position(&Lines::new(text), 5),
            Some(Position::new(0, 3))
        );
        assert_eq!(encoding.offset(text, Position::new(0, 2)), None);
    }

    #[test]
    fn clamps_positions_past_a_line_or_the_text_in_both_encodings() {
        let text = "a😀b\nnext";
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            // Past the end of a line: the line's end, before its newline.
            assert_eq!(encoding.offset(text, Position::new(0, 999)), Some(6));
            // Past the last line: the end of the text.
            assert_eq!(
                encoding.offset(text, Position::new(1, 999)),
                Some(text.len())
            );
            assert_eq!(encoding.offset(text, Position::new(2, 0)), Some(text.len()));
            assert_eq!(encoding.offset(text, Position::new(9, 3)), Some(text.len()));
        }
        assert_eq!(
            PositionEncoding::Utf16.offset("ab\r\ncd", Position::new(0, 5)),
            Some(2),
            "a line's end is before its carriage return"
        );
    }

    #[test]
    fn converts_utf8_positions_as_byte_columns() {
        let text = "éx";
        let encoding = PositionEncoding::Utf8;

        assert_eq!(encoding.offset(text, Position::new(0, 2)), Some(2));
        assert_eq!(
            encoding.position(&Lines::new(text), 2),
            Some(Position::new(0, 2))
        );
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
}
