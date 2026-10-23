//! WP 4.2: the view tree written as Typst, with every block and inline kind, escaping that
//! keeps document text out of Typst's syntax, and text that survives compilation.

mod common;

use std::path::Path;

use adoc_pdf::{to_typst, typeset_text, PageView, PdfOptions};
use adoc_render::{ViewBlock, ViewInline, ViewKind};

fn typst_of(text: &str) -> adoc_pdf::Typst {
    let view = common::view_of_text(Path::new("/doc/page.adoc"), text);
    to_typst(
        &common::article(view),
        &common::default_theme(),
        &PdfOptions::default(),
    )
}

/// The laid-out body text, with all whitespace removed so line and page breaks do not matter.
fn compiled_text(typst: &adoc_pdf::Typst) -> String {
    let pages = adoc_pdf::typeset_pages(typst, &common::default_theme(), &PdfOptions::default())
        .unwrap_or_else(|error| panic!("{error}\n{}", typst.source));
    // The body only: running content in the margins would land between a page's last line
    // and the next page's first.
    pages
        .iter()
        .flat_map(|page| {
            let height = page.height;
            page.runs
                .iter()
                .filter(move |run| run.y > 36.0 && run.y < height - 48.0)
        })
        .flat_map(|run| run.text.chars())
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn squeeze(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn emits_every_block_kind() {
    let path = common::fixtures().join("render/blocks.adoc");
    let pages = common::article(common::view_of(&path));
    let typst = to_typst(&pages, &common::default_theme(), &PdfOptions::default());
    for call in [
        "#heading(level: 2",
        "#adoc-list(",
        "#adoc-dlist(",
        "#adoc-admonition(\"note\"",
        "#adoc-admonition(\"warning\"",
        "#adoc-code(raw(",
        "#adoc-callouts(",
        "#adoc-quote(",
        "prefix: \"verse\"",
        "#adoc-container(\"sidebar\"",
        "#adoc-container(\"example\"",
        "#adoc-open(",
        "#adoc-break()",
    ] {
        assert!(
            typst.source.contains(call),
            "no {call} in\n{}",
            typst.source
        );
    }
    // And it typesets.
    assert!(!compiled_text(&typst).is_empty());

    let tables = common::fixtures().join("render/tables.adoc");
    let typst = to_typst(
        &common::article(common::view_of(&tables)),
        &common::default_theme(),
        &PdfOptions::default(),
    );
    assert!(typst.source.contains("#adoc-table("));
    assert!(typst.source.contains("table.cell(colspan: 2"));
    assert!(!compiled_text(&typst).is_empty());
}

#[test]
fn emits_every_inline_kind() {
    let typst = typst_of(
        ":experimental:\n\n\
         A *strong*, _emphasis_, `code`, #mark#, ^sup^, ~sub~, [.underline]#role#,\n\
         https://example.com[a link], <<anchor,a reference>>, [[anchor]]an anchor,\n\
         a note.footnote:[The note.] +\n\
         kbd:[Ctrl+T], btn:[OK], menu:File[Save], icon:heart[] and stem:[x^2].\n",
    );
    for call in [
        "#strong[",
        "#emph[",
        "#adoc-codespan[",
        "#adoc-mark[",
        "#adoc-super[",
        "#adoc-sub[",
        "#adoc-role(\"underline\"",
        "#adoc-link(\"https://example.com\"",
        "#adoc-link(label(\"p0#anchor\")",
        "#metadata(none)#label(\"p0#anchor\")",
        "#adoc-footnote-ref(1",
        "#linebreak()",
        "#adoc-kbd((\"Ctrl\", \"T\",))",
        "#adoc-button(\"OK\")",
        "#adoc-menu((\"File\", \"Save\",))",
    ] {
        assert!(
            typst.source.contains(call),
            "no {call} in\n{}",
            typst.source
        );
    }
    let text = compiled_text(&typst);
    for expected in [
        "strong",
        "emphasis",
        "code",
        "areference",
        "Ctrl",
        "[heart]",
        "x^2",
    ] {
        assert!(text.contains(expected), "no {expected} in {text}");
    }
}

#[test]
fn escapes_typst_markup() {
    let corpus = [
        r"#set page(width: 1pt) and #emph[not emphasised]",
        r"$x^2$ is not maths",
        r"<not-a-label> and @not-a-reference",
        r"// not a comment, /* nor this */",
        r"= not a heading",
        r"- not a list",
        r"+ not an enum",
        r"/ Term: not a term",
        r"\ a backslash \\ and \u{41}",
        r#"quotes "double" and 'single'"#,
        r"`raw` and ```raw block```",
        r"~ tilde, -- dashes ---, ... dots",
        r"#{ panic() } #let x = 1",
        r"[brackets] and ]unbalanced[",
    ];
    for line in corpus {
        // `++++` passes the line through Asciidoctor's substitutions untouched, so the text the
        // view holds is exactly the line.
        let typst = typst_of(&format!("++++\n{line}\n++++\n"));
        let text = compiled_text(&typst);
        assert!(text.contains(&squeeze(line)), "{line:?} became {text:?}");
    }
}

/// The plain text of every paragraph whose inlines all come through as written.
fn paragraphs(blocks: &[ViewBlock], out: &mut Vec<String>) {
    fn simple(inlines: &[ViewInline]) -> bool {
        inlines.iter().all(|inline| match inline {
            ViewInline::Text(_) | ViewInline::Raw(_) => true,
            ViewInline::Strong(content)
            | ViewInline::Emphasis(content)
            | ViewInline::Code(content)
            | ViewInline::Mark(content)
            | ViewInline::Span { content, .. } => simple(content),
            _ => false,
        })
    }
    for block in blocks {
        match &block.kind {
            ViewKind::Paragraph(content) if simple(content) => {
                out.push(ViewInline::plain_text(content));
            }
            ViewKind::Section { blocks, .. }
            | ViewKind::Admonition { blocks, .. }
            | ViewKind::Quote { blocks, .. }
            | ViewKind::Sidebar(blocks)
            | ViewKind::Example(blocks)
            | ViewKind::Open(blocks) => paragraphs(blocks, out),
            _ => {}
        }
    }
}

/// The render fixtures, the reference article and every page of this repository's own
/// documentation.
fn corpus() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let mut dirs = vec![
        common::fixtures().join("render"),
        common::fixtures().join("pdf/reference"),
    ];
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/docs/modules");
    for module in std::fs::read_dir(&docs).unwrap() {
        dirs.push(module.unwrap().path().join("pages"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some("adoc") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

#[test]
fn text_round_trips_through_compilation() {
    let mut checked = 0;
    for path in corpus() {
        let view = common::view_of(&path);
        let mut expected = Vec::new();
        paragraphs(&view.blocks, &mut expected);
        let typst = to_typst(
            &common::article(view),
            &common::default_theme(),
            &PdfOptions::default(),
        );
        let text = compiled_text(&typst);
        for paragraph in expected {
            assert!(
                text.contains(&squeeze(&paragraph)),
                "{}: {paragraph:?} is missing",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 200, "only {checked} paragraphs checked");
}

#[test]
fn labels_are_namespaced_per_page() {
    let page = |key: &str, path: &str| PageView {
        key: key.to_owned(),
        view: common::view_of_text(
            Path::new(path),
            "= A page\n\n== Install\n\nSee <<_install>>.\n",
        ),
        depth: 1,
    };
    let book = adoc_pdf::Document::Book(adoc_pdf::Book {
        title: "Two pages".to_owned(),
        items: vec![
            adoc_pdf::BookItem::Page(page("p0", "/doc/a.adoc")),
            adoc_pdf::BookItem::Page(page("p1", "/doc/b.adoc")),
        ],
        ..adoc_pdf::Book::default()
    });
    let typst = to_typst(&book, &common::default_theme(), &PdfOptions::default());
    assert!(typst.source.contains("label(\"p0#_install\")"));
    assert!(typst.source.contains("label(\"p1#_install\")"));
    // Each page's reference leads to its own section, so the labels never clash.
    assert!(!compiled_text(&typst).is_empty());
}

#[test]
fn unresolved_references_stay_text() {
    let typst = typst_of("= Page\n\nSee xref:missing.adoc[the missing page] and <<nowhere>>.\n");
    assert!(typst.source.contains("#\"the missing page\""));
    assert!(
        !typst.source.contains("adoc-link(label("),
        "an unresolved reference links somewhere:\n{}",
        typst.source
    );
}

#[test]
fn passthrough_is_text() {
    let typst = typst_of(
        "Inline +++#set page(width: 10pt)+++ pass.\n\n++++\n#pagebreak() #pagebreak()\n++++\n",
    );
    let pages = typeset_text(&typst, &common::default_theme(), &PdfOptions::default()).unwrap();
    assert_eq!(pages.len(), 1, "passthrough content was obeyed as Typst");
    let text: String = pages.concat();
    assert!(squeeze(&text).contains("#setpage(width:10pt)"));
    assert!(squeeze(&text).contains("#pagebreak()#pagebreak()"));
}
