//! The last session, saved on quitting and brought back at the next launch, and the folders
//! opened recently.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, CommandOutcome, DocumentId, EditorMode, Engine,
    OpenDisposition, Settings, TabItem, RECENT_FOLDERS_FILE, SESSION_FILE,
};

/// A state folder, and a workspace of three pages and an image.
struct Setup {
    state: tempfile::TempDir,
    pages: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let pages = tempfile::tempdir().unwrap();
        for name in ["one", "two", "three"] {
            fs::write(
                pages.path().join(format!("{name}.adoc")),
                format!("= Page {name}\n\nSome text about {name}.\n"),
            )
            .unwrap();
        }
        fs::write(
            pages.path().join("picture.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#,
        )
        .unwrap();
        Self {
            state: tempfile::tempdir().unwrap(),
            pages,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        normalize_path(&self.pages.path().join(name))
    }

    fn root(&self) -> PathBuf {
        normalize_path(self.pages.path())
    }

    /// An engine as the app has one: preview tabs and sessions on, its state kept here.
    fn engine(&self) -> Engine {
        let settings = Settings::default()
            .with_git_opt_in()
            .with_state_dir(Some(self.state.path().to_path_buf()));
        let mut engine = Engine::new(settings);
        engine.enable_preview_tabs();
        engine.enable_sessions();
        engine
    }
}

fn open_workspace(engine: &mut Engine, root: &Path) {
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
}

fn open(engine: &mut Engine, path: &Path, disposition: OpenDisposition) -> TabItem {
    engine.open_item(path, disposition).unwrap()
}

fn document(item: TabItem) -> DocumentId {
    item.document().unwrap()
}

/// The path each tab shows, in tab order.
fn tab_paths(engine: &Engine) -> Vec<PathBuf> {
    engine
        .tab_order()
        .iter()
        .map(|item| match item {
            TabItem::Document(id) => engine.document(*id).unwrap().path.clone(),
            TabItem::Image(id) => engine.image(*id).unwrap().path.clone(),
        })
        .collect()
}

fn id_of(engine: &Engine, path: &Path) -> DocumentId {
    engine.document_for_path(path).unwrap()
}

#[test]
fn four_tabs_in_mixed_modes_and_the_preview_come_back() {
    let setup = Setup::new();
    let carets = {
        let mut engine = setup.engine();
        open_workspace(&mut engine, &setup.root());
        let one = document(open(
            &mut engine,
            &setup.path("one.adoc"),
            OpenDisposition::Persistent,
        ));
        let two = document(open(
            &mut engine,
            &setup.path("two.adoc"),
            OpenDisposition::Persistent,
        ));
        open(
            &mut engine,
            &setup.path("picture.svg"),
            OpenDisposition::Persistent,
        );
        let three = document(open(
            &mut engine,
            &setup.path("three.adoc"),
            OpenDisposition::Preview,
        ));
        engine
            .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(two))
            .unwrap();
        engine
            .invoke(ids::VIEW_SHOW_SPLIT, CommandArgs::Document(three))
            .unwrap();
        engine
            .invoke(ids::PREVIEW_PIN, CommandArgs::Document(two))
            .unwrap();
        engine
            .invoke(ids::VIEW_TOGGLE_SCROLL_SYNC, CommandArgs::None)
            .unwrap();
        engine.activate(TabItem::Document(one));
        let carets = HashMap::from([(one, 3), (two, 12), (three, 0)]);
        engine.write_session(&carets).unwrap();
        carets.len()
    };
    assert!(setup.state.path().join(SESSION_FILE).is_file());

    let mut engine = setup.engine();
    let carets_back = engine.restore_session();
    assert_eq!(engine.roots(), [setup.root()]);
    assert_eq!(
        tab_paths(&engine),
        [
            setup.path("one.adoc"),
            setup.path("two.adoc"),
            setup.path("picture.svg"),
            setup.path("three.adoc"),
        ]
    );
    let (one, two, three) = (
        id_of(&engine, &setup.path("one.adoc")),
        id_of(&engine, &setup.path("two.adoc")),
        id_of(&engine, &setup.path("three.adoc")),
    );
    assert_eq!(engine.editor_mode(one), EditorMode::Source);
    assert_eq!(engine.editor_mode(two), EditorMode::Visual);
    assert_eq!(engine.editor_mode(three), EditorMode::Split);
    assert!(engine.visual(two).is_some());
    // The page browsed to is in the preview tab again; the others are kept.
    assert!(engine.is_preview(three));
    assert!(!engine.is_preview(one) && !engine.is_preview(two));
    assert_eq!(engine.active_item(), Some(TabItem::Document(one)));

    let layout = engine.preview_layout();
    assert!(layout.visible);
    assert!(!layout.detached);
    assert!(!layout.scroll_sync);
    assert_eq!(layout.pinned, Some(two));

    assert_eq!(carets_back.len(), carets);
    assert_eq!(carets_back[&one], 3);
    assert_eq!(carets_back[&two], 12);
    assert_eq!(carets_back[&three], 0);
}

#[test]
fn files_and_folders_that_have_gone_are_skipped() {
    let setup = Setup::new();
    let other = tempfile::tempdir().unwrap();
    {
        let mut engine = setup.engine();
        engine
            .open_workspace(vec![setup.root(), other.path().to_path_buf()])
            .unwrap();
        for name in ["one.adoc", "two.adoc", "three.adoc"] {
            open(&mut engine, &setup.path(name), OpenDisposition::Persistent);
        }
        let two = id_of(&engine, &setup.path("two.adoc"));
        engine.activate(TabItem::Document(two));
        engine
            .invoke(ids::PREVIEW_PIN, CommandArgs::Document(two))
            .unwrap();
        engine.write_session(&HashMap::new()).unwrap();
    }
    fs::remove_file(setup.path("two.adoc")).unwrap();
    let other_root = normalize_path(other.path());
    drop(other);

    let mut engine = setup.engine();
    engine.restore_session();
    assert_eq!(engine.roots(), [setup.root()]);
    assert_eq!(
        tab_paths(&engine),
        [setup.path("one.adoc"), setup.path("three.adoc")]
    );
    // The active and pinned page went; another tab is active and nothing is pinned.
    assert!(engine.active_item().is_some());
    assert_eq!(engine.preview_layout().pinned, None);
    assert!(!other_root.exists());
}

#[test]
fn a_caret_past_the_end_or_inside_a_character_is_brought_back_inside() {
    let setup = Setup::new();
    fs::write(setup.path("one.adoc"), "= Café\n").unwrap();
    {
        let mut engine = setup.engine();
        open_workspace(&mut engine, &setup.root());
        let one = document(open(
            &mut engine,
            &setup.path("one.adoc"),
            OpenDisposition::Persistent,
        ));
        let two = document(open(
            &mut engine,
            &setup.path("two.adoc"),
            OpenDisposition::Persistent,
        ));
        // Inside `é`, two bytes from 5; and far beyond the end.
        engine
            .write_session(&HashMap::from([(one, 6), (two, 10_000)]))
            .unwrap();
    }
    // The page was shortened after the session ended.
    fs::write(setup.path("two.adoc"), "= Two\n").unwrap();

    let mut engine = setup.engine();
    let carets = engine.restore_session();
    assert_eq!(carets[&id_of(&engine, &setup.path("one.adoc"))], 5);
    assert_eq!(carets[&id_of(&engine, &setup.path("two.adoc"))], 6);
}

#[test]
fn untitled_pages_are_not_saved_in_the_session() {
    let setup = Setup::new();
    {
        let mut engine = setup.engine();
        open_workspace(&mut engine, &setup.root());
        engine.open_untitled();
        open(
            &mut engine,
            &setup.path("one.adoc"),
            OpenDisposition::Persistent,
        );
        engine.write_session(&HashMap::new()).unwrap();
    }
    let mut engine = setup.engine();
    engine.restore_session();
    assert_eq!(tab_paths(&engine), [setup.path("one.adoc")]);
}

#[test]
fn nothing_is_restored_without_a_session_or_from_a_damaged_one() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    assert!(engine.restore_session().is_empty());
    assert!(engine.roots().is_empty() && engine.tab_order().is_empty());

    fs::write(setup.state.path().join(SESSION_FILE), "{ not json").unwrap();
    let mut engine = setup.engine();
    assert!(engine.restore_session().is_empty());
    assert!(engine.roots().is_empty() && engine.tab_order().is_empty());
}

#[test]
fn a_front_end_without_sessions_saves_and_restores_nothing() {
    let setup = Setup::new();
    let settings = Settings::default()
        .with_git_opt_in()
        .with_state_dir(Some(setup.state.path().to_path_buf()));
    let mut engine = Engine::new(settings);
    open_workspace(&mut engine, &setup.root());
    engine.open(&setup.path("one.adoc")).unwrap();
    engine.write_session(&HashMap::new()).unwrap();
    assert!(engine.restore_session().is_empty());
    assert!(fs::read_dir(setup.state.path()).unwrap().next().is_none());
}

#[test]
fn folders_opened_are_listed_most_recent_first() {
    let setup = Setup::new();
    let folders: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let paths: Vec<PathBuf> = folders
        .iter()
        .map(|dir| normalize_path(dir.path()))
        .collect();
    {
        let mut engine = setup.engine();
        for path in [&paths[0], &paths[1], &paths[2], &paths[0]] {
            engine
                .invoke(ids::WORKSPACE_OPEN_FOLDER, CommandArgs::Path(path.clone()))
                .unwrap();
        }
        assert_eq!(
            engine.recent_folders(),
            [paths[0].clone(), paths[2].clone(), paths[1].clone()]
        );
    }
    assert!(setup.state.path().join(RECENT_FOLDERS_FILE).is_file());

    // Remembered in the next session; a folder that has gone is left out.
    let mut folders = folders;
    folders.remove(1);
    let mut engine = setup.engine();
    assert_eq!(
        engine.recent_folders(),
        [paths[0].clone(), paths[2].clone()]
    );
    let outcome = engine
        .invoke(ids::WORKSPACE_OPEN_RECENT, CommandArgs::None)
        .unwrap();
    assert_eq!(
        outcome,
        CommandOutcome::RecentFolders(vec![paths[0].clone(), paths[2].clone()])
    );
}

#[test]
fn only_the_latest_folders_are_remembered() {
    let setup = Setup::new();
    let folders: Vec<tempfile::TempDir> = (0..adoc_engine::RECENT_FOLDERS + 3)
        .map(|_| tempfile::tempdir().unwrap())
        .collect();
    let mut engine = setup.engine();
    for folder in &folders {
        engine
            .open_workspace(vec![folder.path().to_path_buf()])
            .unwrap();
    }
    let recent = engine.recent_folders();
    assert_eq!(recent.len(), adoc_engine::RECENT_FOLDERS);
    assert_eq!(recent[0], normalize_path(folders.last().unwrap().path()));
}
