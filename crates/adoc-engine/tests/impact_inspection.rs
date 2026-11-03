//! The impact inspector (#311): one kept analysis, worked out on a worker, superseded by a newer
//! request and out of date once the workspace changes; Inspect Impact as if a file were
//! removed (#100), and an impact's dependency path as graph steps.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids,
    impact::{EvidenceSide, ImpactKind, ImpactReport, ImpactStatus},
    normalize_path, ChangeSource, CommandArgs, EdgeKind, Engine, GraphKey, InspectionState,
    Settings,
};

const NESTED: &str = "v1/modules/ROOT/partials/nested.adoc";
const INSTALL: &str = "v1/modules/ROOT/pages/install.adoc";

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

/// `tests/fixtures/impact` copied out of any repository, opened.
fn fixture() -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = normalize_path(&directory.path().join("impact"));
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/impact"),
        &root,
    );
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    (directory, root, engine)
}

fn ready(engine: &Engine) -> &ImpactReport {
    match &engine.impact_inspection().expect("an inspection").state {
        InspectionState::Ready(report) => report,
        other => panic!("not ready: {other:?}"),
    }
}

#[test]
fn inspect_impact_runs_on_a_worker_and_goes_out_of_date() {
    let (_directory, root, mut engine) = fixture();
    let nested = root.join(NESTED);
    engine
        .invoke(ids::IMPACT_INSPECT, CommandArgs::Path(nested.clone()))
        .unwrap();
    // Asked, not answered: the work is on a worker.
    let inspection = engine.impact_inspection().unwrap();
    assert_eq!(
        inspection.source,
        ChangeSource::Removal(vec![nested.clone()])
    );
    assert_eq!(inspection.state, InspectionState::Running);
    assert!(!engine.impact_stale());
    assert!(engine.wait_for_workers(Duration::from_secs(30)));

    // Removing the nested partial changes what its includers render, transitively, and nothing
    // was removed.
    let report = ready(&engine);
    assert!(nested.is_file());
    let rendered: Vec<(&str, &[String])> = report
        .impacts
        .iter()
        .filter(|impact| impact.kind == ImpactKind::Render)
        .map(|impact| (impact.path.as_str(), impact.via.as_slice()))
        .collect();
    for (page, via) in [
        (
            "v1/modules/ROOT/pages/index.adoc",
            &[
                NESTED,
                "v1/modules/ROOT/partials/intro.adoc",
                "v1/modules/ROOT/pages/index.adoc",
            ][..],
        ),
        (
            "v1/modules/ROOT/pages/install.adoc",
            &[
                NESTED,
                "v1/modules/ROOT/partials/intro.adoc",
                "v1/modules/ROOT/pages/install.adoc",
            ][..],
        ),
    ] {
        assert!(
            rendered
                .iter()
                .any(|(path, found)| *path == page && *found == via),
            "{page}: {rendered:?}"
        );
    }

    // Typing in a page makes the kept analysis out of date, until it is asked again.
    let page = engine
        .open(&root.join("v1/modules/ROOT/pages/guide.adoc"))
        .unwrap();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: page,
                range: adoc_core::SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .unwrap();
    assert!(engine.impact_stale());
    engine
        .invoke(ids::IMPACT_INSPECT, CommandArgs::Path(nested))
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    assert!(!engine.impact_stale());
}

/// #100's question: before a page is moved or deleted, what would break: the references to it
/// (with where they are written), the navigation that lists it, and the pages that include it.
#[test]
fn inspecting_a_page_lists_what_removing_it_would_break() {
    let (_directory, root, mut engine) = fixture();
    engine
        .invoke(ids::IMPACT_INSPECT, CommandArgs::Path(root.join(INSTALL)))
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    let report = ready(&engine);
    let broken: Vec<(&str, Option<&str>)> = report
        .impacts
        .iter()
        .filter(|impact| impact.kind == ImpactKind::ReferenceIntegrity)
        .map(|impact| {
            assert_eq!(impact.status, Some(ImpactStatus::Broken), "{impact:?}");
            assert!(impact.source.is_some(), "where it is written: {impact:?}");
            (impact.path.as_str(), impact.anchor.as_deref())
        })
        .collect();
    assert!(
        broken.contains(&("v1/modules/ROOT/pages/guide.adoc", None)),
        "{broken:?}"
    );
    assert!(
        broken
            .iter()
            .any(|(path, _)| *path == "v1/modules/ROOT/pages/index.adoc"),
        "{broken:?}"
    );
    assert!(report
        .impacts
        .iter()
        .any(|impact| impact.kind == ImpactKind::Navigation));
    // A reference opens where it is written.
    let guide = report
        .impacts
        .iter()
        .find(|impact| impact.path == "v1/modules/ROOT/pages/guide.adoc")
        .unwrap();
    let place = engine.impact_place(guide).unwrap();
    let text = fs::read_to_string(root.join("v1/modules/ROOT/pages/guide.adoc")).unwrap();
    assert_eq!(place.path, root.join("v1/modules/ROOT/pages/guide.adoc"));
    assert!(
        text[place.offset..].starts_with("xref:install.adoc"),
        "{}",
        &text[place.offset..]
    );
    assert!(report
        .impacts
        .iter()
        .all(|impact| impact.evidence != EvidenceSide::Head || impact.source.is_some()));
}

#[test]
fn an_impact_s_path_becomes_graph_steps_in_the_links_direction() {
    let (_directory, root, mut engine) = fixture();
    engine
        .invoke(ids::IMPACT_INSPECT, CommandArgs::Path(root.join(NESTED)))
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    let report = ready(&engine).clone();
    let impact = report
        .impacts
        .iter()
        .find(|impact| impact.kind == ImpactKind::Render && impact.via.len() == 3)
        .unwrap();
    let steps = engine.impact_trail(impact);
    assert_eq!(steps.len(), 3);
    assert_eq!(
        steps[0].key,
        GraphKey::File(std::sync::Arc::from(root.join(NESTED).as_path()))
    );
    // The partial includes the nested one, and the page the partial: each link runs from the
    // step to the one before.
    for step in &steps[1..] {
        assert_eq!(step.kinds, [EdgeKind::Include], "{step:?}");
        assert!(step.backwards, "{step:?}");
    }
}

#[test]
fn a_newer_request_supersedes_one_running_and_failures_are_kept() {
    let (_directory, root, mut engine) = fixture();
    engine
        .invoke(ids::IMPACT_INSPECT, CommandArgs::Path(root.join(NESTED)))
        .unwrap();
    // The copy is in no repository: the working tree has no `HEAD` to compare with.
    engine
        .invoke(ids::IMPACT_ANALYSE_CHANGES, CommandArgs::None)
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    let inspection = engine.impact_inspection().unwrap();
    assert_eq!(inspection.source, ChangeSource::WorkingTree);
    assert!(
        matches!(&inspection.state, InspectionState::Failed(message) if message.contains("Git")),
        "{:?}",
        inspection.state
    );
    assert!(engine
        .invoke(
            ids::IMPACT_INSPECT,
            CommandArgs::Path(PathBuf::from("/elsewhere/page.adoc"))
        )
        .is_ok());
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    assert!(matches!(
        &engine.impact_inspection().unwrap().state,
        InspectionState::Failed(message) if message.contains("outside")
    ));
}
