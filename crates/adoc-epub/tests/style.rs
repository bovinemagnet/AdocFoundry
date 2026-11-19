//! EPUB 6/9 (#643): the stylesheet, the metadata and the accessibility baseline (PRD §11 tests
//! 14 to 16).

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_epub::{
    check_stylesheet, testing, write_epub, Document, EpubError, EpubOptions, Modified, Page,
    Severity,
};

fn view(path: &Path, text: &str) -> adoc_render::ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

fn article(text: &str) -> Document {
    Document::Article(Page {
        key: "p0".to_owned(),
        view: view(Path::new("/docs/page.adoc"), text),
        depth: 0,
    })
}

fn options() -> EpubOptions {
    EpubOptions::new(Modified::from_unix_seconds(1_795_084_200).unwrap())
}

fn warnings(output: &adoc_epub::EpubOutput) -> Vec<String> {
    output
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .map(|diagnostic| diagnostic.message.clone())
        .collect()
}

/// What the package document's `metadata` holds: each element or property and its value.
fn metadata(bytes: &[u8]) -> Vec<(String, String)> {
    let (files, package) = testing::assert_valid_ocf(bytes);
    let opf = testing::parse("opf", testing::text(&files, &package.path));
    let metadata = opf
        .descendants()
        .find(|node| node.has_tag_name((testing::OPF, "metadata")))
        .unwrap();
    metadata
        .children()
        .filter(|node| node.is_element())
        .map(|node| {
            let name = node
                .attribute("property")
                .map_or_else(|| node.tag_name().name().to_owned(), str::to_owned);
            (name, node.text().unwrap_or_default().to_owned())
        })
        .collect()
}

/// PRD §11 test 14.
#[test]
fn epub_metadata_is_complete_or_reported() {
    let page = "= The Manual\n:lang: en-GB\n:author: Ann Author\n:description: How it works.\n\
                :keywords: guides, manuals ,\n:revdate: 2026-11-19\n:revnumber: 2.0\n\nText.\n";
    let output = write_epub(&article(page), &options()).unwrap();
    let held = metadata(&output.bytes);
    let value = |name: &str| -> Vec<&str> {
        held.iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    };
    assert_eq!(value("title"), ["The Manual"]);
    assert_eq!(value("language"), ["en-GB"]);
    assert_eq!(value("creator"), ["Ann Author"]);
    assert_eq!(value("description"), ["How it works."]);
    assert_eq!(value("subject"), ["guides", "manuals"]);
    assert_eq!(value("date"), ["2026-11-19"]);
    assert_eq!(value("dcterms:modified"), ["2026-11-19T10:30:00Z"]);
    let identifier = value("identifier");
    assert!(identifier[0].starts_with("urn:uuid:"), "{identifier:?}");
    assert!(warnings(&output).is_empty(), "{:?}", warnings(&output));

    // The export's own metadata wins over the page's.
    let mut given = options();
    given.book.title = Some("Given Title".to_owned());
    given.book.language = Some("fr".to_owned());
    given.book.identifier = Some("urn:isbn:9780000000002".to_owned());
    let output = write_epub(&article(page), &given).unwrap();
    let held = metadata(&output.bytes);
    assert!(held.contains(&("title".to_owned(), "Given Title".to_owned())));
    assert!(held.contains(&("language".to_owned(), "fr".to_owned())));
    assert!(held.contains(&("identifier".to_owned(), "urn:isbn:9780000000002".to_owned())));

    // Nothing is invented silently: a missing title and language, and one that is no
    // language tag, are each said; a free-text date is not written as a date.
    let output = write_epub(
        &article("Just text, with no title.\n\n:revdate: last spring\n"),
        &options(),
    )
    .unwrap();
    let said = warnings(&output);
    assert!(
        said.iter().any(|message| message.contains("no title")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|message| message.contains("names no language")),
        "{said:?}"
    );
    let held = metadata(&output.bytes);
    assert!(held.iter().all(|(key, _)| key != "date"));
    assert!(held.contains(&("language".to_owned(), "en".to_owned())));
    let output = write_epub(&article("= Title\n:lang: English\n\nText.\n"), &options()).unwrap();
    assert!(
        warnings(&output)
            .iter()
            .any(|message| message.contains("\"English\" is not a language tag")),
        "{:?}",
        warnings(&output)
    );
}

/// PRD §11 test 15.
#[test]
fn epub_css_has_no_external_dependencies() {
    let page = "= Styled\n:lang: en\n\nText.\n";
    let output = write_epub(&article(page), &options()).unwrap();
    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    let css = testing::text(&files, "EPUB/css/book.css");
    for needle in ["@import", "url(", "@font-face", "http"] {
        assert!(
            !css.to_ascii_lowercase().contains(needle),
            "{needle} in the default style"
        );
    }
    assert_eq!(
        package.manifest["css"],
        ("EPUB/css/book.css".to_owned(), "text/css".to_owned(), None)
    );
    // Every document links the stylesheet, which is in the book.
    for document in ["EPUB/nav.xhtml", "EPUB/text/c1.xhtml"] {
        let parsed = testing::parse(document, testing::text(&files, document));
        let links: Vec<String> = parsed
            .descendants()
            .filter(|node| node.has_tag_name((testing::XHTML, "link")))
            .map(|link| testing::resolve(document, link.attribute("href").unwrap()))
            .collect();
        assert_eq!(links, ["EPUB/css/book.css"], "{document}");
    }

    // A user stylesheet is linked after the default one, to override it.
    let mut styled = options();
    styled.stylesheet = Some("body { font-family: serif; }\nh1 { color: #333; }\n".to_owned());
    let output = write_epub(&article(page), &styled).unwrap();
    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    assert_eq!(package.manifest["css-user"].0, "EPUB/css/user.css");
    let chapter = testing::text(&files, "EPUB/text/c1.xhtml");
    let default_at = chapter.find("../css/book.css").unwrap();
    let user_at = chapter.find("../css/user.css").unwrap();
    assert!(default_at < user_at);

    // One that could reach outside the book, or run anything, is refused, and nothing is
    // written.
    for stylesheet in [
        "@import url(https://example.com/a.css);",
        "@IMPORT 'other.css';",
        "body { background: url(https://example.com/x.png); }",
        "body { background: URL(images/i1.png); }",
        "@font-face { font-family: X; src: local(X); }",
        "p { behavior: url(x.htc); }",
        "p { width: expression(alert(1)); }",
        "p { color: \\72 ed; }",
        "</style><script>alert(1)</script>",
    ] {
        assert!(check_stylesheet(stylesheet).is_err(), "{stylesheet}");
        let mut options = options();
        options.stylesheet = Some(stylesheet.to_owned());
        let result = write_epub(&article(page), &options);
        assert!(matches!(result, Err(EpubError::Invalid(_))), "{stylesheet}");
    }
    assert!(check_stylesheet("p { margin: 0 }").is_ok());
}

/// PRD §11 test 16.
#[test]
fn epub_accessibility_structure() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/docx/themes/brand-logo.png"),
        folder.path().join("logo.png"),
    )
    .unwrap();
    let good =
        "= Accessible\n:lang: en-GB\n\n== One\n\nText.\n\n=== Two\n\nimage::logo.png[The logo]\n\n\
                [%header]\n|===\n|Name |Value\n|a |1\n|===\n";
    let path = folder.path().join("good.adoc");
    let mut options = options();
    options.book.access = adoc_core::FileAccess::within([folder.path().to_path_buf()]);
    let document = Document::Article(Page {
        key: "p0".to_owned(),
        view: view(&path, good),
        depth: 0,
    });
    let output = write_epub(&document, &options).unwrap();
    assert!(warnings(&output).is_empty(), "{:?}", warnings(&output));
    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    assert_eq!(package.language, "en-GB");
    let chapter = testing::parse("c1", testing::text(&files, "EPUB/text/c1.xhtml"));
    // The language, on the document.
    assert_eq!(
        chapter
            .root_element()
            .attribute(("http://www.w3.org/XML/1998/namespace", "lang")),
        Some("en-GB")
    );
    // Headings in order, with no level skipped.
    let levels: Vec<u8> = chapter
        .descendants()
        .filter_map(|node| {
            let name = node.tag_name().name();
            (node.tag_name().namespace() == Some(testing::XHTML)
                && name.len() == 2
                && name.starts_with('h'))
            .then(|| name[1..].parse().ok())
            .flatten()
        })
        .collect();
    assert_eq!(levels, [1, 2, 3]);
    // The picture's description, and the table's column headers.
    let img = chapter
        .descendants()
        .find(|node| node.has_tag_name((testing::XHTML, "img")))
        .unwrap();
    assert_eq!(img.attribute("alt"), Some("The logo"));
    assert_eq!(
        chapter
            .descendants()
            .filter(|node| node.has_tag_name((testing::XHTML, "th")))
            .count(),
        2
    );
    // Landmarks, and accessibility metadata the book can stand behind.
    let nav = testing::text(&files, &package.nav);
    assert!(nav.contains("epub:type=\"landmarks\""));
    let held = metadata(&output.bytes);
    for wanted in [
        ("schema:accessMode", "textual"),
        ("schema:accessMode", "visual"),
        ("schema:accessibilityFeature", "tableOfContents"),
        ("schema:accessibilityFeature", "structuralNavigation"),
        ("schema:accessibilityHazard", "unknown"),
    ] {
        assert!(
            held.contains(&(wanted.0.to_owned(), wanted.1.to_owned())),
            "{wanted:?} in {held:?}"
        );
    }
    let summary = held
        .iter()
        .find(|(key, _)| key == "schema:accessibilitySummary")
        .unwrap();
    assert!(summary.1.contains("has not been checked"), "{summary:?}");

    // Each absence is warned about, at its line: no language, a skipped level, a picture with
    // no description and a table with no header row.
    let poor = "= Poor\n\n=== Too deep\n\nimage::logo.png[]\n\n|===\n|a |1\n|===\n";
    let document = Document::Article(Page {
        key: "p0".to_owned(),
        view: view(&folder.path().join("poor.adoc"), poor),
        depth: 0,
    });
    let output = write_epub(&document, &options).unwrap();
    let said: Vec<(String, Option<u32>)> = output
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .map(|diagnostic| {
            (
                diagnostic.message.clone(),
                diagnostic.source.as_ref().map(|span| span.line),
            )
        })
        .collect();
    let at = |needle: &str| {
        said.iter()
            .find(|(message, _)| message.contains(needle))
            .unwrap_or_else(|| panic!("no warning about {needle}: {said:?}"))
            .1
    };
    assert_eq!(at("names no language"), None);
    assert_eq!(at("skipping a level"), Some(3));
    assert_eq!(at("has no description"), Some(5));
    assert_eq!(at("no header row"), Some(7));
}
