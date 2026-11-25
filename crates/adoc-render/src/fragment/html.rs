//! A fragment as HTML for a word processor: plain elements with their look in place, since a
//! paste brings no style sheet with it. Nothing written names a file on this computer.

use std::fmt::Write as _;

use adoc_dom::ListKind;

use super::{content_of, plain, Fragment, FragmentBlock, FragmentCell, FragmentItem, FragmentKind};
use crate::{escape, ResolvedLink, ViewImage, ViewInline};

const CODE: &str = "font-family:'Courier New',monospace";
const BLOCK: &str =
    "font-family:'Courier New',monospace;background:#f6f6f6;padding:6pt;white-space:pre-wrap";
const CELL: &str = "border:1px solid #bbb;padding:3pt 6pt;vertical-align:top";

type Links<'a> = &'a dyn Fn(&ResolvedLink) -> Option<String>;

pub(super) fn write(fragment: &Fragment, link: Links<'_>) -> String {
    // A part of one text has no element of its own, so it joins the text it is pasted into.
    if let [FragmentBlock {
        kind: FragmentKind::Paragraph(content),
        title,
        whole: None,
        ..
    }] = fragment.blocks.as_slice()
    {
        if title.is_empty() {
            return inlines(content, link);
        }
    }
    let mut out = String::new();
    blocks(&fragment.blocks, link, &mut out);
    for note in fragment.noted() {
        let _ = write!(
            out,
            "<p><sup>[{}]</sup> {}</p>",
            note.number,
            inlines(&note.content, link)
        );
    }
    out
}

fn blocks(blocks: &[FragmentBlock], link: Links<'_>, out: &mut String) {
    for part in blocks {
        block(part, link, out);
    }
}

fn block(block: &FragmentBlock, link: Links<'_>, out: &mut String) {
    let mut title = String::new();
    if !block.title.is_empty() {
        let caption = block
            .caption
            .as_ref()
            .map_or_else(String::new, |caption| format!("{} ", escape(caption)));
        let _ = write!(
            title,
            "<p><em>{caption}{}</em></p>",
            inlines(&block.title, link)
        );
    }
    let after = matches!(
        block.kind,
        FragmentKind::Image(_) | FragmentKind::Diagram(_)
    );
    if !after {
        out.push_str(&title);
    }
    body(&block.kind, link, out);
    if after {
        out.push_str(&title);
    }
}

fn body(kind: &FragmentKind, link: Links<'_>, out: &mut String) {
    match kind {
        FragmentKind::Heading {
            level,
            content,
            number,
            ..
        } => {
            let level = (level + 1).min(6);
            let number = number
                .as_ref()
                .map_or_else(String::new, |number| format!("{} ", escape(number)));
            let _ = write!(
                out,
                "<h{level}>{number}{}</h{level}>",
                inlines(content, link)
            );
        }
        FragmentKind::Paragraph(content) => {
            let _ = write!(out, "<p>{}</p>", inlines(content, link));
        }
        FragmentKind::List {
            kind,
            items,
            reversed,
        } => list(*kind, items, *reversed, link, out),
        FragmentKind::Callouts(items) => list(ListKind::Ordered, items, false, link, out),
        FragmentKind::Descriptions(items) => {
            out.push_str("<dl>");
            for item in items {
                if !item.term.is_empty() {
                    let _ = write!(
                        out,
                        "<dt><strong>{}</strong></dt>",
                        inlines(&item.term, link)
                    );
                }
                if !(item.content.is_empty() && item.blocks.is_empty()) {
                    let _ = write!(out, "<dd>{}", inlines(&item.content, link));
                    blocks(&item.blocks, link, out);
                    out.push_str("</dd>");
                }
            }
            out.push_str("</dl>");
        }
        FragmentKind::Admonition {
            kind,
            blocks: inside,
            ..
        } => {
            let _ = write!(
                out,
                "<table style=\"border-collapse:collapse\"><tr><td style=\"{CELL}\"><strong>{}</strong></td><td style=\"{CELL}\">",
                kind.label()
            );
            blocks(inside, link, out);
            out.push_str("</td></tr></table>");
        }
        FragmentKind::Code { text, callouts, .. } => {
            preformatted(&plain::code(text, callouts), out)
        }
        FragmentKind::Literal(text) | FragmentKind::Passthrough(text) => preformatted(text, out),
        FragmentKind::Stem { source, .. } => preformatted(source.trim(), out),
        FragmentKind::Quote {
            attribution,
            citation,
            blocks: inside,
        } => {
            out.push_str("<blockquote>");
            blocks(inside, link, out);
            signed(attribution, citation, out);
            out.push_str("</blockquote>");
        }
        FragmentKind::Verse {
            attribution,
            citation,
            text,
        } => {
            let _ = write!(
                out,
                "<blockquote><p>{}</p>",
                escape(text).replace('\n', "<br>")
            );
            signed(attribution, citation, out);
            out.push_str("</blockquote>");
        }
        FragmentKind::Sidebar(inside) | FragmentKind::Example(inside) => {
            let _ = write!(
                out,
                "<table style=\"border-collapse:collapse\"><tr><td style=\"{CELL}\">"
            );
            blocks(inside, link, out);
            out.push_str("</td></tr></table>");
        }
        FragmentKind::Open(inside) => blocks(inside, link, out),
        FragmentKind::Table { header, rows } => {
            out.push_str("<table style=\"border-collapse:collapse\">");
            for (index, cells) in rows.iter().enumerate() {
                row(cells, *header && index == 0, link, out);
            }
            out.push_str("</table>");
        }
        FragmentKind::Diagram(diagram) => {
            if let Some((_, source)) = diagram {
                preformatted(source, out);
            }
        }
        FragmentKind::Image(picture) => {
            let _ = write!(out, "<p>{}</p>", image(picture));
        }
        FragmentKind::Media { targets, .. } => {
            let _ = write!(out, "<p>{}</p>", escape(&targets.join(", ")));
        }
        FragmentKind::Rule => out.push_str("<hr>"),
        FragmentKind::PageBreak => {}
        FragmentKind::Text(text) => {
            let _ = write!(out, "<p>{}</p>", escape(text).replace('\n', "<br>"));
        }
    }
}

fn preformatted(text: &str, out: &mut String) {
    let _ = write!(out, "<pre style=\"{BLOCK}\">{}</pre>", escape(text));
}

fn signed(attribution: &Option<String>, citation: &Option<String>, out: &mut String) {
    let line = match (attribution, citation) {
        (None, None) => return,
        (Some(one), None) | (None, Some(one)) => escape(one),
        (Some(attribution), Some(citation)) => {
            format!("{}, {}", escape(attribution), escape(citation))
        }
    };
    let _ = write!(out, "<p>— {line}</p>");
}

fn list(kind: ListKind, items: &[FragmentItem], reversed: bool, link: Links<'_>, out: &mut String) {
    let tag = if kind == ListKind::Ordered {
        "ol"
    } else {
        "ul"
    };
    let _ = write!(out, "<{tag}");
    if tag == "ol" {
        if let Some(first) = items.first().filter(|first| first.number != 1) {
            let _ = write!(out, " start=\"{}\"", first.number);
        }
        if reversed {
            out.push_str(" reversed");
        }
    }
    out.push('>');
    for item in items {
        let check = match item.checked {
            Some(true) => "☑ ",
            Some(false) => "☐ ",
            None => "",
        };
        let _ = write!(out, "<li>{check}{}", inlines(&item.content, link));
        blocks(&item.blocks, link, out);
        out.push_str("</li>");
    }
    let _ = write!(out, "</{tag}>");
}

fn row(cells: &[FragmentCell], header: bool, link: Links<'_>, out: &mut String) {
    out.push_str("<tr>");
    let tag = if header { "th" } else { "td" };
    for cell in cells {
        let _ = write!(out, "<{tag} style=\"{CELL}\"");
        if cell.colspan > 1 {
            let _ = write!(out, " colspan=\"{}\"", cell.colspan);
        }
        if cell.rowspan > 1 {
            let _ = write!(out, " rowspan=\"{}\"", cell.rowspan);
        }
        out.push('>');
        // A cell of one paragraph is its text.
        match cell.blocks.as_slice() {
            [FragmentBlock {
                kind: FragmentKind::Paragraph(content),
                title,
                ..
            }] if title.is_empty() => out.push_str(&inlines(content, link)),
            inside => blocks(inside, link, out),
        }
        let _ = write!(out, "</{tag}>");
    }
    out.push_str("</tr>");
}

/// A picture at a web address, or the text that stands for one in a file.
fn image(image: &ViewImage) -> String {
    match &image.url {
        Some(url) if followable(url) => format!(
            "<img src=\"{}\" alt=\"{}\">",
            escape(url),
            escape(&image.alt)
        ),
        _ => escape(&image.alt),
    }
}

/// Whether an address is one a reader of a pasted document may follow.
fn followable(url: &str) -> bool {
    ["https://", "http://", "mailto:", "tel:", "ftp://"]
        .iter()
        .any(|scheme| {
            url.get(..scheme.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
        })
}

fn inlines(content: &[ViewInline], link: Links<'_>) -> String {
    content.iter().map(|part| inline(part, link)).collect()
}

fn inline(inline: &ViewInline, link: Links<'_>) -> String {
    let wrap =
        |tag: &str, content: &[ViewInline]| format!("<{tag}>{}</{tag}>", inlines(content, link));
    match inline {
        ViewInline::Text(text) | ViewInline::Raw(text) => escape(&text.replace('\n', " ")),
        ViewInline::Strong(content) => wrap("strong", content),
        ViewInline::Emphasis(content) => wrap("em", content),
        ViewInline::Superscript(content) => wrap("sup", content),
        ViewInline::Subscript(content) => wrap("sub", content),
        ViewInline::Code(content) => {
            format!("<code style=\"{CODE}\">{}</code>", inlines(content, link))
        }
        ViewInline::Mark(content) => format!(
            "<span style=\"background:#ffff66\">{}</span>",
            inlines(content, link)
        ),
        ViewInline::Link { url, content } if followable(url) => {
            format!("<a href=\"{}\">{}</a>", escape(url), inlines(content, link))
        }
        ViewInline::Xref {
            link: target,
            content,
            ..
        } => match link(target).filter(|url| followable(url)) {
            Some(url) => format!(
                "<a href=\"{}\">{}</a>",
                escape(&url),
                inlines(content, link)
            ),
            None => inlines(content, link),
        },
        ViewInline::Image(picture) => image(picture),
        ViewInline::FootnoteRef { number, .. } => format!("<sup>[{number}]</sup>"),
        ViewInline::LineBreak => "<br>".to_owned(),
        ViewInline::Keyboard(_) => format!(
            "<code style=\"{CODE}\">{}</code>",
            escape(&plain::inlines(std::slice::from_ref(inline)))
        ),
        ViewInline::Button(label) => format!("<strong>{}</strong>", escape(label)),
        ViewInline::Menu(items) => format!("<strong>{}</strong>", escape(&items.join(" › "))),
        ViewInline::Stem { source, .. } => {
            format!("<code style=\"{CODE}\">{}</code>", escape(source.trim()))
        }
        other => match content_of(other) {
            Some(content) => inlines(content, link),
            None => escape(&plain::inlines(std::slice::from_ref(other))),
        },
    }
}
