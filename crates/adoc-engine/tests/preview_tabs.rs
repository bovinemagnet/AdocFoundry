//! Preview tabs: browsing shows pages in one reusable tab until one is edited or kept open.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DocumentId, EditorMode, Engine, EngineEvent,
    MarkSet, NavigationPosition, OpenDisposition, Settings, VisualArgs, VisualValue,
};

/// A copy of the `preview-tabs` fixture, so tests may save into it.
fn pages() -> tempfile::TempDir {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/preview-tabs");
    let directory = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(fixture).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), directory.path().join(entry.file_name())).unwrap();
    }
    directory
}

fn page(directory: &tempfile::TempDir, name: &str) -> PathBuf {
    adoc_engine::normalize_path(&directory.path().join(format!("{name}.adoc")))
}

/// An engine for a front end that shows preview tabs, with `settings`.
fn engine_with(settings: Settings) -> Engine {
    let mut engine = Engine::new(settings);
    engine.enable_preview_tabs();
    engine
}

fn engine() -> Engine {
    engine_with(Settings::default())
}

/// Opens `pages` as the workspace, which links need to resolve.
fn open_workspace(engine: &mut Engine, pages: &tempfile::TempDir) {
    engine
        .open_workspace(vec![pages.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
}

fn browse(engine: &mut Engine, path: &Path) -> DocumentId {
    match engine
        .invoke(ids::DOCUMENT_BROWSE, CommandArgs::Path(path.to_path_buf()))
        .unwrap()
    {
        CommandOutcome::Opened(id) => id,
        other => panic!("browsing gave {other:?}"),
    }
}

fn open(engine: &mut Engine, path: &Path) -> DocumentId {
    match engine
        .invoke(ids::DOCUMENT_OPEN, CommandArgs::Path(path.to_path_buf()))
        .unwrap()
    {
        CommandOutcome::Opened(id) => id,
        other => panic!("opening gave {other:?}"),
    }
}

fn type_text(engine: &mut Engine, id: DocumentId, text: &str) {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

fn open_paths(engine: &Engine) -> Vec<PathBuf> {
    engine
        .tab_order()
        .iter()
        .map(|item| {
            engine
                .document(item.document().unwrap())
                .unwrap()
                .path
                .clone()
        })
        .collect()
}

fn events(receiver: &Receiver<EngineEvent>) -> Vec<EngineEvent> {
    receiver.try_iter().collect()
}

#[test]
fn browsing_reuses_one_preview_tab() {
    let pages = pages();
    let mut engine = engine();
    browse(&mut engine, &page(&pages, "introduction"));
    browse(&mut engine, &page(&pages, "installation"));
    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    assert_eq!(open_paths(&engine), [page(&pages, "configuration")]);
    assert!(engine.is_preview(configuration));
    assert_eq!(engine.active(), Some(configuration));
}

#[test]
fn editing_keeps_a_preview_tab_open() {
    let pages = pages();
    let mut engine = engine();
    let installation = browse(&mut engine, &page(&pages, "installation"));
    type_text(&mut engine, installation, "x");
    assert!(!engine.is_preview(installation));

    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    assert!(engine.is_preview(configuration));
    let troubleshooting = browse(&mut engine, &page(&pages, "troubleshooting"));
    assert_eq!(
        open_paths(&engine),
        [
            page(&pages, "installation"),
            page(&pages, "troubleshooting")
        ]
    );
    assert!(engine.is_preview(troubleshooting));
}

#[test]
fn formatting_and_visual_edits_keep_a_preview_tab_open() {
    let pages = pages();
    let mut engine = engine();
    // A formatting command in the source editor.
    let installation = browse(&mut engine, &page(&pages, "installation"));
    let text = engine.document(installation).unwrap().text_string();
    let plain = text.find("plain").unwrap();
    engine
        .invoke(
            ids::EDIT_TOGGLE_STRONG,
            CommandArgs::Range {
                id: installation,
                range: SourceRange::new(plain, plain + 5),
            },
        )
        .unwrap();
    assert!(!engine.is_preview(installation));

    // Typing in the visual editor.
    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(configuration))
        .unwrap();
    assert_eq!(engine.editor_mode(configuration), EditorMode::Visual);
    assert!(engine.is_preview(configuration), "a mode change is no edit");
    let text = engine.document(configuration).unwrap().text_string();
    let at = text.find("it.").unwrap();
    engine
        .invoke(
            ids::VISUAL_INSERT_TEXT,
            CommandArgs::Visual(VisualArgs {
                id: configuration,
                version: engine.document(configuration).unwrap().version,
                selection: SourceRange::new(at, at),
                marks: MarkSet::EMPTY,
                value: VisualValue::Text("x".to_owned()),
            }),
        )
        .unwrap();
    assert!(!engine.is_preview(configuration));
    browse(&mut engine, &page(&pages, "troubleshooting"));
    assert_eq!(engine.tab_order().len(), 3);
}

#[test]
fn saving_or_undoing_does_not_demote_a_kept_tab() {
    let pages = pages();
    let mut engine = engine();
    let installation = browse(&mut engine, &page(&pages, "installation"));
    type_text(&mut engine, installation, "x");
    assert!(engine.undo(installation));
    assert!(!engine.document(installation).unwrap().dirty);
    assert!(!engine.is_preview(installation));

    // Saving a clean preview keeps it open too.
    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(configuration))
        .unwrap();
    assert!(!engine.is_preview(configuration));
    browse(&mut engine, &page(&pages, "troubleshooting"));
    assert_eq!(engine.tab_order().len(), 3);
}

#[test]
fn keep_open_promotes_the_tab_in_place() {
    let pages = pages();
    let mut engine = engine();
    let installation = browse(&mut engine, &page(&pages, "installation"));
    engine
        .invoke(ids::VIEW_SHOW_SPLIT, CommandArgs::Document(installation))
        .unwrap();
    let receiver = engine.subscribe();
    engine
        .invoke(ids::DOCUMENT_KEEP_OPEN, CommandArgs::Document(installation))
        .unwrap();
    assert!(!engine.is_preview(installation));
    assert!(events(&receiver).contains(&EngineEvent::TabKept(installation.into())));
    assert_eq!(engine.editor_mode(installation), EditorMode::Split);
    assert_eq!(engine.active(), Some(installation));

    // Without arguments, the active document.
    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    engine
        .invoke(ids::DOCUMENT_KEEP_OPEN, CommandArgs::None)
        .unwrap();
    assert!(!engine.is_preview(configuration));
    assert_eq!(
        engine.document_for_path(&page(&pages, "installation")),
        Some(installation)
    );
}

#[test]
fn browsing_to_an_open_page_activates_its_tab() {
    let pages = pages();
    let mut engine = engine();
    let introduction = open(&mut engine, &page(&pages, "introduction"));
    assert!(
        !engine.is_preview(introduction),
        "Open File keeps the page open"
    );
    let installation = browse(&mut engine, &page(&pages, "installation"));
    assert_eq!(
        browse(&mut engine, &page(&pages, "introduction")),
        introduction
    );
    assert!(
        !engine.is_preview(introduction),
        "a kept tab is never made a preview"
    );
    assert_eq!(engine.active(), Some(introduction));
    assert!(
        engine.is_preview(installation),
        "the preview stays where it was"
    );
    assert_eq!(engine.tab_order(), [introduction, installation]);

    // Opening the preview's page with Open File keeps it open.
    assert_eq!(
        open(&mut engine, &page(&pages, "installation")),
        installation
    );
    assert!(!engine.is_preview(installation));

    // And browsing to the page again: the same document, not reopened.
    assert_eq!(
        browse(&mut engine, &page(&pages, "installation")),
        installation
    );
}

#[test]
fn a_replaced_preview_keeps_its_place_among_the_tabs() {
    let pages = pages();
    let mut engine = engine();
    let introduction = open(&mut engine, &page(&pages, "introduction"));
    let installation = browse(&mut engine, &page(&pages, "installation"));
    let troubleshooting = open(&mut engine, &page(&pages, "troubleshooting"));
    let receiver = engine.subscribe();
    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    assert_eq!(
        engine.tab_order(),
        [introduction, configuration, troubleshooting]
    );
    let events = events(&receiver);
    let replaced = EngineEvent::PreviewTabReplaced {
        old: installation.into(),
        new: configuration.into(),
    };
    let closed = EngineEvent::DocumentClosed(installation);
    let at = |event: &EngineEvent| events.iter().position(|e| e == event).unwrap();
    assert!(at(&replaced) < at(&closed), "{events:?}");

    // Close Left and Close Right follow the tab order.
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_RIGHT, configuration),
        [troubleshooting]
    );
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_LEFT, configuration),
        [introduction]
    );
}

#[test]
fn a_failed_browse_leaves_the_preview_open() {
    let pages = pages();
    let mut engine = engine();
    let installation = browse(&mut engine, &page(&pages, "installation"));
    let missing = engine.invoke(
        ids::DOCUMENT_BROWSE,
        CommandArgs::Path(page(&pages, "missing")),
    );
    assert!(missing.is_err());
    assert_eq!(engine.tab_order(), [installation]);
    assert!(engine.is_preview(installation));
    assert_eq!(engine.active(), Some(installation));
}

#[test]
fn a_pinned_preview_keeps_its_page_open() {
    let pages = pages();
    let mut engine = engine();
    let installation = browse(&mut engine, &page(&pages, "installation"));
    engine
        .invoke(ids::PREVIEW_PIN, CommandArgs::Document(installation))
        .unwrap();
    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    assert_eq!(engine.preview_layout().pinned, Some(installation));
    assert_eq!(engine.tab_order(), [installation, configuration]);
    assert!(!engine.is_preview(installation));
    assert!(engine.is_preview(configuration));
}

#[test]
fn history_survives_a_replaced_preview() {
    let pages = pages();
    let mut engine = engine();
    open_workspace(&mut engine, &pages);
    let introduction = browse(&mut engine, &page(&pages, "introduction"));
    let text = engine.document(introduction).unwrap().text_string();
    let offset = text.find("xref:").unwrap() + 2;
    engine
        .invoke(
            ids::NAVIGATE_FOLLOW_LINK,
            CommandArgs::Offset {
                id: introduction,
                offset,
            },
        )
        .unwrap();
    let installation = engine.active().unwrap();
    assert_eq!(open_paths(&engine), [page(&pages, "installation")]);
    assert!(engine.is_preview(installation));

    engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap();
    assert_eq!(open_paths(&engine), [page(&pages, "introduction")]);
    engine
        .invoke(ids::NAVIGATE_FORWARD, CommandArgs::None)
        .unwrap();
    assert_eq!(open_paths(&engine), [page(&pages, "installation")]);
    let position = engine.navigation_position().unwrap();
    assert_eq!(position.path, page(&pages, "installation"));

    // A link in the rendered preview browses too.
    engine
        .invoke(
            ids::NAVIGATE_GO_TO,
            CommandArgs::Position(NavigationPosition {
                path: page(&pages, "troubleshooting"),
                offset: 0,
            }),
        )
        .unwrap();
    assert_eq!(open_paths(&engine), [page(&pages, "troubleshooting")]);
}

#[test]
fn turning_preview_tabs_off_keeps_every_page() {
    let pages = pages();
    let mut settings = Settings::default();
    settings.editor.preview_tabs = false;
    let mut engine = engine_with(settings);
    let introduction = browse(&mut engine, &page(&pages, "introduction"));
    let installation = browse(&mut engine, &page(&pages, "installation"));
    assert_eq!(engine.tab_order(), [introduction, installation]);
    assert!(!engine.is_preview(introduction) && !engine.is_preview(installation));
}

#[test]
fn turning_preview_tabs_off_keeps_the_preview_open() {
    let pages = pages();
    let user = pages.path().join("settings.toml");
    fs::write(&user, "[editor]\npreview_tabs = true\n").unwrap();
    let mut engine = engine_with(Settings::load(Some(&user), None).unwrap());
    let introduction = open(&mut engine, &page(&pages, "introduction"));
    let installation = browse(&mut engine, &page(&pages, "installation"));

    fs::write(&user, "[editor]\npreview_tabs = false\n").unwrap();
    engine.reload_settings().unwrap();
    assert_eq!(engine.tab_order(), [introduction, installation]);
    assert!(!engine.is_preview(installation));
    let configuration = browse(&mut engine, &page(&pages, "configuration"));
    assert_eq!(engine.tab_order().len(), 3);

    // On again: the kept tabs stay kept, and browsing uses a preview from now on.
    fs::write(&user, "[editor]\npreview_tabs = true\n").unwrap();
    engine.reload_settings().unwrap();
    assert!(!engine.is_preview(configuration));
    let troubleshooting = browse(&mut engine, &page(&pages, "troubleshooting"));
    assert!(engine.is_preview(troubleshooting));
    assert_eq!(engine.tab_order().len(), 4);
}

#[test]
fn without_preview_tabs_a_front_end_keeps_what_it_opens() {
    // The CLI, the language server and the MCP server never ask for preview tabs.
    let pages = pages();
    let mut engine = Engine::new(Settings::default());
    open_workspace(&mut engine, &pages);
    let introduction = engine
        .open_as(&page(&pages, "introduction"), OpenDisposition::Preview)
        .unwrap();
    let text = engine.document(introduction).unwrap().text_string();
    let offset = text.find("xref:").unwrap() + 2;
    engine
        .invoke(
            ids::NAVIGATE_FOLLOW_LINK,
            CommandArgs::Offset {
                id: introduction,
                offset,
            },
        )
        .unwrap();
    assert_eq!(engine.tab_order().len(), 2);
    assert!(engine.tab_order().iter().all(|id| !engine.is_preview(*id)));
}
