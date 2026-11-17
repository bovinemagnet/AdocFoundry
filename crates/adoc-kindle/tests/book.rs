//! The book both writers read: anchors, links, the table of contents and metadata.

mod common;

use adoc_kindle::{KindleBook, KindleOptions, Target};
use common::*;

const INSTALL: &str = "= Install\n\n[[setup]]\n== Set up\n\nSee xref:use.adoc#setup[using it] and <<later>>.\n\n[[later]]\n== Later\n\nText.\n";
const USE: &str = "= Use\n\n[[setup]]\n== Set up\n\nSee xref:install.adoc[the install page].\n";

fn built(folder: &std::path::Path) -> KindleBook {
    let document = book(
        folder,
        "Guide",
        &[("install.adoc", INSTALL), ("use.adoc", USE)],
    );
    KindleBook::build(&document, &KindleOptions::default()).0
}

/// PRD §12 test 3: anchors and chapter identifiers are the same on every run, do not depend
/// on paths, and never collide across pages, even where two pages choose the same id.
#[test]
fn kindle_anchors_are_stable_and_unique() {
    let first = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let nested = elsewhere.path().join("some/other/place");
    std::fs::create_dir_all(&nested).unwrap();

    let book = built(first.path());
    assert_eq!(
        book.anchors(),
        built(first.path()).anchors(),
        "the same run twice"
    );
    // The same pages somewhere else: the same anchors and chapter ids.
    let moved = built(&nested);
    assert_eq!(book.anchors(), moved.anchors());
    let ids = |book: &KindleBook| -> Vec<String> {
        book.chapters
            .iter()
            .map(|chapter| chapter.id.clone())
            .collect()
    };
    assert_eq!(ids(&book), ["c1", "c2"]);
    assert_eq!(ids(&book), ids(&moved));

    // Unique, though both pages define `setup`, and never naming a path.
    let anchors = book.anchors();
    let mut unique: Vec<&String> = anchors.iter().map(|(_, anchor)| anchor).collect();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), anchors.len(), "{anchors:?}");
    for (_, anchor) in &anchors {
        assert!(
            !anchor.contains("install") && !anchor.contains('/'),
            "{anchor}"
        );
    }

    // Every link lands on an anchor of the chapter it names.
    let links = book.internal_links();
    assert_eq!(links.len(), 3, "{links:?}");
    for target in &links {
        if let Some(anchor) = &target.anchor {
            assert!(
                anchors.contains(&(target.chapter, anchor.clone())),
                "{target:?} in {anchors:?}"
            );
        }
    }
    // The first page's link to the second page's `setup` leads there, not to its own.
    let second_setup = Target {
        chapter: 1,
        anchor: Some("c2-a1".to_owned()),
    };
    assert_eq!(links[0], second_setup);
    assert_eq!(links[1].chapter, 0);
    assert_eq!(
        links[2],
        Target {
            chapter: 0,
            anchor: None
        }
    );
}

#[test]
fn the_table_of_contents_follows_pages_and_sections() {
    let folder = tempfile::tempdir().unwrap();
    let book = built(folder.path());
    let titles: Vec<(&str, usize)> = book
        .toc
        .iter()
        .map(|entry| (entry.title.as_str(), entry.children.len()))
        .collect();
    assert_eq!(titles, [("Install", 2), ("Use", 1)]);
    assert_eq!(book.toc[0].children[0].title, "Set up");
    assert_eq!(
        book.toc[0].children[0].target,
        Target {
            chapter: 0,
            anchor: Some("c1-a1".to_owned())
        }
    );
    assert_eq!(book.metadata.title, "Guide");
}

#[test]
fn a_reference_out_of_the_book_is_text_with_a_warning() {
    let document = article("= Page\n\nSee xref:elsewhere.adoc[another page].\n");
    let (book, diagnostics) = KindleBook::build(&document, &KindleOptions::default());
    let text = format!("{:?}", book.chapters[0].blocks);
    assert!(text.contains("another page"), "{text}");
    assert!(!book
        .internal_links()
        .iter()
        .any(|target| target.chapter != 0));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].message.contains("elsewhere.adoc"));
    assert_eq!(
        diagnostics[0].source.as_ref().map(|span| span.line),
        Some(3)
    );
}

#[test]
fn metadata_comes_from_the_attributes_and_the_options() {
    let document = article("= Handbook\nAnn Author\n:lang: en-GB\n:description: How to.\n:revnumber: 2.1\n:revdate: 2026-11-01\n\nText.\n");
    let (book, _) = KindleBook::build(&document, &KindleOptions::default());
    assert_eq!(book.metadata.title, "Handbook");
    assert_eq!(book.metadata.authors, ["Ann Author"]);
    assert_eq!(book.metadata.language, "en-GB");
    assert_eq!(book.metadata.description.as_deref(), Some("How to."));
    assert_eq!(book.metadata.version.as_deref(), Some("2.1"));
    assert_eq!(book.metadata.date.as_deref(), Some("2026-11-01"));
    let options = KindleOptions {
        title: Some("Other".to_owned()),
        creator: Some("Bo".to_owned()),
        language: Some("ja".to_owned()),
        identifier: Some("isbn:1".to_owned()),
        ..KindleOptions::default()
    };
    let (book, _) = KindleBook::build(&document, &options);
    assert_eq!(book.metadata.title, "Other");
    assert_eq!(book.metadata.authors, ["Bo"]);
    assert_eq!(book.metadata.language, "ja");
    assert_eq!(book.metadata.identifier, "isbn:1");
}

#[test]
fn footnotes_lead_both_ways() {
    let document = article("= Page\n\nA claim.footnote:[The source.] Again.footnote:[Another.]\n");
    let (book, _) = KindleBook::build(&document, &KindleOptions::default());
    let blocks = &book.chapters[0].blocks;
    let adoc_kindle::Block::Notes(notes) = blocks.last().unwrap() else {
        panic!("{blocks:?}");
    };
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].anchor, "c1-n1");
    assert_eq!(notes[0].back.as_deref(), Some("c1-r1"));
    let text = format!("{blocks:?}");
    assert!(text.contains("The source."), "{text}");
    let anchors = book.anchors();
    assert!(anchors.contains(&(0, "c1-r1".to_owned())), "{anchors:?}");
}
