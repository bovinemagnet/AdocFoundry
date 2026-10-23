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
    assert!(
        text.contains("println!(\"hi\");\n"),
        "the comment before a callout goes with it, as in Asciidoctor: {text:?}"
    );
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

#[test]
fn spans_cover_the_whole_block() {
    // Location ends are exclusive byte offsets, including after a multi-byte character.
    let text = "= Page\n:toc:\n\nSome text.\n\nEnds in é\n\n|===\n|1 |22\n|===\n";
    let dom = AcdcParser.parse(&ParseInput::new(Path::new("/page.adoc"), text));
    let spans: Vec<&str> = dom
        .walk()
        .into_iter()
        .filter(|block| block.kind_name() == "paragraph")
        .map(|block| &text[block.meta.span.range.start..block.meta.span.range.end])
        .collect();
    assert_eq!(spans, ["Some text.", "Ends in é", "1", "22"]);
    assert_eq!(dom.attributes.get("toc").map(String::as_str), Some(""));

    let dom = parse_fixture("blocks.adoc");
    let partial = std::fs::read_to_string(fixture("partials/included.adoc")).unwrap();
    let paragraph = dom
        .walk()
        .into_iter()
        .find(|block| block.meta.span.path.ends_with("partials/included.adoc"))
        .unwrap();
    let range = paragraph.meta.span.range;
    assert_eq!(
        &partial[range.start..range.end],
        "A paragraph from a partial."
    );
}

#[test]
fn antora_image_targets_are_block_images() {
    // acdc does not accept `:` or `$` in a block image target; Antora ids need both.
    let text = "= Page\n\nimage::ROOT:image$architecture.svg[Architecture,200,100]\n\nimage::partial$x.png[]\n\nNot image::a:b.png[] an image.\n";
    let dom = AcdcParser.parse(&ParseInput::new(Path::new("/page.adoc"), text));
    let images: Vec<_> = dom
        .walk()
        .into_iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Image {
                target,
                alt,
                width,
                height,
            } => Some((
                target.clone(),
                alt.clone(),
                width.clone(),
                height.clone(),
                block.meta.span.range,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 2, "{images:?}");
    assert_eq!(images[0].0, "ROOT:image$architecture.svg");
    assert_eq!(images[0].1, "Architecture");
    assert_eq!(images[0].2.as_deref(), Some("200"));
    assert_eq!(images[0].3.as_deref(), Some("100"));
    assert!(text[images[0].4.start..images[0].4.end].starts_with("image::ROOT:"));
    assert_eq!(images[1].0, "partial$x.png");
    assert_eq!(images[1].1, "");
}

#[test]
fn inline_images_keep_alt_and_antora_targets() {
    let text =
        "Icons image:ROOT:icon.svg[Icon] and image:a$b.png[] and image:plain.png[Plain,16] here.\n";
    let dom = AcdcParser.parse(&ParseInput::new(Path::new("/page.adoc"), text));
    let BlockKind::Paragraph { inlines } = &dom.blocks[0].kind else {
        panic!("{:?}", dom.blocks[0].kind);
    };
    let images: Vec<(&str, &str)> = inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Image { target, alt } => Some((target.as_str(), alt.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        images,
        [
            ("ROOT:icon.svg", "Icon"),
            ("a$b.png", ""),
            ("plain.png", "Plain")
        ]
    );
    assert_eq!(
        Inline::plain_text(inlines),
        "Icons Icon and  and Plain here."
    );
}

#[test]
fn replacements_apply_outside_verbatim_content() {
    let text = "Copyright (C) -- it's done...\n\n[source]\n----\na -> b ... (C)\n----\n\n[literal]\nx -> y\n\nkeep \\(C) and +(R)+\n";
    let dom = AcdcParser.parse(&ParseInput::new(Path::new("/page.adoc"), text));
    let blocks = dom.walk();
    let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
        panic!("{:?}", blocks[0].kind);
    };
    assert_eq!(
        Inline::plain_text(inlines),
        "Copyright \u{a9}\u{2009}\u{2014}\u{2009}it\u{2019}s done\u{2026}\u{200b}"
    );
    assert!(blocks.iter().any(|block| matches!(
        &block.kind,
        BlockKind::Listing { text, .. } if text == "a -> b ... (C)"
    )));
    assert!(blocks.iter().any(|block| matches!(
        &block.kind,
        BlockKind::Literal { text } if text == "x -> y"
    )));
    let last = blocks
        .iter()
        .rev()
        .find_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(Inline::plain_text(inlines)),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        last, "keep (C) and (R)",
        "escaped and passed-through text is kept"
    );
}
