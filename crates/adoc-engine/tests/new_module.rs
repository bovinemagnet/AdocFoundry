//! New Module (#290): `module.new` makes a module's families, navigation file and index page in
//! a component version and lists the navigation file in `antora.yml`, as one undo step. On
//! copies of `tests/fixtures/antora-single-component`, whose `antora.yml` lists
//! `modules/ROOT/nav.adoc` indented by two spaces and which has a `security` module.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, CommandOutcome, Engine, EngineError, NewModule,
    ResourceFamily, Settings,
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

struct Setup {
    _directory: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
}

impl Setup {
    /// The fixture, with `antora.yml` replaced by `descriptor` when given.
    fn new(descriptor: Option<&str>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        copy_tree(&fixture("antora-single-component"), &root);
        if let Some(descriptor) = descriptor {
            fs::write(root.join("antora.yml"), descriptor).unwrap();
        }
        let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        Self {
            _directory: directory,
            root,
            engine,
        }
    }

    fn request(&self, name: &str) -> NewModule {
        NewModule {
            component: self.root.clone(),
            name: name.to_owned(),
            families: NewModule::DEFAULT_FAMILIES.to_vec(),
            nav: true,
            index_title: Some(String::new()),
        }
    }

    fn make(&mut self, request: NewModule) -> Result<CommandOutcome, EngineError> {
        self.engine
            .invoke(ids::MODULE_NEW, CommandArgs::NewModule(request))
    }

    fn descriptor(&self) -> String {
        fs::read_to_string(self.root.join("antora.yml")).unwrap()
    }
}

#[test]
fn new_module_creates_families_nav_and_index() {
    let mut setup = Setup::new(None);
    let before = setup.descriptor();
    let CommandOutcome::Opened(index) = setup.make(setup.request("admin-guide")).unwrap() else {
        panic!("the index page is opened");
    };
    let module = setup.root.join("modules/admin-guide");
    assert_eq!(
        fs::read_to_string(module.join("pages/index.adoc")).unwrap(),
        "= Admin Guide\n\nAn introduction to Admin Guide.\n"
    );
    assert_eq!(
        fs::read_to_string(module.join("nav.adoc")).unwrap(),
        "* xref:index.adoc[]\n"
    );
    assert!(module.join("partials/.gitkeep").is_file());
    assert!(module.join("images/.gitkeep").is_file());
    assert!(
        !module.join("pages/.gitkeep").exists(),
        "pages holds the index"
    );
    assert!(!module.join("examples").exists(), "not chosen");
    assert_eq!(setup.engine.active(), Some(index));
    assert_eq!(
        setup.engine.document(index).unwrap().path,
        module.join("pages/index.adoc")
    );
    // The descriptor gained exactly one line, and is saved for the catalog to read.
    let after = setup.descriptor();
    assert_eq!(after.lines().count(), before.lines().count() + 1);
    assert!(after.contains("  - modules/admin-guide/nav.adoc\n"));

    // Without navigation or an index: families only, each kept by a `.gitkeep`.
    let mut setup = Setup::new(None);
    let mut bare = setup.request("bare");
    bare.nav = false;
    bare.index_title = None;
    bare.families = vec![ResourceFamily::Example];
    assert_eq!(setup.make(bare).unwrap(), CommandOutcome::Done);
    let module = setup.root.join("modules/bare");
    assert!(module.join("pages/.gitkeep").is_file());
    assert!(module.join("examples/.gitkeep").is_file());
    assert!(!module.join("nav.adoc").exists());
    assert_eq!(
        setup.descriptor(),
        fs::read_to_string(fixture("antora-single-component/antora.yml")).unwrap()
    );
}

#[test]
fn new_module_appends_to_the_nav_list_preserving_indentation() {
    let mut setup = Setup::new(None);
    setup.make(setup.request("security2")).unwrap();
    assert!(
        setup.descriptor().contains(
            "nav:\n  - modules/ROOT/nav.adoc\n  - modules/security2/nav.adoc\n\nasciidoc:"
        ),
        "{}",
        setup.descriptor()
    );

    let mut setup = Setup::new(Some(
        "name: demo\nversion: latest\nnav:\n- 'modules/ROOT/nav.adoc'\n",
    ));
    setup.make(setup.request("extra")).unwrap();
    assert_eq!(
        setup.descriptor(),
        "name: demo\nversion: latest\nnav:\n- 'modules/ROOT/nav.adoc'\n- 'modules/extra/nav.adoc'\n"
    );
}

#[test]
fn new_module_creates_the_nav_key_when_absent() {
    let mut setup = Setup::new(Some("name: demo\nversion: latest\n"));
    setup.make(setup.request("extra")).unwrap();
    assert_eq!(
        setup.descriptor(),
        "name: demo\nversion: latest\nnav:\n- modules/extra/nav.adoc\n"
    );
    let trees = setup.engine.nav_trees("demo", Some("latest")).unwrap();
    assert_eq!(trees.len(), 1, "the new navigation file is the component's");
}

#[test]
fn new_module_refuses_an_existing_name() {
    let mut setup = Setup::new(None);
    let before = setup.descriptor();
    let error = setup.make(setup.request("security")).unwrap_err();
    assert!(error.to_string().contains("exists already"), "{error}");
    // A folder that is not yet a module is refused too.
    fs::create_dir_all(setup.root.join("modules/drafts")).unwrap();
    assert!(setup.make(setup.request("drafts")).is_err());
    for bad in ["Security", "a b", "../escape", ""] {
        assert!(setup.make(setup.request(bad)).is_err(), "{bad}");
    }
    assert_eq!(setup.descriptor(), before, "nothing written");
}

#[test]
fn new_module_refuses_root_when_root_exists() {
    let mut setup = Setup::new(None);
    let error = setup.make(setup.request("ROOT")).unwrap_err();
    assert!(error.to_string().contains("exists already"), "{error}");
}

#[test]
fn new_module_refuses_a_non_list_nav() {
    let mut setup = Setup::new(Some(
        "name: demo\nversion: latest\nnav: [modules/ROOT/nav.adoc]\n",
    ));
    let error = setup.make(setup.request("extra")).unwrap_err();
    assert!(error.to_string().contains("not a block list"), "{error}");
    assert!(
        !setup.root.join("modules/extra").exists(),
        "nothing written"
    );
    assert_eq!(
        setup.descriptor(),
        "name: demo\nversion: latest\nnav: [modules/ROOT/nav.adoc]\n"
    );
}

#[test]
fn undo_removes_untouched_files_and_reverts_the_descriptor() {
    let mut setup = Setup::new(None);
    let before = setup.descriptor();
    let CommandOutcome::Opened(index) = setup.make(setup.request("security2")).unwrap() else {
        panic!();
    };
    let module = setup.root.join("modules/security2");
    assert!(setup.engine.undo(index));
    assert!(!module.exists(), "every file and folder made is gone");
    assert_eq!(setup.descriptor(), before, "byte for byte");
    assert!(setup
        .engine
        .workspace()
        .antora
        .modules()
        .all(|module| module.name != "security2"));

    // Redo makes it again.
    let descriptor = setup
        .engine
        .document_for_path(&setup.root.join("antora.yml"))
        .unwrap();
    assert!(setup.engine.redo(descriptor));
    assert!(module.join("pages/index.adoc").is_file());
    assert!(setup.descriptor().contains("modules/security2/nav.adoc"));
}

#[test]
fn undo_keeps_a_file_edited_after_creation() {
    let mut setup = Setup::new(None);
    let CommandOutcome::Opened(index) = setup.make(setup.request("security2")).unwrap() else {
        panic!();
    };
    let module = setup.root.join("modules/security2");
    // The navigation file is changed on disk after it was made.
    fs::write(module.join("nav.adoc"), "* xref:index.adoc[Home]\n").unwrap();
    assert!(setup.engine.undo(index));
    assert_eq!(
        fs::read_to_string(module.join("nav.adoc")).unwrap(),
        "* xref:index.adoc[Home]\n",
        "kept, and so is its folder"
    );
    assert!(!module.join("pages/index.adoc").exists());
}

#[test]
fn new_module_resolves_without_reindex() {
    let mut setup = Setup::new(None);
    setup.make(setup.request("security2")).unwrap();
    let home = setup.root.join("modules/ROOT/pages/index.adoc");
    let id = setup.engine.open(&home).unwrap();
    let end = setup.engine.document(id).unwrap().text.len();
    setup
        .engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end),
                text: "\nSee xref:security2:index.adoc[].\n".to_owned(),
            },
        )
        .unwrap();
    let diagnostics = &setup.engine.document(id).unwrap().diagnostics;
    assert!(
        diagnostics.iter().all(|diagnostic| diagnostic.code.as_str()
            != "adoc.unresolved-xref-file"
            && diagnostic.code.as_str() != "adoc.antora.unknown-module"),
        "{diagnostics:?}"
    );
}

#[test]
fn the_components_a_module_can_go_in() {
    let setup = Setup::new(None);
    let components = setup.engine.new_module_components();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].label, "latest@demo");
    assert_eq!(components[0].existing_modules, ["ROOT", "security"]);
    assert!(components[0].has_nav_list);
    assert!(setup
        .engine
        .check_new_module(&setup.root, "billing")
        .is_ok());
    assert!(setup
        .engine
        .check_new_module(&setup.root, "security")
        .is_err());
}
