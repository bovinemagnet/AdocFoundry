//! Tables and images: rows, columns, new tables and image macros.

use std::path::Path;

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_model::{apply_edits, invert, Model, NewBlock, Step, StepError};

fn build(text: &str) -> Model {
    let path = Path::new("page.adoc");
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    Model::build(path, text, &dom)
}

fn offset(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"))
}

fn apply(text: &str, step: Step) -> (String, adoc_core::SourceRange) {
    let result = build(text)
        .apply(&step)
        .unwrap_or_else(|error| panic!("{step:?}: {error}"));
    let after = apply_edits(text, &result.edits);
    assert_eq!(apply_edits(&after, &invert(text, &result.edits)), text);
    (after, result.selection)
}

/// Each row's cell texts, as the rebuilt model reads them.
fn cells(text: &str) -> Vec<Vec<String>> {
    let model = build(text);
    let mut rows = Vec::new();
    for (_, node) in model.walk() {
        if matches!(node.kind, adoc_model::NodeKind::Row { .. }) {
            rows.push(
                node.children
                    .iter()
                    .map(|cell| {
                        cell.text
                            .as_ref()
                            .map_or(String::new(), |t| t.display.clone())
                    })
                    .collect(),
            );
        }
    }
    rows
}

const TABLE: &str = "[cols=\"1,2\"]\n|===\n|Name |Value\n\n|a |1\n|b |2\n|===\n";

#[test]
fn rows_are_inserted_and_deleted() {
    let (after, selection) = apply(
        TABLE,
        Step::InsertTableRow {
            at: offset(TABLE, "a |"),
        },
    );
    assert_eq!(
        after,
        "[cols=\"1,2\"]\n|===\n|Name |Value\n\n|a |1\n| |\n|b |2\n|===\n"
    );
    assert_eq!(
        cells(&after),
        [["Name", "Value"], ["a", "1"], ["", ""], ["b", "2"]]
    );
    assert_eq!(selection.start, offset(&after, "| |\n") + 1);

    // Below the header, the new row goes before the first body row.
    let (after, _) = apply(
        TABLE,
        Step::InsertTableRow {
            at: offset(TABLE, "Name"),
        },
    );
    assert_eq!(
        after,
        "[cols=\"1,2\"]\n|===\n|Name |Value\n\n| |\n|a |1\n|b |2\n|===\n"
    );
    assert_eq!(
        cells(&after)[0],
        ["Name", "Value"],
        "the header stays a header"
    );

    let (after, _) = apply(
        TABLE,
        Step::DeleteTableRow {
            at: offset(TABLE, "b |"),
        },
    );
    assert_eq!(after, "[cols=\"1,2\"]\n|===\n|Name |Value\n\n|a |1\n|===\n");
    assert!(build(TABLE)
        .apply(&Step::DeleteTableRow {
            at: offset(TABLE, "Name")
        })
        .is_err());
}

#[test]
fn columns_are_inserted_and_deleted_with_their_widths() {
    let (after, _) = apply(
        TABLE,
        Step::InsertTableColumn {
            at: offset(TABLE, "a |"),
        },
    );
    assert_eq!(
        after,
        "[cols=\"1,1,2\"]\n|===\n|Name | |Value\n\n|a | |1\n|b | |2\n|===\n"
    );
    assert_eq!(
        cells(&after),
        [["Name", "", "Value"], ["a", "", "1"], ["b", "", "2"]]
    );

    let (after, _) = apply(
        TABLE,
        Step::DeleteTableColumn {
            at: offset(TABLE, "Value"),
        },
    );
    assert_eq!(after, "[cols=\"1\"]\n|===\n|Name\n\n|a\n|b\n|===\n");
    let (after, _) = apply(
        TABLE,
        Step::DeleteTableColumn {
            at: offset(TABLE, "Name"),
        },
    );
    // One width is one column whatever it was: `cols="2"` would mean two columns.
    assert_eq!(after, "[cols=\"1\"]\n|===\n|Value\n\n|1\n|2\n|===\n");
    assert_eq!(cells(&after), [["Value"], ["1"], ["2"]]);
}

#[test]
fn delete_to_one_column_writes_one() {
    let text = "[cols=\"2,3\"]\n|===\n|A |B\n\n|1 |2\n|===\n";
    let (after, _) = apply(
        text,
        Step::DeleteTableColumn {
            at: offset(text, "A"),
        },
    );
    assert_eq!(after, "[cols=\"1\"]\n|===\n|B\n\n|2\n|===\n");
    assert_eq!(cells(&after), [["B"], ["2"]]);
}

#[test]
fn irregular_tables_keep_their_structure() {
    let spans = "|===\n|A |B\n\n2+|Both\n|===\n";
    assert!(matches!(
        build(spans).apply(&Step::InsertTableColumn {
            at: offset(spans, "A")
        }),
        Err(StepError::Unsupported(_))
    ));
}

#[test]
fn tables_and_images_are_inserted_after_the_caret_block() {
    let text = "Intro.\n";
    let (after, selection) = apply(
        text,
        Step::InsertBlock {
            at: 2,
            block: NewBlock::Table {
                columns: 2,
                rows: 1,
            },
        },
    );
    assert_eq!(after, "Intro.\n\n|===\n|Column 1 |Column 2\n\n| |\n|===\n");
    assert_eq!(&after[selection.start..selection.end], "Column 1");
    assert_eq!(cells(&after), [["Column 1", "Column 2"], ["", ""]]);

    let (after, _) = apply(
        text,
        Step::InsertBlock {
            at: 2,
            block: NewBlock::Image {
                target: "diagram.png".to_owned(),
                alt: "A diagram".to_owned(),
            },
        },
    );
    assert_eq!(after, "Intro.\n\nimage::diagram.png[A diagram]\n");
}

#[test]
fn image_edits_change_only_target_and_alt() {
    let text = "image::old.png[Old text,200,role=thumb]\n";
    let (after, _) = apply(
        text,
        Step::SetImage {
            at: 0,
            target: "new.png".to_owned(),
            alt: "New text".to_owned(),
        },
    );
    assert_eq!(after, "image::new.png[New text,200,role=thumb]\n");

    let named = "image::ROOT:x.svg[width=100,alt=\"Old\"]\n";
    let (after, _) = apply(
        named,
        Step::SetImage {
            at: 0,
            target: "ROOT:x.svg".to_owned(),
            alt: "New".to_owned(),
        },
    );
    assert_eq!(after, "image::ROOT:x.svg[width=100,alt=\"New\"]\n");

    let bare = "image::x.png[]\n";
    let (after, _) = apply(
        bare,
        Step::SetImage {
            at: 0,
            target: "x.png".to_owned(),
            alt: "An x".to_owned(),
        },
    );
    assert_eq!(after, "image::x.png[An x]\n");
}
