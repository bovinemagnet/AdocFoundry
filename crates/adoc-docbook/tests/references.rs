//! DocBook 6/9 (#654): publication-wide ids and cross references (PRD §5.3, §11 tests 15 and
//! 16). Every document is checked against the DocBook 5.2.1 RELAX NG schema and the export's
//! own check, which refuses a dangling `linkend`.

mod common;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use adoc_core::{Location, SourceRange};
use adoc_docbook::{testing, write_docbook, Book, BookItem, DocBookDocument, DocBookOptions, Page};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_render::{DiagramView, ResolvedLink, ViewResolver};
use common::{elements, text_of};

const XML_ID: (&str, &str) = ("http://www.w3.org/XML/1998/namespace", "id");

/// Resolves `name.adoc[#anchor]` to `<root>/name.adoc`, where the test's pages are; a name
/// starting `missing` resolves to nothing.
struct Pages {
    root: PathBuf,
}

impl ViewResolver for Pages {
    fn xref(&self, _from: &Path, target: &str) -> ResolvedLink {
        let (file, fragment) = match target.split_once('#') {
            Some((file, fragment)) => (file, Some(fragment.to_owned())),
            None => (target, None),
        };
        if file.starts_with("missing") {
            return ResolvedLink {
                unresolved: true,
                ..ResolvedLink::default()
            };
        }
        ResolvedLink {
            target: Some(Location {
                path: self.root.join(file),
                range: SourceRange::new(0, 0),
            }),
            fragment,
            title: None,
            unresolved: false,
        }
    }

    fn image(&self, _from: &Path, _target: &str, _imagesdir: Option<&str>) -> Option<PathBuf> {
        None
    }

    fn diagram(&self, kind: &str, source: &str, _: &BTreeMap<String, String>) -> DiagramView {
        DiagramView::Pending {
            kind: kind.to_owned(),
            source: source.to_owned(),
        }
    }
}

fn page(root: &Path, key: &str, name: &str, text: &str) -> BookItem {
    let path = root.join(name);
    let dom = AcdcParser.parse(&ParseInput::new(&path, text));
    BookItem::Page(Page {
        key: key.to_owned(),
        view: adoc_render::build_view(
            &path,
            &dom,
            &Pages {
                root: root.to_owned(),
            },
        ),
        depth: 1,
    })
}

fn book(root: &Path) -> DocBookDocument {
    DocBookDocument::Book(Book {
        title: "Manual".to_owned(),
        items: vec![
            page(
                root,
                "manual-2.0-ROOT-install",
                "install.adoc",
                "= Install\n\n[#intro]\nWhy install.\n\n== Requirements\n\nA [[spot]]place, and \
                 again [[spot]]here.\n\n[#Überblick]\n== Überblick\n\nText.\n\n[#a:b]\nOdd id.\n",
            ),
            page(
                root,
                "manual-2.0-ROOT-use",
                "use.adoc",
                "= Use\n\n[#intro]\nWhy use.\n\nSee xref:install.adoc[], \
                 xref:install.adoc#_requirements[its requirements], \
                 xref:install.adoc#spot[the spot], <<intro>>, xref:install.adoc#Überblick[], \
                 xref:outside.adoc#part[outside with an address], xref:elsewhere.adoc[elsewhere], \
                 xref:missing.adoc[a missing page] and xref:install.adoc#nowhere[nothing there].\n",
            ),
            page(
                root,
                "manual-2.0-ROOT-use",
                "again.adoc",
                "= Again\n\n[#intro]\nRepeated key.\n",
            ),
        ],
        ..Book::default()
    })
}

fn written(document: &DocBookDocument, options: &DocBookOptions) -> adoc_docbook::DocBookOutput {
    let output = write_docbook(document, options).expect("written");
    testing::assert_valid(&output.xml);
    output
}

#[test]
fn docbook_ids_are_unique_and_stable() {
    let root = Path::new("/one/checkout/pages");
    let output = written(&book(root), &DocBookOptions::default());
    let document = testing::parse(&output.xml);
    let ids: Vec<&str> = document
        .descendants()
        .filter_map(|node| node.attribute(XML_ID))
        .collect();
    // Pages are their keys, anchors the page's key and the anchor; a repeated key or anchor
    // gets a number; what no name may hold is `_`.
    for expected in [
        "manual-2.0-ROOT-install",
        "manual-2.0-ROOT-install.intro",
        "manual-2.0-ROOT-install._requirements",
        "manual-2.0-ROOT-install.spot",
        "manual-2.0-ROOT-install.Überblick",
        "manual-2.0-ROOT-install.a_b",
        "manual-2.0-ROOT-use",
        "manual-2.0-ROOT-use.intro",
        "manual-2.0-ROOT-use-2",
    ] {
        assert!(ids.contains(&expected), "{expected} in {ids:?}");
    }
    let unique: std::collections::BTreeSet<&&str> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    // An anchor defined twice on a page is written once.
    assert_eq!(
        ids.iter()
            .filter(|id| **id == "manual-2.0-ROOT-install.spot")
            .count(),
        1
    );

    // The same pages from another checkout give the same document.
    let elsewhere = written(
        &book(Path::new("/elsewhere/clone/docs")),
        &DocBookOptions::default(),
    );
    assert_eq!(output.xml, elsewhere.xml);
    assert!(!output.xml.contains("/one/checkout"));

    // An article keeps its anchors as they are.
    let output = written(
        &common::article("= Guide\n\n== Usage\n\n[#step-1]\nFirst.\n\nSee <<step-1>>.\n"),
        &DocBookOptions::default(),
    );
    let document = testing::parse(&output.xml);
    let ids: Vec<&str> = document
        .descendants()
        .filter_map(|node| node.attribute(XML_ID))
        .collect();
    assert!(
        ids.contains(&"_usage") && ids.contains(&"step-1"),
        "{ids:?}"
    );
    let link = elements(&document, "xref").next().unwrap();
    assert_eq!(link.attribute("linkend"), Some("step-1"));
}

#[test]
fn docbook_references_resolve() {
    let root = Path::new("/docs/pages");
    let options = DocBookOptions {
        page_urls: [(
            root.join("outside.adoc"),
            "https://docs.example.com/outside".to_owned(),
        )]
        .into_iter()
        .collect(),
        ..DocBookOptions::default()
    };
    let output = written(&book(root), &options);
    let document = testing::parse(&output.xml);
    let use_page = elements(&document, "chapter")
        .find(|chapter| chapter.attribute(XML_ID) == Some("manual-2.0-ROOT-use"))
        .unwrap();
    let links: Vec<(String, String)> = use_page
        .descendants()
        .filter(|node| {
            node.has_tag_name((adoc_docbook::NAMESPACE, "link"))
                || node.has_tag_name((adoc_docbook::NAMESPACE, "xref"))
        })
        .map(|node| {
            let target = node
                .attribute("linkend")
                .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")))
                .unwrap()
                .to_owned();
            (target, text_of(node))
        })
        .collect();
    assert_eq!(
        links,
        [
            ("manual-2.0-ROOT-install".to_owned(), String::new()),
            (
                "manual-2.0-ROOT-install._requirements".to_owned(),
                "its requirements".to_owned()
            ),
            (
                "manual-2.0-ROOT-install.spot".to_owned(),
                "the spot".to_owned()
            ),
            ("manual-2.0-ROOT-use.intro".to_owned(), String::new()),
            (
                "manual-2.0-ROOT-install.Überblick".to_owned(),
                "Überblick".to_owned()
            ),
            (
                "https://docs.example.com/outside#part".to_owned(),
                "outside with an address".to_owned()
            ),
        ]
    );
    // What cannot lead anywhere in the publication is its text, said so; nothing dangles.
    let text = text_of(use_page);
    for shown in ["elsewhere", "a missing page", "nothing there"] {
        assert!(text.contains(shown), "{shown} in {text}");
    }
    let messages: Vec<&str> = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(messages[0].contains("elsewhere.adoc leads outside the publication"));
    assert!(messages[1].contains("missing.adoc leads to nothing"));
    assert!(messages[2].contains("install.adoc#nowhere names nothing"));
    assert!(output
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.source.is_some()));
}

/// A reference to an anchor written in a partial leads to it in the page that includes it.
#[test]
fn anchors_in_partials_are_found_in_their_page() {
    let root = Path::new("/docs/pages");
    let mut including = match page(
        root,
        "guide-install",
        "install.adoc",
        "= Install\n\nIntro.\n",
    ) {
        BookItem::Page(page) => page,
        _ => unreachable!(),
    };
    // The block as an include gives it: written in the partial, part of the page.
    let partial = Path::new("/docs/partials/steps.adoc");
    let mut block = including.view.blocks[0].clone();
    block.span.path = partial.to_path_buf();
    block.span.include_chain = vec!["partial$steps.adoc".to_owned()];
    block.id = Some("first-step".to_owned());
    including.view.blocks.push(block);
    let partial_root = Pages {
        root: Path::new("/docs/partials").to_owned(),
    };
    let text = "= Use\n\nSee xref:steps.adoc#first-step[the first step].\n";
    let path = root.join("use.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(&path, text));
    let using = Page {
        key: "guide-use".to_owned(),
        view: adoc_render::build_view(&path, &dom, &partial_root),
        depth: 1,
    };
    let output = written(
        &DocBookDocument::Book(Book {
            title: "Guide".to_owned(),
            items: vec![BookItem::Page(including), BookItem::Page(using)],
            ..Book::default()
        }),
        &DocBookOptions::default(),
    );
    let document = testing::parse(&output.xml);
    let link = elements(&document, "link").next().unwrap();
    assert_eq!(link.attribute("linkend"), Some("guide-install.first-step"));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}
