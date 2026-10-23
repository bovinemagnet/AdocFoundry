//! The inline scanner: reads a text block's source into pieces that tile it exactly, so every
//! byte is a text run, a mark delimiter or part of an atom.
//!
//! It follows Asciidoctor's inline rules closely enough to agree with the parser on ordinary
//! prose; where it does not, the block is checked against the DOM and made read-only.

use std::collections::BTreeMap;

use adoc_core::SourceRange;

use crate::tree::{Atom, Mark, Piece};

/// Inline macros the scanner reads as atoms.
const MACROS: &[&str] = &[
    "xref",
    "link",
    "mailto",
    "image",
    "footnote",
    "footnoteref",
    "kbd",
    "btn",
    "menu",
    "icon",
    "stem",
    "latexmath",
    "asciimath",
    "pass",
    "anchor",
    "indexterm",
    "indexterm2",
];

const URL_SCHEMES: &[&str] = &["https://", "http://", "ftp://", "irc://", "file://"];

/// Attributes every document has.
const INTRINSIC: &[(&str, &str)] = &[
    ("empty", ""),
    ("sp", " "),
    ("nbsp", "\u{a0}"),
    ("zwsp", "\u{200b}"),
    ("wj", "\u{2060}"),
    ("apos", "'"),
    ("quot", "\""),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("ldquo", "\u{201c}"),
    ("rdquo", "\u{201d}"),
    ("deg", "\u{b0}"),
    ("plus", "+"),
    ("brvbar", "\u{a6}"),
    ("vbar", "|"),
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("startsb", "["),
    ("endsb", "]"),
    ("caret", "^"),
    ("asterisk", "*"),
    ("tilde", "~"),
    ("backslash", "\\"),
    ("backtick", "`"),
    ("two-colons", "::"),
    ("two-semicolons", ";;"),
    ("cpp", "C++"),
    ("cxx", "C++"),
    ("pp", "++"),
];

/// How a mark's delimiters are written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Form {
    Constrained,
    Unconstrained,
    /// Superscript and subscript: a single character, no spaces inside.
    Tight,
}

/// Marks in the order Asciidoctor's quote substitutions try them.
const MARK_ORDER: &[(Mark, Form)] = &[
    (Mark::Strong, Form::Unconstrained),
    (Mark::Strong, Form::Constrained),
    (Mark::Code, Form::Unconstrained),
    (Mark::Code, Form::Constrained),
    (Mark::Emphasis, Form::Unconstrained),
    (Mark::Emphasis, Form::Constrained),
    (Mark::Highlight, Form::Unconstrained),
    (Mark::Highlight, Form::Constrained),
    (Mark::Superscript, Form::Tight),
    (Mark::Subscript, Form::Tight),
];

pub(crate) struct Scanner<'a> {
    source: &'a str,
    attributes: &'a BTreeMap<String, String>,
    /// In a table cell, where `\|` keeps a bar literal.
    cell: bool,
}

pub(crate) fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(
        source: &'a str,
        attributes: &'a BTreeMap<String, String>,
        cell: bool,
    ) -> Self {
        Self {
            source,
            attributes,
            cell,
        }
    }

    /// The pieces of `range`, which they tile.
    pub(crate) fn scan(&self, range: SourceRange) -> Vec<Piece> {
        self.pieces(range.start, range.end, range.start)
    }

    fn pieces(&self, start: usize, end: usize, block_start: usize) -> Vec<Piece> {
        let mut out = Vec::new();
        let mut text_start = start;
        let mut index = start;
        while index < end {
            if let Some((piece, next)) = self.construct(index, end, block_start) {
                self.push_text(&mut out, text_start, index);
                out.push(piece);
                index = next;
                text_start = next;
            } else {
                index += self.char_len(index);
            }
        }
        self.push_text(&mut out, text_start, end);
        out
    }

    /// Plain text, split around typographic replacements.
    fn push_text(&self, out: &mut Vec<Piece>, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let text = &self.source[start..end];
        let mut last = start;
        for span in adoc_dom::replacement_spans(text) {
            let (from, to) = (start + span.range.start, start + span.range.end);
            if from > last {
                out.push(Piece::Text {
                    range: SourceRange::new(last, from),
                });
            }
            let escaped = text[span.range.clone()].starts_with('\\');
            out.push(Piece::Atom {
                atom: if escaped {
                    Atom::Escape
                } else {
                    Atom::Replacement
                },
                range: SourceRange::new(from, to),
                shown: span.replacement.to_owned(),
            });
            last = to;
        }
        if end > last {
            out.push(Piece::Text {
                range: SourceRange::new(last, end),
            });
        }
    }

    fn char_at(&self, index: usize) -> Option<char> {
        self.source.get(index..)?.chars().next()
    }

    fn char_len(&self, index: usize) -> usize {
        self.char_at(index).map_or(1, char::len_utf8)
    }

    fn char_before(&self, index: usize, block_start: usize) -> Option<char> {
        if index <= block_start {
            return None;
        }
        self.source[block_start..index].chars().next_back()
    }

    fn starts(&self, index: usize, end: usize, pattern: &str) -> bool {
        self.source
            .get(index..end)
            .is_some_and(|rest| rest.starts_with(pattern))
    }

    /// The construct starting at `index`, and where it ends.
    fn construct(&self, index: usize, end: usize, block_start: usize) -> Option<(Piece, usize)> {
        self.escape(index, end, block_start)
            .or_else(|| self.atom(index, end, block_start))
            .or_else(|| self.mark(index, end, block_start))
    }

    /// A backslash keeping the construct after it literal.
    fn escape(&self, index: usize, end: usize, block_start: usize) -> Option<(Piece, usize)> {
        if self.char_at(index)? != '\\' {
            return None;
        }
        let next = index + 1;
        let character = self.char_at(next).filter(|_| next < end)?;
        let escapable = matches!(
            character,
            '*' | '_' | '`' | '#' | '^' | '~' | '+' | '[' | '{' | '<' | '"' | '\''
        ) || (self.cell && character == '|')
            || self.atom_at(next, end, block_start, false).is_some();
        if !escapable {
            return None;
        }
        let after = next + character.len_utf8();
        Some((
            Piece::Atom {
                atom: Atom::Escape,
                range: SourceRange::new(index, after),
                shown: character.to_string(),
            },
            after,
        ))
    }

    /// An atom starting at `index`: anchors, passthroughs, references, macros, links,
    /// attribute references, index terms, curved quotes and hard line breaks.
    fn atom(&self, index: usize, end: usize, block_start: usize) -> Option<(Piece, usize)> {
        self.atom_at(index, end, block_start, true)
    }

    /// As [`Self::atom`]; without `check_before`, a macro or URL is read even after a word
    /// character or backslash, as when deciding whether a backslash escapes it.
    fn atom_at(
        &self,
        index: usize,
        end: usize,
        block_start: usize,
        check_before: bool,
    ) -> Option<(Piece, usize)> {
        let atom = |atom: Atom, to: usize, shown: String| {
            Some((
                Piece::Atom {
                    atom,
                    range: SourceRange::new(index, to),
                    shown,
                },
                to,
            ))
        };
        let rest = self.source.get(index..end)?;
        let first = rest.chars().next()?;
        match first {
            '[' if rest.starts_with("[[") => {
                let close = rest.find("]]")?;
                let inner = &rest[2..close];
                if inner.is_empty() || inner.contains(['\n', '[']) {
                    return None;
                }
                // `[[[bibliography]]]` takes one more bracket on each side.
                let to = index + close + 2;
                let id = inner.trim_start_matches('[');
                let id = id.split(',').next().unwrap_or(id).trim().to_owned();
                return atom(Atom::Anchor { id }, to, String::new());
            }
            '+' => return self.passthrough(index, end, block_start),
            '<' if rest.starts_with("<<") => {
                let close = rest.find(">>")?;
                let inner = &rest[2..close];
                if inner.is_empty() || inner.contains('\n') || inner.starts_with(' ') {
                    return None;
                }
                let (target, text) = match inner.split_once(',') {
                    Some((target, text)) => (target.trim(), Some(text.trim().to_owned())),
                    None => (inner.trim(), None),
                };
                let shown = text.clone().unwrap_or_else(|| xref_label(target));
                return atom(
                    Atom::Xref {
                        target: target.to_owned(),
                        text: text.filter(|text| !text.is_empty()),
                    },
                    index + close + 2,
                    shown,
                );
            }
            '{' => {
                let close = rest.find('}')?;
                let name = &rest[1..close];
                let valid = !name.is_empty()
                    && name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if !valid {
                    return None;
                }
                let value = self.attributes.get(name).cloned().or_else(|| {
                    INTRINSIC
                        .iter()
                        .find(|(intrinsic, _)| *intrinsic == name)
                        .map(|(_, value)| (*value).to_owned())
                })?;
                return atom(
                    Atom::Attribute {
                        name: name.to_owned(),
                    },
                    index + close + 1,
                    value,
                );
            }
            '(' if rest.starts_with("(((") => {
                let close = rest.find(")))")?;
                return atom(Atom::IndexTerm, index + close + 3, String::new());
            }
            '(' if rest.starts_with("((") => {
                let close = rest.find("))")?;
                let term = &rest[2..close];
                if term.is_empty() || term.contains('\n') {
                    return None;
                }
                return atom(Atom::IndexTerm, index + close + 2, term.to_owned());
            }
            ' ' if rest.starts_with(" +") => {
                let after = index + 2;
                let at_line_end = after == end || self.starts(after, end, "\n");
                if at_line_end {
                    return atom(Atom::LineBreak, after, "\n".to_owned());
                }
                return None;
            }
            '"' | '\'' if rest[1..].starts_with('`') => {
                return self.curved_quote(index, end, block_start);
            }
            _ => {}
        }
        if first.is_ascii_alphabetic() {
            if check_before
                && self
                    .char_before(index, block_start)
                    .is_some_and(|before| is_word(before) || before == '\\')
            {
                return None;
            }
            if let Some(found) = self.url(index, end) {
                return Some(found);
            }
            return self.inline_macro(index, end);
        }
        None
    }

    /// `+text+`, `++text++`, `+++text+++` and `pass:[text]` show their text as written.
    fn passthrough(&self, index: usize, end: usize, block_start: usize) -> Option<(Piece, usize)> {
        let rest = &self.source[index..end];
        for delimiter in ["+++", "++"] {
            if let Some(body) = rest.strip_prefix(delimiter) {
                let close = body.find(delimiter)?;
                if close == 0 {
                    return None;
                }
                let to = index + delimiter.len() + close + delimiter.len();
                return Some((
                    Piece::Atom {
                        atom: Atom::Passthrough,
                        range: SourceRange::new(index, to),
                        shown: body[..close].to_owned(),
                    },
                    to,
                ));
            }
        }
        // Constrained: `+text+` at a word boundary.
        let before = self.char_before(index, block_start);
        if before.is_some_and(|c| is_word(c) || matches!(c, ';' | ':' | '\\')) {
            return None;
        }
        let body = &rest[1..];
        if body.starts_with(char::is_whitespace) {
            return None;
        }
        let mut search = 0;
        while let Some(found) = body[search..].find('+') {
            let close = search + found;
            let inner = &body[..close];
            let after = body[close + 1..].chars().next();
            if !inner.is_empty()
                && !inner.ends_with(char::is_whitespace)
                && !after.is_some_and(is_word)
            {
                let to = index + 1 + close + 1;
                return Some((
                    Piece::Atom {
                        atom: Atom::Passthrough,
                        range: SourceRange::new(index, to),
                        shown: inner.to_owned(),
                    },
                    to,
                ));
            }
            search = close + 1;
        }
        None
    }

    /// ``"`text`"`` and ``'`text`'``.
    fn curved_quote(&self, index: usize, end: usize, block_start: usize) -> Option<(Piece, usize)> {
        let rest = &self.source[index..end];
        let quote = rest.chars().next()?;
        if self
            .char_before(index, block_start)
            .is_some_and(|c| is_word(c) || matches!(c, ';' | ':' | '}'))
        {
            return None;
        }
        let closing = format!("`{quote}");
        let body = &rest[2..];
        let close = body.find(&closing)?;
        let inner = &body[..close];
        let after = body[close + 2..].chars().next();
        if inner.is_empty()
            || inner.starts_with(char::is_whitespace)
            || inner.ends_with(char::is_whitespace)
            || after.is_some_and(is_word)
        {
            return None;
        }
        let (open, shut) = if quote == '"' {
            ('\u{201c}', '\u{201d}')
        } else {
            ('\u{2018}', '\u{2019}')
        };
        let to = index + 2 + close + 2;
        let inner_start = index + 2;
        let shown = self.plain(inner_start, inner_start + inner.len(), inner_start);
        Some((
            Piece::Atom {
                atom: Atom::Quoted,
                range: SourceRange::new(index, to),
                shown: format!("{open}{shown}{shut}"),
            },
            to,
        ))
    }

    /// The text of `start..end` as shown, without formatting.
    fn plain(&self, start: usize, end: usize, block_start: usize) -> String {
        let mut out = String::new();
        push_plain(self.source, &self.pieces(start, end, block_start), &mut out);
        out
    }

    /// A URL, bare or with link text in brackets.
    fn url(&self, index: usize, end: usize) -> Option<(Piece, usize)> {
        let rest = &self.source[index..end];
        URL_SCHEMES
            .iter()
            .find(|scheme| rest.starts_with(**scheme))?;
        let target_len = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '[' | ']' | '<'))
            .unwrap_or(rest.len());
        if rest[target_len..].starts_with('[') {
            let (text, to) = self.brackets(index + target_len, end)?;
            let target = rest[..target_len].to_owned();
            let text = text.filter(|text| !text.is_empty());
            let shown = text.clone().unwrap_or_else(|| target.clone());
            return Some((
                Piece::Atom {
                    atom: Atom::Link { target, text },
                    range: SourceRange::new(index, to),
                    shown,
                },
                to,
            ));
        }
        // A bare URL does not take trailing punctuation.
        let target = rest[..target_len].trim_end_matches(['.', ',', '?', '!', ')', ';', ':']);
        if URL_SCHEMES.contains(&target) {
            return None;
        }
        let to = index + target.len();
        Some((
            Piece::Atom {
                atom: Atom::Link {
                    target: target.to_owned(),
                    text: None,
                },
                range: SourceRange::new(index, to),
                shown: target.to_owned(),
            },
            to,
        ))
    }

    /// The shown text of an attribute list starting with `[` at `open`, and the offset after
    /// its `]`. The first unescaped `]` closes it, as in Asciidoctor.
    fn brackets(&self, open: usize, end: usize) -> Option<(Option<String>, usize)> {
        let rest = &self.source[open..end];
        if !rest.starts_with('[') {
            return None;
        }
        let mut escaped = false;
        for (offset, character) in rest.char_indices().skip(1) {
            match character {
                '\\' if !escaped => escaped = true,
                ']' if !escaped => {
                    let inner_start = open + 1;
                    let inner_end = open + offset;
                    let text = self.plain(inner_start, inner_end, inner_start);
                    let text = text.replace("\\]", "]");
                    return Some((Some(text), inner_end + 1));
                }
                _ => escaped = false,
            }
        }
        None
    }

    /// `name:target[attributes]` for the macros the model knows.
    fn inline_macro(&self, index: usize, end: usize) -> Option<(Piece, usize)> {
        let rest = &self.source[index..end];
        let colon = rest.find(':')?;
        let name = &rest[..colon];
        if !MACROS.contains(&name) {
            return None;
        }
        let after_name = &rest[colon + 1..];
        if after_name.starts_with(':') {
            // A block macro.
            return None;
        }
        let bracket = after_name.find('[')?;
        let target = &after_name[..bracket];
        if target.contains(char::is_whitespace) {
            return None;
        }
        let open = index + colon + 1 + bracket;
        let (text, to) = self.brackets(open, end)?;
        let attributes = text.unwrap_or_default();
        let text = Some(attributes.clone()).filter(|text| !text.is_empty());
        let target = target.to_owned();
        // Keyboard, button and menu macros need the `experimental` attribute.
        if matches!(name, "kbd" | "btn" | "menu") && !self.attributes.contains_key("experimental") {
            return None;
        }
        let (atom, shown) = match name {
            "xref" => {
                let shown = text.clone().unwrap_or_else(|| xref_label(&target));
                (Atom::Xref { target, text }, shown)
            }
            "link" | "mailto" => {
                let target = if name == "mailto" {
                    format!("mailto:{target}")
                } else {
                    target
                };
                let shown = text
                    .clone()
                    .unwrap_or_else(|| target.trim_start_matches("mailto:").to_owned());
                (Atom::Link { target, text }, shown)
            }
            "image" => {
                if target.is_empty() {
                    return None;
                }
                let alt = attributes
                    .split(',')
                    .map(str::trim)
                    .find_map(|attribute| attribute.strip_prefix("alt="))
                    .or_else(|| {
                        attributes
                            .split(',')
                            .next()
                            .filter(|first| !first.contains('='))
                    })
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .to_owned();
                let shown = format!("[{}]", if alt.is_empty() { &target } else { &alt });
                (Atom::Image { target, alt }, shown)
            }
            "footnote" | "footnoteref" => (
                Atom::Footnote {
                    id: Some(target).filter(|target| !target.is_empty()),
                },
                String::new(),
            ),
            "anchor" => (Atom::Anchor { id: target }, String::new()),
            "pass" => (Atom::Passthrough, attributes),
            "indexterm" => (Atom::IndexTerm, String::new()),
            "indexterm2" => (Atom::IndexTerm, attributes),
            "kbd" => (
                Atom::Macro {
                    name: name.to_owned(),
                },
                attributes
                    .split(['+', ','])
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("+"),
            ),
            "menu" => {
                let mut items = vec![target.clone()];
                items.extend(
                    attributes
                        .split('>')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_owned),
                );
                (
                    Atom::Macro {
                        name: name.to_owned(),
                    },
                    items.join(" > "),
                )
            }
            "icon" => (
                Atom::Macro {
                    name: name.to_owned(),
                },
                target,
            ),
            _ => (
                Atom::Macro {
                    name: name.to_owned(),
                },
                attributes,
            ),
        };
        Some((
            Piece::Atom {
                atom,
                range: SourceRange::new(index, to),
                shown,
            },
            to,
        ))
    }

    /// A formatting mark at `index`, with an optional `[attributes]` prefix.
    fn mark(&self, index: usize, end: usize, block_start: usize) -> Option<(Piece, usize)> {
        let (prefix, roles) = self.mark_prefix(index, end);
        let delimiter_at = index + prefix;
        for &(mark, form) in MARK_ORDER {
            if let Some((open_end, close_start, close_end)) =
                self.mark_match(index, delimiter_at, end, mark, form, block_start)
            {
                let mark = if mark == Mark::Highlight && roles {
                    Mark::Span
                } else {
                    mark
                };
                let children = self.pieces(open_end, close_start, block_start);
                return Some((
                    Piece::Mark {
                        mark,
                        open: SourceRange::new(index, open_end),
                        close: SourceRange::new(close_start, close_end),
                        children,
                    },
                    close_end,
                ));
            }
        }
        None
    }

    /// The length of a `[...]` prefix at `index` that a delimiter follows, and whether it
    /// names a role or id.
    fn mark_prefix(&self, index: usize, end: usize) -> (usize, bool) {
        let rest = &self.source[index..end];
        if !rest.starts_with('[') || rest.starts_with("[[") {
            return (0, false);
        }
        let Some(close) = rest.find(']') else {
            return (0, false);
        };
        let inner = &rest[1..close];
        if inner.is_empty() || inner.contains(['[', '\n']) {
            return (0, false);
        }
        let follows = rest[close + 1..].chars().next();
        if !follows.is_some_and(|c| matches!(c, '*' | '_' | '`' | '#' | '^' | '~')) {
            return (0, false);
        }
        (close + 1, true)
    }

    /// Where a mark's content and closing delimiter are, if `mark` in `form` opens at `at`.
    fn mark_match(
        &self,
        start: usize,
        at: usize,
        end: usize,
        mark: Mark,
        form: Form,
        block_start: usize,
    ) -> Option<(usize, usize, usize)> {
        let character = mark.delimiter();
        let delimiter: String = match form {
            Form::Unconstrained => [character, character].iter().collect(),
            Form::Constrained | Form::Tight => character.to_string(),
        };
        if !self.starts(at, end, &delimiter) {
            return None;
        }
        let open_end = at + delimiter.len();
        let first = self.char_at(open_end).filter(|_| open_end < end)?;
        let code = mark == Mark::Code;
        match form {
            Form::Constrained => {
                let before = self.char_before(start, block_start);
                let blocked = before.is_some_and(|c| {
                    is_word(c)
                        || matches!(c, ';' | ':' | '}')
                        || (code && matches!(c, '"' | '\'' | '`'))
                });
                if blocked || first.is_whitespace() {
                    return None;
                }
            }
            Form::Tight => {
                if first.is_whitespace() {
                    return None;
                }
            }
            Form::Unconstrained => {}
        }
        // The first closing delimiter that qualifies. Escapes and passthroughs are read first,
        // as Asciidoctor extracts them before formatting; macros come after it.
        let mut index = open_end;
        while index < end {
            if let Some((_, next)) = self.escape(index, end, block_start) {
                index = next;
                continue;
            }
            if let Some(found) = self.closes(index, end, open_end, &delimiter, form, code) {
                return Some(found);
            }
            if self.char_at(index) == Some('+') {
                if let Some((_, next)) = self.passthrough(index, end, block_start) {
                    index = next;
                    continue;
                }
            }
            let character = self.char_at(index)?;
            if form == Form::Tight && character.is_whitespace() {
                return None;
            }
            index += character.len_utf8();
        }
        None
    }

    fn closes(
        &self,
        index: usize,
        end: usize,
        open_end: usize,
        delimiter: &str,
        form: Form,
        code: bool,
    ) -> Option<(usize, usize, usize)> {
        if !self.starts(index, end, delimiter) || index == open_end {
            return None;
        }
        let close_end = index + delimiter.len();
        let before = self.source[..index].chars().next_back()?;
        let after = self.char_at(close_end).filter(|_| close_end <= end);
        match form {
            Form::Constrained => {
                let after_blocked = after.is_some_and(|c| {
                    close_end < end && (is_word(c) || (code && matches!(c, '"' | '\'' | '`')))
                });
                if before.is_whitespace() || after_blocked {
                    return None;
                }
            }
            Form::Tight => {
                if before.is_whitespace() {
                    return None;
                }
            }
            Form::Unconstrained => {}
        }
        Some((open_end, index, close_end))
    }
}

/// The shown text of `pieces`.
pub(crate) fn push_plain(source: &str, pieces: &[Piece], out: &mut String) {
    for piece in pieces {
        match piece {
            Piece::Text { range } => out.push_str(&source[range.start..range.end]),
            Piece::Mark { children, .. } => push_plain(source, children, out),
            Piece::Atom { shown, .. } => out.push_str(shown),
        }
    }
}

/// What an xref without text shows: the target's name, as the preview does when it cannot
/// find a title.
pub(crate) fn xref_label(target: &str) -> String {
    target
        .rsplit(['/', ':', '$', '#'])
        .next()
        .unwrap_or(target)
        .trim_end_matches(".adoc")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str) -> Vec<Piece> {
        let attributes = BTreeMap::from([
            ("product".to_owned(), "Foundry".to_owned()),
            ("experimental".to_owned(), String::new()),
        ]);
        Scanner::new(text, &attributes, false).scan(SourceRange::new(0, text.len()))
    }

    fn shown(text: &str) -> String {
        let mut out = String::new();
        push_plain(text, &scan(text), &mut out);
        out
    }

    /// The pieces as `kind:source` strings, nested marks in brackets.
    fn outline(text: &str) -> String {
        fn visit(text: &str, pieces: &[Piece], out: &mut Vec<String>) {
            for piece in pieces {
                match piece {
                    Piece::Text { range } => {
                        out.push(format!("t:{}", &text[range.start..range.end]))
                    }
                    Piece::Atom { range, atom, .. } => {
                        let kind = format!("{atom:?}");
                        let kind = kind.split([' ', '{']).next().unwrap_or("").to_owned();
                        out.push(format!("{kind}:{}", &text[range.start..range.end]));
                    }
                    Piece::Mark { mark, children, .. } => {
                        let mut inner = Vec::new();
                        visit(text, children, &mut inner);
                        out.push(format!("{mark:?}[{}]", inner.join(" ")));
                    }
                }
            }
        }
        let mut out = Vec::new();
        visit(text, &scan(text), &mut out);
        out.join(" ")
    }

    fn tiles(text: &str) {
        fn visit(pieces: &[Piece], start: usize, end: usize) {
            let mut at = start;
            for piece in pieces {
                let range = piece.range();
                assert_eq!(range.start, at, "pieces must be contiguous");
                if let Piece::Mark {
                    open,
                    close,
                    children,
                    ..
                } = piece
                {
                    visit(children, open.end, close.start);
                }
                at = range.end;
            }
            assert_eq!(at, end, "pieces must reach the end");
        }
        visit(&scan(text), 0, text.len());
    }

    #[test]
    fn reads_constrained_and_unconstrained_marks() {
        assert_eq!(outline("a *b* c"), "t:a  Strong[t:b] t: c");
        assert_eq!(outline("**str**ong"), "Strong[t:str] t:ong");
        assert_eq!(
            outline("_it_ and `co` and #hi#"),
            "Emphasis[t:it] t: and  Code[t:co] t: and  Highlight[t:hi]"
        );
        assert_eq!(
            outline("x^2^ H~2~O"),
            "t:x Superscript[t:2] t: H Subscript[t:2] t:O"
        );
        assert_eq!(outline("[.underline]#role#"), "Span[t:role]");
        assert_eq!(outline("*bold _both_*"), "Strong[t:bold  Emphasis[t:both]]");
        assert_eq!(
            outline("2 * 3 * 4"),
            "t:2 * 3 * 4",
            "spaced stars are not marks"
        );
        assert_eq!(outline("snake_case_name"), "t:snake_case_name");
        assert_eq!(outline("*unclosed"), "t:*unclosed");
    }

    #[test]
    fn reads_atoms() {
        assert_eq!(
            outline("See xref:other.adoc#d[the page], <<lists>> and https://example.com[a link]."),
            "t:See  Xref:xref:other.adoc#d[the page] t:,  Xref:<<lists>> t: and  Link:https://example.com[a link] t:."
        );
        assert_eq!(
            outline("Visit https://example.com."),
            "t:Visit  Link:https://example.com t:."
        );
        assert_eq!(
            outline("note.footnote:[The note.]"),
            "t:note. Footnote:footnote:[The note.]"
        );
        assert_eq!(
            outline("about {product} and {unknown}"),
            "t:about  Attribute:{product} t: and {unknown}"
        );
        assert_eq!(
            outline("(C) it's"),
            "Replacement:(C) t: it Replacement:' t:s"
        );
        assert_eq!(outline("line +\nnext"), "t:line LineBreak: + t:\nnext");
        assert_eq!(
            outline("kbd:[Ctrl+S] and +*raw*+"),
            "Macro:kbd:[Ctrl+S] t: and  Passthrough:+*raw*+"
        );
        assert_eq!(outline("\\*not bold*"), "Escape:\\* t:not bold*");
        assert_eq!(outline("[[id]]Text"), "Anchor:[[id]] t:Text");
        assert_eq!(shown("about {product}, (C)"), "about Foundry, \u{a9}");

        let text = "kbd:[Ctrl+S]";
        let plain =
            Scanner::new(text, &BTreeMap::new(), false).scan(SourceRange::new(0, text.len()));
        assert!(
            matches!(plain.as_slice(), [Piece::Text { .. }]),
            "keyboard macros need `experimental`"
        );
    }

    #[test]
    fn pieces_tile_the_source() {
        for text in [
            "A paragraph with *bold*, _italic_, `mono`, #marked#, [.underline]#role#, ^sup^ and ~sub~ text about {product}.\nSee xref:other.adoc#details[the other page], <<lists>>, https://example.com[a link] and a footnote.footnote:[The note.]",
            "*a _b* c_",
            "``co``de and **",
            "[x] unclosed [ and { and <<",
            "日本語 *強調* text",
            "\"`quoted`\" and '`single`'",
        ] {
            tiles(text);
        }
    }

    #[test]
    fn half_typed_input_does_not_panic() {
        for text in [
            "*",
            "**",
            "`",
            "_x",
            "[",
            "[.",
            "[.r]",
            "[.r]#",
            "<<",
            "<<a",
            "xref:",
            "xref:a[",
            "https://",
            "https://x[",
            "{",
            "{a",
            "+",
            "++",
            "+++a",
            "pass:[",
            "footnote:[a",
            "\\",
            "a \\",
            " +",
            "(((",
            "((a",
            "\"`",
            "'`x",
            "image:[]",
            "kbd:[",
            "é*",
            "*é",
            "**é",
            "^",
            "~é~",
            "#",
        ] {
            tiles(text);
        }
    }
}
