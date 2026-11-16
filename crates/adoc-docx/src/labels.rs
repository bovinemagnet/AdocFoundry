//! The bookmarks a document's links can lead to, named before anything is written, so a link
//! may lead forward. A page has one for its start, and one for each id it defines: a section's
//! or block's id and every inline anchor, as the PDF exporter's labels are. Names are `_a1`,
//! `_a2` and so on in document order: short, valid in Word whatever the id, and hidden from
//! Word's list of bookmarks by the leading underscore.

use std::collections::HashMap;

use adoc_render::{ViewBlock, ViewInline, ViewKind};

use crate::DocxPage;

#[derive(Default)]
pub(crate) struct Labels {
    /// `(page key, id)` to bookmark name; the empty id is the page itself.
    names: HashMap<(String, String), String>,
}

impl Labels {
    pub(crate) fn collect(pages: &[&DocxPage]) -> Self {
        let mut labels = Self::default();
        for page in pages {
            labels.add(&page.key, "");
            labels.blocks(&page.key, &page.view.blocks);
        }
        labels
    }

    fn add(&mut self, key: &str, id: &str) {
        let next = self.names.len() + 1;
        self.names
            .entry((key.to_owned(), id.to_owned()))
            .or_insert_with(|| format!("_a{next}"));
    }

    /// The bookmark of `id` on page `key`, the page's own for the empty id.
    pub(crate) fn get(&self, key: &str, id: &str) -> Option<&str> {
        self.names
            .get(&(key.to_owned(), id.to_owned()))
            .map(String::as_str)
    }

    fn inlines(&mut self, key: &str, inlines: &[ViewInline]) {
        for inline in inlines {
            match inline {
                ViewInline::Anchor(id) => self.add(key, id),
                ViewInline::Strong(content)
                | ViewInline::Emphasis(content)
                | ViewInline::Code(content)
                | ViewInline::Mark(content)
                | ViewInline::Superscript(content)
                | ViewInline::Subscript(content)
                | ViewInline::Span { content, .. }
                | ViewInline::Link { content, .. }
                | ViewInline::Xref { content, .. }
                | ViewInline::IndexTerm { content, .. } => self.inlines(key, content),
                _ => {}
            }
        }
    }

    fn blocks(&mut self, key: &str, blocks: &[ViewBlock]) {
        for block in blocks {
            if let Some(id) = &block.id {
                self.add(key, id);
            }
            self.inlines(key, &block.title);
            match &block.kind {
                ViewKind::Section { title, blocks, .. } => {
                    self.inlines(key, title);
                    self.blocks(key, blocks);
                }
                ViewKind::Heading { title, .. } => self.inlines(key, title),
                ViewKind::Paragraph(content) => self.inlines(key, content),
                ViewKind::List { items, .. } => {
                    for item in items {
                        self.inlines(key, &item.content);
                        self.blocks(key, &item.blocks);
                    }
                }
                ViewKind::DescriptionList(items) => {
                    for item in items {
                        self.inlines(key, &item.term);
                        self.inlines(key, &item.content);
                        self.blocks(key, &item.blocks);
                    }
                }
                ViewKind::CalloutList(items) => {
                    for item in items {
                        self.inlines(key, &item.content);
                        self.blocks(key, &item.blocks);
                    }
                }
                ViewKind::Admonition { blocks, .. }
                | ViewKind::Quote { blocks, .. }
                | ViewKind::Sidebar(blocks)
                | ViewKind::Example(blocks)
                | ViewKind::Open(blocks) => self.blocks(key, blocks),
                ViewKind::Table(table) => {
                    let rows = table
                        .header
                        .iter()
                        .chain(&table.rows)
                        .chain(table.footer.iter());
                    for row in rows {
                        for cell in row {
                            self.blocks(key, &cell.blocks);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
