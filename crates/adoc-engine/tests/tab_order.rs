//! The tab strip's order: tabs moved by dragging, and pinned tabs, which sit first and which
//! closing several tabs at once leaves open (#82).

use std::fs;

use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineError, EngineEvent, OpenDisposition, Settings,
    TabItem,
};

/// Opens `count` untitled documents, in tab order.
fn open_tabs(engine: &mut Engine, count: usize) -> Vec<TabItem> {
    (0..count)
        .map(|_| TabItem::Document(engine.open_untitled()))
        .collect()
}

fn move_tab(engine: &mut Engine, item: TabItem, to: usize) {
    engine
        .invoke(ids::DOCUMENT_MOVE_TAB, CommandArgs::MoveTab { item, to })
        .unwrap();
}

fn pin(engine: &mut Engine, item: TabItem) {
    engine.invoke(ids::DOCUMENT_PIN_TAB, args(item)).unwrap();
}

fn args(item: TabItem) -> CommandArgs {
    match item {
        TabItem::Document(id) => CommandArgs::Document(id),
        TabItem::Image(id) => CommandArgs::Image(id),
    }
}

fn close(engine: &mut Engine, command: adoc_engine::CommandId, item: Option<TabItem>) {
    let args = item.map_or(CommandArgs::None, args);
    engine.invoke(command, args).unwrap();
}

#[test]
fn a_tab_moved_to_the_front_stays_there_and_close_left_from_it_closes_nothing() {
    let mut engine = Engine::new(Settings::default());
    let events = engine.subscribe();
    let tabs = open_tabs(&mut engine, 4);
    let [a, b, c, d] = tabs[..] else {
        unreachable!()
    };
    move_tab(&mut engine, c, 0);
    assert_eq!(engine.tab_order(), [c, a, b, d]);
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::TabsReordered));
    // Close Left and Close Right follow the order shown, not the order opened.
    assert!(engine.closed_by(ids::DOCUMENT_CLOSE_LEFT, c).is_empty());
    assert_eq!(
        engine.closed_by(ids::DOCUMENT_CLOSE_RIGHT, c),
        vec![a, b, d]
    );
    close(&mut engine, ids::DOCUMENT_CLOSE_LEFT, Some(c));
    assert_eq!(engine.tab_order(), [c, a, b, d]);
    // A tab opened later still goes at the end; one moved there stays there.
    let [e] = open_tabs(&mut engine, 1)[..] else {
        unreachable!()
    };
    move_tab(&mut engine, c, 99);
    assert_eq!(engine.tab_order(), [a, b, d, e, c]);
    move_tab(&mut engine, d, 1);
    assert_eq!(engine.tab_order(), [a, d, b, e, c]);
}

#[test]
fn moving_a_tab_where_it_is_or_one_that_is_not_open_changes_nothing() {
    let mut engine = Engine::new(Settings::default());
    let tabs = open_tabs(&mut engine, 2);
    let events = engine.subscribe();
    move_tab(&mut engine, tabs[1], 1);
    assert_eq!(engine.tab_order(), tabs);
    assert!(!events
        .try_iter()
        .any(|event| event == EngineEvent::TabsReordered));
    let [gone] = open_tabs(&mut engine, 1)[..] else {
        unreachable!()
    };
    close(&mut engine, ids::DOCUMENT_CLOSE, Some(gone));
    assert!(matches!(
        engine.invoke(
            ids::DOCUMENT_MOVE_TAB,
            CommandArgs::MoveTab { item: gone, to: 0 }
        ),
        Err(EngineError::NotFound(_))
    ));
    assert_eq!(engine.tab_order(), tabs);
}

#[test]
fn a_pinned_tab_survives_close_all() {
    let mut engine = Engine::new(Settings::default());
    let tabs = open_tabs(&mut engine, 4);
    let [a, b, c, d] = tabs[..] else {
        unreachable!()
    };
    // Pinned tabs sit first, in the order they were pinned.
    pin(&mut engine, c);
    pin(&mut engine, a);
    assert_eq!(engine.tab_order(), [c, a, b, d]);
    assert!(engine.is_pinned(c) && engine.is_pinned(a) && !engine.is_pinned(b));

    // Closing several tabs at once leaves the pinned ones; closing one by itself does not.
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE_OTHERS, b), vec![d]);
    assert!(engine.closed_by(ids::DOCUMENT_CLOSE_LEFT, b).is_empty());
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE_RIGHT, c), vec![b, d]);
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE_CLEAN, b), vec![b, d]);
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE_ALL, b), vec![b, d]);
    assert_eq!(engine.closed_by(ids::DOCUMENT_CLOSE, c), vec![c]);

    engine.set_active(b.document().unwrap());
    close(&mut engine, ids::DOCUMENT_CLOSE_ALL, None);
    assert_eq!(engine.tab_order(), [c, a]);
    // The active tab closed, so one of those left takes over.
    assert!(engine.active().is_some());

    close(&mut engine, ids::DOCUMENT_CLOSE, Some(c));
    assert_eq!(engine.tab_order(), [a]);
    assert!(!engine.is_pinned(c));
}

#[test]
fn a_tab_moves_only_among_its_own_kind() {
    let mut engine = Engine::new(Settings::default());
    let tabs = open_tabs(&mut engine, 4);
    let [a, b, c, d] = tabs[..] else {
        unreachable!()
    };
    pin(&mut engine, a);
    pin(&mut engine, b);
    // An unpinned tab dropped among the pinned ones goes first after them.
    move_tab(&mut engine, d, 0);
    assert_eq!(engine.tab_order(), [a, b, d, c]);
    // A pinned tab dropped among the others goes last of the pinned ones.
    move_tab(&mut engine, a, 3);
    assert_eq!(engine.tab_order(), [b, a, d, c]);
    move_tab(&mut engine, a, 0);
    assert_eq!(engine.tab_order(), [a, b, d, c]);
}

#[test]
fn an_unpinned_tab_goes_first_of_the_others() {
    let mut engine = Engine::new(Settings::default());
    let tabs = open_tabs(&mut engine, 3);
    let [a, b, c] = tabs[..] else { unreachable!() };
    pin(&mut engine, c);
    pin(&mut engine, b);
    assert_eq!(engine.tab_order(), [c, b, a]);
    let events = engine.subscribe();
    engine
        .invoke(
            ids::DOCUMENT_UNPIN_TAB,
            CommandArgs::Document(c.document().unwrap()),
        )
        .unwrap();
    assert_eq!(engine.tab_order(), [b, c, a]);
    assert!(!engine.is_pinned(c));
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::TabsReordered));
    // Pinning a pinned tab, or unpinning another, changes nothing.
    pin(&mut engine, b);
    engine
        .invoke(
            ids::DOCUMENT_UNPIN_TAB,
            CommandArgs::Document(a.document().unwrap()),
        )
        .unwrap();
    assert_eq!(engine.tab_order(), [b, c, a]);
}

#[test]
fn pinning_the_preview_tab_keeps_it_open() {
    let directory = tempfile::tempdir().unwrap();
    let (one, two) = (
        directory.path().join("one.adoc"),
        directory.path().join("two.adoc"),
    );
    fs::write(&one, "= One\n").unwrap();
    fs::write(&two, "= Two\n").unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.enable_preview_tabs();
    let first = engine.open_item(&one, OpenDisposition::Preview).unwrap();
    assert!(engine.is_preview(first));
    pin(&mut engine, first);
    assert!(!engine.is_preview(first));
    // The next page browsed to opens a tab of its own rather than replacing it.
    let second = engine.open_item(&two, OpenDisposition::Preview).unwrap();
    assert_eq!(engine.tab_order(), [first, second]);
}

#[test]
fn an_image_tab_can_be_pinned_and_moved() {
    let directory = tempfile::tempdir().unwrap();
    let picture = directory.path().join("picture.svg");
    fs::write(
        &picture,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#,
    )
    .unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let [page] = open_tabs(&mut engine, 1)[..] else {
        unreachable!()
    };
    let image = engine
        .open_item(&picture, OpenDisposition::Persistent)
        .unwrap();
    pin(&mut engine, image);
    assert_eq!(engine.tab_order(), [image, page]);
    close(&mut engine, ids::DOCUMENT_CLOSE_OTHERS, Some(page));
    assert_eq!(engine.tab_order(), [image, page]);
}
