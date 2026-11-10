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
    // Counted per thread: typesetting on this one would count here, however busy the machine.
    adoc_core::probe::reset();
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
    settle(&mut engine);
    eprintln!("edit to PDF preview: {:?}", started.elapsed());
    // Neither the edit nor any pump that followed typeset the page on this thread.
    assert_eq!(
        adoc_core::probe::count(adoc_core::probe::Probe::PreviewTypeset),
        0,
        "typeset on the caller's thread"
    );
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

/// Issue #188: the preview keeps its theme between typesettings, and a change to a theme file,
/// here one the theme extends, shows on the next.
#[test]
fn a_theme_change_shows_on_the_next_preview() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("house-base-theme.yml");
    fs::write(&base, "extends: default\npage:\n  size: A4\n").unwrap();
    fs::write(
        directory.path().join("house-theme.yml"),
        "extends: house-base\nbase:\n  font-color: 222222\n",
    )
    .unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(
        &page,
        "= Page\n:pdf-themesdir: .\n:pdf-theme: house\n\nText.\n",
    )
    .unwrap();
    let (mut engine, clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();
    engine
        .invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None)
        .unwrap();
    settle(&mut engine);
    assert_eq!(pdf_ready(&events, id), [0]);
    let width = |engine: &Engine| {
        let preview = engine.pdf_preview(id).unwrap();
        let pages = preview
            .pages
            .as_ref()
            .unwrap_or_else(|error| panic!("{error}"));
        pages[0].width
    };
    let a4 = width(&engine);

    let edit = |engine: &mut Engine, offset: usize| {
        engine
            .invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(offset, offset),
                    text: "x".to_owned(),
                },
            )
            .unwrap();
        clock.advance(PREVIEW_SETTLE);
        settle(engine);
    };
    let end = fs::read_to_string(&page).unwrap().len();
    edit(&mut engine, end);
    assert_eq!(pdf_ready(&events, id), [1]);
    assert_eq!(width(&engine), a4, "the same theme");

    fs::write(
        &base,
        "extends: default\npage:\n  size: A5 # a size of its own\n",
    )
    .unwrap();
    edit(&mut engine, end);
    assert_eq!(pdf_ready(&events, id), [2]);
    assert!(width(&engine) < a4, "A5 is narrower than A4");
}

/// Issue #420: what a PDF command opened, turning the PDF off closes again.
fn layout_after(engine: &mut Engine, command: adoc_engine::CommandId) -> (bool, bool, bool) {
    engine.invoke(command, CommandArgs::None).unwrap();
    let layout = engine.preview_layout();
    (layout.visible, layout.detached, layout.pdf)
}

#[test]
fn turning_the_pdf_off_closes_the_preview_it_opened() {
    let (mut engine, _clock) = engine_with_clock();
    // (visible, detached, pdf)
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_PREVIEW),
        (true, false, true)
    );
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_PREVIEW),
        (false, false, false)
    );
}

#[test]
fn turning_the_pdf_off_shows_the_rendered_page_it_replaced() {
    let (mut engine, _clock) = engine_with_clock();
    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_PREVIEW),
        (true, false, true)
    );
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_PREVIEW),
        (true, false, false)
    );
}

#[test]
fn the_pdf_window_opens_and_closes_its_own_window() {
    let (mut engine, _clock) = engine_with_clock();
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_WINDOW),
        (true, true, true)
    );
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_WINDOW),
        (false, false, false)
    );

    // Beside a docked rendered page, the window leaves it docked again.
    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_WINDOW),
        (true, true, true)
    );
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_WINDOW),
        (true, false, false)
    );
}

#[test]
fn the_pdf_window_takes_a_docked_pdf_and_gives_back_what_was_before_it() {
    let (mut engine, _clock) = engine_with_clock();
    engine
        .invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None)
        .unwrap();
    // The docked PDF moves into its own window.
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_WINDOW),
        (true, true, true)
    );
    // The preview was closed before the PDF opened it.
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_PREVIEW),
        (false, false, false)
    );
}

#[test]
fn a_pdf_shown_after_the_preview_was_closed_opens_it_again() {
    let (mut engine, _clock) = engine_with_clock();
    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    engine
        .invoke(ids::VIEW_TOGGLE_PDF_PREVIEW, CommandArgs::None)
        .unwrap();
    // Closed by its own close button while the PDF showed.
    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_PREVIEW),
        (true, false, true)
    );
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PDF_PREVIEW),
        (false, false, false)
    );
}

/// Issue #420: the PDF is a way of showing an open preview; closing the preview turns it off,
/// so the preview opens again as the rendered page.
#[test]
fn closing_the_preview_turns_the_pdf_off() {
    let (mut engine, _clock) = engine_with_clock();
    engine
        .invoke(ids::VIEW_TOGGLE_PDF_WINDOW, CommandArgs::None)
        .unwrap();
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PREVIEW),
        (false, false, false)
    );
    assert_eq!(engine.preview_layout().before_pdf, None);
    assert_eq!(
        layout_after(&mut engine, ids::VIEW_TOGGLE_PREVIEW),
        (true, false, false)
    );
}
