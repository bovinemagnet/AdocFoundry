//! The view tree and HTML backend over the render fixture corpus.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use adoc_antora::{discover_antora_workspace, parse_resource_id, AntoraCatalog, AntoraResolver};
use adoc_core::{Location, SourceRange};
use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_render::{
    block_at, block_by_trail, build_view, to_html, DiagramView, HtmlOptions, LocalResolver,
    ResolvedLink, ViewInline, ViewKind, ViewResolver, ViewTree,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn view_of(path: &Path, text: &str, resolver: &dyn ViewResolver) -> ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    build_view(path, &dom, resolver)
}

fn corpus() -> (PathBuf, String, ViewTree) {
    let path = fixtures().join("render/blocks.adoc");
    let text = std::fs::read_to_string(&path).unwrap();
    let view = view_of(&path, &text, &LocalResolver);
    (path, text, view)
}

/// Every inline in the tree, depth first.
fn all_inlines(view: &ViewTree) -> Vec<ViewInline> {
    fn collect(inlines: &[ViewInline], out: &mut Vec<ViewInline>) {
        for inline in inlines {
            out.push(inline.clone());
            match inline {
                ViewInline::Strong(content)
                | ViewInline::Emphasis(content)
                | ViewInline::Code(content)
                | ViewInline::Mark(content)
                | ViewInline::Superscript(content)
                | ViewInline::Subscript(content)
                | ViewInline::Span { content, .. }
                | ViewInline::Link { content, .. }
                | ViewInline::Xref { content, .. } => collect(content, out),
                _ => {}
            }
        }
    }
    fn walk(blocks: Vec<&adoc_render::ViewBlock>, out: &mut Vec<ViewInline>) {
        for block in blocks {
            match &block.kind {
                ViewKind::Paragraph(content) => collect(content, out),
                ViewKind::List { items, .. } => {
                    for item in items {
                        collect(&item.content, out);
                    }
                }
                _ => {}
            }
            walk(block.child_refs(), out);
        }
    }
    let mut out = Vec::new();
    walk(view.blocks.iter().collect(), &mut out);
    out
}

#[test]
fn builds_sections_toc_and_numbers() {
    let text = "= Guide\n:toc:\n:sectnums:\n\n== Install\n\nText.\n\n=== From source\n\nMore.\n\n[discrete]\n=== Not numbered\n\n== Use\n\n.Steps\n|===\n|a\n|===\n\n.A sample\n====\nInside.\n====\n";
    let view = view_of(Path::new("/guide.adoc"), text, &LocalResolver);

    assert_eq!(
        view.title.as_deref().map(ViewInline::plain_text).as_deref(),
        Some("Guide")
    );
    let toc: Vec<(u8, Option<&str>, &str)> = view
        .toc
        .iter()
        .map(|entry| (entry.level, entry.number.as_deref(), entry.title.as_str()))
        .collect();
    assert_eq!(
        toc,
        [
            (1, Some("1."), "Install"),
            (2, Some("1.1."), "From source"),
            (1, Some("2."), "Use"),
        ]
    );
    assert_eq!(view.toc[0].id, "_install");

    let ViewKind::Section { number, blocks, .. } = &view.blocks[0].kind else {
        panic!("{:?}", view.blocks[0].kind);
    };
    assert_eq!(number.as_deref(), Some("1."));
    assert_eq!(view.blocks[0].id.as_deref(), Some("_install"));
    assert!(blocks
        .iter()
        .any(|block| matches!(block.kind, ViewKind::Section { level: 2, .. })));

    let captions: Vec<&str> = view.blocks[1]
        .child_refs()
        .into_iter()
        .filter_map(|block| block.caption.as_deref())
        .collect();
    assert_eq!(captions, ["Table 1.", "Example 1."]);

    // The corpus has a table of contents and no numbering; the discrete heading stays out.
    let (_, _, corpus) = corpus();
    let titles: Vec<&str> = corpus
        .toc
        .iter()
        .map(|entry| entry.title.as_str())
        .collect();
    assert!(titles.contains(&"Lists"), "{titles:?}");
    assert!(!titles.contains(&"A discrete heading"), "{titles:?}");
    assert!(corpus.toc.iter().all(|entry| entry.number.is_none()));
    assert_eq!(
        corpus
            .toc
            .iter()
            .find(|entry| entry.title == "Lists")
            .map(|entry| entry.id.as_str()),
        Some("lists")
    );
}

struct Workspace;

impl ViewResolver for Workspace {
    fn xref(&self, _from: &Path, target: &str) -> ResolvedLink {
        if target.starts_with("other.adoc") {
            ResolvedLink {
                target: Some(Location {
                    path: PathBuf::from("/docs/other.adoc"),
                    range: SourceRange::new(10, 20),
                }),
                fragment: Some("details".to_owned()),
                title: Some("Details".to_owned()),
                unresolved: false,
            }
        } else {
            ResolvedLink {
                unresolved: true,
                ..ResolvedLink::default()
            }
        }
    }

    fn image(&self, _from: &Path, _target: &str, _imagesdir: Option<&str>) -> Option<PathBuf> {
        None
    }

    fn diagram(
        &self,
        kind: &str,
        source: &str,
        _attributes: &BTreeMap<String, String>,
    ) -> DiagramView {
        DiagramView::Pending {
            kind: kind.to_owned(),
            source: source.to_owned(),
        }
    }
}

#[test]
fn resolves_xrefs_and_marks_unresolved() {
    let text = "= Page\n\n[[here]]\n== Here\n\nSee <<here>>, <<nowhere>>, xref:other.adoc#details[], xref:missing.adoc[Missing] and xref:#here[back].\n";
    let view = view_of(Path::new("/docs/page.adoc"), text, &Workspace);
    let xrefs: Vec<(String, ResolvedLink, String)> = all_inlines(&view)
        .into_iter()
        .filter_map(|inline| match inline {
            ViewInline::Xref {
                target,
                link,
                content,
                ..
            } => Some((target, link, ViewInline::plain_text(&content))),
            _ => None,
        })
        .collect();
    assert_eq!(xrefs.len(), 5, "{xrefs:#?}");

    let (_, here, text) = &xrefs[0];
    assert!(!here.unresolved);
    assert_eq!(here.fragment.as_deref(), Some("here"));
    assert_eq!(
        here.target.as_ref().map(|target| target.path.as_path()),
        Some(Path::new("/docs/page.adoc"))
    );
    assert_eq!(
        text, "Here",
        "a reference without text shows the target's title"
    );

    assert!(xrefs[1].1.unresolved, "{:?}", xrefs[1]);

    let (_, other, text) = &xrefs[2];
    assert!(!other.unresolved);
    assert_eq!(
        other.target.as_ref().map(|target| target.path.as_path()),
        Some(Path::new("/docs/other.adoc"))
    );
    assert_eq!(text, "Details");

    let (_, missing, text) = &xrefs[3];
    assert!(missing.unresolved);
    assert_eq!(text, "Missing");

    assert!(!xrefs[4].1.unresolved);
    assert_eq!(xrefs[4].2, "back");

    let html = to_html(&view, &HtmlOptions::default());
    assert!(html.contains("<a href=\"#here\">Here</a>"), "{html}");
    assert!(html.contains("class=\"unresolved\""), "{html}");
}

/// Resolves images as the engine does inside an Antora module: `image$` resources.
struct AntoraImages(AntoraCatalog);

impl ViewResolver for AntoraImages {
    fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
        LocalResolver.xref(from, target)
    }

    fn image(&self, from: &Path, target: &str, _imagesdir: Option<&str>) -> Option<PathBuf> {
        let context = self.0.context_for_path(from)?;
        let mut id = parse_resource_id(target).ok()?;
        id.family.get_or_insert(adoc_antora::ResourceFamily::Image);
        AntoraResolver::resolve(&self.0, &id, &context)
            .ok()
            .map(|resource| resource.source_path.clone())
    }

    fn diagram(
        &self,
        kind: &str,
        source: &str,
        attributes: &BTreeMap<String, String>,
    ) -> DiagramView {
        LocalResolver.diagram(kind, source, attributes)
    }
}

#[test]
fn resolves_antora_images() {
    let root = fixtures().join("antora-single-component");
    let catalog = discover_antora_workspace(std::slice::from_ref(&root))
        .unwrap()
        .catalog;
    let page = root.join("modules/security/pages/authentication.adoc");
    let text = "= Auth\n\nimage::architecture.svg[]\n\nimage::ROOT:image$architecture.svg[Architecture]\n\nimage::missing.png[Gone]\n\nInline image:ROOT:architecture.svg[icon] too.\n";
    let view = view_of(&page, text, &AntoraImages(catalog));

    let images: Vec<_> = view
        .blocks
        .iter()
        .filter_map(|block| match &block.kind {
            ViewKind::Image(image) => Some(image.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 3);
    // The catalog normalises paths, so compare the tail.
    let expected = Path::new("antora-single-component/modules/ROOT/images/architecture.svg");
    assert!(
        images[0].is_missing(),
        "the security module has no images: {:?}",
        images[0]
    );
    assert_eq!(images[0].alt, "architecture");
    assert!(
        images[1]
            .path
            .as_deref()
            .is_some_and(|path| path.ends_with(expected) && path.is_file()),
        "{:?}",
        images[1]
    );
    assert_eq!(images[1].alt, "Architecture");
    assert!(images[2].is_missing());
    assert_eq!(
        images[2].target, "missing.png",
        "a missing image keeps its id"
    );

    let inline = all_inlines(&view)
        .into_iter()
        .find_map(|inline| match inline {
            ViewInline::Image(image) => Some(image),
            _ => None,
        });
    assert!(
        inline
            .and_then(|image| image.path)
            .is_some_and(|path| path.ends_with(expected)),
        "inline images resolve too"
    );

    // Outside Antora, `imagesdir` applies.
    let guide = fixtures().join("images/guide.adoc");
    let view = view_of(
        &guide,
        "= Guide\n:imagesdir: media\n\nimage::present.svg[]\n",
        &LocalResolver,
    );
    let ViewKind::Image(image) = &view.blocks[0].kind else {
        panic!("{:?}", view.blocks[0].kind);
    };
    assert_eq!(
        image.path.as_deref(),
        Some(fixtures().join("images/media/present.svg").as_path())
    );
}

#[test]
fn html_contains_every_block_kind() {
    let (_, _, view) = corpus();
    let html = to_html(&view, &HtmlOptions { standalone: true });
    for class in [
        "class=\"sect1\"",
        "class=\"sectionbody\"",
        "class=\"discrete\"",
        "class=\"paragraph\"",
        "class=\"ulist\"",
        "class=\"olist arabic\"",
        "class=\"ulist checklist\"",
        "class=\"dlist\"",
        "class=\"colist arabic\"",
        "class=\"admonitionblock note\"",
        "class=\"admonitionblock warning\"",
        "class=\"listingblock\"",
        "class=\"literalblock\"",
        "class=\"quoteblock\"",
        "class=\"verseblock\"",
        "class=\"sidebarblock\"",
        "class=\"exampleblock\"",
        "class=\"openblock\"",
        "class=\"tableblock frame-all grid-all stretch\"",
        "class=\"imageblock\"",
        "class=\"listingblock diagram\"",
        "class=\"stemblock\"",
        "id=\"toc\"",
        "id=\"footnotes\"",
    ] {
        assert!(html.contains(class), "no {class} in\n{html}");
    }
    for fragment in [
        "<!DOCTYPE html>",
        "<title>Every Block Kind</title>",
        "<strong>bold</strong>",
        "<em>italic</em>",
        "<code>mono</code>",
        "<mark>marked</mark>",
        "<span class=\"underline\">role</span>",
        "<sup>sup</sup>",
        "<sub>sub</sub>",
        "about Foundry",
        "<a href=\"https://example.com\" class=\"bare\">a link</a>",
        "<code class=\"language-rust\" data-lang=\"rust\">",
        "<b class=\"conum\">(1)</b>",
        "<b>raw</b>",
        "<hr>",
        "page-break-after",
        "A paragraph from a partial.",
        "&#8212; Ada Lovelace",
        "<th class=\"tableblock",
    ] {
        assert!(html.contains(fragment), "no {fragment} in\n{html}");
    }
    assert!(
        !html.contains("<1>"),
        "callout markers are replaced:\n{html}"
    );

    let fragment = to_html(&view, &HtmlOptions::default());
    assert!(!fragment.contains("<html"));
    assert!(fragment.contains("id=\"content\""));
}

#[test]
fn block_at_finds_the_innermost_block() {
    let (path, text, view) = corpus();
    let at = |needle: &str| {
        let offset = text.find(needle).unwrap();
        let trail = block_at(&view, &path, offset).unwrap_or_else(|| panic!("nothing at {needle}"));
        block_by_trail(&view, &trail).unwrap().clone()
    };

    let block = at("Sidebar text.");
    assert!(
        matches!(&block.kind, ViewKind::Paragraph(content) if ViewInline::plain_text(content) == "Sidebar text."),
        "{block:?}"
    );
    let block = at("A warning block.");
    assert!(matches!(block.kind, ViewKind::Paragraph(_)), "{block:?}");
    let block = at("println!");
    assert!(matches!(block.kind, ViewKind::Code { .. }), "{block:?}");
    let block = at("1 |2");
    assert!(
        matches!(&block.kind, ViewKind::Paragraph(content) if ViewInline::plain_text(content) == "1"),
        "a cell's paragraph, not the whole table: {block:?}"
    );

    // A heading line belongs to its section.
    let block = at("== Verbatim");
    assert!(
        matches!(&block.kind, ViewKind::Section { title, .. } if ViewInline::plain_text(title) == "Verbatim"),
        "{block:?}"
    );

    // Blocks from a partial are found by the partial's path, not the page's.
    let partial = fixtures().join("render/partials/included.adoc");
    let partial = adoc_render_path(&view, &partial);
    let trail = block_at(&view, &partial, 0).unwrap();
    assert!(matches!(
        block_by_trail(&view, &trail).unwrap().kind,
        ViewKind::Paragraph(_)
    ));
    assert_eq!(block_at(&view, Path::new("/elsewhere.adoc"), 0), None);
}

/// The partial's path as the parser recorded it (the page directory joined with the target).
fn adoc_render_path(view: &ViewTree, partial: &Path) -> PathBuf {
    fn find(blocks: Vec<&adoc_render::ViewBlock>, name: &str) -> Option<PathBuf> {
        blocks.into_iter().find_map(|block| {
            if block.span.path.ends_with(name) {
                Some(block.span.path.clone())
            } else {
                find(block.child_refs(), name)
            }
        })
    }
    find(view.blocks.iter().collect(), "partials/included.adoc").unwrap_or(partial.to_path_buf())
}

#[test]
fn html_follows_asciidoctor_where_forms_differ() {
    let text = "= Page\n\nNOTE: Inline note.\n\n[NOTE]\n====\nBlock note.\n====\n\n|===\n|Head\n\n|Body\n|===\n\n[mermaid]\n----\ngraph TD\n  A --> B\n----\n";
    struct Rendered;
    impl ViewResolver for Rendered {
        fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
            LocalResolver.xref(from, target)
        }
        fn image(&self, _: &Path, _: &str, _: Option<&str>) -> Option<PathBuf> {
            None
        }
        fn diagram(&self, _: &str, _: &str, _: &BTreeMap<String, String>) -> DiagramView {
            DiagramView::Svg(PathBuf::from("/cache/d.svg"))
        }
    }
    let view = view_of(Path::new("/page.adoc"), text, &Rendered);
    let html = to_html(&view, &HtmlOptions::default());
    // A paragraph-form admonition holds its text directly; a delimited one holds blocks.
    assert!(
        html.contains("<td class=\"content\">\nInline note.\n</td>"),
        "{html}"
    );
    assert!(
        html.contains("<div class=\"paragraph\"><p>Block note.</p></div>"),
        "{html}"
    );
    // Header cells hold their text without a paragraph.
    assert!(html.contains(">Head</th>"), "{html}");
    assert!(html.contains("<p class=\"tableblock\">Body</p>"), "{html}");
    assert!(html.contains("class=\"imageblock diagram\""), "{html}");
}

#[test]
fn html_renders_list_styles_and_asciidoc_cells() {
    let text = "= Page\n:experimental:\n\n[qanda]\nWhat?::\n  This.\n\n[horizontal]\nKey:: Value.\n\n[cols=\"1,1a\"]\n|===\n|Plain\n|First.\n\nSecond.\n|===\n\nmenu:File[Save]\n";
    let view = view_of(Path::new("/page.adoc"), text, &LocalResolver);
    let html = to_html(&view, &HtmlOptions::default());
    assert!(html.contains("<div class=\"qlist qanda\">"), "{html}");
    assert!(
        html.contains("<li><p><em>What?</em></p><p>This.</p>"),
        "{html}"
    );
    assert!(html.contains("<div class=\"hdlist\">"), "{html}");
    assert!(html.contains("<td class=\"hdlist1\">"), "{html}");
    assert!(
        html.contains("<div class=\"content\"><div class=\"paragraph\"><p>First.</p></div>"),
        "an AsciiDoc cell holds blocks: {html}"
    );
    assert!(html.contains("<p class=\"tableblock\">Plain</p>"), "{html}");
    assert!(html.contains("File&#160;&#8250; Save"), "{html}");
}

#[test]
fn file_urls_are_absolute_for_relative_paths() {
    // A page opened by a relative path resolves images and diagrams relative to the working
    // directory; `file://docs/...` would name a host called `docs`.
    struct Relative;
    impl ViewResolver for Relative {
        fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
            LocalResolver.xref(from, target)
        }
        fn image(&self, _: &Path, _: &str, _: Option<&str>) -> Option<PathBuf> {
            Some(PathBuf::from("docs/images/a.png"))
        }
        fn diagram(&self, _: &str, _: &str, _: &BTreeMap<String, String>) -> DiagramView {
            DiagramView::Svg(PathBuf::from("cache/d.svg"))
        }
    }
    let text = "= Page\n\nimage::a.png[A]\n\n[mermaid]\n----\ngraph TD\n  A --> B\n----\n";
    let view = view_of(Path::new("docs/page.adoc"), text, &Relative);
    let html = to_html(&view, &HtmlOptions::default());
    for path in ["docs/images/a.png", "cache/d.svg"] {
        let absolute = std::path::absolute(path).unwrap();
        let expected = format!("<img src=\"file://{}\"", absolute.display());
        assert!(html.contains(&expected), "{path}: {html}");
    }
}

/// Renders a page whose one image resolves to `path`, and returns the image's `src`.
fn image_src(path: &str) -> String {
    struct At(PathBuf);
    impl ViewResolver for At {
        fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
            LocalResolver.xref(from, target)
        }
        fn image(&self, _: &Path, _: &str, _: Option<&str>) -> Option<PathBuf> {
            Some(self.0.clone())
        }
        fn diagram(&self, _: &str, _: &str, _: &BTreeMap<String, String>) -> DiagramView {
            DiagramView::Svg(self.0.clone())
        }
    }
    let view = view_of(
        Path::new("/page.adoc"),
        "= Page\n\nimage::a.png[A]\n",
        &At(PathBuf::from(path)),
    );
    let html = to_html(&view, &HtmlOptions::default());
    let start = html.find("<img src=\"").unwrap() + "<img src=\"".len();
    html[start..start + html[start..].find('"').unwrap()].to_owned()
}

#[cfg(unix)]
#[test]
fn file_urls_percent_encode_reserved_characters() {
    assert_eq!(
        image_src("/my docs/50%/a#b?.png"),
        "file:///my%20docs/50%25/a%23b%3F.png"
    );
}

#[cfg(windows)]
#[test]
fn file_urls_name_windows_drives() {
    assert_eq!(image_src(r"C:\my docs\a.png"), "file:///C:/my%20docs/a.png");
}
