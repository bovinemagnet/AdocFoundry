//! The cells of `|===` tables, followed as Asciidoctor's table parser reads them, to tell which
//! text stands in a literal cell (`l|`, or a cell of a column `cols` marks `l`): Asciidoctor
//! shows such a cell as it is written, so what looks like a reference or an anchor there is
//! text (#532). Only the bookkeeping that decides a cell's style is kept: its place in the row
//! (Asciidoctor's `close_cell`, `close_row` and active rowspans) and the header row, which
//! is never literal.

use std::collections::VecDeque;

use crate::line_parser::is_line_comment;

/// The most columns or rows a span or repeat factor is taken for: more is no real table, and
/// a factor typed by mistake (`999999999*`) must cost nothing.
const MAX_FACTOR: usize = 1_000;

/// Follows the `|===` tables of a page line by line.
#[derive(Debug, Default)]
pub(crate) struct Tables {
    open: Option<Table>,
}

impl Tables {
    /// Reads the next line, `content` being the line without its ending: the byte ranges of
    /// it that stand in a literal cell. `opens` says whether the line may open a table, read as
    /// text rather than inside a verbatim block; `attributes` are the block attribute lists
    /// before it and `following` the text after it.
    pub(crate) fn line(
        &mut self,
        content: &str,
        opens: bool,
        attributes: &[String],
        following: &str,
    ) -> Vec<(usize, usize)> {
        let trimmed = content.trim();
        match &mut self.open {
            Some(table) if trimmed == table.delimiter => {
                self.open = None;
                Vec::new()
            }
            // Asciidoctor drops a table's line comments before reading its cells.
            Some(table) if table.follows && !is_line_comment(trimmed) => {
                table.read(content.trim_end())
            }
            Some(_) => Vec::new(),
            None => {
                if opens && is_table_delimiter(trimmed) {
                    self.open = Some(Table::new(trimmed, attributes, following));
                }
                Vec::new()
            }
        }
    }
}

/// `|===`, or a longer run of `=`.
fn is_table_delimiter(trimmed: &str) -> bool {
    trimmed
        .strip_prefix('|')
        .is_some_and(|rest| rest.len() >= 3 && rest.bytes().all(|byte| byte == b'='))
}

/// A cell's spec: its spans, how many cells it stands for, and whether its style is literal
/// (`None` when it names no style).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spec {
    colspan: usize,
    rowspan: usize,
    repeat: usize,
    literal: Option<bool>,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            colspan: 1,
            rowspan: 1,
            repeat: 1,
            literal: None,
        }
    }
}

#[derive(Debug)]
struct Table {
    /// The line that closes the table.
    delimiter: String,
    /// Whether its cells are followed: only the default `psv` format with `|` between cells.
    follows: bool,
    /// Whether each column `cols` gives is literal; empty without `cols`.
    columns: Vec<bool>,
    /// The number of columns, once known: from `cols`, else from the first row.
    colcount: Option<usize>,
    /// The columns the current row's cells take.
    visits: usize,
    /// The columns rowspans from rows above take in this row and the next, this row's first.
    rowspans: VecDeque<usize>,
    /// The cells of the current row.
    row_cells: usize,
    /// The rows closed.
    rows: usize,
    /// Whether the first row is a header row.
    header: bool,
    /// Lines that began a cell, after the first: Asciidoctor closes the first row of a table
    /// without `cols` only at the end of a line.
    linenum: usize,
    /// The spec of the open cell.
    open: Option<Spec>,
    /// Whether the open cell is literal.
    literal: bool,
}

impl Table {
    fn new(delimiter: &str, attributes: &[String], following: &str) -> Self {
        let mut cols = None;
        let mut header = false;
        let mut noheader = false;
        let mut follows = true;
        for list in attributes {
            for (position, entry) in entries(list).into_iter().enumerate() {
                match entry.split_once('=') {
                    Some((name, value)) => {
                        let value = unquoted(value.trim());
                        match name.trim() {
                            "cols" => cols = Some(value.to_owned()),
                            "options" | "opts" => {
                                for option in value.split(',').map(str::trim) {
                                    header |= option == "header";
                                    noheader |= option == "noheader";
                                }
                            }
                            "format" => follows &= value == "psv",
                            "separator" => follows &= value == "|",
                            _ => {}
                        }
                    }
                    None if position == 0 => {
                        for option in entry.split('%').skip(1) {
                            let option = option.split(['#', '.']).next().unwrap_or_default();
                            header |= option == "header";
                            noheader |= option == "noheader";
                        }
                    }
                    None => {}
                }
            }
        }
        let columns = cols.as_deref().map(column_styles).unwrap_or_default();
        let header = header || (!noheader && implicit_header(delimiter, following));
        Self {
            delimiter: delimiter.to_owned(),
            follows,
            colcount: (!columns.is_empty()).then_some(columns.len()),
            columns,
            visits: 0,
            rowspans: VecDeque::from([0]),
            row_cells: 0,
            rows: 0,
            header,
            linenum: 0,
            open: None,
            literal: false,
        }
    }

    /// Reads one line of the table: the ranges of it in literal cells.
    fn read(&mut self, line: &str) -> Vec<(usize, usize)> {
        let mut literal = Vec::new();
        if line.trim().is_empty() {
            // A blank line belongs to the open cell.
            return literal;
        }
        // Where the open cell's text on this line begins, and where the text a spec may end
        // begins, after any escaped bar.
        let mut text_start = 0;
        if let Some(rest) = line.strip_prefix('|') {
            self.begin_line(Spec::default());
            text_start = line.len() - rest.len();
        } else if let Some(bar) = line.find('|') {
            if let Some(spec) = start_spec(&line[..bar]) {
                self.begin_line(spec);
                text_start = bar + 1;
            }
        }
        let mut spec_start = text_start;
        let mut search = text_start;
        while let Some(relative) = line[search..].find('|') {
            let bar = search + relative;
            search = bar + 1;
            if line[..bar].ends_with('\\') {
                spec_start = search;
                continue;
            }
            if self.literal {
                literal.push((text_start, bar));
            }
            let spec = end_spec(&line[spec_start..bar]);
            self.close(false);
            self.begin(spec);
            text_start = search;
            spec_start = search;
        }
        if self.literal && text_start < line.len() {
            literal.push((text_start, line.len()));
        }
        literal
    }

    /// A line begins with a cell: the open cell, from a line before, closes there.
    fn begin_line(&mut self, spec: Spec) {
        if self.open.is_some() {
            self.close(true);
        }
        self.linenum += 1;
        self.begin(spec);
    }

    /// A cell with `spec` begins.
    fn begin(&mut self, spec: Spec) {
        self.open = Some(spec);
        // The header row is never literal, whatever its cells' or columns' styles.
        self.literal = !(self.header && self.rows == 0)
            && spec.literal.unwrap_or_else(|| {
                (0..spec.repeat).all(|copy| self.columns.get(self.row_cells + copy) == Some(&true))
            });
    }

    /// The open cell ends, at the end of a line when `eol`.
    fn close(&mut self, eol: bool) {
        let spec = self.open.take().unwrap_or_default();
        self.literal = false;
        for copy in 1..=spec.repeat {
            if spec.rowspan > 1 {
                for row in 1..spec.rowspan {
                    if self.rowspans.len() <= row {
                        self.rowspans.resize(row + 1, 0);
                    }
                    self.rowspans[row] += spec.colspan;
                }
            }
            self.visits += spec.colspan;
            self.row_cells += 1;
            let ended = self
                .colcount
                .is_none_or(|count| self.visits + self.rowspans[0] >= count);
            if ended
                && (self.colcount.is_some() || self.linenum > 1 || (eol && copy == spec.repeat))
            {
                self.close_row();
            }
        }
    }

    fn close_row(&mut self) {
        self.colcount.get_or_insert(self.visits);
        self.visits = 0;
        self.row_cells = 0;
        self.rows += 1;
        self.rowspans.pop_front();
        if self.rowspans.is_empty() {
            self.rowspans.push_back(0);
        }
    }
}

/// Whether Asciidoctor takes the table's first row for a header without being asked: the
/// table's first line, straight after the delimiter, holds it, a blank line follows, and the
/// line after the blank lines begins a cell.
fn implicit_header(delimiter: &str, following: &str) -> bool {
    let mut lines = following
        .lines()
        .map(str::trim_end)
        .take_while(|line| line.trim() != delimiter)
        .filter(|line| !is_line_comment(line.trim()));
    if lines.next().is_none_or(|first| first.trim().is_empty())
        || !lines.next().is_some_and(|second| second.trim().is_empty())
    {
        return false;
    }
    lines
        .find(|line| !line.trim().is_empty())
        .is_none_or(|line| {
            line.starts_with('|')
                || line
                    .find('|')
                    .is_some_and(|bar| start_spec(&line[..bar]).is_some())
        })
}

/// The entries of an attribute list, split at commas outside quotes.
fn entries(list: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut quote = None;
    let mut start = 0;
    for (at, character) in list.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if character == open => quote = None,
            (None, ',') => {
                entries.push(list[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    entries.push(list[start..].trim());
    entries
}

fn unquoted(value: &str) -> &str {
    ["\"", "'"]
        .into_iter()
        .find_map(|quote| value.strip_prefix(quote)?.strip_suffix(quote))
        .unwrap_or(value)
}

/// Whether each column `cols` gives is literal, read as Asciidoctor's `parse_colspecs`: a
/// bare number counts columns; otherwise each comma- (or semicolon-) separated spec is a
/// column, `n*` repeating it, and a spec it cannot read is no column.
fn column_styles(cols: &str) -> Vec<bool> {
    let records: String = cols.chars().filter(|character| *character != ' ').collect();
    if let Ok(count) = records.parse::<usize>() {
        return vec![false; count];
    }
    let separator = if records.contains(',') { ',' } else { ';' };
    let mut columns = Vec::new();
    for record in records.split(separator) {
        if let Some((repeat, literal)) = column_spec(record) {
            columns.extend(std::iter::repeat_n(literal, repeat));
        }
    }
    columns
}

/// A column spec, `(n*)?(align)?(width)?(style)?`: how many columns it stands for and whether
/// they are literal.
fn column_spec(record: &str) -> Option<(usize, bool)> {
    let mut rest = record;
    let mut repeat = 1;
    let digits = leading(rest, |byte| byte.is_ascii_digit());
    if digits > 0 && rest[digits..].starts_with('*') {
        repeat = rest[..digits]
            .parse::<usize>()
            .map_or(MAX_FACTOR, |repeat| repeat.min(MAX_FACTOR));
        rest = &rest[digits + 1..];
    }
    rest = &rest[alignment(rest)..];
    let width = leading(rest, |byte| byte.is_ascii_digit());
    if width > 0 {
        rest = &rest[width..];
        rest = rest.strip_prefix('%').unwrap_or(rest);
    } else {
        rest = rest.strip_prefix('~').unwrap_or(rest);
    }
    let literal = match rest.as_bytes() {
        [] => false,
        [style] if style.is_ascii_lowercase() => *style == b'l',
        _ => return None,
    };
    Some((repeat, literal))
}

/// The spec of a cell at the start of a line, `text` being all of the line before its first
/// bar: Asciidoctor's `CellSpecStartRx`.
fn start_spec(text: &str) -> Option<Spec> {
    cell_spec(text.trim_start_matches([' ', '\t']))
}

/// The spec of the next cell at the end of `text`, the text before a bar, after whitespace:
/// Asciidoctor's `CellSpecEndRx`. Text that ends in no spec gives the empty one.
fn end_spec(text: &str) -> Spec {
    text.rfind([' ', '\t'])
        .and_then(|space| cell_spec(&text[space + 1..]))
        .unwrap_or_default()
}

/// `(span[+*])?(align)?(style)?`, all of `text`.
fn cell_spec(text: &str) -> Option<Spec> {
    let mut spec = Spec::default();
    let mut rest = text;
    let number = leading(rest, |byte| byte.is_ascii_digit() || byte == b'.');
    if number > 0 {
        if let Some(operator) = rest
            .as_bytes()
            .get(number)
            .filter(|byte| b"+*".contains(byte))
        {
            let (columns, rows) = factors(&rest[..number])?;
            if *operator == b'+' {
                (spec.colspan, spec.rowspan) = (columns, rows);
            } else {
                spec.repeat = columns;
            }
            rest = &rest[number + 1..];
        }
    }
    rest = &rest[alignment(rest)..];
    match rest.as_bytes() {
        [] => {}
        [style] if style.is_ascii_lowercase() => {
            spec.literal = b"dsemhlva".contains(style).then_some(*style == b'l');
        }
        _ => return None,
    }
    Some(spec)
}

/// The column and row factors of `n`, `n.m` or `.m`, each 1 when left out.
fn factors(number: &str) -> Option<(usize, usize)> {
    let (columns, rows) = number.split_once('.').unwrap_or((number, ""));
    if rows.contains('.') || (columns.is_empty() && rows.is_empty()) {
        return None;
    }
    let factor = |digits: &str| {
        if digits.is_empty() {
            Some(1)
        } else {
            match digits.parse::<usize>() {
                Ok(0) => None,
                Ok(factor) => Some(factor.min(MAX_FACTOR)),
                // Only digits, so too large.
                Err(_) => Some(MAX_FACTOR),
            }
        }
    };
    Some((factor(columns)?, factor(rows)?))
}

/// The length of the alignment at the start of `text`: `x`, `x.`, `x.y` or `.y`, each of `x`
/// and `y` one of `<`, `^` and `>`.
fn alignment(text: &str) -> usize {
    let aligned = |at: usize| {
        text.as_bytes()
            .get(at)
            .is_some_and(|byte| b"<^>".contains(byte))
    };
    let dot = |at: usize| text.as_bytes().get(at) == Some(&b'.');
    if aligned(0) {
        match (dot(1), aligned(2)) {
            (true, true) => 3,
            (true, false) => 2,
            _ => 1,
        }
    } else if dot(0) && aligned(1) {
        2
    } else {
        0
    }
}

fn leading(text: &str, accepted: impl Fn(u8) -> bool) -> usize {
    text.bytes().take_while(|byte| accepted(*byte)).count()
}

#[cfg(test)]
mod tests {
    use super::{cell_spec, column_styles, end_spec, Spec, Tables, MAX_FACTOR};

    #[test]
    fn reads_cell_specs_as_asciidoctor_does() {
        let spec = |colspan, rowspan, repeat, literal| Spec {
            colspan,
            rowspan,
            repeat,
            literal,
        };
        assert_eq!(cell_spec("l"), Some(spec(1, 1, 1, Some(true))));
        assert_eq!(cell_spec("2+l"), Some(spec(2, 1, 1, Some(true))));
        assert_eq!(cell_spec(".2+^.>l"), Some(spec(1, 2, 1, Some(true))));
        assert_eq!(cell_spec("3*a"), Some(spec(1, 1, 3, Some(false))));
        assert_eq!(cell_spec("<.>"), Some(spec(1, 1, 1, None)));
        assert_eq!(
            cell_spec("x"),
            Some(spec(1, 1, 1, None)),
            "no style, still a spec"
        );
        assert_eq!(cell_spec(""), Some(Spec::default()));
        assert_eq!(cell_spec("xl"), None);
        assert_eq!(cell_spec("2"), None);
        assert_eq!(cell_spec("l "), None);
        assert_eq!(end_spec("text l"), spec(1, 1, 1, Some(true)));
        assert_eq!(end_spec("textl"), Spec::default());
        assert_eq!(end_spec("text "), Spec::default());
    }

    /// Factors typed by mistake are capped, so a table of them costs little.
    #[test]
    fn huge_factors_are_capped() {
        assert_eq!(
            cell_spec("99999999999999999999*l").map(|spec| spec.repeat),
            Some(MAX_FACTOR)
        );
        assert_eq!(
            cell_spec(".999999999+").map(|spec| spec.rowspan),
            Some(MAX_FACTOR)
        );
        assert_eq!(column_styles("99999999999*l").len(), MAX_FACTOR);
        let mut tables = Tables::default();
        let lines = ["|===", "999999999*l|a .999999999+|b", "|c", "|==="];
        for (at, line) in lines.iter().enumerate() {
            let following = lines[at + 1..].join("\n");
            let _ = tables.line(line, true, &[], &following);
        }
    }

    #[test]
    fn reads_column_styles_as_asciidoctor_does() {
        assert_eq!(column_styles("1,1l"), [false, true]);
        assert_eq!(column_styles("2*l,1"), [true, true, false]);
        assert_eq!(column_styles("3"), [false, false, false]);
        assert_eq!(column_styles("<.^2l; 30%m;~"), [true, false, false]);
        assert_eq!(column_styles("1, ,1l"), [false, false, true]);
        assert_eq!(
            column_styles("1,!,l"),
            [false, true],
            "an unreadable spec is no column"
        );
    }
}
