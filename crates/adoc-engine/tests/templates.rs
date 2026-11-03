//! New Antora Project's templates (#295): every built-in makes a project that checks clean,
//! and the user's and a workspace's template folders are listed after the built-ins, read
//! afresh each time.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, CommandOutcome, Engine, NewProject, Settings,
    TemplateSource,
};

/// An engine on the empty folder `work` in a temporary directory, with a user settings file
/// there (so `templates/` beside it is the user's template folder).
fn engine_on_empty_folder() -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let work = normalize_path(&directory.path().join("work"));
    fs::create_dir(&work).unwrap();
    let user = directory.path().join("settings.toml");
    fs::write(&user, "").unwrap();
    let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![work.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    (directory, work, engine)
}

/// Every `antora.yml` under `folder`.
fn descriptors(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else if path.ends_with("antora.yml") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn every_builtin_template_scaffolds_and_checks_clean() {
    let (_directory, _work, engine) = engine_on_empty_folder();
    let builtins: Vec<String> = engine
        .templates()
        .into_iter()
        .filter(|template| template.source == TemplateSource::Builtin)
        .map(|template| template.id)
        .collect();
    assert_eq!(
        builtins,
        ["minimal", "product-docs", "multi-component", "single-page"]
    );
    for id in builtins {
        let (_directory, work, mut engine) = engine_on_empty_folder();
        let request = NewProject {
            template: id.clone(),
            title: "Billing: the \"guide\"".to_owned(),
            git_init: false,
            ..NewProject::new(work.clone(), "billing")
        };
        let planned = engine.planned_tree(&request);
        let CommandOutcome::Opened(index) = engine
            .invoke(ids::PROJECT_NEW, CommandArgs::NewProject(request))
            .unwrap_or_else(|error| panic!("{id}: {error}"))
        else {
            panic!("{id}: a page is opened");
        };
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        for path in &planned {
            assert!(path.is_file(), "{id}: {} planned and made", path.display());
        }
        let problems: Vec<_> = engine
            .workspace_diagnostics()
            .into_iter()
            .filter(|(_, found)| !found.is_empty())
            .collect();
        assert!(problems.is_empty(), "{id}: {problems:#?}");
        let found = descriptors(&work);
        for descriptor in &found {
            adoc_antora::read_component_descriptor(descriptor.parent().unwrap())
                .unwrap_or_else(|error| panic!("{id}: {error}"));
        }
        let opened = engine.document(index).unwrap().path.clone();
        match id.as_str() {
            "minimal" => {
                assert_eq!(found, [work.join("antora.yml")]);
                assert!(opened.ends_with("modules/ROOT/pages/index.adoc"));
            }
            "product-docs" => {
                let descriptor = adoc_antora::read_component_descriptor(&work).unwrap();
                assert_eq!(descriptor.nav.len(), 5, "{:?}", descriptor.nav);
                let page =
                    fs::read_to_string(work.join("modules/how-to/pages/index.adoc")).unwrap();
                assert!(page.contains("include::ROOT:partial$attributes.adoc[]"));
                assert!(!work.join("modules/ROOT/partials/.gitkeep").exists());
                let attributes =
                    fs::read_to_string(work.join("modules/ROOT/partials/attributes.adoc")).unwrap();
                assert!(!attributes.contains("{{"), "{attributes}");
                let catalog = &engine.workspace().antora;
                assert!(catalog.resolve_start_page("billing::index.adoc").is_some());
            }
            "multi-component" => {
                assert_eq!(
                    found,
                    [
                        work.join("components/guide/antora.yml"),
                        work.join("components/reference/antora.yml")
                    ]
                );
                let playbook = fs::read_to_string(work.join("antora-playbook.yml")).unwrap();
                assert!(
                    playbook.contains("start_page: billing-guide::index.adoc"),
                    "{playbook}"
                );
                assert!(
                    playbook.contains(
                        "    start_paths:\n    - components/guide\n    - components/reference\n"
                    ),
                    "{playbook}"
                );
                assert!(engine.workspace().antora.playbook().is_some());
                assert!(opened.ends_with("components/guide/modules/ROOT/pages/index.adoc"));
            }
            "single-page" => {
                assert!(found.is_empty());
                assert_eq!(opened, work.join("index.adoc"));
                assert!(work.join(".adocfoundry.toml").is_file());
                assert!(!work.join("antora-playbook.yml").exists());
                assert!(!work.join("README.adoc").exists());
            }
            other => panic!("unexpected template {other}"),
        }
    }
}

#[test]
fn single_page_needs_no_component_name() {
    let (_directory, work, mut engine) = engine_on_empty_folder();
    let request = NewProject {
        template: "single-page".to_owned(),
        title: "Release Notes".to_owned(),
        version: Some("not a version".to_owned()),
        git_init: false,
        ..NewProject::new(work.clone(), "")
    };
    assert_eq!(
        engine.check_new_project(&request),
        Ok(work.join("index.adoc"))
    );
    engine.new_project(&request).unwrap();
    assert_eq!(
        fs::read_to_string(work.join("index.adoc")).unwrap(),
        "= Release Notes\n\nWrite your document here.\n"
    );
}

#[test]
fn user_template_dir_is_listed_after_builtins() {
    let (directory, work, mut engine) = engine_on_empty_folder();
    let house = directory.path().join("templates/house");
    fs::create_dir_all(house.join("modules/ROOT/pages")).unwrap();
    fs::write(
        house.join("template.toml"),
        "name = \"House style\"\ndescription = \"Ours.\"\n",
    )
    .unwrap();
    fs::write(
        house.join("modules/ROOT/pages/{{name}}-history.adoc"),
        "= History of {{title}}\n\nSince {{year}}.\n",
    )
    .unwrap();
    // A team's templates, from the workspace's settings.
    let team = work.join("tools/templates/team");
    fs::create_dir_all(&team).unwrap();
    fs::write(team.join("template.toml"), "name = \"Team\"\n").unwrap();
    fs::write(
        work.join(".adocfoundry.toml"),
        "[templates]\ndir = \"tools/templates\"\n",
    )
    .unwrap();
    engine.open_workspace(vec![work.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));

    let listed: Vec<(String, String)> = engine
        .templates()
        .into_iter()
        .map(|template| (template.id, template.name))
        .collect();
    let ids: Vec<&str> = listed.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "minimal",
            "product-docs",
            "multi-component",
            "single-page",
            "user/house",
            "workspace/team"
        ]
    );
    assert_eq!(listed[4].1, "House style");

    // One that does not read is left out, saying why.
    let broken = directory.path().join("templates/broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(
        broken.join("template.toml"),
        "name = \"Broken\"\nflavour = 1\n",
    )
    .unwrap();
    assert!(!engine
        .templates()
        .iter()
        .any(|template| template.id == "user/broken"));
    let problems = engine.template_problems();
    assert!(
        problems.len() == 1 && problems[0].starts_with("user/broken: "),
        "{problems:?}"
    );

    // Read afresh each time: a template added now is listed at once.
    let later = directory.path().join("templates/later");
    fs::create_dir_all(&later).unwrap();
    fs::write(later.join("template.toml"), "name = \"Later\"\n").unwrap();
    assert!(engine
        .templates()
        .iter()
        .any(|template| template.id == "user/later"));

    // And a project made from it has its files, substituted.
    let target = normalize_path(&directory.path().join("site"));
    fs::create_dir(&target).unwrap();
    let request = NewProject {
        template: "user/house".to_owned(),
        git_init: false,
        ..NewProject::new(target.clone(), "billing")
    };
    engine.new_project(&request).unwrap();
    let history =
        fs::read_to_string(target.join("modules/ROOT/pages/billing-history.adoc")).unwrap();
    assert!(
        history.starts_with("= History of Billing\n\nSince 20"),
        "{history}"
    );
    assert!(engine
        .check_new_project(&NewProject {
            template: "user/missing".to_owned(),
            ..NewProject::new(target, "billing")
        })
        .is_err_and(|problem| problem.field == adoc_engine::NewProjectField::Template));
}

#[test]
fn a_template_cannot_write_outside_the_project() {
    let (directory, work, engine) = engine_on_empty_folder();
    let escape = directory.path().join("templates/escape");
    fs::create_dir_all(escape.join("{{name}}")).unwrap();
    fs::write(
        escape.join("template.toml"),
        "name = \"Escape\"\n[[component]]\npath = \"..\"\n",
    )
    .unwrap();
    let request = NewProject {
        template: "user/escape".to_owned(),
        git_init: false,
        ..NewProject::new(work, "billing")
    };
    let problem = engine.check_new_project(&request).unwrap_err();
    assert_eq!(problem.field, adoc_engine::NewProjectField::Template);
    assert!(problem.message.contains("inside the project"), "{problem}");
    assert!(engine.planned_tree(&request).is_empty());
}
