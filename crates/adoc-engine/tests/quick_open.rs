//! Quick Open: finding a workspace's files by name, title or Antora resource id.

use std::{fs, path::Path, time::Duration};

use adoc_core::probe::{self, Probe};

use adoc_engine::{commands::ids, CommandArgs, CommandOutcome, Engine, QuickOpenItem, Settings};

fn open_workspace(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn paths(items: &[QuickOpenItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.relative_path.clone())
        .collect()
}

#[test]
fn quick_open_finds_a_page_by_name_title_or_resource_id() {
    let engine = open_workspace(&fixture("antora-multi-component"));

    let by_name = engine.quick_open("guide", 50);
    let guide = &by_name[0];
    assert_eq!(guide.relative_path, "alpha/modules/ROOT/pages/guide.adoc");
    assert_eq!(guide.title.as_deref(), Some("Guide"));
    assert_eq!(
        guide.resource_id.as_deref(),
        Some("1.0@alpha:ROOT:guide.adoc")
    );

    // The title, not the file name, says "API".
    let by_title = engine.quick_open("api reference", 50);
    assert_eq!(
        by_title[0].relative_path,
        "beta/modules/api/pages/reference.adoc"
    );
    assert_eq!(
        by_title[0].resource_id.as_deref(),
        Some("beta:api:reference.adoc")
    );

    // A component and a module, as in a resource id.
    let by_id = engine.quick_open("beta:api", 50);
    assert_eq!(
        by_id[0].relative_path,
        "beta/modules/api/pages/reference.adoc"
    );

    // A navigation file is listed too, with no resource id.
    let nav = engine.quick_open("nav", 50);
    assert_eq!(nav[0].relative_path, "alpha/modules/ROOT/nav.adoc");
    assert_eq!(nav[0].resource_id, None);
}

#[test]
fn quick_open_matches_letters_in_order_ignoring_case() {
    let engine = open_workspace(&fixture("antora-multi-component"));

    assert_eq!(
        engine.quick_open("GDE", 50)[0].relative_path,
        "alpha/modules/ROOT/pages/guide.adoc"
    );
    assert!(engine.quick_open("zzqx", 50).is_empty());
    // Letters out of order do not match.
    assert!(paths(&engine.quick_open("edig", 50))
        .iter()
        .all(|path| !path.ends_with("guide.adoc")));
    // Odd input is harmless.
    for query in ["é", "$", "::", "@", "  ", "a\u{301}"] {
        let _ = engine.quick_open(query, 50);
    }
}

#[test]
fn quick_open_prefers_the_file_name_over_the_folders() {
    let directory = tempfile::tempdir().unwrap();
    let setup = directory.path().join("setup");
    fs::create_dir(&setup).unwrap();
    fs::write(setup.join("other.adoc"), "= Other\n").unwrap();
    fs::write(directory.path().join("setup.adoc"), "= Setting Up\n").unwrap();
    fs::write(directory.path().join("sea-tuple.adoc"), "= Sea\n").unwrap();
    let engine = open_workspace(directory.path());

    let setup_matches = paths(&engine.quick_open("setup", 50));
    assert_eq!(setup_matches[0], "setup.adoc");
    assert!(setup_matches.contains(&"setup/other.adoc".to_owned()));
    // Letters at the starts of words beat letters scattered through one.
    assert_eq!(
        engine.quick_open("st", 50)[0].relative_path,
        "sea-tuple.adoc"
    );
}

#[test]
fn recently_opened_files_come_first() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["alpha", "beta", "gamma", "delta"] {
        fs::write(
            directory.path().join(format!("{name}.adoc")),
            format!("= {name}\n"),
        )
        .unwrap();
    }
    let mut engine = open_workspace(directory.path());

    // Before anything is opened, by path.
    assert_eq!(
        paths(&engine.quick_open("", 50)),
        ["alpha.adoc", "beta.adoc", "delta.adoc", "gamma.adoc"]
    );

    engine.open(&directory.path().join("gamma.adoc")).unwrap();
    engine.open(&directory.path().join("beta.adoc")).unwrap();
    assert_eq!(
        paths(&engine.quick_open("", 50)),
        ["beta.adoc", "gamma.adoc", "alpha.adoc", "delta.adoc"]
    );
    // Among the matches too: `a` is in every name.
    assert_eq!(
        paths(&engine.quick_open("a", 50)),
        ["beta.adoc", "gamma.adoc", "alpha.adoc", "delta.adoc"]
    );
    // Switching tabs counts as opening.
    let gamma = engine
        .document_for_path(&directory.path().join("gamma.adoc"))
        .unwrap();
    engine.set_active(gamma);
    assert_eq!(engine.quick_open("", 50)[0].relative_path, "gamma.adoc");

    // The limit applies after ranking.
    assert_eq!(paths(&engine.quick_open("", 1)), ["gamma.adoc"]);
}

#[test]
fn quick_open_command_lists_the_matches() {
    let mut engine = open_workspace(&fixture("antora-multi-component"));
    let outcome = engine
        .invoke(
            ids::WORKSPACE_QUICK_OPEN,
            CommandArgs::Query {
                query: "guide".to_owned(),
                document: None,
            },
        )
        .unwrap();
    let CommandOutcome::QuickOpen(items) = outcome else {
        panic!("expected matches, got {outcome:?}");
    };
    assert_eq!(
        items[0].relative_path,
        "alpha/modules/ROOT/pages/guide.adoc"
    );
    assert!(items[0].path.is_absolute());

    let spec = engine
        .commands()
        .get(ids::WORKSPACE_QUICK_OPEN)
        .unwrap()
        .spec();
    assert_eq!(spec.default_binding, Some("cmd-p"));
    assert!(engine
        .invoke(ids::WORKSPACE_QUICK_OPEN, CommandArgs::None)
        .is_err());
}

#[test]
fn quick_open_finds_a_page_among_thousands() {
    let directory = tempfile::tempdir().unwrap();
    for module in 0..20 {
        let pages = directory.path().join(format!("module-{module}"));
        fs::create_dir(&pages).unwrap();
        for page in 0..150 {
            fs::write(
                pages.join(format!("topic-{page}.adoc")),
                format!("= Topic {module}.{page}\n"),
            )
            .unwrap();
        }
    }
    fs::write(
        directory.path().join("module-7/installing.adoc"),
        "= Installing the Widget Server\n",
    )
    .unwrap();
    let engine = open_workspace(directory.path());

    assert_eq!(
        engine.quick_open("instal", 50)[0].relative_path,
        "module-7/installing.adoc"
    );
    assert_eq!(
        engine.quick_open("widget serv", 50)[0].relative_path,
        "module-7/installing.adoc"
    );
    assert_eq!(engine.quick_open("", 100).len(), 100);
}

/// A component of `pages` pages below a fresh folder.
fn component(pages: usize) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("antora.yml"),
        "name: docs\nversion: '1.0'\n",
    )
    .unwrap();
    let folder = directory.path().join("modules/ROOT/pages");
    fs::create_dir_all(&folder).unwrap();
    for page in 0..pages {
        fs::write(
            folder.join(format!("topic-{page}.adoc")),
            format!("= Topic {page}\n"),
        )
        .unwrap();
    }
    directory
}

fn page(root: &Path, name: &str) -> std::path::PathBuf {
    adoc_engine::normalize_path(&root.join("modules/ROOT/pages").join(name))
}

fn built() -> u64 {
    probe::count(Probe::QuickOpenItem)
}

/// Pumps until `done` holds, as the watcher reports changes on disk.
fn pump_until(engine: &mut Engine, mut done: impl FnMut(&Engine) -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        engine.pump();
        if done(engine) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn a_keystroke_makes_no_items_and_an_edit_remakes_none() {
    let root = component(200);
    let mut engine = open_workspace(root.path());
    // The index worker made them, on its own thread.
    probe::reset();
    for query in ["", "t", "to", "top", "topic 12"] {
        assert!(!engine.quick_open(query, 50).is_empty());
    }
    assert_eq!(built(), 0, "each keystroke reuses the items");

    // Retitling an open page keeps every item, the page's with its new title.
    let id = engine.open(&page(root.path(), "topic-12.adoc")).unwrap();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(2, 2),
                text: "Retitled ".to_owned(),
            },
        )
        .unwrap();
    let CommandOutcome::QuickOpen(items) = engine
        .invoke(
            ids::WORKSPACE_QUICK_OPEN,
            CommandArgs::Query {
                query: "retitled".to_owned(),
                document: None,
            },
        )
        .unwrap()
    else {
        panic!("expected matches");
    };
    assert_eq!(items[0].title.as_deref(), Some("Retitled Topic 12"));
    assert_eq!(
        items[0].resource_id.as_deref(),
        Some("1.0@docs:ROOT:topic-12.adoc")
    );
    assert_eq!(built(), 0);

    // A page opened from outside the index is the one item made.
    let added = page(root.path(), "added.adoc");
    fs::write(&added, "= Added Later\n").unwrap();
    engine.open(&added).unwrap();
    assert_eq!(
        paths(&engine.quick_open("added later", 5)),
        ["modules/ROOT/pages/added.adoc"]
    );
    assert_eq!(built(), 1);
}

#[test]
fn quick_open_follows_pages_and_versions_changed_on_disk() {
    let root = component(3);
    let mut engine = open_workspace(root.path());
    assert_eq!(engine.quick_open("topic", 10).len(), 3);

    fs::remove_file(page(root.path(), "topic-1.adoc")).unwrap();
    fs::write(page(root.path(), "fresh.adoc"), "= Fresh Page\n").unwrap();
    assert!(pump_until(&mut engine, |engine| {
        let listed = paths(&engine.quick_open("", 10));
        listed.contains(&"modules/ROOT/pages/fresh.adoc".to_owned())
            && !listed.contains(&"modules/ROOT/pages/topic-1.adoc".to_owned())
    }));

    fs::write(
        root.path().join("antora.yml"),
        "name: docs\nversion: '2.0'\n",
    )
    .unwrap();
    assert!(pump_until(&mut engine, |engine| {
        engine.quick_open("topic-0", 1)[0].resource_id.as_deref()
            == Some("2.0@docs:ROOT:topic-0.adoc")
    }));
}

#[test]
fn a_quick_open_keystroke_rebuilds_no_visual_model() {
    let root = component(3);
    let mut engine = open_workspace(root.path());
    let id = engine.open(&page(root.path(), "topic-0.adoc")).unwrap();
    engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
        .unwrap();
    // Source typing leaves the model stale until it pauses.
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(7, 7),
                text: "\nTyped.\n".to_owned(),
            },
        )
        .unwrap();
    let events = engine.subscribe();
    engine
        .invoke(
            ids::WORKSPACE_QUICK_OPEN,
            CommandArgs::Query {
                query: "topic".to_owned(),
                document: None,
            },
        )
        .unwrap();
    assert!(!events
        .try_iter()
        .any(|event| matches!(event, adoc_engine::EngineEvent::ModelChanged { .. })));
}
