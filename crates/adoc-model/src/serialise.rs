//! Writing the document from the model.
//!
//! The round-trip rules: an untouched node is written as its source slice, byte for byte; a
//! text block is written from its pieces, each mark with the delimiters it had and each atom
//! as written; what lies between nodes (blank lines, comments, attribute entries, directives)
//! is kept as it is. Steps follow the same rules when they write an edited block, so
//! serialising a model built from a page gives the page back exactly.

use crate::tree::{Access, Model, Node, Piece, ReadOnly, TextBlock};

impl Model {
    /// The document written from the model.
    #[must_use]
    pub fn serialise(&self) -> String {
        let mut out = String::with_capacity(self.source.len());
        let mut at = 0;
        for node in &self.blocks {
            write_node(&self.source, node, &mut at, &mut out);
        }
        out.push_str(&self.source[at.min(self.source.len())..]);
        out
    }
}

impl TextBlock {
    /// The block's source, written from its pieces.
    #[must_use]
    pub fn serialise(&self, source: &str) -> String {
        let mut out = String::with_capacity(self.range.len());
        write_pieces(source, &self.pieces, &mut out);
        out
    }
}

/// Writes the source from `*at` up to the end of `node`.
fn write_node(source: &str, node: &Node, at: &mut usize, out: &mut String) {
    let copy = |out: &mut String, at: &mut usize, to: usize| {
        if to > *at {
            out.push_str(&source[*at..to]);
            *at = to;
        }
    };
    copy(out, at, node.range.start);
    let included = matches!(node.access, Access::ReadOnly(ReadOnly::Included { .. }));
    // Children of an include hold content of another file, not of the page.
    let children: &[Node] = if included { &[] } else { &node.children };
    let mut parts: Vec<Part<'_>> = children.iter().map(Part::Child).collect();
    if let Some(text) = &node.text {
        let position = parts
            .iter()
            .position(
                |part| matches!(part, Part::Child(child) if child.range.start >= text.range.end),
            )
            .unwrap_or(parts.len());
        parts.insert(position, Part::Text(text));
    }
    for part in parts {
        match part {
            Part::Text(text) => {
                copy(out, at, text.range.start);
                write_pieces(source, &text.pieces, out);
                *at = text.range.end;
            }
            Part::Child(child) => write_node(source, child, at, out),
        }
    }
    copy(out, at, node.range.end);
}

enum Part<'a> {
    Text(&'a TextBlock),
    Child(&'a Node),
}

fn write_pieces(source: &str, pieces: &[Piece], out: &mut String) {
    for piece in pieces {
        match piece {
            Piece::Text { range } | Piece::Atom { range, .. } => {
                out.push_str(&source[range.start..range.end]);
            }
            Piece::Mark {
                open,
                close,
                children,
                ..
            } => {
                out.push_str(&source[open.start..open.end]);
                write_pieces(source, children, out);
                out.push_str(&source[close.start..close.end]);
            }
        }
    }
}
