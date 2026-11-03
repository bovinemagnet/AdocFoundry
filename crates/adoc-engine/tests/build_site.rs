//! Build Site (#293): `project.build_site` runs the playbook's build with npm, bounded in time
//! and output, streaming its log, and is unavailable, saying why, without Node.js.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, Engine, EngineError, NewProject, Settings,
    SiteBuildBlocker, SiteBuildState,
};

/// Held while a fake tool is written and run, so no other test forks while one is open for
/// writing (`ETXTBSY`), as `adoc-render`'s `spawn_guard` does there.
static SPAWN: Mutex<()> = Mutex::new(());

/// A workspace holding the project `demo` that `project.new` made, with its playbook, opened
/// with `[tools] node` set to `node` and `[tools] build_timeout` to `timeout`.
fn project(node: &Path, timeout: u64) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("settings.toml");
    fs::write(
        &user,
        format!(
            "[tools]\nnode = {:?}\nbuild_timeout = {timeout}\n",
            node.display().to_string()
        ),
    )
    .unwrap();
    let root = normalize_path(&directory.path().join("demo"));
    fs::create_dir_all(&root).unwrap();
    let settings = Settings::load(Some(&user), None).unwrap().with_git_opt_in();
    assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let request = NewProject {
        git_init: false,
        ..NewProject::new(root.clone(), "demo")
    };
    engine
        .invoke(ids::PROJECT_NEW, CommandArgs::NewProject(request))
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    (directory, root, engine)
}

#[test]
fn build_site_disabled_without_node() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("no-node-here/node");
    let (_project, _root, mut engine) = project(&missing, 60);

    assert!(!engine.command_available(ids::PROJECT_BUILD_SITE));
    assert_eq!(engine.site_build_plan(), None);
    let blocker = engine.site_build_blocker().unwrap();
    assert!(
        matches!(&blocker, SiteBuildBlocker::NoNode { configured: Some(node) } if node.ends_with("node")),
        "{blocker:?}"
    );
    let hint = blocker.to_string();
    assert!(hint.contains("Node.js"), "{hint}");
    let refused = engine
        .invoke(ids::PROJECT_BUILD_SITE, CommandArgs::None)
        .unwrap_err();
    assert!(
        matches!(&refused, EngineError::NotFound(message) if message == &hint),
        "{refused:?}"
    );
    assert!(engine.site_build().is_none());
}

#[test]
fn build_site_needs_a_playbook() {
    let directory = tempfile::tempdir().unwrap();
    let root = normalize_path(directory.path());
    fs::write(root.join("page.adoc"), "= Page\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![root]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));

    assert!(!engine.command_available(ids::PROJECT_BUILD_SITE));
    assert_eq!(
        engine.site_build_blocker(),
        Some(SiteBuildBlocker::NoPlaybook)
    );
}

#[cfg(unix)]
#[test]
fn build_site_streams_output_and_times_out() {
    use std::os::unix::fs::PermissionsExt as _;

    let tools = tempfile::tempdir().unwrap();
    let node = tools.path().join("node");
    let npm = tools.path().join("npm");
    let guard = SPAWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    fs::write(&node, "#!/bin/sh\nexit 0\n").unwrap();
    // A stand-in for npm: it says what it was asked, `install` makes `node_modules`, and `run
    // build` writes the site, or waits far longer than the build may run while `slow` exists.
    fs::write(
        &npm,
        "#!/bin/sh\n\
         echo \"npm $*\"\n\
         case \"$1\" in\n\
         install) mkdir -p node_modules ;;\n\
         run) if [ -f slow ]; then echo waiting; sleep 30; fi\n\
              mkdir -p build/site && echo '<html></html>' > build/site/index.html\n\
              echo 'site written' >&2 ;;\n\
         esac\n",
    )
    .unwrap();
    for tool in [&node, &npm] {
        fs::set_permissions(tool, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (_project, root, mut engine) = project(&node, 1);
    assert!(engine.command_available(ids::PROJECT_BUILD_SITE));
    assert_eq!(engine.site_build_blocker(), None);
    assert_eq!(
        engine.site_build_plan(),
        Some((
            root.clone(),
            vec!["npm install".to_owned(), "npm run build".to_owned()]
        ))
    );

    // The first build installs, then builds, and says where the site is.
    engine
        .invoke(ids::PROJECT_BUILD_SITE, CommandArgs::None)
        .unwrap();
    assert_eq!(
        engine.site_build().map(|build| &build.state),
        Some(&SiteBuildState::Running)
    );
    assert!(!engine.command_available(ids::PROJECT_BUILD_SITE));
    assert!(matches!(
        engine.invoke(ids::PROJECT_BUILD_SITE, CommandArgs::None),
        Err(EngineError::Refused(_))
    ));
    assert!(engine.wait_for_workers(Duration::from_secs(20)));
    let build = engine.site_build().unwrap();
    assert_eq!(
        build.state,
        SiteBuildState::Built {
            site: Some(root.join("build/site/index.html"))
        },
        "{:?}",
        build.log
    );
    for line in [
        "$ npm install",
        "npm install",
        "$ npm run build",
        "npm run build",
        "site written",
    ] {
        assert!(
            build.log.iter().any(|logged| logged == line),
            "{line}: {:?}",
            build.log
        );
    }

    // With `node_modules` in place it only builds, and a build that outlasts the timeout is
    // stopped, its output so far kept.
    fs::write(root.join("slow"), "").unwrap();
    let started = Instant::now();
    engine
        .invoke(ids::PROJECT_BUILD_SITE, CommandArgs::None)
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(20)));
    drop(guard);
    assert!(started.elapsed() < Duration::from_secs(15));
    let build = engine.site_build().unwrap();
    assert!(
        matches!(&build.state, SiteBuildState::Failed(message) if message.contains("1 s")),
        "{:?}",
        build.state
    );
    assert!(!build.log.iter().any(|line| line == "$ npm install"));
    assert!(
        build.log.iter().any(|line| line == "waiting"),
        "{:?}",
        build.log
    );
    assert!(engine.command_available(ids::PROJECT_BUILD_SITE));
}

#[test]
fn a_new_project_s_playbook_is_the_site_s_until_undone() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("no-node-here/node");
    let (_project, root, mut engine) = project(&missing, 60);
    assert_eq!(
        engine.site_build_blocker(),
        Some(SiteBuildBlocker::NoNode {
            configured: Some(missing.display().to_string())
        }),
        "the playbook is found without indexing again"
    );
    let index = engine
        .document_for_path(&root.join("modules/ROOT/pages/index.adoc"))
        .unwrap();
    assert!(engine.undo(index));
    assert!(!root.join("antora-playbook.yml").exists());
    assert_eq!(
        engine.site_build_blocker(),
        Some(SiteBuildBlocker::NoPlaybook)
    );
}
