//! The document's body: the view tree walked into Word paragraphs. Titles and headings take the
//! Title, Subtitle and heading styles; paragraphs are editable text with their line breaks.
//! Blocks with a Word structure of their own (lists, tables, code, images and the rest) are, for
//! now, written as plain paragraphs of their text and reported, so nothing is lost silently.

use adoc_dom::{ListKind, SourceSpan};
use adoc_render::{ViewBlock, ViewInline, ViewKind, ViewTree};

use crate::{
    parts::DEEPEST_HEADING, xml::Xml, BookItem, DocxDiagnostic, DocxDocument, DocxPage,
    DocxSeverity,
};

pub(crate) struct Body {
    xml: Xml,
    pub(crate) diagnostics: Vec<DocxDiagnostic>,
}

/// A paragraph's style and whether it starts a page.
#[derive(Clone, Copy, Default)]
struct Style<'a> {
    id: Option<&'a str>,
    page_break_before: bool,
}

fn heading(level: u8) -> String {
    format!("Heading{}", level.clamp(1, DEEPEST_HEADING))
}

impl Body {
    pub(crate) fn new() -> Self {
        Self {
            xml: Xml::fragment(),
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn into_xml(self) -> Xml {
        self.xml
    }

    pub(crate) fn document(&mut self, document: &DocxDocument) {
        match document {
            DocxDocument::Article(page) => {
                if let Some(title) = page_title(&page.view) {
                    self.paragraph(
                        Style {
                            id: Some("Title"),
                            ..Style::default()
                        },
                        &title,
                    );
                }
                self.page(page, page.depth);
            }
            DocxDocument::Book(book) => {
                if !book.title.trim().is_empty() {
                    self.paragraph(
                        Style {
                            id: Some("Title"),
                            ..Style::default()
                        },
                        &book.title,
                    );
                }
                if let Some(subtitle) = book.subtitle.as_deref().filter(|s| !s.trim().is_empty()) {
                    self.paragraph(
                        Style {
                            id: Some("Subtitle"),
                            ..Style::default()
                        },
                        subtitle,
                    );
                }
                // A part is a level above the chapters, so where there are parts everything
                // else moves down one.
                let shift = u8::from(
                    book.items
                        .iter()
                        .any(|item| matches!(item, BookItem::Part(_))),
                );
                for item in &book.items {
                    match item {
                        BookItem::Part(title) => self.paragraph(
                            Style {
                                id: Some(&heading(1)),
                                page_break_before: true,
                            },
                            title,
                        ),
                        BookItem::Heading { title, depth } => self.paragraph(
                            Style {
                                id: Some(&heading(depth.saturating_add(shift))),
                                ..Style::default()
                            },
                            title,
                        ),
                        BookItem::Page(page) => {
                            let depth = page.depth.max(1).saturating_add(shift);
                            if let Some(title) = page_title(&page.view) {
                                self.paragraph(
                                    Style {
                                        id: Some(&heading(depth)),
                                        ..Style::default()
                                    },
                                    &title,
                                );
                            }
                            self.page(page, depth);
                        }
                    }
                }
            }
        }
    }

    /// A page's blocks, its sections `depth` levels down, then its footnotes.
    fn page(&mut self, page: &DocxPage, depth: u8) {
        for block in &page.view.blocks {
            self.block(block, depth);
        }
        if !page.view.footnotes.is_empty() {
            for footnote in &page.view.footnotes {
                let text = format!(
                    "[{}] {}",
                    footnote.number,
                    ViewInline::plain_text(&footnote.content)
                );
                self.paragraph(Style::default(), &text);
            }
            self.warn(
                None,
                format!(
                    "the footnotes of {} are exported as text at the end of the page",
                    page.view.source.display()
                ),
            );
        }
    }

    fn block(&mut self, block: &ViewBlock, depth: u8) {
        let title = ViewInline::plain_text(&block.title);
        let titled = |body: &mut Self| {
            if !title.trim().is_empty() {
                let caption = match &block.caption {
                    Some(caption) => format!("{caption} {title}"),
                    None => title.clone(),
                };
                body.paragraph(Style::default(), &caption);
            }
        };
        match &block.kind {
            ViewKind::Section {
                level,
                title,
                blocks,
                number,
            } => {
                let title = ViewInline::plain_text(title);
                let title = match number {
                    Some(number) => format!("{number} {title}"),
                    None => title,
                };
                self.paragraph(
                    Style {
                        id: Some(&heading(level.saturating_add(depth))),
                        ..Style::default()
                    },
                    &title,
                );
                for block in blocks {
                    self.block(block, depth);
                }
            }
            ViewKind::Heading { level, title } => self.paragraph(
                Style {
                    id: Some(&heading(level.saturating_add(depth))),
                    ..Style::default()
                },
                &ViewInline::plain_text(title),
            ),
            ViewKind::Paragraph(content) => {
                titled(self);
                self.paragraph(Style::default(), &ViewInline::plain_text(content));
            }
            ViewKind::ThematicBreak => {
                self.xml.open("w:p", &[]);
                self.xml.open("w:pPr", &[]);
                self.xml.open("w:pBdr", &[]);
                self.xml.empty(
                    "w:bottom",
                    &[
                        ("w:val", "single"),
                        ("w:sz", "6"),
                        ("w:space", "1"),
                        ("w:color", "auto"),
                    ],
                );
                self.xml.close("w:pBdr");
                self.xml.close("w:pPr");
                self.xml.close("w:p");
            }
            ViewKind::PageBreak => {
                self.xml.open("w:p", &[]);
                self.xml.open("w:r", &[]);
                self.xml.empty("w:br", &[("w:type", "page")]);
                self.xml.close("w:r");
                self.xml.close("w:p");
            }
            ViewKind::Toc => self.warn(
                Some(&block.span),
                "a table of contents is not exported to Word".to_owned(),
            ),
            kind => {
                titled(self);
                let mut lines = Vec::new();
                plain(kind, &mut lines);
                for line in &lines {
                    self.paragraph(Style::default(), line);
                }
                self.warn(
                    Some(&block.span),
                    format!("{} is exported as plain text", describe(kind)),
                );
            }
        }
    }

    fn warn(&mut self, span: Option<&SourceSpan>, message: String) {
        self.diagnostics.push(DocxDiagnostic {
            severity: DocxSeverity::Warning,
            message,
            source: span.cloned(),
        });
    }

    /// A paragraph of `text` in one run, its tabs as Word tabs and each line ending a break.
    fn paragraph(&mut self, style: Style<'_>, text: &str) {
        self.xml.open("w:p", &[]);
        if style.id.is_some() || style.page_break_before {
            self.xml.open("w:pPr", &[]);
            if let Some(id) = style.id {
                self.xml.empty("w:pStyle", &[("w:val", id)]);
            }
            if style.page_break_before {
                self.xml.empty("w:pageBreakBefore", &[]);
            }
            self.xml.close("w:pPr");
        }
        if !text.is_empty() {
            self.xml.open("w:r", &[]);
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            for (line_number, line) in text.split('\n').enumerate() {
                if line_number > 0 {
                    self.xml.empty("w:br", &[]);
                }
                for (piece_number, piece) in line.split('\t').enumerate() {
                    if piece_number > 0 {
                        self.xml.empty("w:tab", &[]);
                    }
                    if !piece.is_empty() {
                        self.xml.element("w:t", &[("xml:space", "preserve")], piece);
                    }
                }
            }
            self.xml.close("w:r");
        }
        self.xml.close("w:p");
    }
}

/// A page's title as text, when it has one that is not blank.
fn page_title(view: &ViewTree) -> Option<String> {
    view.title
        .as_deref()
        .map(ViewInline::plain_text)
        .filter(|title| !title.trim().is_empty())
}

/// What a block is, for a diagnostic.
fn describe(kind: &ViewKind) -> &'static str {
    match kind {
        ViewKind::List { .. } => "a list",
        ViewKind::DescriptionList(_) => "a description list",
        ViewKind::CalloutList(_) => "a callout list",
        ViewKind::Admonition { .. } => "an admonition",
        ViewKind::Code { .. } => "a source block",
        ViewKind::Literal(_) => "a literal block",
        ViewKind::Quote { .. } => "a quote",
        ViewKind::Verse { .. } => "a verse",
        ViewKind::Sidebar(_) => "a sidebar",
        ViewKind::Example(_) => "an example",
        ViewKind::Open(_) => "an open block",
        ViewKind::Table(_) => "a table",
        ViewKind::Image(_) => "an image",
        ViewKind::Media { .. } => "a video or audio block",
        ViewKind::Diagram(_) => "a diagram",
        ViewKind::Stem { .. } => "a formula",
        ViewKind::Passthrough(_) => "a passthrough block",
        ViewKind::Unsupported { .. } => "an unsupported block",
        ViewKind::Section { .. }
        | ViewKind::Heading { .. }
        | ViewKind::Paragraph(_)
        | ViewKind::ThematicBreak
        | ViewKind::PageBreak
        | ViewKind::Toc => "a block",
    }
}

/// The text of a block, one line per paragraph, with list markers and table cells kept
/// readable.
fn plain(kind: &ViewKind, lines: &mut Vec<String>) {
    let blocks = |blocks: &[ViewBlock], lines: &mut Vec<String>| {
        for block in blocks {
            let title = ViewInline::plain_text(&block.title);
            if !title.trim().is_empty() {
                lines.push(title);
            }
            plain(&block.kind, lines);
        }
    };
    match kind {
        ViewKind::Section {
            title,
            blocks: inner,
            ..
        } => {
            lines.push(ViewInline::plain_text(title));
            blocks(inner, lines);
        }
        ViewKind::Heading { title, .. } => lines.push(ViewInline::plain_text(title)),
        ViewKind::Paragraph(content) => lines.push(ViewInline::plain_text(content)),
        ViewKind::List { kind, items } => {
            for (number, item) in items.iter().enumerate() {
                let marker = match (kind, item.checked) {
                    (_, Some(true)) => "☑ ".to_owned(),
                    (_, Some(false)) => "☐ ".to_owned(),
                    (ListKind::Ordered, None) => format!("{}. ", number + 1),
                    (_, None) => "• ".to_owned(),
                };
                lines.push(format!("{marker}{}", ViewInline::plain_text(&item.content)));
                blocks(&item.blocks, lines);
            }
        }
        ViewKind::DescriptionList(items) => {
            for item in items {
                lines.push(ViewInline::plain_text(&item.term));
                let content = ViewInline::plain_text(&item.content);
                if !content.is_empty() {
                    lines.push(content);
                }
                blocks(&item.blocks, lines);
            }
        }
        ViewKind::CalloutList(items) => {
            for item in items {
                lines.push(format!(
                    "({}) {}",
                    item.number,
                    ViewInline::plain_text(&item.content)
                ));
                blocks(&item.blocks, lines);
            }
        }
        ViewKind::Admonition {
            kind,
            blocks: inner,
        } => {
            lines.push(format!("{kind:?}").to_uppercase());
            blocks(inner, lines);
        }
        ViewKind::Code { text, .. }
        | ViewKind::Literal(text)
        | ViewKind::Verse { text, .. }
        | ViewKind::Passthrough(text) => lines.push(text.clone()),
        ViewKind::Quote { blocks: inner, .. }
        | ViewKind::Sidebar(inner)
        | ViewKind::Example(inner)
        | ViewKind::Open(inner) => blocks(inner, lines),
        ViewKind::Table(table) => {
            let rows = table
                .header
                .iter()
                .chain(&table.rows)
                .chain(table.footer.iter());
            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| {
                        let mut text = Vec::new();
                        blocks(&cell.blocks, &mut text);
                        text.join(" ")
                    })
                    .collect();
                lines.push(cells.join("\t"));
            }
        }
        ViewKind::Image(image) => lines.push(if image.alt.is_empty() {
            image.target.clone()
        } else {
            image.alt.clone()
        }),
        ViewKind::Media { targets, .. } => lines.push(targets.join(", ")),
        ViewKind::Diagram(diagram) => match diagram {
            adoc_render::DiagramView::Svg(_) => lines.push("[diagram]".to_owned()),
            adoc_render::DiagramView::Pending { source, .. }
            | adoc_render::DiagramView::Failed { source, .. } => lines.push(source.clone()),
        },
        ViewKind::Stem { source, .. } => lines.push(source.clone()),
        ViewKind::Unsupported { source, .. } => lines.push(source.clone()),
        ViewKind::ThematicBreak | ViewKind::PageBreak | ViewKind::Toc => {}
    }
}
