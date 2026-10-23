//! The acdc-parser implementation over the render fixture corpus.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use adoc_dom::{
    AcdcParser, BlockKind, DocumentParser, IncludeResolver, Inline, ListKind, NoIncludes,
    ParseInput,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/render")
        .join(name)
}

fn parse_fixture(name: &str) -> adoc_dom::Dom {
    let path = fixture(name);
    let text = std::fs::read_to_string(&path).unwrap();
    AcdcParser.parse(&ParseInput::new(&path, &text))
}

#[test]
fn parses_every_block_kind() {
    let dom = parse_fixture("blocks.adoc");
    let kinds: BTreeSet<&str> = dom.walk().iter().map(|block| block.kind_name()).collect();
    for expected in [
        "section",
        "heading",
        "paragraph",
        "unordered-list",
        "ordered-list",
        "checklist",
        "description-list",
        "callout-list",
        "admonition",
        "listing",
        "literal",
        "quote",
        "verse",
        "sidebar",
        "example",
        "open",
        "table",
        "image",
        "diagram",
        "stem",
        "passthrough",
        "thematic-break",
        "page-break",
    ] {
        assert!(kinds.contains(expected), "no {expected} block in {kinds:?}");
    }
    assert!(!kinds.contains("unsupported"), "{kinds:?}");
    assert_eq!(
        dom.title.as_deref().map(Inline::plain_text).as_deref(),
        Some("Every Block Kind")
    );

    let (language, text, callouts) = dom
        .walk()
        .into_iter()
        .find_map(|block| match &block.kind {
            BlockKind::Listing {
                language,
                text,
                callouts,
            } => Some((language.clone(), text.clone(), callouts.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(language.as_deref(), Some("rust"));
    assert!(text.contains("println!(\"hi\");"));
    assert!(!text.contains("<1>"));
    assert_eq!(callouts.len(), 1);
    assert_eq!(callouts[0].line, 1);

    let (kind, source) = dom
        .walk()
        .into_iter()
        .find_map(|block| match &block.kind {
            BlockKind::Diagram { kind, source } => Some((kind.clone(), source.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(kind, "mermaid");
    assert!(source.contains("A --> B"));

    let checklist = dom
        .walk()
        .into_iter()
        .find_map(|block| match &block.kind {
            BlockKind::List {
                kind: ListKind::Checklist,
                items,
            } => Some(items.clone()),
            _ => None,
        })
        .unwrap();
    let checked: Vec<_> = checklist.iter().map(|item| item.checked).collect();
    assert_eq!(checked, [Some(true), Some(false)]);
}

#[test]
fn inline_formatting_and_references_are_kept() {
    let dom = parse_fixture("blocks.adoc");
    let paragraph = dom
        .walk()
        .into_iter()
        .find_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines }
                if Inline::plain_text(inlines).starts_with("A paragraph") =>
            {
                Some(inlines.clone())
            }
            _ => None,
        })
        .unwrap();
    let has = |test: &dyn Fn(&Inline) -> bool| paragraph.iter().any(test);
    assert!(has(&|inline| matches!(inline, Inline::Bold(_))));
    assert!(has(&|inline| matches!(inline, Inline::Italic(_))));
    assert!(has(&|inline| matches!(inline, Inline::Monospace(_))));
    assert!(has(&|inline| matches!(inline, Inline::Highlight(_))));
    assert!(has(
        &|inline| matches!(inline, Inline::Span { roles, .. } if roles == &["underline"])
    ));
    assert!(has(
        &|inline| matches!(inline, Inline::Xref { target, .. } if target == "other.adoc#details")
    ));
    assert!(has(
        &|inline| matches!(inline, Inline::Link { target, .. } if target == "https://example.com")
    ));
    assert!(has(&|inline| matches!(
        inline,
        Inline::Footnote { number: 1, .. }
    )));
    assert!(
        Inline::plain_text(&paragraph).contains("about Foundry"),
        "attributes are substituted: {}",
        Inline::plain_text(&paragraph)
    );
}

#[test]
fn spans_map_through_includes() {
    let dom = parse_fixture("blocks.adoc");
    let from_partial: Vec<_> = dom
        .walk()
        .into_iter()
        .filter(|block| block.meta.span.path.ends_with("partials/included.adoc"))
        .collect();
    assert_eq!(
        from_partial.len(),
        2,
        "a paragraph and a list come from the partial"
    );
    let list = from_partial
        .iter()
        .find(|block| block.kind_name() == "unordered-list")
        .unwrap();
    assert_eq!(list.meta.span.line, 3);
    assert_eq!(list.meta.span.include_chain, ["partials/included.adoc"]);
    let partial = std::fs::read_to_string(fixture("partials/included.adoc")).unwrap();
    let range = list.meta.span.range;
    assert!(partial[range.start..range.end].starts_with("* An item"));

    // Page content keeps the page's own path and lines.
    let first = &dom.blocks[0];
    assert!(first.meta.span.path.ends_with("render/blocks.adoc"));
    assert!(first.meta.span.include_chain.is_empty());
}

struct Antora;

impl IncludeResolver for Antora {
    fn read(&self, requested: &Path) -> Option<(PathBuf, String)> {
        requested
            .file_name()?
            .to_str()?
            .strip_prefix("partial$")
            .map(|name| {
                (
                    PathBuf::from(format!("/component/partials/{name}")),
                    "Shared text.\n".to_owned(),
                )
            })
    }
}

#[test]
fn antora_attributes_apply() {
    let path = Path::new("/component/pages/index.adoc");
    let text = "= Page\n\nThis is {page-component-name} {page-component-version}.\n\ninclude::partial$shared.adoc[]\n";
    let dom = AcdcParser.parse(&ParseInput {
        path,
        text,
        attributes: vec![
            ("page-component-name".to_owned(), "demo".to_owned()),
            ("page-component-version".to_owned(), "2.0".to_owned()),
        ],
        includes: Arc::new(Antora),
    });
    let texts: Vec<String> = dom
        .walk()
        .into_iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(Inline::plain_text(inlines)),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["This is demo 2.0.", "Shared text."]);
    let shared = dom.walk().into_iter().last().unwrap();
    assert_eq!(
        shared.meta.span.path,
        Path::new("/component/partials/shared.adoc")
    );
    assert_eq!(
        dom.attributes
            .get("page-component-name")
            .map(String::as_str),
        Some("demo")
    );
}

#[test]
fn half_typed_input_does_not_panic() {
    for text in [
        "",
        "=",
        "= ",
        "== Title\n----\nunterminated",
        "|===\n|a |b",
        "* [",
        "[source,\n----",
        "xref:",
        "include::[]",
        "image::[",
        "footnote:[",
        "<<",
        "[[",
        "....\n",
        "++++",
        ":attr",
        "* item\n** nested\n*** deeper\n.",
        "[cols=\"1,\n|===",
        "NOTE:",
        "[mermaid]\n----",
        "\u{feff}= Bom\n",
        "= T\n\nä ö ü 日本 🎉\n",
    ] {
        let dom = AcdcParser.parse(&ParseInput {
            path: Path::new("/draft.adoc"),
            text,
            attributes: Vec::new(),
            includes: Arc::new(NoIncludes),
        });
        let _ = dom.walk();
    }
}

#[test]
fn unsupported_constructs_are_kept() {
    // Nothing the author wrote is silently dropped: an unknown block macro and an unresolved
    // include both leave something visible behind.
    let text = "= Page\n\ncustom::target[option]\n\ninclude::missing.adoc[]\n";
    let dom = AcdcParser.parse(&ParseInput {
        path: Path::new("/page.adoc"),
        text,
        attributes: Vec::new(),
        includes: Arc::new(NoIncludes),
    });
    let all = dom
        .walk()
        .into_iter()
        .map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Inline::plain_text(inlines),
            BlockKind::Unsupported { source, .. } => source.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("custom::target"), "{all:?}");
    assert!(
        all.contains("missing.adoc") || !dom.warnings.is_empty(),
        "{all:?} {:?}",
        dom.warnings
    );
}

#[test]
fn ifeval_arithmetic_is_evaluated() {
    let text = "= Page\n:count: 2\n\nifeval::[{count} + 1 == 3]\nShown.\nendif::[]\n\nifeval::[{count} * 2 > 10]\nHidden.\nendif::[]\n";
    let dom = AcdcParser.parse(&ParseInput {
        path: Path::new("/page.adoc"),
        text,
        attributes: Vec::new(),
        includes: Arc::new(NoIncludes),
    });
    let texts: Vec<String> = dom
        .walk()
        .into_iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(Inline::plain_text(inlines)),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["Shown."]);
}
