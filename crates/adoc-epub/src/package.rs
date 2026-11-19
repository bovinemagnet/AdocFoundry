//! The files of a book and the order they are archived in: `mimetype`, the container, the
//! package document, the navigation document and the chapters. Paths are fixed and named from
//! the book's own chapter ids, never from a source path.

use adoc_ebook::{Asset, Book, Chapter, Target, TocEntry};

use crate::{xml::Xml, zip::ZipWriter, EpubError, Modified};

/// The exact bytes of the first entry (EPUB 3.3, OCF §4.3).
pub(crate) const MIMETYPE: &[u8] = b"application/epub+zip";
/// Where the container says the package document is.
pub(crate) const PACKAGE: &str = "EPUB/package.opf";
const NAV: &str = "nav.xhtml";
const XHTML: &str = "application/xhtml+xml";

/// The book's archive, every XML file checked first.
/// The default stylesheet: readable, reflowable, naming nothing outside the book.
pub(crate) const BOOK_CSS: &str = include_str!("book.css");

/// The stylesheets every document links to, from a document `prefix` away from the package
/// document: the default, then the user's, which comes after it to override it.
pub(crate) fn stylesheets(prefix: &str, user: bool) -> Vec<String> {
    let mut links = vec![format!("{prefix}css/book.css")];
    if user {
        links.push(format!("{prefix}css/user.css"));
    }
    links
}

/// The book's archive, with `stylesheet`, already checked, after the default one; every XML
/// file checked first.
pub(crate) fn write(
    book: &Book,
    modified: Modified,
    stylesheet: Option<&str>,
) -> Result<Vec<u8>, EpubError> {
    let files = files(book, modified, stylesheet);
    for (name, data) in &files {
        check_xml(name, data)?;
    }
    let mut zip = ZipWriter::new(modified);
    zip.add_stored("mimetype", MIMETYPE)?;
    for (name, data) in &files {
        zip.add_deflated(name, data)?;
    }
    zip.finish()
}

/// Every file but `mimetype`, in archive order.
fn files(book: &Book, modified: Modified, stylesheet: Option<&str>) -> Vec<(String, Vec<u8>)> {
    let user = stylesheet.is_some();
    let mut files = vec![
        ("META-INF/container.xml".to_owned(), container()),
        (PACKAGE.to_owned(), package(book, modified, user)),
        (format!("EPUB/{NAV}"), nav(book, user)),
        ("EPUB/css/book.css".to_owned(), BOOK_CSS.as_bytes().to_vec()),
    ];
    if let Some(stylesheet) = stylesheet {
        files.push((
            "EPUB/css/user.css".to_owned(),
            stylesheet.as_bytes().to_vec(),
        ));
    }
    if book.cover.is_some() {
        files.push((format!("EPUB/{COVER}"), cover(book, user)));
    }
    for (index, chapter) in book.chapters.iter().enumerate() {
        files.push((
            format!("EPUB/{}", chapter_file(chapter)),
            crate::xhtml::chapter(book, index, user),
        ));
    }
    for (index, asset) in book.assets.iter().enumerate() {
        files.push((
            format!("EPUB/{}", picture_file(index, asset)),
            asset.bytes.clone(),
        ));
    }
    files
}

/// The cover document, relative to the package document.
const COVER: &str = "cover.xhtml";

/// A picture's file, relative to the package document: `images/i<n>.<extension>`, each picture
/// once however often it is shown.
pub(crate) fn picture_file(index: usize, asset: &Asset) -> String {
    format!("images/i{}.{}", index + 1, asset.kind.extension())
}

/// The cover document: the cover picture, alone.
fn cover(book: &Book, user: bool) -> Vec<u8> {
    let mut xml = Xml::xhtml(
        &book.metadata.language,
        &book.metadata.title,
        &stylesheets("", user),
    );
    xml.open("body", &[]);
    xml.newline();
    xml.open("section", &[("epub:type", "cover"), ("class", "cover")]);
    if let Some(index) = book.cover {
        let src = picture_file(index, &book.assets[index]);
        xml.empty("img", &[("src", &src), ("alt", &book.metadata.title)]);
    }
    xml.close("section");
    xml.newline();
    xml.close("body");
    xml.close("html");
    xml.newline();
    xml.into_bytes()
}

/// A chapter's content document, relative to the package document.
fn chapter_file(chapter: &Chapter) -> String {
    format!("text/{}", crate::xhtml::chapter_name(chapter))
}

/// Where a target is, relative to the package document.
fn href(book: &Book, target: &Target) -> String {
    let mut href = book
        .chapters
        .get(target.chapter)
        .map(chapter_file)
        .unwrap_or_default();
    if let Some(anchor) = &target.anchor {
        href.push('#');
        href.push_str(anchor);
    }
    href
}

fn container() -> Vec<u8> {
    let mut xml = Xml::file();
    xml.open(
        "container",
        &[
            ("version", "1.0"),
            ("xmlns", "urn:oasis:names:tc:opendocument:xmlns:container"),
        ],
    );
    xml.open("rootfiles", &[]);
    xml.empty(
        "rootfile",
        &[
            ("full-path", PACKAGE),
            ("media-type", "application/oebps-package+xml"),
        ],
    );
    xml.close("rootfiles");
    xml.close("container");
    xml.newline();
    xml.into_bytes()
}

fn package(book: &Book, modified: Modified, user: bool) -> Vec<u8> {
    let metadata = &book.metadata;
    let mut xml = Xml::file();
    xml.open(
        "package",
        &[
            ("xmlns", "http://www.idpf.org/2007/opf"),
            ("version", "3.0"),
            ("unique-identifier", "book-id"),
            ("xml:lang", &metadata.language),
        ],
    );
    xml.newline();
    xml.open(
        "metadata",
        &[("xmlns:dc", "http://purl.org/dc/elements/1.1/")],
    );
    xml.newline();
    xml.element("dc:identifier", &[("id", "book-id")], &metadata.identifier);
    xml.newline();
    xml.element("dc:title", &[], &metadata.title);
    xml.newline();
    xml.element("dc:language", &[], &metadata.language);
    xml.newline();
    for author in &metadata.authors {
        xml.element("dc:creator", &[], author);
        xml.newline();
    }
    if let Some(publisher) = &metadata.publisher {
        xml.element("dc:publisher", &[], publisher);
        xml.newline();
    }
    if let Some(description) = &metadata.description {
        xml.element("dc:description", &[], description);
        xml.newline();
    }
    for subject in &metadata.subjects {
        xml.element("dc:subject", &[], subject);
        xml.newline();
    }
    // `revdate` is free text: only a date EPUB can read is written as one.
    if let Some(date) = metadata.date.as_deref().filter(|date| w3c_date(date)) {
        xml.element("dc:date", &[], date);
        xml.newline();
    }
    xml.element(
        "meta",
        &[("property", "dcterms:modified")],
        &modified.to_w3c(),
    );
    xml.newline();
    for (property, value) in accessibility(book) {
        xml.element("meta", &[("property", property)], value);
        xml.newline();
    }
    xml.close("metadata");
    xml.newline();
    xml.open("manifest", &[]);
    xml.newline();
    xml.empty(
        "item",
        &[
            ("id", "nav"),
            ("href", NAV),
            ("media-type", XHTML),
            ("properties", "nav"),
        ],
    );
    xml.newline();
    xml.empty(
        "item",
        &[
            ("id", "css"),
            ("href", "css/book.css"),
            ("media-type", "text/css"),
        ],
    );
    xml.newline();
    if user {
        xml.empty(
            "item",
            &[
                ("id", "css-user"),
                ("href", "css/user.css"),
                ("media-type", "text/css"),
            ],
        );
        xml.newline();
    }
    if book.cover.is_some() {
        xml.empty(
            "item",
            &[("id", "cover"), ("href", COVER), ("media-type", XHTML)],
        );
        xml.newline();
    }
    for chapter in &book.chapters {
        xml.empty(
            "item",
            &[
                ("id", &chapter.id),
                ("href", &chapter_file(chapter)),
                ("media-type", XHTML),
            ],
        );
        xml.newline();
    }
    for (index, asset) in book.assets.iter().enumerate() {
        let id = format!("i{}", index + 1);
        let href = picture_file(index, asset);
        let mut attributes = vec![
            ("id", id.as_str()),
            ("href", href.as_str()),
            ("media-type", asset.kind.media_type()),
        ];
        if book.cover == Some(index) {
            attributes.push(("properties", "cover-image"));
        }
        xml.empty("item", &attributes);
        xml.newline();
    }
    xml.close("manifest");
    xml.newline();
    xml.open("spine", &[]);
    xml.newline();
    if book.cover.is_some() {
        xml.empty("itemref", &[("idref", "cover")]);
        xml.newline();
    }
    for chapter in &book.chapters {
        xml.empty("itemref", &[("idref", &chapter.id)]);
        xml.newline();
    }
    xml.close("spine");
    xml.newline();
    xml.close("package");
    xml.newline();
    xml.into_bytes()
}

/// Whether `date` is a date `dc:date` may hold: `CCYY`, `CCYY-MM` or `CCYY-MM-DD`.
fn w3c_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    let digits = |part: &str, length: usize| {
        part.len() == length && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    match parts.as_slice() {
        [year] => digits(year, 4),
        [year, month] => digits(year, 4) && digits(month, 2),
        [year, month, day] => digits(year, 4) && digits(month, 2) && digits(day, 2),
        _ => false,
    }
}

/// The accessibility properties the book can be shown to have from what it is made of, and no
/// more (PRD §5.6): textual, with pictures where it has any; a table of contents always; headings
/// to navigate by where it has any. It claims no conformance and no absence of hazards.
fn accessibility(book: &Book) -> Vec<(&'static str, &'static str)> {
    let mut properties = vec![("schema:accessMode", "textual")];
    if !book.assets.is_empty() {
        properties.push(("schema:accessMode", "visual"));
    }
    properties.push(("schema:accessibilityFeature", "tableOfContents"));
    let headings = book.chapters.iter().any(|chapter| {
        chapter
            .blocks
            .iter()
            .any(|block| matches!(block, adoc_ebook::Block::Heading { .. }))
    });
    if headings {
        properties.push(("schema:accessibilityFeature", "structuralNavigation"));
    }
    properties.push(("schema:accessibilityHazard", "unknown"));
    properties.push((
        "schema:accessibilitySummary",
        "Reflowable text with a table of contents and headings to navigate by. Pictures carry \
         the descriptions their source gives. The publication has not been checked against an \
         accessibility standard.",
    ));
    properties
}

/// The navigation document: the book's table of contents as one `toc` navigation, or, for a
/// book with none, its chapters in order.
fn nav(book: &Book, user: bool) -> Vec<u8> {
    let language = &book.metadata.language;
    let mut xml = Xml::xhtml(language, &book.metadata.title, &stylesheets("", user));
    xml.open("body", &[]);
    xml.newline();
    xml.open("nav", &[("epub:type", "toc"), ("id", "toc")]);
    xml.element("h1", &[], "Contents");
    xml.newline();
    let fallback: Vec<TocEntry>;
    let entries = if book.toc.is_empty() {
        fallback = book
            .chapters
            .iter()
            .enumerate()
            .map(|(index, chapter)| TocEntry {
                title: chapter.title.clone(),
                target: Target {
                    chapter: index,
                    anchor: None,
                },
                children: Vec::new(),
            })
            .collect();
        &fallback
    } else {
        &book.toc
    };
    toc_list(&mut xml, book, entries);
    xml.close("nav");
    xml.newline();
    landmarks(&mut xml, book);
    xml.close("body");
    xml.close("html");
    xml.newline();
    xml.into_bytes()
}

/// The landmarks a reading system offers to go to: the table of contents and where the
/// book's text begins, its first chapter. Hidden, since they are for the reading system.
fn landmarks(xml: &mut Xml, book: &Book) {
    xml.open(
        "nav",
        &[
            ("epub:type", "landmarks"),
            ("id", "landmarks"),
            ("hidden", "hidden"),
        ],
    );
    xml.element("h2", &[], "Landmarks");
    xml.newline();
    xml.open("ol", &[]);
    xml.newline();
    let first = href(
        book,
        &Target {
            chapter: 0,
            anchor: None,
        },
    );
    let mut landmarks = Vec::new();
    if book.cover.is_some() {
        landmarks.push(("cover", COVER, "Cover"));
    }
    landmarks.push(("toc", "nav.xhtml#toc", "Contents"));
    landmarks.push(("bodymatter", first.as_str(), "Start of content"));
    for (kind, href, title) in landmarks {
        xml.open("li", &[]);
        xml.element("a", &[("epub:type", kind), ("href", href)], title);
        xml.close("li");
        xml.newline();
    }
    xml.close("ol");
    xml.newline();
    xml.close("nav");
    xml.newline();
}

/// `entries` as an ordered list, each nested list only where an entry has children.
fn toc_list(xml: &mut Xml, book: &Book, entries: &[TocEntry]) {
    xml.open("ol", &[]);
    xml.newline();
    for entry in entries {
        xml.open("li", &[]);
        let title = if entry.title.trim().is_empty() {
            book.metadata.title.as_str()
        } else {
            entry.title.as_str()
        };
        xml.element("a", &[("href", &href(book, &entry.target))], title);
        if !entry.children.is_empty() {
            xml.newline();
            toc_list(xml, book, &entry.children);
        }
        xml.close("li");
        xml.newline();
    }
    xml.close("ol");
    xml.newline();
}

/// Refuses an XML file that is not well-formed, which a reading system could not read.
pub(crate) fn check_xml(name: &str, data: &[u8]) -> Result<(), EpubError> {
    if !(name.ends_with(".xml") || name.ends_with(".opf") || name.ends_with(".xhtml")) {
        return Ok(());
    }
    let text = std::str::from_utf8(data)
        .map_err(|_| EpubError::Invalid(format!("{name} is not UTF-8")))?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    roxmltree::Document::parse_with_options(text, options)
        .map(|_| ())
        .map_err(|error| EpubError::Invalid(format!("{name} is not well-formed XML: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_xml_is_refused() {
        assert!(check_xml("EPUB/text/c1.xhtml", b"<html><p>open</html>").is_err());
        assert!(check_xml("EPUB/package.opf", b"<package>&nbsp;</package>").is_err());
        assert!(check_xml("META-INF/container.xml", &[0xFF, 0xFE]).is_err());
        assert!(check_xml(
            "EPUB/text/c1.xhtml",
            b"<?xml version=\"1.0\"?>\n<!DOCTYPE html>\n<html/>"
        )
        .is_ok());
        // Not XML: not checked here.
        assert!(check_xml("EPUB/images/a.png", b"<").is_ok());
    }
}
