//! A chapter as an XHTML content document: semantic markup a reading system can reflow,
//! restyle and read aloud. Every element is the crate's own choice and every document string is
//! text, so nothing a page holds becomes markup, script or a link out of the book that was not
//! a web address.

use adoc_ebook::{
    AsideKind, Block, Book, Cell, Chapter, Inline, InlineStyle, Link, Note, ParagraphClass, Row,
    Target,
};

use crate::xml::Xml;

/// The file a chapter is written to, relative to the folder the chapters share.
pub(crate) fn chapter_name(chapter: &Chapter) -> String {
    format!("{}.xhtml", chapter.id)
}

/// The content document of `book`'s chapter `index`.
pub(crate) fn chapter(book: &Book, index: usize) -> Vec<u8> {
    let chapter = &book.chapters[index];
    let title = if chapter.title.trim().is_empty() {
        &book.metadata.title
    } else {
        &chapter.title
    };
    let mut writer = Writer {
        book,
        chapter: index,
        xml: Xml::xhtml(&book.metadata.language, title),
        in_link: false,
    };
    writer.xml.open("body", &[]);
    writer.xml.newline();
    writer
        .xml
        .open("section", &[("epub:type", "chapter"), ("id", &chapter.id)]);
    writer.xml.newline();
    writer.blocks(&chapter.blocks);
    writer.xml.close("section");
    writer.xml.newline();
    writer.xml.close("body");
    writer.xml.close("html");
    writer.xml.newline();
    writer.xml.into_bytes()
}

struct Writer<'a> {
    book: &'a Book,
    /// The chapter being written.
    chapter: usize,
    xml: Xml,
    /// Whether the inlines being written are inside a link, where no other link may start.
    in_link: bool,
}

/// `anchor` as an `id` attribute, if there is one.
fn id(anchor: Option<&str>) -> Option<(&'static str, &str)> {
    anchor.map(|anchor| ("id", anchor))
}

impl Writer<'_> {
    /// Where `target` is, from this chapter: a fragment in it, or another chapter's file.
    fn href(&self, target: &Target) -> String {
        let mut href = String::new();
        if target.chapter != self.chapter || target.anchor.is_none() {
            if let Some(chapter) = self.book.chapters.get(target.chapter) {
                href.push_str(&chapter_name(chapter));
            }
        }
        if let Some(anchor) = &target.anchor {
            href.push('#');
            href.push_str(anchor);
        }
        href
    }

    fn blocks(&mut self, blocks: &[Block]) {
        for block in blocks {
            self.block(block);
        }
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Heading {
                level,
                anchor,
                content,
            } => {
                let name = format!("h{}", (*level).clamp(1, 6));
                self.xml.open(&name, id(anchor.as_deref()).as_slice());
                self.inlines(content);
                self.xml.close(&name);
            }
            Block::Paragraph {
                anchor,
                class,
                content,
            } => {
                let class = match class {
                    ParagraphClass::Normal => None,
                    ParagraphClass::Caption => Some("caption"),
                    ParagraphClass::Attribution => Some("attribution"),
                    ParagraphClass::Label => Some("label"),
                };
                let mut attributes: Vec<(&str, &str)> = id(anchor.as_deref()).into_iter().collect();
                attributes.extend(class.map(|class| ("class", class)));
                self.xml.open("p", &attributes);
                self.inlines(content);
                self.xml.close("p");
            }
            Block::Preformatted { anchor, text, code } => {
                let mut attributes: Vec<(&str, &str)> = id(anchor.as_deref()).into_iter().collect();
                attributes.push(("class", if *code { "listing" } else { "verse" }));
                self.xml.open("pre", &attributes);
                if *code {
                    self.xml.element("code", &[], text);
                } else {
                    self.xml.text(text);
                }
                self.xml.close("pre");
            }
            Block::List {
                anchor,
                ordered,
                items,
            } => {
                let name = if *ordered { "ol" } else { "ul" };
                self.xml.open(name, id(anchor.as_deref()).as_slice());
                self.xml.newline();
                for item in items {
                    self.xml.open("li", &[]);
                    self.blocks(item);
                    self.xml.close("li");
                    self.xml.newline();
                }
                self.xml.close(name);
            }
            Block::Definitions { anchor, items } => {
                self.xml.open("dl", id(anchor.as_deref()).as_slice());
                self.xml.newline();
                for (term, description) in items {
                    self.xml.open("dt", &[]);
                    self.inlines(term);
                    self.xml.close("dt");
                    self.xml.newline();
                    self.xml.open("dd", &[]);
                    self.blocks(description);
                    self.xml.close("dd");
                    self.xml.newline();
                }
                self.xml.close("dl");
            }
            Block::Quote { anchor, blocks } => {
                self.xml
                    .open("blockquote", id(anchor.as_deref()).as_slice());
                self.xml.newline();
                self.blocks(blocks);
                self.xml.close("blockquote");
            }
            Block::Aside {
                anchor,
                kind,
                blocks,
            } => {
                // A `div`, not an `aside`: some reading systems hide an `aside` from the text,
                // taking it for a note.
                let class = match kind {
                    AsideKind::Admonition(name) => format!("admonition {name}"),
                    AsideKind::Sidebar => "sidebar".to_owned(),
                    AsideKind::Example => "example".to_owned(),
                };
                let mut attributes: Vec<(&str, &str)> = id(anchor.as_deref()).into_iter().collect();
                attributes.push(("class", &class));
                if matches!(kind, AsideKind::Admonition(_)) {
                    attributes.push(("role", "note"));
                }
                self.xml.open("div", &attributes);
                self.xml.newline();
                self.blocks(blocks);
                self.xml.close("div");
            }
            Block::Table { anchor, rows } => self.table(anchor.as_deref(), rows),
            Block::Figure { anchor, alt, .. } => {
                // Pictures are embedded with EPUB 5/9; until then, their description.
                let mut attributes: Vec<(&str, &str)> = id(anchor.as_deref()).into_iter().collect();
                attributes.push(("class", "figure"));
                self.xml.open("div", &attributes);
                self.xml.element("p", &[], alt);
                self.xml.close("div");
            }
            Block::Rule => self.xml.empty("hr", &[]),
            // A separator, not a page: a reflowable book has none (PRD §5.1).
            Block::PageBreak => self.xml.empty("hr", &[("class", "page-break")]),
            Block::Notes(notes) => self.notes(notes),
        }
        self.xml.newline();
    }

    fn table(&mut self, anchor: Option<&str>, rows: &[Row]) {
        self.xml.open("table", id(anchor).as_slice());
        self.xml.newline();
        let header: Vec<&Row> = rows.iter().filter(|row| row.header).collect();
        let body: Vec<&Row> = rows
            .iter()
            .filter(|row| !row.header && !row.footer)
            .collect();
        let footer: Vec<&Row> = rows
            .iter()
            .filter(|row| row.footer && !row.header)
            .collect();
        for (group, rows) in [("thead", header), ("tbody", body), ("tfoot", footer)] {
            if rows.is_empty() {
                continue;
            }
            self.xml.open(group, &[]);
            self.xml.newline();
            for row in rows {
                self.xml.open("tr", &[]);
                for cell in &row.cells {
                    self.cell(cell, row.header);
                }
                self.xml.close("tr");
                self.xml.newline();
            }
            self.xml.close(group);
            self.xml.newline();
        }
        self.xml.close("table");
    }

    fn cell(&mut self, cell: &Cell, header: bool) {
        let name = if header { "th" } else { "td" };
        let colspan = cell.colspan.to_string();
        let rowspan = cell.rowspan.to_string();
        let mut attributes: Vec<(&str, &str)> = Vec::new();
        if header {
            attributes.push(("scope", "col"));
        }
        if cell.colspan > 1 {
            attributes.push(("colspan", &colspan));
        }
        if cell.rowspan > 1 {
            attributes.push(("rowspan", &rowspan));
        }
        self.xml.open(name, &attributes);
        self.blocks(&cell.blocks);
        self.xml.close(name);
    }

    /// The chapter's footnotes: each a note its marks lead to, leading back to its first mark.
    fn notes(&mut self, notes: &[Note]) {
        self.xml.open(
            "section",
            &[("epub:type", "footnotes"), ("class", "footnotes")],
        );
        self.xml.newline();
        for note in notes {
            self.xml
                .open("aside", &[("epub:type", "footnote"), ("id", &note.anchor)]);
            self.xml.open("p", &[]);
            let number = note.number.to_string();
            match &note.back {
                Some(back) => {
                    let href = format!("#{back}");
                    self.xml.element("a", &[("href", &href)], &number);
                }
                None => self.xml.text(&number),
            }
            self.xml.text(". ");
            self.inlines(&note.content);
            self.xml.close("p");
            self.xml.close("aside");
            self.xml.newline();
        }
        self.xml.close("section");
    }

    fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(inline);
        }
    }

    /// `name` holding `content`.
    fn wrapped(&mut self, name: &str, attributes: &[(&str, &str)], content: &[Inline]) {
        self.xml.open(name, attributes);
        self.inlines(content);
        self.xml.close(name);
    }

    fn inline(&mut self, inline: &Inline) {
        match inline {
            Inline::Text(text) => self.xml.text(text),
            Inline::Strong(content) => self.wrapped("strong", &[], content),
            Inline::Emphasis(content) => self.wrapped("em", &[], content),
            Inline::Code(content) => self.wrapped("code", &[], content),
            Inline::Mark(content) => self.wrapped("mark", &[], content),
            Inline::Superscript(content) => self.wrapped("sup", &[], content),
            Inline::Subscript(content) => self.wrapped("sub", &[], content),
            Inline::Styled { style, content } => match style {
                InlineStyle::Keyboard => self.wrapped("kbd", &[], content),
                InlineStyle::Menu => self.wrapped("span", &[("class", "menu")], content),
                InlineStyle::Button => self.wrapped("span", &[("class", "button")], content),
            },
            // A link inside a link is not XHTML: the inner one is its text.
            Inline::Link { content, .. } if self.in_link => self.inlines(content),
            Inline::Link { target, content } => {
                let href = match target {
                    Link::Internal(target) => self.href(target),
                    Link::External(url) => url.clone(),
                };
                self.in_link = true;
                self.wrapped("a", &[("href", &href)], content);
                self.in_link = false;
            }
            Inline::Anchor(anchor) => {
                self.xml.open("span", &[("id", anchor)]);
                self.xml.close("span");
            }
            Inline::NoteRef {
                number,
                note,
                anchor,
            } => {
                let number = number.to_string();
                self.xml.open("sup", &[]);
                if self.in_link {
                    self.xml.text(&number);
                } else {
                    let href = format!("#{note}");
                    let mut attributes = vec![("epub:type", "noteref"), ("href", href.as_str())];
                    if let Some(anchor) = anchor {
                        attributes.push(("id", anchor));
                    }
                    self.xml.element("a", &attributes, &number);
                }
                self.xml.close("sup");
            }
            // Pictures are embedded with EPUB 5/9; until then, their description.
            Inline::Image { alt, .. } => self.xml.text(alt),
            Inline::LineBreak => self.xml.empty("br", &[]),
        }
    }
}
