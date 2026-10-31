//! The workspace's problems, for a Problems panel: every file's diagnostics with their line and
//! column, computed by the engine, the whole workspace on a worker and, after an edit, only the
//! files it can change.

use std::{
    fs,
    path::Path,
    sync::{mpsc::Receiver, Arc},
    time::{Duration, Instant},
};

use adoc_core::{DiagnosticCode, SourceRange};
use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, Engine, EngineEvent, ManualClock, Settings,
    INDEX_SETTLE,
};

fn engine_with_problems() -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let settings = Settings::load(None, None).unwrap().with_git_opt_in();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine.enable_problems();
    (engine, clock)
}

/// Pumps until `ProblemsChanged` arrives, or ten seconds pass.
fn until_problems_changed(engine: &mut Engine, events: &Receiver<EngineEvent>) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        engine.pump();
        if events
            .try_iter()
            .any(|event| event == EngineEvent::ProblemsChanged)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn codes_in(engine: &Engine, path: &Path) -> Vec<DiagnosticCode> {
    engine
        .problems()
        .get(&normalize_path(path))
        .map(|problems| problems.iter().map(|problem| problem.code).collect())
        .unwrap_or_default()
}

#[test]
fn problems_are_computed_off_the_engine_thread() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for page in 0..200 {
        fs::write(
            root.join(format!("page-{page}.adoc")),
            format!("= Page {page}\n\nSee xref:missing-{page}.adoc[].\n"),
        )
        .unwrap();
    }
    // A character of two bytes before the problem: its column counts characters.
    fs::write(root.join("café.adoc"), "= Café\n\nÉté: <<nowhere>>.\n").unwrap();
    let (mut engine, _clock) = engine_with_problems();
    let events = engine.subscribe();
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));

    // Indexed, but the problems are still to come: a worker computes them.
    assert!(engine.problems().is_empty());
    assert!(engine.has_pending());
    assert!(until_problems_changed(&mut engine, &events));
    assert_eq!(engine.problems().len(), 201);
    assert_eq!(engine.problem_count(), 201);

    let cafe = &engine.problems()[&normalize_path(&root.join("café.adoc"))];
    assert_eq!(cafe.len(), 1);
    assert_eq!(cafe[0].code, DiagnosticCode::UnresolvedAnchor);
    assert_eq!((cafe[0].line, cafe[0].column), (3, 6));
    assert_eq!(cafe[0].message, "Unresolved AsciiDoc anchor: nowhere");
    let start = "= Café\n\nÉté: ".len();
    assert_eq!(
        cafe[0].range,
        SourceRange::new(start, start + "<<nowhere>>".len())
    );
}

#[test]
fn problems_recompute_only_referencing_pages() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("target.adoc"), "= Target\n\nNo anchor yet.\n").unwrap();
    // Not open: the problems are the workspace's, not the open pages'.
    fs::write(
        root.join("linker.adoc"),
        "= Linker\n\nSee xref:target.adoc#later[].\n",
    )
    .unwrap();
    fs::write(
        root.join("other.adoc"),
        "= Other\n\nSee xref:missing.adoc[].\n",
    )
    .unwrap();
    let (mut engine, clock) = engine_with_problems();
    let events = engine.subscribe();
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    assert!(until_problems_changed(&mut engine, &events));
    assert_eq!(
        codes_in(&engine, &root.join("linker.adoc")),
        [DiagnosticCode::UnresolvedAnchor]
    );
    let other = engine.problems()[&normalize_path(&root.join("other.adoc"))].clone();

    let target = engine.open(&root.join("target.adoc")).unwrap();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id: target,
                range: SourceRange::new(10, 10),
                text: "[[later]]\n".to_owned(),
            },
        )
        .unwrap();
    // Typing waits for its pause, and the problems with it.
    assert!(engine.has_pending());
    clock.advance(INDEX_SETTLE);
    assert!(until_problems_changed(&mut engine, &events));
    assert!(!engine
        .problems()
        .contains_key(&normalize_path(&root.join("linker.adoc"))));
    assert_eq!(
        engine.problems()[&normalize_path(&root.join("other.adoc"))],
        other
    );
    assert_eq!(engine.problem_count(), 1);
}

#[test]
fn a_front_end_that_shows_no_problems_computes_none() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("page.adoc"),
        "= Page\n\nSee <<nowhere>>.\n",
    )
    .unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    assert!(!engine.has_pending());
    assert!(engine.problems().is_empty());
}
