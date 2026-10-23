//! Workspace search, replace with preview, and page rename.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, EditPreview, Engine, EngineEvent, SearchRequest,
    Settings,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn open_workspace(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(adoc_index::normalize_path(root))
        .unwrap()
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn workspace_search_results() {
    let root = fixture("antora-multi-component");
    let mut engine = open_workspace(&root);
    let events = engine.subscribe();
    engine
        .invoke(
            ids::WORKSPACE_SEARCH,
            CommandArgs::Search(SearchRequest {
                query: r"xref:[a-z0-9.@]*::".to_owned(),
                regex: true,
                ..SearchRequest::default()
            }),
        )
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let results = events
        .try_iter()
        .find_map(|event| match event {
            EngineEvent::SearchResults { results, done, .. } => {
                assert!(done);
                Some(results)
            }
            _ => None,
        })
        .expect("search results arrive as an event");
    let lines: Vec<String> = results
        .iter()
        .map(|found| {
            format!(
                "{}:{} {}..{} {}",
                relative(&found.path, &root),
                found.line + 1,
                found.range.start,
                found.range.end,
                found.line_text
            )
        })
        .collect();
    insta::assert_snapshot!(lines.join("\n"));
}

#[test]
fn search_honours_case_whole_word_and_scope() {
    let root = fixture("antora-multi-component");
    let mut engine = open_workspace(&root);
    let count = |engine: &mut Engine, request: SearchRequest| {
        let events = engine.subscribe();
        engine.search(request).unwrap();
        engine.wait_for_workers(Duration::from_secs(10));
        events
            .try_iter()
            .find_map(|event| match event {
                EngineEvent::SearchResults { results, .. } => Some(results.len()),
                _ => None,
            })
            .unwrap()
    };
    let beta = |case_sensitive, whole_word, scope: Option<PathBuf>| SearchRequest {
        query: "beta".to_owned(),
        case_sensitive,
        whole_word,
        scope,
        ..SearchRequest::default()
    };
    let insensitive = count(&mut engine, beta(false, false, None));
    let sensitive = count(&mut engine, beta(true, false, None));
    assert!(
        insensitive > sensitive,
        "`= Beta` only matches case-insensitively"
    );
    assert_eq!(
        count(&mut engine, beta(false, false, Some(root.join("beta")))),
        1,
        "scope limits the search to one component"
    );
    assert!(engine
        .search(SearchRequest {
            query: "(".to_owned(),
            regex: true,
            ..SearchRequest::default()
        })
        .is_err());
}

fn preview(outcome: CommandOutcome) -> EditPreview {
    match outcome {
        CommandOutcome::Preview(preview) => preview,
        other => panic!("expected a preview, got {other:?}"),
    }
}

#[test]
fn replace_applies_as_one_undo_group() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    copy_tree(&fixture("antora-multi-component"), &root);
    let mut engine = open_workspace(&root);

    let preview = preview(
        engine
            .invoke(
                ids::WORKSPACE_REPLACE,
                CommandArgs::Custom(serde_json::json!({
                    "request": { "query": "= (\\w+)", "regex": true },
                    "replacement": "= Title: $1",
                })),
            )
            .unwrap(),
    );
    let files = preview.edits.len();
    assert!(files >= 4, "every page title is replaced");
    let reference = root.join("beta/modules/api/pages/reference.adoc");
    let reference_edits = preview
        .edits
        .iter()
        .find(|file| file.path.ends_with("api/pages/reference.adoc"))
        .unwrap();
    assert_eq!(reference_edits.edits[0].replacement, "= Title: API");

    engine
        .invoke(
            ids::WORKSPACE_APPLY_PREVIEW,
            CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
        )
        .unwrap();
    let id = engine.document_for_path(&reference).unwrap();
    assert_eq!(
        engine.document(id).unwrap().text.to_string(),
        "= Title: API Reference\n"
    );
    assert!(
        engine.preview(preview.id).is_none(),
        "a preview applies once"
    );

    // One undo from any of the documents restores all of them.
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert!(engine
        .documents()
        .all(|document| !document.text.to_string().contains("Title:")));
    assert!(engine.documents().all(|document| !document.dirty));

    // Replacing only selected matches.
    let selected = preview_for_selection(&mut engine, &reference);
    assert_eq!(selected.edits.len(), 1);
}

fn preview_for_selection(engine: &mut Engine, path: &Path) -> EditPreview {
    preview(
        engine
            .invoke(
                ids::WORKSPACE_REPLACE,
                CommandArgs::Custom(serde_json::json!({
                    "request": { "query": "API" , "case_sensitive": true },
                    "replacement": "Interface",
                    "matches": [{ "path": path, "start": 2, "end": 5 }],
                })),
            )
            .unwrap(),
    )
}

#[test]
fn rename_page_rewrites_xrefs_as_one_undo_group() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(&directory.path().join("docs"));
    copy_tree(&fixture("antora-multi-component"), &root);
    let mut engine = open_workspace(&root);
    let pages = root.join("alpha/modules/ROOT/pages");
    let old_path = pages.join("guide.adoc");
    let new_path = pages.join("howto/setup-guide.adoc");
    let nav = root.join("alpha/modules/ROOT/nav.adoc");
    let index = pages.join("index.adoc");
    let original_nav = fs::read_to_string(&nav).unwrap();
    let original_index = fs::read_to_string(&index).unwrap();

    let guide = engine.open(&old_path).unwrap();
    let preview = preview(
        engine
            .invoke(
                ids::PAGE_RENAME,
                CommandArgs::Rename {
                    id: guide,
                    new_path: new_path.clone(),
                },
            )
            .unwrap(),
    );
    assert_eq!(preview.moves.len(), 1);
    assert_eq!(preview.moves[0].to, new_path);
    let edited: Vec<String> = preview
        .edits
        .iter()
        .map(|file| relative(&file.path, &root))
        .collect();
    assert_eq!(
        edited,
        [
            "alpha/modules/ROOT/nav.adoc",
            "alpha/modules/ROOT/pages/index.adoc"
        ]
    );
    assert!(old_path.exists(), "a preview changes nothing");

    engine
        .invoke(
            ids::WORKSPACE_APPLY_PREVIEW,
            CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
        )
        .unwrap();
    assert!(!old_path.exists());
    assert!(new_path.exists());
    assert_eq!(engine.document(guide).unwrap().path, new_path);
    let nav_id = engine.document_for_path(&nav).unwrap();
    let nav_text = engine.document(nav_id).unwrap().text.to_string();
    assert!(nav_text.contains("* xref:index.adoc[Home]"));
    assert!(nav_text.contains("** xref:howto/setup-guide.adoc[Guide]"));
    assert!(nav_text.contains("*** xref:howto/setup-guide.adoc#setup[Setup]"));
    let index_id = engine.document_for_path(&index).unwrap();
    assert!(engine
        .document(index_id)
        .unwrap()
        .text
        .to_string()
        .contains("xref:howto/setup-guide.adoc[old guide link]"));
    // The renamed page still resolves.
    let offset = nav_text.find("xref:howto").unwrap() + 6;
    assert!(engine.definition(nav_id, offset).is_some());

    // One undo restores every file and moves the page back.
    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(nav_id))
        .unwrap();
    assert!(old_path.exists());
    assert!(!new_path.exists());
    assert_eq!(engine.document(guide).unwrap().path, old_path);
    assert_eq!(
        engine.document(nav_id).unwrap().text.to_string(),
        original_nav
    );
    assert_eq!(
        engine.document(index_id).unwrap().text.to_string(),
        original_index
    );

    // Redo re-applies the whole group.
    engine
        .invoke(ids::EDIT_REDO, CommandArgs::Document(guide))
        .unwrap();
    assert!(new_path.exists());
    assert!(engine
        .document(nav_id)
        .unwrap()
        .text
        .to_string()
        .contains("howto/setup-guide.adoc"));
}

#[test]
fn rename_rejects_an_existing_target() {
    let root = fixture("xrefs");
    let mut engine = open_workspace(&root);
    let id = engine.open(&root.join("index.adoc")).unwrap();
    assert!(engine
        .invoke(
            ids::PAGE_RENAME,
            CommandArgs::Rename {
                id,
                new_path: root.join("other.adoc"),
            },
        )
        .is_err());
}
