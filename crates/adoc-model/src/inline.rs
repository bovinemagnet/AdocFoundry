//! Inline edits on one text block.
//!
//! A block is flattened into units (text runs and atoms) each carrying the stack of marks around
//! it, edited as a list, and written back with every mark keeping the delimiters it was written
//! with. The result is rescanned: it must show the text and marks intended, or the edit is
//! retried with escapes or unconstrained delimiters, and refused if neither keeps the meaning.

use adoc_core::SourceRange;

use crate::{
    scan::{is_word, Scanner},
    tree::{Atom, Mark, MarkSet, Piece, TextBlock, TextContext},
};

/// A mark around units, as written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Frame {
    pub(crate) id: u32,
    pub(crate) mark: Mark,
    /// A role or id prefix, `[.role]`.
    pub(crate) prefix: String,
    /// The delimiter, written before and after: `*`, `**`, `^`, ….
    pub(crate) delimiter: String,
}

impl Frame {
    fn unconstrained(&self) -> Self {
        let character = self.mark.delimiter();
        let delimiter = if self.mark.has_forms() {
            [character, character].iter().collect()
        } else {
            character.to_string()
        };
        Self {
            delimiter,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UnitKind {
    Text,
    Atom(Atom),
}

/// A text run or an atom, with its marks outermost first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Unit {
    pub(crate) kind: UnitKind,
    pub(crate) shown: String,
    pub(crate) source: String,
    pub(crate) stack: Vec<Frame>,
}

impl Unit {
    fn text(text: &str, stack: Vec<Frame>) -> Self {
        Self {
            kind: UnitKind::Text,
            shown: text.to_owned(),
            source: text.to_owned(),
            stack,
        }
    }

    fn marks(&self) -> MarkSet {
        self.stack.iter().map(|frame| frame.mark).collect()
    }
}

/// A block's units and the source span of each, with the next free frame id.
pub(crate) struct Flat {
    pub(crate) units: Vec<Unit>,
    /// Source range of each unit, parallel to `units`.
    pub(crate) ranges: Vec<SourceRange>,
    /// Every frame with its open delimiter's end and close delimiter's start.
    pub(crate) frames: Vec<(Frame, usize, usize)>,
    pub(crate) next_id: u32,
}

pub(crate) fn flatten(source: &str, block: &TextBlock) -> Flat {
    fn visit(source: &str, pieces: &[Piece], stack: &mut Vec<Frame>, flat: &mut Flat) {
        for piece in pieces {
            match piece {
                Piece::Text { range } => {
                    flat.units
                        .push(Unit::text(&source[range.start..range.end], stack.clone()));
                    flat.ranges.push(*range);
                }
                Piece::Atom { atom, range, shown } => {
                    flat.units.push(Unit {
                        kind: UnitKind::Atom(atom.clone()),
                        shown: shown.clone(),
                        source: source[range.start..range.end].to_owned(),
                        stack: stack.clone(),
                    });
                    flat.ranges.push(*range);
                }
                Piece::Mark {
                    mark,
                    open,
                    close,
                    children,
                } => {
                    let delimiter = source[close.start..close.end].to_owned();
                    let open_text = &source[open.start..open.end];
                    let prefix = open_text
                        .strip_suffix(delimiter.as_str())
                        .unwrap_or("")
                        .to_owned();
                    let frame = Frame {
                        id: flat.next_id,
                        mark: *mark,
                        prefix,
                        delimiter,
                    };
                    flat.next_id += 1;
                    flat.frames.push((frame.clone(), open.end, close.start));
                    stack.push(frame);
                    visit(source, children, stack, flat);
                    stack.pop();
                }
            }
        }
    }
    let mut flat = Flat {
        units: Vec::new(),
        ranges: Vec::new(),
        frames: Vec::new(),
        next_id: 1,
    };
    visit(source, &block.pieces, &mut Vec::new(), &mut flat);
    flat
}

/// Units written back as source, with where each unit landed (relative to the block start).
pub(crate) struct Written {
    pub(crate) text: String,
    pub(crate) starts: Vec<usize>,
    pub(crate) ends: Vec<usize>,
}

pub(crate) fn write(units: &[Unit]) -> Written {
    let mut text = String::new();
    let mut open: Vec<&Frame> = Vec::new();
    let mut starts = Vec::with_capacity(units.len());
    let mut ends = Vec::with_capacity(units.len());
    for unit in units {
        let common = open
            .iter()
            .zip(&unit.stack)
            .take_while(|(left, right)| left.id == right.id)
            .count();
        while open.len() > common {
            if let Some(frame) = open.pop() {
                text.push_str(&frame.delimiter);
            }
        }
        for frame in &unit.stack[common..] {
            text.push_str(&frame.prefix);
            text.push_str(&frame.delimiter);
            open.push(frame);
        }
        starts.push(text.len());
        text.push_str(&unit.source);
        ends.push(text.len());
    }
    while let Some(frame) = open.pop() {
        text.push_str(&frame.delimiter);
    }
    Written { text, starts, ends }
}

/// What a block shows, for checking an edit: characters with their marks, and atoms by kind
/// and source. Replacements count as the text they replace, escapes as what they keep, and
/// `{empty}` as nothing.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    Char(char, MarkSet),
    Atom(String, String, MarkSet),
}

fn signature(units: &[Unit]) -> Vec<Shown> {
    let mut out = Vec::new();
    for unit in units {
        let marks = unit.marks();
        match &unit.kind {
            UnitKind::Text => out.extend(unit.shown.chars().map(|c| Shown::Char(c, marks))),
            UnitKind::Atom(Atom::Escape) => {
                out.extend(unit.shown.chars().map(|c| Shown::Char(c, marks)));
            }
            UnitKind::Atom(Atom::Replacement) => {
                out.extend(unit.source.chars().map(|c| Shown::Char(c, marks)));
            }
            UnitKind::Atom(Atom::Attribute { name }) if name == "empty" => {}
            UnitKind::Atom(atom) => {
                let kind = format!("{atom:?}");
                let kind = kind.split([' ', '{']).next().unwrap_or("").to_owned();
                out.push(Shown::Atom(kind, unit.source.clone(), marks));
            }
        }
    }
    out
}

/// The page around one block, for writing and rescanning it.
pub(crate) struct Context<'a> {
    pub(crate) source: &'a str,
    pub(crate) attributes: &'a std::collections::BTreeMap<String, String>,
    pub(crate) block: &'a TextBlock,
}

/// An inline edit's outcome: the block's new source, where the selection lands in it, and its
/// new pieces.
pub(crate) struct Realised {
    pub(crate) text: String,
    /// Selection in the new block text, relative to its start.
    pub(crate) selection: (usize, usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Refusal {
    /// No escape or delimiter form keeps the meaning of the text around the edit.
    WouldChangeMeaning,
}

impl Context<'_> {
    /// Writes `units`, retrying with escapes and unconstrained delimiters for `touched` frames,
    /// until a rescan shows exactly what `units` intend. `selection` is a pair of unit
    /// display positions to report in the written text.
    pub(crate) fn realise(
        &self,
        units: Vec<Unit>,
        touched: &[u32],
        selection: (Caret, Caret),
    ) -> Result<Realised, Refusal> {
        let intended = signature(&units);
        let mut candidates = vec![units.clone()];
        let upgraded: Vec<Unit> = units
            .iter()
            .map(|unit| Unit {
                stack: unit
                    .stack
                    .iter()
                    .map(|frame| {
                        if touched.contains(&frame.id) {
                            frame.unconstrained()
                        } else {
                            frame.clone()
                        }
                    })
                    .collect(),
                ..unit.clone()
            })
            .collect();
        if upgraded != units {
            candidates.push(upgraded);
        }
        for candidate in candidates {
            let mut units = candidate;
            // Each round escapes one construct the edit created by accident.
            for _ in 0..=units.len() + 4 {
                let written = write(&units);
                let rescanned = self.rescan(&written.text);
                if signature(&rescanned) == intended {
                    if let Some(units_with_guard) = self.guard_line_starts(&units, &written) {
                        units = units_with_guard;
                        continue;
                    }
                    if self.block.context == TextContext::Cell && has_bare_bar(&units) {
                        units = escape_bars(units);
                        continue;
                    }
                    return Ok(Realised {
                        selection: (
                            selection.0.offset(&units, &written),
                            selection.1.offset(&units, &written),
                        ),
                        text: written.text,
                    });
                }
                match self.accidental(&units, &rescanned) {
                    Some(escaped) => units = escaped,
                    None => break,
                }
            }
        }
        Err(Refusal::WouldChangeMeaning)
    }

    /// The units of `text` written in place of the block.
    fn rescan(&self, text: &str) -> Vec<Unit> {
        let start = self.block.range.start;
        let mut page = String::with_capacity(self.source.len() + text.len());
        page.push_str(&self.source[..start]);
        page.push_str(text);
        page.push_str(&self.source[self.block.range.end..]);
        let range = SourceRange::new(start, start + text.len());
        let pieces = Scanner::new(
            &page,
            self.attributes,
            self.block.context == TextContext::Cell,
        )
        .scan(range);
        let block = TextBlock {
            range,
            pieces,
            plain: false,
            display: String::new(),
            context: self.block.context,
        };
        flatten(&page, &block).units
    }

    /// `units` with an escape before the first construct the rescan found that the units do not
    /// intend: plain text that became a mark or an atom.
    fn accidental(&self, units: &[Unit], rescanned: &[Unit]) -> Option<Vec<Unit>> {
        let intended = signature(units);
        let found = signature(rescanned);
        let first = intended
            .iter()
            .zip(&found)
            .position(|(left, right)| left != right)
            .unwrap_or(intended.len().min(found.len()));
        // Only a character that can start markup is worth escaping.
        if let Some(Shown::Char(character, _)) = intended.get(first) {
            let starts_markup = matches!(
                character,
                '*' | '_' | '`' | '#' | '^' | '~' | '+' | '[' | '{' | '<' | '"' | '\'' | '(' | '|'
            ) || character.is_ascii_alphabetic();
            if !starts_markup {
                return None;
            }
        }
        // The intended character at that position, and the unit holding it.
        let mut seen = 0;
        for (index, unit) in units.iter().enumerate() {
            let length = signature(std::slice::from_ref(unit)).len();
            if seen + length > first {
                let offset_chars = first - seen;
                return escape_in(units, index, offset_chars);
            }
            seen += length;
        }
        // The rescan found more than intended at the end: escape the last text character.
        let last = units
            .iter()
            .rposition(|unit| unit.kind == UnitKind::Text && !unit.source.is_empty())?;
        let count = units[last].source.chars().count();
        escape_in(units, last, count - 1)
    }

    /// Prefixes `{empty}` to lines of a flowing block that would otherwise start a block of
    /// another kind; `None` when no line needs it.
    fn guard_line_starts(&self, units: &[Unit], written: &Written) -> Option<Vec<Unit>> {
        if self.block.context != TextContext::Flow {
            return None;
        }
        let text = &written.text;
        let mut line_start = 0;
        loop {
            let line_end = text[line_start..]
                .find('\n')
                .map_or(text.len(), |offset| line_start + offset);
            let line = &text[line_start..line_end];
            if starts_block(line, line_start == 0) {
                return insert_empty(units, written, line_start);
            }
            if line_end == text.len() {
                return None;
            }
            line_start = line_end + 1;
        }
    }
}

/// A selection end: a display offset, and whether it belongs to the text before it (inside
/// that text's marks) rather than the text after.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Caret {
    pub(crate) display: usize,
    pub(crate) sticky: bool,
}

impl Caret {
    /// The offset in the written text.
    fn offset(self, units: &[Unit], written: &Written) -> usize {
        let mut at = 0;
        for (index, unit) in units.iter().enumerate() {
            let length = unit.shown.len();
            if self.display < at + length {
                if self.display == at {
                    return if self.sticky && index > 0 {
                        written.ends[index - 1]
                    } else {
                        written.starts[index]
                    };
                }
                return match unit.kind {
                    UnitKind::Text => written.starts[index] + (self.display - at),
                    UnitKind::Atom(_) => written.ends[index],
                };
            }
            at += length;
        }
        match written.ends.last() {
            Some(end) if self.sticky => *end,
            _ => written.text.len(),
        }
    }
}

/// Splits the text unit at `index` so that its character `offset_chars` becomes an escape.
fn escape_in(units: &[Unit], index: usize, offset_chars: usize) -> Option<Vec<Unit>> {
    let unit = &units[index];
    if unit.kind != UnitKind::Text {
        return None;
    }
    let (byte, character) = unit.source.char_indices().nth(offset_chars)?;
    let before = &unit.source[..byte];
    let after = &unit.source[byte + character.len_utf8()..];
    let mut replaced: Vec<Unit> = Vec::new();
    if !before.is_empty() {
        replaced.push(Unit::text(before, unit.stack.clone()));
    }
    replaced.push(Unit {
        kind: UnitKind::Atom(Atom::Escape),
        shown: character.to_string(),
        source: format!("\\{character}"),
        stack: unit.stack.clone(),
    });
    if !after.is_empty() {
        replaced.push(Unit::text(after, unit.stack.clone()));
    }
    let mut out = units[..index].to_vec();
    out.extend(replaced);
    out.extend_from_slice(&units[index + 1..]);
    Some(out)
}

fn has_bare_bar(units: &[Unit]) -> bool {
    units
        .iter()
        .any(|unit| unit.kind == UnitKind::Text && unit.source.contains('|'))
}

fn escape_bars(units: Vec<Unit>) -> Vec<Unit> {
    let mut out = Vec::new();
    for unit in units {
        if unit.kind != UnitKind::Text || !unit.source.contains('|') {
            out.push(unit);
            continue;
        }
        let mut rest = unit.source.as_str();
        while let Some(at) = rest.find('|') {
            if at > 0 {
                out.push(Unit::text(&rest[..at], unit.stack.clone()));
            }
            out.push(Unit {
                kind: UnitKind::Atom(Atom::Escape),
                shown: "|".to_owned(),
                source: "\\|".to_owned(),
                stack: unit.stack.clone(),
            });
            rest = &rest[at + 1..];
        }
        if !rest.is_empty() {
            out.push(Unit::text(rest, unit.stack.clone()));
        }
    }
    out
}

/// `units` with an `{empty}` attribute reference written at byte `at` of their text.
fn insert_empty(units: &[Unit], written: &Written, at: usize) -> Option<Vec<Unit>> {
    let index = (0..units.len())
        .find(|index| written.starts[*index] <= at && at < written.ends[*index])
        .or_else(|| (0..units.len()).find(|index| written.starts[*index] >= at))?;
    let unit = &units[index];
    let empty = |stack: Vec<Frame>| Unit {
        kind: UnitKind::Atom(Atom::Attribute {
            name: "empty".to_owned(),
        }),
        shown: String::new(),
        source: "{empty}".to_owned(),
        stack,
    };
    let mut out = units[..index].to_vec();
    if written.starts[index] >= at || unit.kind != UnitKind::Text {
        if index > 0 && units[index - 1].source == "{empty}" {
            return None;
        }
        out.push(empty(unit.stack.clone()));
        out.extend_from_slice(&units[index..]);
        return Some(out);
    }
    let split = at - written.starts[index];
    if unit.source[split..].starts_with("{empty}") {
        return None;
    }
    out.push(Unit::text(&unit.source[..split], unit.stack.clone()));
    out.push(empty(unit.stack.clone()));
    out.push(Unit::text(&unit.source[split..], unit.stack.clone()));
    out.extend_from_slice(&units[index + 1..]);
    Some(out)
}

/// Whether `line`, in a paragraph, would start a block of another kind.
pub(crate) fn starts_block(line: &str, first: bool) -> bool {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return false;
    }
    let is_run = |c: char, min: usize| trimmed.len() >= min && trimmed.chars().all(|x| x == c);
    let delimiter = ["-", "=", "*", "_", "+", ".", "/"]
        .iter()
        .any(|c| is_run(c.chars().next().unwrap_or('-'), 4))
        || trimmed == "--"
        || trimmed.starts_with("|===")
        || trimmed.starts_with("```")
        || trimmed == "+";
    let marker = crate::build::list_marker(line).is_some();
    let heading = {
        let level = trimmed
            .chars()
            .take_while(|c| *c == '=' || *c == '#')
            .count();
        level > 0 && trimmed[level..].starts_with(' ')
    };
    let description = trimmed.ends_with("::")
        || trimmed.ends_with(";;")
        || [":: ", "::: ", ":::: ", ";; "]
            .iter()
            .any(|d| trimmed.contains(d));
    let directive = [
        "include::",
        "ifdef::",
        "ifndef::",
        "ifeval::",
        "endif::",
        "image::",
        "video::",
        "audio::",
        "toc::",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix));
    let attribute = trimmed.starts_with(':') && trimmed[1..].contains(':');
    let comment = trimmed.starts_with("//");
    let first_only = first
        && (line.starts_with([' ', '\t'])
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
            || (trimmed.starts_with('.')
                && trimmed.len() > 1
                && !trimmed[1..].starts_with(['.', ' ']))
            || ["NOTE: ", "TIP: ", "IMPORTANT: ", "CAUTION: ", "WARNING: "]
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
            || trimmed == "<<<"
            || trimmed == "'''");
    delimiter || marker || heading || description || directive || attribute || comment || first_only
}

/// Whether a new mark around `display` of a block may use the constrained form.
pub(crate) fn constrained_fits(display: &str, start: usize, end: usize) -> bool {
    let before = display[..start].chars().next_back();
    let after = display[end..].chars().next();
    let content = &display[start..end];
    !before.is_some_and(is_word)
        && !after.is_some_and(is_word)
        && !content.starts_with(char::is_whitespace)
        && !content.ends_with(char::is_whitespace)
}
