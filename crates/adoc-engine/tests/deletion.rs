//! Deleting a file from the app (#100): the file is moved aside into the state folder, its tab
//! closed and the workspace told; Undo Delete puts back the latest deletion, and Restore Deleted
//! File… any deletion still kept, from this session or an earlier one (#434). What removing it
//! would break is Inspect Impact's to show first.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use adoc_core::DiagnosticCode;
use adoc_engine::{
    commands::ids,
    impact::{ImpactKind, ImpactReport},
    normalize_path, CommandArgs, CommandOutcome, Engine, EngineError, EngineEvent, InspectionState,
    KeptDeletion, Settings, DELETED_DIR,
};

/// A component whose partial is included by three pages, with the app's state folder.
struct Setup {
    _directory: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

impl Setup {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        let module = root.join("modules/ROOT");
        for (path, text) in [
            (
                "antora.yml",
                "name: product\nversion: '1.0'\nnav:\n  - modules/ROOT/nav.adoc\n",
            ),
            (
                "modules/ROOT/nav.adoc",
                "* xref:index.adoc[]\n* xref:install.adoc[]\n",
            ),
            (
                "modules/ROOT/pages/index.adoc",
                "= Product\n\ninclude::partial$shared.adoc[]\n\nSee xref:install.adoc[].\n",
            ),
            (
                "modules/ROOT/pages/install.adoc",
                "= Installing\n\ninclude::partial$shared.adoc[]\n",
            ),
            (
                "modules/ROOT/pages/upgrade.adoc",
                "= Upgrading\n\ninclude::partial$shared.adoc[]\n",
            ),
            ("modules/ROOT/partials/shared.adoc", "Shared words.\n"),
            ("modules/ROOT/images/logo.png", "not really a picture"),
        ] {
            let file = root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }
        assert!(module.is_dir());
        let state = directory.path().join("state");
        Self {
            _directory: directory,
            root,
            state,
        }
    }

    fn engine(&self) -> Engine {
        let settings = Settings::default()
            .with_git_opt_in()
            .with_state_dir(Some(self.state.clone()));
        let mut engine = Engine::new(settings);
        engine.open_workspace(vec![self.root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        engine
    }

    fn file(&self, path: &str) -> PathBuf {
        self.root.join("modules/ROOT").join(path)
    }
}

fn codes_in(engine: &Engine, path: &Path) -> Vec<DiagnosticCode> {
    engine
        .workspace_diagnostics()
        .into_iter()
        .filter(|(file, _)| file == path)
        .flat_map(|(_, diagnostics)| diagnostics)
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn events(receiver: &std::sync::mpsc::Receiver<EngineEvent>) -> Vec<EngineEvent> {
    receiver.try_iter().collect()
}

#[test]
fn deleting_a_file_moves_it_aside_and_undo_delete_puts_it_back() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let install = setup.file("pages/install.adoc");
    let index = setup.file("pages/index.adoc");
    let text = fs::read_to_string(&install).unwrap();
    let id = engine.open(&install).unwrap();
    assert!(engine.command_available(ids::FILE_DELETE));
    assert!(!engine.command_available(ids::FILE_UNDO_DELETE));
    let receiver = engine.subscribe();

    // With nothing named, the active tab's file.
    engine.invoke(ids::FILE_DELETE, CommandArgs::None).unwrap();
    assert!(!install.exists());
    assert!(engine.document(id).is_none(), "its tab is closed");
    assert!(events(&receiver).contains(&EngineEvent::FileDeleted(install.clone())));
    assert!(
        codes_in(&engine, &index).contains(&DiagnosticCode::UnresolvedXrefFile),
        "{:?}",
        codes_in(&engine, &index)
    );
    // Kept in the state folder, not lost.
    let kept: Vec<PathBuf> = fs::read_dir(setup.state.join(DELETED_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(engine.last_deleted(), Some(install.as_path()));
    assert!(engine.command_available(ids::FILE_UNDO_DELETE));

    engine
        .invoke(ids::FILE_UNDO_DELETE, CommandArgs::None)
        .unwrap();
    assert_eq!(fs::read_to_string(&install).unwrap(), text);
    assert!(events(&receiver).contains(&EngineEvent::FileRestored(install.clone())));
    // Open again, as it was when deleted, and resolving again.
    let reopened = engine.document_for_path(&install).expect("reopened");
    assert_eq!(engine.active(), Some(reopened));
    assert!(!codes_in(&engine, &index).contains(&DiagnosticCode::UnresolvedXrefFile));
    assert!(fs::read_dir(setup.state.join(DELETED_DIR))
        .unwrap()
        .next()
        .is_none());
    assert_eq!(engine.last_deleted(), None);
    assert!(!engine.command_available(ids::FILE_UNDO_DELETE));
}

/// #100's "Done when": a partial included by three pages lists them before it is deleted, and
/// deleting it leaves their includes naming nothing.
#[test]
fn deleting_a_partial_included_by_three_pages_shows_them_first() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let shared = setup.file("partials/shared.adoc");
    engine
        .invoke(ids::IMPACT_INSPECT, CommandArgs::Path(shared.clone()))
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    let InspectionState::Ready(report) = &engine.impact_inspection().unwrap().state else {
        panic!("{:?}", engine.impact_inspection());
    };
    let report: &ImpactReport = report;
    let mut includers: Vec<&str> = report
        .impacts
        .iter()
        .filter(|impact| impact.kind == ImpactKind::Render && impact.via.len() > 1)
        .map(|impact| impact.path.as_str())
        .collect();
    includers.sort_unstable();
    assert_eq!(
        includers,
        [
            "modules/ROOT/pages/index.adoc",
            "modules/ROOT/pages/install.adoc",
            "modules/ROOT/pages/upgrade.adoc",
        ]
    );

    let pages =
        ["index", "install", "upgrade"].map(|page| setup.file(&format!("pages/{page}.adoc")));
    for page in &pages {
        assert_eq!(codes_in(&engine, page), [], "{}", page.display());
    }
    engine
        .invoke(ids::FILE_DELETE, CommandArgs::Path(shared.clone()))
        .unwrap();
    assert!(!shared.exists());
    // `partial$shared.adoc` names a resource the catalog no longer has.
    for page in pages {
        assert!(
            codes_in(&engine, &page).contains(&DiagnosticCode::AntoraUnknownResource),
            "{}: {:?}",
            page.display(),
            codes_in(&engine, &page)
        );
    }
    // The kept analysis is out of date now.
    assert!(engine.impact_stale());
}

#[test]
fn an_open_image_closes_and_unsaved_edits_go_with_the_page() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let logo = setup.file("images/logo.png");
    let image = engine
        .invoke(ids::DOCUMENT_OPEN, CommandArgs::Path(logo.clone()))
        .unwrap();
    let image = match image {
        adoc_engine::CommandOutcome::OpenedImage(id) => id,
        other => panic!("{other:?}"),
    };
    engine
        .invoke(ids::FILE_DELETE, CommandArgs::Image(image))
        .unwrap();
    assert!(!logo.exists());
    assert!(engine.image(image).is_none());
    engine
        .invoke(ids::FILE_UNDO_DELETE, CommandArgs::None)
        .unwrap();
    assert!(logo.is_file());
    assert!(engine.image_for_path(&logo).is_some(), "open again");

    // An edited page is deleted as it is on disk; the edits are discarded with its tab.
    let upgrade = setup.file("pages/upgrade.adoc");
    let on_disk = fs::read_to_string(&upgrade).unwrap();
    let id = engine.open(&upgrade).unwrap();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(0, 0),
                text: "unsaved ".to_owned(),
            },
        )
        .unwrap();
    engine
        .invoke(ids::FILE_DELETE, CommandArgs::Document(id))
        .unwrap();
    engine
        .invoke(ids::FILE_UNDO_DELETE, CommandArgs::None)
        .unwrap();
    assert_eq!(fs::read_to_string(&upgrade).unwrap(), on_disk);
}

#[test]
fn deletions_are_undone_latest_first_and_never_over_a_new_file() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let install = setup.file("pages/install.adoc");
    let upgrade = setup.file("pages/upgrade.adoc");
    for file in [&install, &upgrade] {
        engine
            .invoke(ids::FILE_DELETE, CommandArgs::Path(file.clone()))
            .unwrap();
    }
    assert_eq!(engine.last_deleted(), Some(upgrade.as_path()));
    // A file made since at the same path is never replaced, and the deletion stays to undo.
    fs::write(&upgrade, "= New\n").unwrap();
    assert!(engine
        .invoke(ids::FILE_UNDO_DELETE, CommandArgs::None)
        .is_err());
    assert_eq!(fs::read_to_string(&upgrade).unwrap(), "= New\n");
    assert_eq!(engine.last_deleted(), Some(upgrade.as_path()));
    fs::remove_file(&upgrade).unwrap();
    engine
        .invoke(ids::FILE_UNDO_DELETE, CommandArgs::None)
        .unwrap();
    assert!(upgrade.is_file());
    assert_eq!(engine.last_deleted(), Some(install.as_path()));
    engine
        .invoke(ids::FILE_UNDO_DELETE, CommandArgs::None)
        .unwrap();
    assert!(install.is_file());
    // Not open before, so not opened now.
    assert!(engine.document_for_path(&install).is_none());
}

#[test]
fn deleting_refuses_folders_links_and_paths_outside_the_roots() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let refused = |engine: &mut Engine, path: PathBuf| {
        assert!(
            engine
                .invoke(ids::FILE_DELETE, CommandArgs::Path(path.clone()))
                .is_err(),
            "{}",
            path.display()
        );
    };
    refused(&mut engine, setup.file("pages"));
    refused(&mut engine, setup.file("pages/missing.adoc"));
    let outside = setup.state.parent().unwrap().join("outside.adoc");
    fs::write(&outside, "= Outside\n").unwrap();
    refused(&mut engine, outside.clone());
    assert!(outside.is_file());
    #[cfg(unix)]
    {
        let link = setup.file("pages/link.adoc");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        refused(&mut engine, link.clone());
        assert!(link.exists());
        let folder = setup.root.join("linked");
        std::os::unix::fs::symlink(setup.file("pages"), &folder).unwrap();
        refused(&mut engine, folder.join("index.adoc"));
        assert!(setup.file("pages/index.adoc").is_file());
    }
}

#[test]
fn without_a_state_folder_nothing_is_deleted() {
    let setup = Setup::new();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.open_workspace(vec![setup.root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    let install = setup.file("pages/install.adoc");
    engine.open(&install).unwrap();
    assert!(!engine.command_available(ids::FILE_DELETE));
    assert!(engine.invoke(ids::FILE_DELETE, CommandArgs::None).is_err());
    assert!(install.is_file());
}

#[test]
fn deletions_older_than_the_history_keeps_are_swept() {
    let setup = Setup::new();
    let old = setup.state.join(DELETED_DIR).join("1000-0");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("path"), "/elsewhere/page.adoc").unwrap();
    fs::write(old.join("file"), "= Old\n").unwrap();
    let mut engine = setup.engine();
    engine
        .invoke(
            ids::FILE_DELETE,
            CommandArgs::Path(setup.file("pages/upgrade.adoc")),
        )
        .unwrap();
    assert!(!old.exists());
    // An earlier session's deletion is not this one's to undo.
    engine
        .invoke(ids::FILE_UNDO_DELETE, CommandArgs::None)
        .unwrap();
    assert!(engine.last_deleted().is_none());
}

fn restore(engine: &mut Engine, deletion: &str) -> Result<CommandOutcome, EngineError> {
    engine.invoke(
        ids::FILE_RESTORE_DELETED,
        CommandArgs::Custom(serde_json::json!({ "deletion": deletion })),
    )
}

fn kept_paths(engine: &Engine) -> Vec<PathBuf> {
    engine
        .kept_deletions()
        .into_iter()
        .map(|kept| kept.path)
        .collect()
}

/// #434: a deletion records where the file was, so a later session lists it, newest first, and
/// restores it, opening a page again.
#[test]
fn a_deletion_from_an_earlier_session_is_listed_and_restored() {
    let setup = Setup::new();
    let install = setup.file("pages/install.adoc");
    let upgrade = setup.file("pages/upgrade.adoc");
    let index = setup.file("pages/index.adoc");
    let text = fs::read_to_string(&install).unwrap();
    let before = SystemTime::now() - Duration::from_secs(1);
    {
        let mut engine = setup.engine();
        for file in [&install, &upgrade] {
            engine
                .invoke(ids::FILE_DELETE, CommandArgs::Path(file.clone()))
                .unwrap();
        }
    }
    // The original path is recorded beside the file kept.
    let folders: Vec<PathBuf> = fs::read_dir(setup.state.join(DELETED_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(folders.len(), 2);
    for folder in &folders {
        let recorded = PathBuf::from(fs::read_to_string(folder.join("path")).unwrap());
        assert!(recorded == install || recorded == upgrade, "{recorded:?}");
        assert!(folder.join("file").is_file());
    }

    let mut engine = setup.engine();
    assert!(
        engine.last_deleted().is_none(),
        "not this session's to undo"
    );
    assert!(engine.command_available(ids::FILE_RESTORE_DELETED));
    let kept: Vec<KeptDeletion> = engine.kept_deletions();
    assert_eq!(kept_paths(&engine), [upgrade.clone(), install.clone()]);
    assert!(kept.iter().all(|kept| !kept.occupied));
    assert!(kept
        .iter()
        .all(|kept| kept.deleted >= before && kept.deleted <= SystemTime::now()));
    assert!(codes_in(&engine, &index).contains(&DiagnosticCode::UnresolvedXrefFile));

    let receiver = engine.subscribe();
    let outcome = restore(&mut engine, &kept[1].id).unwrap();
    assert_eq!(fs::read_to_string(&install).unwrap(), text);
    let opened = engine.document_for_path(&install).expect("a page opens");
    assert_eq!(outcome, CommandOutcome::Opened(opened));
    assert_eq!(engine.active(), Some(opened));
    assert!(events(&receiver).contains(&EngineEvent::FileRestored(install.clone())));
    assert!(!codes_in(&engine, &index).contains(&DiagnosticCode::UnresolvedXrefFile));
    assert_eq!(kept_paths(&engine), [upgrade], "no longer kept");
    assert_eq!(
        fs::read_dir(setup.state.join(DELETED_DIR)).unwrap().count(),
        1
    );
    // Once restored, it is not there to restore again.
    assert!(restore(&mut engine, &kept[1].id).is_err());
}

/// As Undo Delete, restoring never replaces a file made at the path since; a file whose folder
/// has gone gets its folder back; and a file that is not a page is not opened.
#[test]
fn restoring_refuses_a_file_at_the_path_and_makes_missing_folders() {
    let setup = Setup::new();
    let upgrade = setup.file("pages/upgrade.adoc");
    let logo = setup.file("images/logo.png");
    let mut engine = setup.engine();
    for file in [&upgrade, &logo] {
        engine
            .invoke(ids::FILE_DELETE, CommandArgs::Path(file.clone()))
            .unwrap();
    }
    fs::write(&upgrade, "= New\n").unwrap();
    fs::remove_dir(setup.file("images")).unwrap();
    let kept = engine.kept_deletions();
    assert_eq!(kept_paths(&engine), [logo.clone(), upgrade.clone()]);
    assert!(!kept[0].occupied);
    assert!(kept[1].occupied, "a file is at its path now");

    assert!(matches!(
        restore(&mut engine, &kept[1].id),
        Err(EngineError::Refused(_))
    ));
    assert_eq!(fs::read_to_string(&upgrade).unwrap(), "= New\n");
    assert_eq!(engine.kept_deletions().len(), 2, "still kept");
    assert_eq!(
        engine.last_deleted(),
        Some(logo.as_path()),
        "still undoable"
    );

    assert_eq!(
        restore(&mut engine, &kept[0].id).unwrap(),
        CommandOutcome::Done
    );
    assert!(logo.is_file());
    assert!(engine.image_for_path(&logo).is_none(), "only pages open");
    // Restored by Restore Deleted File…, it is no longer Undo Delete's to put back.
    assert_eq!(engine.last_deleted(), Some(upgrade.as_path()));
}

/// The state folder is read as untrusted: a recorded path that is relative, climbs with `..`,
/// lies outside the workspace or goes through a linked folder is never listed or restored, nor
/// is an entry with no record, one whose kept file or folder is a link, or one named with a
/// separator.
#[test]
fn restoring_refuses_entries_it_cannot_trust() {
    let setup = Setup::new();
    let store = setup.state.join(DELETED_DIR);
    let entry = |name: &str, path: Option<&str>| {
        let folder = store.join(name);
        fs::create_dir_all(&folder).unwrap();
        if let Some(path) = path {
            fs::write(folder.join("path"), path).unwrap();
        }
        fs::write(folder.join("file"), "= Kept\n").unwrap();
        folder
    };
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let pages = setup.file("pages");
    let outside = setup.state.parent().unwrap().join("outside.adoc");
    let climbing = format!("{}/../../outside.adoc", pages.display());
    let mut refused = vec![
        entry(&format!("{millis}-1-1"), Some("pages/relative.adoc")),
        entry(&format!("{millis}-1-2"), Some(&climbing)),
        entry(
            &format!("{millis}-1-3"),
            Some(&outside.display().to_string()),
        ),
        entry(&format!("{millis}-1-4"), None),
    ];
    #[cfg(unix)]
    {
        let linked = setup.root.join("linked");
        std::os::unix::fs::symlink(&pages, &linked).unwrap();
        refused.push(entry(
            &format!("{millis}-1-5"),
            Some(&linked.join("through.adoc").display().to_string()),
        ));
        let folder = entry(
            &format!("{millis}-1-6"),
            Some(&pages.join("linked-file.adoc").display().to_string()),
        );
        fs::remove_file(folder.join("file")).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("file")).unwrap();
        refused.push(folder);
        let real = entry(
            "elsewhere",
            Some(&pages.join("linked-folder.adoc").display().to_string()),
        );
        let link = store.join(format!("{millis}-1-7"));
        std::os::unix::fs::symlink(&real, &link).unwrap();
        refused.push(link);
    }
    fs::write(&outside, "= Outside\n").unwrap();
    let mut engine = setup.engine();
    assert_eq!(engine.kept_deletions(), []);
    for folder in refused {
        let name = folder.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(restore(&mut engine, &name).is_err(), "{name}");
    }
    assert_eq!(fs::read_to_string(&outside).unwrap(), "= Outside\n");
    for name in ["../state", "a/b", "..", ""] {
        assert!(restore(&mut engine, name).is_err(), "{name}");
    }
    assert!(engine
        .invoke(ids::FILE_RESTORE_DELETED, CommandArgs::None)
        .is_err());
    for page in ["relative", "linked-file", "linked-folder", "through"] {
        assert!(
            !setup.file(&format!("pages/{page}.adoc")).exists(),
            "{page}"
        );
    }
}

#[test]
fn without_a_state_folder_nothing_is_kept_to_restore() {
    let setup = Setup::new();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.open_workspace(vec![setup.root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    assert!(!engine.command_available(ids::FILE_RESTORE_DELETED));
    assert_eq!(engine.kept_deletions(), []);
}
