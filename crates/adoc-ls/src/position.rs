use adoc_core::SourceRange;
use lsp_types::{ClientCapabilities, Position, PositionEncodingKind, Range};

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

    pub(crate) fn offset(self, text: &str, position: Position) -> Option<usize> {
        let (start, end) = line_bounds(text, usize::try_from(position.line).ok()?)?;
        let line = &text[start..end];
        let character = usize::try_from(position.character).ok()?;

        match self {
            Self::Utf8 => {
                let offset = start.checked_add(character)?;
                (offset <= end && text.is_char_boundary(offset)).then_some(offset)
            }
            Self::Utf16 => utf16_offset(line, character).map(|offset| start + offset),
        }
    }

    pub(crate) fn position(self, text: &str, offset: usize) -> Option<Position> {
        if offset > text.len() || !text.is_char_boundary(offset) {
            return None;
        }
        let prefix = &text[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
        let line_start = prefix.rfind('\n').map_or(0, |newline| newline + 1);
        let line_prefix = &text[line_start..offset];
        let character = match self {
            Self::Utf8 => line_prefix.len(),
            Self::Utf16 => line_prefix.encode_utf16().count(),
        };
        Some(Position::new(
            u32::try_from(line).ok()?,
            u32::try_from(character).ok()?,
        ))
    }

    pub(crate) fn range(self, text: &str, range: SourceRange) -> Option<Range> {
        Some(Range::new(
            self.position(text, range.start)?,
            self.position(text, range.end)?,
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
    (units == target_units).then_some(line.len())
}

#[cfg(test)]
mod tests {
    use lsp_types::Position;

    use super::PositionEncoding;

    #[test]
    fn converts_utf16_positions_with_non_bmp_characters() {
        let text = "a😀b\nnext";
        let encoding = PositionEncoding::Utf16;

        assert_eq!(encoding.offset(text, Position::new(0, 3)), Some(5));
        assert_eq!(encoding.position(text, 5), Some(Position::new(0, 3)));
        assert_eq!(encoding.offset(text, Position::new(0, 2)), None);
    }

    #[test]
    fn converts_utf8_positions_as_byte_columns() {
        let text = "éx";
        let encoding = PositionEncoding::Utf8;

        assert_eq!(encoding.offset(text, Position::new(0, 2)), Some(2));
        assert_eq!(encoding.position(text, 2), Some(Position::new(0, 2)));
    }
}
