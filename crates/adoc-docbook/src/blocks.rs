//! A page's blocks as DocBook blocks: sections nest, paragraphs are `para`, and what has no
//! DocBook element yet keeps its text, with a warning.

use adoc_render::{ViewBlock, ViewInline, ViewKind};

use crate::document::Writer;

impl Writer<'_> {
    /// Writes `blocks`, whether they show anything or not.
    pub(crate) fn blocks(&mut self, blocks: &[ViewBlock]) -> bool {
        let mut wrote = false;
        for block in blocks {
            wrote |= self.block(block);
        }
        wrote
    }

    /// Writes the blocks of an element DocBook does not let be empty (an article, a chapter, a
    /// section), with an empty paragraph when they show nothing: it adds nothing a reader sees.
    pub(crate) fn body(&mut self, blocks: &[ViewBlock]) {
        if !self.blocks(blocks) {
            self.xml.empty("para", &[]);
            self.xml.newline();
        }
    }

    /// Writes `block`, and whether it wrote anything.
    fn block(&mut self, block: &ViewBlock) -> bool {
        match &block.kind {
            ViewKind::Section { title, blocks, .. } => {
                self.xml.open("section", &[]);
                self.xml.open("title", &[]);
                self.inlines(title);
                self.xml.close("title");
                self.xml.newline();
                self.body(blocks);
                self.xml.close("section");
            }
            ViewKind::Paragraph(content) => {
                self.xml.open("para", &[]);
                self.inlines(content);
                self.xml.close("para");
            }
            // A processor makes its own table of contents.
            ViewKind::Toc => return false,
            _ => {
                let text = block_text(block);
                self.warn(
                    format!(
                        "{} is not written as DocBook yet, so only its text is kept",
                        what(&block.kind)
                    ),
                    Some(&block.span),
                );
                if text.contains('\n') {
                    self.xml.element("literallayout", &[], &text);
                } else {
                    self.xml.element("para", &[], &text);
                }
            }
        }
        self.xml.newline();
        true
    }
}

/// A block's kind, as a diagnostic names it.
fn what(kind: &ViewKind) -> String {
    match kind {
        ViewKind::Unsupported { name, .. } => format!("the {name} block"),
        ViewKind::List { .. } => "a list".to_owned(),
        ViewKind::DescriptionList(_) => "a description list".to_owned(),
        ViewKind::CalloutList(_) => "a callout list".to_owned(),
        ViewKind::Admonition { .. } => "an admonition".to_owned(),
        ViewKind::Code { .. } => "a source block".to_owned(),
        ViewKind::Literal(_) => "a literal block".to_owned(),
        ViewKind::Quote { .. } => "a quotation".to_owned(),
        ViewKind::Verse { .. } => "a verse".to_owned(),
        ViewKind::Sidebar(_) => "a sidebar".to_owned(),
        ViewKind::Example(_) => "an example".to_owned(),
        ViewKind::Open(_) => "an open block".to_owned(),
        ViewKind::Table(_) => "a table".to_owned(),
        ViewKind::Image(_) => "an image".to_owned(),
        ViewKind::Media { .. } => "an audio or video block".to_owned(),
        ViewKind::Diagram(_) => "a diagram".to_owned(),
        ViewKind::Stem { .. } => "a formula".to_owned(),
        ViewKind::Passthrough(_) => "a passthrough block".to_owned(),
        ViewKind::Heading { .. } => "a discrete heading".to_owned(),
        ViewKind::ThematicBreak => "a thematic break".to_owned(),
        ViewKind::PageBreak => "a page break".to_owned(),
        ViewKind::Section { .. } | ViewKind::Paragraph(_) | ViewKind::Toc => "a block".to_owned(),
    }
}

/// The text a block shows, its lines kept, for a block written as text alone.
pub(crate) fn block_text(block: &ViewBlock) -> String {
    let mut lines = Vec::new();
    push_text(block, &mut lines);
    lines.join("\n")
}

fn push_text(block: &ViewBlock, lines: &mut Vec<String>) {
    let inline = |content: &[ViewInline], lines: &mut Vec<String>| {
        let text = ViewInline::plain_text(content);
        if !text.trim().is_empty() {
            lines.push(text);
        }
    };
    let children = |blocks: &[ViewBlock], lines: &mut Vec<String>| {
        for block in blocks {
            push_text(block, lines);
        }
    };
    match &block.kind {
        ViewKind::Section { title, blocks, .. } => {
            inline(title, lines);
            children(blocks, lines);
        }
        ViewKind::Heading { title, .. } => inline(title, lines),
        ViewKind::Paragraph(content) => inline(content, lines),
        ViewKind::List { items, .. } => {
            for item in items {
                inline(&item.content, lines);
                children(&item.blocks, lines);
            }
        }
        ViewKind::DescriptionList(items) => {
            for item in items {
                inline(&item.term, lines);
                inline(&item.content, lines);
                children(&item.blocks, lines);
            }
        }
        ViewKind::CalloutList(items) => {
            for item in items {
                inline(&item.content, lines);
                children(&item.blocks, lines);
            }
        }
        ViewKind::Admonition { blocks, .. }
        | ViewKind::Quote { blocks, .. }
        | ViewKind::Sidebar(blocks)
        | ViewKind::Example(blocks)
        | ViewKind::Open(blocks) => children(blocks, lines),
        ViewKind::Code { text, .. }
        | ViewKind::Literal(text)
        | ViewKind::Verse { text, .. }
        | ViewKind::Passthrough(text) => lines.push(text.clone()),
        ViewKind::Table(table) => {
            let rows = table
                .header
                .iter()
                .chain(&table.rows)
                .chain(table.footer.iter());
            for row in rows {
                for cell in row {
                    children(&cell.blocks, lines);
                }
            }
        }
        ViewKind::Image(image) => lines.push(image.alt.clone()),
        ViewKind::Media { targets, .. } => lines.extend(targets.iter().cloned()),
        ViewKind::Diagram(diagram) => match diagram {
            adoc_render::DiagramView::Svg(_) => {}
            adoc_render::DiagramView::Pending { source, .. }
            | adoc_render::DiagramView::Failed { source, .. } => lines.push(source.clone()),
        },
        ViewKind::Stem { source, .. } | ViewKind::Unsupported { source, .. } => {
            lines.push(source.clone());
        }
        ViewKind::ThematicBreak | ViewKind::PageBreak | ViewKind::Toc => {}
    }
}
