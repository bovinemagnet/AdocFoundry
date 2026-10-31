//! Link following and back/forward history.

use std::{path::Path, path::PathBuf, time::Duration};

use adoc_engine::{commands::ids, CommandArgs, CommandOutcome, DocumentId, Engine, Settings};

fn fixture(name: &str) -> PathBuf {
    adoc_index::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name),
    )
}

fn open_workspace(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine
}

fn offset_of(engine: &Engine, id: DocumentId, needle: &str) -> usize {
    engine
        .document(id)
        .unwrap()
        .parsed
        .text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} not found"))
        + 2
}

fn follow(engine: &mut Engine, id: DocumentId, offset: usize) -> CommandOutcome {
    engine
        .invoke(
            ids::NAVIGATE_FOLLOW_LINK,
            CommandArgs::Offset { id, offset },
        )
        .unwrap()
}

#[test]
fn follow_link_opens_target_and_pushes_history() {
    let root = fixture("antora-multi-component");
    let mut engine = open_workspace(&root);
    let index = engine
        .open(&root.join("alpha/modules/ROOT/pages/index.adoc"))
        .unwrap();
    assert!(!engine.can_go_back());

    let offset = offset_of(&engine, index, "xref:beta::index.adoc");
    let CommandOutcome::Location(location) = follow(&mut engine, index, offset) else {
        panic!("an xref leads to a location");
    };
    assert!(location
        .path
        .ends_with("beta/modules/ROOT/pages/index.adoc"));
    let target = engine.document_for_path(&location.path).unwrap();
    assert_eq!(
        engine.active(),
        Some(target),
        "the target is opened and activated"
    );
    assert!(engine.can_go_back());
    assert!(!engine.can_go_forward());
    assert_eq!(
        engine.navigation_position().unwrap().path,
        location.path,
        "the current position is the target"
    );

    // Following a link from the target pushes again.
    let back_link = offset_of(&engine, target, "xref:1.0@alpha");
    follow(&mut engine, target, back_link);
    assert_eq!(engine.active(), Some(index));
}

#[test]
fn back_forward_navigation() {
    let root = fixture("xrefs");
    let mut engine = open_workspace(&root);
    let index = engine.open(&root.join("index.adoc")).unwrap();
    let offset = offset_of(&engine, index, "xref:other.adoc");
    follow(&mut engine, index, offset);
    let other = engine.active().unwrap();
    assert_ne!(other, index);

    let CommandOutcome::Opened(id) = engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap()
    else {
        panic!("back reopens the previous document");
    };
    assert_eq!(id, index);
    assert_eq!(engine.active(), Some(index));
    assert_eq!(engine.navigation_position().unwrap().offset, offset);
    assert!(!engine.can_go_back());
    assert!(engine.can_go_forward());

    let CommandOutcome::Opened(id) = engine
        .invoke(ids::NAVIGATE_FORWARD, CommandArgs::None)
        .unwrap()
    else {
        panic!("forward returns to the target");
    };
    assert_eq!(id, other);
    assert!(engine.can_go_back());
    assert!(!engine.can_go_forward());
    assert!(engine
        .invoke(ids::NAVIGATE_FORWARD, CommandArgs::None)
        .is_err());

    // A new link after going back discards the forward history.
    engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap();
    follow(&mut engine, index, offset);
    assert!(!engine.can_go_forward());
}

#[test]
fn images_open_in_image_tabs_and_includes_peek() {
    let root = fixture("images");
    let mut engine = open_workspace(&root);
    let guide = engine.open(&root.join("guide.adoc")).unwrap();
    let image = offset_of(&engine, guide, "image::present");
    let CommandOutcome::OpenedImage(opened) = follow(&mut engine, guide, image + 6) else {
        panic!("an image opens in an image tab");
    };
    assert!(engine
        .image(opened)
        .unwrap()
        .path
        .ends_with("media/present.svg"));
    assert!(engine.can_go_back(), "the page is in the back history");
    engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.active(), Some(guide));

    let include = offset_of(&engine, guide, "include::chapters");
    let CommandOutcome::Peek { location, text } = engine
        .invoke(
            ids::NAVIGATE_PEEK_INCLUDE,
            CommandArgs::Offset {
                id: guide,
                offset: include,
            },
        )
        .unwrap()
    else {
        panic!("an include can be peeked");
    };
    assert!(location.path.ends_with("chapters/setup.adoc"));
    // The included file's text comes with it, so the front end reads nothing.
    assert_eq!(text, std::fs::read_to_string(&location.path).unwrap());
    assert_eq!(location.range, adoc_core::SourceRange::new(0, text.len()));
    assert!(
        engine.document_for_path(&location.path).is_none(),
        "peek does not open"
    );
    assert!(engine
        .invoke(
            ids::NAVIGATE_PEEK_INCLUDE,
            CommandArgs::Offset {
                id: guide,
                offset: image
            },
        )
        .is_err());
}

fn symbols(outcome: CommandOutcome) -> Vec<adoc_engine::SymbolMatch> {
    match outcome {
        CommandOutcome::Symbols(found) => found,
        other => panic!("expected symbols, got {other:?}"),
    }
}

#[test]
fn go_to_symbol_finds_a_section_in_another_component_and_opens_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("alpha/antora.yml", "name: alpha\nversion: '1.0'\n");
    write(
        "alpha/modules/ROOT/pages/index.adoc",
        "= Alpha\n\n== Overview\n",
    );
    write("beta/antora.yml", "name: beta\nversion: ~\n");
    let guide = "= Guide\n\n[[setup]]\n== Installing the Server\n\nText.\n\n:server-port: 8080\n";
    write("beta/modules/admin/pages/guide.adoc", guide);
    let mut engine = open_workspace(&root);
    let alpha = engine
        .open(&root.join("alpha/modules/ROOT/pages/index.adoc"))
        .unwrap();

    let found = symbols(
        engine
            .invoke(
                ids::NAVIGATE_GO_TO_SYMBOL,
                CommandArgs::Query {
                    query: "instal serv".to_owned(),
                    document: None,
                },
            )
            .unwrap(),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    let section = &found[0];
    assert_eq!(section.name, "Installing the Server");
    assert_eq!(section.kind, adoc_engine::WorkspaceSymbolKind::Section);
    assert_eq!(section.detail, "Guide · beta › admin");
    let page = root.join("beta/modules/admin/pages/guide.adoc");
    assert_eq!(section.position.path, page);
    assert_eq!(section.position.offset, guide.find("Installing").unwrap());

    // Attributes are not symbols to go to; anchors and page titles are.
    let found = symbols(
        engine
            .invoke(
                ids::NAVIGATE_GO_TO_SYMBOL,
                CommandArgs::Query {
                    query: "server".to_owned(),
                    document: None,
                },
            )
            .unwrap(),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    let found = symbols(
        engine
            .invoke(
                ids::NAVIGATE_GO_TO_SYMBOL,
                CommandArgs::Query {
                    query: "setup".to_owned(),
                    document: None,
                },
            )
            .unwrap(),
    );
    assert_eq!(found[0].kind, adoc_engine::WorkspaceSymbolKind::Anchor);

    // Choosing it opens the page there, and Back returns.
    engine
        .invoke(
            ids::NAVIGATE_GO_TO,
            CommandArgs::Position(section.position.clone()),
        )
        .unwrap();
    let active = engine.active().unwrap();
    assert_eq!(engine.document(active).unwrap().path, page);
    assert!(engine.can_go_back());
    engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.active(), Some(alpha));
}

#[test]
fn go_to_symbol_in_page_lists_the_page_s_own() {
    let root = fixture("antora-multi-component");
    let mut engine = open_workspace(&root);
    let guide = engine
        .open(&root.join("alpha/modules/ROOT/pages/guide.adoc"))
        .unwrap();
    let names = |engine: &mut Engine, query: &str| -> Vec<String> {
        symbols(
            engine
                .invoke(
                    ids::NAVIGATE_GO_TO_SYMBOL_IN_PAGE,
                    CommandArgs::Query {
                        query: query.to_owned(),
                        document: Some(guide),
                    },
                )
                .unwrap(),
        )
        .into_iter()
        .map(|symbol| symbol.name)
        .collect()
    };
    // The title, the section and its anchor, in page order; every symbol with no query.
    assert_eq!(names(&mut engine, ""), ["Guide", "setup", "Setup"]);
    assert_eq!(names(&mut engine, "gui"), ["Guide"]);
}
