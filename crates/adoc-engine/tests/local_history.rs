//! Local history: each page's saved versions, kept beside the settings file, compared with the
//! page and restored.

use std::{fs, path::PathBuf, sync::Arc};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DiffLine, DocumentId, Engine, ManualClock,
    SavedVersion, Settings, HISTORY_DIR,
};

struct Setup {
    _config: tempfile::TempDir,
    pages: tempfile::TempDir,
    user: PathBuf,
}

impl Setup {
    fn new(settings: &str) -> Self {
        let config = tempfile::tempdir().unwrap();
        let pages = tempfile::tempdir().unwrap();
        let user = config.path().join("adocfoundry.toml");
        fs::write(&user, settings).unwrap();
        fs::write(pages.path().join("page.adoc"), "first\n").unwrap();
        Self {
            _config: config,
            pages,
            user,
        }
    }

    fn page(&self) -> PathBuf {
        adoc_engine::normalize_path(&self.pages.path().join("page.adoc"))
    }

    /// The app's state folder, apart from the settings file.
    fn state(&self) -> PathBuf {
        self.user.parent().unwrap().join("state")
    }

    fn store(&self) -> PathBuf {
        self.state().join(HISTORY_DIR)
    }

    /// The settings as the app loads them, with its state folder.
    fn settings(&self) -> Settings {
        Settings::load(Some(&self.user), None)
            .unwrap()
            .with_git_opt_in()
            .with_state_dir(Some(self.state()))
    }

    fn engine(&self) -> Engine {
        let mut engine = Engine::with_clock(self.settings(), Arc::new(ManualClock::new()));
        engine.enable_local_history();
        engine
    }
}

fn replace_all(engine: &mut Engine, id: DocumentId, text: &str) {
    let len = engine.document(id).unwrap().text.len();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, len),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

fn save_as_text(engine: &mut Engine, id: DocumentId, text: &str) {
    replace_all(engine, id, text);
    engine.save(id).unwrap();
}

fn texts(engine: &Engine, setup: &Setup) -> Vec<String> {
    engine
        .local_history(&setup.page())
        .iter()
        .map(|version| engine.saved_version(&setup.page(), &version.id).unwrap())
        .collect()
}

fn history(engine: &mut Engine, id: DocumentId) -> Vec<SavedVersion> {
    match engine.invoke(ids::DOCUMENT_LOCAL_HISTORY, CommandArgs::Document(id)) {
        Ok(CommandOutcome::LocalHistory { document, versions }) => {
            assert_eq!(document, id);
            versions
        }
        other => panic!("{other:?}"),
    }
}

fn restore(engine: &mut Engine, id: DocumentId, version: &str) -> Result<CommandOutcome, String> {
    engine
        .invoke(
            ids::DOCUMENT_RESTORE_VERSION,
            CommandArgs::Custom(serde_json::json!({ "document": id.get(), "version": version })),
        )
        .map_err(|error| error.to_string())
}

#[test]
fn an_earlier_saved_version_can_be_restored() {
    let setup = Setup::new("");
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    save_as_text(&mut engine, id, "second\n");
    save_as_text(&mut engine, id, "third\n");

    // Newest first, with the file as it was before the first save.
    assert_eq!(texts(&engine, &setup), ["third\n", "second\n", "first\n"]);
    let versions = history(&mut engine, id);
    assert_eq!(versions.len(), 3);
    assert!(versions[0].saved >= versions[1].saved && versions[1].saved >= versions[2].saved);
    assert_eq!(versions[2].size, 6);

    // What the page has changed since the first version.
    assert_eq!(
        engine.version_diff(id, &versions[2].id).unwrap(),
        vec![
            DiffLine::Removed("first".into()),
            DiffLine::Added("third".into()),
        ]
    );

    assert_eq!(
        restore(&mut engine, id, &versions[2].id),
        Ok(CommandOutcome::Done)
    );
    let document = engine.document(id).unwrap();
    assert_eq!(document.text.to_string(), "first\n");
    assert!(document.dirty, "restored, not saved");
    assert_eq!(fs::read_to_string(setup.page()).unwrap(), "third\n");
    assert!(engine.undo(id), "one undo step");
    assert_eq!(engine.document(id).unwrap().text.to_string(), "third\n");
}

#[test]
fn saving_the_same_text_again_adds_no_version() {
    let setup = Setup::new("");
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    save_as_text(&mut engine, id, "second\n");
    engine.save(id).unwrap();
    save_as_text(&mut engine, id, "second\n");
    assert_eq!(texts(&engine, &setup), ["second\n", "first\n"]);
}

#[test]
fn a_change_made_outside_is_kept_before_it_is_overwritten() {
    let setup = Setup::new("");
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    save_as_text(&mut engine, id, "second\n");
    fs::write(setup.page(), "from another editor\n").unwrap();
    replace_all(&mut engine, id, "mine\n");
    engine.overwrite(id).unwrap();
    assert_eq!(
        texts(&engine, &setup),
        ["mine\n", "from another editor\n", "second\n", "first\n"]
    );
}

#[test]
fn only_the_newest_versions_are_kept() {
    let setup = Setup::new("[history]\nmax_versions = 3\n");
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    for text in ["2\n", "3\n", "4\n", "5\n"] {
        save_as_text(&mut engine, id, text);
    }
    assert_eq!(texts(&engine, &setup), ["5\n", "4\n", "3\n"]);
}

#[test]
fn versions_older_than_keep_days_are_removed() {
    let setup = Setup::new("[history]\nkeep_days = 1\n");
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    save_as_text(&mut engine, id, "second\n");
    // A version saved two days ago, in the page's folder of the store.
    let folder = fs::read_dir(setup.store())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
        .unwrap();
    let two_days_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        - 2 * 24 * 60 * 60 * 1000;
    fs::write(
        folder.join(format!("{two_days_ago}-0123456789abcdef")),
        "old\n",
    )
    .unwrap();
    assert_eq!(texts(&engine, &setup).len(), 3);
    save_as_text(&mut engine, id, "third\n");
    assert_eq!(texts(&engine, &setup), ["third\n", "second\n", "first\n"]);
}

#[test]
fn a_version_is_named_only_by_what_the_history_lists() {
    let setup = Setup::new("");
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    save_as_text(&mut engine, id, "second\n");
    for name in ["../adocfoundry.toml", "page", "", "1-2/../../x"] {
        assert!(restore(&mut engine, id, name).is_err(), "{name:?}");
        assert!(engine.saved_version(&setup.page(), name).is_err());
    }
    assert_eq!(engine.document(id).unwrap().text.to_string(), "second\n");
}

#[test]
fn no_history_is_kept_when_turned_off_or_not_asked_for() {
    for (settings, enable) in [("[history]\nenabled = false\n", true), ("", false)] {
        let setup = Setup::new(settings);
        let mut engine = Engine::new(setup.settings());
        if enable {
            engine.enable_local_history();
        }
        let id = engine.open(&setup.page()).unwrap();
        save_as_text(&mut engine, id, "second\n");
        assert!(!setup.store().exists());
        assert!(history(&mut engine, id).is_empty());
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

/// The versions in another page's folder of the store, by name.
fn versions_in(folder: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != "path")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn the_whole_history_is_kept_under_max_size_mb_oldest_first() {
    let setup = Setup::new("[history]\nmax_size_mb = 1\n");
    // Another page's history from an earlier session: 1.2 MB in two versions.
    let other = setup.store().join("0123456789abcdef0123456789abcdef");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("path"), "/elsewhere/other.adoc").unwrap();
    let big = "x".repeat(600 * 1024);
    let now = now_millis();
    let older = format!("{}-aaaaaaaaaaaaaaaa", now - 2000);
    let newer = format!("{}-bbbbbbbbbbbbbbbb", now - 1000);
    fs::write(other.join(&older), &big).unwrap();
    fs::write(other.join(&newer), &big).unwrap();

    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(setup.settings(), Arc::new(clock.clone()));
    engine.enable_local_history();
    let id = engine.open(&setup.page()).unwrap();
    save_as_text(&mut engine, id, "second\n");
    assert_eq!(
        versions_in(&other),
        [newer],
        "the oldest version went first"
    );
    assert_eq!(texts(&engine, &setup), ["second\n", "first\n"]);

    // Checked again once an hour, not at every save.
    let latest = format!("{}-cccccccccccccccc", now_millis());
    fs::write(other.join(&latest), &big).unwrap();
    save_as_text(&mut engine, id, "third\n");
    assert_eq!(versions_in(&other).len(), 2);
    clock.advance(adoc_engine::HISTORY_SWEEP_INTERVAL);
    save_as_text(&mut engine, id, "fourth\n");
    assert_eq!(versions_in(&other), [latest]);
}

#[test]
fn a_renamed_page_keeps_its_history() {
    let setup = Setup::new("");
    let mut engine = setup.engine();
    engine
        .open_workspace(vec![setup.pages.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let id = engine.open(&setup.page()).unwrap();
    save_as_text(&mut engine, id, "second\n");
    let renamed = adoc_engine::normalize_path(&setup.pages.path().join("renamed.adoc"));
    let Ok(CommandOutcome::Preview(preview)) = engine.invoke(
        ids::PAGE_RENAME,
        CommandArgs::Rename {
            id,
            new_path: renamed.clone(),
        },
    ) else {
        panic!("no rename preview");
    };
    engine
        .invoke(
            ids::WORKSPACE_APPLY_PREVIEW,
            CommandArgs::Custom(serde_json::json!({ "preview_id": preview.id.get() })),
        )
        .unwrap();

    assert!(engine.local_history(&setup.page()).is_empty());
    let versions = history(&mut engine, id);
    assert_eq!(versions.len(), 2);
    assert_eq!(
        engine.saved_version(&renamed, &versions[1].id).unwrap(),
        "first\n"
    );
    save_as_text(&mut engine, id, "third\n");
    assert_eq!(engine.local_history(&renamed).len(), 3);
    // The folder names the page at its new path.
    let folders: Vec<_> = fs::read_dir(setup.store()).unwrap().flatten().collect();
    assert_eq!(folders.len(), 1);
    assert_eq!(
        fs::read_to_string(folders[0].path().join("path")).unwrap(),
        renamed.display().to_string()
    );
}
