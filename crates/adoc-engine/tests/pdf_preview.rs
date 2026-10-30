//! Issue #62: the live PDF preview typesets the preview target as the export would, on a
//! worker, after each settled change.

use std::{
    fs,
    sync::{mpsc::Receiver, Arc},
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, EngineEvent, ManualClock, Settings,
    PREVIEW_SETTLE,
};

const WAIT: Duration = Duration::from_secs(60);

fn engine_with_clock() -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let engine = Engine::with_clock(
        Settings::load(None, None).unwrap().with_git_opt_in(),
        Arc::new(clock.clone()),
    );
    (engine, clock)
}

fn pdf_ready(events: &Receiver<EngineEvent>, id: DocumentId) -> Vec<u64> {
    events
        .try_iter()
        .filter_map(|event| match event {
            EngineEvent::PdfPreviewReady { id: ready, version } if ready == id => Some(version),
            _ => None,
        })
        .collect()
}

/// Pumps until the engine has nothing left to do.
fn settle(engine: &mut Engine) {
    loop {
        engine.pump();
        assert!(engine.wait_for_workers(WAIT));
        if !engine.is_typesetting_preview() && engine.pump() == 0 {
            break;
        }
    }
}

#[test]
fn the_pdf_preview_follows_the_page() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nFirst paragraph.\n").unwrap();
    let (mut engine, clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();

    // Nothing is typeset until the preview shows the PDF.
    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    clock.advance(PREVIEW_SETTLE);
    settle(&mut engine);
    assert!(engine.pdf_preview(id).is_none());
    assert!(pdf_ready(&events, id).is_empty());

    engine
        .invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None)
        .unwrap();
    let layout = engine.preview_layout();
    assert!(layout.visible && layout.pdf);
    settle(&mut engine);
    assert_eq!(pdf_ready(&events, id), [0]);
    let preview = engine.pdf_preview(id).unwrap();
    let pages = preview.pages.as_ref().unwrap();
    assert_eq!(pages.len(), 1);
    assert!(pages[0].svg.starts_with("<svg"));
    let first = "= Page\n\n".len();
    assert!(
        pages[0]
            .anchors
            .iter()
            .any(|anchor| anchor.path == page && anchor.offset == first),
        "{:?}",
        pages[0].anchors
    );

    // Typing returns at once; the pages follow once the page settles, typeset on a worker.
    let end = "= Page\n\nFirst paragraph.\n".len();
    let started = Instant::now();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(end, end),
                text: "\n<<<\n\nA second page.\n".to_owned(),
            },
        )
        .unwrap();
    clock.advance(PREVIEW_SETTLE);
    engine.pump();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the edit waited {:?}",
        started.elapsed()
    );
    settle(&mut engine);
    eprintln!("edit to PDF preview: {:?}", started.elapsed());
    assert_eq!(pdf_ready(&events, id), [1]);
    let preview = engine.pdf_preview(id).unwrap();
    assert_eq!(preview.version, 1);
    assert_eq!(preview.pages.as_ref().unwrap().len(), 2);

    // Back to the rendered page: no more typesetting.
    engine
        .invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None)
        .unwrap();
    assert!(!engine.preview_layout().pdf);
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: "x".to_owned(),
            },
        )
        .unwrap();
    clock.advance(PREVIEW_SETTLE);
    settle(&mut engine);
    assert!(pdf_ready(&events, id).is_empty());
}

#[test]
fn a_page_that_cannot_be_typeset_says_why() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n:pdf-theme: missing-theme\n\nText.\n").unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();
    engine
        .invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None)
        .unwrap();
    // The first preview of a page is rendered at once.
    settle(&mut engine);
    assert_eq!(pdf_ready(&events, id), [0]);
    let error = engine.pdf_preview(id).unwrap().pages.clone().unwrap_err();
    assert!(error.contains("missing-theme"), "{error}");
}
