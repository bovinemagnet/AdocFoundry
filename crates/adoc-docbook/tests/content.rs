//! DocBook 4/9 (#652): AsciiDoc's blocks and inlines as DocBook elements, tables as CALS, and
//! what has none kept as text with a warning (PRD §5.2, §11 tests 9 to 11). Every document is
//! checked against the DocBook 5.2.1 RELAX NG schema.

mod common;

use adoc_docbook::{testing, write_docbook, DocBookDocument, DocBookOptions, Page, NAMESPACE};
use adoc_render::{ViewBlock, ViewKind};
use common::{article, children, elements, text_of};

fn written(document: &DocBookDocument) -> adoc_docbook::DocBookOutput {
    let output = write_docbook(document, &DocBookOptions::default()).expect("written");
    testing::assert_valid(&output.xml);
    output
}

fn names(document: &roxmltree::Document<'_>, name: &str) -> usize {
    elements(document, name).count()
}

const RICH: &str = r#"= Content
:experimental:

== Lists

* one
** one a
*** one a i
* two

[loweralpha,start=3]
. third
. fourth
+
A paragraph joined to the item.

* [x] done
* [ ] to do

Term:: Its description.
Another term::
+
A block under it.

[qanda]
What is it?:: A product.

== Code

.A listing
[source,rust]
----
fn main() { // <1>
    println!("hi"); // <2>
}
----
<1> The entry point.
<2> Says hello.

....
literal   text
....

== Set apart

NOTE: A note.

[TIP]
.A titled tip
====
A tip.
====

IMPORTANT: Important.

CAUTION: Careful.

WARNING: Danger.

[quote, Ada Lovelace, Notes]
____
The engine weaves patterns.
____

[verse, A Poet]
____
Line one
  line two
____

.A sidebar
****
Aside.
****

.An example
====
Shown.
====

====
Untitled.
====

.Grouped
--
Open content.
--

[discrete]
=== A discrete heading

.A titled paragraph
Text under a title.

== Inline

*strong* _emphasis_ `code` #marked# ^super^ ~sub~ [.underline]#underlined#
https://example.com[a link] and https://example.org alone
kbd:[Ctrl+T] kbd:[F1] btn:[Save] menu:File[Save As] menu:Edit[]
((indexed)) (((hidden,term))) +
after a break.

A note.footnote:disclaimer[The first note.] Again.footnote:disclaimer[] And another.footnote:[A second note.]
"#;

#[test]
fn docbook_rich_content_uses_docbook_elements() {
    let output = written(&article(RICH));
    let document = testing::parse(&output.xml);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

    // Lists nest as lists.
    let itemized: Vec<_> = elements(&document, "itemizedlist").collect();
    assert_eq!(itemized.len(), 4, "three nested and a checklist");
    let ordered = elements(&document, "orderedlist").next().unwrap();
    assert_eq!(ordered.attribute("numeration"), Some("loweralpha"));
    assert_eq!(ordered.attribute("startingnumber"), Some("3"));
    let joined = ordered
        .descendants()
        .filter(|node| node.has_tag_name((NAMESPACE, "para")))
        .count();
    assert_eq!(joined, 3, "the joined paragraph is in its item");
    let checklist = itemized[3];
    assert_eq!(checklist.attribute("role"), Some("checklist"));
    let marks: Vec<_> = checklist
        .children()
        .filter_map(|item| item.attribute("override"))
        .collect();
    assert_eq!(marks, ["checkedbox", "checkbox"]);
    assert_eq!(names(&document, "varlistentry"), 2);
    assert_eq!(names(&document, "qandaentry"), 1);
    assert_eq!(
        text_of(elements(&document, "question").next().unwrap()),
        "What is it?"
    );

    // Code keeps its whitespace and language, its callouts are marks the list points at.
    let listing = elements(&document, "programlisting").next().unwrap();
    assert_eq!(listing.attribute("language"), Some("rust"));
    let code = text_of(listing);
    let lines: Vec<&str> = code.lines().map(str::trim_end).collect();
    // The markers go, with the comment that held them.
    assert_eq!(
        lines,
        ["fn main() {", "    println!(\"hi\");", "}"],
        "{code:?}"
    );
    let marks: Vec<String> = elements(&document, "co")
        .filter_map(|co| co.attribute(("http://www.w3.org/XML/1998/namespace", "id")))
        .map(str::to_owned)
        .collect();
    assert_eq!(marks.len(), 2);
    let callouts: Vec<&str> = elements(&document, "callout")
        .filter_map(|callout| callout.attribute("arearefs"))
        .collect();
    assert_eq!(
        callouts,
        marks.iter().map(String::as_str).collect::<Vec<_>>()
    );
    let titled = listing.parent().unwrap().parent().unwrap();
    assert!(
        titled.has_tag_name((NAMESPACE, "formalpara")),
        "a titled listing keeps its title"
    );
    assert_eq!(
        text_of(elements(&document, "screen").next().unwrap()),
        "literal   text"
    );

    // Admonitions, quotations, sidebars and examples.
    for name in ["note", "tip", "important", "caution", "warning"] {
        assert_eq!(names(&document, name), 1, "{name}");
    }
    let tip = elements(&document, "tip").next().unwrap();
    assert_eq!(children(tip), ["title", "para"]);
    let quotes: Vec<_> = elements(&document, "blockquote").collect();
    assert_eq!(quotes.len(), 2);
    assert_eq!(children(quotes[0]), ["attribution", "para"]);
    assert_eq!(
        text_of(elements(&document, "citetitle").next().unwrap()),
        "Notes"
    );
    assert_eq!(
        text_of(elements(&document, "literallayout").next().unwrap()),
        "Line one\n  line two"
    );
    assert_eq!(names(&document, "sidebar"), 1);
    assert_eq!(names(&document, "example"), 1);
    assert_eq!(names(&document, "informalexample"), 1);
    assert_eq!(names(&document, "bridgehead"), 1);
    assert_eq!(
        names(&document, "formalpara"),
        2,
        "the titled paragraph and listing"
    );

    // Inline semantics.
    let strong: Vec<_> = elements(&document, "emphasis")
        .filter_map(|emphasis| emphasis.attribute("role"))
        .collect();
    assert_eq!(strong, ["strong", "marked"]);
    assert_eq!(names(&document, "literal"), 1);
    assert_eq!(names(&document, "superscript"), 1);
    assert_eq!(names(&document, "subscript"), 1);
    assert!(
        elements(&document, "phrase").any(|phrase| phrase.attribute("role") == Some("underline"))
    );
    let links: Vec<(String, String)> = elements(&document, "link")
        .map(|link| {
            (
                link.attribute(("http://www.w3.org/1999/xlink", "href"))
                    .unwrap()
                    .to_owned(),
                text_of(link),
            )
        })
        .collect();
    assert_eq!(
        links,
        [
            ("https://example.com".to_owned(), "a link".to_owned()),
            (
                "https://example.org".to_owned(),
                "https://example.org".to_owned()
            ),
        ]
    );
    let combo = elements(&document, "keycombo").next().unwrap();
    assert_eq!(children(combo), ["keycap", "keycap"]);
    assert_eq!(names(&document, "keycap"), 3, "F1 is a key on its own");
    assert_eq!(
        text_of(elements(&document, "guibutton").next().unwrap()),
        "Save"
    );
    let menu = elements(&document, "menuchoice").next().unwrap();
    assert_eq!(children(menu), ["guimenu", "guimenuitem"]);
    assert_eq!(names(&document, "guimenu"), 2, "a menu on its own too");
    let terms: Vec<String> = elements(&document, "indexterm")
        .map(|term| text_of(term))
        .collect();
    assert_eq!(terms, ["indexed", "hiddenterm"]);
    assert!(output.xml.contains("<?asciidoc-br?>\nafter a break."));

    // A footnote is written where first referenced, and referenced again after.
    let notes: Vec<_> = elements(&document, "footnote").collect();
    assert_eq!(notes.len(), 2);
    let first = notes[0]
        .attribute(("http://www.w3.org/XML/1998/namespace", "id"))
        .unwrap();
    let again: Vec<_> = elements(&document, "footnoteref")
        .filter_map(|reference| reference.attribute("linkend"))
        .collect();
    assert_eq!(again, [first]);
    assert_eq!(text_of(notes[0]), "The first note.");
}

const TABLES: &str = r#"= Tables

.Prices
[cols="1,2,3",options="header,footer",frame=ends,grid=rows]
|===
|Item |Price |Notes

|Apple ^|1 >|Fresh
2+|Spans two |Third
.2+|Spans down |B1 |C1
|B2 |C2
a|
* A list in a cell
* Two
|plain
a|
[cols="1,1"]
!===
!nested !table
!===

|Total |3 |
|===

[%header]
|===
|Only |A header
|===

[cols="1,1"]
|===
|one 3+|too wide
|===
"#;

#[test]
fn docbook_tables_are_cals() {
    let output = written(&article(TABLES));
    let document = testing::parse(&output.xml);
    let table = elements(&document, "table").next().unwrap();
    assert_eq!(table.attribute("frame"), Some("topbot"));
    assert_eq!(table.attribute("rowsep"), Some("1"));
    assert_eq!(table.attribute("colsep"), Some("0"));
    assert_eq!(children(table), ["title", "tgroup"]);
    let group = elements(&document, "tgroup").next().unwrap();
    assert_eq!(group.attribute("cols"), Some("3"));
    assert_eq!(
        children(group),
        ["colspec", "colspec", "colspec", "thead", "tfoot", "tbody"]
    );
    let widths: Vec<&str> = group
        .children()
        .filter_map(|spec| spec.attribute("colwidth"))
        .collect();
    assert_eq!(widths, ["1*", "2*", "3*"]);

    let rows: Vec<_> = group
        .children()
        .find(|child| child.has_tag_name((NAMESPACE, "tbody")))
        .unwrap()
        .children()
        .filter(roxmltree::Node::is_element)
        .collect();
    assert_eq!(rows.len(), 5);
    fn entries<'a, 'input>(row: roxmltree::Node<'a, 'input>) -> Vec<roxmltree::Node<'a, 'input>> {
        row.children().filter(roxmltree::Node::is_element).collect()
    }
    let first = entries(rows[0]);
    assert_eq!(first[1].attribute("align"), Some("center"));
    assert_eq!(first[2].attribute("align"), Some("right"));
    let span = entries(rows[1])[0];
    assert_eq!(span.attribute("namest"), Some("col_1"));
    assert_eq!(span.attribute("nameend"), Some("col_2"));
    let down = entries(rows[2])[0];
    assert_eq!(down.attribute("morerows"), Some("1"));
    assert_eq!(
        entries(rows[3]).len(),
        2,
        "the spanned row has its other cells"
    );
    let blocks = entries(rows[4]);
    assert_eq!(children(blocks[0]), ["itemizedlist"]);
    assert!(
        blocks[2]
            .descendants()
            .any(|node| node.has_tag_name((NAMESPACE, "informaltable"))),
        "a table in a cell stays a table"
    );
    let footer = group
        .children()
        .find(|child| child.has_tag_name((NAMESPACE, "tfoot")))
        .unwrap();
    assert_eq!(
        text_of(footer).split_whitespace().collect::<String>(),
        "Total3"
    );

    // A table of a header row alone has it as its body. A row whose span does not fit is
    // dropped by the parser, so that table has no rows and is left out, with a warning.
    let informal: Vec<_> = elements(&document, "informaltable").collect();
    assert_eq!(
        informal.len(),
        2,
        "the nested table and the header-only one"
    );
    assert!(output
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("no rows")));
}

#[test]
fn docbook_unsupported_nodes_warn() {
    // A passthrough is text, never markup, and said so.
    let output = written(&article(
        "= Pass\n\n++++\n<b>raw</b> &amp;\n++++\n\nInline pass:[<i>x</i>] here.\n",
    ));
    let document = testing::parse(&output.xml);
    assert!(!output.xml.contains("<b>"));
    assert!(!output.xml.contains("<i>"));
    assert!(text_of(document.root_element()).contains("<b>raw</b> &amp;"));
    let warnings: Vec<&str> = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    // The view gives an inline passthrough as text, so it is escaped as text is, and only the
    // block is reported.
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("never as markup"));
    assert!(text_of(document.root_element()).contains("Inline <i>x</i> here"));
    assert!(output
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.source.is_some()));

    // A block with no DocBook element keeps its text, with a warning at its source.
    let mut view = common::view(
        "/docs/modules/ROOT/pages/guide.adoc",
        "= Unknown\n\nBefore.\n",
    );
    let span = view.blocks[0].span.clone();
    view.blocks.push(ViewBlock {
        span: span.clone(),
        id: None,
        roles: Vec::new(),
        style: None,
        title: Vec::new(),
        caption: None,
        attributes: Default::default(),
        options: Default::default(),
        kind: ViewKind::Unsupported {
            name: "gizmo".to_owned(),
            source: "gizmo::thing[]\nsecond line".to_owned(),
        },
    });
    let output = written(&DocBookDocument::Article(Page {
        key: "p0".to_owned(),
        view,
        depth: 0,
    }));
    let document = testing::parse(&output.xml);
    assert_eq!(
        text_of(elements(&document, "literallayout").next().unwrap()),
        "gizmo::thing[]\nsecond line"
    );
    assert_eq!(output.diagnostics.len(), 1);
    assert!(output.diagnostics[0].message.contains("the gizmo block"));
    assert_eq!(output.diagnostics[0].source.as_ref(), Some(&span));
}
