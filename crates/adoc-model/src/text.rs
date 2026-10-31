//! Positions: a text block's display text and its source, mapped both ways.

use std::{fmt, ops::Range, sync::OnceLock};

use adoc_core::SourceRange;

use crate::tree::{BlockPath, Mark, MarkSet, Model, Node, Piece, TextBlock};

/// A stretch of a text block shown with one set of marks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    /// Bytes of the display text.
    pub display: Range<usize>,
    /// Bytes of the page.
    pub source: SourceRange,
    pub marks: MarkSet,
    /// An atom: shown as a unit, its display and source lengths unrelated.
    pub atom: bool,
}

impl TextBlock {
    /// The block's segments in display order.
    #[must_use]
    pub fn segments(&self) -> Vec<Segment> {
        fn visit(pieces: &[Piece], marks: MarkSet, at: &mut usize, out: &mut Vec<Segment>) {
            for piece in pieces {
                match piece {
                    Piece::Text { range } => {
                        out.push(Segment {
                            display: *at..*at + range.len(),
                            source: *range,
                            marks,
                            atom: false,
                        });
                        *at += range.len();
                    }
                    Piece::Atom { range, shown, .. } => {
                        out.push(Segment {
                            display: *at..*at + shown.len(),
                            source: *range,
                            marks,
                            atom: true,
                        });
                        *at += shown.len();
                    }
                    Piece::Mark { mark, children, .. } => {
                        visit(children, marks.with(*mark), at, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        visit(&self.pieces, MarkSet::EMPTY, &mut 0, &mut out);
        out
    }

    /// Where source offset `offset` shows in the display text. Offsets inside delimiters show
    /// at the boundary they sit on; offsets inside an atom at its start.
    #[must_use]
    pub fn display_offset(&self, offset: usize) -> Option<usize> {
        self.display_offset_towards(offset, false)
    }

    /// As [`Self::display_offset`], but an offset inside an atom shows at its end when
    /// `towards_end`: for the end of a range, so that the range takes the atom whole.
    #[must_use]
    pub fn display_offset_towards(&self, offset: usize, towards_end: bool) -> Option<usize> {
        if offset < self.range.start || offset > self.range.end {
            return None;
        }
        let segments = self.segments();
        let mut shown = 0;
        for segment in &segments {
            if offset < segment.source.start {
                return Some(shown);
            }
            if offset <= segment.source.end {
                return Some(if segment.atom {
                    if offset == segment.source.end
                        || (towards_end && offset > segment.source.start)
                    {
                        segment.display.end
                    } else {
                        segment.display.start
                    }
                } else {
                    segment.display.start + (offset - segment.source.start)
                });
            }
            shown = segment.display.end;
        }
        Some(shown)
    }

    /// The source offsets that show at display offset `display`, each with the marks text typed
    /// there would carry; the first is the one with the fewest marks.
    #[must_use]
    pub fn caret_stops(&self, display: usize) -> Vec<(usize, MarkSet)> {
        let segments = self.segments();
        if segments.is_empty() {
            return vec![(self.range.start, MarkSet::EMPTY)];
        }
        let mut stops: Vec<(usize, MarkSet)> = Vec::new();
        for segment in &segments {
            let position = if segment.atom {
                if display == segment.display.start {
                    Some(segment.source.start)
                } else if display == segment.display.end {
                    Some(segment.source.end)
                } else {
                    None
                }
            } else if segment.display.start <= display && display <= segment.display.end {
                Some(segment.source.start + (display - segment.display.start))
            } else {
                None
            };
            if let Some(position) = position {
                if !stops.iter().any(|(existing, _)| *existing == position) {
                    stops.push((position, segment.marks));
                }
            }
        }
        // A position at a block edge outside every mark is a stop too.
        let total = segments.last().map_or(0, |segment| segment.display.end);
        if display == 0 && !stops.iter().any(|(at, _)| *at == self.range.start) {
            stops.push((self.range.start, MarkSet::EMPTY));
        }
        if display == total && !stops.iter().any(|(at, _)| *at == self.range.end) {
            stops.push((self.range.end, MarkSet::EMPTY));
        }
        stops.sort_by_key(|(_, marks)| marks.iter().count());
        stops
    }

    /// The source offset for display offset `display` whose marks are `marks`, or the closest:
    /// the stop inside the text before the caret, else the first.
    #[must_use]
    pub fn source_offset(&self, display: usize, marks: MarkSet) -> Option<usize> {
        let stops = self.caret_stops(display);
        if let Some((offset, _)) = stops.iter().find(|(_, stop)| *stop == marks) {
            return Some(*offset);
        }
        let before = self
            .segments()
            .into_iter()
            .filter(|segment| segment.display.end == display && !segment.atom)
            .map(|segment| segment.source.end)
            .next();
        before.or_else(|| stops.first().map(|(offset, _)| *offset))
    }

    /// The marks of the text just before display offset `display` (or just after it at the
    /// start): what typing there carries.
    #[must_use]
    pub fn marks_at(&self, display: usize) -> MarkSet {
        let segments = self.segments();
        segments
            .iter()
            .rev()
            .find(|segment| segment.display.start < display && display <= segment.display.end)
            .or_else(|| {
                segments
                    .iter()
                    .find(|segment| segment.display.start == display)
            })
            .map_or(MarkSet::EMPTY, |segment| segment.marks)
    }

    /// The display length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.display.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
    }

    /// Whether every byte of the display range has `mark`.
    #[must_use]
    pub fn has_mark(&self, display: Range<usize>, mark: Mark) -> bool {
        let segments = self.segments();
        let covering: Vec<&Segment> = segments
            .iter()
            .filter(|segment| {
                segment.display.start < display.end && display.start < segment.display.end
            })
            .collect();
        !covering.is_empty() && covering.iter().all(|segment| segment.marks.contains(mark))
    }
}

/// A model's text blocks ordered by where they start, built the first time one is looked up.
/// A clone starts without one, since a model is cloned to be changed.
#[derive(Default)]
pub(crate) struct TextIndex(OnceLock<Vec<Indexed>>);

struct Indexed {
    range: SourceRange,
    /// The furthest end of this block and every block before it here.
    reach: usize,
    /// Its place in document order.
    order: usize,
    path: BlockPath,
}

impl Clone for TextIndex {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for TextIndex {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl fmt::Debug for TextIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TextIndex")
    }
}

impl Model {
    fn text_index(&self) -> &[Indexed] {
        self.text_index.0.get_or_init(|| {
            let mut index: Vec<Indexed> = self
                .text_blocks()
                .enumerate()
                .map(|(order, (path, _, text))| Indexed {
                    range: text.range,
                    reach: 0,
                    order,
                    path,
                })
                .collect();
            index.sort_by_key(|entry| entry.range.start);
            let mut reach = 0;
            for entry in &mut index {
                reach = reach.max(entry.range.end);
                entry.reach = reach;
            }
            index
        })
    }

    /// The innermost node whose text block contains source offset `offset`, with its path.
    #[must_use]
    pub fn text_block_at(&self, offset: usize) -> Option<(BlockPath, &Node, &TextBlock)> {
        let index = self.text_index();
        let after = index.partition_point(|entry| entry.range.start <= offset);
        // Back from the last block starting at or before `offset`, while one could reach it;
        // the last in document order wins, as the innermost.
        let found = index[..after]
            .iter()
            .rev()
            .take_while(|entry| entry.reach >= offset)
            .filter(|entry| entry.range.end >= offset)
            .max_by_key(|entry| entry.order)?;
        let node = self.node(&found.path)?;
        Some((found.path.clone(), node, node.text.as_ref()?))
    }

    /// The text block and display offset source offset `offset` shows at.
    #[must_use]
    pub fn display_offset(&self, offset: usize) -> Option<(BlockPath, usize)> {
        let (path, _, text) = self.text_block_at(offset)?;
        Some((path, text.display_offset(offset)?))
    }

    /// The source offset of display offset `display` in the text block of the node at `path`,
    /// carrying `marks` where there is a choice.
    #[must_use]
    pub fn source_offset(&self, path: &[usize], display: usize, marks: MarkSet) -> Option<usize> {
        self.node(path)?
            .text
            .as_ref()?
            .source_offset(display, marks)
    }

    /// Every source offset that shows at the same display position as `offset`, with the marks
    /// typed text there would carry.
    #[must_use]
    pub fn caret_stops(&self, offset: usize) -> Vec<(usize, MarkSet)> {
        self.text_block_at(offset)
            .and_then(|(_, _, text)| Some(text.caret_stops(text.display_offset(offset)?)))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{build::Builder, tree::TextContext};

    fn block(source: &str) -> TextBlock {
        let attributes = BTreeMap::new();
        let builder = Builder {
            page: std::path::Path::new("page.adoc"),
            source,
            attributes: &attributes,
            footnotes: std::cell::Cell::new(0),
        };
        builder.text_block(SourceRange::new(0, source.len()), TextContext::Flow)
    }

    #[test]
    fn maps_display_and_source_both_ways() {
        let text = block("a *bold* (C) end");
        assert_eq!(text.display, "a bold \u{a9} end");
        assert_eq!(
            text.display_offset(3),
            Some(2),
            "inside the open delimiter's end"
        );
        assert_eq!(text.display_offset(4), Some(3));
        assert_eq!(
            text.display_offset(9),
            Some(7),
            "at the start of the replacement"
        );
        assert_eq!(text.display_offset(12), Some(9), "after the replacement");
        assert_eq!(
            text.source_offset(3, MarkSet::EMPTY.with(Mark::Strong)),
            Some(4)
        );
        assert_eq!(text.source_offset(2, MarkSet::EMPTY), Some(2));
        assert_eq!(
            text.source_offset(2, MarkSet::EMPTY.with(Mark::Strong)),
            Some(3)
        );
        assert_eq!(
            text.source_offset(6, MarkSet::EMPTY),
            Some(8),
            "after the close"
        );
        assert_eq!(
            text.source_offset(6, MarkSet::EMPTY.with(Mark::Strong)),
            Some(7)
        );
        assert_eq!(text.marks_at(6), MarkSet::EMPTY.with(Mark::Strong));
        assert_eq!(text.marks_at(7), MarkSet::EMPTY);
        assert!(text.has_mark(2..6, Mark::Strong));
        assert!(!text.has_mark(1..6, Mark::Strong));
    }
}
