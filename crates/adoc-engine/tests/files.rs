//! The Files view's directory listings: shallow, on request, sorted folders first, with the
//! workspace's exclusions, errors kept apart from empty folders, symbolic links shown but not
//! followed, and results for a workspace no longer open dropped. Opening the files found there:
//! non-AsciiDoc text stays out of the index, and binary files are refused.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use adoc_engine::{
    commands::ids, CommandArgs, DirectoryError, Engine, EngineError, EngineEvent, EntryKind,
    Listing, Settings,
};

/// A mixed repository: documentation, its descriptor and playbook, a theme, a font, a script,
/// dotfiles, an empty folder and an excluded one.
fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (path, text) in [
        ("README.md", "# Readme\n"),
        (".adocfoundry.toml", "[editor]\n"),
        (".gitignore", "build/\n"),
        ("antora-playbook.yml", "site:\n  title: Docs\n"),
        ("docs/antora.yml", "name: docs\nversion: ~\n"),
        ("docs/modules/ROOT/pages/index.adoc", "= Index\n"),
        (
            "resources/themes/company-theme.yml",
            "base:\n  font-size: 10\n",
        ),
        ("scripts/build-docs.sh", "#!/bin/sh\necho built\n"),
        ("file10.txt", "ten\n"),
        ("file2.txt", "two\n"),
        ("File 3 é.txt", "three\n"),
        ("node_modules/left-pad/index.js", "\n"),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::write(root.join("resources/fonts.ttf"), [0u8, 1, 0, 0, 0, 255]).unwrap();
    directory
}

fn engine_for(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::default());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    engine
}

/// Lists `dir` and waits for the listing.
fn list(engine: &mut Engine, dir: &Path) -> Listing {
    engine.request_directory(dir).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    engine.directory(dir).expect("a listing")
}

fn names(listing: &Listing) -> Vec<String> {
    match listing {
        Listing::Loaded(entries) => entries.iter().map(|entry| entry.name.clone()).collect(),
        other => panic!("not loaded: {other:?}"),
    }
}

fn root_of(directory: &tempfile::TempDir) -> PathBuf {
    adoc_engine::normalize_path(directory.path())
}

#[test]
fn a_root_lists_its_children_folders_first_in_natural_order() {
    let directory = repository();
    let root = root_of(&directory);
    let mut engine = engine_for(&root);
    let listing = list(&mut engine, &root);
    assert_eq!(
        names(&listing),
        [
            "docs",
            "empty",
            "resources",
            "scripts",
            ".adocfoundry.toml",
            ".gitignore",
            "antora-playbook.yml",
            "File 3 é.txt",
            "file2.txt",
            "file10.txt",
            "README.md",
        ],
        "dotfiles show; node_modules, in `workspace.ignore`, does not"
    );
    let Listing::Loaded(entries) = &listing else {
        unreachable!()
    };
    let docs = &entries[0];
    assert_eq!(docs.kind, EntryKind::Directory);
    assert_eq!(docs.path, root.join("docs"));
    assert!(!docs.excluded);
    assert_eq!(
        entries[7].path,
        root.join("File 3 é.txt"),
        "names survive as they are"
    );
}

#[test]
fn excluded_entries_show_on_request_and_say_so() {
    let directory = repository();
    let root = root_of(&directory);
    let mut engine = engine_for(&root);
    list(&mut engine, &root);
    engine.set_show_excluded(true);
    let Listing::Loaded(entries) = engine.directory(&root).unwrap() else {
        panic!("loaded")
    };
    let modules = entries
        .iter()
        .find(|entry| entry.name == "node_modules")
        .expect("shown");
    assert!(modules.excluded);
    assert_eq!(modules.kind, EntryKind::Directory);
    engine.set_show_excluded(false);
    assert!(!names(&engine.directory(&root).unwrap()).contains(&"node_modules".into()));
}

#[test]
fn only_requested_folders_are_listed() {
    let directory = repository();
    let root = root_of(&directory);
    let mut engine = engine_for(&root);
    list(&mut engine, &root);
    assert!(engine.directory(&root.join("docs")).is_none());
    assert!(engine.directory(&root.join("docs/modules")).is_none());
    assert_eq!(
        names(&list(&mut engine, &root.join("docs"))),
        ["modules", "antora.yml"]
    );
    assert!(engine.directory(&root.join("docs/modules")).is_none());
    assert_eq!(
        names(&list(&mut engine, &root.join("empty"))),
        Vec::<String>::new()
    );
}

#[test]
fn folders_outside_the_roots_are_refused() {
    let directory = repository();
    let root = root_of(&directory);
    let mut engine = engine_for(&root.join("docs"));
    assert!(matches!(
        engine.request_directory(&root),
        Err(EngineError::Refused(_))
    ));
    assert!(matches!(
        engine.request_directory(&root.join("docs/../resources")),
        Err(EngineError::Refused(_))
    ));
}

#[test]
fn a_folder_that_cannot_be_read_is_not_empty() {
    let directory = repository();
    let root = root_of(&directory);
    let mut engine = engine_for(&root);
    let gone = list(&mut engine, &root.join("vanished"));
    assert!(
        matches!(gone, Listing::Failed(DirectoryError::Unavailable(_))),
        "{gone:?}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = root.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let listing = list(&mut engine, &locked);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // Root can read anything; elsewhere the folder is refused.
        if fs::read_dir("/root").is_err() {
            assert_eq!(listing, Listing::Failed(DirectoryError::PermissionDenied));
        }
        // Retrying once it can be read lists it.
        engine.refresh_directory(&locked).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        assert_eq!(engine.directory(&locked), Some(Listing::Loaded(Vec::new())));
    }
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_shown_but_not_followed() {
    let directory = repository();
    let root = root_of(&directory);
    std::os::unix::fs::symlink(&root, root.join("docs/loop")).unwrap();
    std::os::unix::fs::symlink(root.join("README.md"), root.join("docs/readme-link")).unwrap();
    let mut engine = engine_for(&root);
    let Listing::Loaded(entries) = list(&mut engine, &root.join("docs")) else {
        panic!("loaded")
    };
    let kinds: Vec<(&str, &EntryKind)> = entries
        .iter()
        .map(|entry| (entry.name.as_str(), &entry.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("loop", &EntryKind::Link { to_directory: true }),
            ("modules", &EntryKind::Directory),
            ("antora.yml", &EntryKind::File),
            (
                "readme-link",
                &EntryKind::Link {
                    to_directory: false
                }
            ),
        ]
    );
    // A link to a folder is not opened as one.
    assert!(matches!(
        engine.request_directory(&root.join("docs/loop")),
        Err(EngineError::Refused(_))
    ));
}

#[test]
fn a_listing_for_a_workspace_no_longer_open_is_dropped() {
    let first = repository();
    let second = repository();
    let mut engine = engine_for(&root_of(&first));
    engine.request_directory(&root_of(&first)).unwrap();
    // Opened before the listing comes back.
    engine.open_workspace(vec![root_of(&second)]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    assert!(engine.directory(&root_of(&first)).is_none());
}

#[test]
fn changes_on_disk_update_the_folders_listed() {
    let directory = repository();
    let root = root_of(&directory);
    let mut engine = engine_for(&root);
    let scripts = root.join("scripts");
    list(&mut engine, &scripts);
    let events = engine.subscribe();
    fs::write(scripts.join("deploy.sh"), "#!/bin/sh\n").unwrap();
    fs::create_dir(scripts.join("tools")).unwrap();
    fs::remove_file(scripts.join("build-docs.sh")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        engine.pump();
        engine.wait_for_workers(Duration::from_secs(5));
        let listed = engine.directory(&scripts).map(|listing| names(&listing));
        if listed.as_deref() == Some(&["tools".to_owned(), "deploy.sh".to_owned()][..]) {
            break;
        }
        assert!(Instant::now() < deadline, "still {listed:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::DirectoryListed(scripts.clone())));
}

#[test]
fn text_files_open_outside_the_index_and_binary_ones_are_refused() {
    let directory = repository();
    let root = root_of(&directory);
    let mut engine = engine_for(&root);
    let theme = root.join("resources/themes/company-theme.yml");
    let id = engine.open(&theme).unwrap();
    let document = engine.document(id).unwrap();
    assert!(!document.asciidoc);
    assert!(document.diagnostics.is_empty());
    assert!(
        engine.workspace().index.file(&theme).is_none(),
        "not indexed"
    );
    assert!(engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
        .is_err());
    // The rendered preview has nothing to show for it.
    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.preview_target(), None);
    // Typing in it indexes nothing either.
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(0, 0),
                text: "= Not a title\n".to_owned(),
            },
        )
        .unwrap();
    assert!(engine.workspace().index.file(&theme).is_none());
    assert!(engine.document(id).unwrap().diagnostics.is_empty());

    // An AsciiDoc page is a page.
    let page = engine
        .open(&root.join("docs/modules/ROOT/pages/index.adoc"))
        .unwrap();
    assert!(engine.document(page).unwrap().asciidoc);

    // Binary content and text that is not UTF-8 are refused, and nothing opens.
    let before = engine.documents().count();
    fs::write(root.join("latin1.txt"), [b'c', b'a', b'f', 0xe9]).unwrap();
    for refused in [root.join("resources/fonts.ttf"), root.join("latin1.txt")] {
        let error = engine.open(&refused).unwrap_err();
        assert!(
            error.to_string().contains("not a text file") || error.to_string().contains("UTF-8"),
            "{error}"
        );
    }
    assert_eq!(engine.documents().count(), before);
}

#[test]
fn a_large_tree_lists_only_what_is_opened() {
    let directory = tempfile::tempdir().unwrap();
    let root = root_of(&directory);
    for folder in 0..50 {
        let dir = root.join(format!("section-{folder}"));
        fs::create_dir_all(dir.join("deeper")).unwrap();
        for file in 0..100 {
            fs::write(dir.join(format!("asset-{file}.json")), "{}\n").unwrap();
        }
    }
    let mut engine = engine_for(&root);
    let started = Instant::now();
    let listing = list(&mut engine, &root);
    // Generous: one folder's names, however loaded the machine.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(names(&listing).len(), 50);
    assert_eq!(
        names(&listing)[..3],
        ["section-0", "section-1", "section-2"]
    );
    assert_eq!(names(&listing)[49], "section-49", "natural order");
    // Nothing beneath the root was listed.
    for folder in 0..50 {
        assert!(engine
            .directory(&root.join(format!("section-{folder}")))
            .is_none());
    }
    // Opening one lists its hundred files and its folder, and still nothing deeper.
    let section = list(&mut engine, &root.join("section-7"));
    assert_eq!(names(&section).len(), 101);
    assert!(engine.directory(&root.join("section-7/deeper")).is_none());
}
