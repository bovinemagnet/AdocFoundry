//! New Antora Project (#292): `project.new` makes `antora.yml`, the `ROOT` module, a
//! `.gitignore` and a README in an empty folder, or in a subfolder of a code repository, as one
//! undo step, with a Git repository when asked and none exists.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, normalize_path, planned_tree, CommandArgs, CommandOutcome, Engine, EngineError,
    NewPage, NewProject, NewProjectField, Settings, Welcome,
};
use adoc_git::{testing::TestRepo, Repository};

/// A temporary folder open as the workspace.
struct Setup {
    _directory: tempfile::TempDir,
    workspace: PathBuf,
    engine: Engine,
}

impl Setup {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = normalize_path(&directory.path().join("work"));
        fs::create_dir_all(&workspace).unwrap();
        Self::open(directory, workspace)
    }

    /// The workspace once `prepare` has put its files in place.
    fn with(prepare: impl FnOnce(&Path)) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = normalize_path(&directory.path().join("work"));
        fs::create_dir_all(&workspace).unwrap();
        prepare(&workspace);
        Self::open(directory, workspace)
    }

    fn open(directory: tempfile::TempDir, workspace: PathBuf) -> Self {
        let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
        engine.open_workspace(vec![workspace.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        Self {
            _directory: directory,
            workspace,
            engine,
        }
    }

    /// A request for the component `demo` in the empty folder `work/demo`, made here.
    fn request(&self) -> NewProject {
        let root = self.workspace.join("demo");
        fs::create_dir_all(&root).unwrap();
        NewProject {
            git_init: false,
            ..NewProject::new(root, "demo")
        }
    }

    fn make(&mut self, request: NewProject) -> Result<CommandOutcome, EngineError> {
        self.engine
            .invoke(ids::PROJECT_NEW, CommandArgs::NewProject(request))
    }
}

/// Every file beneath `root`, relative to it, `/`-separated, `.git` left out.
fn files(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                folders.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

const LAYOUT: [&str; 7] = [
    ".gitignore",
    "README.adoc",
    "antora.yml",
    "modules/ROOT/images/.gitkeep",
    "modules/ROOT/nav.adoc",
    "modules/ROOT/pages/index.adoc",
    "modules/ROOT/partials/.gitkeep",
];

#[test]
fn new_project_writes_a_valid_descriptor() {
    let mut setup = Setup::new();
    let request = setup.request();
    let root = request.root.clone();
    let planned = planned_tree(&request);
    let CommandOutcome::Opened(index) = setup.make(request).unwrap() else {
        panic!("the index page is opened");
    };
    assert_eq!(files(&root), LAYOUT);

    // planned_tree_matches_written_files
    let mut planned: Vec<String> = planned
        .iter()
        .map(|path| {
            let relative = path.strip_prefix(&root).unwrap();
            relative.to_string_lossy().replace('\\', "/")
        })
        .collect();
    planned.sort();
    assert_eq!(planned, LAYOUT);

    let text = fs::read_to_string(root.join("antora.yml")).unwrap();
    let descriptor = adoc_antora::read_component_descriptor(&root).unwrap();
    assert_eq!(descriptor.name, "demo");
    assert_eq!(descriptor.title.as_deref(), Some("Demo"));
    assert_eq!(descriptor.version, None);
    assert_eq!(descriptor.nav, ["modules/ROOT/nav.adoc"]);
    assert!(
        adoc_antora::check_component_descriptor(&root, &text).is_empty(),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(root.join("modules/ROOT/nav.adoc")).unwrap(),
        "* xref:index.adoc[]\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "build/\nnode_modules/\n.cache/\n"
    );

    // The component is in the catalog and its index page open.
    assert!(setup
        .engine
        .workspace()
        .antora
        .component("demo", None)
        .is_some());
    assert_eq!(setup.engine.active(), Some(index));
    assert_eq!(
        setup.engine.document(index).unwrap().path,
        root.join("modules/ROOT/pages/index.adoc")
    );
    assert!(setup
        .engine
        .document(index)
        .unwrap()
        .text
        .to_string()
        .starts_with("= Demo\n"));
}

#[test]
fn new_project_writes_what_it_is_asked() {
    let mut setup = Setup::new();
    let request = NewProject {
        title: "Billing \"Guide\": the: works".to_owned(),
        version: Some("2.0".to_owned()),
        index_title: "Welcome".to_owned(),
        readme: false,
        gitignore: false,
        families: vec![adoc_engine::ResourceFamily::Example],
        ..setup.request()
    };
    let root = request.root.clone();
    setup.make(request).unwrap();
    assert_eq!(
        files(&root),
        [
            "antora.yml",
            "modules/ROOT/examples/.gitkeep",
            "modules/ROOT/nav.adoc",
            "modules/ROOT/pages/index.adoc",
        ]
    );
    let descriptor = adoc_antora::read_component_descriptor(&root).unwrap();
    assert_eq!(
        descriptor.title.as_deref(),
        Some("Billing \"Guide\": the: works")
    );
    assert_eq!(descriptor.version.as_deref(), Some("2.0"));
    assert!(
        fs::read_to_string(root.join("modules/ROOT/pages/index.adoc"))
            .unwrap()
            .starts_with("= Welcome\n")
    );
    assert!(setup
        .engine
        .workspace()
        .antora
        .component("demo", Some("2.0"))
        .is_some());
}

#[test]
fn new_project_in_a_subfolder_of_a_code_repo() {
    let mut setup = Setup::with(|workspace| {
        let repo = TestRepo::init(workspace);
        repo.write("src/main.rs", "fn main() {}\n");
        repo.commit_all("code");
    });
    let request = NewProject {
        subfolder: Some("docs".to_owned()),
        ..NewProject::new(setup.workspace.clone(), "demo")
    };
    let docs = setup.workspace.join("docs");
    assert_eq!(setup.engine.check_new_project(&request), Ok(docs.clone()));
    setup.make(request).unwrap();
    assert_eq!(files(&docs), LAYOUT);
    assert!(adoc_antora::read_component_descriptor(&docs).is_ok());
    assert!(setup
        .engine
        .workspace()
        .antora
        .component("demo", None)
        .is_some());
}

#[test]
fn new_project_refuses_a_non_empty_folder_without_subfolder() {
    let mut setup = Setup::new();
    let request = setup.request();
    fs::write(request.root.join("notes.txt"), "kept\n").unwrap();
    let root = request.root.clone();
    let error = setup.make(request.clone()).unwrap_err();
    assert!(error.to_string().contains("not empty"), "{error}");
    assert_eq!(files(&root), ["notes.txt"], "nothing written");

    // A subfolder that exists already is refused too.
    fs::create_dir(root.join("docs")).unwrap();
    let error = setup
        .make(NewProject {
            subfolder: Some("docs".to_owned()),
            ..request.clone()
        })
        .unwrap_err();
    assert!(error.to_string().contains("exists already"), "{error}");

    // An empty folder holding only a repository counts as empty.
    fs::remove_file(root.join("notes.txt")).unwrap();
    fs::remove_dir(root.join("docs")).unwrap();
    Repository::init(&root).unwrap();
    assert!(setup.make(request).is_ok());
}

#[test]
fn new_project_refuses_inside_an_existing_component() {
    let mut setup = Setup::with(|workspace| {
        let docs = workspace.join("docs");
        fs::create_dir_all(docs.join("modules/ROOT/pages")).unwrap();
        fs::write(docs.join("antora.yml"), "name: old\nversion: ~\n").unwrap();
        fs::create_dir_all(docs.join("drafts")).unwrap();
    });
    let docs = setup.workspace.join("docs");
    for request in [
        NewProject::new(docs.join("drafts"), "demo"),
        NewProject {
            subfolder: Some("more".to_owned()),
            ..NewProject::new(docs.clone(), "demo")
        },
    ] {
        let error = setup.make(request).unwrap_err();
        assert!(error.to_string().contains("New Module"), "{error}");
    }
    assert!(!docs.join("drafts/antora.yml").exists());
    assert!(!docs.join("more").exists());
}

#[test]
fn new_project_validates_its_fields() {
    let setup = Setup::new();
    let request = setup.request();
    for (request, field, problem) in [
        (
            NewProject {
                name: "Demo".to_owned(),
                ..request.clone()
            },
            NewProjectField::Name,
            "not a component name",
        ),
        (
            NewProject {
                version: Some("1.0/beta".to_owned()),
                ..request.clone()
            },
            NewProjectField::Version,
            "not a version",
        ),
        (
            NewProject {
                version: Some("one two".to_owned()),
                ..request.clone()
            },
            NewProjectField::Version,
            "not a version",
        ),
        (
            NewProject {
                subfolder: Some("../outside".to_owned()),
                ..request.clone()
            },
            NewProjectField::Subfolder,
            "subfolder",
        ),
        (
            NewProject::new(setup.workspace.join("missing"), "demo"),
            NewProjectField::Location,
            "is not a folder",
        ),
    ] {
        let found = setup.engine.check_new_project(&request).unwrap_err();
        assert_eq!(found.field, field, "{found}");
        assert!(found.message.contains(problem), "{found}");
    }
}

#[test]
fn the_dialog_starts_from_the_open_folder() {
    let setup = Setup::new();
    let request = setup.engine.new_project_request();
    assert_eq!(request.root, setup.workspace);
    assert_eq!(request.name, "work", "named from the folder");
    assert_eq!(request.subfolder, None, "the folder is empty");
    assert!(request.git_init, "the folder is in no repository");
    assert!(!setup.engine.in_repository(&setup.workspace));
    assert!(setup.engine.check_new_project(&request).is_ok());

    // A folder with files in a repository: a `docs` subfolder, and no repository to make.
    let setup = Setup::with(|workspace| {
        let repo = TestRepo::init(workspace);
        repo.write("src/main.rs", "fn main() {}\n");
    });
    let request = setup.engine.new_project_request();
    assert_eq!(request.subfolder.as_deref(), Some("docs"));
    assert!(!request.git_init);
    assert!(setup.engine.in_repository(&setup.workspace));

    // A folder whose name is no component name gives none.
    let setup = Setup::with(|_| {});
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    let odd = setup.workspace.join("My Docs!");
    fs::create_dir(&odd).unwrap();
    engine.open_workspace(vec![odd]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert_eq!(engine.new_project_request().name, "my-docs");
    let none = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    assert_eq!(none.new_project_request().root, PathBuf::new());
}

#[test]
fn welcome_follows_what_the_folder_holds() {
    let none = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    assert_eq!(none.welcome(), Some(Welcome::NoFolder));

    let mut setup = Setup::new();
    assert_eq!(
        setup.engine.welcome(),
        Some(Welcome::NoDocumentation(setup.workspace.clone()))
    );

    let setup_pages = Setup::with(|workspace| {
        fs::write(workspace.join("notes.adoc"), "= Notes\n").unwrap();
    });
    assert_eq!(
        setup_pages.engine.welcome(),
        Some(Welcome::NotAntora(setup_pages.workspace.clone()))
    );

    setup.make(setup.request()).unwrap();
    assert_eq!(
        setup.engine.welcome(),
        None,
        "a component: nothing to offer"
    );
}

#[test]
fn new_project_git_init_makes_a_repository() {
    let mut setup = Setup::new();
    let request = NewProject {
        git_init: true,
        ..setup.request()
    };
    let root = request.root.clone();
    setup.make(request).unwrap();
    let repository = Repository::discover(&root).unwrap().expect("a repository");
    assert_eq!(repository.workdir(), root);
    assert_eq!(
        repository.head().unwrap(),
        adoc_git::Head::Unborn,
        "no commit"
    );
}

#[test]
fn new_project_without_git_init_inside_a_repo_does_not_nest() {
    let mut setup = Setup::with(|workspace| {
        let _ = TestRepo::init(workspace);
    });
    let request = NewProject {
        git_init: true,
        ..setup.request()
    };
    let root = request.root.clone();
    setup.make(request).unwrap();
    assert!(!root.join(".git").exists());
    let repository = Repository::discover(&root).unwrap().expect("the outer one");
    assert_eq!(repository.workdir(), setup.workspace);
}

#[test]
fn undo_removes_everything_untouched() {
    let mut setup = Setup::new();
    let request = setup.request();
    let root = request.root.clone();
    let CommandOutcome::Opened(index) = setup.make(request).unwrap() else {
        panic!();
    };
    assert!(setup.engine.undo(index));
    assert!(files(&root).is_empty(), "{:?}", files(&root));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "no folder left");
    assert!(setup
        .engine
        .workspace()
        .antora
        .component("demo", None)
        .is_none());
    assert!(
        setup.engine.workspace().descriptor_issues.is_empty(),
        "{:?}",
        setup.engine.workspace().descriptor_issues
    );

    // Made again, a file changed after it was made is kept on undo, and its folder with it.
    let CommandOutcome::Opened(index) = setup.make(setup.request()).unwrap() else {
        panic!();
    };
    assert_eq!(files(&root), LAYOUT);
    fs::write(
        root.join("modules/ROOT/nav.adoc"),
        "* xref:index.adoc[Home]\n",
    )
    .unwrap();
    assert!(setup.engine.undo(index));
    assert_eq!(files(&root), ["modules/ROOT/nav.adoc"]);
}

#[test]
fn new_project_resolves_at_once() {
    let mut setup = Setup::new();
    let request = setup.request();
    let root = request.root.clone();
    let CommandOutcome::Opened(index) = setup.make(request).unwrap() else {
        panic!();
    };
    setup
        .engine
        .invoke(
            ids::PAGE_NEW,
            CommandArgs::NewPage(NewPage {
                module: root.join("modules/ROOT"),
                title: "Setting Up".to_owned(),
                add_to_nav: true,
                file: Some(PathBuf::from("setup.adoc")),
            }),
        )
        .unwrap();
    let end = setup.engine.document(index).unwrap().text.len();
    setup
        .engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: index,
                range: SourceRange::new(end, end),
                text: "\nSee xref:setup.adoc[] and xref:demo::setup.adoc[].\n".to_owned(),
            },
        )
        .unwrap();
    let diagnostics = &setup.engine.document(index).unwrap().diagnostics;
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn new_project_outside_the_workspace_becomes_a_root() {
    let mut setup = Setup::new();
    let elsewhere = tempfile::tempdir().unwrap();
    let root = normalize_path(elsewhere.path());
    let CommandOutcome::Opened(index) = setup
        .make(NewProject {
            git_init: false,
            ..NewProject::new(root.clone(), "demo")
        })
        .unwrap()
    else {
        panic!();
    };
    assert_eq!(
        setup.engine.roots(),
        [setup.workspace.clone(), root.clone()]
    );
    assert!(setup.engine.wait_for_workers(Duration::from_secs(10)));
    assert!(setup
        .engine
        .workspace()
        .antora
        .component("demo", None)
        .is_some());
    assert_eq!(setup.engine.active(), Some(index));
}
