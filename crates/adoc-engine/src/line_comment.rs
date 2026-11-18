//! Toggle Line Comment (#722): comments out the lines a selection touches with `//`, or
//! uncomments them.
//!
//! Asciidoctor reads a line as a comment when it begins with `//` and no third `/` follows
//! (`////` opens a comment block, and `///x` is text), only at the start of the line. So the
//! command puts `// ` at the start of each line, never after its indentation, and takes out
//! only what Asciidoctor reads as a comment. When every non-blank line touched is a comment,
//! each loses its `//` and one space after it; otherwise every non-blank line is commented,
//! comments too, so that toggling twice gives the text back. Blank lines are left alone.

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, TextEdit,
};
use adoc_core::SourceRange;

const PREFIX: &str = "// ";

pub(crate) fn register_line_comment(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::EDIT_TOGGLE_LINE_COMMENT,
            "Toggle Line Comment",
            CommandCategory::Edit,
            Some("cmd-/"),
            CommandContext::ActiveDocument,
        ),
        toggle_line_comment_command,
    );
}

/// Toggles the lines of the range (or of the offset's line) as one undo step, and gives the
/// selection over the same text after it. Nothing is added to the undo history when no line
/// changes.
fn toggle_line_comment_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let (id, range) = match args {
        CommandArgs::Range { id, range } => (id, range),
        CommandArgs::Offset { id, offset } => (id, SourceRange::new(offset, offset)),
        _ => {
            return Err(EngineError::InvalidArguments {
                command: ids::EDIT_TOGGLE_LINE_COMMENT,
                expected: "a document and a range",
            })
        }
    };
    let text = engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?
        .text_string();
    let (edits, selection) = toggle_line_comments(&text, range);
    if !edits.is_empty() {
        let group = engine.new_group();
        engine.apply_edits(id, edits, UndoMode::Group(group))?;
    }
    Ok(CommandOutcome::Selection(selection))
}

/// The edits that toggle the lines `range` touches, and `range` moved over them. A range
/// ending at the start of a line, past others, leaves that line out, as a selection of whole
/// lines does.
pub(crate) fn toggle_line_comments(text: &str, range: SourceRange) -> (Vec<TextEdit>, SourceRange) {
    let (low, high) = (
        range.start.min(range.end).min(text.len()),
        range.start.max(range.end).min(text.len()),
    );
    let first = line_start(text, low);
    let mut last = line_start(text, high);
    if high > low && last == high && last > first {
        last = line_start(text, high - 1);
    }
    let lines: Vec<(usize, &str)> = lines_from(text, first)
        .take_while(|(start, _)| *start <= last)
        .filter(|(_, body)| !body.trim().is_empty())
        .collect();
    let uncomment = !lines.is_empty() && lines.iter().all(|(_, body)| is_comment(body));
    let edits: Vec<TextEdit> = lines
        .iter()
        .map(|&(start, body)| {
            if uncomment {
                let length = if body[2..].starts_with(' ') { 3 } else { 2 };
                TextEdit::new(SourceRange::new(start, start + length), String::new())
            } else {
                TextEdit::new(SourceRange::new(start, start), PREFIX.to_owned())
            }
        })
        .collect();
    let moved = |offset: usize| moved_by(&edits, offset);
    let selection = SourceRange::new(moved(range.start), moved(range.end));
    (edits, selection)
}

/// Whether Asciidoctor reads the line as a comment line.
fn is_comment(line: &str) -> bool {
    line.starts_with("//") && !line[2..].starts_with('/')
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |at| at + 1)
}

/// Each line from `start` on, as its start and its text without the ending.
fn lines_from(text: &str, start: usize) -> impl Iterator<Item = (usize, &str)> {
    let mut at = start;
    std::iter::from_fn(move || {
        if at > text.len() || (at == text.len() && at > start && text.ends_with('\n')) {
            return None;
        }
        let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
        let body = text[at..end].strip_suffix('\r').unwrap_or(&text[at..end]);
        let line = (at, body);
        at = end + 1;
        Some(line)
    })
}

/// Where `offset` is once `edits` (in order, none overlapping) are made: after an insertion at
/// or before it, and at the start of a removal it falls inside.
fn moved_by(edits: &[TextEdit], offset: usize) -> usize {
    let mut moved = offset;
    for edit in edits {
        let (start, end) = (edit.range.start, edit.range.end);
        if offset < start {
            break;
        }
        if offset < end {
            moved -= offset - start;
        } else {
            moved = moved + edit.replacement.len() - (end - start);
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edits: &[TextEdit]) -> String {
        let mut out = text.to_owned();
        for edit in edits.iter().rev() {
            out.replace_range(edit.range.start..edit.range.end, &edit.replacement);
        }
        out
    }

    #[test]
    fn half_typed_and_odd_input_never_panics() {
        for text in ["", "\n", "//", "/", "///", "\r\n", "x", "// é\n", "é"] {
            for start in 0..=text.len() {
                for end in 0..=text.len() {
                    if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                        continue;
                    }
                    let (edits, _) = toggle_line_comments(text, SourceRange::new(start, end));
                    let _ = apply(text, &edits);
                }
            }
        }
    }

    #[test]
    fn a_comment_marker_alone_is_uncommented_to_nothing() {
        let (edits, selection) = toggle_line_comments("//\n", SourceRange::new(0, 0));
        assert_eq!(apply("//\n", &edits), "\n");
        assert_eq!(selection, SourceRange::new(0, 0));
    }

    #[test]
    fn the_last_line_without_an_ending_is_reached() {
        let (edits, _) = toggle_line_comments("a\nb", SourceRange::new(3, 3));
        assert_eq!(apply("a\nb", &edits), "a\n// b");
    }

    #[test]
    fn an_offset_past_the_end_is_the_last_line() {
        let (edits, _) = toggle_line_comments("a\n", SourceRange::new(9, 9));
        assert!(edits.is_empty(), "the empty line after the ending is blank");
    }
}
