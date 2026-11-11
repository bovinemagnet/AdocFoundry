//! Inspect Attribute: where the value of the attribute at the caret comes from (issue #99).

use std::{path::Path, time::Duration};

use adoc_engine::{
    commands::ids, AttributeInspection, CommandArgs, CommandOutcome, Engine, InspectionRowKind,
    Settings,
};

fn fixture() -> std::path::PathBuf {
    adoc_engine::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/effective-attributes"),
    )
}

fn open_workspace(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine
}

fn inspect(engine: &mut Engine, path: &Path, needle: &str) -> Option<AttributeInspection> {
    let id = engine.open(path).unwrap();
    let offset = engine
        .document(id)
        .unwrap()
        .text
        .to_string()
        .find(needle)
        .unwrap()
        + 1;
    let CommandOutcome::Attribute(found) = engine
        .invoke(
            ids::NAVIGATE_INSPECT_ATTRIBUTE,
            CommandArgs::Offset { id, offset },
        )
        .unwrap()
    else {
        panic!("an attribute outcome");
    };
    found.map(|found| *found)
}

/// Each row as `source | value | status | place`.
fn rows(inspection: &AttributeInspection) -> Vec<String> {
    inspection
        .rows
        .iter()
        .map(|row| {
            format!(
                "{} | {} | {} | {}",
                row.source,
                row.value,
                row.status,
                row.place.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

#[test]
fn inspector_shows_component_and_page_header_and_which_wins() {
    let root = fixture();
    let mut engine = open_workspace(&root);
    let index = root.join("docs/modules/ROOT/pages/index.adoc");
    let inspection = inspect(&mut engine, &index, "{edition}").unwrap();

    assert_eq!(inspection.name, "edition");
    assert_eq!(inspection.summary, "edition is “Enterprise” here");
    assert_eq!(
        rows(&inspection),
        [
            "antora.yml | Standard | soft set, overridden by this page | docs/antora.yml:7",
            "This page | Enterprise | wins | docs/modules/ROOT/pages/index.adoc:2",
            "This page | Later | after this point | docs/modules/ROOT/pages/index.adoc:10",
        ]
    );
    // Each row leads to where it is written.
    let component = inspection.rows[0].location.as_ref().unwrap();
    assert_eq!(component.path, root.join("docs/antora.yml"));
    assert!(inspection.rows[1].wins());
    assert!(!inspection.rows[0].wins());
}

#[test]
fn a_hard_set_attribute_says_what_it_ignores() {
    let root = fixture();
    let mut engine = open_workspace(&root);
    let index = root.join("docs/modules/ROOT/pages/index.adoc");
    let inspection = inspect(&mut engine, &index, "{product}").unwrap();
    assert_eq!(
        rows(&inspection),
        [
            "Playbook | Playbook Product | soft set, overridden by antora.yml | antora-playbook.yml:10",
            "antora.yml | Guide | hard set, wins | docs/antora.yml:6",
            "This page | Page Product | ignored: antora.yml hard sets it | docs/modules/ROOT/pages/index.adoc:3",
        ]
    );
}

#[test]
fn a_partial_lists_its_value_in_each_page_that_includes_it() {
    let root = fixture();
    let mut engine = open_workspace(&root);
    let terms = root.join("docs/modules/ROOT/partials/terms.adoc");
    let inspection = inspect(&mut engine, &terms, "{edition}").unwrap();
    assert_eq!(inspection.summary, "edition is “Standard” here");
    let includers: Vec<String> = inspection
        .rows
        .iter()
        .filter(|row| row.kind == InspectionRowKind::Includer)
        .map(|row| format!("{} | {} | {}", row.source, row.value, row.status))
        .collect();
    assert_eq!(
        includers,
        [
            "docs/modules/ROOT/pages/index.adoc | Enterprise | from docs/modules/ROOT/pages/index.adoc:2",
            "docs/modules/ROOT/pages/plain.adoc | Standard | from docs/antora.yml:7",
        ]
    );
}

#[test]
fn nothing_to_inspect_away_from_an_attribute() {
    let root = fixture();
    let mut engine = open_workspace(&root);
    let index = root.join("docs/modules/ROOT/pages/index.adoc");
    assert_eq!(inspect(&mut engine, &index, "edition of"), None);
}
