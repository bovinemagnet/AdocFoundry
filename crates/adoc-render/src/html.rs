//! HTML from the view tree, shaped like Asciidoctor's output (the same element structure and
//! block class names) so it can be exported and diffed against Asciidoctor.

use std::fmt::Write as _;

use adoc_dom::{Align, CellStyle, ColumnWidth, ListKind, MediaKind};

use crate::view::{
    DiagramView, ResolvedLink, ViewBlock, ViewImage, ViewInline, ViewKind, ViewTree,
};

#[derive(Clone, Debug, Default)]
pub struct HtmlOptions {
    /// A whole page with `<html>`, head and body, rather than the content alone.
    pub standalone: bool,
}

/// Renders `view` to HTML.
#[must_use]
pub fn to_html(view: &ViewTree, options: &HtmlOptions) -> String {
    let mut out = String::new();
    let title = view
        .title
        .as_ref()
        .map(|title| ViewInline::plain_text(title));
    if options.standalone {
        out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n");
        let _ = writeln!(
            out,
            "<title>{}</title>",
            escape(title.as_deref().unwrap_or("Untitled"))
        );
        out.push_str(STYLE);
        out.push_str("</head>\n<body class=\"article\">\n");
    }
    out.push_str("<div id=\"header\">\n");
    if let Some(title) = &view.title {
        let _ = writeln!(out, "<h1>{}</h1>", inlines(title));
    }
    if !view.toc.is_empty() {
        toc(&mut out, view);
    }
    out.push_str("</div>\n<div id=\"content\">\n");
    for block in &view.blocks {
        render_block(&mut out, view, block);
    }
    out.push_str("</div>\n");
    if !view.footnotes.is_empty() {
        out.push_str("<div id=\"footnotes\">\n<hr>\n");
        for note in &view.footnotes {
            let _ = writeln!(
                out,
                "<div class=\"footnote\" id=\"_footnotedef_{0}\"><a href=\"#_footnoteref_{0}\">{0}</a>. {1}</div>",
                note.number,
                inlines(&note.content)
            );
        }
        out.push_str("</div>\n");
    }
    if options.standalone {
        out.push_str("</body>\n</html>\n");
    }
    out
}

const STYLE: &str = "<style>\nbody{font-family:sans-serif;max-width:60em;margin:auto;padding:1em;line-height:1.5}\npre{background:#f6f6f6;padding:.6em;overflow:auto}\n.admonitionblock td.icon{font-weight:bold;padding-right:1em}\ntable.tableblock{border-collapse:collapse}td.tableblock,th.tableblock{border:1px solid #ccc;padding:.3em}\n.title{font-style:italic}\n.sidebarblock{border:1px solid #ccc;padding:.6em}\n.exampleblock{border:1px solid #ddd;padding:.6em}\n</style>\n";

fn toc(out: &mut String, view: &ViewTree) {
    out.push_str("<div id=\"toc\" class=\"toc\">\n<div id=\"toctitle\">Table of Contents</div>\n<ul class=\"sectlevel1\">\n");
    for entry in &view.toc {
        let number = entry
            .number
            .as_deref()
            .map(|number| format!("{number} "))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "<li class=\"level{}\"><a href=\"#{}\">{}{}</a></li>",
            entry.level,
            escape(&entry.id),
            escape(&number),
            escape(&entry.title)
        );
    }
    out.push_str("</ul>\n</div>\n");
}

fn id_attribute(block: &ViewBlock) -> String {
    block
        .id
        .as_deref()
        .map(|id| format!(" id=\"{}\"", escape(id)))
        .unwrap_or_default()
}

fn classes(base: &str, block: &ViewBlock) -> String {
    let mut classes = base.to_owned();
    for role in &block.roles {
        classes.push(' ');
        classes.push_str(&escape(role));
    }
    classes
}

fn block_title(out: &mut String, block: &ViewBlock) {
    if block.title.is_empty() {
        return;
    }
    let caption = block
        .caption
        .as_deref()
        .map(|caption| format!("{caption} "))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "<div class=\"title\">{}{}</div>",
        escape(&caption),
        inlines(&block.title)
    );
}

fn render_blocks(out: &mut String, view: &ViewTree, blocks: &[ViewBlock]) {
    for block in blocks {
        render_block(out, view, block);
    }
}

fn render_block(out: &mut String, view: &ViewTree, block: &ViewBlock) {
    let id = id_attribute(block);
    match &block.kind {
        ViewKind::Section {
            level,
            number,
            title,
            blocks,
        } => {
            let heading = (level + 1).min(6);
            let number = number
                .as_deref()
                .map(|number| format!("{number} "))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "<div class=\"{}\">",
                classes(&format!("sect{level}"), block)
            );
            let _ = writeln!(
                out,
                "<h{heading}{id}>{}{}</h{heading}>",
                escape(&number),
                inlines(title)
            );
            if *level == 1 {
                out.push_str("<div class=\"sectionbody\">\n");
                render_blocks(out, view, blocks);
                out.push_str("</div>\n");
            } else {
                render_blocks(out, view, blocks);
            }
            out.push_str("</div>\n");
        }
        ViewKind::Heading { level, title } => {
            let heading = (level + 1).min(6);
            let _ = writeln!(
                out,
                "<h{heading}{id} class=\"{}\">{}</h{heading}>",
                classes("discrete", block),
                inlines(title)
            );
        }
        ViewKind::Paragraph(content) => {
            let _ = write!(out, "<div{id} class=\"{}\">", classes("paragraph", block));
            block_title(out, block);
            let _ = writeln!(out, "<p>{}</p></div>", inlines(content));
        }
        ViewKind::List { kind, items } => {
            let (class, tag) = match kind {
                ListKind::Unordered => ("ulist", "ul"),
                ListKind::Checklist => ("ulist checklist", "ul"),
                ListKind::Ordered => ("olist arabic", "ol"),
            };
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes(class, block));
            block_title(out, block);
            let _ = writeln!(out, "<{tag}>");
            for item in items {
                out.push_str("<li><p>");
                if let Some(checked) = item.checked {
                    out.push_str(if checked { "&#10003; " } else { "&#10063; " });
                }
                out.push_str(&inlines(&item.content));
                out.push_str("</p>\n");
                render_blocks(out, view, &item.blocks);
                out.push_str("</li>\n");
            }
            let _ = writeln!(out, "</{tag}>\n</div>");
        }
        ViewKind::DescriptionList(items) if block.style.as_deref() == Some("qanda") => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("qlist qanda", block));
            block_title(out, block);
            out.push_str("<ol>\n");
            for item in items {
                let _ = write!(out, "<li><p><em>{}</em></p>", inlines(&item.term));
                if !item.content.is_empty() {
                    let _ = write!(out, "<p>{}</p>", inlines(&item.content));
                }
                render_blocks(out, view, &item.blocks);
                out.push_str("</li>\n");
            }
            out.push_str("</ol>\n</div>\n");
        }
        ViewKind::DescriptionList(items) if block.style.as_deref() == Some("horizontal") => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("hdlist", block));
            block_title(out, block);
            out.push_str("<table>\n");
            for item in items {
                let _ = write!(
                    out,
                    "<tr>\n<td class=\"hdlist1\">\n{}\n</td>\n<td class=\"hdlist2\">\n",
                    inlines(&item.term)
                );
                if !item.content.is_empty() {
                    let _ = writeln!(out, "<p>{}</p>", inlines(&item.content));
                }
                render_blocks(out, view, &item.blocks);
                out.push_str("</td>\n</tr>\n");
            }
            out.push_str("</table>\n</div>\n");
        }
        ViewKind::DescriptionList(items) => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("dlist", block));
            block_title(out, block);
            out.push_str("<dl>\n");
            for item in items {
                let _ = writeln!(out, "<dt class=\"hdlist1\">{}</dt>", inlines(&item.term));
                out.push_str("<dd>");
                if !item.content.is_empty() {
                    let _ = write!(out, "<p>{}</p>", inlines(&item.content));
                }
                render_blocks(out, view, &item.blocks);
                out.push_str("</dd>\n");
            }
            out.push_str("</dl>\n</div>\n");
        }
        ViewKind::CalloutList(items) => {
            let _ = writeln!(
                out,
                "<div{id} class=\"{}\">",
                classes("colist arabic", block)
            );
            out.push_str("<ol>\n");
            for item in items {
                let _ = write!(out, "<li><p>{}</p>", inlines(&item.content));
                render_blocks(out, view, &item.blocks);
                out.push_str("</li>\n");
            }
            out.push_str("</ol>\n</div>\n");
        }
        ViewKind::Admonition { kind, blocks } => {
            let label = kind.label();
            let _ = writeln!(
                out,
                "<div{id} class=\"{}\">",
                classes(&format!("admonitionblock {}", label.to_lowercase()), block)
            );
            let _ = writeln!(
                out,
                "<table><tr><td class=\"icon\"><div class=\"title\">{label}</div></td><td class=\"content\">"
            );
            block_title(out, block);
            match blocks.as_slice() {
                // `NOTE: text`: the paragraph is the admonition itself, starting on its line,
                // and Asciidoctor writes its text straight into the cell. A delimited block's
                // content starts lower down.
                [only]
                    if only.span.path == block.span.path && only.span.line == block.span.line =>
                {
                    if let ViewKind::Paragraph(content) = &only.kind {
                        let _ = writeln!(out, "{}", inlines(content));
                    } else {
                        render_block(out, view, only);
                    }
                }
                _ => render_blocks(out, view, blocks),
            }
            out.push_str("</td></tr></table>\n</div>\n");
        }
        ViewKind::Code {
            language,
            text,
            callouts,
        } => {
            let _ = writeln!(
                out,
                "<div{id} class=\"{}\">",
                classes("listingblock", block)
            );
            block_title(out, block);
            out.push_str("<div class=\"content\">\n");
            let code = code_with_callouts(text, callouts);
            match language {
                Some(language) => {
                    let _ = writeln!(
                        out,
                        "<pre class=\"highlight\"><code class=\"language-{0}\" data-lang=\"{0}\">{1}</code></pre>",
                        escape(language),
                        code
                    );
                }
                None => {
                    let _ = writeln!(out, "<pre>{code}</pre>");
                }
            }
            out.push_str("</div>\n</div>\n");
        }
        ViewKind::Literal(text) => {
            let _ = writeln!(
                out,
                "<div{id} class=\"{}\">",
                classes("literalblock", block)
            );
            block_title(out, block);
            let _ = writeln!(
                out,
                "<div class=\"content\">\n<pre>{}</pre>\n</div>\n</div>",
                escape(text)
            );
        }
        ViewKind::Quote {
            attribution,
            citation,
            blocks,
        } => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("quoteblock", block));
            block_title(out, block);
            out.push_str("<blockquote>\n");
            render_blocks(out, view, blocks);
            out.push_str("</blockquote>\n");
            attribution_html(out, attribution.as_deref(), citation.as_deref());
            out.push_str("</div>\n");
        }
        ViewKind::Verse {
            attribution,
            citation,
            text,
        } => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("verseblock", block));
            block_title(out, block);
            let _ = writeln!(out, "<pre class=\"content\">{}</pre>", escape(text));
            attribution_html(out, attribution.as_deref(), citation.as_deref());
            out.push_str("</div>\n");
        }
        ViewKind::Sidebar(blocks) => container(out, view, block, "sidebarblock", blocks),
        ViewKind::Example(blocks) => container(out, view, block, "exampleblock", blocks),
        ViewKind::Open(blocks) => container(out, view, block, "openblock", blocks),
        ViewKind::Table(table) => {
            let _ = writeln!(
                out,
                "<table{id} class=\"{}\">",
                classes("tableblock frame-all grid-all stretch", block)
            );
            if !block.title.is_empty() {
                let caption = block
                    .caption
                    .as_deref()
                    .map(|caption| format!("{caption} "))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "<caption class=\"title\">{}{}</caption>",
                    escape(&caption),
                    inlines(&block.title)
                );
            }
            let total: u32 = table
                .columns
                .iter()
                .map(|column| match column.width {
                    ColumnWidth::Proportional(width) => width,
                    _ => 1,
                })
                .sum::<u32>()
                .max(1);
            out.push_str("<colgroup>\n");
            for column in &table.columns {
                let percent = match column.width {
                    ColumnWidth::Percentage(width) => f64::from(width),
                    ColumnWidth::Proportional(width) => f64::from(width) * 100.0 / f64::from(total),
                    ColumnWidth::Auto => 100.0 / table.columns.len().max(1) as f64,
                };
                let _ = writeln!(out, "<col style=\"width: {percent:.4}%;\">");
            }
            out.push_str("</colgroup>\n");
            let row = |out: &mut String, cells: &[crate::view::ViewCell], header: bool| {
                out.push_str("<tr>\n");
                for cell in cells {
                    let tag = if header || cell.style == CellStyle::Header {
                        "th"
                    } else {
                        "td"
                    };
                    let span = format!(
                        "{}{}",
                        if cell.colspan > 1 {
                            format!(" colspan=\"{}\"", cell.colspan)
                        } else {
                            String::new()
                        },
                        if cell.rowspan > 1 {
                            format!(" rowspan=\"{}\"", cell.rowspan)
                        } else {
                            String::new()
                        }
                    );
                    let align = match cell.align {
                        Align::Start => "left",
                        Align::Centre => "center",
                        Align::End => "right",
                    };
                    let _ = write!(
                        out,
                        "<{tag} class=\"tableblock halign-{align} valign-top\"{span}>"
                    );
                    if cell.style == CellStyle::AsciiDoc {
                        // An AsciiDoc cell is a nested document: its blocks render as blocks.
                        out.push_str("<div class=\"content\">");
                        for block in &cell.blocks {
                            render_block(out, view, block);
                        }
                        out.push_str("</div>");
                        let _ = writeln!(out, "</{tag}>");
                        continue;
                    }
                    for block in &cell.blocks {
                        match &block.kind {
                            ViewKind::Paragraph(content) if tag == "th" => {
                                out.push_str(&inlines(content));
                            }
                            ViewKind::Paragraph(content) => {
                                let _ =
                                    write!(out, "<p class=\"tableblock\">{}</p>", inlines(content));
                            }
                            _ => render_block(out, view, block),
                        }
                    }
                    let _ = writeln!(out, "</{tag}>");
                }
                out.push_str("</tr>\n");
            };
            if let Some(header) = &table.header {
                out.push_str("<thead>\n");
                row(out, header, true);
                out.push_str("</thead>\n");
            }
            out.push_str("<tbody>\n");
            for cells in &table.rows {
                row(out, cells, false);
            }
            out.push_str("</tbody>\n");
            if let Some(footer) = &table.footer {
                out.push_str("<tfoot>\n");
                row(out, footer, false);
                out.push_str("</tfoot>\n");
            }
            out.push_str("</table>\n");
        }
        ViewKind::Image(image) => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("imageblock", block));
            let _ = writeln!(out, "<div class=\"content\">\n{}\n</div>", img(image));
            block_title(out, block);
            out.push_str("</div>\n");
        }
        ViewKind::Media { kind, targets } => {
            let (class, tag) = match kind {
                MediaKind::Audio => ("audioblock", "audio"),
                MediaKind::Video => ("videoblock", "video"),
            };
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes(class, block));
            block_title(out, block);
            let source = targets.first().map(String::as_str).unwrap_or("");
            let _ = writeln!(
                out,
                "<div class=\"content\">\n<{tag} src=\"{}\" controls></{tag}>\n</div>\n</div>",
                escape(source)
            );
        }
        ViewKind::Diagram(diagram) => match diagram {
            DiagramView::Svg(path) => {
                let _ = writeln!(
                    out,
                    "<div{id} class=\"{}\">",
                    classes("imageblock diagram", block)
                );
                let _ = writeln!(
                    out,
                    "<div class=\"content\">\n<img src=\"{}\" alt=\"Diagram\">\n</div>",
                    escape(&file_url(path))
                );
                block_title(out, block);
                out.push_str("</div>\n");
            }
            DiagramView::Pending { source, .. } | DiagramView::Failed { source, .. } => {
                let _ = writeln!(
                    out,
                    "<div{id} class=\"{}\">",
                    classes("listingblock diagram", block)
                );
                block_title(out, block);
                let _ = writeln!(
                    out,
                    "<div class=\"content\">\n<pre>{}</pre>\n</div>\n</div>",
                    escape(source)
                );
            }
        },
        ViewKind::Stem { latex, source } => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("stemblock", block));
            block_title(out, block);
            let (open, close) = if *latex {
                ("\\[", "\\]")
            } else {
                ("\\$", "\\$")
            };
            let _ = writeln!(
                out,
                "<div class=\"content\">\n{open}{}{close}\n</div>\n</div>",
                escape(source)
            );
        }
        ViewKind::Passthrough(content) => {
            out.push_str(content);
            out.push('\n');
        }
        ViewKind::ThematicBreak => out.push_str("<hr>\n"),
        ViewKind::PageBreak => out.push_str("<div style=\"page-break-after: always;\"></div>\n"),
        ViewKind::Toc => toc(out, view),
        ViewKind::Unsupported { name, source } => {
            let _ = writeln!(
                out,
                "<div{id} class=\"listingblock unsupported\" data-construct=\"{}\"><div class=\"content\"><pre>{}</pre></div></div>",
                escape(name),
                escape(source)
            );
        }
    }
}

fn container(
    out: &mut String,
    view: &ViewTree,
    block: &ViewBlock,
    class: &str,
    blocks: &[ViewBlock],
) {
    let id = id_attribute(block);
    let _ = writeln!(out, "<div{id} class=\"{}\">", classes(class, block));
    out.push_str("<div class=\"content\">\n");
    block_title(out, block);
    render_blocks(out, view, blocks);
    out.push_str("</div>\n</div>\n");
}

fn attribution_html(out: &mut String, attribution: Option<&str>, citation: Option<&str>) {
    if attribution.is_none() && citation.is_none() {
        return;
    }
    out.push_str("<div class=\"attribution\">\n");
    if let Some(attribution) = attribution {
        let _ = write!(out, "&#8212; {}", escape(attribution));
    }
    if let Some(citation) = citation {
        let _ = write!(out, "<br>\n<cite>{}</cite>", escape(citation));
    }
    out.push_str("\n</div>\n");
}

fn code_with_callouts(text: &str, callouts: &[adoc_dom::Callout]) -> String {
    let mut out = String::new();
    for (line_number, line) in text.split('\n').enumerate() {
        if line_number > 0 {
            out.push('\n');
        }
        out.push_str(&escape(line));
        for callout in callouts
            .iter()
            .filter(|callout| callout.line == line_number)
        {
            let _ = write!(out, " <b class=\"conum\">({})</b>", callout.number);
        }
    }
    out
}

fn file_url(path: &std::path::Path) -> String {
    // A relative path after `file://` would be read as a host name; `from_file_path`
    // percent-encodes each segment and writes Windows drives as `/C:/`.
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    url::Url::from_file_path(&absolute)
        .map_or_else(|()| format!("file://{}", absolute.display()), String::from)
}

fn img(image: &ViewImage) -> String {
    let source = match (&image.path, &image.url) {
        (Some(path), _) => file_url(path),
        (None, Some(url)) => url.clone(),
        (None, None) => image.target.clone(),
    };
    let mut attributes = String::new();
    if let Some(width) = &image.width {
        let _ = write!(attributes, " width=\"{}\"", escape(width));
    }
    if let Some(height) = &image.height {
        let _ = write!(attributes, " height=\"{}\"", escape(height));
    }
    format!(
        "<img src=\"{}\" alt=\"{}\"{attributes}>",
        escape(&source),
        escape(&image.alt)
    )
}

fn xref_href(target: &str, link: &ResolvedLink) -> String {
    match (&link.fragment, target.split_once('#')) {
        (Some(fragment), _) if !target.contains(".adoc") && !target.contains('$') => {
            format!("#{fragment}")
        }
        (_, Some((file, fragment))) => format!("{}#{fragment}", file.replace(".adoc", ".html")),
        _ => target.replace(".adoc", ".html"),
    }
}

/// HTML for a run of inlines.
#[must_use]
pub fn inlines(content: &[ViewInline]) -> String {
    let mut out = String::new();
    for inline in content {
        inline_html(&mut out, inline);
    }
    out
}

fn inline_html(out: &mut String, inline: &ViewInline) {
    let wrap = |out: &mut String, tag: &str, content: &[ViewInline]| {
        let _ = write!(out, "<{tag}>{}</{tag}>", inlines(content));
    };
    match inline {
        ViewInline::Text(text) => out.push_str(&escape(text)),
        ViewInline::Strong(content) => wrap(out, "strong", content),
        ViewInline::Emphasis(content) => wrap(out, "em", content),
        ViewInline::Code(content) => wrap(out, "code", content),
        ViewInline::Mark(content) => wrap(out, "mark", content),
        ViewInline::Superscript(content) => wrap(out, "sup", content),
        ViewInline::Subscript(content) => wrap(out, "sub", content),
        ViewInline::Span { roles, content } => {
            let _ = write!(
                out,
                "<span class=\"{}\">{}</span>",
                escape(&roles.join(" ")),
                inlines(content)
            );
        }
        ViewInline::Link { url, content } => {
            let _ = write!(
                out,
                "<a href=\"{}\" class=\"bare\">{}</a>",
                escape(url),
                inlines(content)
            );
        }
        ViewInline::Xref {
            target,
            link,
            content,
            ..
        } => {
            let class = if link.unresolved {
                " class=\"unresolved\""
            } else {
                ""
            };
            let _ = write!(
                out,
                "<a href=\"{}\"{class}>{}</a>",
                escape(&xref_href(target, link)),
                inlines(content)
            );
        }
        ViewInline::Anchor(id) => {
            let _ = write!(out, "<a id=\"{}\"></a>", escape(id));
        }
        ViewInline::Image(image) => {
            let _ = write!(out, "<span class=\"image\">{}</span>", img(image));
        }
        ViewInline::Icon(name) => {
            let _ = write!(out, "<span class=\"icon\">[{}]</span>", escape(name));
        }
        ViewInline::FootnoteRef(number) => {
            let _ = write!(
                out,
                "<sup class=\"footnote\">[<a id=\"_footnoteref_{number}\" class=\"footnote\" href=\"#_footnotedef_{number}\">{number}</a>]</sup>"
            );
        }
        ViewInline::LineBreak => out.push_str("<br>\n"),
        ViewInline::Keyboard(keys) => {
            let keys: Vec<String> = keys
                .iter()
                .map(|key| format!("<kbd>{}</kbd>", escape(key)))
                .collect();
            let _ = write!(out, "<span class=\"keyseq\">{}</span>", keys.join("+"));
        }
        ViewInline::Button(label) => {
            let _ = write!(out, "<b class=\"button\">{}</b>", escape(label));
        }
        ViewInline::Menu(items) => {
            let _ = write!(
                out,
                "<span class=\"menuseq\">{}</span>",
                items
                    .iter()
                    .map(|item| escape(item))
                    .collect::<Vec<_>>()
                    .join("&#160;&#8250; ")
            );
        }
        ViewInline::Stem { latex, source } => {
            let (open, close) = if *latex {
                ("\\(", "\\)")
            } else {
                ("\\$", "\\$")
            };
            let _ = write!(out, "{open}{}{close}", escape(source));
        }
        ViewInline::Raw(text) => out.push_str(text),
    }
}

/// Escapes text for HTML element content and attribute values.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(character),
        }
    }
    out
}
