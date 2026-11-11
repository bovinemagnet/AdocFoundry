//! HTML from the view tree, shaped like Asciidoctor's output (the same element structure and
//! block class names) so it can be exported and diffed against Asciidoctor.

use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Component, Path, PathBuf},
};

use adoc_dom::{Align, CellStyle, ColumnWidth, ListKind, MediaKind, VAlign};

use crate::view::{
    DiagramView, ResolvedLink, ViewBlock, ViewImage, ViewInline, ViewKind, ViewTree,
};

#[derive(Clone, Debug, Default)]
pub struct HtmlOptions {
    /// A whole page with `<html>`, head and body, rather than the content alone.
    pub standalone: bool,
    /// The published address of each page, by its source path (the site's, as PDF export
    /// links to it). A cross-page xref to a page listed here leads there; one to any other page
    /// leads to where that page's own export would sit beside its source.
    pub page_urls: HashMap<PathBuf, String>,
}

/// The page being written, with the options it is written with.
struct Page<'a> {
    view: &'a ViewTree,
    options: &'a HtmlOptions,
}

impl std::ops::Deref for Page<'_> {
    type Target = ViewTree;

    fn deref(&self) -> &ViewTree {
        self.view
    }
}

/// Renders `view` to HTML.
#[must_use]
pub fn to_html(view: &ViewTree, options: &HtmlOptions) -> String {
    let page = Page { view, options };
    let view = &page;
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
        let _ = writeln!(out, "<h1>{}</h1>", page_inlines(view, title));
    }
    if !view.toc.is_empty() && !view.toc_at_macro() {
        toc(&mut out, view, "");
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
                page_inlines(view, &note.content)
            );
        }
        out.push_str("</div>\n");
    }
    if options.standalone {
        out.push_str("</body>\n</html>\n");
    }
    out
}

const STYLE: &str = "<style>\nbody{font-family:sans-serif;max-width:60em;margin:auto;padding:1em;line-height:1.5}\npre{background:#f6f6f6;padding:.6em;overflow:auto}\n.admonitionblock td.icon{font-weight:bold;padding-right:1em}\n.admonitionblock td.icon i.fa::before{content:attr(title);font-style:normal}\ntable.tableblock{border-collapse:collapse}td.tableblock,th.tableblock{border:1px solid #ccc;padding:.3em}\n.title{font-style:italic}\n.sidebarblock{border:1px solid #ccc;padding:.6em}\n.exampleblock{border:1px solid #ddd;padding:.6em}\n</style>\n";

/// The table of contents, as Asciidoctor writes it: a list per level, each inside the entry
/// it belongs to. `title_class` is the title's own (` class="title"` at a `toc::[]` macro).
fn toc(out: &mut String, view: &Page<'_>, title_class: &str) {
    let _ = writeln!(
        out,
        "<div id=\"toc\" class=\"toc\">\n<div id=\"toctitle\"{title_class}>{}</div>",
        escape(view.toc_title())
    );
    // The levels of the lists open around the entry being written.
    let mut open: Vec<u8> = Vec::new();
    for entry in &view.toc {
        match open.last() {
            Some(&level) if entry.level > level => {
                let _ = write!(out, "\n<ul class=\"sectlevel{}\">\n", entry.level);
                open.push(entry.level);
            }
            Some(_) => {
                out.push_str("</li>\n");
                while open.len() > 1 && open.last().is_some_and(|&level| entry.level < level) {
                    out.push_str("</ul>\n</li>\n");
                    open.pop();
                }
            }
            None => {
                let _ = writeln!(out, "<ul class=\"sectlevel{}\">", entry.level);
                open.push(entry.level);
            }
        }
        let number = entry
            .number
            .as_deref()
            .map(|number| format!("{number} "))
            .unwrap_or_default();
        let _ = write!(
            out,
            "<li><a href=\"#{}\">{}{}</a>",
            escape(&entry.id),
            escape(&number),
            escape(&entry.title)
        );
    }
    if !open.is_empty() {
        out.push_str("</li>\n");
        for _ in 1..open.len() {
            out.push_str("</ul>\n</li>\n");
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</div>\n");
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

fn block_title(out: &mut String, view: &Page<'_>, block: &ViewBlock) {
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
        page_inlines(view, &block.title)
    );
}

fn render_blocks(out: &mut String, view: &Page<'_>, blocks: &[ViewBlock]) {
    for block in blocks {
        render_block(out, view, block);
    }
}

fn render_block(out: &mut String, view: &Page<'_>, block: &ViewBlock) {
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
                page_inlines(view, title)
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
                page_inlines(view, title)
            );
        }
        ViewKind::Paragraph(content) => {
            let _ = write!(out, "<div{id} class=\"{}\">", classes("paragraph", block));
            block_title(out, view, block);
            let _ = writeln!(out, "<p>{}</p></div>", page_inlines(view, content));
        }
        ViewKind::List { kind, items } => {
            let (class, tag, list) = match kind {
                ListKind::Unordered => ("ulist".to_owned(), "ul", String::new()),
                ListKind::Checklist => ("ulist checklist".to_owned(), "ul", String::new()),
                ListKind::Ordered => {
                    let style = block.style.as_deref().unwrap_or("arabic");
                    (
                        format!("olist {}", escape(style)),
                        "ol",
                        ordered_list_attributes(style, block),
                    )
                }
            };
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes(&class, block));
            block_title(out, view, block);
            let _ = writeln!(out, "<{tag}{list}>");
            for item in items {
                out.push_str("<li><p>");
                if let Some(checked) = item.checked {
                    out.push_str(if checked { "&#10003; " } else { "&#10063; " });
                }
                out.push_str(&page_inlines(view, &item.content));
                out.push_str("</p>\n");
                render_blocks(out, view, &item.blocks);
                out.push_str("</li>\n");
            }
            let _ = writeln!(out, "</{tag}>\n</div>");
        }
        ViewKind::DescriptionList(items) if block.style.as_deref() == Some("qanda") => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("qlist qanda", block));
            block_title(out, view, block);
            out.push_str("<ol>\n");
            for item in items {
                let _ = write!(
                    out,
                    "<li><p><em>{}</em></p>",
                    page_inlines(view, &item.term)
                );
                if !item.content.is_empty() {
                    let _ = write!(out, "<p>{}</p>", page_inlines(view, &item.content));
                }
                render_blocks(out, view, &item.blocks);
                out.push_str("</li>\n");
            }
            out.push_str("</ol>\n</div>\n");
        }
        ViewKind::DescriptionList(items) if block.style.as_deref() == Some("horizontal") => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("hdlist", block));
            block_title(out, view, block);
            out.push_str("<table>\n");
            for item in items {
                let _ = write!(
                    out,
                    "<tr>\n<td class=\"hdlist1\">\n{}\n</td>\n<td class=\"hdlist2\">\n",
                    page_inlines(view, &item.term)
                );
                if !item.content.is_empty() {
                    let _ = writeln!(out, "<p>{}</p>", page_inlines(view, &item.content));
                }
                render_blocks(out, view, &item.blocks);
                out.push_str("</td>\n</tr>\n");
            }
            out.push_str("</table>\n</div>\n");
        }
        ViewKind::DescriptionList(items) => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("dlist", block));
            block_title(out, view, block);
            out.push_str("<dl>\n");
            for item in items {
                let _ = writeln!(
                    out,
                    "<dt class=\"hdlist1\">{}</dt>",
                    page_inlines(view, &item.term)
                );
                out.push_str("<dd>");
                if !item.content.is_empty() {
                    let _ = write!(out, "<p>{}</p>", page_inlines(view, &item.content));
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
            block_title(out, view, block);
            out.push_str("<ol>\n");
            for item in items {
                let _ = write!(out, "<li><p>{}</p>", page_inlines(view, &item.content));
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
            // With `icons=font`, Font Awesome's element for the stylesheet to draw, unless the
            // block names an icon image of its own, which is not written.
            let icon = if font_icons(view) && !block.attributes.contains_key("icon") {
                format!(
                    "<i class=\"fa icon-{}\" title=\"{label}\"></i>",
                    label.to_lowercase()
                )
            } else {
                format!("<div class=\"title\">{label}</div>")
            };
            let _ = writeln!(
                out,
                "<table><tr><td class=\"icon\">{icon}</td><td class=\"content\">"
            );
            block_title(out, view, block);
            match blocks.as_slice() {
                // `NOTE: text`: the paragraph is the admonition itself, starting on its line,
                // and Asciidoctor writes its text straight into the cell. A delimited block's
                // content starts lower down.
                [only]
                    if only.span.path == block.span.path && only.span.line == block.span.line =>
                {
                    if let ViewKind::Paragraph(content) = &only.kind {
                        let _ = writeln!(out, "{}", page_inlines(view, content));
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
            block_title(out, view, block);
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
            block_title(out, view, block);
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
            block_title(out, view, block);
            out.push_str("<blockquote>\n");
            match blocks.as_slice() {
                // A `[quote]` paragraph, which is its quote's only text and shares its place.
                [only] if only.span == block.span => {
                    if let ViewKind::Paragraph(content) = &only.kind {
                        let _ = writeln!(out, "{}", page_inlines(view, content));
                    } else {
                        render_block(out, view, only);
                    }
                }
                _ => render_blocks(out, view, blocks),
            }
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
            block_title(out, view, block);
            let _ = writeln!(out, "<pre class=\"content\">{}</pre>", escape(text));
            attribution_html(out, attribution.as_deref(), citation.as_deref());
            out.push_str("</div>\n");
        }
        ViewKind::Sidebar(blocks) => container(out, view, block, "sidebarblock", blocks),
        ViewKind::Example(blocks) if block.options.contains("collapsible") => {
            // As Asciidoctor writes it: a disclosure, titled `Details` unless the block is.
            let class = classes("", block);
            let class = class.trim();
            let class = if class.is_empty() {
                String::new()
            } else {
                format!(" class=\"{class}\"")
            };
            let open = if block.options.contains("open") {
                " open"
            } else {
                ""
            };
            let _ = writeln!(out, "<details{id}{class}{open}>");
            let summary = if block.title.is_empty() {
                "Details".to_owned()
            } else {
                page_inlines(view, &block.title)
            };
            let _ = writeln!(out, "<summary class=\"title\">{summary}</summary>");
            out.push_str("<div class=\"content\">\n");
            render_blocks(out, view, blocks);
            out.push_str("</div>\n</details>\n");
        }
        ViewKind::Example(blocks) => container(out, view, block, "exampleblock", blocks),
        ViewKind::Open(blocks) => container(out, view, block, "openblock", blocks),
        ViewKind::Table(table) => {
            let (class, style) = table_presentation(block);
            let _ = writeln!(
                out,
                "<table{id} class=\"{}\"{style}>",
                classes(&class, block)
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
                    page_inlines(view, &block.title)
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
                    let valign = match cell.valign {
                        VAlign::Top => "top",
                        VAlign::Middle => "middle",
                        VAlign::Bottom => "bottom",
                    };
                    let _ = write!(
                        out,
                        "<{tag} class=\"tableblock halign-{align} valign-{valign}\"{span}>"
                    );
                    // An AsciiDoc cell is a nested document: its blocks render as blocks. A header
                    // row's cells are plain, whatever their column's style.
                    if cell.style == CellStyle::AsciiDoc && !header {
                        out.push_str("<div class=\"content\">");
                        for block in &cell.blocks {
                            render_block(out, view, block);
                        }
                        out.push_str("</div>");
                        let _ = writeln!(out, "</{tag}>");
                        continue;
                    }
                    // A header row's cells are plain; a literal cell keeps its text as written.
                    if cell.style == CellStyle::Literal && !header {
                        let text: Vec<String> = cell
                            .blocks
                            .iter()
                            .map(|block| match &block.kind {
                                ViewKind::Paragraph(content) => ViewInline::plain_text(content),
                                ViewKind::Literal(text) => text.clone(),
                                _ => String::new(),
                            })
                            .collect();
                        let _ = writeln!(
                            out,
                            "<div class=\"literal\"><pre>{}</pre></div></{tag}>",
                            escape(&text.join("\n\n"))
                        );
                        continue;
                    }
                    let wrap = match cell.style {
                        _ if header => None,
                        CellStyle::Emphasis => Some("em"),
                        CellStyle::Strong => Some("strong"),
                        CellStyle::Monospace => Some("code"),
                        _ => None,
                    };
                    for block in &cell.blocks {
                        match &block.kind {
                            ViewKind::Paragraph(content) if header => {
                                out.push_str(&page_inlines(view, content));
                            }
                            ViewKind::Paragraph(content) => {
                                let content = page_inlines(view, content);
                                let content = match wrap {
                                    Some(wrap) => format!("<{wrap}>{content}</{wrap}>"),
                                    None => content,
                                };
                                let _ = write!(out, "<p class=\"tableblock\">{content}</p>");
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
            let mut class = "imageblock".to_owned();
            if let Some(float) = block.attributes.get("float") {
                let _ = write!(class, " {}", escape(float));
            }
            if let Some(align) = block.attributes.get("align") {
                let _ = write!(class, " text-{}", escape(align));
            }
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes(&class, block));
            let shown = match block.attributes.get("link") {
                Some(link) => format!(
                    "<a class=\"image\" href=\"{}\">{}</a>",
                    escape(link),
                    img(image)
                ),
                None => img(image),
            };
            let _ = writeln!(out, "<div class=\"content\">\n{shown}\n</div>");
            block_title(out, view, block);
            out.push_str("</div>\n");
        }
        ViewKind::Media { kind, targets } => {
            let (class, tag) = match kind {
                MediaKind::Audio => ("audioblock", "audio"),
                MediaKind::Video => ("videoblock", "video"),
            };
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes(class, block));
            block_title(out, view, block);
            let _ = writeln!(
                out,
                "<div class=\"content\">\n{}\n</div>\n</div>",
                media_element(*kind, tag, targets, block)
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
                block_title(out, view, block);
                out.push_str("</div>\n");
            }
            DiagramView::Pending { source, .. } | DiagramView::Failed { source, .. } => {
                let _ = writeln!(
                    out,
                    "<div{id} class=\"{}\">",
                    classes("listingblock diagram", block)
                );
                block_title(out, view, block);
                let _ = writeln!(
                    out,
                    "<div class=\"content\">\n<pre>{}</pre>\n</div>\n</div>",
                    escape(source)
                );
            }
        },
        ViewKind::Stem {
            rendered: DiagramView::Svg(path),
            source,
            ..
        } => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("stemblock", block));
            block_title(out, view, block);
            let _ = writeln!(
                out,
                "<div class=\"content\">\n<img src=\"{}\" alt=\"{}\">\n</div>\n</div>",
                escape(&file_url(path)),
                escape(source.trim())
            );
        }
        ViewKind::Stem { latex, source, .. } => {
            let _ = writeln!(out, "<div{id} class=\"{}\">", classes("stemblock", block));
            block_title(out, view, block);
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
        ViewKind::Toc if view.toc_at_macro() && !view.toc.is_empty() => {
            toc(out, view, " class=\"title\"");
        }
        ViewKind::Toc => {}
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
    view: &Page<'_>,
    block: &ViewBlock,
    class: &str,
    blocks: &[ViewBlock],
) {
    let id = id_attribute(block);
    let _ = writeln!(out, "<div{id} class=\"{}\">", classes(class, block));
    out.push_str("<div class=\"content\">\n");
    block_title(out, view, block);
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

/// A video or audio block's player, as Asciidoctor writes it: a YouTube or Vimeo video
/// (`video::id[youtube]`) embedded from its host, any other a `<video>` or `<audio>` with the
/// block's `width`, `height`, `poster`, `start` and `end` and its options.
fn media_element(kind: MediaKind, tag: &str, targets: &[String], block: &ViewBlock) -> String {
    let attribute = |name: &str| block.attributes.get(name).map(String::as_str);
    let option = |name: &str| block.options.contains(name);
    // The host is the first positional attribute, which may also be named `poster`.
    let host = ["youtube", "vimeo"]
        .into_iter()
        .find(|host| block.attributes.contains_key(*host) || attribute("poster") == Some(host))
        .filter(|_| kind == MediaKind::Video);
    let mut size = String::new();
    for name in ["width", "height"] {
        if let Some(value) = attribute(name) {
            let _ = write!(size, " {name}=\"{}\"", escape(value));
        }
    }
    let fullscreen = if option("nofullscreen") {
        ""
    } else {
        " allowfullscreen"
    };
    match host {
        Some("youtube") => {
            let video = targets.first().map(String::as_str).unwrap_or("");
            let mut query = format!("rel={}", u8::from(option("related")));
            for name in ["start", "end"] {
                if let Some(value) = attribute(name) {
                    let _ = write!(query, "&{name}={value}");
                }
            }
            for (name, parameter) in [
                ("autoplay", "&autoplay=1"),
                ("loop", "&loop=1"),
                ("muted", "&mute=1"),
                ("nocontrols", "&controls=0"),
            ] {
                if option(name) {
                    query.push_str(parameter);
                }
            }
            // More videos, or a loop, play as a playlist.
            if targets.len() > 1 || option("loop") {
                let _ = write!(query, "&playlist={}", targets.join(","));
            }
            if option("nofullscreen") {
                query.push_str("&fs=0");
            }
            format!(
                "<iframe{size} src=\"{}\" frameborder=\"0\"{fullscreen}></iframe>",
                escape(&format!("https://www.youtube.com/embed/{video}?{query}"))
            )
        }
        Some(_) => {
            let video = targets.first().map(String::as_str).unwrap_or("");
            let parameters: Vec<&str> = [
                ("autoplay", "autoplay=1"),
                ("loop", "loop=1"),
                ("muted", "muted=1"),
            ]
            .into_iter()
            .filter(|(name, _)| option(name))
            .map(|(_, parameter)| parameter)
            .collect();
            let mut address = format!("https://player.vimeo.com/video/{video}");
            if !parameters.is_empty() {
                let _ = write!(address, "?{}", parameters.join("&"));
            }
            if let Some(start) = attribute("start") {
                let _ = write!(address, "#at={start}");
            }
            format!(
                "<iframe{size} src=\"{}\" frameborder=\"0\"{fullscreen}></iframe>",
                escape(&address)
            )
        }
        None => {
            let mut source = targets.first().cloned().unwrap_or_default();
            let (start, end) = (attribute("start"), attribute("end"));
            if start.is_some() || end.is_some() {
                let _ = write!(source, "#t={}", start.unwrap_or(""));
                if let Some(end) = end {
                    let _ = write!(source, ",{end}");
                }
            }
            let mut element = format!("<{tag} src=\"{}\"", escape(&source));
            if kind == MediaKind::Video {
                element.push_str(&size);
                if let Some(poster) = attribute("poster").filter(|poster| !poster.is_empty()) {
                    let _ = write!(element, " poster=\"{}\"", escape(poster));
                }
            }
            for (name, shown) in [
                ("autoplay", option("autoplay")),
                ("muted", kind == MediaKind::Video && option("muted")),
                ("controls", !option("nocontrols")),
                ("loop", option("loop")),
            ] {
                if shown {
                    let _ = write!(element, " {name}");
                }
            }
            let _ = write!(element, "></{tag}>");
            element
        }
    }
}

/// An ordered list's `class`, `type`, `start` and `reversed`, as Asciidoctor writes them.
fn ordered_list_attributes(style: &str, block: &ViewBlock) -> String {
    let mut attributes = format!(" class=\"{}\"", escape(style));
    let marker = match style {
        "loweralpha" => Some("a"),
        "upperalpha" => Some("A"),
        "lowerroman" => Some("i"),
        "upperroman" => Some("I"),
        _ => None,
    };
    if let Some(marker) = marker {
        let _ = write!(attributes, " type=\"{marker}\"");
    }
    if let Some(start) = block.attributes.get("start") {
        let _ = write!(attributes, " start=\"{}\"", escape(start));
    }
    if block.options.contains("reversed") {
        attributes.push_str(" reversed");
    }
    attributes
}

/// A table's classes and any `style` attribute, as Asciidoctor gives them: its frame, grid and
/// stripes, then `fit-content` for an automatic width, `stretch` for the full width, or else
/// the width as a style, then a float.
fn table_presentation(block: &ViewBlock) -> (String, String) {
    let attribute = |name: &str| block.attributes.get(name).map(String::as_str);
    let frame = match attribute("frame").unwrap_or("all") {
        "topbot" => "ends",
        frame => frame,
    };
    let grid = attribute("grid").unwrap_or("all");
    let mut class = format!("tableblock frame-{} grid-{}", escape(frame), escape(grid));
    if let Some(stripes) = attribute("stripes") {
        let _ = write!(class, " stripes-{}", escape(stripes));
    }
    let width = attribute("width").map(|width| width.trim_end_matches('%'));
    let mut style = String::new();
    match width {
        None if block.options.contains("autowidth") => class.push_str(" fit-content"),
        None | Some("100") => class.push_str(" stretch"),
        Some(width) => style = format!(" style=\"width: {}%;\"", escape(width)),
    }
    if let Some(float) = attribute("float") {
        let _ = write!(class, " {}", escape(float));
    }
    (class, style)
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

/// Where an xref leads: `#id` in the page; for a page that resolves, its published address
/// (`HtmlOptions::page_urls`), else the path from this page's export to that page's, each
/// beside its source; for one that does not, the target as written with a trailing `.adoc` made
/// `.html`. None for an unresolved Antora resource id, which no address could stand for (a
/// browser would read `security:` as a URL scheme).
fn xref_href(target: &str, link: &ResolvedLink, page: Option<&Page<'_>>) -> Option<String> {
    let (file, fragment) = match target.split_once('#') {
        Some((file, fragment)) => (file, Some(fragment)),
        None => (target, None),
    };
    if let Some(fragment) = &link.fragment {
        if !file.contains(".adoc") && !file.contains('$') {
            return Some(format!("#{fragment}"));
        }
    }
    let with_fragment = |mut href: String| {
        if let Some(fragment) = fragment {
            href.push('#');
            href.push_str(fragment);
        }
        href
    };
    if let (Some(location), Some(page)) = (&link.target, page) {
        if let Some(url) = page.options.page_urls.get(&location.path) {
            return Some(with_fragment(url.clone()));
        }
        let from = page.view.source.parent().unwrap_or(Path::new(""));
        return Some(with_fragment(relative_href(
            from,
            &location.path.with_extension("html"),
        )));
    }
    if file.contains(['$', ':', '@']) {
        return None;
    }
    let file = match file.strip_suffix(".adoc") {
        Some(stem) => format!("{stem}.html"),
        None => file.to_owned(),
    };
    Some(with_fragment(file))
}

/// `to` relative to the folder `from`, with `/` separators.
fn relative_href(from: &Path, to: &Path) -> String {
    let parts = |path: &Path| -> Vec<String> {
        path.components()
            .filter(|component| !matches!(component, Component::CurDir))
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    let (from, to) = (parts(from), parts(to));
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(from, to)| from == to)
        .count();
    let mut href: Vec<String> = vec!["..".to_owned(); from.len() - shared];
    href.extend(to[shared..].iter().cloned());
    href.join("/")
}

/// HTML for a run of inlines.
#[must_use]
pub fn inlines(content: &[ViewInline]) -> String {
    inlines_with(content, None)
}

/// Whether the page asks for font icons (`icons=font`), which Asciidoctor writes as Font
/// Awesome's `<i>` elements for the page's stylesheet to draw.
fn font_icons(view: &ViewTree) -> bool {
    view.attributes
        .get("icons")
        .is_some_and(|icons| icons == "font")
}

/// HTML for a run of inlines on the page `view`.
fn page_inlines(view: &Page<'_>, content: &[ViewInline]) -> String {
    inlines_with(content, Some(view))
}

fn inlines_with(content: &[ViewInline], page: Option<&Page<'_>>) -> String {
    let mut out = String::new();
    for inline in content {
        inline_html(&mut out, inline, page);
    }
    out
}

fn inline_html(out: &mut String, inline: &ViewInline, page: Option<&Page<'_>>) {
    let wrap = |out: &mut String, tag: &str, content: &[ViewInline]| {
        let _ = write!(out, "<{tag}>{}</{tag}>", inlines_with(content, page));
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
                inlines_with(content, page)
            );
        }
        ViewInline::Link { url, content } => {
            let _ = write!(
                out,
                "<a href=\"{}\" class=\"bare\">{}</a>",
                escape(url),
                inlines_with(content, page)
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
            let href = xref_href(target, link, page)
                .map(|href| format!(" href=\"{}\"", escape(&href)))
                .unwrap_or_default();
            let _ = write!(out, "<a{href}{class}>{}</a>", inlines_with(content, page));
        }
        ViewInline::Anchor(id) => {
            let _ = write!(out, "<a id=\"{}\"></a>", escape(id));
        }
        ViewInline::Image(image) => {
            let _ = write!(out, "<span class=\"image\">{}</span>", img(image));
        }
        ViewInline::Icon(name) if page.is_some_and(|page| font_icons(page)) => {
            let _ = write!(
                out,
                "<span class=\"icon\"><i class=\"fa fa-{}\"></i></span>",
                escape(name)
            );
        }
        ViewInline::Icon(name) => {
            let _ = write!(out, "<span class=\"icon\">[{}]</span>", escape(name));
        }
        ViewInline::FootnoteRef {
            number,
            repeat: true,
            ..
        } => {
            let _ = write!(
                out,
                "<sup class=\"footnoteref\">[<a class=\"footnote\" href=\"#_footnotedef_{number}\">{number}</a>]</sup>"
            );
        }
        ViewInline::FootnoteRef { number, name, .. } => {
            let id = name
                .as_deref()
                .map(|name| format!(" id=\"_footnote_{}\"", escape(name)))
                .unwrap_or_default();
            let _ = write!(
                out,
                "<sup class=\"footnote\"{id}>[<a id=\"_footnoteref_{number}\" class=\"footnote\" href=\"#_footnotedef_{number}\">{number}</a>]</sup>"
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
        ViewInline::Stem {
            rendered: Some(image),
            source,
            ..
        } => {
            let _ = write!(
                out,
                "<img class=\"stem\" src=\"{}\" alt=\"{}\">",
                escape(&file_url(&image.path)),
                escape(source.trim())
            );
        }
        ViewInline::Stem { latex, source, .. } => {
            let (open, close) = if *latex {
                ("\\(", "\\)")
            } else {
                ("\\$", "\\$")
            };
            let _ = write!(out, "{open}{}{close}", escape(source));
        }
        ViewInline::Raw(text) => out.push_str(text),
        // Asciidoctor's HTML shows a flow term's text and nothing else.
        ViewInline::IndexTerm { content, .. } => out.push_str(&inlines_with(content, page)),
    }
}

/// `html` with every local image and diagram (`src="file://…"`) embedded as a `data:` URI, so
/// the page opens anywhere: in another folder, on another machine or attached to a message.
/// A file that cannot be read, or is not an image, keeps its `file://` address.
#[must_use]
pub fn embed_local_files(html: &str) -> String {
    const START: &str = "src=\"file://";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(found) = rest.find(START) {
        let address_start = found + "src=\"".len();
        let Some(length) = rest[address_start..].find('"') else {
            break;
        };
        let address = &rest[address_start..address_start + length];
        out.push_str(&rest[..address_start]);
        match data_uri(&unescape(address)) {
            Some(data) => out.push_str(&data),
            None => out.push_str(address),
        }
        rest = &rest[address_start + length..];
    }
    out.push_str(rest);
    out
}

/// The file at a `file://` address as a `data:` URI, when it is an image that can be read.
fn data_uri(address: &str) -> Option<String> {
    use base64::Engine as _;
    let path = url::Url::parse(address).ok()?.to_file_path().ok()?;
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let media_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => return None,
    };
    let bytes = std::fs::read(&path).ok()?;
    Some(format!(
        "data:{media_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Undoes [`escape`].
fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
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

#[cfg(test)]
mod tests {
    use super::*;

    use adoc_core::{Location, SourceRange};

    fn resolved(path: &str) -> ResolvedLink {
        ResolvedLink {
            target: Some(Location {
                path: std::path::PathBuf::from(path),
                range: SourceRange::new(0, 0),
            }),
            ..ResolvedLink::default()
        }
    }

    fn unresolved() -> ResolvedLink {
        ResolvedLink {
            unresolved: true,
            ..ResolvedLink::default()
        }
    }

    /// The `href` of each `(target, link)` written on the page at `source`.
    fn hrefs(
        source: &str,
        options: &HtmlOptions,
        links: &[(&str, ResolvedLink)],
    ) -> Vec<Option<String>> {
        let view = ViewTree {
            source: std::path::PathBuf::from(source),
            ..ViewTree::default()
        };
        let page = Page {
            view: &view,
            options,
        };
        links
            .iter()
            .map(|(target, link)| xref_href(target, link, Some(&page)))
            .collect()
    }

    #[test]
    fn cross_page_xrefs_lead_to_where_the_page_is_exported() {
        let source = "/docs/modules/ROOT/pages/index.adoc";
        let found = hrefs(
            source,
            &HtmlOptions::default(),
            &[
                (
                    "security:authentication.adoc",
                    resolved("/docs/modules/security/pages/authentication.adoc"),
                ),
                (
                    "2.0@api:ROOT:index.adoc#start",
                    resolved("/api/modules/ROOT/pages/index.adoc"),
                ),
                (
                    "other.adoc",
                    resolved("/docs/modules/ROOT/pages/other.adoc"),
                ),
                (
                    "notes.adoc.adoc#x",
                    resolved("/docs/modules/ROOT/pages/notes.adoc.adoc"),
                ),
                (
                    "guide/setup.adoc",
                    resolved("/docs/modules/ROOT/pages/guide/setup.adoc"),
                ),
            ],
        );
        assert_eq!(
            found,
            [
                Some("../../security/pages/authentication.html".to_owned()),
                Some("../../../../api/modules/ROOT/pages/index.html#start".to_owned()),
                Some("other.html".to_owned()),
                Some("notes.adoc.html#x".to_owned()),
                Some("guide/setup.html".to_owned()),
            ]
        );
    }

    #[test]
    fn with_published_addresses_cross_page_xrefs_lead_there() {
        let target = "/docs/modules/security/pages/authentication.adoc";
        let options = HtmlOptions {
            page_urls: std::collections::HashMap::from([(
                std::path::PathBuf::from(target),
                "https://docs.example.com/guide/security/authentication.html".to_owned(),
            )]),
            ..HtmlOptions::default()
        };
        assert_eq!(
            hrefs(
                "/docs/modules/ROOT/pages/index.adoc",
                &options,
                &[("security:authentication.adoc#tokens", resolved(target))],
            ),
            [Some(
                "https://docs.example.com/guide/security/authentication.html#tokens".to_owned()
            )]
        );
    }

    #[test]
    fn unresolved_xrefs_never_become_a_scheme_or_lose_inner_adoc() {
        assert_eq!(
            hrefs(
                "/docs/index.adoc",
                &HtmlOptions::default(),
                &[
                    ("security:missing.adoc", unresolved()),
                    ("2.0@api::index.adoc", unresolved()),
                    ("partial$note.adoc", unresolved()),
                    ("drafts.adoc/missing.adoc#x", unresolved()),
                    (
                        "#local",
                        ResolvedLink {
                            fragment: Some("local".to_owned()),
                            ..ResolvedLink::default()
                        }
                    ),
                ],
            ),
            [
                None,
                None,
                None,
                Some("drafts.adoc/missing.html#x".to_owned()),
                Some("#local".to_owned()),
            ]
        );
    }

    fn file_src(path: &std::path::Path) -> String {
        format!("<img src=\"{}\" alt=\"x\">", escape(&file_url(path)))
    }

    #[test]
    fn local_images_are_embedded() {
        let directory = tempfile::tempdir().unwrap();
        let png = directory.path().join("a & b.png");
        std::fs::write(&png, [0x89, b'P', b'N', b'G']).unwrap();
        let svg = directory.path().join("diagram.svg");
        std::fs::write(&svg, "<svg/>").unwrap();
        let html = format!(
            "<p>{}</p>\n<p>{}</p>\n<img src=\"https://example.com/remote.png\">",
            file_src(&png),
            file_src(&svg)
        );

        let embedded = embed_local_files(&html);
        assert!(embedded.contains("<img src=\"data:image/png;base64,iVBORw==\" alt=\"x\">"));
        assert!(embedded.contains("src=\"data:image/svg+xml;base64,PHN2Zy8+\""));
        assert!(embedded.contains("src=\"https://example.com/remote.png\""));
        assert!(!embedded.contains("file://"), "{embedded}");
    }

    #[test]
    fn files_that_cannot_be_embedded_keep_their_address() {
        let directory = tempfile::tempdir().unwrap();
        let missing = file_src(&directory.path().join("missing.png"));
        let text = directory.path().join("notes.txt");
        std::fs::write(&text, "not an image").unwrap();
        let other = file_src(&text);
        assert_eq!(embed_local_files(&missing), missing);
        assert_eq!(embed_local_files(&other), other);
        assert_eq!(
            embed_local_files("src=\"file://unclosed"),
            "src=\"file://unclosed"
        );
    }
}
