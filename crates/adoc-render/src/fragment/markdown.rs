//! A fragment as GitHub-flavoured Markdown. What Markdown has no form for is its text.

use adoc_dom::ListKind;

use super::{content_of, plain, Fragment, FragmentBlock, FragmentCell, FragmentItem, FragmentKind};
use crate::ViewInline;

pub(super) fn write(fragment: &Fragment) -> String {
    let mut out = blocks(&fragment.blocks);
    for note in fragment.noted() {
        out.push_str(&format!(
            "\n\n[^{}]: {}",
            note.number,
            inlines(&note.content)
        ));
    }
    out
}

fn blocks(blocks: &[FragmentBlock]) -> String {
    blocks
        .iter()
        .map(block)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `text` with `first` before its first line and `rest` before every other.
fn prefixed(text: &str, first: &str, rest: &str) -> String {
    text.split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { first } else { rest };
            format!("{prefix}{line}").trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text with the characters Markdown reads as formatting escaped.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    let mut before = ' ';
    while let Some(character) = characters.next() {
        let inside_word = before.is_alphanumeric()
            && characters.peek().is_some_and(|next| next.is_alphanumeric());
        let special = match character {
            '\\' | '`' | '*' | '[' | ']' | '<' | '>' | '|' | '~' => true,
            // `snake_case` is not emphasis.
            '_' => !inside_word,
            '&' => characters
                .peek()
                .is_some_and(|next| next.is_alphanumeric() || *next == '#'),
            _ => false,
        };
        if special {
            out.push('\\');
        }
        out.push(character);
        before = character;
    }
    out
}

/// A line that Markdown would read as the start of a block, escaped.
fn escape_start(line: String) -> String {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && line[digits..].starts_with(['.', ')']) {
        return format!("{}\\{}", &line[..digits], &line[digits..]);
    }
    if line.starts_with(['#', '+', '-', '=']) {
        return format!("\\{line}");
    }
    line
}

/// `content` between `mark`s, with the spaces at its ends outside them.
fn marked(content: &str, mark: &str) -> String {
    let inside = content.trim();
    if inside.is_empty() {
        return content.to_owned();
    }
    let start = content.len() - content.trim_start().len();
    let end = content.trim_end().len();
    format!(
        "{}{mark}{inside}{mark}{}",
        &content[..start],
        &content[end..]
    )
}

/// `text` as a code span, in as many backticks as it needs.
fn code_span(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let longest = longest_run(text, '`');
    let ticks = "`".repeat(longest + 1);
    if text.starts_with('`') || text.ends_with('`') {
        format!("{ticks} {text} {ticks}")
    } else {
        format!("{ticks}{text}{ticks}")
    }
}

fn longest_run(text: &str, of: char) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        run = if character == of { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

fn inline(inline: &ViewInline) -> String {
    match inline {
        ViewInline::Text(text) | ViewInline::Raw(text) => escape(&text.replace('\n', " ")),
        ViewInline::Strong(content) => marked(&inlines(content), "**"),
        ViewInline::Emphasis(content) => marked(&inlines(content), "*"),
        ViewInline::Code(content) => code_span(&plain::inlines(content)),
        ViewInline::Superscript(content) => format!("<sup>{}</sup>", inlines(content)),
        ViewInline::Subscript(content) => format!("<sub>{}</sub>", inlines(content)),
        ViewInline::Link { url, content } => {
            let text = inlines(content);
            if plain::inlines(content) == *url {
                format!("<{url}>")
            } else {
                format!("[{text}]({})", address(url))
            }
        }
        ViewInline::Image(image) => format!(
            "![{}]({})",
            escape(&image.alt),
            address(image.url.as_deref().unwrap_or(&image.target))
        ),
        ViewInline::FootnoteRef { number, .. } => format!("[^{number}]"),
        ViewInline::LineBreak => "\\\n".to_owned(),
        ViewInline::Keyboard(keys) => keys
            .iter()
            .map(|key| format!("<kbd>{}</kbd>", escape(key)))
            .collect::<Vec<_>>()
            .join("+"),
        ViewInline::Button(label) => format!("**{}**", escape(label)),
        ViewInline::Menu(items) => format!("**{}**", escape(&items.join(" › "))),
        ViewInline::Stem { latex, source, .. } if *latex => format!("${}$", source.trim()),
        ViewInline::Stem { source, .. } => code_span(source.trim()),
        other => match content_of(other) {
            Some(content) => inlines(content),
            None => escape(&plain::inlines(std::slice::from_ref(other))),
        },
    }
}

/// An address as a link's, in angle brackets when it holds what would end it.
fn address(url: &str) -> String {
    if url.contains([' ', '(', ')']) {
        format!("<{url}>")
    } else {
        url.to_owned()
    }
}

fn inlines(content: &[ViewInline]) -> String {
    let mut out = String::new();
    for part in content {
        let part = inline(part);
        if out.ends_with("\\\n") {
            out.push_str(part.trim_start());
        } else {
            out.push_str(&part);
        }
    }
    out
}

fn paragraph(content: &[ViewInline]) -> String {
    escape_start(inlines(content))
}

fn block(block: &FragmentBlock) -> String {
    let title = inlines(&block.title);
    let body = body(&block.kind);
    if title.is_empty() {
        return body;
    }
    let title = match &block.caption {
        Some(caption) => format!("{} {title}", escape(caption)),
        None => title,
    };
    let title = marked(&title, "**");
    match block.kind {
        _ if body.is_empty() => title,
        FragmentKind::Image(_) | FragmentKind::Diagram(_) => format!("{body}\n\n{title}"),
        _ => format!("{title}\n\n{body}"),
    }
}

fn body(kind: &FragmentKind) -> String {
    match kind {
        FragmentKind::Heading {
            level,
            content,
            number,
            ..
        } => {
            let marks = "#".repeat((usize::from(*level) + 1).min(6));
            let number = number
                .as_ref()
                .map_or_else(String::new, |number| format!("{number} "));
            format!("{marks} {number}{}", inlines(content))
        }
        FragmentKind::Paragraph(content) => paragraph(content),
        FragmentKind::List { kind, items, .. } => items
            .iter()
            .map(|taken| {
                let marker = match (kind, taken.checked) {
                    (ListKind::Ordered, None) => format!("{}. ", taken.number.max(0)),
                    _ => format!("{} ", taken.bullet),
                };
                let check = match taken.checked {
                    Some(true) => "[x] ",
                    Some(false) => "[ ] ",
                    None => "",
                };
                item(taken, &marker, check)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        FragmentKind::Callouts(items) => items
            .iter()
            .map(|taken| item(taken, &format!("{}. ", taken.number.max(0)), ""))
            .collect::<Vec<_>>()
            .join("\n"),
        FragmentKind::Descriptions(items) => items
            .iter()
            .map(|item| {
                let mut parts = Vec::new();
                if !item.term.is_empty() {
                    parts.push(marked(&inlines(&item.term), "**"));
                }
                if !item.content.is_empty() {
                    parts.push(paragraph(&item.content));
                }
                let inside = blocks(&item.blocks);
                if !inside.is_empty() {
                    parts.push(inside);
                }
                parts.join("\n\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        FragmentKind::Admonition {
            kind,
            blocks: inside,
            ..
        } => prefixed(
            &format!("[!{}]\n{}", kind.label().to_uppercase(), blocks(inside)),
            "> ",
            "> ",
        ),
        FragmentKind::Code {
            language,
            text,
            callouts,
        } => fenced(
            &plain::code(text, callouts),
            language.as_deref().unwrap_or(""),
        ),
        FragmentKind::Literal(text) => fenced(text, ""),
        FragmentKind::Passthrough(text) => fenced(text, "html"),
        FragmentKind::Stem { latex, source } => {
            if *latex {
                format!("$$\n{}\n$$", source.trim())
            } else {
                fenced(source.trim(), "")
            }
        }
        FragmentKind::Quote {
            attribution,
            citation,
            blocks: inside,
        } => quoted(blocks(inside), attribution, citation),
        FragmentKind::Verse {
            attribution,
            citation,
            text,
        } => quoted(
            text.split('\n')
                .map(escape)
                .collect::<Vec<_>>()
                .join("\\\n"),
            attribution,
            citation,
        ),
        FragmentKind::Sidebar(inside)
        | FragmentKind::Example(inside)
        | FragmentKind::Open(inside) => blocks(inside),
        FragmentKind::Table { header, rows } => table(*header, rows),
        FragmentKind::Diagram(diagram) => diagram
            .as_ref()
            .map_or_else(String::new, |(kind, source)| fenced(source, kind)),
        FragmentKind::Image(image) => inline(&ViewInline::Image(image.clone())),
        FragmentKind::Media { targets, .. } => targets
            .iter()
            .map(|target| format!("[{}]({})", escape(target), address(target)))
            .collect::<Vec<_>>()
            .join("\n"),
        FragmentKind::Rule => "---".to_owned(),
        FragmentKind::PageBreak => String::new(),
        FragmentKind::Text(text) => text
            .split('\n')
            .map(|line| escape_start(escape(line)))
            .collect::<Vec<_>>()
            .join("\\\n"),
    }
}

fn item(item: &FragmentItem, marker: &str, check: &str) -> String {
    let indent = " ".repeat(marker.len());
    let mut text = prefixed(
        &format!("{check}{}", paragraph(&item.content)),
        marker,
        &indent,
    );
    for block in &item.blocks {
        // A list directly inside an item follows it; other blocks are paragraphs of it.
        let gap = match block.kind {
            FragmentKind::List { .. } if block.title.is_empty() => "\n",
            _ => "\n\n",
        };
        text.push_str(gap);
        text.push_str(&prefixed(&self::block(block), &indent, &indent));
    }
    text
}

fn fenced(text: &str, language: &str) -> String {
    let fence = "`".repeat(longest_run(text, '`').max(2) + 1);
    format!("{fence}{language}\n{text}\n{fence}")
}

fn quoted(text: String, attribution: &Option<String>, citation: &Option<String>) -> String {
    let line = match (attribution, citation) {
        (None, None) => None,
        (Some(one), None) | (None, Some(one)) => Some(format!("— {}", escape(one))),
        (Some(attribution), Some(citation)) => {
            Some(format!("— {}, {}", escape(attribution), escape(citation)))
        }
    };
    let text = match line {
        Some(line) => format!("{text}\n\n{line}"),
        None => text,
    };
    prefixed(&text, "> ", "> ")
}

/// A pipe table, which always has a header row; a cell over several columns is followed by
/// empty ones.
fn table(header: bool, rows: &[Vec<FragmentCell>]) -> String {
    let written: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .flat_map(|cell| {
                    let text = blocks(&cell.blocks)
                        .split('\n')
                        .map(|line| line.trim_end_matches('\\'))
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<_>>()
                        .join("<br>");
                    std::iter::once(text)
                        .chain(std::iter::repeat_n(String::new(), cell.colspan - 1))
                })
                .collect()
        })
        .collect();
    let columns = written.iter().map(Vec::len).max().unwrap_or(0);
    let line = |cells: &[String]| {
        let mut out = String::from("|");
        for index in 0..columns {
            let cell = cells.get(index).map_or("", String::as_str);
            out.push_str(&format!(" {cell} |"));
        }
        out
    };
    let mut lines = Vec::new();
    let mut rows = written.iter();
    let first = if header { rows.next() } else { None };
    lines.push(line(first.map_or(&[][..], Vec::as_slice)));
    lines.push(format!("|{}", " --- |".repeat(columns)));
    lines.extend(rows.map(|row| line(row)));
    lines.join("\n")
}
