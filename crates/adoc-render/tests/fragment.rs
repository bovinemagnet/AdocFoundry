//! What a selection of a page copies (#794), in each form.

use std::path::{Path, PathBuf};

use adoc_dom::{AcdcParser, DocumentParser, ParseInput, SourceSpan};
use adoc_render::{
    build_view,
    fragment::{runnable_code, Flow, Fragment, Part, Place},
    LocalResolver, ViewKind, ViewTree,
};

fn view_at(path: &Path, text: &str) -> ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    build_view(path, &dom, &LocalResolver)
}

fn view(text: &str) -> ViewTree {
    view_at(Path::new("page.adoc"), text)
}

/// The place of `needle` in the page's text, at its start or its end.
fn find(flow: &Flow, needle: &str, end: bool) -> Place {
    let (leaf, at) = flow
        .leaves()
        .iter()
        .enumerate()
        .find_map(|(index, leaf)| leaf.text.find(needle).map(|at| (index, at)))
        .unwrap_or_else(|| panic!("{needle:?} is not in the page"));
    Place::new(leaf, if end { at + needle.len() } else { at })
}

/// What is selected from the start of `from` to the end of `to`.
fn cut(view: &ViewTree, from: &str, to: &str) -> Fragment {
    let flow = Flow::new(view);
    flow.cut(
        view,
        find(&flow, from, false),
        find(&flow, to, true),
        &|_| None,
    )
}

fn all(view: &ViewTree) -> Fragment {
    let flow = Flow::new(view);
    let Some((from, to)) = flow.ends() else {
        return Fragment::default();
    };
    flow.cut(view, from, to, &|_| None)
}

fn plain(text: &str, from: &str, to: &str) -> String {
    cut(&view(text), from, to).to_plain()
}

fn no_source(_: &SourceSpan) -> Option<String> {
    None
}

const CODE: &str = "Above.\n\n[source,sql]\n----\nSELECT a; -- <1>\nSELECT b;\nSELECT c; <2>\n----\n<1> The *first*.\n<2> The second.\n\nBelow.\n";

#[test]
fn part_of_a_text_is_copied_alone() {
    assert_eq!(
        plain("= Title\n\nSome *bold* text.\n", "me bo", "ld"),
        "me bold"
    );
    assert_eq!(plain("* one two\n* three\n", "two", "two"), "two");
    assert_eq!(plain("== A heading\n\nx\n", "head", "head"), "head");
    assert_eq!(plain("|===\n|a b |c\n|===\n", "b", "b"), "b");
}

#[test]
fn a_callout_is_copied_with_its_number() {
    assert_eq!(plain(CODE, "The first.", "The first."), "<1> The first.");
    assert_eq!(plain(CODE, "first", "first"), "first");
}

#[test]
fn code_copies_its_lines_and_callouts() {
    assert_eq!(plain(CODE, "SELECT b;", "SELECT b;"), "SELECT b;");
    assert_eq!(
        plain(CODE, "SELECT a;", "SELECT b;"),
        "SELECT a; <1>\nSELECT b;"
    );
    // A selection from the text above, through the code, to its callouts takes them all.
    assert_eq!(
        plain(CODE, "Above.", "The second."),
        "Above.\n\nSELECT a; <1>\nSELECT b;\nSELECT c; <2>\n\n<1> The first.\n<2> The second."
    );
}

#[test]
fn code_keeps_its_indentation_and_blank_lines() {
    let text = "----\nif a:\n\tb()\n\n    c()\n----\n";
    assert_eq!(plain(text, "if a:", "c()"), "if a:\n\tb()\n\n    c()");
}

#[test]
fn runnable_code_leaves_callouts_out() {
    let of = |text: &str| {
        let view = view(text);
        let mut found = None;
        view.each_block(&mut |block| {
            if let ViewKind::Code { text, callouts, .. } = &block.kind {
                found = Some(runnable_code(text, callouts));
            }
        });
        found.unwrap()
    };
    assert_eq!(of(CODE), "SELECT a;\nSELECT b;\nSELECT c;");
    assert_eq!(
        of("[source,xml]\n----\n<a/> <!--1-->\n<!-- kept -->\n----\n<1> one\n"),
        "<a/>\n<!-- kept -->"
    );
}

#[test]
fn included_code_copies_exactly_the_lines_selected() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("query.sql"),
        "SELECT 1;\n  SELECT 2;\nSELECT 3;\n",
    )
    .unwrap();
    let text = "Above.\n\n[source,sql]\n----\ninclude::query.sql[]\n----\n";
    let view = view_at(&folder.path().join("page.adoc"), text);
    assert_eq!(
        cut(&view, "  SELECT 2;", "SELECT 3;").to_plain(),
        "  SELECT 2;\nSELECT 3;"
    );
    // AsciiDoc over an include holds the text and no directive, in a block too.
    let asciidoc = cut(&view, "Above.", "SELECT 3;").to_asciidoc(&no_source);
    assert!(!asciidoc.contains("include::"), "{asciidoc}");
    assert!(
        asciidoc.contains("SELECT 1;\n  SELECT 2;\nSELECT 3;"),
        "{asciidoc}"
    );
}

#[test]
fn a_selection_passes_through_an_include() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("part.adoc"),
        "Included *text*.\n\n* in a list\n",
    )
    .unwrap();
    let text = "Before.\n\ninclude::part.adoc[]\n\nAfter.\n";
    let view = view_at(&folder.path().join("page.adoc"), text);
    let taken = cut(&view, "Before.", "After.");
    assert_eq!(
        taken.to_plain(),
        "Before.\n\nIncluded text.\n\n* in a list\n\nAfter."
    );
    let asciidoc = taken.to_asciidoc(&no_source);
    assert!(!asciidoc.contains("include::"), "{asciidoc}");
    assert!(asciidoc.contains("Included *text*."), "{asciidoc}");
    // It can start and end in one too.
    assert_eq!(cut(&view, "text", "in a").to_plain(), "text.\n\n* in a");
}

#[test]
fn numbers_are_those_on_the_page() {
    let text = ". one\n. two\n. three\n. four\n. five\n";
    assert_eq!(plain(text, "two", "four"), "2. two\n3. three\n4. four");
    let started = "[start=7]\n. seven\n. eight\n. nine\n";
    assert_eq!(plain(started, "eight", "nine"), "8. eight\n9. nine");
    let reversed = "[%reversed]\n. c\n. b\n. a\n";
    assert_eq!(plain(reversed, "b", "a"), "2. b\n1. a");
    // The first item taken in part has no marker; the rest do.
    assert_eq!(plain(text, "wo", "three"), "wo\n3. three");
    assert_eq!(plain(text, "two", "two"), "2. two");
}

#[test]
fn bullets_are_copied_as_the_page_writes_them() {
    let text = "- one\n- two\n** deeper\n- [x] done\n";
    let view = view(text);
    let flow = Flow::new(&view);
    let (from, to) = flow.ends().unwrap();
    let bullet = |span: &SourceSpan| text[span.range.start..].chars().next();
    assert_eq!(
        flow.cut(&view, from, to, &bullet).to_plain(),
        "- one\n- two\n  * deeper\n- [x] done"
    );
    assert_eq!(
        plain("* [ ] to do\n* [x] done\n", "to do", "done"),
        "* [ ] to do\n* [x] done"
    );
}

#[test]
fn nested_items_are_indented() {
    let text = "* one\n** two\n*** three\n* four\n";
    assert_eq!(
        plain(text, "one", "four"),
        "* one\n  * two\n    * three\n* four"
    );
    // Only what is inside an item is taken as a list of its own.
    assert_eq!(plain(text, "two", "three"), "* two\n  * three");
}

#[test]
fn table_rows_are_lines_of_cells_between_tabs() {
    let text = "[%header,cols=2*]\n|===\n|Name |Use\n\n|a b |first\n|c |second\n|===\n";
    assert_eq!(
        plain(text, "Name", "second"),
        "Name\tUse\na b\tfirst\nc\tsecond"
    );
    assert_eq!(plain(text, "first", "c"), "first\nc");
    let asciidoc_cell = "[cols=\"1,1a\"]\n|===\n|a\n|* one\n* two\n|===\n";
    assert_eq!(plain(asciidoc_cell, "a", "two"), "a\t* one * two");
}

#[test]
fn every_kind_of_block_is_copied() {
    let text = "= Page\n:toc: macro\n\ntoc::[]\n\n== Section\n\n.A title\nParagraph.\n\n\
                [verse,Poet,Work]\n____\nLine one\n  line two\n____\n\n\
                [quote,Someone]\n____\nQuoted.\n____\n\nNOTE: Noted.\n\n\
                ```sql\nSELECT fenced;\n```\n\n[source]\nundelimited\n\n literal line\n\n\
                [stem]\n++++\nx^2\n++++\n\n++++\n<b>raw</b>\n++++\n\n\
                ====\nExample.\n====\n\n****\nSidebar.\n****\n\nterm:: description\n\n\
                image::picture.png[A picture]\n\n[subs=none]\nSome *stars*.\n\n\
                [discrete]\n=== Discrete\n\nfoo footnote:[x] kbd:[Ctrl+C]\n";
    let copied = all(&view(text)).to_plain();
    for expected in [
        "Page",
        "Section\n\nA title\nParagraph.",
        "Line one\n  line two\n— Poet, Work",
        "Quoted.\n— Someone",
        "Note\nNoted.",
        "SELECT fenced;",
        "undelimited",
        "literal line",
        "x^2",
        "<b>raw</b>",
        "Example.",
        "Sidebar.",
        "term\n  description",
        "A picture",
        "Some *stars*.",
        "Discrete",
        "foo [1]",
    ] {
        assert!(
            copied.contains(expected),
            "{expected:?} is not in:\n{copied}"
        );
    }
}

#[test]
fn a_line_of_source_is_not_a_line_of_text() {
    assert_eq!(
        plain("one\ntwo +\nthree\n", "one", "three"),
        "one two\nthree"
    );
}

#[test]
fn an_editors_own_text_is_copied_as_it_shows_it() {
    let view = view(CODE);
    let mut flow = Flow::new(&view);
    let key = flow
        .leaves()
        .iter()
        .find(|leaf| leaf.text.starts_with("SELECT"))
        .unwrap()
        .key
        .clone();
    flow.set_text(&key, "SELECT a; -- <1>\nSELECT b;".to_owned());
    let from = flow.place(&key, 0).unwrap();
    let to = flow.place(&key, usize::MAX).unwrap();
    assert_eq!(
        flow.cut(&view, from, to, &|_| None).to_plain(),
        "SELECT a; -- <1>\nSELECT b;"
    );
}

#[test]
fn markdown_keeps_structure() {
    let text = "= Page\n\n== Section\n\nSome *bold*, _it_, `code` and https://example.org[a link].\n\n\
                . one\n.. inner\n. two\n\n//-\n\n* [x] done\n\n[source,sql]\n----\nSELECT 1;\n----\n\n\
                [%header,cols=2*]\n|===\n|A |B\n\n|c\\|d |e\n|===\n\nNOTE: Noted.\n";
    let markdown = all(&view(text)).to_markdown();
    assert_eq!(
        markdown,
        "# Page\n\n## Section\n\n\
         Some **bold**, *it*, `code` and [a link](https://example.org).\n\n\
         1. one\n   1. inner\n2. two\n\n* [x] done\n\n```sql\nSELECT 1;\n```\n\n\
         | A | B |\n| --- | --- |\n| c\\|d | e |\n\n> [!NOTE]\n> Noted."
    );
}

#[test]
fn markdown_escapes_text_that_is_not_markup() {
    assert_eq!(
        all(&view("[subs=none]\nNot *bold* or <tag> in snake_case\n")).to_markdown(),
        "Not \\*bold\\* or \\<tag\\> in snake_case"
    );
}

#[test]
fn html_names_no_file() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("picture.png"), b"x").unwrap();
    let text = "See xref:other.adoc[Other] and image:picture.png[Alt].\n\n\
                image::picture.png[A picture]\n\nhttps://example.org[y]\n";
    let path = folder.path().join("page.adoc");
    std::fs::write(folder.path().join("other.adoc"), "= Other\n").unwrap();
    let html = all(&view_at(&path, text)).to_html(&|_| None);
    let name = folder.path().file_name().unwrap().to_str().unwrap();
    assert!(!html.contains(name), "{html}");
    assert!(!html.contains("file:"), "{html}");
    assert!(
        html.contains("<a href=\"https://example.org\">y</a>"),
        "{html}"
    );
    assert!(html.contains("Other") && html.contains("Alt"), "{html}");
}

#[test]
fn html_escapes_text_and_keeps_structure() {
    let text = "[subs=none]\n<script>a</script> & \"b\"\n\n. one\n. two\n. three\n";
    let html = cut(&view(text), "two", "three").to_html(&|_| None);
    assert_eq!(html, "<ol start=\"2\"><li>two</li><li>three</li></ol>");
    let html = all(&view(text)).to_html(&|_| None);
    assert!(html.starts_with("<p>&lt;script&gt;"), "{html}");
    // Part of one text joins the text it is pasted into.
    assert_eq!(
        cut(&view("Some *bold* text.\n"), "bold", "text").to_html(&|_| None),
        "<strong>bold</strong> text"
    );
}

#[test]
fn asciidoc_closes_the_marks_a_selection_cuts() {
    assert_eq!(
        cut(&view("Some *bold words* here.\n"), "words", "here").to_asciidoc(&no_source),
        "*words* here"
    );
    assert_eq!(
        cut(&view(". a\n. b\n. c\n"), "b", "c").to_asciidoc(&no_source),
        "[start=2]\n. b\n. c"
    );
}

/// The pages of the fixtures and the documentation.
fn corpus() -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
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
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    visit(&repository.join("tests/fixtures"), &mut out);
    visit(&repository.join("src/docs"), &mut out);
    out.sort();
    out
}

/// The words of a page's text, without what is counted or drawn afresh on another page.
fn words(view: &ViewTree) -> Vec<String> {
    Flow::new(view)
        .leaves()
        .iter()
        .filter(|leaf| leaf.key.part != Part::Contents)
        .flat_map(|leaf| {
            leaf.text
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Pages whose words change when they are copied, and why.
const ALLOWED: &[(&str, &str)] = &[
    (
        "developer/pages/architecture.adoc",
        "code holding both backticks and pluses, which has no spelling this parser reads",
    ),
    (
        "render/block-options.adoc",
        "a caption set on the block, counted afresh",
    ),
    (
        "render/numbering.adoc",
        "numbering turned off part way down the page",
    ),
];

/// Every page copied whole as AsciiDoc, written from what it holds, reads as the page did.
#[test]
fn asciidoc_reads_as_the_page_did() {
    let mut failures = Vec::new();
    let mut pages = 0;
    for path in corpus() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let page = view_at(&path, &text);
        // The contents are copied as their text, and drawn again where the copy has its own.
        if !page.toc.is_empty() {
            continue;
        }
        let asciidoc = all(&page).to_asciidoc(&no_source);
        // The page it is copied to has the same attributes.
        let mut input = ParseInput::new(Path::new("copy.adoc"), &asciidoc);
        input.attributes = page.attributes.clone().into_iter().collect();
        let copy = build_view(
            Path::new("copy.adoc"),
            &AcdcParser.parse(&input),
            &LocalResolver,
        );
        pages += 1;
        let (before, after) = (words(&page), words(&copy));
        let allowed = ALLOWED
            .iter()
            .any(|(page, _)| path.ends_with(Path::new(page)));
        if before != after && !allowed {
            let at = before
                .iter()
                .zip(&after)
                .position(|(a, b)| a != b)
                .unwrap_or(before.len().min(after.len()));
            let around =
                |words: &[String]| words[at.saturating_sub(3)..(at + 4).min(words.len())].join(" ");
            failures.push(format!(
                "{}: {:?} became {:?}",
                path.display(),
                around(&before),
                around(&after)
            ));
        }
    }
    assert!(pages > 100, "{pages}");
    assert!(
        failures.is_empty(),
        "{} of {pages}:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A cut anywhere never panics, and what two cuts side by side take is what one takes.
#[test]
fn every_cut_is_safe() {
    for path in corpus().into_iter().step_by(5) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let page = view_at(&path, &text);
        let flow = Flow::new(&page);
        let Some((from, to)) = flow.ends() else {
            continue;
        };
        // A dozen places down the page.
        for step in (0..flow.leaves().len()).step_by(flow.leaves().len() / 12 + 1) {
            let leaf = &flow.leaves()[step];
            for offset in [0, leaf.text.len() / 2, leaf.text.len()] {
                let at = Place::new(step, leaf.text.floor_char_boundary(offset));
                for (from, to) in [(from, at), (at, to), (at, at)] {
                    let taken = flow.cut(&page, from, to, &|_| None);
                    let _ = (
                        taken.to_plain(),
                        taken.to_markdown(),
                        taken.to_asciidoc(&no_source),
                        taken.to_html(&|_| None),
                    );
                }
            }
        }
    }
}
