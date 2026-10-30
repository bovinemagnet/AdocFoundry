//! New Page: a page made in a module, opened, and added to the module's navigation.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, CommandOutcome, Engine, NavItem, NewPage, Settings,
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

/// A copy of the multi-component fixture: `alpha` 1.0 has a navigation file, `beta` none.
struct Setup {
    _directory: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
}

impl Setup {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        copy_tree(&fixture("antora-multi-component"), &root);
        let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        Self {
            _directory: directory,
            root,
            engine,
        }
    }

    fn alpha(&self) -> PathBuf {
        self.root.join("alpha/modules/ROOT")
    }

    fn nav(&self) -> PathBuf {
        self.alpha().join("nav.adoc")
    }

    fn new_page(
        &mut self,
        module: PathBuf,
        title: &str,
        add_to_nav: bool,
    ) -> Result<CommandOutcome, adoc_engine::EngineError> {
        self.engine.invoke(
            ids::PAGE_NEW,
            CommandArgs::NewPage(NewPage {
                module,
                title: title.to_owned(),
                add_to_nav,
            }),
        )
    }

    fn text(&self, path: &Path) -> String {
        let id = self.engine.document_for_path(path).unwrap();
        self.engine.document(id).unwrap().text.to_string()
    }
}

fn targets(items: &[NavItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|item| {
            item.target
                .iter()
                .map(|target| target.path.clone())
                .chain(targets(&item.children))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn a_new_page_resolves_at_once_and_appears_in_the_navigation() {
    let mut setup = Setup::new();
    let guide = setup.alpha().join("pages/guide.adoc");
    setup.engine.open(&guide).unwrap();

    let outcome = setup
        .new_page(setup.alpha(), "Release Notes", true)
        .unwrap();
    let page = setup.alpha().join("pages/release-notes.adoc");
    let CommandOutcome::Opened(id) = outcome else {
        panic!("expected the page opened, got {outcome:?}");
    };
    assert_eq!(fs::read_to_string(&page).unwrap(), "= Release Notes\n");
    assert_eq!(setup.engine.document(id).unwrap().path, page);
    assert_eq!(setup.engine.active(), Some(id));
    assert!(!setup.engine.document(id).unwrap().dirty);

    // After the guide's entry and the entries beneath it, at the guide's level.
    assert_eq!(
        setup.text(&setup.nav()),
        ".Alpha\n\
         * xref:index.adoc[Home]\n\
         ** xref:guide.adoc[Guide]\n\
         *** xref:guide.adoc#setup[Setup]\n\
         ** xref:release-notes.adoc[]\n\
         ** Background\n\
         * xref:beta::index.adoc[Beta home]\n\
         * xref:missing.adoc[Missing page]\n"
    );

    // The entry resolves: no diagnostic on its line, and it leads to the page.
    let nav = setup.engine.document_for_path(&setup.nav()).unwrap();
    let nav_text = setup.text(&setup.nav());
    let line = nav_text.find("xref:release-notes").unwrap();
    let diagnostics = &setup.engine.document(nav).unwrap().diagnostics;
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| !(diagnostic.range.start..=diagnostic.range.end).contains(&line)),
        "{diagnostics:?}"
    );
    assert!(setup.engine.definition(nav, line + 6).is_some());

    // The Navigation tab is built from these trees.
    let trees = setup.engine.nav_trees("alpha", Some("1.0")).unwrap();
    assert!(targets(&trees[0].items).contains(&"release-notes.adoc".to_owned()));
}

#[test]
fn one_undo_takes_back_the_page_and_its_entry_and_redo_brings_them_back() {
    let mut setup = Setup::new();
    let guide = setup.alpha().join("pages/guide.adoc");
    let guide_id = setup.engine.open(&guide).unwrap();
    let original_nav = fs::read_to_string(setup.nav()).unwrap();
    setup
        .new_page(setup.alpha(), "Release Notes", true)
        .unwrap();
    let page = setup.alpha().join("pages/release-notes.adoc");
    let nav = setup.engine.document_for_path(&setup.nav()).unwrap();

    setup
        .engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(nav))
        .unwrap();
    assert!(!page.exists());
    assert_eq!(setup.engine.document_for_path(&page), None);
    assert_eq!(setup.text(&setup.nav()), original_nav);
    assert!(setup.engine.document(guide_id).is_some());

    setup
        .engine
        .invoke(ids::EDIT_REDO, CommandArgs::Document(nav))
        .unwrap();
    assert_eq!(fs::read_to_string(&page).unwrap(), "= Release Notes\n");
    assert!(setup.engine.document_for_path(&page).is_some());
    assert!(setup
        .text(&setup.nav())
        .contains("** xref:release-notes.adoc[]"));
    let trees = setup.engine.nav_trees("alpha", Some("1.0")).unwrap();
    assert!(targets(&trees[0].items).contains(&"release-notes.adoc".to_owned()));
}

#[test]
fn undo_in_the_new_page_takes_it_back_too() {
    let mut setup = Setup::new();
    let CommandOutcome::Opened(id) = setup.new_page(setup.alpha(), "Glossary", false).unwrap()
    else {
        panic!("expected the page opened");
    };
    let page = setup.alpha().join("pages/glossary.adoc");
    assert!(page.exists());
    setup
        .engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert!(!page.exists());
    assert!(setup.engine.document(id).is_none());
}

#[test]
fn undo_keeps_a_page_changed_since() {
    let mut setup = Setup::new();
    let CommandOutcome::Opened(id) = setup.new_page(setup.alpha(), "Glossary", false).unwrap()
    else {
        panic!("expected the page opened");
    };
    let page = setup.alpha().join("pages/glossary.adoc");
    fs::write(&page, "= Glossary\n\nWritten elsewhere.\n").unwrap();
    setup
        .engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(
        fs::read_to_string(&page).unwrap(),
        "= Glossary\n\nWritten elsewhere.\n"
    );
}

#[test]
fn a_page_not_in_the_navigation_is_followed_by_an_entry_at_its_end() {
    let mut setup = Setup::new();
    // The active page is in another component, and alpha's navigation does not list it.
    setup
        .engine
        .open(&setup.root.join("beta/modules/api/pages/reference.adoc"))
        .unwrap();
    setup.new_page(setup.alpha(), "FAQ", true).unwrap();
    assert!(setup
        .text(&setup.nav())
        .ends_with("* xref:missing.adoc[Missing page]\n* xref:faq.adoc[]\n"));
}

#[test]
fn a_title_is_kept_to_one_line() {
    let mut setup = Setup::new();
    setup
        .new_page(setup.alpha(), "  Two\nLines\t here ", false)
        .unwrap();
    assert_eq!(
        fs::read_to_string(setup.alpha().join("pages/two-lines-here.adoc")).unwrap(),
        "= Two Lines here\n"
    );
}

#[test]
fn without_the_navigation_only_the_page_is_made() {
    let mut setup = Setup::new();
    setup.new_page(setup.alpha(), "Glossary", false).unwrap();
    assert!(setup.alpha().join("pages/glossary.adoc").is_file());
    assert_eq!(setup.engine.document_for_path(&setup.nav()), None);
}

#[test]
fn a_new_page_is_refused_without_a_name_a_module_or_a_free_file() {
    let mut setup = Setup::new();
    let beta = setup.root.join("beta/modules/ROOT");
    let before = |setup: &Setup| fs::read_dir(setup.alpha().join("pages")).unwrap().count();
    let count = before(&setup);
    for (module, title, nav) in [
        (setup.alpha(), "", false),
        (setup.alpha(), "  ?!  ", false),
        // `guide.adoc` is there already.
        (setup.alpha(), "Guide", false),
        (setup.root.join("nowhere"), "Elsewhere", false),
        // beta has no navigation file.
        (beta.clone(), "Notes", true),
    ] {
        let refused = setup.new_page(module, title, nav);
        assert!(refused.is_err(), "{title:?}: {refused:?}");
    }
    assert_eq!(before(&setup), count);
    assert!(!beta.join("pages/notes.adoc").exists());
    assert_eq!(
        fs::read_to_string(setup.alpha().join("pages/guide.adoc")).unwrap(),
        fs::read_to_string(fixture(
            "antora-multi-component/alpha/modules/ROOT/pages/guide.adoc"
        ))
        .unwrap()
    );
}

#[test]
fn the_modules_offered_are_listed_with_the_active_pages_first_choice() {
    let mut setup = Setup::new();
    setup
        .engine
        .open(&setup.root.join("beta/modules/api/pages/reference.adoc"))
        .unwrap();
    let modules = setup.engine.new_page_modules();
    let labels: Vec<&str> = modules.iter().map(|module| module.label.as_str()).collect();
    assert_eq!(labels, ["1.0@alpha:ROOT", "beta:ROOT", "beta:api"]);
    let active: Vec<&str> = modules
        .iter()
        .filter(|module| module.active)
        .map(|module| module.label.as_str())
        .collect();
    assert_eq!(active, ["beta:api"]);
    assert!(modules[0].nav.is_some() && modules[2].nav.is_none());
    assert_eq!(modules[2].root, setup.root.join("beta/modules/api"));

    let spec = setup.engine.commands().get(ids::PAGE_NEW).unwrap().spec();
    assert_eq!(spec.title, "New Page…");
    assert_eq!(spec.default_binding, Some("cmd-n"));
}
