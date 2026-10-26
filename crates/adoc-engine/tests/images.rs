//! Images open in tabs of their own: never as text, never indexed, sharing the preview tab and
//! the tab order with pages.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
    thread,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, is_image_path, CommandArgs, CommandOutcome, DocumentId, Engine, EngineEvent,
    ImageId, OpenDisposition, Settings, TabItem,
};

/// A copy of the `image-viewer` fixture, so tests may change its files.
fn gallery() -> tempfile::TempDir {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/image-viewer");
    let directory = tempfile::tempdir().unwrap();
    copy_tree(&fixture, directory.path());
    directory
}

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

fn image_path(gallery: &tempfile::TempDir, name: &str) -> PathBuf {
    adoc_engine::normalize_path(&gallery.path().join("modules/ROOT/images").join(name))
}

fn page_path(gallery: &tempfile::TempDir) -> PathBuf {
    adoc_engine::normalize_path(&gallery.path().join("modules/ROOT/pages/index.adoc"))
}

fn engine_with(settings: Settings) -> Engine {
    let mut engine = Engine::new(settings);
    engine.enable_preview_tabs();
    engine
}

fn engine() -> Engine {
    engine_with(Settings::default())
}

fn open_workspace(engine: &mut Engine, gallery: &tempfile::TempDir) {
    engine
        .open_workspace(vec![gallery.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
}

fn opened(outcome: CommandOutcome) -> TabItem {
    match outcome {
        CommandOutcome::Opened(id) => TabItem::Document(id),
        CommandOutcome::OpenedImage(id) => TabItem::Image(id),
        other => panic!("opening gave {other:?}"),
    }
}

fn browse(engine: &mut Engine, path: &Path) -> TabItem {
    opened(
        engine
            .invoke(ids::DOCUMENT_BROWSE, CommandArgs::Path(path.to_path_buf()))
            .unwrap(),
    )
}

fn open(engine: &mut Engine, path: &Path) -> TabItem {
    opened(
        engine
            .invoke(ids::DOCUMENT_OPEN, CommandArgs::Path(path.to_path_buf()))
            .unwrap(),
    )
}

fn image(item: TabItem) -> ImageId {
    item.image().expect("an image tab")
}

fn document(item: TabItem) -> DocumentId {
    item.document().expect("a document tab")
}

fn events(receiver: &Receiver<EngineEvent>) -> Vec<EngineEvent> {
    receiver.try_iter().collect()
}

fn type_into(engine: &mut Engine, id: DocumentId) {
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
}

#[test]
fn images_are_told_from_text_by_extension_whatever_its_case() {
    for path in [
        "logo.png",
        "Photo Café.PNG",
        "a/screenshot.JPG",
        "b.jpeg",
        "architecture.svg",
        "still.gif",
        "still.webp",
        "bitmap.bmp",
        "icon.ico",
        "scan.tif",
        "scan.tiff",
        "photo.heic",
        "photo.avif",
    ] {
        assert!(is_image_path(Path::new(path)), "{path}");
    }
    for path in [
        "index.adoc",
        "guide.pdf",
        "README",
        "png",
        ".png.adoc",
        "notes.txt",
    ] {
        assert!(!is_image_path(Path::new(path)), "{path}");
    }
}

#[test]
fn an_image_opens_in_an_image_tab_and_never_as_text() {
    let gallery = gallery();
    let mut engine = engine();
    open_workspace(&mut engine, &gallery);
    let logo = image_path(&gallery, "logo.png");
    let receiver = engine.subscribe();

    let item = open(&mut engine, &logo);

    let id = image(item);
    assert_eq!(engine.image(id).unwrap().path, logo);
    assert_eq!(engine.tab_order(), [item]);
    assert_eq!(engine.active_item(), Some(item));
    assert_eq!(engine.active(), None, "no text document is active");
    assert_eq!(engine.documents().count(), 0);
    assert!(engine.workspace().index.file(&logo).is_none());
    assert!(events(&receiver).contains(&EngineEvent::ImageOpened(id)));
    // Text that merely has an image's name is still an image to open, and fails to decode in
    // the viewer; it never becomes a document.
    let notes = open(&mut engine, &image_path(&gallery, "notes.png"));
    assert!(notes.image().is_some());
    assert_eq!(engine.documents().count(), 0);
}

#[test]
fn opening_an_open_image_again_activates_its_tab() {
    let gallery = gallery();
    let mut engine = engine();
    let logo = image_path(&gallery, "logo.png");
    let kept = open(&mut engine, &logo);
    let page = open(&mut engine, &page_path(&gallery));

    assert_eq!(browse(&mut engine, &logo), kept);
    assert_eq!(engine.active_item(), Some(kept));
    assert!(
        !engine.is_preview(kept),
        "a kept image never becomes a preview"
    );
    assert_eq!(engine.tab_order(), [kept, page]);
}

#[test]
fn browsing_images_replaces_the_preview_tab_in_its_place() {
    let gallery = gallery();
    let mut engine = engine();
    let page = open(&mut engine, &page_path(&gallery));
    let logo = browse(&mut engine, &image_path(&gallery, "logo.png"));
    assert!(engine.is_preview(logo));
    let receiver = engine.subscribe();

    let screenshot = browse(&mut engine, &image_path(&gallery, "screenshot.jpg"));

    assert!(engine.is_preview(screenshot));
    assert_eq!(engine.tab_order(), [page, screenshot]);
    assert!(
        engine.image(image(logo)).is_none(),
        "the old preview closed"
    );
    let events = events(&receiver);
    assert!(events.contains(&EngineEvent::PreviewTabReplaced {
        old: logo,
        new: screenshot
    }));
    assert!(events.contains(&EngineEvent::ImageClosed(image(logo))));
}

#[test]
fn pages_and_images_share_the_one_preview_tab() {
    let gallery = gallery();
    let mut engine = engine();
    open_workspace(&mut engine, &gallery);
    let page = browse(&mut engine, &page_path(&gallery));

    let logo = browse(&mut engine, &image_path(&gallery, "logo.png"));
    assert_eq!(engine.tab_order(), [logo]);
    assert_eq!(engine.documents().count(), 0);

    let again = browse(&mut engine, &page_path(&gallery));
    assert_eq!(engine.tab_order(), [again]);
    assert!(engine.image(image(logo)).is_none());
    assert_ne!(again, page, "the page was closed and opened again");
}

#[test]
fn an_edited_page_in_the_preview_tab_is_kept_when_an_image_is_browsed() {
    let gallery = gallery();
    let mut engine = engine();
    let page = browse(&mut engine, &page_path(&gallery));
    // Typing keeps the page open, so the image takes a tab of its own.
    type_into(&mut engine, document(page));
    assert!(engine.document(document(page)).unwrap().dirty);

    let logo = browse(&mut engine, &image_path(&gallery, "logo.png"));

    assert_eq!(engine.tab_order(), [page, logo]);
    assert!(engine.document(document(page)).unwrap().dirty);
}

#[test]
fn keep_open_keeps_an_image() {
    let gallery = gallery();
    let mut engine = engine();
    let logo = browse(&mut engine, &image_path(&gallery, "logo.png"));
    let receiver = engine.subscribe();

    engine
        .invoke(ids::DOCUMENT_KEEP_OPEN, CommandArgs::Image(image(logo)))
        .unwrap();
    assert!(!engine.is_preview(logo));
    assert!(events(&receiver).contains(&EngineEvent::TabKept(logo)));

    let screenshot = browse(&mut engine, &image_path(&gallery, "screenshot.jpg"));
    assert_eq!(engine.tab_order(), [logo, screenshot]);
    // With no arguments it keeps the active tab, an image too.
    engine
        .invoke(ids::DOCUMENT_KEEP_OPEN, CommandArgs::None)
        .unwrap();
    assert!(!engine.is_preview(screenshot));
}

#[test]
fn every_image_stays_open_with_preview_tabs_off() {
    let gallery = gallery();
    let settings = gallery.path().join("settings.toml");
    fs::write(&settings, "[editor]\npreview_tabs = false\n").unwrap();
    let mut engine = engine_with(Settings::load(Some(&settings), None).unwrap());

    let logo = browse(&mut engine, &image_path(&gallery, "logo.png"));
    let screenshot = browse(&mut engine, &image_path(&gallery, "screenshot.jpg"));

    assert_eq!(engine.tab_order(), [logo, screenshot]);
    assert!(!engine.is_preview(logo) && !engine.is_preview(screenshot));
}

#[test]
fn closing_mixed_tabs_follows_the_tab_order() {
    let gallery = gallery();
    let mut engine = engine();
    let page = open(&mut engine, &page_path(&gallery));
    let logo = open(&mut engine, &image_path(&gallery, "logo.png"));
    let other = gallery.path().join("other.adoc");
    fs::write(&other, "= Other\n").unwrap();
    let other = open(&mut engine, &other);
    let screenshot = open(&mut engine, &image_path(&gallery, "screenshot.jpg"));
    type_into(&mut engine, document(other));

    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_LEFT, other),
        [page, logo]
    );
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_RIGHT, logo),
        [other, screenshot]
    );
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_OTHERS, logo),
        [page, other, screenshot]
    );
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_CLEAN, logo),
        [page, logo, screenshot],
        "images are always clean"
    );

    engine
        .invoke(ids::DOCUMENT_CLOSE_OTHERS, CommandArgs::Image(image(logo)))
        .unwrap();
    assert_eq!(engine.tab_order(), [logo]);
    assert_eq!(engine.active_item(), Some(logo));
    engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Image(image(logo)))
        .unwrap();
    assert!(engine.tab_order().is_empty());
    assert_eq!(engine.active_item(), None);
}

#[test]
fn closing_the_active_image_activates_the_last_tab() {
    let gallery = gallery();
    let mut engine = engine();
    let logo = open(&mut engine, &image_path(&gallery, "logo.png"));
    let page = open(&mut engine, &page_path(&gallery));
    let screenshot = open(&mut engine, &image_path(&gallery, "screenshot.jpg"));

    engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Image(image(screenshot)))
        .unwrap();
    assert_eq!(engine.active_item(), Some(page));
    assert_eq!(engine.active(), Some(document(page)));

    engine
        .invoke(ids::DOCUMENT_CLOSE, CommandArgs::Document(document(page)))
        .unwrap();
    assert_eq!(engine.active_item(), Some(logo));
    assert_eq!(engine.active(), None);
}

#[test]
fn an_active_image_offers_no_text_commands_and_save_all_leaves_it_alone() {
    let gallery = gallery();
    let mut engine = engine();
    let page = open(&mut engine, &page_path(&gallery));
    type_into(&mut engine, document(page));
    let logo_path = image_path(&gallery, "logo.png");
    let before = fs::read(&logo_path).unwrap();
    let logo = open(&mut engine, &logo_path);

    for text_command in [
        ids::DOCUMENT_SAVE,
        ids::DOCUMENT_RELOAD,
        ids::EXPORT_PDF_PAGE,
        ids::NAVIGATE_FOLLOW_LINK,
    ] {
        assert!(
            !engine.command_available(text_command),
            "{text_command} with an image active"
        );
    }
    for tab_command in [
        ids::DOCUMENT_CLOSE,
        ids::DOCUMENT_CLOSE_OTHERS,
        ids::DOCUMENT_KEEP_OPEN,
        ids::IMAGE_RELOAD,
    ] {
        assert!(engine.command_available(tab_command), "{tab_command}");
    }
    assert!(engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::None)
        .is_err());

    engine
        .invoke(ids::DOCUMENT_SAVE_ALL, CommandArgs::None)
        .unwrap();
    assert!(!engine.document(document(page)).unwrap().dirty);
    assert_eq!(fs::read(&logo_path).unwrap(), before);
    assert_eq!(engine.active_item(), Some(logo));

    open(&mut engine, &page_path(&gallery));
    assert!(!engine.command_available(ids::IMAGE_RELOAD));
}

#[test]
fn an_image_that_cannot_be_opened_leaves_the_preview_tab_as_it_was() {
    let gallery = gallery();
    let mut engine = engine();
    let logo = browse(&mut engine, &image_path(&gallery, "logo.png"));

    for missing in [
        image_path(&gallery, "missing.png"),
        gallery.path().join("modules/ROOT/images.png"),
    ] {
        assert!(engine
            .invoke(ids::DOCUMENT_BROWSE, CommandArgs::Path(missing))
            .is_err());
    }
    fs::create_dir(gallery.path().join("folder.png")).unwrap();
    assert!(engine
        .invoke(
            ids::DOCUMENT_BROWSE,
            CommandArgs::Path(gallery.path().join("folder.png"))
        )
        .is_err());
    assert_eq!(engine.tab_order(), [logo]);
    assert!(engine.is_preview(logo));
}

#[test]
fn reload_gives_an_image_a_new_revision() {
    let gallery = gallery();
    let mut engine = engine();
    let logo = image(open(&mut engine, &image_path(&gallery, "logo.png")));
    let receiver = engine.subscribe();
    let first = engine.image(logo).unwrap().revision;

    engine.invoke(ids::IMAGE_RELOAD, CommandArgs::None).unwrap();
    engine
        .invoke(ids::IMAGE_RELOAD, CommandArgs::Image(logo))
        .unwrap();

    assert_eq!(engine.image(logo).unwrap().revision, first + 2);
    assert_eq!(
        events(&receiver),
        [
            EngineEvent::ImageChanged(logo),
            EngineEvent::ImageChanged(logo)
        ]
    );
}

/// Pumps the engine until `done` holds or ten seconds pass.
fn pump_until(engine: &mut Engine, mut done: impl FnMut(&Engine) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        engine.pump();
        if done(engine) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn an_image_changed_or_deleted_on_disk_gets_a_new_revision() {
    let gallery = gallery();
    let mut engine = Engine::new(Settings::load(None, None).unwrap());
    open_workspace(&mut engine, &gallery);
    let path = image_path(&gallery, "logo.png");
    let logo = image(open(&mut engine, &path));
    let receiver = engine.subscribe();
    let first = engine.image(logo).unwrap().revision;
    thread::sleep(Duration::from_millis(100));

    fs::copy(image_path(&gallery, "still.gif"), &path).unwrap();
    assert!(pump_until(&mut engine, |_| receiver
        .try_iter()
        .any(|event| event == EngineEvent::ImageChanged(logo))));
    let changed = engine.image(logo).unwrap().revision;
    assert!(changed > first);

    fs::remove_file(&path).unwrap();
    assert!(pump_until(&mut engine, |_| receiver
        .try_iter()
        .any(|event| event == EngineEvent::ImageChanged(logo))));
    assert!(engine.image(logo).unwrap().revision > changed);
    assert!(
        engine.image(logo).is_some(),
        "the tab stays, to show the file is gone"
    );
}

#[test]
fn following_an_image_link_opens_the_image_and_back_returns() {
    let gallery = gallery();
    let mut engine = engine();
    open_workspace(&mut engine, &gallery);
    let page = document(open(&mut engine, &page_path(&gallery)));
    let text = engine.document(page).unwrap().text_string();
    let offset = text.find("logo.png").unwrap() + 2;

    let outcome = engine
        .invoke(
            ids::NAVIGATE_FOLLOW_LINK,
            CommandArgs::Offset { id: page, offset },
        )
        .unwrap();

    let CommandOutcome::OpenedImage(logo) = outcome else {
        panic!("following the image gave {outcome:?}");
    };
    assert_eq!(
        engine.image(logo).unwrap().path,
        image_path(&gallery, "logo.png")
    );
    assert_eq!(engine.active_item(), Some(TabItem::Image(logo)));
    assert!(engine.can_go_back());

    engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.active(), Some(page));
    engine
        .invoke(ids::NAVIGATE_FORWARD, CommandArgs::None)
        .unwrap();
    let active = engine.active_item().and_then(TabItem::image).unwrap();
    assert_eq!(
        engine.image(active).unwrap().path,
        image_path(&gallery, "logo.png")
    );
}

#[test]
fn front_ends_without_tabs_open_images_too_and_keep_them() {
    let gallery = gallery();
    let mut engine = Engine::new(Settings::default());
    let logo = browse(&mut engine, &image_path(&gallery, "logo.png"));
    let screenshot = browse(&mut engine, &image_path(&gallery, "screenshot.jpg"));
    assert_eq!(engine.tab_order(), [logo, screenshot]);
    assert_eq!(
        engine
            .open_item(&image_path(&gallery, "logo.png"), OpenDisposition::Preview)
            .unwrap(),
        logo
    );
}
