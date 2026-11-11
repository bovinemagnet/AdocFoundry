//! What the paths an app is launched with, or handed by the system, open (#498): a folder as
//! the workspace and files in tabs, a file alone opening the workspace it belongs to.

use std::path::{Path, PathBuf};

use adoc_engine::{file_url_path, launch_targets, workspace_for_file, LaunchTargets};

/// A project with a playbook and settings at its top, a component below, and a loose folder.
fn project() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path()).join("project");
    for (path, text) in [
        ("antora-playbook.yml", "site:\n  title: Docs\n"),
        ("docs/antora.yml", "name: docs\nversion: ~\n"),
        ("docs/modules/ROOT/pages/index.adoc", "= Index\n"),
        ("docs/modules/ROOT/pages/guide.adoc", "= Guide\n"),
        ("component/antora.yml", "name: other\nversion: ~\n"),
        ("component/modules/ROOT/pages/page.adoc", "= Page\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let loose = directory.path().join("loose");
    std::fs::create_dir_all(&loose).unwrap();
    std::fs::write(loose.join("notes.adoc"), "= Notes\n").unwrap();
    (directory, root)
}

fn targets(paths: &[&Path]) -> LaunchTargets {
    launch_targets(paths.iter().map(|path| path.to_path_buf()))
}

#[test]
fn a_folder_argument_is_the_workspace_and_the_rest_are_files() {
    let (_directory, root) = project();
    let index = root.join("docs/modules/ROOT/pages/index.adoc");
    assert_eq!(
        targets(&[&root]),
        LaunchTargets {
            folder: Some(root.clone()),
            files: Vec::new(),
        }
    );
    assert_eq!(
        targets(&[&root, &index]),
        LaunchTargets {
            folder: Some(root.clone()),
            files: vec![index],
        }
    );
    assert_eq!(targets(&[]), LaunchTargets::default());
}

#[test]
fn a_file_argument_is_opened_in_the_workspace_it_belongs_to() {
    let (directory, root) = project();
    let index = root.join("docs/modules/ROOT/pages/index.adoc");
    let guide = root.join("docs/modules/ROOT/pages/guide.adoc");
    // The first argument a file, as a file manager hands one over: not taken as the folder.
    assert_eq!(
        targets(&[&index]),
        LaunchTargets {
            folder: Some(root.clone()),
            files: vec![index.clone()],
        }
    );
    // Several files, all of them opened.
    assert_eq!(
        targets(&[&index, &guide]),
        LaunchTargets {
            folder: Some(root.clone()),
            files: vec![index, guide],
        }
    );
    // Outside any project: the file's own folder.
    let notes = directory.path().join("loose/notes.adoc");
    assert_eq!(
        targets(&[&notes]).folder,
        Some(directory.path().join("loose"))
    );
}

#[test]
fn a_file_s_workspace_is_its_project_else_its_component_else_its_folder() {
    let (directory, root) = project();
    // The playbook's folder above the component.
    assert_eq!(
        workspace_for_file(&root.join("component/modules/ROOT/pages/page.adoc")),
        root
    );
    // Without a playbook or settings above, the component's folder.
    std::fs::remove_file(root.join("antora-playbook.yml")).unwrap();
    assert_eq!(
        workspace_for_file(&root.join("component/modules/ROOT/pages/page.adoc")),
        root.join("component")
    );
    // Workspace settings mark a project as a playbook does.
    std::fs::write(root.join(".adocfoundry.toml"), "").unwrap();
    assert_eq!(
        workspace_for_file(&root.join("docs/modules/ROOT/pages/index.adoc")),
        root
    );
    assert_eq!(
        workspace_for_file(&directory.path().join("loose/notes.adoc")),
        directory.path().join("loose")
    );
}

#[test]
fn files_handed_over_as_urls_are_paths() {
    assert_eq!(
        file_url_path("file:///Users/me/My%20Docs/guide.adoc"),
        Some(PathBuf::from("/Users/me/My Docs/guide.adoc"))
    );
    assert_eq!(
        file_url_path("file://localhost/tmp/caf%C3%A9.adoc"),
        Some(PathBuf::from("/tmp/café.adoc"))
    );
    assert_eq!(file_url_path("https://example.org/page.adoc"), None);
    assert_eq!(file_url_path("file://server/share/page.adoc"), None);
    assert_eq!(file_url_path("file:///broken%2"), None);
}
