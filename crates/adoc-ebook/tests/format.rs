//! What the mapping keeps for each format: Kindle's book is as it always was, and an EPUB keeps
//! what EPUB can show (EPUB PRD §15).

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_ebook::{
    AsideKind, Block, Book, Document, Format, Inline, InlineStyle, Options, Page, Severity,
};

fn book(text: &str, format: Format) -> (Book, Vec<adoc_ebook::Diagnostic>) {
    let path = Path::new("/docs/page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    let view = adoc_render::build_view(path, &dom, &adoc_render::LocalResolver);
    let document = Document::Article(Page {
        key: "p0".to_owned(),
        view,
        depth: 0,
    });
    Book::build(
        &document,
        &Options {
            format,
            ..Options::default()
        },
    )
}

const PAGE: &str = "= Page\n:experimental:\n\nPress kbd:[Ctrl+C] and btn:[OK].\n\n\
                    [cols=\"1,1\"]\n|===\na|\n[cols=\"1\"]\n!===\n!Inner\n!===\n|Beside\n|===\n\n\
                    TIP: A tip.\n\n****\nA sidebar.\n****\n\nvideo::https://example.com/v.mp4[]\n";

fn inlines(book: &Book) -> Vec<&Inline> {
    book.chapters[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph { content, .. } => Some(content.iter()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The table inside the outer table's first cell, if it stayed a table.
fn inner_table(book: &Book) -> Option<&Block> {
    book.chapters[0]
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Table { rows, .. } => rows[0].cells[0]
                .blocks
                .iter()
                .find(|block| matches!(block, Block::Table { .. })),
            _ => None,
        })
}

#[test]
fn kindle_is_the_default_and_lowers_what_it_cannot_show() {
    assert_eq!(Options::default().format, Format::Kindle);
    let (book, diagnostics) = book(PAGE, Format::Kindle);
    let inlines = inlines(&book);
    assert!(inlines
        .iter()
        .any(|inline| matches!(inline, Inline::Code(_))));
    assert!(!inlines
        .iter()
        .any(|inline| matches!(inline, Inline::Styled { .. })));
    assert!(
        inner_table(&book).is_none(),
        "a nested table is lines of text"
    );
    assert!(diagnostics
        .iter()
        .any(|d| d.message.contains("inside a table cell")));
    assert!(diagnostics
        .iter()
        .any(|d| d.message.starts_with("a Kindle book cannot play video")));
}

#[test]
fn an_epub_keeps_nested_tables_and_styled_text() {
    let (book, diagnostics) = book(PAGE, Format::Epub);
    let styles: Vec<InlineStyle> = inlines(&book)
        .into_iter()
        .filter_map(|inline| match inline {
            Inline::Styled { style, .. } => Some(*style),
            _ => None,
        })
        .collect();
    assert_eq!(
        styles,
        [
            InlineStyle::Keyboard,
            InlineStyle::Keyboard,
            InlineStyle::Button
        ]
    );
    assert!(inner_table(&book).is_some());
    assert!(!diagnostics
        .iter()
        .any(|d| d.message.contains("inside a table cell")));
    assert!(diagnostics
        .iter()
        .any(|d| d.severity == Severity::Warning
            && d.message.starts_with("an EPUB cannot play video")));
}

#[test]
fn asides_say_what_they_set_apart_in_every_format() {
    for format in [Format::Kindle, Format::Epub] {
        let (book, _) = book(PAGE, format);
        let kinds: Vec<AsideKind> = book.chapters[0]
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Aside { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            [AsideKind::Admonition("tip"), AsideKind::Sidebar],
            "{format:?}"
        );
    }
}
