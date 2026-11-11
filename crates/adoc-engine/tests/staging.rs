//! Staged changes (MCP PRD §7.5): worked out and checked without touching the workspace, then
//! applied as one undo group only while every file still holds what they were worked out
//! against. On copies of `tests/fixtures/discovery`.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{normalize_path, text_hash, Engine, EngineError, NewPage, Settings, TextEdit};

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

fn workspace() -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = normalize_path(&directory.path().join("discovery"));
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/discovery"),
        &root,
    );
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    (directory, root, engine)
}

fn module(root: &Path) -> PathBuf {
    root.join("product/main/modules/ROOT")
}

fn new_page(root: &Path) -> NewPage {
    NewPage {
        module: module(root),
        title: "Configuring Authentication".into(),
        add_to_nav: true,
        file: None,
    }
}

#[test]
fn a_staged_page_is_what_new_page_makes_and_writes_nothing() {
    let (_directory, root, engine) = workspace();
    let nav = module(&root).join("nav.adoc");
    let before = fs::read_to_string(&nav).unwrap();
    let changes = engine.stage_new_page(&new_page(&root)).unwrap();
    let page = module(&root).join("pages/configuring-authentication.adoc");
    assert_eq!(changes.creates[0].path, page);
    assert_eq!(changes.creates[0].text, "= Configuring Authentication\n");
    assert_eq!(changes.edits[0].path, nav);
    assert_eq!(changes.edits[0].base, before);

    let files = engine.check_staged(&changes).unwrap();
    let staged_nav = files.iter().find(|file| file.path == nav).unwrap();
    assert!(staged_nav
        .after
        .ends_with("* xref:install.adoc[Installing]\n* xref:configuring-authentication.adoc[]\n"));
    // Checked as the workspace would be, with the page made: the entry resolves.
    assert!(
        staged_nav.diagnostics.is_empty(),
        "{:?}",
        staged_nav.diagnostics
    );
    assert!(!page.exists());
    assert_eq!(fs::read_to_string(&nav).unwrap(), before);
    assert!(engine.document_for_path(&nav).is_none(), "nothing opened");
    assert!(
        engine.workspace().index.file(&page).is_none(),
        "nothing indexed"
    );
}

#[test]
fn a_staged_change_is_one_undo_step() {
    let (_directory, root, mut engine) = workspace();
    let install = module(&root).join("pages/install.adoc");
    let mut changes = engine.stage_new_page(&new_page(&root)).unwrap();
    let text = fs::read_to_string(&install).unwrap();
    let at = text.find("You need things.").unwrap();
    changes
        .merge(
            engine
                .stage_edits(
                    &install,
                    vec![TextEdit::new(
                        SourceRange::new(at, at + "You need things.".len()),
                        "You need a licence.".to_owned(),
                    )],
                )
                .unwrap(),
        )
        .unwrap();
    engine.apply_staged(&changes).unwrap();
    let page = module(&root).join("pages/configuring-authentication.adoc");
    assert!(
        page.exists(),
        "a made file is written as page.new writes it"
    );
    let nav = module(&root).join("nav.adoc");
    let nav_id = engine.document_for_path(&nav).unwrap();
    assert!(engine
        .document(nav_id)
        .unwrap()
        .text_string()
        .contains("configuring-authentication"));
    let install_id = engine.document_for_path(&install).unwrap();
    assert!(engine
        .document(install_id)
        .unwrap()
        .text_string()
        .contains("a licence"));

    // One undo, from any of its pages, takes back all of it.
    assert!(engine.undo(install_id));
    assert!(!engine
        .document(nav_id)
        .unwrap()
        .text_string()
        .contains("configuring-authentication"));
    assert!(!engine
        .document(install_id)
        .unwrap()
        .text_string()
        .contains("a licence"));
    assert!(!page.exists());
}

#[test]
fn a_change_against_text_since_changed_is_refused_whole() {
    let (_directory, root, mut engine) = workspace();
    let changes = engine.stage_new_page(&new_page(&root)).unwrap();
    let nav = module(&root).join("nav.adoc");
    fs::write(&nav, "* xref:index.adoc[]\n").unwrap();
    let error = engine.apply_staged(&changes).unwrap_err();
    assert!(matches!(error, EngineError::PreviewStale { .. }), "{error}");
    assert!(!module(&root)
        .join("pages/configuring-authentication.adoc")
        .exists());

    // A file to make that appears meanwhile refuses the change as well.
    let changes = engine
        .stage_new_partial(&module(&root).join("partials/fresh.adoc"), "Fresh.\n")
        .unwrap();
    fs::write(module(&root).join("partials/fresh.adoc"), "Mine.\n").unwrap();
    assert!(engine.apply_staged(&changes).is_err());
    assert_eq!(
        fs::read_to_string(module(&root).join("partials/fresh.adoc")).unwrap(),
        "Mine.\n"
    );
}

#[test]
fn a_refused_change_leaves_no_page_open() {
    let (_directory, root, mut engine) = workspace();
    let pages = module(&root).join("pages");
    let mut changes = engine
        .stage_edits(
            &pages.join("index.adoc"),
            vec![TextEdit::new(SourceRange::new(0, 0), "// one\n")],
        )
        .unwrap();
    changes.edits.extend(
        engine
            .stage_edits(
                &pages.join("install.adoc"),
                vec![TextEdit::new(SourceRange::new(0, 0), "// two\n")],
            )
            .unwrap()
            .edits,
    );
    // The last file the change edits is rewritten after it was staged.
    let last = changes.edits.last().unwrap().path.clone();
    fs::write(&last, "Rewritten.\n").unwrap();
    let tabs = engine.tab_order().to_vec();
    let error = engine.apply_staged(&changes).unwrap_err();
    assert!(matches!(error, EngineError::PreviewStale { .. }), "{error}");
    assert_eq!(engine.tab_order(), tabs);
    assert_eq!(engine.documents().count(), 0);
}

#[test]
fn a_staged_rename_moves_the_page_and_rewrites_what_names_it() {
    let (_directory, root, mut engine) = workspace();
    let install = module(&root).join("pages/install.adoc");
    let renamed = module(&root).join("pages/setup.adoc");
    let changes = engine.stage_rename(&install, &renamed).unwrap();
    assert!(
        engine.document_for_path(&install).is_none(),
        "opened and closed again"
    );
    assert!(install.exists() && !renamed.exists(), "nothing moved yet");
    let files = engine.check_staged(&changes).unwrap();
    let nav = files
        .iter()
        .find(|file| file.path == module(&root).join("nav.adoc"))
        .unwrap();
    assert!(nav.after.contains("xref:setup.adoc[Installing]"));
    let moved = files.iter().find(|file| file.path == install).unwrap();
    assert_eq!(moved.moved_to.as_deref(), Some(renamed.as_path()));

    engine.apply_staged(&changes).unwrap();
    assert!(renamed.exists() && !install.exists());
}

#[test]
fn hashes_name_texts() {
    assert_eq!(
        text_hash(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_ne!(text_hash("a"), text_hash("b"));
}
