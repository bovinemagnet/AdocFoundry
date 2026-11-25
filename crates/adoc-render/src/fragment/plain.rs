//! A fragment as plain text: spaces, tabs, new lines and list markers only.

use adoc_dom::ListKind;

use super::{content_of, Fragment, FragmentBlock, FragmentCell, FragmentItem, FragmentKind};
use crate::ViewInline;

pub(super) fn write(fragment: &Fragment) -> String {
    let mut out = blocks(&fragment.blocks);
    out.truncate(out.trim_end_matches('\n').len());
    out
}

/// Blocks, a blank line between each.
fn blocks(blocks: &[FragmentBlock]) -> String {
    blocks
        .iter()
        .map(block)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Inline content as text: a line of the page's source is not a line of its text.
pub(super) fn inlines(content: &[ViewInline]) -> String {
    fn push(inline: &ViewInline, out: &mut String) {
        if let Some(content) = content_of(inline) {
            content.iter().for_each(|inline| push(inline, out));
            return;
        }
        match inline {
            ViewInline::Text(text) | ViewInline::Raw(text) => {
                // What follows a hard break starts its own line.
                let text = if out.ends_with('\n') {
                    text.trim_start()
                } else {
                    text
                };
                out.push_str(&text.replace('\n', " "));
            }
            ViewInline::LineBreak => {
                out.truncate(out.trim_end_matches(' ').len());
                out.push('\n');
            }
            ViewInline::Image(image) => out.push_str(&image.alt),
            ViewInline::Stem { source, .. } => out.push_str(source.trim()),
            other => out.push_str(&super::shown_text(std::slice::from_ref(other))),
        }
    }
    let mut out = String::new();
    content.iter().for_each(|inline| push(inline, &mut out));
    out
}

/// `text` with every line but the first indented.
fn hanging(text: &str, indent: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}

fn block(block: &FragmentBlock) -> String {
    let mut title = inlines(&block.title);
    if let Some(caption) = &block.caption {
        title = format!("{caption} {title}");
    }
    let body = body(&block.kind);
    match (title.is_empty(), body.is_empty()) {
        (true, _) => body,
        (false, true) => title,
        (false, false) => match block.kind {
            FragmentKind::Image(_) | FragmentKind::Diagram(_) => format!("{body}\n{title}"),
            _ => format!("{title}\n{body}"),
        },
    }
}

fn body(kind: &FragmentKind) -> String {
    match kind {
        FragmentKind::Heading {
            content, number, ..
        } => match number {
            Some(number) => format!("{number} {}", inlines(content)),
            None => inlines(content),
        },
        FragmentKind::Paragraph(content) => inlines(content),
        FragmentKind::List { kind, items, .. } => items
            .iter()
            .map(|item| {
                let marker = match (kind, item.checked) {
                    (_, Some(true)) => format!("{} [x]", item.bullet),
                    (_, Some(false)) => format!("{} [ ]", item.bullet),
                    (ListKind::Ordered, None) => format!("{}.", item.number),
                    _ => item.bullet.to_string(),
                };
                item_text(item, &marker)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        FragmentKind::Callouts(items) => items
            .iter()
            .map(|item| item_text(item, &format!("<{}>", item.number)))
            .collect::<Vec<_>>()
            .join("\n"),
        FragmentKind::Descriptions(items) => items
            .iter()
            .map(|item| {
                let mut lines = Vec::new();
                if !item.term.is_empty() {
                    lines.push(inlines(&item.term));
                }
                if !item.content.is_empty() {
                    lines.push(format!("  {}", hanging(&inlines(&item.content), "  ")));
                }
                let inside = blocks(&item.blocks);
                if !inside.is_empty() {
                    lines.push(format!("  {}", hanging(&inside, "  ")));
                }
                lines.join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        FragmentKind::Admonition {
            kind,
            label,
            blocks: inside,
        } => {
            let inside = blocks(inside);
            if *label {
                format!("{}\n{inside}", kind.label())
            } else {
                inside
            }
        }
        FragmentKind::Code { text, callouts, .. } => code(text, callouts),
        FragmentKind::Quote {
            attribution,
            citation,
            blocks: inside,
        } => signed(blocks(inside), attribution, citation),
        FragmentKind::Verse {
            attribution,
            citation,
            text,
        } => signed(text.clone(), attribution, citation),
        FragmentKind::Sidebar(inside)
        | FragmentKind::Example(inside)
        | FragmentKind::Open(inside) => blocks(inside),
        FragmentKind::Table { rows, .. } => rows
            .iter()
            .map(|cells| row(cells))
            .collect::<Vec<_>>()
            .join("\n"),
        FragmentKind::Image(image) => image.alt.clone(),
        FragmentKind::Diagram(diagram) => diagram
            .as_ref()
            .map_or_else(String::new, |(_, source)| source.clone()),
        FragmentKind::Media { targets, .. } => targets.join(", "),
        FragmentKind::Stem { source, .. } => source.trim().to_owned(),
        FragmentKind::Literal(text)
        | FragmentKind::Passthrough(text)
        | FragmentKind::Text(text) => text.clone(),
        FragmentKind::Rule | FragmentKind::PageBreak => String::new(),
    }
}

fn item_text(item: &FragmentItem, marker: &str) -> String {
    let mut text = inlines(&item.content);
    if item.marked {
        text = format!("{marker} {}", hanging(&text, "  "));
        text.truncate(text.trim_end_matches(' ').len());
    }
    let inside = blocks(&item.blocks);
    match (text.is_empty(), inside.is_empty()) {
        (_, true) => text,
        (true, false) => format!("  {}", hanging(&inside, "  ")),
        (false, false) => format!("{text}\n  {}", hanging(&inside, "  ")),
    }
}

/// Code with its callouts after their lines, as `<1>`.
pub(super) fn code(text: &str, callouts: &[(usize, usize)]) -> String {
    text.split('\n')
        .enumerate()
        .map(|(number, line)| {
            let mut taken = callouts
                .iter()
                .filter(|(at, _)| *at == number)
                .map(|(_, callout)| format!("<{callout}>"))
                .peekable();
            if taken.peek().is_none() {
                return line.to_owned();
            }
            let mut line = super::without_callout(line).to_owned();
            for callout in taken {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&callout);
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn signed(text: String, attribution: &Option<String>, citation: &Option<String>) -> String {
    match (attribution, citation) {
        (None, None) => text,
        (Some(one), None) | (None, Some(one)) => format!("{text}\n— {one}"),
        (Some(attribution), Some(citation)) => format!("{text}\n— {attribution}, {citation}"),
    }
}

/// A table's row: its cells separated by tabs, from the first taken to the last.
fn row(cells: &[FragmentCell]) -> String {
    let taken = |cell: &FragmentCell| !cell.blocks.is_empty();
    let first = cells.iter().position(taken).unwrap_or(0);
    let last = cells.iter().rposition(taken).unwrap_or(0);
    cells[first..=last.min(cells.len().saturating_sub(1))]
        .iter()
        .map(|cell| {
            blocks(&cell.blocks)
                .split(['\n', '\t'])
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\t")
}
