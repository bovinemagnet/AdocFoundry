//! EPUB 4/9 (#641): the navigation document, its table of contents and landmarks, and links
//! inside and out of a book (PRD §11 test 10; test 11 is the engine's, over an Antora book).

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_epub::{testing, write_epub, BookItem, Document, EpubOptions, Modified, Page, Plan};

fn view(path: &Path, text: &str) -> adoc_render::ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    adoc_render::build_view(path, &dom, &adoc_render::LocalResolver)
}

fn options() -> EpubOptions {
    EpubOptions::new(Modified::from_unix_seconds(1_795_084_200).unwrap())
}

fn page(folder: &Path, name: &str, text: &str, depth: u8) -> BookItem {
    let path = folder.join(name);
    std::fs::write(&path, text).unwrap();
    BookItem::Page(Page {
        key: name.to_owned(),
        view: view(&path, text),
        depth,
    })
}

/// The table of contents as (depth, title, href), depth first.
fn toc(bytes: &[u8]) -> Vec<(usize, String, String)> {
    let (files, package) = testing::assert_valid_ocf(bytes);
    let nav = testing::parse("nav", testing::text(&files, &package.nav));
    let toc = nav
        .descendants()
        .find(|node| node.attribute((testing::OPS, "type")) == Some("toc"))
        .unwrap();
    let mut entries = Vec::new();
    for a in toc
        .descendants()
        .filter(|node| node.has_tag_name((testing::XHTML, "a")))
    {
        let depth = a
            .ancestors()
            .filter(|node| node.has_tag_name((testing::XHTML, "ol")))
            .count();
        let title = a
            .descendants()
            .filter(|node| node.is_text())
            .filter_map(|node| node.text())
            .collect();
        entries.push((depth, title, a.attribute("href").unwrap().to_owned()));
    }
    entries
}

/// PRD §11 test 10.
#[test]
fn epub_nav_matches_the_plan() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path();
    let document = Document::Book(Plan {
        title: "The Guide".to_owned(),
        items: vec![
            BookItem::Part("First part".to_owned()),
            page(
                folder,
                "a.adoc",
                "= Page A\n\n== Section one\n\nText.\n\n=== Deeper\n\nMore.\n",
                1,
            ),
            page(folder, "b.adoc", "= Page B\n\nText.\n", 2),
            BookItem::Heading {
                title: "A heading".to_owned(),
                depth: 1,
            },
            page(folder, "c.adoc", "= Page C\n\nText.\n", 2),
            BookItem::Part("Second part".to_owned()),
            page(folder, "d.adoc", "= Page D\n\nText.\n", 1),
        ],
        ..Plan::default()
    });
    let output = write_epub(&document, &options()).unwrap();
    assert_eq!(output.chapters, 7);
    let (files, package) = testing::assert_valid_ocf(&output.bytes);
    // The spine is the plan's order, a chapter an item.
    assert_eq!(package.spine, ["c1", "c2", "c3", "c4", "c5", "c6", "c7"]);

    // Exactly one toc navigation, nested as the plan is: parts, then their pages and headings,
    // then a page's sections.
    let entries = toc(&output.bytes);
    let shape: Vec<(usize, &str)> = entries
        .iter()
        .map(|(depth, title, _)| (*depth, title.as_str()))
        .collect();
    assert_eq!(
        shape,
        [
            (1, "First part"),
            (2, "Page A"),
            (3, "Section one"),
            (4, "Deeper"),
            (3, "Page B"),
            (2, "A heading"),
            (3, "Page C"),
            (1, "Second part"),
            (2, "Page D"),
        ]
    );
    // The entries lead to the chapters in the spine's order, and every one resolves.
    let mut order: Vec<&str> = Vec::new();
    for (_, _, href) in &entries {
        let file = href.split('#').next().unwrap();
        if order.last() != Some(&file) {
            order.push(file);
        }
    }
    let spine_files: Vec<String> = package
        .spine
        .iter()
        .map(|id| format!("text/{id}.xhtml"))
        .collect();
    assert_eq!(order, spine_files);
    assert!(
        entries[2].2.starts_with("text/c2.xhtml#"),
        "{:?}",
        entries[2]
    );
    testing::assert_links_resolve(&output.bytes);

    // The landmarks: the start of the text. No `toc` landmark: the navigation document is not
    // in the spine, so a link may not lead to it.
    let nav = testing::text(&files, &package.nav);
    let document = testing::parse("nav", nav);
    let landmarks: Vec<(String, String)> = document
        .descendants()
        .find(|node| node.attribute((testing::OPS, "type")) == Some("landmarks"))
        .expect("landmarks")
        .descendants()
        .filter(|node| node.has_tag_name((testing::XHTML, "a")))
        .map(|a| {
            (
                a.attribute((testing::OPS, "type")).unwrap().to_owned(),
                a.attribute("href").unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        landmarks,
        [("bodymatter".to_owned(), "text/c1.xhtml".to_owned()),]
    );
    let tocs = document
        .descendants()
        .filter(|node| {
            node.has_tag_name((testing::XHTML, "nav"))
                && node.attribute((testing::OPS, "type")) == Some("toc")
        })
        .count();
    assert_eq!(tocs, 1);
}

/// Links between pages of a book, to an id two pages define, to a missing id and out of the
/// book, with and without a published address.
#[test]
fn links_inside_and_out_of_a_book() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path();
    std::fs::write(folder.join("outside.adoc"), "= Outside\n").unwrap();
    // On disk before the page naming it is read, so its references resolve.
    let two = "= Two\n\n[[shared]]\n== Shared too\n\nText.\n";
    std::fs::write(folder.join("two.adoc"), two).unwrap();
    let items = vec![
        page(
            folder,
            "one.adoc",
            "= One\n\n[[shared]]\n== Shared\n\nSee xref:two.adoc#shared[the other shared], \
             xref:two.adoc[two], xref:two.adoc#nowhere[nothing], <<shared>> and \
             xref:outside.adoc[outside].\n",
            1,
        ),
        page(folder, "two.adoc", two, 1),
    ];
    let document = Document::Book(Plan {
        title: "Links".to_owned(),
        items,
        ..Plan::default()
    });

    let output = write_epub(&document, &options()).unwrap();
    let links = testing::assert_links_resolve(&output.bytes);
    let from_one: Vec<(&str, &str)> = links
        .iter()
        .filter(|link| link.from == "EPUB/text/c1.xhtml")
        .map(|link| (link.href.as_str(), link.text.as_str()))
        .collect();
    let two_shared = from_one
        .iter()
        .find(|(_, text)| *text == "the other shared")
        .unwrap_or_else(|| panic!("{from_one:?} {:?}", output.diagnostics));
    let own_shared = from_one
        .iter()
        .find(|(href, _)| href.starts_with('#'))
        .unwrap_or_else(|| {
            panic!(
                "a reference in the chapter is a fragment: {from_one:?} {:?}",
                output.diagnostics
            )
        });
    // Each page keeps its own `shared`: the two links lead to different places.
    assert!(two_shared.0.starts_with("c2.xhtml#"), "{two_shared:?}");
    assert_ne!(two_shared.0.trim_start_matches("c2.xhtml"), own_shared.0);
    assert!(from_one.contains(&("c2.xhtml", "two")), "{from_one:?}");
    // The missing id and the page outside the book are text, each with a warning.
    assert!(!from_one
        .iter()
        .any(|(_, text)| *text == "nothing" || *text == "outside"));
    for wanted in ["names nothing in the book", "leads outside the book"] {
        assert!(
            output.diagnostics.iter().any(
                |diagnostic| diagnostic.message.contains(wanted) && diagnostic.source.is_some()
            ),
            "{wanted}: {:?}",
            output.diagnostics
        );
    }

    // With the page's published address, the link out of the book is that address.
    let mut options = options();
    options.book.page_urls.insert(
        folder.join("outside.adoc"),
        "https://docs.example.com/outside.html".to_owned(),
    );
    let output = write_epub(&document, &options).unwrap();
    let links = testing::assert_links_resolve(&output.bytes);
    assert!(
        links
            .iter()
            .any(|link| link.href == "https://docs.example.com/outside.html"
                && link.text == "outside")
    );
}
