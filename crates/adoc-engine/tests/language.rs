//! Language features through the engine.

use std::{fs, path::Path, time::Duration};

use adoc_core::{DiagnosticCode, SourceRange};
use adoc_engine::{commands::ids, CommandArgs, CommandOutcome, Engine, EngineEvent, Settings};

fn open_workspace(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
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
        engine.document(id).unwrap().parsed().references[0].target,
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
        .parsed()
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
        .parsed()
        .text
        .starts_with("x= Page"));
    assert_eq!(&*indexed(&engine), "= Page\n\nSee xref:missing.adoc[].\n");
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

#[test]
fn targets_and_resource_ids_resolve_without_an_open_page() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-single-component");
    let engine = open_workspace(&root);
    let index = adoc_index::normalize_path(&root.join("modules/ROOT/pages/index.adoc"));
    let authentication =
        adoc_index::normalize_path(&root.join("modules/security/pages/authentication.adoc"));

    let Some(adoc_engine::DefinitionTarget::Location(location)) =
        engine.resolve_target(&index, "xref:security:authentication.adoc[]")
    else {
        panic!("the xref resolves");
    };
    assert_eq!(location.path, authentication);
    assert!(engine
        .resolve_target(&index, "security:authentication.adoc")
        .is_some());
    assert!(engine
        .resolve_target(&index, "include::partial$welcome.adoc[]")
        .is_some());
    assert!(engine.resolve_target(&index, "missing.adoc").is_none());

    assert_eq!(
        engine.resource_id(&authentication).as_deref(),
        Some("latest@demo:security:page$authentication.adoc")
    );
    assert_eq!(
        engine
            .resolve_resource_id("latest@demo:security:page$authentication.adoc", None)
            .unwrap(),
        authentication
    );
    assert_eq!(
        engine
            .resolve_resource_id("security:authentication.adoc", Some(&index))
            .unwrap(),
        authentication
    );
    assert!(engine
        .resolve_resource_id("authentication.adoc", None)
        .is_err());
}

#[test]
fn problems_inside_antora_yml_are_listed_and_follow_its_changes() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/antora-descriptor-problems");
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    for file in [
        "antora.yml",
        "modules/ROOT/nav.adoc",
        "modules/ROOT/pages/index.adoc",
    ] {
        let target = root.join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(fixture.join(file), target).unwrap();
    }
    let descriptor = root.join("antora.yml");
    let mut engine = open_workspace(&root);

    let codes = |engine: &Engine| -> Vec<&'static str> {
        engine
            .workspace_diagnostics()
            .into_iter()
            .filter(|(path, _)| *path == descriptor)
            .flat_map(|(_, found)| found.into_iter().map(|diagnostic| diagnostic.code.as_str()))
            .collect()
    };
    assert_eq!(
        codes(&engine),
        [
            "adoc.antora.missing-key",
            "adoc.antora.unknown-key",
            "adoc.antora.missing-nav-file",
            "adoc.antora.invalid-attribute-value",
        ]
    );
    // The component still loads, with the attributes Antora can use.
    let page = engine
        .open(&root.join("modules/ROOT/pages/index.adoc"))
        .unwrap();
    assert!(engine.document(page).unwrap().diagnostics.is_empty());

    fs::write(
        &descriptor,
        "name: demo\nversion: ~\nnav: modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    engine
        .invoke(ids::WORKSPACE_REINDEX, CommandArgs::None)
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert!(codes(&engine).is_empty(), "{:?}", codes(&engine));
}

#[test]
fn a_descriptor_without_a_name_is_reported_once_inside_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/antora-invalid-descriptor");
    let engine = open_workspace(&root);
    let descriptor = adoc_index::normalize_path(&root.join("antora.yml"));
    let found: Vec<_> = engine
        .workspace_diagnostics()
        .into_iter()
        .filter(|(path, _)| *path == descriptor)
        .flat_map(|(_, found)| found)
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_str(), "adoc.antora.missing-key");
    assert!(found[0].message.contains("`name`"), "{found:?}");
    // The component's pages still say why Antora checks are off.
    assert!(engine
        .workspace_diagnostics()
        .iter()
        .any(|(path, found)| *path != descriptor
            && found
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::AntoraInvalidDescriptor)));
}

#[test]
fn a_dragged_page_from_another_component_inserts_an_xref_that_resolves() {
    let root = adoc_engine::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-multi-component"),
    );
    let mut engine = open_workspace(&root);
    let id = engine
        .open(&root.join("alpha/modules/ROOT/pages/index.adoc"))
        .unwrap();
    // The page's one diagnostic is for its link through an alias.
    let before = engine.document(id).unwrap().diagnostics.clone();
    let reference = root.join("beta/modules/api/pages/reference.adoc");

    let link = engine.xref_to(id, &reference, None).unwrap();
    assert_eq!(link.source(), "xref:beta:api:reference.adoc[API Reference]");

    let end = engine.document(id).unwrap().text.len();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end),
                text: link.source(),
            },
        )
        .unwrap();
    let document = engine.document(id).unwrap();
    assert_eq!(document.diagnostics, before, "the xref adds no diagnostic");
    let target = engine.definition(id, end + "xref:".len());
    assert!(
        matches!(&target, Some(adoc_engine::DefinitionTarget::Location(location)) if location.path == reference),
        "{target:?}"
    );
}

#[test]
fn a_dragged_outline_section_links_to_it_in_the_page() {
    let root = adoc_engine::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-multi-component"),
    );
    let mut engine = open_workspace(&root);
    let guide = root.join("alpha/modules/ROOT/pages/guide.adoc");
    let id = engine.open(&guide).unwrap();
    let setup = engine.outline(id)[0].children[0].selection_range;

    let link = engine.xref_to(id, &guide, Some(setup)).unwrap();
    assert_eq!(link.source(), "<<setup,Setup>>");
    // From another page, through the page.
    let index = engine
        .open(&root.join("alpha/modules/ROOT/pages/index.adoc"))
        .unwrap();
    assert_eq!(
        engine.xref_to(index, &guide, Some(setup)).unwrap().source(),
        "xref:guide.adoc#setup[Setup]"
    );
}

/// Two components: Alpha's guide declares `setup`, and pages in both components name it.
fn anchored_components() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("alpha/antora.yml", "name: alpha\nversion: '1.0'\n");
    write(
        "alpha/modules/ROOT/pages/guide.adoc",
        "= Guide\n\n[[setup]]\n== Setup\n\nSee <<setup>>.\n",
    );
    write(
        "alpha/modules/ROOT/pages/index.adoc",
        "= Alpha\n\nStart with xref:guide.adoc#setup[setting up].\n",
    );
    write("beta/antora.yml", "name: beta\nversion: ~\n");
    write(
        "beta/modules/ROOT/pages/index.adoc",
        "= Beta\n\nRead xref:alpha::guide.adoc#setup[Alpha's set-up] or xref:alpha::guide.adoc[the guide].\n",
    );
    directory
}

#[test]
fn find_references_lists_every_reference_to_an_anchor_across_components() {
    let directory = anchored_components();
    let root = adoc_engine::normalize_path(directory.path());
    let mut engine = open_workspace(&root);
    let guide = root.join("alpha/modules/ROOT/pages/guide.adoc");
    let id = engine.open(&guide).unwrap();
    let offset = engine
        .document(id)
        .unwrap()
        .text
        .to_string()
        .find("setup]]")
        .unwrap();

    let CommandOutcome::References(found) = engine
        .invoke(
            ids::NAVIGATE_FIND_REFERENCES,
            CommandArgs::Offset { id, offset },
        )
        .unwrap()
    else {
        panic!("the references");
    };
    let found: Vec<(String, usize, String)> = found
        .iter()
        .map(|found| {
            (
                found
                    .path
                    .strip_prefix(&root)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/"),
                found.line,
                found.line_text.clone(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "alpha/modules/ROOT/pages/guide.adoc".to_owned(),
                5,
                "See <<setup>>.".to_owned()
            ),
            (
                "alpha/modules/ROOT/pages/index.adoc".to_owned(),
                2,
                "Start with xref:guide.adoc#setup[setting up].".to_owned()
            ),
            (
                "beta/modules/ROOT/pages/index.adoc".to_owned(),
                2,
                "Read xref:alpha::guide.adoc#setup[Alpha's set-up] or xref:alpha::guide.adoc[the guide].".to_owned()
            ),
        ]
    );
}
