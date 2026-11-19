//! The files of a book and the order they are archived in: `mimetype`, the container, the
//! package document, the navigation document and the chapters. Paths are fixed and named from
//! the book's own chapter ids, never from a source path.

use adoc_ebook::{Book, Chapter, Target, TocEntry};

use crate::{xml::Xml, zip::ZipWriter, EpubError, Modified};

/// The exact bytes of the first entry (EPUB 3.3, OCF §4.3).
pub(crate) const MIMETYPE: &[u8] = b"application/epub+zip";
/// Where the container says the package document is.
pub(crate) const PACKAGE: &str = "EPUB/package.opf";
const NAV: &str = "nav.xhtml";
const XHTML: &str = "application/xhtml+xml";

/// The book's archive, every XML file checked first.
pub(crate) fn write(book: &Book, modified: Modified) -> Result<Vec<u8>, EpubError> {
    let files = files(book, modified);
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
fn files(book: &Book, modified: Modified) -> Vec<(String, Vec<u8>)> {
    let mut files = vec![
        ("META-INF/container.xml".to_owned(), container()),
        (PACKAGE.to_owned(), package(book, modified)),
        (format!("EPUB/{NAV}"), nav(book)),
    ];
    for (index, chapter) in book.chapters.iter().enumerate() {
        files.push((
            format!("EPUB/{}", chapter_file(chapter)),
            crate::xhtml::chapter(book, index),
        ));
    }
    files
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

fn package(book: &Book, modified: Modified) -> Vec<u8> {
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
    xml.element(
        "meta",
        &[("property", "dcterms:modified")],
        &modified.to_w3c(),
    );
    xml.newline();
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
    xml.close("manifest");
    xml.newline();
    xml.open("spine", &[]);
    xml.newline();
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

/// The navigation document: the book's table of contents as one `toc` navigation, or, for a
/// book with none, its chapters in order.
fn nav(book: &Book) -> Vec<u8> {
    let language = &book.metadata.language;
    let mut xml = Xml::xhtml(language, &book.metadata.title);
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
    xml.close("body");
    xml.close("html");
    xml.newline();
    xml.into_bytes()
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
