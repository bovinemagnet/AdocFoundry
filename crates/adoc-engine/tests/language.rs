//! Language features through the engine.

use std::{fs, path::Path, time::Duration};

use adoc_core::{DiagnosticCode, SourceRange};
use adoc_engine::{commands::ids, CommandArgs, CommandOutcome, Engine, EngineEvent, Settings};

fn open_workspace(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine
}

#[test]
fn edit_reparses_and_updates_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSee xref:missing.adoc[].\n").unwrap();
    fs::write(directory.path().join("present.adoc"), "= Present\n").unwrap();
    let mut engine = open_workspace(directory.path());
    let id = engine.open(&page).unwrap();
    let codes = |engine: &Engine| -> Vec<DiagnosticCode> {
        engine
            .document(id)
            .unwrap()
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    };
    assert_eq!(codes(&engine), [DiagnosticCode::UnresolvedXrefFile]);

    let events = engine.subscribe();
    let text = engine.document(id).unwrap().text.to_string();
    let start = text.find("missing").unwrap();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(start, start + "missing".len()),
                text: "present".to_owned(),
            },
        )
        .unwrap();
    assert!(codes(&engine).is_empty(), "the edit resolves the xref");
    assert_eq!(
        engine.document(id).unwrap().parsed.references[0].target,
        "present.adoc"
    );
    assert!(events.try_iter().any(|event| event
        == EngineEvent::DiagnosticsUpdated {
            path: engine.document(id).unwrap().path.clone()
        }));

    // Adding a section updates the parsed outline.
    let end = engine.document(id).unwrap().text.len();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end),
                text: "\n== Later\n".to_owned(),
            },
        )
        .unwrap();
    let outline = engine.outline(id);
    assert_eq!(outline[0].name, "Page");
    assert_eq!(outline[0].children[0].name, "Later");
}

#[test]
fn navigate_definition_returns_the_target_location() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/xrefs");
    let mut engine = open_workspace(&root);
    let id = engine.open(&root.join("index.adoc")).unwrap();
    let offset = engine
        .document(id)
        .unwrap()
        .parsed
        .text
        .find("xref:")
        .unwrap()
        + 6;
    let CommandOutcome::Location(location) = engine
        .invoke(ids::NAVIGATE_DEFINITION, CommandArgs::Offset { id, offset })
        .unwrap()
    else {
        panic!("expected a location");
    };
    assert!(location.path.ends_with("xrefs/other.adoc"));
    assert!(engine
        .invoke(
            ids::NAVIGATE_DEFINITION,
            CommandArgs::Offset { id, offset: 0 }
        )
        .is_err());
    assert!(engine.hover(id, offset).is_some());
}

#[test]
fn workspace_diagnostics_include_unopened_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/antora-invalid-descriptor");
    let engine = open_workspace(&root);
    let all = engine.workspace_diagnostics();
    assert!(all.iter().any(|(_, found)| found
        .iter()
        .any(|diagnostic| diagnostic.code == DiagnosticCode::AntoraInvalidDescriptor)));
}

#[test]
fn nav_trees_read_unsaved_buffers() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-multi-component");
    let mut engine = open_workspace(&root);
    let nav = root.join("alpha/modules/ROOT/nav.adoc");
    let id = engine.open(&nav).unwrap();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, ".Alpha".len()),
                text: ".Renamed".to_owned(),
            },
        )
        .unwrap();
    let trees = engine.nav_trees("alpha", Some("1.0")).unwrap();
    assert_eq!(trees[0].items[0].title, "Renamed");
}

#[test]
fn typing_defers_reindexing_until_typing_pauses() {
    use std::sync::Arc;

    use adoc_engine::{ManualClock, INDEX_SETTLE};

    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSee xref:missing.adoc[].\n").unwrap();
    let clock = ManualClock::new();
    let mut engine =
        Engine::with_clock(Settings::load(None, None).unwrap(), Arc::new(clock.clone()));
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&page).unwrap();
    let indexed = |engine: &Engine| {
        engine
            .workspace()
            .index
            .file(&page)
            .unwrap()
            .document
            .text
            .clone()
    };

    // A typed character re-parses at once but leaves the index until typing pauses.
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .unwrap();
    assert!(engine
        .document(id)
        .unwrap()
        .parsed
        .text
        .starts_with("x= Page"));
    assert_eq!(indexed(&engine), "= Page\n\nSee xref:missing.adoc[].\n");
    assert!(engine.has_pending());

    let events = engine.subscribe();
    engine.pump();
    assert!(
        indexed(&engine).starts_with("= Page"),
        "not before the pause"
    );
    clock.advance(INDEX_SETTLE);
    engine.pump();
    assert!(indexed(&engine).starts_with("x= Page"));
    assert!(events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::DiagnosticsUpdated { .. })));

    // Any other command brings the index up to date first.
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "y".to_owned(),
            },
        )
        .unwrap();
    engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        .unwrap();
    assert!(indexed(&engine).starts_with("yx= Page"));
}

#[test]
fn a_client_buffer_opens_without_a_file() {
    let mut engine = Engine::new(Settings::default());
    let path = Path::new("/nowhere/page.adoc");
    let id = engine.open_text(path, "= Page\n\nSee <<missing>>.\n");
    let document = engine.document(id).unwrap();
    assert_eq!(document.text_string(), "= Page\n\nSee <<missing>>.\n");
    assert!(!document.diagnostics.is_empty(), "diagnosed like any page");

    // Opening it again takes the client's text as an edit.
    assert_eq!(engine.open_text(path, "= Page\n"), id);
    assert_eq!(engine.document(id).unwrap().text_string(), "= Page\n");
    assert!(engine.undo(id), "the new text is an ordinary edit");
}

#[test]
fn references_and_anchor_renames_reach_every_page() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/references");
    let mut engine = open_workspace(&root);
    let index = root.join("index.adoc");
    let id = engine.open(&index).unwrap();
    let text = fs::read_to_string(&index).unwrap();

    let anchor = text.find("[[setup]]").unwrap() + 3;
    let found = engine.references(id, anchor);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(engine.references(id, 2).len(), 2, "the page's own xrefs");

    let edits = engine.anchor_rename_edits(id, anchor, "install").unwrap();
    let files: Vec<_> = edits
        .iter()
        .map(|file| (file.path.file_name().unwrap().to_owned(), file.edits.len()))
        .collect();
    assert_eq!(files, [("index.adoc".into(), 2), ("other.adoc".into(), 1)]);
    assert!(engine.anchor_rename_edits(id, 2, "install").is_none());
}
