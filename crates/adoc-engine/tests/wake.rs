//! A front end sleeps until the engine has work (#222): `Engine::next_due` gives the time of the
//! next thing a pump would do, and the waker is called whenever a worker, the writer or the
//! file watcher sends the engine something.

use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use adoc_engine::{
    commands::ids, AutoSave, Clock, CommandArgs, DocumentId, Engine, ExportArgs, ExportTarget,
    ManualClock, Settings, SidebarPanel, AUTO_SAVE_DELAY, EXPANSION_SAVE_SETTLE, INDEX_SETTLE,
    MODEL_SETTLE, PREVIEW_SETTLE, RECOVERY_SETTLE, SPELLING_SETTLE, WORD_COUNT_SETTLE,
};

fn insert(engine: &mut Engine, id: DocumentId, text: &str) {
    let end = engine.document(id).unwrap().text.len();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(end, end),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

/// A waker that counts its calls.
fn counting(engine: &mut Engine) -> Arc<AtomicUsize> {
    let woken = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&woken);
    engine.set_waker(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    woken
}

/// Waits, without pumping, until `woken` has counted a call (at most ten seconds).
fn woken_without_pumping(woken: &AtomicUsize) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if woken.load(Ordering::SeqCst) > 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Pumps until the engine has had nothing to do for 300 ms: what the watcher saw of the test's
/// own setup is applied first.
fn settle(engine: &mut Engine) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut quiet = Instant::now();
    while quiet.elapsed() < Duration::from_millis(300) && Instant::now() < deadline {
        if engine.pump() > 0 || engine.has_pending() {
            assert!(engine.wait_for_workers(Duration::from_secs(10)));
            quiet = Instant::now();
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn manual(settings: Settings) -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    (Engine::with_clock(settings, Arc::new(clock.clone())), clock)
}

#[test]
fn an_idle_engine_has_nothing_due() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let (mut engine, _) = manual(Settings::load(None, None).unwrap().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    engine.set_active(id);
    engine.pump();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    assert_eq!(engine.next_due(), None);
    assert!(!engine.has_pending());
}

#[test]
fn typing_is_due_when_the_index_settles() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let (mut engine, clock) = manual(Settings::load(None, None).unwrap().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    engine.pump();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();

    let typed = clock.now();
    insert(&mut engine, id, "x");
    assert_eq!(engine.next_due(), Some(typed + INDEX_SETTLE));
    assert!(engine.has_pending());
    // Typing again moves the deadline with it.
    clock.advance(Duration::from_millis(40));
    insert(&mut engine, id, "y");
    assert_eq!(
        engine.next_due(),
        Some(typed + Duration::from_millis(40) + INDEX_SETTLE)
    );
}

/// Every deadline `next_due` gives is one a pump acts on, and pumping at each in turn leaves
/// every settle done: nothing is left waiting that `next_due` did not name.
#[test]
fn pumping_at_each_deadline_settles_everything() {
    let directory = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSome words.\n").unwrap();
    let mut settings = Settings::load(None, None)
        .unwrap()
        .with_git_opt_in()
        .with_state_dir(Some(state.path().to_path_buf()));
    settings.editor.auto_save = AutoSave::AfterDelay;
    let (mut engine, clock) = manual(settings);
    engine.enable_word_count();
    engine.enable_spelling();
    engine.enable_recovery();
    engine.enable_problems();
    let id = engine.open(&page).unwrap();
    engine.set_active(id);
    engine.request_preview(id);
    engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
        .unwrap();
    insert(&mut engine, id, "Mispelt wurds.\n");

    let mut steps = 0;
    while let Some(due) = engine.next_due() {
        steps += 1;
        assert!(steps < 50, "a deadline the pump does not act on");
        clock.advance(due.saturating_duration_since(clock.now()));
        engine.pump();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
    }
    engine.pump();
    assert!(!engine.has_pending());
    assert_eq!(engine.next_due(), None);

    assert!(
        fs::read_to_string(&page).unwrap().contains("Mispelt"),
        "auto-saved"
    );
    assert!(engine
        .workspace()
        .index
        .file(&adoc_engine::normalize_path(&page))
        .is_some_and(|file| file.document.text.contains("Mispelt")));
    assert!(engine.word_count(id).is_some());
    assert!(!engine.misspellings(id).is_empty());
    assert!(engine.rendered_preview(id).is_some());
}

/// The times after an edit, in order, at which `next_due` says a pump has something to do, with
/// the engine as `prepare` leaves it and nothing else going on. A later deadline's pump would do
/// whatever an earlier one was due to, so each settle is checked alone.
fn deadlines_after_typing(
    settings: Settings,
    prepare: impl FnOnce(&mut Engine, DocumentId),
) -> Vec<Duration> {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSome words.\n").unwrap();
    let (mut engine, clock) = manual(settings);
    let id = engine.open(&page).unwrap();
    engine.set_active(id);
    prepare(&mut engine, id);
    let run = |engine: &mut Engine, seen: &mut Vec<Duration>, from: Instant| {
        for _ in 0..50 {
            let Some(due) = engine.next_due() else {
                return;
            };
            clock.advance(due.saturating_duration_since(clock.now()));
            let at = due.saturating_duration_since(from);
            if seen.last() != Some(&at) {
                seen.push(at);
            }
            engine.pump();
            assert!(engine.wait_for_workers(Duration::from_secs(10)));
        }
        panic!("a deadline the pump does not act on");
    };
    let start = clock.now();
    run(&mut engine, &mut Vec::new(), start);

    let typed = clock.now();
    insert(&mut engine, id, "x");
    let mut seen = Vec::new();
    run(&mut engine, &mut seen, typed);
    seen.retain(|at| !at.is_zero());
    seen
}

fn plain() -> Settings {
    Settings::load(None, None).unwrap().with_git_opt_in()
}

#[test]
fn each_settle_is_due_at_its_own_deadline() {
    assert_eq!(deadlines_after_typing(plain(), |_, _| ()), [INDEX_SETTLE]);
    assert_eq!(
        deadlines_after_typing(plain(), |engine, _| engine.enable_word_count()),
        [INDEX_SETTLE, WORD_COUNT_SETTLE]
    );
    assert_eq!(
        deadlines_after_typing(plain(), |engine, _| engine.enable_spelling()),
        [INDEX_SETTLE, SPELLING_SETTLE]
    );
    // The first to settle starts the page's one parse, which is the index's too (#401), so the
    // index has no deadline of its own left.
    assert_eq!(
        deadlines_after_typing(plain(), |engine, id| engine.request_preview(id)),
        [PREVIEW_SETTLE]
    );
    assert_eq!(
        deadlines_after_typing(plain(), |engine, id| {
            engine
                .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
                .unwrap();
        }),
        [MODEL_SETTLE]
    );
    let mut saving = plain();
    saving.editor.auto_save = AutoSave::AfterDelay;
    assert_eq!(
        deadlines_after_typing(saving, |_, _| ()),
        [INDEX_SETTLE, AUTO_SAVE_DELAY]
    );
    let state = tempfile::tempdir().unwrap();
    let recording = plain().with_state_dir(Some(state.path().to_path_buf()));
    assert_eq!(
        deadlines_after_typing(recording, |engine, _| engine.enable_recovery()),
        [INDEX_SETTLE, RECOVERY_SETTLE]
    );
}

#[test]
fn a_chosen_branch_is_due_when_it_settles() {
    let (mut engine, clock) = manual(plain());
    let chosen = clock.now();
    engine.set_branch_expanded(SidebarPanel::Modules, "demo@1.0", true);
    assert_eq!(engine.next_due(), Some(chosen + EXPANSION_SAVE_SETTLE));
    clock.advance(EXPANSION_SAVE_SETTLE);
    engine.pump();
    assert_eq!(engine.next_due(), None);
}

/// Work that waits only for the next pump is due at once, and the pump does it.
#[test]
fn problems_to_diagnose_are_due_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let (mut engine, clock) = manual(plain());
    let id = engine.open(&page).unwrap();
    engine.enable_problems();
    assert_eq!(engine.next_due(), Some(clock.now()));

    engine.pump();
    assert_eq!(engine.next_due(), None, "its worker's result comes first");
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine.pump();
    assert_eq!(engine.next_due(), None);

    // More than a character is indexed at once, which leaves the problems to the next pump.
    insert(&mut engine, id, "\nxref:missing.adoc[]\n");
    assert_eq!(engine.next_due(), Some(clock.now()));
    engine.pump();
    assert_eq!(engine.next_due(), None);
    assert!(engine.problem_count() > 0);
}

/// Pumps at each deadline until nothing is due and no worker is running, whose result would wake
/// a front end.
fn pump_at_each_deadline(engine: &mut Engine, clock: &ManualClock) {
    for _ in 0..50 {
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        let Some(due) = engine.next_due() else {
            return;
        };
        clock.advance(due.saturating_duration_since(clock.now()));
        engine.pump();
    }
    panic!("a deadline the pump does not act on");
}

#[test]
fn an_audit_to_build_is_due_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let (mut engine, clock) = manual(plain());
    engine.open(&page).unwrap();
    engine.enable_audit();
    assert_eq!(engine.next_due(), Some(clock.now()));
    pump_at_each_deadline(&mut engine, &clock);
    assert!(!engine.audit_stale());
}

/// A page indexed at once leaves the graph to the next pump.
#[test]
fn a_graph_to_update_is_due_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let (mut engine, clock) = manual(plain());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&page).unwrap();
    settle(&mut engine);
    insert(&mut engine, id, "\nxref:other.adoc[]\n");
    assert_eq!(engine.next_due(), Some(clock.now()));
}

/// An export waits for its page's preview, then is due at once.
#[test]
fn an_html_export_is_due_once_its_preview_has_finished() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let output = directory.path().join("page.html");
    let (mut engine, clock) = manual(plain());
    let id = engine.open(&page).unwrap();
    engine
        .invoke(
            ids::EXPORT_HTML_PAGE,
            CommandArgs::Export(ExportArgs {
                target: ExportTarget::Page(id),
                output: output.clone(),
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
                attributes: Vec::new(),
            }),
        )
        .unwrap();
    pump_at_each_deadline(&mut engine, &clock);
    assert!(output.is_file());
}

/// A file that is not AsciiDoc has no index settle, which falls at the same time.
#[test]
fn line_changes_to_compute_are_due_when_typing_pauses() {
    let directory = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(directory.path());
    test.write("notes.txt", "Notes.\n");
    test.commit_all("first");
    let notes = test.path().join("notes.txt");
    let settings = Settings::load(None, None).unwrap();
    let (mut engine, clock) = manual(settings);
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&notes).unwrap();
    settle(&mut engine);
    pump_at_each_deadline(&mut engine, &clock);

    let typed = clock.now();
    insert(&mut engine, id, "x");
    assert_eq!(engine.next_due(), Some(typed + adoc_engine::GIT_SETTLE));
    pump_at_each_deadline(&mut engine, &clock);
    assert!(!engine.git().hunks_for(id).is_empty());
}

#[test]
fn a_worker_result_wakes_the_front_end() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("page.adoc"), "= Page\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    let woken = counting(&mut engine);
    // The index runs on a worker, whose result is sent to the engine.
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(woken_without_pumping(&woken));
}

#[test]
fn a_change_on_disk_wakes_the_front_end() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    let woken = counting(&mut engine);
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    settle(&mut engine);
    woken.store(0, Ordering::SeqCst);

    fs::write(&page, "= Page, changed\n").unwrap();
    assert!(woken_without_pumping(&woken));
    // The change waits out the watcher's debounce, which `next_due` names.
    engine.pump();
    let due = engine.next_due().expect("the change is due");
    assert!(due <= Instant::now() + adoc_engine::WATCH_DEBOUNCE);
}
