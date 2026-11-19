//! The publication's shape: the root (`article` or `book`) with its namespaces, version and
//! `info`, and a book's parts and chapters.

use adoc_dom::SourceSpan;
use adoc_render::{ViewInline, ViewTree};

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
    };
    match document {
        DocBookDocument::Article(page) => writer.article(page),
        DocBookDocument::Book(book) => writer.book(book),
    }
    writer.xml.newline();
    (writer.xml.into_string(), writer.diagnostics)
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
        self.open_root("article", view.attributes.get("lang").map(String::as_str));
        self.xml.open("info", &[]);
        self.title(view);
        self.xml.close("info");
        self.xml.newline();
        self.body(&view.blocks);
        self.xml.close("article");
    }

    fn book(&mut self, book: &Book) {
        self.open_root("book", book.attributes.get("lang").map(String::as_str));
        self.xml.open("info", &[]);
        self.xml.element("title", &[], &book.title);
        self.xml.close("info");
        self.xml.newline();
        // A part is opened by its first page, since a part with none is not DocBook.
        let mut pending_part: Option<&str> = None;
        let mut in_part = false;
        for item in &book.items {
            match item {
                BookItem::Part(title) => {
                    if in_part {
                        self.xml.close("part");
                        self.xml.newline();
                        in_part = false;
                    }
                    if let Some(empty) = pending_part.replace(title) {
                        self.warn(
                            format!(
                                "the part \u{201c}{empty}\u{201d} has no pages, so it is left out"
                            ),
                            None,
                        );
                    }
                }
                BookItem::Heading { title, .. } => {
                    self.warn(
                        format!("the navigation heading \u{201c}{title}\u{201d} is not written"),
                        None,
                    );
                }
                BookItem::Page(page) => {
                    if let Some(title) = pending_part.take() {
                        self.xml.open("part", &[]);
                        self.xml.element("title", &[], title);
                        self.xml.newline();
                        in_part = true;
                    }
                    self.chapter(page);
                }
            }
        }
        if in_part {
            self.xml.close("part");
            self.xml.newline();
        }
        if let Some(empty) = pending_part {
            self.warn(
                format!("the part \u{201c}{empty}\u{201d} has no pages, so it is left out"),
                None,
            );
        }
        self.xml.close("book");
    }

    fn chapter(&mut self, page: &Page) {
        self.xml.open("chapter", &[]);
        self.title(&page.view);
        self.xml.newline();
        self.body(&page.view.blocks);
        self.xml.close("chapter");
        self.xml.newline();
    }

    /// A page's `title`: its own, else its file's name and a warning, since DocBook needs one.
    fn title(&mut self, view: &ViewTree) {
        match view
            .title
            .as_deref()
            .filter(|title| !ViewInline::plain_text(title).trim().is_empty())
        {
            Some(title) => {
                self.xml.open("title", &[]);
                self.inlines(title);
                self.xml.close("title");
            }
            None => {
                let name = view
                    .source
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or_else(|| "Untitled".to_owned());
                self.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("the page has no title, so it is titled \u{201c}{name}\u{201d}, its file's name"),
                    source: page_span(view),
                });
                self.xml.element("title", &[], &name);
            }
        }
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
