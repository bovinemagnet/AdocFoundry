//! The view tree as plain text: what a reader of the page sees, without markup, for tools that
//! read pages rather than display them.

use crate::{ListKind, ViewBlock, ViewInline, ViewKind, ViewTree};

/// Renders `view` as plain text: the title, then each block separated by a blank line, lists
/// with markers, tables a row per line with cells separated by ` | `, and verbatim blocks as
/// they are.
#[must_use]
pub fn to_text(view: &ViewTree) -> String {
    let mut out = String::new();
    if let Some(title) = &view.title {
        out.push_str(&ViewInline::plain_text(title));
        out.push_str("\n\n");
    }
    for block in &view.blocks {
        block_text(block, "", &mut out);
    }
    for footnote in &view.footnotes {
        out.push_str(&format!(
            "[{}] {}\n",
            footnote.number,
            ViewInline::plain_text(&footnote.content)
        ));
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// Appends `text` as one paragraph, each line prefixed by `indent`.
fn paragraph(text: &str, indent: &str, out: &mut String) {
    for line in text.lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
}

fn block_text(block: &ViewBlock, indent: &str, out: &mut String) {
    if !block.title.is_empty() {
        let title = ViewInline::plain_text(&block.title);
        let title = match &block.caption {
            Some(caption) => format!("{caption} {title}"),
            None => title,
        };
        paragraph(&title, indent, out);
    }
    match &block.kind {
        ViewKind::Section {
            number,
            title,
            blocks,
            ..
        } => {
            let title = ViewInline::plain_text(title);
            let title = match number {
                Some(number) => format!("{number} {title}"),
                None => title,
            };
            paragraph(&title, indent, out);
            for child in blocks {
                block_text(child, indent, out);
            }
        }
        ViewKind::Heading { title, .. } => paragraph(&ViewInline::plain_text(title), indent, out),
        ViewKind::Paragraph(content) => paragraph(&ViewInline::plain_text(content), indent, out),
        ViewKind::List { kind, items } => {
            // An ordered list counts from its `start`, down when it is reversed.
            let reversed = block.options.contains("reversed");
            let start = block
                .attributes
                .get("start")
                .and_then(|start| start.trim().parse::<i64>().ok())
                .unwrap_or(if reversed { items.len() as i64 } else { 1 });
            for (index, item) in items.iter().enumerate() {
                let number = if reversed {
                    start - index as i64
                } else {
                    start + index as i64
                };
                let marker = match (kind, item.checked) {
                    (_, Some(true)) => "[x] ".to_owned(),
                    (_, Some(false)) => "[ ] ".to_owned(),
                    (ListKind::Ordered, None) => format!("{number}. "),
                    _ => "- ".to_owned(),
                };
                out.push_str(indent);
                out.push_str(&marker);
                out.push_str(&ViewInline::plain_text(&item.content));
                out.push('\n');
                let nested = format!("{indent}  ");
                for child in &item.blocks {
                    block_text(child, &nested, out);
                }
            }
            out.push('\n');
        }
        ViewKind::DescriptionList(items) => {
            for item in items {
                paragraph(&ViewInline::plain_text(&item.term), indent, out);
                let nested = format!("{indent}  ");
                if !item.content.is_empty() {
                    paragraph(&ViewInline::plain_text(&item.content), &nested, out);
                }
                for child in &item.blocks {
                    block_text(child, &nested, out);
                }
            }
        }
        ViewKind::CalloutList(items) => {
            for item in items {
                out.push_str(&format!(
                    "{indent}({}) {}\n",
                    item.number,
                    ViewInline::plain_text(&item.content)
                ));
                let nested = format!("{indent}  ");
                for child in &item.blocks {
                    block_text(child, &nested, out);
                }
            }
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        }
        ViewKind::Admonition { kind, blocks } => {
            out.push_str(&format!("{indent}{}:\n", kind.label().to_uppercase()));
            for child in blocks {
                block_text(child, indent, out);
            }
        }
        ViewKind::Code { text, .. }
        | ViewKind::Literal(text)
        | ViewKind::Verse { text, .. }
        | ViewKind::Passthrough(text) => paragraph(text, indent, out),
        ViewKind::Quote { blocks, .. }
        | ViewKind::Sidebar(blocks)
        | ViewKind::Example(blocks)
        | ViewKind::Open(blocks) => {
            for child in blocks {
                block_text(child, indent, out);
            }
        }
        ViewKind::Table(table) => {
            for row in table.header.iter().chain(&table.rows).chain(&table.footer) {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| {
                        let mut text = String::new();
                        for child in &cell.blocks {
                            block_text(child, "", &mut text);
                        }
                        text.split_whitespace().collect::<Vec<_>>().join(" ")
                    })
                    .collect();
                out.push_str(indent);
                out.push_str(&cells.join(" | "));
                out.push('\n');
            }
            out.push('\n');
        }
        ViewKind::Image(image) => paragraph(&image.alt, indent, out),
        ViewKind::Stem { source, .. } => paragraph(source, indent, out),
        ViewKind::Unsupported { source, .. } => paragraph(source, indent, out),
        ViewKind::Media { .. }
        | ViewKind::Diagram(_)
        | ViewKind::ThematicBreak
        | ViewKind::PageBreak
        | ViewKind::Toc => {}
    }
}
