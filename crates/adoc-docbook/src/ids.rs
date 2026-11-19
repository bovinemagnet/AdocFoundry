//! Every `xml:id` of a publication, decided before anything is written so a reference may lead
//! forward (PRD §5.3; §14 "Identifier scheme").
//!
//! A page's id is its key, which the engine derives from the page's Antora resource
//! (`manual-2.0-ROOT-install`), never from a path. An anchor the page defines (a section's or
//! block's id, an inline anchor) is `<page id>.<anchor>` in a book, where many pages meet, and
//! the anchor itself in an article. Each is made a valid XML name (a character no name may
//! hold becomes `_`, and a name that may not start as it does gets a leading `_`) and unique
//! (a later one that would repeat an earlier gets `-2`, `-3`, …). Footnotes and callout marks
//! take ids from the same allocator as they are written, so nothing repeats.
//!
//! An anchor is found by the page that holds it and by the file it was written in, so a
//! reference to an anchor in a partial leads to it in the page that includes the partial.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use adoc_render::{ViewBlock, ViewInline, ViewKind};

use crate::Page;

#[derive(Default)]
pub(crate) struct Ids {
    used: HashSet<String>,
    /// Each page's id, by its place in the publication.
    pages: Vec<String>,
    /// Each page's place, by its file.
    by_path: HashMap<PathBuf, usize>,
    /// `(page, anchor)` to its id.
    anchors: HashMap<(usize, String), String>,
    /// `(file the anchor is written in, anchor)` to the page holding it.
    by_file: HashMap<(PathBuf, String), usize>,
    /// The ids already written, so an anchor defined twice is written once.
    placed: HashSet<String>,
}

/// `wanted` as an XML name with no colon: what no name may hold is `_`, and a name that may
/// not start as it does starts with `_`.
pub(crate) fn sanitise(wanted: &str) -> String {
    let mut name: String = wanted
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !name
        .chars()
        .next()
        .is_some_and(|first| first == '_' || first.is_alphabetic())
    {
        name.insert(0, '_');
    }
    name
}

impl Ids {
    /// The ids of `pages`, in the order they are written; `article` for a single page, whose
    /// anchors keep their own names.
    pub(crate) fn collect(pages: &[&Page], article: bool) -> Self {
        let mut ids = Self::default();
        for (index, page) in pages.iter().enumerate() {
            let id = ids.allocate(&page.key);
            ids.pages.push(id);
            ids.by_path
                .entry(adoc_core::normalize_path(&page.view.source))
                .or_insert(index);
        }
        for (index, page) in pages.iter().enumerate() {
            let mut found = Vec::new();
            anchors_in(&page.view.blocks, &page.view.source, &mut found);
            for (file, anchor) in found {
                if ids.anchors.contains_key(&(index, anchor.clone())) {
                    continue;
                }
                let wanted = if article {
                    anchor.clone()
                } else {
                    format!("{}.{anchor}", page.key)
                };
                let id = ids.allocate(&wanted);
                ids.anchors.insert((index, anchor.clone()), id);
                ids.by_file
                    .entry((adoc_core::normalize_path(&file), anchor))
                    .or_insert(index);
            }
        }
        ids
    }

    /// A new id for `wanted`, made a valid name and unique.
    pub(crate) fn allocate(&mut self, wanted: &str) -> String {
        let base = sanitise(wanted);
        let mut id = base.clone();
        let mut number = 1;
        while self.used.contains(&id) {
            number += 1;
            id = format!("{base}-{number}");
        }
        self.used.insert(id.clone());
        id
    }

    pub(crate) fn page(&self, index: usize) -> Option<&str> {
        self.pages.get(index).map(String::as_str)
    }

    pub(crate) fn anchor(&self, page: usize, anchor: &str) -> Option<&str> {
        self.anchors
            .get(&(page, anchor.to_owned()))
            .map(String::as_str)
    }

    /// The page a reference leads to: the one whose file is `path`, else, for an anchor, the
    /// one holding it that was written in `path` (a partial it includes).
    pub(crate) fn page_of(&self, path: &Path, anchor: Option<&str>) -> Option<usize> {
        let path = adoc_core::normalize_path(path);
        self.by_path.get(&path).copied().or_else(|| {
            anchor.and_then(|anchor| self.by_file.get(&(path, anchor.to_owned())).copied())
        })
    }

    /// Whether `id` is yet to be written, marking it written.
    pub(crate) fn place(&mut self, id: &str) -> bool {
        self.placed.insert(id.to_owned())
    }
}

/// The anchors `blocks` define, each with the file it is written in, in document order.
fn anchors_in(blocks: &[ViewBlock], page: &Path, out: &mut Vec<(PathBuf, String)>) {
    for block in blocks {
        let file = if block.span.path.as_os_str().is_empty() {
            page.to_path_buf()
        } else {
            block.span.path.clone()
        };
        if let Some(id) = block.id.as_ref().filter(|id| !id.is_empty()) {
            out.push((file.clone(), id.clone()));
        }
        let inlines = |content: &[ViewInline], out: &mut Vec<(PathBuf, String)>| {
            inline_anchors(content, &file, out);
        };
        inlines(&block.title, out);
        match &block.kind {
            ViewKind::Section { title, blocks, .. } => {
                inlines(title, out);
                anchors_in(blocks, page, out);
            }
            ViewKind::Heading { title, .. } => inlines(title, out),
            ViewKind::Paragraph(content) => inlines(content, out),
            ViewKind::List { items, .. } => {
                for item in items {
                    inlines(&item.content, out);
                    anchors_in(&item.blocks, page, out);
                }
            }
            ViewKind::DescriptionList(items) => {
                for item in items {
                    inlines(&item.term, out);
                    inlines(&item.content, out);
                    anchors_in(&item.blocks, page, out);
                }
            }
            ViewKind::CalloutList(items) => {
                for item in items {
                    inlines(&item.content, out);
                    anchors_in(&item.blocks, page, out);
                }
            }
            ViewKind::Admonition { blocks, .. }
            | ViewKind::Quote { blocks, .. }
            | ViewKind::Sidebar(blocks)
            | ViewKind::Example(blocks)
            | ViewKind::Open(blocks) => anchors_in(blocks, page, out),
            ViewKind::Table(table) => {
                let rows = table
                    .header
                    .iter()
                    .chain(&table.rows)
                    .chain(table.footer.iter());
                for row in rows {
                    for cell in row {
                        anchors_in(&cell.blocks, page, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn inline_anchors(content: &[ViewInline], file: &Path, out: &mut Vec<(PathBuf, String)>) {
    for inline in content {
        match inline {
            ViewInline::Anchor(id) if !id.is_empty() => out.push((file.to_path_buf(), id.clone())),
            ViewInline::Strong(content)
            | ViewInline::Emphasis(content)
            | ViewInline::Code(content)
            | ViewInline::Mark(content)
            | ViewInline::Superscript(content)
            | ViewInline::Subscript(content)
            | ViewInline::Span { content, .. }
            | ViewInline::Link { content, .. }
            | ViewInline::Xref { content, .. }
            | ViewInline::IndexTerm { content, .. } => inline_anchors(content, file, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_valid() {
        assert_eq!(sanitise("_installing"), "_installing");
        assert_eq!(sanitise("1st step"), "_1st_step");
        assert_eq!(sanitise("a:b#c"), "a_b_c");
        assert_eq!(sanitise("-x"), "_-x");
        assert_eq!(sanitise("日本語"), "日本語");
        assert_eq!(sanitise(""), "_");
        for wanted in ["1st step", "a:b#c", "-x", "", "日本語", ". "] {
            assert!(crate::check::is_name(&sanitise(wanted)), "{wanted}");
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut ids = Ids::default();
        assert_eq!(ids.allocate("a"), "a");
        assert_eq!(ids.allocate("a"), "a-2");
        assert_eq!(ids.allocate("a:"), "a_");
        assert_eq!(ids.allocate("a?"), "a_-2");
        assert_eq!(ids.allocate("a"), "a-3");
    }
}
