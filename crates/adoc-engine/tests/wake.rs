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

/// A preview waiting for a parse on a worker has no deadline: the worker's message wakes the
/// front end, which would otherwise pump at every tick until it came.
#[test]
fn a_preview_waiting_for_its_parse_is_not_due() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nSome words.\n").unwrap();
    let (mut engine, clock) = manual(plain());
    let id = engine.open(&page).unwrap();
    engine.set_active(id);
    engine.request_preview(id);
    pump_at_each_deadline(&mut engine, &clock);

    insert(&mut engine, id, "x");
    clock.advance(PREVIEW_SETTLE);
    // The settle starts the page's line parse, which the preview's input reads, and waits.
    engine.pump();
    assert!(
        engine.next_due().is_none_or(|due| due > clock.now()),
        "{:?} at {:?}",
        engine.next_due(),
        clock.now()
    );
    assert!(engine.has_pending(), "the worker is still counted");
    // Its message brings the preview back, and the pumps that follow finish it.
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    pump_at_each_deadline(&mut engine, &clock);
    let version = engine.document(id).unwrap().version;
    assert_eq!(engine.rendered_preview(id).unwrap().version, version);
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

/// A page whose line changes wait for its working tree's job has no deadline until the job is in.
#[test]
fn line_changes_waiting_for_their_working_tree_are_not_due() {
    let directory = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(directory.path());
    // Long enough that its diff is still running when the second page is due.
    test.write("long.txt", &"A line of the long file.\n".repeat(200_000));
    test.write("notes.txt", "Notes.\n");
    test.commit_all("first");
    let (mut engine, clock) = manual(Settings::load(None, None).unwrap());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let long = engine.open(&test.path().join("long.txt")).unwrap();
    let notes = engine.open(&test.path().join("notes.txt")).unwrap();
    settle(&mut engine);
    pump_at_each_deadline(&mut engine, &clock);

    insert(&mut engine, long, "x");
    clock.advance(adoc_engine::GIT_SETTLE);
    engine.pump();
    insert(&mut engine, notes, "x");
    clock.advance(adoc_engine::GIT_SETTLE);
    engine.pump();
    assert!(
        engine.next_due().is_none_or(|due| due > clock.now()),
        "{:?} at {:?}",
        engine.next_due(),
        clock.now()
    );
    pump_at_each_deadline(&mut engine, &clock);
    assert!(!engine.git().hunks_for(notes).is_empty());
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

/// A host's own change to a shared file is due at once, to be sent (COLLAB 4/8); once sent,
/// nothing is due. A document shared outside the session is never due.
#[cfg(feature = "collab")]
#[test]
fn a_shared_change_to_send_is_due_at_once() {
    use std::{collections::VecDeque, sync::Mutex};

    use adoc_engine::{
        protocol::{ErrorCode, Message, PeerId},
        Link, LinkError, LinkEvent,
    };

    /// A link that hands over the events queued, and accepts everything sent.
    struct Scripted(Arc<Mutex<VecDeque<LinkEvent>>>);
    impl Link for Scripted {
        fn try_recv(&mut self) -> Option<LinkEvent> {
            self.0.lock().unwrap().pop_front()
        }
        fn send(&mut self, _: PeerId, _: Message) -> Result<(), LinkError> {
            Ok(())
        }
        fn admit(&mut self, _: PeerId) -> Result<(), LinkError> {
            Ok(())
        }
        fn remove(&mut self, _: PeerId, _: ErrorCode, _: &str) {}
    }

    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("page.adoc"), "= Page\n").unwrap();
    fs::write(directory.path().join("other.adoc"), "= Other\n").unwrap();
    let mut engine = Engine::new(plain());
    let peer = PeerId(1);
    let events = Arc::new(Mutex::new(VecDeque::from([LinkEvent::Knock {
        peer,
        name: "guest".to_owned(),
    }])));
    engine
        .host_session(
            Box::new(Scripted(Arc::clone(&events))),
            directory.path(),
            &["page.adoc".into()],
            "Host",
        )
        .unwrap();
    engine.pump();
    engine.admit_peer(peer).unwrap();
    // Who is in the session is due to be told at once, then nothing.
    assert!(engine.next_due().is_some_and(|due| due <= Instant::now()));
    engine.pump();
    assert_eq!(engine.next_due(), None);

    // Shared outside the session: its changes are nobody's to send, and never due.
    let other = engine.open(&directory.path().join("other.adoc")).unwrap();
    engine
        .share_document(
            other,
            adoc_engine::SharedFileId(9),
            adoc_engine::ReplicaId::new(9).unwrap(),
        )
        .unwrap();
    insert(&mut engine, other, "Text.\n");
    settle(&mut engine);
    assert_eq!(engine.next_due(), None);
    engine.close(other).unwrap();

    // Opened by the guest through the session, the page's changes are due at once.
    events.lock().unwrap().push_back(LinkEvent::Message {
        peer,
        message: Message::OpenFile { file: 1 },
    });
    engine.pump();
    let page = engine.session_document(1).unwrap();
    settle(&mut engine);
    assert_eq!(engine.next_due(), None);
    insert(&mut engine, page, "Text.\n");
    assert!(engine.next_due().is_some_and(|due| due <= Instant::now()));
    engine.pump();
    assert!(engine.take_shared_updates(page).is_empty());
}

/// A participant's move to another shared page is due at once, to be told (#767): the host's
/// own place is in the roster its guests are sent.
#[cfg(feature = "collab")]
#[test]
fn a_move_to_another_shared_page_is_due_at_once() {
    use std::{collections::VecDeque, sync::Mutex};

    use adoc_engine::{
        protocol::{ErrorCode, Message, PeerId},
        Link, LinkError, LinkEvent,
    };

    struct Scripted(Arc<Mutex<VecDeque<LinkEvent>>>);
    impl Link for Scripted {
        fn try_recv(&mut self) -> Option<LinkEvent> {
            self.0.lock().unwrap().pop_front()
        }
        fn send(&mut self, _: PeerId, _: Message) -> Result<(), LinkError> {
            Ok(())
        }
        fn admit(&mut self, _: PeerId) -> Result<(), LinkError> {
            Ok(())
        }
        fn remove(&mut self, _: PeerId, _: ErrorCode, _: &str) {}
    }

    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("one.adoc"), "= One\n").unwrap();
    fs::write(directory.path().join("two.adoc"), "= Two\n").unwrap();
    let mut engine = Engine::new(plain());
    let peer = PeerId(1);
    let events = Arc::new(Mutex::new(VecDeque::from([LinkEvent::Knock {
        peer,
        name: "guest".to_owned(),
    }])));
    engine
        .host_session(
            Box::new(Scripted(Arc::clone(&events))),
            directory.path(),
            &["one.adoc".into(), "two.adoc".into()],
            "Host",
        )
        .unwrap();
    engine.pump();
    engine.admit_peer(peer).unwrap();
    let one = engine.open(&directory.path().join("one.adoc")).unwrap();
    let two = engine.open(&directory.path().join("two.adoc")).unwrap();
    settle(&mut engine);
    assert_eq!(engine.next_due(), None);

    engine.set_active(one);
    assert!(engine.next_due().is_some_and(|due| due <= Instant::now()));
    engine.pump();
    assert_eq!(engine.next_due(), None);
    engine.set_active(two);
    assert!(engine.next_due().is_some_and(|due| due <= Instant::now()));
    engine.pump();
    assert_eq!(engine.next_due(), None);
}

#[test]
fn a_comment_for_a_guest_to_be_told_is_due_at_once() {
    use std::{collections::VecDeque, sync::Mutex};

    use adoc_engine::{
        protocol::{ErrorCode, Message, PeerId},
        Link, LinkError, LinkEvent,
    };

    struct Scripted(Arc<Mutex<VecDeque<LinkEvent>>>, Arc<Mutex<Vec<Message>>>);
    impl Link for Scripted {
        fn try_recv(&mut self) -> Option<LinkEvent> {
            self.0.lock().unwrap().pop_front()
        }
        fn send(&mut self, _: PeerId, message: Message) -> Result<(), LinkError> {
            self.1.lock().unwrap().push(message);
            Ok(())
        }
        fn admit(&mut self, _: PeerId) -> Result<(), LinkError> {
            Ok(())
        }
        fn remove(&mut self, _: PeerId, _: ErrorCode, _: &str) {}
    }

    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("one.adoc"), "= One\n\nSome text.\n").unwrap();
    let (mut engine, clock) = manual(plain());
    let peer = PeerId(1);
    let events = Arc::new(Mutex::new(VecDeque::from([LinkEvent::Knock {
        peer,
        name: "guest".to_owned(),
    }])));
    let sent = Arc::new(Mutex::new(Vec::new()));
    engine
        .host_session(
            Box::new(Scripted(Arc::clone(&events), Arc::clone(&sent))),
            directory.path(),
            &["one.adoc".into()],
            "Host",
        )
        .unwrap();
    engine.pump();
    engine.admit_peer(peer).unwrap();
    events.lock().unwrap().push_back(LinkEvent::Message {
        peer,
        message: Message::OpenFile { file: 1 },
    });
    run_out(&mut engine, &clock);
    let id = engine.session_document(1).unwrap();
    engine
        .invoke(
            ids::COMMENTS_ADD,
            CommandArgs::Comment(adoc_engine::CommentRequest::Add {
                id,
                range: adoc_core::SourceRange::new(7, 11),
                text: "Some?".to_owned(),
            }),
        )
        .unwrap();
    assert_eq!(engine.next_due(), Some(clock.now()), "told at once");
    engine.pump();
    assert!(sent
        .lock()
        .unwrap()
        .iter()
        .any(|message| matches!(message, Message::CommentThread { .. })));
    assert_eq!(
        engine.next_due(),
        Some(clock.now() + adoc_engine::COMMENT_SETTLE),
        "only its write waits"
    );
}

/// A page with a comment on "commented", open in a workspace of its own.
fn commented(clock_settings: Settings) -> (tempfile::TempDir, Engine, ManualClock, DocumentId) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    let page = root.join("page.adoc");
    fs::write(&page, "= Page\n\nSome commented text.\n").unwrap();
    let (mut engine, clock) = manual(clock_settings);
    engine.open_workspace(vec![root]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&page).unwrap();
    run_out(&mut engine, &clock);
    let start = "= Page\n\nSome ".len();
    engine
        .invoke(
            ids::COMMENTS_ADD,
            CommandArgs::Comment(adoc_engine::CommentRequest::Add {
                id,
                range: adoc_core::SourceRange::new(start, start + "commented".len()),
                text: "Why?".to_owned(),
            }),
        )
        .unwrap();
    (directory, engine, clock, id)
}

/// Pumps at each deadline until nothing is due. A change on disk's deadline (the watcher seeing
/// the comments file written) is by the system's time, which the manual clock does not move, so
/// that one is waited for.
fn run_out(engine: &mut Engine, clock: &ManualClock) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let Some(due) = engine.next_due() else {
            return;
        };
        if due <= clock.now() && engine.pump() == 0 {
            thread::sleep(Duration::from_millis(20));
        }
        clock.advance(due.saturating_duration_since(clock.now()));
        engine.pump();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
    }
    panic!("a deadline the pump does not act on");
}

#[test]
fn a_comment_change_is_due_after_its_settle() {
    let (_directory, mut engine, clock, _) = commented(plain());
    assert_eq!(
        engine.next_due(),
        Some(clock.now() + adoc_engine::COMMENT_SETTLE)
    );
    run_out(&mut engine, &clock);
    assert_eq!(engine.next_due(), None);
}

#[test]
fn a_detached_comment_is_due_once_typing_pauses() {
    let (_directory, mut engine, clock, id) = commented(plain());
    run_out(&mut engine, &clock);
    let start = "= Page\n\nSome ".len();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(start, start + "commented ".len()),
                text: String::new(),
            },
        )
        .unwrap();
    assert!(engine.comments(id)[0].detached);
    let deleted = clock.now();
    assert!(engine
        .next_due()
        .is_some_and(|due| due <= deleted + INDEX_SETTLE));
    run_out(&mut engine, &clock);
    assert_eq!(
        engine.next_due(),
        None,
        "a search that finds nothing is not due again"
    );
    assert!(engine.comments(id)[0].detached);
}
