//! A chapter as KF8 markup: a skeleton (the XHTML shell, its body empty) and the body's content
//! as top-level elements, each with its byte range, so they can be grouped into fragments.
//! Anchors are recorded where their element starts; internal links are written as fixed-width
//! placeholders and filled in once every fragment's place is known.

use crate::book::{Block, Cell, Chapter, Inline, KindleBook, Link, Note, ParagraphClass, Target};

/// A link not yet placed: `kindle:pos:fid:ZZZZ:off:ZZZZZZZZZZ`, the same length as the link it
/// becomes.
pub(super) const PLACEHOLDER: &str = "kindle:pos:fid:ZZZZ:off:ZZZZZZZZZZ";

/// A chapter's markup.
pub(super) struct ChapterMarkup {
    /// The skeleton before the body's content: up to and including `<body aid="…">`.
    pub(super) prefix: Vec<u8>,
    /// The skeleton after it.
    pub(super) suffix: Vec<u8>,
    /// The body's content.
    pub(super) body: Vec<u8>,
    /// Where each top-level element of the body ends, in order.
    pub(super) element_ends: Vec<usize>,
    /// Each anchor and where in `body` its element starts.
    pub(super) anchors: Vec<(String, usize)>,
    /// Each placeholder in `body` and the place its link leads.
    pub(super) links: Vec<(usize, Target)>,
}

/// The stylesheet every chapter links to: sizes in `em`, no fonts and no fixed widths, so the
/// reader's choices of font, size and theme apply.
pub(super) const STYLESHEET: &str = "\
body{margin:0;padding:0}\
h1,h2,h3,h4,h5,h6{font-weight:bold;margin:1em 0 0.5em 0;page-break-after:avoid}\
h1{font-size:1.6em}h2{font-size:1.35em}h3{font-size:1.2em}h4,h5,h6{font-size:1em}\
p{margin:0 0 0.6em 0;text-indent:0}\
p.caption{font-style:italic;margin:0.6em 0 0.3em 0}\
p.attribution{text-align:right;font-style:italic}\
p.label{margin-bottom:0.2em}\
pre{white-space:pre-wrap;margin:0.4em 0 0.8em 0}\
pre.code,code{font-family:monospace}\
blockquote{margin:0.6em 1.5em}\
div.aside{margin:0.8em 0;padding:0.4em 0.6em;border-left:0.2em solid gray}\
div.figure{margin:0.8em 0;text-align:center}\
div.figure img{max-width:100%}\
img{max-width:100%;height:auto}\
table{border-collapse:collapse;margin:0.6em 0}\
th,td{border:1px solid gray;padding:0.2em 0.4em;vertical-align:top;text-align:left}\
th{font-weight:bold}\
dt{font-weight:bold;margin-top:0.4em}dd{margin:0 0 0.4em 1.5em}\
span.mark{background-color:#ffff99}\
div.break{page-break-after:always}\
div.notes{margin-top:1.5em;border-top:1px solid gray;font-size:0.9em}\
hr{margin:1em 0}";

pub(super) fn chapter(book: &KindleBook, index: usize, chapter: &Chapter) -> ChapterMarkup {
    let mut prefix = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <html xmlns=\"http://www.w3.org/1999/xhtml\"",
    );
    prefix.push_str(" xml:lang=\"");
    escape_into(&mut prefix, &book.metadata.language);
    prefix.push_str("\"><head><title>");
    escape_into(&mut prefix, &chapter.title);
    prefix.push_str(
        "</title><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"/>\
         <link rel=\"stylesheet\" type=\"text/css\" href=\"kindle:flow:0001?mime=text/css\"/>\
         </head><body aid=\"",
    );
    prefix.push_str(&super::base32(u32::try_from(index).unwrap_or(u32::MAX), 0));
    prefix.push_str("\">");
    let mut writer = Writer {
        book,
        out: String::new(),
        anchors: Vec::new(),
        links: Vec::new(),
    };
    let mut element_ends = Vec::new();
    for block in &chapter.blocks {
        writer.block(block);
        if element_ends.last() != Some(&writer.out.len()) {
            element_ends.push(writer.out.len());
        }
    }
    ChapterMarkup {
        prefix: prefix.into_bytes(),
        suffix: b"</body></html>".to_vec(),
        body: writer.out.into_bytes(),
        element_ends,
        anchors: writer.anchors,
        links: writer.links,
    }
}

/// Escapes `text` for XHTML text or a quoted attribute, dropping the characters XML 1.0
/// forbids.
pub(super) fn escape_into(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(character),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {
                out.push('\u{FFFD}');
            }
            c => out.push(c),
        }
    }
}

struct Writer<'a> {
    book: &'a KindleBook,
    out: String,
    anchors: Vec<(String, usize)>,
    links: Vec<(usize, Target)>,
}

impl Writer<'_> {
    /// Opens `tag` with `class` and the block's anchor as its id, recording where it starts.
    fn open(&mut self, tag: &str, class: Option<&str>, anchor: Option<&str>) {
        if let Some(anchor) = anchor {
            self.anchors.push((anchor.to_owned(), self.out.len()));
        }
        self.out.push('<');
        self.out.push_str(tag);
        if let Some(anchor) = anchor {
            self.out.push_str(" id=\"");
            escape_into(&mut self.out, anchor);
            self.out.push('"');
        }
        if let Some(class) = class {
            self.out.push_str(" class=\"");
            self.out.push_str(class);
            self.out.push('"');
        }
        self.out.push('>');
    }

    fn close(&mut self, tag: &str) {
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push('>');
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
                let tag = format!("h{}", (*level).clamp(1, 6));
                self.open(&tag, None, anchor.as_deref());
                self.inlines(content);
                self.close(&tag);
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
                self.open("p", class, anchor.as_deref());
                self.inlines(content);
                self.close("p");
            }
            Block::Preformatted { anchor, text, code } => {
                self.open("pre", code.then_some("code"), anchor.as_deref());
                escape_into(&mut self.out, text);
                self.close("pre");
            }
            Block::List {
                anchor,
                ordered,
                items,
            } => {
                let tag = if *ordered { "ol" } else { "ul" };
                self.open(tag, None, anchor.as_deref());
                for item in items {
                    self.open("li", None, None);
                    self.blocks(item);
                    self.close("li");
                }
                self.close(tag);
            }
            Block::Definitions { anchor, items } => {
                self.open("dl", None, anchor.as_deref());
                for (term, description) in items {
                    self.open("dt", None, None);
                    self.inlines(term);
                    self.close("dt");
                    self.open("dd", None, None);
                    self.blocks(description);
                    self.close("dd");
                }
                self.close("dl");
            }
            Block::Quote { anchor, blocks } => {
                self.open("blockquote", None, anchor.as_deref());
                self.blocks(blocks);
                self.close("blockquote");
            }
            Block::Aside { anchor, blocks } => {
                self.open("div", Some("aside"), anchor.as_deref());
                self.blocks(blocks);
                self.close("div");
            }
            Block::Table { anchor, rows } => {
                self.open("table", None, anchor.as_deref());
                for row in rows {
                    self.open("tr", None, None);
                    for cell in &row.cells {
                        self.cell(cell, row.header);
                    }
                    self.close("tr");
                }
                self.close("table");
            }
            Block::Figure { anchor, asset, alt } => {
                self.open("div", Some("figure"), anchor.as_deref());
                self.image(*asset, alt);
                self.close("div");
            }
            Block::Rule => self.out.push_str("<hr/>"),
            Block::PageBreak => self.out.push_str("<div class=\"break\"></div>"),
            Block::Notes(notes) => {
                self.open("div", Some("notes"), None);
                for note in notes {
                    self.note(note);
                }
                self.close("div");
            }
        }
    }

    fn cell(&mut self, cell: &Cell, header: bool) {
        let tag = if header { "th" } else { "td" };
        self.out.push('<');
        self.out.push_str(tag);
        if cell.colspan > 1 {
            self.out.push_str(&format!(" colspan=\"{}\"", cell.colspan));
        }
        if cell.rowspan > 1 {
            self.out.push_str(&format!(" rowspan=\"{}\"", cell.rowspan));
        }
        self.out.push('>');
        self.blocks(&cell.blocks);
        self.close(tag);
    }

    fn note(&mut self, note: &Note) {
        self.open("p", None, Some(&note.anchor));
        match &note.back {
            Some(back) => {
                self.out.push_str("<a href=\"");
                self.placeholder(Target {
                    chapter: self.chapter_of(back),
                    anchor: Some(back.clone()),
                });
                self.out.push_str("\">");
                self.out.push_str(&note.number.to_string());
                self.out.push_str("</a>");
            }
            None => self.out.push_str(&note.number.to_string()),
        }
        self.out.push_str(". ");
        self.inlines(&note.content);
        self.close("p");
    }

    /// The chapter an anchor of this writer's chapter belongs to: its own, named in the
    /// anchor's prefix (`c3-r1` is in chapter index 2).
    fn chapter_of(&self, anchor: &str) -> usize {
        anchor
            .strip_prefix('c')
            .and_then(|rest| rest.split('-').next())
            .and_then(|number| number.parse::<usize>().ok())
            .and_then(|number| number.checked_sub(1))
            .unwrap_or(0)
    }

    fn placeholder(&mut self, target: Target) {
        self.links.push((self.out.len(), target));
        self.out.push_str(PLACEHOLDER);
    }

    fn image(&mut self, asset: usize, alt: &str) {
        match self.book.assets.get(asset) {
            Some(found) => {
                self.out.push_str("<img src=\"kindle:embed:");
                self.out.push_str(&super::base32(
                    u32::try_from(asset + 1).unwrap_or(u32::MAX),
                    4,
                ));
                self.out.push_str("?mime=");
                self.out.push_str(found.kind.media_type());
                self.out.push_str("\" alt=\"");
                escape_into(&mut self.out, alt);
                self.out.push_str("\"/>");
            }
            None => escape_into(&mut self.out, alt),
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(inline);
        }
    }

    fn wrapped(&mut self, tag: &str, class: Option<&str>, content: &[Inline]) {
        self.open(tag, class, None);
        self.inlines(content);
        self.close(tag);
    }

    fn inline(&mut self, inline: &Inline) {
        match inline {
            Inline::Text(text) => escape_into(&mut self.out, text),
            Inline::Strong(content) => self.wrapped("b", None, content),
            Inline::Emphasis(content) => self.wrapped("i", None, content),
            Inline::Code(content) => self.wrapped("code", None, content),
            Inline::Mark(content) => self.wrapped("span", Some("mark"), content),
            Inline::Superscript(content) => self.wrapped("sup", None, content),
            Inline::Subscript(content) => self.wrapped("sub", None, content),
            Inline::Link { target, content } => {
                self.out.push_str("<a href=\"");
                match target {
                    Link::Internal(target) => self.placeholder(target.clone()),
                    Link::External(url) => escape_into(&mut self.out, url),
                }
                self.out.push_str("\">");
                self.inlines(content);
                self.out.push_str("</a>");
            }
            Inline::Anchor(anchor) => {
                self.open("a", None, Some(anchor));
                self.out.push_str("</a>");
            }
            Inline::NoteRef {
                number,
                note,
                anchor,
            } => {
                self.out.push_str("<sup>");
                if let Some(anchor) = anchor {
                    self.anchors.push((anchor.clone(), self.out.len()));
                    self.out.push_str("<a id=\"");
                    escape_into(&mut self.out, anchor);
                    self.out.push_str("\" href=\"");
                } else {
                    self.out.push_str("<a href=\"");
                }
                let chapter = self.chapter_of(note);
                self.placeholder(Target {
                    chapter,
                    anchor: Some(note.clone()),
                });
                self.out.push_str("\">");
                self.out.push_str(&number.to_string());
                self.out.push_str("</a></sup>");
            }
            Inline::Image { asset, alt } => self.image(*asset, alt),
            Inline::LineBreak => self.out.push_str("<br/>"),
        }
    }
}
