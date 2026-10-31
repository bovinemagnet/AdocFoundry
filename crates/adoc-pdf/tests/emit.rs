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
    // Which blocks are known to fit on a page is checked on its own.
    let source = typst.source.replace("fits: true, ", "");
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
        assert!(source.contains(call), "no {call} in\n{source}");
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
        "#adoc-stem(\"x^(2)\", \"x^2\", block: false)",
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
        // The formula `x^2`, typeset: a mathematical italic x.
        "\u{1d465}2",
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

#[test]
fn only_blocks_that_may_not_fit_are_measured() {
    // A block asciidoctor-pdf keeps together is measured only when its text cannot show that
    // it fits on a page: measuring every block cost most of a book's memory.
    let long_code: String = (1..=200).map(|line| format!("line {line}\n")).collect();
    let text = format!(
        "= Page\n\nNOTE: A short note.\n\n----\nshort code\n----\n\n----\n{long_code}----\n\n====\nAn example with a picture.\n\nimage::missing.png[]\n====\n"
    );
    let view = common::view_of_text(std::path::Path::new("/doc/page.adoc"), &text);
    let typst = to_typst(
        &common::article(view),
        &common::default_theme(),
        &PdfOptions::default(),
    );
    let calls = |name: &str| -> Vec<String> {
        typst
            .source
            .lines()
            .filter(|line| line.starts_with(name))
            .map(str::to_owned)
            .collect()
    };
    let fits = |call: &str| call.contains("(fits: true, ");
    let admonitions = calls("#adoc-admonition(");
    assert!(fits(&admonitions[0]), "{admonitions:?}");
    let code = calls("#adoc-code(");
    assert!(fits(&code[0]), "{code:?}");
    assert!(!fits(&code[1]), "the long listing is measured");
    let containers = calls("#adoc-container(");
    assert!(
        !fits(&containers[0]),
        "an image's height is not known from text"
    );
}

#[test]
fn source_blocks_are_highlighted_as_the_preview_highlights_them() {
    let page = ":source-highlighter: rouge\n\n[source,rust]\n----\nfn main() { // <1>\n\tlet s = \"x\";\n}\n----\n<1> The entry point.\n";
    let typst = typst_of(&format!("= Page\n{page}"));
    // Each line's runs in the preview's light colours: `fn` a keyword, the string green.
    assert!(
        typst.source.contains("adoc-highlighted((("),
        "{}",
        typst.source
    );
    assert!(
        typst.source.contains("(\"fn\", rgb(4, 51, 255)"),
        "{}",
        typst.source
    );
    assert!(
        typst.source.contains("(\"\\\"x\\\"\", rgb(3, 106, 7)"),
        "{}",
        typst.source
    );
    // The callout (its comment marker removed, as Asciidoctor removes it) stays on its line, and the tab is expanded as `raw` expands it.
    let text = compiled_text(&typst);
    assert!(text.contains("fnmain(){①"), "{text}");
    assert!(text.contains("lets=\"x\";"), "{text}");
}

#[test]
fn highlighted_code_is_never_typst() {
    for line in [
        r#"")); #panic("injected") (("#,
        r"#set page(width: 1pt) $x$ <label> @ref",
        r"] [ \ \u{41} // /* */",
    ] {
        let typst = typst_of(&format!(
            "= Page\n:source-highlighter: rouge\n\n[source,bash]\n----\necho '{line}'\n----\n"
        ));
        assert!(
            typst.source.contains("adoc-highlighted((("),
            "{}",
            typst.source
        );
        let text = compiled_text(&typst);
        assert!(text.contains(&squeeze(line)), "{line:?} became {text:?}");
    }
}

#[test]
fn languages_without_a_grammar_keep_typst_s_highlighter() {
    let typst = typst_of("= Page\n:source-highlighter: rouge\n\n[source,xml]\n----\n<a/>\n----\n");
    assert!(!typst.source.contains("adoc-highlighted((("));
    assert!(typst.source.contains("lang: \"xml\""), "{}", typst.source);
    // Without `source-highlighter`, nothing is highlighted, as asciidoctor-pdf does.
    let typst = typst_of("= Page\n\n[source,rust]\n----\nfn main() {}\n----\n");
    assert!(
        !typst.source.contains("adoc-highlighted(((") && !typst.source.contains("lang: \"rust\"")
    );
}

#[test]
fn emits_list_styles_and_table_options() {
    let typst = typst_of(
        "= Options\n\n\
         [loweralpha,start=3]\n. three\n. four\n\n\
         [%reversed]\n. two\n. one\n\n//-\n\n\
         . outer\n.. nested\n\n\
         [cols=\"1,1\",frame=ends,grid=rows,stripes=even,width=50%]\n|===\n|A |B\n|C |D\n|===\n\n\
         [%autowidth]\n|===\n|Narrow\n|===\n\n\
         .Shown on request\n[%collapsible]\n====\nInside.\n====\n",
    );
    let source = &typst.source;
    // The style's letters from the start given, a count down, and the nested list's letters.
    for markers in [
        "#adoc-list(([#\"c.\"], [#\"d.\"],)",
        "#adoc-list(([#\"2.\"], [#\"1.\"],)",
        "#adoc-list(([#\"a.\"],)",
    ] {
        assert!(source.contains(markers), "no {markers} in\n{source}");
    }
    assert!(
        source.contains(
            "row-count: 2, frame: \"ends\", grid: \"rows\", stripes: \"even\", width: 50%)"
        ),
        "{source}"
    );
    assert!(source.contains("#adoc-table((auto,), none,"), "{source}");
    assert!(source.contains(", width: auto)"), "{source}");
    // A collapsible block is captioned `▼ Title`, unnumbered, and not framed as an example.
    assert!(
        source.contains("[#\"\u{25bc} \"#\"Shown on request\"]"),
        "{source}"
    );
    assert!(source.contains("#adoc-collapsible("), "{source}");
    assert!(!source.contains("Example 1."), "{source}");
    let text = compiled_text(&typst);
    for shown in [
        "c.three",
        "2.two",
        "1.one",
        "AB",
        "Narrow",
        "\u{25bc}ShownonrequestInside.",
    ] {
        assert!(text.contains(shown), "no {shown} in {text}");
    }
}

#[test]
fn emits_cell_vertical_alignment() {
    let typst =
        typst_of("= Cells\n\n[cols=\".^1,1\"]\n|===\n|Middle |Top .>|Bottom\n|Next\n|===\n");
    let source = &typst.source;
    for align in [
        "align: left + horizon)[#\"Middle\"",
        "align: left + top)[#\"Top\"",
        "align: left + bottom)[#\"Bottom\"",
    ] {
        assert!(source.contains(align), "no {align} in\n{source}");
    }
    assert!(compiled_text(&typst).contains("MiddleTop"));
}

#[test]
fn image_alignment_and_pdf_width_are_kept() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(
        directory.path().join("dot.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#,
    )
    .unwrap();
    std::fs::write(
        &page,
        "= Images\n\nimage::dot.svg[Dot,align=center,pdfwidth=40%,width=10]\n\nimage::dot.svg[Dot,float=right,link=https://example.org]\n",
    )
    .unwrap();
    let typst = to_typst(
        &common::article(common::view_of(&page)),
        &common::default_theme(),
        &PdfOptions::default(),
    );
    let source = &typst.source;
    assert!(
        source.contains("width: 40%), align-to: \"center\")"),
        "{source}"
    );
    assert!(
        source.contains("#adoc-image(link(\"https://example.org\", image("),
        "{source}"
    );
    assert!(source.contains(")), align-to: \"right\")"), "{source}");
}
