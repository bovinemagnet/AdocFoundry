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

#[test]
fn rename_rewrites_includes() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(&directory.path().join("docs"));
    copy_tree(&fixture("antora-single-component"), &root);
    copy_tree(&fixture("references"), &root.join("loose"));
    let mut engine = open_workspace(&root);

    // An Antora include keeps its family and coordinates.
    let partials = root.join("modules/security/partials");
    let note = engine.open(&partials.join("token-note.adoc")).unwrap();
    let preview = engine
        .rename_preview(note, &partials.join("notes/token.adoc"))
        .unwrap();
    let edits: Vec<_> = preview
        .edits
        .iter()
        .flat_map(|file| {
            file.edits
                .iter()
                .map(|edit| (relative(&file.path, &root), edit.replacement.clone()))
        })
        .collect();
    assert_eq!(
        edits,
        [(
            "modules/security/pages/authentication.adoc".to_owned(),
            "partial$notes/token.adoc".to_owned()
        )]
    );

    // A relative include stays relative.
    let shared = engine.open(&root.join("loose/shared.adoc")).unwrap();
    let preview = engine
        .rename_preview(shared, &root.join("loose/common/shared.adoc"))
        .unwrap();
    let file = &preview.edits[0];
    assert_eq!(relative(&file.path, &root), "loose/index.adoc");
    assert_eq!(file.edits[0].replacement, "common/shared.adoc");
    let text = fs::read_to_string(&file.path).unwrap();
    let range = file.edits[0].range;
    assert_eq!(&text[range.start..range.end], "shared.adoc");
}

/// Writes each `(path, text)` under `root`.
fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

fn rename_and_apply(engine: &mut Engine, from: &Path, to: &Path) {
    let id = engine.open(from).unwrap();
    let preview = preview(
        engine
            .invoke(
                ids::PAGE_RENAME,
                CommandArgs::Rename {
                    id,
                    new_path: to.to_path_buf(),
                },
            )
            .unwrap(),
    );
    apply(engine, &preview).unwrap();
}

fn diagnostics_of(engine: &Engine, path: &Path) -> Vec<String> {
    engine
        .workspace_diagnostics()
        .into_iter()
        .filter(|(file, _)| file == path)
        .flat_map(|(_, diagnostics)| diagnostics)
        .map(|diagnostic| format!("{:?}: {}", diagnostic.code, diagnostic.message))
        .collect()
}

#[test]
fn moving_a_plain_page_keeps_its_own_references_resolving() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(&directory.path().join("docs"));
    write_files(
        &root,
        &[
            (
                "index.adoc",
                "= Index\n\n[[top]]\nSee xref:child.adoc[Child] and xref:index.adoc#top[the top].\n\ninclude::part.adoc[]\n\nimage::pic.svg[]\n",
            ),
            ("child.adoc", "= Child\n"),
            ("part.adoc", "Part.\n"),
            ("pic.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n"),
        ],
    );
    let mut engine = open_workspace(&root);
    let moved = root.join("guides/index.adoc");
    rename_and_apply(&mut engine, &root.join("index.adoc"), &moved);

    let text = current_text(&engine, &moved);
    assert!(text.contains("xref:../child.adoc[Child]"), "{text}");
    assert!(
        text.contains("xref:index.adoc#top[the top]"),
        "a reference to itself: {text}"
    );
    assert!(text.contains("include::../part.adoc[]"), "{text}");
    assert!(text.contains("image::../pic.svg[]"), "{text}");
    assert_eq!(diagnostics_of(&engine, &moved), Vec::<String>::new());
}

/// An Antora component `docs` with modules `ROOT` and `api`.
fn antora_component() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(&directory.path().join("docs"));
    write_files(
        &root,
        &[
            ("antora.yml", "name: docs\nversion: ~\n"),
            (
                "modules/ROOT/pages/page.adoc",
                "= Page\n\nSee xref:other.adoc[Other].\n\ninclude::partial$p.adoc[]\n\nimage::pic.svg[]\n",
            ),
            ("modules/ROOT/pages/other.adoc", "= Other\n"),
            ("modules/ROOT/partials/p.adoc", "Partial.\n"),
            ("modules/ROOT/images/pic.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n"),
            ("modules/api/pages/api.adoc", "= API\n"),
        ],
    );
    (directory, root)
}

fn current_text(engine: &Engine, path: &Path) -> String {
    let id = engine.document_for_path(path).unwrap();
    engine.document(id).unwrap().text.to_string()
}

/// A copy of the `xrefs` fixture, where `index.adoc` links to `other.adoc`, with `index.adoc`
/// open.
fn xrefs_copy() -> (tempfile::TempDir, PathBuf, Engine, adoc_engine::DocumentId) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(&directory.path().join("docs"));
    copy_tree(&fixture("xrefs"), &root);
    let mut engine = open_workspace(&root);
    let index = engine.open(&root.join("index.adoc")).unwrap();
    (directory, root, engine, index)
}

fn rename_other(engine: &mut Engine, root: &Path, to: &Path) -> Result<EditPreview, String> {
    let id = engine.open(&root.join("other.adoc")).unwrap();
    engine
        .invoke(
            ids::PAGE_RENAME,
            CommandArgs::Rename {
                id,
                new_path: to.to_path_buf(),
            },
        )
        .map(preview)
        .map_err(|error| error.to_string())
}

fn apply(engine: &mut Engine, preview: &EditPreview) -> Result<(), adoc_engine::EngineError> {
    engine
        .invoke(
            ids::WORKSPACE_APPLY_PREVIEW,
            CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
        )
        .map(|_| ())
}

fn text_of(engine: &Engine, id: adoc_engine::DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

#[test]
fn moving_an_antora_page_to_another_module_qualifies_its_references() {
    let (_directory, root) = antora_component();
    let mut engine = open_workspace(&root);
    let moved = root.join("modules/api/pages/page.adoc");
    rename_and_apply(
        &mut engine,
        &root.join("modules/ROOT/pages/page.adoc"),
        &moved,
    );

    let text = current_text(&engine, &moved);
    assert!(text.contains("xref:ROOT:other.adoc[Other]"), "{text}");
    assert!(text.contains("include::ROOT:partial$p.adoc[]"), "{text}");
    assert!(text.contains("image::ROOT:pic.svg[]"), "{text}");
}

#[test]
fn moving_an_antora_page_within_its_module_changes_nothing_in_it() {
    let (_directory, root) = antora_component();
    let mut engine = open_workspace(&root);
    let page = root.join("modules/ROOT/pages/page.adoc");
    let before = fs::read_to_string(&page).unwrap();
    let moved = root.join("modules/ROOT/pages/guides/page.adoc");
    rename_and_apply(&mut engine, &page, &moved);
    assert_eq!(current_text(&engine, &moved), before);
}

#[test]
fn a_preview_is_refused_once_a_file_it_edits_has_changed() {
    let (_directory, root, mut engine, index) = xrefs_copy();
    let preview = rename_other(&mut engine, &root, &root.join("renamed.adoc")).unwrap();
    assert!(preview
        .edits
        .iter()
        .any(|file| file.path == root.join("index.adoc")));
    // What the dialog shows is the text the edits were computed against.
    let base = &preview.edits[0].base;
    assert!(base.contains("xref:other.adoc"), "{base}");

    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: index,
                range: adoc_core::SourceRange::new(0, 0),
                text: "// moved the offsets\n".to_owned(),
            },
        )
        .unwrap();
    let refused = apply(&mut engine, &preview);
    assert!(
        matches!(&refused, Err(adoc_engine::EngineError::PreviewStale { path }) if *path == root.join("index.adoc")),
        "{refused:?}"
    );
    assert!(root.join("other.adoc").is_file(), "nothing moved");
    assert!(text_of(&engine, index).starts_with("// moved the offsets\n= Cross References"));
}

#[test]
fn a_rename_whose_move_cannot_happen_changes_nothing() {
    let (_directory, root, mut engine, index) = xrefs_copy();
    fs::write(root.join("blocked"), "a file, not a directory").unwrap();
    let before = text_of(&engine, index);

    let refused = rename_other(&mut engine, &root, &root.join("blocked/other.adoc"))
        .and_then(|preview| apply(&mut engine, &preview).map_err(|error| error.to_string()));
    assert!(refused.is_err());
    assert_eq!(text_of(&engine, index), before);
    assert!(!engine.document(index).unwrap().dirty);
    assert!(root.join("other.adoc").is_file());
    assert!(!engine.undo(index), "no partial group in the undo history");
}

#[cfg(unix)]
#[test]
fn a_move_that_fails_part_way_is_rolled_back() {
    use std::os::unix::fs::PermissionsExt;
    let (_directory, root, mut engine, index) = xrefs_copy();
    let locked = root.join("locked");
    fs::create_dir(&locked).unwrap();
    let before = text_of(&engine, index);
    let preview = rename_other(&mut engine, &root, &locked.join("other.adoc")).unwrap();

    // The destination passes every check but cannot be written to.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let result = apply(&mut engine, &preview);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(result.is_err());
    assert_eq!(
        text_of(&engine, index),
        before,
        "the xref edit is rolled back"
    );
    assert!(!engine.document(index).unwrap().dirty);
    assert!(root.join("other.adoc").is_file());
    assert!(!engine.undo(index), "no partial group in the undo history");
}
