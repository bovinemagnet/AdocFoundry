//! A fragment as AsciiDoc: a block taken whole as its own source where that can be copied as it
//! is, and anything else written from what it holds, marks closed where the selection cut them.

use adoc_dom::{ListKind, SourceSpan};

use super::{content_of, plain, Fragment, FragmentBlock, FragmentCell, FragmentItem, FragmentKind};
use crate::{ViewFootnote, ViewInline};

type Source<'a> = &'a dyn Fn(&SourceSpan) -> Option<String>;

pub(super) fn write(fragment: &Fragment, source: Source<'_>) -> String {
    let mut fragment = fragment.clone();
    let notes = fragment.footnotes.clone();
    let references = fragment.references();
    let mut defined = Vec::new();
    super::each_inlines(&mut fragment.blocks, &mut |content| {
        define_footnotes(content, &notes, &references, &mut defined);
    });
    let writer = Writer {
        source,
        lists: Depth::default(),
    };
    writer.blocks(&fragment.blocks)
}

/// Writes each reference to a footnote with the note itself, which is defined where it is first
/// referred to, and by its name after that.
fn define_footnotes(
    content: &mut [ViewInline],
    notes: &[ViewFootnote],
    references: &[u32],
    defined: &mut Vec<u32>,
) {
    for inline in content {
        if let ViewInline::FootnoteRef { number, .. } = inline {
            let number = *number;
            let Some(note) = notes.iter().find(|note| note.number == number) else {
                continue;
            };
            let repeated = references.iter().filter(|one| **one == number).count() > 1;
            let name = if repeated {
                format!("note-{number}")
            } else {
                String::new()
            };
            let text = if defined.contains(&number) {
                String::new()
            } else {
                inlines(&note.content, Place::Line).replace(']', "\\]")
            };
            defined.push(number);
            *inline = ViewInline::Raw(format!("{NOTE}{name}[{text}]"));
        } else if let Some(inside) = super::content_of_mut(inline) {
            define_footnotes(inside, notes, references, defined);
        }
    }
}

/// Begins raw text that is a footnote's macro, after its name.
const NOTE: &str = "\u{0}footnote:";

/// How many lists of each kind are open around a list, which says how its items are marked.
#[derive(Clone, Copy, Default)]
struct Depth {
    unordered: usize,
    ordered: usize,
    descriptions: usize,
}

struct Writer<'a> {
    source: Source<'a>,
    lists: Depth,
}

impl Writer<'_> {
    fn blocks(&self, blocks: &[FragmentBlock]) -> String {
        let mut out = String::new();
        let mut before: Option<&FragmentKind> = None;
        for block in blocks {
            let text = self.block(block);
            if text.is_empty() {
                continue;
            }
            if let Some(before) = before {
                // Two lists side by side would be read as one.
                let lists = is_list(before) && is_list(&block.kind);
                out.push_str(if lists { "\n\n//-\n\n" } else { "\n\n" });
            }
            out.push_str(&text);
            before = Some(&block.kind);
        }
        out
    }

    /// Blocks inside a delimited block or a cell, where lists begin afresh.
    fn inside(&self, blocks: &[FragmentBlock]) -> String {
        Writer {
            source: self.source,
            lists: Depth::default(),
        }
        .blocks(blocks)
    }

    fn block(&self, block: &FragmentBlock) -> String {
        // Inside a list a block's own lines may be marked for another depth.
        let own = block
            .whole
            .as_ref()
            .filter(|_| !is_list(&block.kind) || self.lists.is_empty())
            .and_then(|span| (self.source)(span));
        if let Some(text) = own {
            return text.trim_end().to_owned();
        }
        let body = self.body(&block.kind);
        if block.title.is_empty() {
            return body;
        }
        let title = inlines(&block.title, Place::Line);
        // A title that starts with a dot or a space would not be read as one.
        let title = if title.starts_with(['.', ' ']) {
            format!("{{empty}}{title}")
        } else {
            title
        };
        if body.is_empty() {
            title
        } else {
            format!(".{title}\n{body}")
        }
    }

    fn body(&self, kind: &FragmentKind) -> String {
        match kind {
            FragmentKind::Heading {
                level,
                discrete,
                content,
                style,
                ..
            } => {
                let marks = "=".repeat(usize::from(*level) + 1);
                let heading = format!("{marks} {}", inlines(content, Place::Line));
                match style.as_deref().filter(|_| !*discrete) {
                    _ if *discrete => format!("[discrete]\n{heading}"),
                    Some(style) => format!("[{style}]\n{heading}"),
                    None => heading,
                }
            }
            FragmentKind::Paragraph(content) => paragraph(content),
            FragmentKind::List {
                kind,
                items,
                reversed,
            } => self.list(*kind, items, *reversed),
            FragmentKind::Callouts(items) => items
                .iter()
                .map(|item| self.item(item, &format!("<{}>", item.number), "", self.lists))
                .collect::<Vec<_>>()
                .join("\n"),
            FragmentKind::Descriptions(items) => {
                let mark = match self.lists.descriptions {
                    0 => "::",
                    1 => ":::",
                    _ => "::::",
                };
                let inside = Depth {
                    descriptions: self.lists.descriptions + 1,
                    ..self.lists
                };
                // A blank line ends a list inside one before the next term.
                items
                    .iter()
                    .map(|item| {
                        let term = inlines(&item.term, Place::Line);
                        let term = if term.is_empty() {
                            "{empty}".to_owned()
                        } else {
                            term
                        };
                        let content = inlines(&item.content, Place::Flow);
                        let mut text = format!("{term}{mark} {content}").trim_end().to_owned();
                        self.attach(&item.blocks, inside, &mut text);
                        text
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            }
            FragmentKind::Admonition { kind, blocks, .. } => {
                let name = kind.label().to_uppercase();
                match blocks.as_slice() {
                    [FragmentBlock {
                        kind: FragmentKind::Paragraph(content),
                        title,
                        ..
                    }] if title.is_empty() => {
                        format!("{name}: {}", inlines(content, Place::Flow))
                    }
                    _ => format!("[{name}]\n{}", delimited(&self.inside(blocks), '=')),
                }
            }
            FragmentKind::Code {
                language,
                text,
                callouts,
            } => {
                let language = language
                    .as_ref()
                    .map_or_else(String::new, |language| format!(",{language}"));
                format!(
                    "[source{language}]\n{}",
                    delimited(&directives(&plain::code(text, callouts)), '-')
                )
            }
            FragmentKind::Literal(text) => delimited(&directives(text), '.'),
            FragmentKind::Passthrough(text) => delimited(&directives(text), '+'),
            FragmentKind::Stem { latex, source } => {
                let name = if *latex { "latexmath" } else { "asciimath" };
                format!("[{name}]\n{}", delimited(source.trim(), '+'))
            }
            FragmentKind::Quote {
                attribution,
                citation,
                blocks,
            } => format!(
                "{}\n{}",
                signed("quote", attribution, citation),
                delimited(&self.inside(blocks), '_')
            ),
            FragmentKind::Verse {
                attribution,
                citation,
                text,
            } => format!(
                "{}\n{}",
                signed("verse", attribution, citation),
                delimited(
                    &text.split('\n').map(escape).collect::<Vec<_>>().join("\n"),
                    '_'
                )
            ),
            FragmentKind::Sidebar(blocks) => delimited(&self.inside(blocks), '*'),
            FragmentKind::Example(blocks) => delimited(&self.inside(blocks), '='),
            FragmentKind::Open(blocks) => {
                let inside = self.inside(blocks);
                // An open block cannot hold another.
                if inside.lines().any(|line| line.trim_end() == "--") {
                    inside
                } else {
                    format!("--\n{inside}\n--")
                }
            }
            FragmentKind::Table { header, rows } => self.table(*header, rows),
            FragmentKind::Diagram(diagram) => {
                diagram.as_ref().map_or_else(String::new, |(kind, source)| {
                    format!("[{kind}]\n{}", delimited(&directives(source), '.'))
                })
            }
            FragmentKind::Image(image) => {
                format!("image::{}[{}]", image.target, attribute(&image.alt))
            }
            FragmentKind::Media { kind, targets } => {
                let name = match kind {
                    adoc_dom::MediaKind::Audio => "audio",
                    adoc_dom::MediaKind::Video => "video",
                };
                targets
                    .iter()
                    .map(|target| format!("{name}::{target}[]"))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            }
            FragmentKind::Rule => "'''".to_owned(),
            FragmentKind::PageBreak => "<<<".to_owned(),
            FragmentKind::Text(text) if text.is_empty() => String::new(),
            FragmentKind::Text(text) => delimited(&directives(text), '.'),
        }
    }

    fn list(&self, kind: ListKind, items: &[FragmentItem], reversed: bool) -> String {
        let ordered = kind == ListKind::Ordered;
        let (mark, inside) = if ordered {
            (
                ".".repeat(self.lists.ordered + 1),
                Depth {
                    ordered: self.lists.ordered + 1,
                    ..self.lists
                },
            )
        } else {
            (
                "*".repeat(self.lists.unordered + 1),
                Depth {
                    unordered: self.lists.unordered + 1,
                    ..self.lists
                },
            )
        };
        let mut lines = Vec::new();
        if ordered {
            let first = items.first().map_or(1, |item| item.number);
            match (reversed, first) {
                (true, _) => lines.push(format!("[%reversed,start={first}]")),
                (false, 1) => {}
                (false, _) => lines.push(format!("[start={first}]")),
            }
        }
        lines.extend(items.iter().map(|item| {
            let check = match item.checked {
                Some(true) => "[x] ",
                Some(false) => "[ ] ",
                None => "",
            };
            self.item(item, &mark, check, inside)
        }));
        lines.join("\n")
    }

    fn item(&self, item: &FragmentItem, mark: &str, check: &str, inside: Depth) -> String {
        let content = inlines(&item.content, Place::Flow);
        let content = if content.is_empty() {
            "{empty}".to_owned()
        } else {
            content
        };
        let mut text = format!("{mark} {check}{content}");
        self.attach(&item.blocks, inside, &mut text);
        text
    }

    /// Adds the blocks of a list item: a list follows it, anything else is joined to it.
    fn attach(&self, blocks: &[FragmentBlock], inside: Depth, text: &mut String) {
        let writer = Writer {
            source: self.source,
            lists: inside,
        };
        for block in blocks {
            let written = writer.block(block);
            if written.is_empty() {
                continue;
            }
            text.push_str(if is_list(&block.kind) && block.title.is_empty() {
                "\n"
            } else {
                "\n+\n"
            });
            text.push_str(&written);
        }
    }

    fn table(&self, header: bool, rows: &[Vec<FragmentCell>]) -> String {
        let columns = rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.colspan).sum::<usize>())
            .max()
            .unwrap_or(1);
        let header = if header { "%header," } else { "" };
        let mut lines = vec![format!("[{header}cols=\"{columns}*\"]"), "|===".to_owned()];
        for row in rows {
            for cell in row {
                lines.push(self.cell(cell));
            }
            lines.push(String::new());
        }
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.push("|===".to_owned());
        lines.join("\n")
    }

    fn cell(&self, cell: &FragmentCell) -> String {
        let span = match (cell.colspan, cell.rowspan) {
            (1, 1) => String::new(),
            (columns, 1) => format!("{columns}+"),
            (1, rows) => format!(".{rows}+"),
            (columns, rows) => format!("{columns}.{rows}+"),
        };
        match cell.blocks.as_slice() {
            [] => format!("{span}|"),
            [FragmentBlock {
                kind: FragmentKind::Paragraph(content),
                title,
                ..
            }] if title.is_empty() => {
                format!(
                    "{span}|{}",
                    inlines(content, Place::Flow).replace('|', "\\|")
                )
            }
            blocks => format!("{span}a|{}", self.inside(blocks).replace('|', "\\|")),
        }
    }
}

impl Depth {
    fn is_empty(self) -> bool {
        self.unordered == 0 && self.ordered == 0 && self.descriptions == 0
    }
}

fn is_list(kind: &FragmentKind) -> bool {
    matches!(
        kind,
        FragmentKind::List { .. } | FragmentKind::Descriptions(_) | FragmentKind::Callouts(_)
    )
}

/// `text` between delimiter lines longer than any line of the same character in it.
fn delimited(text: &str, character: char) -> String {
    let longest = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && line.chars().all(|other| other == character))
        .map(str::len)
        .max()
        .unwrap_or(0);
    let delimiter = character.to_string().repeat(longest.max(3) + 1);
    format!("{delimiter}\n{text}\n{delimiter}")
}

/// Verbatim lines that would be read as directives, escaped.
fn directives(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let directive = ["include::", "ifdef::", "ifndef::", "ifeval::", "endif::"]
                .iter()
                .any(|name| line.trim_start_matches('\\').starts_with(name));
            if directive {
                format!("\\{line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn signed(style: &str, attribution: &Option<String>, citation: &Option<String>) -> String {
    match (attribution, citation) {
        (None, None) => format!("[{style}]"),
        (Some(attribution), None) => format!("[{style},{}]", attribute(attribution)),
        (attribution, Some(citation)) => format!(
            "[{style},{},{}]",
            attribute(attribution.as_deref().unwrap_or("")),
            attribute(citation)
        ),
    }
}

/// A value in an attribute list, quoted where it holds what would end it.
fn attribute(value: &str) -> String {
    let value = value.replace('\n', " ");
    if value.contains([',', '"', ']', '=']) {
        format!("\"{}\"", value.replace('"', "\\\"").replace(']', "\\]"))
    } else {
        value
    }
}

/// Where inline content is written: on lines of its own, or on one line with what it belongs to.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Place {
    Flow,
    Line,
}

/// A paragraph, kept from being read as another block.
fn paragraph(content: &[ViewInline]) -> String {
    let text = inlines(content, Place::Flow);
    if starts_block(&text) {
        // Written again, as a mark beside the guard is doubled.
        format!("{{empty}}{}", inlines_after(content, Place::Flow, '}'))
    } else {
        text
    }
}

fn starts_block(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("");
    let word = first.split(' ').next().unwrap_or("");
    let marker = !word.is_empty()
        && first.len() > word.len()
        && (word.chars().all(|c| c == '*')
            || word.chars().all(|c| c == '.')
            || word.chars().all(|c| c == '=')
            || word.chars().all(|c| c == '#')
            || word == "-"
            || word.starts_with('<') && word.ends_with('>')
            || word
                .strip_suffix('.')
                .is_some_and(|number| number.chars().all(|c| c.is_ascii_alphanumeric())));
    let label = ["NOTE:", "TIP:", "IMPORTANT:", "WARNING:", "CAUTION:"]
        .iter()
        .any(|label| first.starts_with(label));
    marker
        || label
        || first.starts_with([' ', '\t', '.', '[', '|'])
        || first.starts_with("//")
        || first.starts_with("'''")
        || first.starts_with("<<<")
        || first.contains(":: ")
        || first.contains(";; ")
        || first.ends_with("::")
        || first.ends_with(";;")
        || (first.contains("::") && first.ends_with(']'))
        || first.starts_with(':')
        || (first.len() > 1
            && first
                .chars()
                .all(|c| matches!(c, '-' | '=' | '*' | '_' | '+' | '.' | '/' | '`')))
}

fn inlines(content: &[ViewInline], place: Place) -> String {
    inlines_after(content, place, ' ')
}

/// `content` written after the character `after`.
fn inlines_after(content: &[ViewInline], place: Place, after: char) -> String {
    let mut out = String::new();
    for (index, part) in content.iter().enumerate() {
        let next = content.get(index + 1).map(first_character).unwrap_or(' ');
        let before = out.chars().next_back().unwrap_or(after);
        let written = inline(part, before, next, place);
        if out.ends_with(" +\n") {
            out.push_str(written.trim_start());
        } else {
            out.push_str(&written);
        }
    }
    out
}

/// The first character `inline` is written with, as far as its neighbour's marks need to know.
fn first_character(inline: &ViewInline) -> char {
    super::shown_text(std::slice::from_ref(inline))
        .chars()
        .next()
        .unwrap_or(' ')
}

fn inline(inline: &ViewInline, before: char, next: char, place: Place) -> String {
    let mark =
        |content: &[ViewInline], mark: char| marked(&inlines(content, place), mark, before, next);
    match inline {
        ViewInline::Text(text) => match place {
            Place::Flow => escape(text),
            Place::Line => escape(&text.replace('\n', " ")),
        },
        ViewInline::Raw(text) if text.starts_with(NOTE) => {
            format!("footnote:{}", &text[NOTE.len()..])
        }
        ViewInline::Raw(text) => format!("pass:[{}]", text.replace(']', "\\]")),
        ViewInline::Strong(content) => mark(content, '*'),
        ViewInline::Emphasis(content) => mark(content, '_'),
        ViewInline::Code(content) => match content.as_slice() {
            // Text that would be read as more is kept as it is, whole.
            [ViewInline::Text(text)]
                if text.trim() == text && text.split_whitespace().any(is_markup) =>
            {
                let text = text.replace('\n', " ");
                if text.contains('+') {
                    marked(
                        &format!("pass:c[{}]", text.replace(']', "\\]")),
                        '`',
                        before,
                        next,
                    )
                } else {
                    marked(&format!("+{text}+"), '`', before, next)
                }
            }
            _ => mark(content, '`'),
        },
        ViewInline::Mark(content) => mark(content, '#'),
        ViewInline::Superscript(content) => format!("^{}^", inlines(content, place)),
        ViewInline::Subscript(content) => format!("~{}~", inlines(content, place)),
        ViewInline::Span { roles, content } if !roles.is_empty() => {
            format!("[.{}]##{}##", roles.join("."), inlines(content, place))
        }
        ViewInline::Link { url, content } => {
            if plain::inlines(content) == *url && url.contains("://") {
                url.clone()
            } else {
                let text = label(content, place);
                if url.contains("://") {
                    format!("{url}[{text}]")
                } else if url.contains(['&', ' ', '?', ',']) {
                    format!("link:++{url}++[{text}]")
                } else {
                    format!("link:{url}[{text}]")
                }
            }
        }
        ViewInline::Xref {
            target, content, ..
        } => format!("xref:{target}[{}]", label(content, place)),
        ViewInline::Anchor(id) => format!("[[{id}]]"),
        ViewInline::Image(image) => {
            format!("image:{}[{}]", image.target, attribute(&image.alt))
        }
        ViewInline::Icon(name) => format!("icon:{name}[]"),
        ViewInline::LineBreak => " +\n".to_owned(),
        ViewInline::Keyboard(keys) => format!("kbd:[{}]", keys.join("+").replace(']', "\\]")),
        ViewInline::Button(label) => format!("btn:[{}]", label.replace(']', "\\]")),
        ViewInline::Menu(items) => match items.split_first() {
            Some((menu, items)) => format!("menu:{menu}[{}]", items.join(" > ")),
            None => String::new(),
        },
        ViewInline::Stem { latex, source, .. } => {
            let name = if *latex { "latexmath" } else { "asciimath" };
            format!("{name}:[{}]", source.trim().replace(']', "\\]"))
        }
        ViewInline::IndexTerm { terms, content } => {
            if content.is_empty() {
                format!("(((\u{200b}{})))", terms.join(", ")).replace('\u{200b}', "")
            } else {
                format!("(({}))", inlines(content, place))
            }
        }
        other => match content_of(other) {
            Some(content) => inlines(content, place),
            None => escape(&plain::inlines(std::slice::from_ref(other))),
        },
    }
}

/// The text of a link, inside its brackets.
fn label(content: &[ViewInline], place: Place) -> String {
    let text = inlines(content, place).replace(']', "\\]");
    if text.contains('=') {
        format!("\"{}\"", text.replace('"', "\\\""))
    } else {
        text
    }
}

/// `text` between marks, doubled where it touches a word.
fn marked(text: &str, mark: char, before: char, next: char) -> String {
    let word = |character: char| character.is_alphanumeric() || character == '_';
    let touching = word(before)
        || word(next)
        || before == mark
        || next == mark
        || text.starts_with(char::is_whitespace)
        || text.ends_with(char::is_whitespace)
        || matches!(before, ':' | ';' | '}');
    let marks = mark.to_string().repeat(if touching { 2 } else { 1 });
    format!("{marks}{text}{marks}")
}

/// Text with every word that AsciiDoc would read as more than text kept as it is.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if is_markup(word) {
            if word.contains('+') {
                out.push_str(&format!("pass:c[{}]", word.replace(']', "\\]")));
            } else {
                out.push_str(&format!("++{word}++"));
            }
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for character in text.chars() {
        if character.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(character);
        } else {
            word.push(character);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn is_markup(word: &str) -> bool {
    let formats = word.contains(['*', '`', '#', '^', '~', '{', '}', '<', '>', '\\'])
        || word.starts_with('_')
        || word.ends_with('_')
        || word.contains("__");
    let replaced = [
        "--", "...", "->", "=>", "(C)", "(R)", "(TM)", "((", "[[", "++",
    ]
    .iter()
    .any(|part| word.contains(part));
    // A macro, a link or a hard break.
    let macros = word.contains(":[")
        || word.contains("://")
        || (word.contains(':') && word.contains('['))
        || word == "+"
        || (word.contains('@') && word.contains('.'));
    let apostrophe = word.contains('\'');
    formats || replaced || macros || apostrophe
}
