//! The publication's shape: the root (`article` or `book`) with its namespaces, version and
//! `info`, and a book's preface, parts, chapters and nested sections.
//!
//! A book follows its navigation (PRD §5.1; §14 "Book structure"): the start page the
//! navigation does not list is the `preface`; a navigation list's title is a `part`, and so is
//! a top-level heading of the navigation with no page of its own outside such a part; a
//! top-level page is a `chapter`, as is a top-level heading inside a part, holding its pages;
//! and a page or heading nested under another is a `section` of it, after the page's own
//! sections. A part or heading with no page under it is left out with a warning, since DocBook
//! allows neither empty.

use std::path::PathBuf;

use adoc_dom::SourceSpan;
use adoc_render::{ViewFootnote, ViewInline, ViewTree};

use crate::{
    xml::Xml, Book, BookItem, Diagnostic, DocBookDocument, DocBookOptions, Page, Severity,
    NAMESPACE, VERSION, XLINK,
};

/// What writes the document: the XML so far and what was found on the way.
pub(crate) struct Writer<'a> {
    pub(crate) xml: Xml,
    pub(crate) diagnostics: Vec<Diagnostic>,
    #[allow(dead_code)]
    pub(crate) options: &'a DocBookOptions,
    /// The page being written.
    pub(crate) page: PageContext,
    /// Inside a `link`, where another link keeps only its text.
    pub(crate) in_link: bool,
    /// Inside a `footnote`, where another note keeps only its text.
    pub(crate) in_footnote: bool,
}

/// What the writer needs of the page being written: its key for identifiers, its file for
/// diagnostics, its footnotes, and the marks of its last listing for the callout list after it.
#[derive(Default)]
pub(crate) struct PageContext {
    pub(crate) key: String,
    pub(crate) source: PathBuf,
    pub(crate) footnotes: Vec<ViewFootnote>,
    listings: usize,
    /// The callout marks of the last listing: each number and the `co` it is.
    pub(crate) callouts: Vec<(usize, String)>,
    /// The footnotes written so far, which a repeated reference leads to.
    pub(crate) noted: std::collections::BTreeSet<u32>,
}

impl PageContext {
    fn of(page: &Page) -> Self {
        Self {
            key: page.key.clone(),
            source: page.view.source.clone(),
            footnotes: page.view.footnotes.clone(),
            listings: 0,
            callouts: Vec::new(),
            noted: std::collections::BTreeSet::new(),
        }
    }

    /// The number of the next listing on the page, from 1.
    pub(crate) fn next_listing(&mut self) -> usize {
        self.listings += 1;
        self.listings
    }

    /// The page's start, to place a diagnostic that has no span of its own.
    pub(crate) fn span(&self) -> Option<SourceSpan> {
        (!self.source.as_os_str().is_empty()).then(|| SourceSpan {
            path: self.source.clone(),
            range: adoc_core::SourceRange::default(),
            line: 1,
            include_chain: Vec::new(),
        })
    }
}

impl Writer<'_> {
    pub(crate) fn warn(&mut self, message: impl Into<String>, source: Option<&SourceSpan>) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            source: source.cloned(),
        });
    }
}

pub(crate) fn write(
    document: &DocBookDocument,
    options: &DocBookOptions,
) -> (String, Vec<Diagnostic>) {
    let mut writer = Writer {
        xml: Xml::file(),
        diagnostics: Vec::new(),
        options,
        page: PageContext::default(),
        in_link: false,
        in_footnote: false,
    };
    match document {
        DocBookDocument::Article(page) => writer.article(page),
        DocBookDocument::Book(book) => writer.book(book),
    }
    writer.xml.newline();
    (writer.xml.into_string(), writer.diagnostics)
}

/// A page or a heading of the navigation, with what is nested under it.
enum Node<'a> {
    Page(&'a Page, Vec<Node<'a>>),
    Heading(&'a str, Vec<Node<'a>>),
}

impl Node<'_> {
    /// Whether the node holds a page, so writes an element DocBook allows.
    fn has_page(&self) -> bool {
        match self {
            Self::Page(..) => true,
            Self::Heading(_, children) => children.iter().any(Node::has_page),
        }
    }
}

/// The run of a book under one part's title, or under none.
struct Group<'a> {
    part: Option<&'a str>,
    nodes: Vec<Node<'a>>,
}

/// A part's title, if any, and the items under it, each at its depth.
type Run<'a> = (Option<&'a str>, Vec<(u8, &'a BookItem)>);

/// A book's items as its preface and groups of nested nodes.
fn arrange(items: &[BookItem]) -> (Option<&Page>, Vec<Group<'_>>) {
    let mut preface = None;
    let mut groups: Vec<Run<'_>> = vec![(None, Vec::new())];
    for item in items {
        if let BookItem::Part(title) = item {
            groups.push((Some(title), Vec::new()));
            continue;
        }
        let first = groups.len() == 1 && groups[0].1.is_empty();
        let Some((_, run)) = groups.last_mut() else {
            continue;
        };
        match item {
            BookItem::Preface(page) if first && preface.is_none() => preface = Some(page),
            // A preface after the book has begun is read as a chapter.
            BookItem::Preface(_) => run.push((1, item)),
            BookItem::Heading { depth, .. } => run.push(((*depth).max(1), item)),
            BookItem::Page(page) => run.push((page.depth.max(1), item)),
            BookItem::Part(_) => {}
        }
    }
    let groups = groups
        .into_iter()
        .filter(|(part, items)| part.is_some() || !items.is_empty())
        .map(|(part, items)| {
            let mut at = 0;
            Group {
                part,
                nodes: nest(&items, &mut at, 0),
            }
        })
        .collect();
    (preface, groups)
}

/// The items from `at` deeper than `depth`, each with the deeper ones that follow it.
fn nest<'a>(items: &[(u8, &'a BookItem)], at: &mut usize, depth: u8) -> Vec<Node<'a>> {
    let mut nodes = Vec::new();
    while let Some(&(item_depth, item)) = items.get(*at) {
        if item_depth <= depth {
            break;
        }
        *at += 1;
        let children = nest(items, at, item_depth);
        match item {
            BookItem::Page(page) | BookItem::Preface(page) => {
                nodes.push(Node::Page(page, children));
            }
            BookItem::Heading { title, .. } => nodes.push(Node::Heading(title, children)),
            // Parts start groups, so none is nested.
            BookItem::Part(_) => {}
        }
    }
    nodes
}

impl Writer<'_> {
    /// Opens the root element: the DocBook and XLink namespaces, the version and the language.
    fn open_root(&mut self, name: &str, language: Option<&str>) {
        let mut attributes = vec![
            ("xmlns", NAMESPACE),
            ("xmlns:xlink", XLINK),
            ("version", VERSION),
        ];
        if let Some(language) = language.filter(|language| !language.trim().is_empty()) {
            attributes.push(("xml:lang", language.trim()));
        }
        self.xml.open(name, &attributes);
        self.xml.newline();
    }

    fn article(&mut self, page: &Page) {
        let view = &page.view;
        self.page = PageContext::of(page);
        self.open_root("article", view.attributes.get("lang").map(String::as_str));
        let (title, subtitle) = partition(self.page_title(view));
        self.info(title, subtitle.as_deref(), &view.attributes);
        self.page_content(view, &[]);
        self.xml.close("article");
    }

    fn book(&mut self, book: &Book) {
        self.open_root("book", book.attributes.get("lang").map(String::as_str));
        let title = if book.title.trim().is_empty() {
            self.warn(
                "the book has no title, so it is titled \u{201c}Untitled\u{201d}",
                None,
            );
            Title::Text("Untitled".to_owned())
        } else {
            Title::Text(book.title.clone())
        };
        self.info(title, book.subtitle.as_deref(), &book.attributes);
        let (preface, groups) = arrange(&book.items);
        if let Some(page) = preface {
            self.component("preface", page, &[]);
        }
        for group in &groups {
            match group.part {
                Some(title) => self.part(title, &group.nodes),
                None => {
                    for node in &group.nodes {
                        self.top(node, false);
                    }
                }
            }
        }
        self.xml.close("book");
    }

    /// A `part` titled `title` holding `nodes`, or a warning when none holds a page.
    fn part(&mut self, title: &str, nodes: &[Node<'_>]) {
        if !nodes.iter().any(Node::has_page) {
            self.warn(
                format!("the part \u{201c}{title}\u{201d} has no pages, so it is left out"),
                None,
            );
            return;
        }
        self.xml.open("part", &[]);
        self.xml.element("title", &[], title);
        self.xml.newline();
        for node in nodes {
            self.top(node, true);
        }
        self.xml.close("part");
        self.xml.newline();
    }

    /// A top-level node: a page is a `chapter`; a heading is a `part` outside a part and a
    /// `chapter` of its pages inside one, since parts do not nest.
    fn top(&mut self, node: &Node<'_>, in_part: bool) {
        match node {
            Node::Page(page, children) => self.component("chapter", page, children),
            Node::Heading(title, children) if !in_part => self.part(title, children),
            Node::Heading(title, children) => self.heading("chapter", title, children),
        }
    }

    /// A page as `name` (`preface`, `chapter` or `section`): its title, its blocks, then what
    /// the navigation nests under it, as sections.
    fn component(&mut self, name: &str, page: &Page, children: &[Node<'_>]) {
        self.page = PageContext::of(page);
        self.xml.open(name, &[]);
        let title = self.page_title(&page.view);
        self.title(&title);
        self.xml.newline();
        self.page_content(&page.view, children);
        self.xml.close(name);
        self.xml.newline();
    }

    /// A heading of the navigation as `name` holding its pages as sections, or a warning when
    /// it has none.
    fn heading(&mut self, name: &str, title: &str, children: &[Node<'_>]) {
        if !children.iter().any(Node::has_page) {
            self.warn(
                format!(
                    "the navigation heading \u{201c}{title}\u{201d} has no pages under it, so it \
                     is left out"
                ),
                None,
            );
            return;
        }
        self.xml.open(name, &[]);
        self.xml.element("title", &[], title);
        self.xml.newline();
        for child in children {
            self.section(child);
        }
        self.xml.close(name);
        self.xml.newline();
    }

    fn section(&mut self, node: &Node<'_>) {
        match node {
            Node::Page(page, children) => self.component("section", page, children),
            Node::Heading(title, children) => self.heading("section", title, children),
        }
    }

    /// A page's blocks and the nodes under it, with an empty paragraph when neither writes
    /// anything, as DocBook asks of an article, chapter or section.
    fn page_content(&mut self, view: &ViewTree, children: &[Node<'_>]) {
        let wrote = self.blocks(&view.blocks);
        let nested = children.iter().any(Node::has_page);
        if !wrote && !nested {
            self.xml.empty("para", &[]);
            self.xml.newline();
        }
        for child in children {
            self.section(child);
        }
    }

    /// A page's title: its own, else its file's name and a warning, since DocBook needs one.
    fn page_title(&mut self, view: &ViewTree) -> Title {
        match view
            .title
            .as_ref()
            .filter(|title| !ViewInline::plain_text(title).trim().is_empty())
        {
            Some(title) => Title::Inlines(title.clone()),
            None => {
                let name = view
                    .source
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or_else(|| "Untitled".to_owned());
                self.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "the page has no title, so it is titled \u{201c}{name}\u{201d}, its \
                         file's name"
                    ),
                    source: page_span(view),
                });
                Title::Text(name)
            }
        }
    }

    /// A `title` element.
    pub(crate) fn title(&mut self, title: &Title) {
        match title {
            Title::Text(text) => self.xml.element("title", &[], text),
            Title::Inlines(inlines) => {
                self.xml.open("title", &[]);
                self.inlines(inlines);
                self.xml.close("title");
            }
        }
    }
}

/// A title: text, or inline content with its markup.
pub(crate) enum Title {
    Text(String),
    Inlines(Vec<ViewInline>),
}

/// A document title split, as Asciidoctor splits it, at its last `": "` into a title and a
/// subtitle; only a title of plain text, so no markup is cut in two.
fn partition(title: Title) -> (Title, Option<String>) {
    let Title::Inlines(inlines) = &title else {
        return (title, None);
    };
    if !inlines
        .iter()
        .all(|inline| matches!(inline, ViewInline::Text(_)))
    {
        return (title, None);
    }
    let text = ViewInline::plain_text(inlines);
    match text.rsplit_once(": ") {
        Some((main, sub)) if !main.trim().is_empty() && !sub.trim().is_empty() => (
            Title::Text(main.trim().to_owned()),
            Some(sub.trim().to_owned()),
        ),
        _ => (title, None),
    }
}

/// The start of a page, to place a diagnostic about the page as a whole.
pub(crate) fn page_span(view: &ViewTree) -> Option<SourceSpan> {
    (!view.source.as_os_str().is_empty()).then(|| SourceSpan {
        path: view.source.clone(),
        range: adoc_core::SourceRange::default(),
        line: 1,
        include_chain: Vec::new(),
    })
}
