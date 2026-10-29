//! Format Table: pads the cells of the table at the caret so its `|` separators line up.
//!
//! Only tables written one row to a line, with every cell starting at a bare `|`, are
//! formatted. Asciidoctor strips the spaces around a cell's text, so padding never changes what
//! the table renders; anything whose meaning could depend on the layout of its source (cells
//! spread over lines, spans, styles, another separator or format, styled columns) is left
//! alone, and the refusal says why.

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, TextEdit,
};
use adoc_core::SourceRange;

pub(crate) fn register_format_table(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::EDIT_FORMAT_TABLE,
            "Format Table",
            CommandCategory::Edit,
            None,
            CommandContext::ActiveDocument,
        ),
        format_table_command,
    );
}

/// Formats the table at the offset (or the start of the range) as one undo step. A table that
/// is already formatted is left as it is, with nothing added to the undo history.
fn format_table_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let (id, offset) = match args {
        CommandArgs::Offset { id, offset } => (id, offset),
        CommandArgs::Range { id, range } => (id, range.start.min(range.end)),
        _ => {
            return Err(EngineError::InvalidArguments {
                command: ids::EDIT_FORMAT_TABLE,
                expected: "a document and an offset",
            })
        }
    };
    let text = engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?
        .text_string();
    if let Some(edit) = format_table(&text, offset)? {
        let group = engine.new_group();
        engine.apply_edits(id, vec![edit], UndoMode::Group(group))?;
    }
    Ok(CommandOutcome::Done)
}

/// The edit that aligns the table around `offset`, or `None` when it is aligned already.
pub(crate) fn format_table(text: &str, offset: usize) -> Result<Option<TextEdit>, EngineError> {
    let lines = lines(text);
    let caret = lines
        .iter()
        .rposition(|line| line.start <= offset)
        .unwrap_or(0);
    let (open, close) = table_at(&lines, caret)?
        .ok_or_else(|| EngineError::NotFound("there is no table at the caret".to_owned()))?;
    check_attributes(&lines[..open])?;

    let mut rows: Vec<Option<Vec<&str>>> = Vec::new();
    for line in &lines[open + 1..close] {
        if line.body.trim_matches(is_blank).is_empty() {
            rows.push(None);
        } else {
            rows.push(Some(cells(line.body)?));
        }
    }
    let Some(columns) = rows.iter().flatten().map(Vec::len).next() else {
        return Ok(None);
    };
    if rows.iter().flatten().any(|row| row.len() != columns) {
        return Err(refused("its rows have different numbers of cells"));
    }
    let mut widths = vec![0; columns];
    for row in rows.iter().flatten() {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }

    let mut formatted = String::new();
    for (row, line) in rows.iter().zip(&lines[open + 1..close]) {
        if let Some(row) = row {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| {
                    let padding = width - cell.chars().count();
                    format!("| {cell}{}", " ".repeat(padding))
                })
                .collect();
            formatted.push_str(cells.join(" ").trim_end());
        }
        formatted.push_str(line.ending);
    }
    let range = SourceRange::new(lines[open + 1].start, lines[close].start);
    if text[range.start..range.end] == formatted {
        return Ok(None);
    }
    Ok(Some(TextEdit::new(range, formatted)))
}

struct Line<'a> {
    start: usize,
    /// The line without its ending.
    body: &'a str,
    /// `\n`, `\r\n`, or nothing on a last line without one.
    ending: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for piece in text.split_inclusive('\n') {
        let body = piece
            .strip_suffix('\n')
            .map_or(piece, |body| body.strip_suffix('\r').unwrap_or(body));
        lines.push(Line {
            start,
            body,
            ending: &piece[body.len()..],
        });
        start += piece.len();
    }
    if text.is_empty() || text.ends_with('\n') {
        lines.push(Line {
            start,
            body: "",
            ending: "",
        });
    }
    lines
}

/// The opening and closing delimiter lines of the `|===` table holding line `caret`, skipping
/// tables shown as text inside listing, literal, passthrough and comment blocks.
fn table_at(lines: &[Line<'_>], caret: usize) -> Result<Option<(usize, usize)>, EngineError> {
    let mut verbatim: Option<&str> = None;
    let mut index = 0;
    while index < lines.len() && index <= caret {
        let body = lines[index].body.trim_end_matches(is_blank);
        if let Some(delimiter) = verbatim {
            if body == delimiter {
                verbatim = None;
            }
        } else if is_table_delimiter(body) {
            let Some(close) = (index + 1..lines.len())
                .find(|&line| lines[line].body.trim_end_matches(is_blank) == body)
            else {
                return Err(refused("it has no closing `|===`"));
            };
            if caret <= close {
                return Ok(Some((index, close)));
            }
            index = close;
        } else if is_verbatim_delimiter(body) {
            verbatim = Some(body);
        }
        index += 1;
    }
    Ok(None)
}

fn is_table_delimiter(line: &str) -> bool {
    line.strip_prefix("|===")
        .is_some_and(|rest| rest.chars().all(|c| c == '='))
}

fn is_verbatim_delimiter(line: &str) -> bool {
    line.len() >= 4
        && ['-', '.', '+', '/']
            .iter()
            .any(|&c| line.chars().all(|other| other == c))
}

/// Refuses a table whose attribute lines change how its cells are read: another separator or
/// format, or columns with styles (a literal column keeps its spaces).
fn check_attributes(before: &[Line<'_>]) -> Result<(), EngineError> {
    for line in before.iter().rev() {
        let body = line.body.trim_matches(is_blank);
        if body.starts_with('[') {
            if body.contains("separator") || body.contains("format=") {
                return Err(refused("it uses another separator or format"));
            }
            if column_spec(body).is_some_and(|cols| cols.chars().any(|c| c.is_ascii_alphabetic())) {
                return Err(refused("its columns have styles"));
            }
        } else if !(body.starts_with('.') && !body.starts_with("..")) {
            break;
        }
    }
    Ok(())
}

/// The value of a `cols` attribute in an attribute line.
fn column_spec(line: &str) -> Option<&str> {
    let value = &line[line.find("cols=")? + "cols=".len()..];
    if let Some(quoted) = value.strip_prefix('"') {
        return quoted.split('"').next();
    }
    if let Some(quoted) = value.strip_prefix('\'') {
        return quoted.split('\'').next();
    }
    value.split([',', ']']).next()
}

/// The trimmed cells of a row written on one line, each starting at a bare `|`.
fn cells(line: &str) -> Result<Vec<&str>, EngineError> {
    let Some(rest) = line.strip_prefix('|') else {
        return Err(refused(
            "a row runs over several lines, or a cell has a span or style; only tables with one row to a line are formatted",
        ));
    };
    let mut cells = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (index, c) in rest.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '|' if !escaped => {
                let cell = &rest[start..index];
                if ends_with_cell_spec(cell) {
                    return Err(refused("a cell has a span, style or alignment"));
                }
                cells.push(cell.trim_matches(is_blank));
                start = index + 1;
            }
            _ => escaped = false,
        }
    }
    cells.push(rest[start..].trim_matches(is_blank));
    Ok(cells)
}

/// Whether the text before a `|` ends with a cell specifier for the next cell, as Asciidoctor
/// reads one: after a space, a span (`2+`, `.3+`, `2*`), an alignment (`^`, `.>`, `<.^`) and
/// a style letter, in that order, each optional.
fn ends_with_cell_spec(text: &str) -> bool {
    let Some(space) = text.rfind(is_blank) else {
        return false;
    };
    let token = &text.as_bytes()[space + 1..];
    let length = token.len();
    let digits = |mut index: usize| {
        while index < length && token[index].is_ascii_digit() {
            index += 1;
        }
        index
    };
    let align = |index: usize| index < length && matches!(token[index], b'<' | b'^' | b'>');

    let mut index = digits(0);
    if index < length && token[index] == b'.' {
        index = digits(index + 1);
    }
    if index > 0 && index < length && matches!(token[index], b'+' | b'*') {
        index += 1;
    } else {
        index = 0;
    }
    if align(index) {
        index += 1;
        if index < length && token[index] == b'.' {
            index += 1;
            if align(index) {
                index += 1;
            }
        }
    } else if index < length && token[index] == b'.' && align(index + 1) {
        index += 2;
    }
    if index < length && token[index].is_ascii_lowercase() {
        index += 1;
    }
    length > 0 && index == length
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn refused(reason: &str) -> EngineError {
    EngineError::Refused(format!("This table is left as it is: {reason}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted(text: &str) -> String {
        let offset = text.find("|===").unwrap() + 5;
        match format_table(text, offset).unwrap() {
            Some(edit) => {
                let mut text = text.to_owned();
                text.replace_range(edit.range.start..edit.range.end, &edit.replacement);
                text
            }
            None => text.to_owned(),
        }
    }

    fn reason(text: &str) -> String {
        let offset = text.find("|===").unwrap() + 5;
        format_table(text, offset).unwrap_err().to_string()
    }

    #[test]
    fn a_ragged_table_is_aligned() {
        let text = "\
[cols=\"1,2\"]
|===
|Name|Value

|alpha |1
|  bc| a longer value
|===
";
        assert_eq!(
            formatted(text),
            "\
[cols=\"1,2\"]
|===
| Name  | Value

| alpha | 1
| bc    | a longer value
|===
"
        );
    }

    #[test]
    fn formatting_twice_changes_nothing() {
        let text = "|===\n|a|bb|c\n|ddd||e\n|===\n";
        let once = formatted(text);
        assert_eq!(once, "|===\n| a   | bb | c\n| ddd |    | e\n|===\n");
        let offset = once.find("|===").unwrap() + 5;
        assert!(format_table(&once, offset).unwrap().is_none());
    }

    #[test]
    fn escaped_pipes_and_trailing_empty_cells_are_kept() {
        assert_eq!(
            formatted("|===\n|a \\| bc|c|\n|d|e|\n|===\n"),
            "|===\n| a \\| bc | c |\n| d       | e |\n|===\n"
        );
    }

    #[test]
    fn line_endings_are_kept() {
        assert_eq!(
            formatted("|===\r\n|a|bb\r\n|ccc|d\r\n|===\r\n"),
            "|===\r\n| a   | bb\r\n| ccc | d\r\n|===\r\n"
        );
    }

    #[test]
    fn the_table_at_the_caret_is_the_one_formatted() {
        let text = "|===\n|a|b\n|===\n\n----\n|===\n|x|y\n----\n\n|===\n|cc|d\n|e|ff\n|===\n";
        let offset = text.rfind("|cc").unwrap();
        let edit = format_table(text, offset).unwrap().unwrap();
        assert!(edit.range.start > text.find("----").unwrap());
        assert_eq!(edit.replacement, "| cc | d\n| e  | ff\n");
        // A table shown inside a listing block is text, not a table.
        let listing = text.find("|x|y").unwrap();
        assert!(format_table(text, listing).is_err());
    }

    #[test]
    fn other_tables_are_refused_with_the_reason() {
        assert!(reason("|===\n|a\n|b|c\n|===\n").contains("different numbers of cells"));
        assert!(reason("|===\n|a|b\ncontinued\n|===\n").contains("one row to a line"));
        assert!(reason("|===\n|a 2+|b\n|c|d\n|===\n").contains("span, style or alignment"));
        assert!(reason("|===\n|a a|b\n|c|d\n|===\n").contains("span, style or alignment"));
        assert!(reason("|===\n|a ^.>|b\n|c|d\n|===\n").contains("span, style or alignment"));
        assert!(reason("|===\n2+|a\n|b|c\n|===\n").contains("one row to a line"));
        assert!(reason("[separator=!]\n|===\n!a!b\n|===\n").contains("separator"));
        assert!(reason("[%header,format=csv]\n|===\na,b\n|===\n").contains("format"));
        assert!(reason("[cols=\"1l,1\"]\n|===\n|a|b\n|===\n").contains("styles"));
        assert!(reason("|===\n|a|b\n").contains("closing"));
    }

    #[test]
    fn a_word_before_a_separator_is_not_a_spec() {
        assert!(!ends_with_cell_spec("alpha"));
        assert!(!ends_with_cell_spec("x y2"));
        assert!(!ends_with_cell_spec("x "));
        assert!(ends_with_cell_spec("x 3+"));
        assert!(ends_with_cell_spec("x .2+^s"));
        assert!(ends_with_cell_spec("x >"));
    }

    #[test]
    fn no_table_at_the_caret() {
        let error = format_table("= Title\n\nText.\n", 3).unwrap_err();
        assert!(matches!(error, EngineError::NotFound(_)), "{error}");
    }
}
