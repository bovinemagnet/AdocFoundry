//! The commands behind the Navigation and Files panels' menus: New File, Add to Navigation,
//! Export Pages as PDF, and the pages a folder holds.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, CommandOutcome, Engine, EngineEvent, Settings,
};

const WAIT: Duration = Duration::from_secs(60);

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

/// A copy of the multi-component fixture, with a page no navigation lists and a partial:
/// `alpha` 1.0 has a navigation file, `beta` none.
struct Setup {
    directory: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
}

impl Setup {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        copy_tree(&fixture("antora-multi-component"), &root);
        let pages = root.join("alpha/modules/ROOT/pages");
        fs::write(
            pages.join("extra.adoc"),
            "= Extra\n\nNot in the navigation.\n",
        )
        .unwrap();
        fs::write(pages.join("notes.txt"), "Not AsciiDoc.\n").unwrap();
        let partials = root.join("alpha/modules/ROOT/partials");
        fs::create_dir_all(&partials).unwrap();
        fs::write(partials.join("snippet.adoc"), "A snippet.\n").unwrap();
        let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        Self {
            directory,
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

    fn text(&self, path: &Path) -> String {
        match self.engine.document_for_path(path) {
            Some(id) => self.engine.document(id).unwrap().text.to_string(),
            None => fs::read_to_string(path).unwrap(),
        }
    }
}

#[test]
fn new_file_makes_an_empty_file_of_any_kind_and_undo_takes_it_away() {
    let mut setup = Setup::new();
    let file = setup.alpha().join("examples/settings.yml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();

    let outcome = setup
        .engine
        .invoke(ids::FILE_NEW, CommandArgs::Path(file.clone()))
        .unwrap();
    let CommandOutcome::Opened(id) = outcome else {
        panic!("expected the file opened, got {outcome:?}");
    };
    assert_eq!(fs::read_to_string(&file).unwrap(), "");
    assert_eq!(setup.engine.document(id).unwrap().path, file);
    assert_eq!(setup.engine.active(), Some(id));

    setup
        .engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert!(!file.exists(), "undo takes the new file away");
}

#[test]
fn new_file_refuses_a_file_that_exists_or_one_outside_the_workspace() {
    let mut setup = Setup::new();
    let existing = setup.alpha().join("pages/guide.adoc");
    let before = fs::read_to_string(&existing).unwrap();
    assert!(setup
        .engine
        .invoke(ids::FILE_NEW, CommandArgs::Path(existing.clone()))
        .is_err());
    assert_eq!(fs::read_to_string(&existing).unwrap(), before);

    let outside = setup.directory.path().join("outside.adoc");
    assert!(setup
        .engine
        .invoke(ids::FILE_NEW, CommandArgs::Path(outside.clone()))
        .is_err());
    assert!(!outside.exists());

    let no_folder = setup.alpha().join("no-such-folder/page.adoc");
    assert!(setup
        .engine
        .invoke(ids::FILE_NEW, CommandArgs::Path(no_folder.clone()))
        .is_err());
    assert!(!no_folder.exists());
}

#[test]
fn add_to_navigation_lists_a_page_at_the_end_of_its_modules_navigation() {
    let mut setup = Setup::new();
    let extra = setup.alpha().join("pages/extra.adoc");
    let before = setup.text(&setup.nav());
    assert!(setup.engine.can_add_to_nav(&extra));

    setup
        .engine
        .invoke(ids::NAV_ADD_PAGE, CommandArgs::Path(extra.clone()))
        .unwrap();
    let after = setup.text(&setup.nav());
    assert_eq!(after, format!("{before}* xref:extra.adoc[]\n"));
    assert!(
        !setup.engine.can_add_to_nav(&extra),
        "a listed page is not offered again"
    );

    let nav = setup.engine.document_for_path(&setup.nav()).unwrap();
    setup
        .engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(nav))
        .unwrap();
    assert_eq!(setup.text(&setup.nav()), before);
}

#[test]
fn add_to_navigation_refuses_listed_pages_and_modules_without_navigation() {
    let mut setup = Setup::new();
    let guide = setup.alpha().join("pages/guide.adoc");
    let before = setup.text(&setup.nav());
    assert!(!setup.engine.can_add_to_nav(&guide));
    assert!(setup
        .engine
        .invoke(ids::NAV_ADD_PAGE, CommandArgs::Path(guide))
        .is_err());
    assert_eq!(setup.text(&setup.nav()), before);

    // beta has no navigation file; a partial is not a page; a text file is not AsciiDoc.
    for path in [
        setup.root.join("beta/modules/ROOT/pages/index.adoc"),
        setup.alpha().join("partials/snippet.adoc"),
        setup.alpha().join("pages/notes.txt"),
    ] {
        assert!(!setup.engine.can_add_to_nav(&path), "{}", path.display());
        assert!(setup
            .engine
            .invoke(ids::NAV_ADD_PAGE, CommandArgs::Path(path))
            .is_err());
    }
}

#[test]
fn a_folders_pages_are_its_asciidoc_pages_only() {
    let setup = Setup::new();
    let pages = setup.alpha().join("pages");
    assert_eq!(
        setup.engine.pages_in_folder(&pages),
        [
            pages.join("extra.adoc"),
            pages.join("guide.adoc"),
            pages.join("index.adoc"),
        ]
    );
    assert!(setup
        .engine
        .pages_in_folder(&setup.alpha().join("partials"))
        .is_empty());
}

#[test]
fn export_pages_writes_one_pdf_per_page_one_after_another() {
    let mut setup = Setup::new();
    let events = setup.engine.subscribe();
    let out = setup.directory.path().join("pdfs");
    fs::create_dir_all(&out).unwrap();
    let pages = vec![
        setup.alpha().join("pages/index.adoc"),
        setup.alpha().join("pages/guide.adoc"),
        // Named alike: the second is told apart rather than written over the first.
        setup.root.join("beta/modules/ROOT/pages/index.adoc"),
    ];
    setup
        .engine
        .invoke(
            ids::EXPORT_PDF_PAGES,
            CommandArgs::ExportPages {
                pages,
                folder: out.clone(),
            },
        )
        .unwrap();
    assert!(setup.engine.is_exporting());
    let deadline = std::time::Instant::now() + WAIT;
    while setup.engine.is_exporting() && std::time::Instant::now() < deadline {
        setup.engine.wait_for_workers(WAIT);
        setup.engine.pump();
    }
    assert!(!setup.engine.is_exporting());

    let events: Vec<_> = events.try_iter().collect();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EngineEvent::ExportFinished { .. })),
        "one summary, not a notice per page"
    );
    let summary = events.iter().find_map(|event| match event {
        EngineEvent::PagesExported {
            folder,
            written,
            failed,
        } => Some((folder.clone(), written.clone(), failed.clone())),
        _ => None,
    });
    let (folder, written, failed) = summary.expect("a PagesExported event");
    assert_eq!(folder, out);
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(
        written,
        [
            out.join("index.pdf"),
            out.join("guide.pdf"),
            out.join("index-2.pdf")
        ]
    );
    for pdf in &written {
        assert!(
            fs::read(pdf).unwrap().starts_with(b"%PDF"),
            "{}",
            pdf.display()
        );
    }
}

#[test]
fn export_pages_refuses_nothing_to_export_or_a_missing_folder() {
    let mut setup = Setup::new();
    let out = setup.directory.path().join("pdfs");
    fs::create_dir_all(&out).unwrap();
    assert!(setup
        .engine
        .invoke(
            ids::EXPORT_PDF_PAGES,
            CommandArgs::ExportPages {
                pages: Vec::new(),
                folder: out,
            },
        )
        .is_err());
    assert!(setup
        .engine
        .invoke(
            ids::EXPORT_PDF_PAGES,
            CommandArgs::ExportPages {
                pages: vec![setup.alpha().join("pages/index.adoc")],
                folder: setup.directory.path().join("missing"),
            },
        )
        .is_err());
    assert!(!setup.engine.is_exporting());
}

/// A link planted at an output's name is not written through: that page fails, the rest are
/// exported, and the link's target is untouched.
#[cfg(unix)]
#[test]
fn export_pages_never_writes_through_a_link() {
    let mut setup = Setup::new();
    let events = setup.engine.subscribe();
    let out = setup.directory.path().join("pdfs");
    fs::create_dir_all(&out).unwrap();
    let elsewhere = setup.directory.path().join("precious.txt");
    fs::write(&elsewhere, "keep me").unwrap();
    std::os::unix::fs::symlink(&elsewhere, out.join("index.pdf")).unwrap();
    let index = setup.alpha().join("pages/index.adoc");
    setup
        .engine
        .invoke(
            ids::EXPORT_PDF_PAGES,
            CommandArgs::ExportPages {
                pages: vec![index.clone(), setup.alpha().join("pages/guide.adoc")],
                folder: out.clone(),
            },
        )
        .unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while setup.engine.is_exporting() && std::time::Instant::now() < deadline {
        setup.engine.wait_for_workers(WAIT);
        setup.engine.pump();
    }
    let (written, failed) = events
        .try_iter()
        .find_map(|event| match event {
            EngineEvent::PagesExported {
                written, failed, ..
            } => Some((written, failed)),
            _ => None,
        })
        .expect("a PagesExported event");
    assert_eq!(written, [out.join("guide.pdf")]);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, index);
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "keep me");
}
