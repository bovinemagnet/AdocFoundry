//! Building the model from the fixture corpus and this repository's documentation.

use std::path::{Path, PathBuf};

use adoc_core::SourceRange;
use adoc_dom::{AcdcParser, BlockKind, DocumentParser, Inline, ParseInput};
use adoc_model::{Access, Model, Node, NodeKind, Piece, ReadOnly};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn build_text(path: &Path, text: &str) -> Model {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    Model::build(path, text, &dom)
}

fn build(path: &Path) -> Model {
    let text = std::fs::read_to_string(path).unwrap();
    build_text(path, &text)
}

/// Every `.adoc` file of the fixtures and the documentation site.
fn corpus() -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "adoc")
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(&repository().join("tests/fixtures"), &mut out);
    visit(&repository().join("src/docs"), &mut out);
    out.sort();
    assert!(out.len() > 30, "the corpus is missing: {}", out.len());
    out
}

fn kind_name(node: &Node) -> String {
    match &node.kind {
        NodeKind::Verbatim { name } => format!("verbatim:{name}"),
        NodeKind::List { kind } => format!("list:{kind:?}"),
        NodeKind::Admonition { delimited, .. } => format!("admonition:{delimited}"),
        other => format!("{other:?}")
            .split([' ', '{', '('])
            .next()
            .unwrap_or("")
            .to_owned(),
    }
}

#[test]
fn builds_every_block_kind() {
    let model = build(&repository().join("tests/fixtures/render/blocks.adoc"));
    let kinds: Vec<String> = model
        .walk()
        .iter()
        .map(|(_, node)| kind_name(node))
        .collect();
    for expected in [
        "Title",
        "Section",
        "Paragraph",
        "Heading",
        "list:Unordered",
        "list:Ordered",
        "list:Checklist",
        "ListItem",
        "DescriptionList",
        "DescriptionItem",
        "Term",
        "Description",
        "admonition:false",
        "admonition:true",
        "Code",
        "Literal",
        "Quote",
        "Sidebar",
        "Example",
        "Open",
        "Table",
        "Row",
        "Cell",
        "Image",
        "Include",
        "verbatim:verse",
        "verbatim:callout-list",
        "verbatim:diagram",
        "verbatim:stem",
        "verbatim:passthrough",
        "verbatim:thematic-break",
        "verbatim:page-break",
    ] {
        assert!(
            kinds.iter().any(|kind| kind == expected),
            "no {expected} in {kinds:?}"
        );
    }
    let code = model
        .walk()
        .into_iter()
        .find(|(_, node)| matches!(node.kind, NodeKind::Code { .. }))
        .unwrap()
        .1;
    assert_eq!(
        code.text.as_ref().unwrap().display,
        "fn main() {\n    println!(\"hi\"); // <1>\n}"
    );
    assert_eq!(
        code.kind,
        NodeKind::Code {
            language: Some("rust".to_owned())
        }
    );
}

/// The text `pieces` show.
fn shown(source: &str, pieces: &[Piece]) -> String {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text { range } => source[range.start..range.end].to_owned(),
            Piece::Mark { children, .. } => shown(source, children),
            Piece::Atom { shown, .. } => shown.clone(),
        })
        .collect()
}

/// Checks that pieces tile their text block and that children nest inside their parents in
/// order, returning the problems found.
fn check_tiling(model: &Model) -> Vec<String> {
    fn pieces(run: &[Piece], range: SourceRange, problems: &mut Vec<String>) {
        let mut at = range.start;
        for piece in run {
            let covered = piece.range();
            if covered.start != at {
                problems.push(format!("gap or overlap at {at}..{}", covered.start));
            }
            if let Piece::Mark {
                open,
                close,
                children,
                ..
            } = piece
            {
                pieces(children, SourceRange::new(open.end, close.start), problems);
            }
            at = covered.end;
        }
        if at != range.end {
            problems.push(format!("pieces end at {at}, not {}", range.end));
        }
    }
    fn nodes(source: &str, siblings: &[Node], parent: SourceRange, problems: &mut Vec<String>) {
        let mut previous_end = parent.start;
        for node in siblings {
            let included = matches!(node.access, Access::ReadOnly(ReadOnly::Included { .. }));
            if !included {
                if node.range.start < previous_end || node.range.end > parent.end {
                    problems.push(format!(
                        "{:?} at {}..{} is outside {}..{} or before {previous_end}",
                        node.kind, node.range.start, node.range.end, parent.start, parent.end
                    ));
                }
                previous_end = node.range.end;
            }
            if let Some(text) = &node.text {
                if text.range.start < node.range.start || text.range.end > node.range.end {
                    problems.push(format!("text of {:?} lies outside the node", node.kind));
                }
                pieces(&text.pieces, text.range, problems);
                if shown(source, &text.pieces) != text.display {
                    problems.push(format!("{:?} does not show its pieces", text.display));
                }
            }
            nodes(source, &node.children, node.range, problems);
        }
    }
    let mut problems = Vec::new();
    nodes(
        &model.source,
        &model.blocks,
        SourceRange::new(0, model.source.len()),
        &mut problems,
    );
    problems
}

#[test]
fn pieces_tile_every_text_block() {
    for path in corpus() {
        let model = build(&path);
        let problems = check_tiling(&model);
        assert!(problems.is_empty(), "{}: {problems:#?}", path.display());
    }
}

#[test]
fn scanner_agrees_with_the_dom() {
    for name in ["blocks.adoc", "inline.adoc", "tables.adoc"] {
        let model = build(&repository().join("tests/fixtures/render").join(name));
        let unchecked: Vec<String> = model
            .text_blocks()
            .filter(|(_, node, _)| node.access == Access::ReadOnly(ReadOnly::Unchecked))
            .map(|(_, _, text)| text.display.clone())
            .collect();
        assert!(unchecked.is_empty(), "{name}: {unchecked:#?}");
    }
    let (mut editable, mut total) = (0, 0);
    for path in corpus() {
        for (_, node, _) in build(&path).text_blocks() {
            total += 1;
            editable += usize::from(node.is_editable());
        }
    }
    assert!(
        editable * 100 >= total * 99,
        "only {editable} of {total} text blocks are editable"
    );
}

#[test]
fn disagreeing_blocks_are_read_only() {
    let path = Path::new("page.adoc");
    let text = "A *bold* word.\n\nAnother paragraph.\n";
    let mut dom = AcdcParser.parse(&ParseInput::new(path, text));
    // A parser that read the first paragraph without formatting.
    if let BlockKind::Paragraph { inlines } = &mut dom.blocks[0].kind {
        *inlines = vec![Inline::Text("A *bold* word.".to_owned())];
    }
    let model = Model::build(path, text, &dom);
    assert_eq!(
        model.blocks[0].access,
        Access::ReadOnly(ReadOnly::Unchecked)
    );
    assert!(model.blocks[1].is_editable());
}

#[test]
fn included_content_is_read_only() {
    let model = build(&repository().join("tests/fixtures/render/blocks.adoc"));
    let (_, include) = model
        .walk()
        .into_iter()
        .find(|(_, node)| matches!(node.kind, NodeKind::Include { .. }))
        .unwrap();
    assert_eq!(
        &model.source[include.range.start..include.range.end],
        "include::partials/included.adoc[]"
    );
    assert_eq!(include.children.len(), 2);
    for child in &include.children {
        let Access::ReadOnly(ReadOnly::Included { path, target }) = &child.access else {
            panic!("{:?} is editable", child.kind);
        };
        assert!(path.ends_with("partials/included.adoc"));
        assert_eq!(target, "partials/included.adoc");
        assert!(child.origin.is_some());
    }
}

#[test]
fn includes_written_with_attributes_find_their_directives() {
    // The directives name their targets through attributes, and two include one file by
    // different tags: each is one include node over its own line, and none is left as text.
    let model = build(&repository().join("tests/fixtures/includes/attributes/page.adoc"));
    let directives: Vec<&str> = model
        .blocks
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Include { .. }))
        .map(|node| &model.source[node.range.start..node.range.end])
        .collect();
    assert_eq!(
        directives,
        [
            "include::{partialsdir}/overview.adoc[]",
            "include::schema.adoc[tag=description-{QUERY_REF}]",
            "include::schema.adoc[tag=name-{QUERY_REF}]",
        ]
    );
    // The header's attribute entries belong to the title.
    let kinds: Vec<String> = model.blocks.iter().map(kind_name).collect();
    assert_eq!(
        kinds,
        ["Title", "Include", "Include", "Include", "Paragraph"]
    );
    assert!(check_tiling(&model).is_empty());
}

#[test]
fn conditionals_and_attribute_entries_are_read_only() {
    let text = "Before.\n\nifdef::never[]\nHidden text.\nendif::[]\n\n:set-here: value\n\nAfter.\n";
    let model = build_text(Path::new("page.adoc"), text);
    let kinds: Vec<String> = model.blocks.iter().map(kind_name).collect();
    assert_eq!(
        kinds,
        [
            "Paragraph",
            "Conditional",
            "verbatim:attribute",
            "Paragraph"
        ]
    );
    let conditional = &model.blocks[1];
    assert_eq!(conditional.access, Access::ReadOnly(ReadOnly::Inactive));
    assert_eq!(
        &text[conditional.range.start..conditional.range.end],
        "ifdef::never[]\nHidden text.\nendif::[]"
    );
}

#[test]
fn half_typed_input_does_not_panic() {
    for text in [
        "",
        "=",
        "= ",
        "== ",
        "* ",
        "*",
        ". ",
        "[",
        "[source",
        "----",
        "----\ncode",
        "|===",
        "|===\n|a |b",
        "|===\n|a\n|===",
        "NOTE:",
        "Term::",
        "Term:: ",
        "image::",
        "include::",
        "ifdef::x[]",
        "* item\n+",
        "* item\n+\n",
        "[[",
        "....",
        "____\nquote",
        "* [x]",
        "== Title ==",
        ".Title",
        ":attr:",
        "** nested without parent",
        "a +\n",
        "\u{feff}= Bom",
        "日本語 *強調",
        "|===\n2+|span\n|===",
    ] {
        let model = build_text(Path::new("page.adoc"), text);
        assert!(check_tiling(&model).is_empty(), "{text:?}");
    }
}
