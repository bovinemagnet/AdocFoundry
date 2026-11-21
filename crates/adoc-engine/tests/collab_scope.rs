//! What a guest sees of the host's workspace (COLLAB 7/8, #694; design §11): the shared pages
//! resolve among the shared files only, as the host's Antora components name them, never among
//! the guest's own files, its own clone of the same project included; nothing is read from the
//! guest's disk for them; and several pages are shared side by side.

#![cfg(feature = "collab")]

mod collab_common;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use adoc_engine::{
    protocol::{PeerId, Role},
    DocumentId, Engine, EngineError, Settings,
};
use adoc_render::{ViewBlock, ViewKind};
use collab_common::*;

const WAIT: Duration = Duration::from_secs(20);

fn write(root: &Path, files: &[(&str, &[u8])]) {
    for (name, bytes) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn opened(root: &Path) -> Engine {
    let mut settings = Settings::load(None, None).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    engine
}

/// The host shares an Antora component's page, the page it links to and the partial it
/// includes; the guest has its own clone of the same component open, with other text.
struct AntoraSession {
    session: Session,
    ada: PeerId,
    _clone: tempfile::TempDir,
    clone: PathBuf,
}

/// The host shares `offered`, paths in `directory`, with a guest whose engine is `guest`, who
/// is admitted as an Editor.
fn share(directory: tempfile::TempDir, offered: &[&str], guest: Engine) -> (Session, PeerId) {
    let root = directory.path().to_path_buf();
    let net = Arc::new(Mutex::new(Net::default()));
    let mut host = opened(&root);
    let files: Vec<PathBuf> = offered.iter().map(PathBuf::from).collect();
    host.host_session(Box::new(HostLink(net.clone())), &root, &files, "Host")
        .unwrap();
    let mut session = Session {
        directory,
        net,
        host,
        guests: Vec::new(),
    };
    let ada = session.knock_with(1, guest);
    session.host.admit_peer(ada).unwrap();
    session.settle();
    session.host.set_peer_role(ada, Role::Editor).unwrap();
    session.settle();
    (session, ada)
}

/// Everything the host sent its guests, as text, payloads included.
fn sent(session: &Session) -> String {
    let net = session.net.lock().unwrap();
    net.sent
        .values()
        .flatten()
        .map(|message| String::from_utf8_lossy(&message.encode().unwrap()).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn antora_session() -> AntoraSession {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    write(
        &root,
        &[
            (
                "antora.yml",
                b"name: guide\nversion: '1.0'\ntitle: Hushed Guide Title\nstart_page: index.adoc\nnav:\n- modules/ROOT/nav.adoc\nasciidoc:\n  attributes:\n    hushed-attribute: hushed-value\n",
            ),
            (
                "modules/ROOT/nav.adoc",
                b"* xref:index.adoc[]\n* xref:hushed-page.adoc[]\n",
            ),
            (
                "modules/ROOT/pages/hushed-page.adoc",
                b"= Hushed page\n\nHushed text.\n",
            ),
            (
                "modules/ROOT/pages/index.adoc",
                b"= Index\n\ninclude::partial$note.adoc[]\n\nSee xref:other.adoc[the other page].\n\nimage::diagram.png[A picture]\n",
            ),
            ("modules/ROOT/pages/other.adoc", b"= Other\n\nThe other page.\n"),
            ("modules/ROOT/partials/note.adoc", b"Host partial.\n"),
            ("modules/ROOT/images/diagram.png", b"\x89PNG\r\n\x1a\n"),
        ],
    );

    let clone_dir = tempfile::tempdir().unwrap();
    let clone = clone_dir.path().to_path_buf();
    write(
        &clone,
        &[
            ("antora.yml", b"name: guide\nversion: '1.0'\n"),
            (
                "modules/ROOT/pages/index.adoc",
                b"= Clone index\n\ninclude::partial$note.adoc[]\n",
            ),
            ("modules/ROOT/partials/note.adoc", b"Clone partial.\n"),
        ],
    );
    let (session, ada) = share(
        directory,
        &[
            "modules/ROOT/pages/index.adoc",
            "modules/ROOT/pages/other.adoc",
            "modules/ROOT/partials/note.adoc",
        ],
        opened(&clone),
    );
    AntoraSession {
        session,
        ada,
        _clone: clone_dir,
        clone,
    }
}

/// The session's number for the shared file at `display`.
fn file_of(engine: &Engine, display: &str) -> u64 {
    engine
        .session_manifest()
        .into_iter()
        .find(|entry| entry.path == display)
        .unwrap()
        .file
}

fn open_shared(session: &mut Session, peer: PeerId, display: &str) -> DocumentId {
    let file = file_of(session.engine(peer), display);
    session.engine(peer).open_session_file(file).unwrap();
    session.settle();
    session.engine(peer).session_document(file).unwrap()
}

/// Every block of `id`'s preview, depth first, rendered afresh for the text as it is now.
fn preview(engine: &mut Engine, id: DocumentId) -> Vec<ViewBlock> {
    fn walk(blocks: Vec<&ViewBlock>, out: &mut Vec<ViewBlock>) {
        for block in blocks {
            out.push(block.clone());
            walk(block.child_refs(), out);
        }
    }
    let events = engine.subscribe();
    engine.request_preview(id);
    for _ in 0..50 {
        engine.pump();
        assert!(engine.wait_for_workers(WAIT));
        let ready = events.try_iter().any(|event| {
            matches!(event, adoc_engine::EngineEvent::PreviewReady { id: ready, .. } if ready == id)
        });
        if ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let shown = engine.rendered_preview(id).expect("a preview");
    let mut out = Vec::new();
    walk(shown.view.blocks.iter().collect(), &mut out);
    out
}

fn text_of(blocks: &[ViewBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match &block.kind {
            ViewKind::Paragraph(content) => Some(adoc_render::ViewInline::plain_text(content)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn collab_scope_never_leaks() {
    let AntoraSession {
        mut session,
        ada,
        clone,
        _clone: _keep,
    } = antora_session();
    let index = open_shared(&mut session, ada, "modules/ROOT/pages/index.adoc");
    let index_path = session.engine(ada).document(index).unwrap().path.clone();
    assert!(
        !index_path.starts_with(session.directory.path()) && !index_path.starts_with(&clone),
        "a shared page is neither the host's path nor one of the guest's own"
    );

    // A decoy where the partial would be, were the shared page's folder on disk.
    let decoy = index_path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("partials/note.adoc");
    fs::create_dir_all(decoy.parent().unwrap()).unwrap();
    fs::write(&decoy, "Decoy partial.\n").unwrap();

    // Before the partial is open the include has nothing to read: not the decoy, not the
    // clone's partial.
    let shown = text_of(&preview(session.engine(ada), index));
    assert!(!shown.contains("Decoy"), "{shown}");
    assert!(!shown.contains("Clone partial"), "{shown}");

    // Opened, the host's partial is what the shared page includes.
    let _note = open_shared(&mut session, ada, "modules/ROOT/partials/note.adoc");
    let blocks = preview(session.engine(ada), index);
    let shown = text_of(&blocks);
    assert!(shown.contains("Host partial."), "{shown}");
    assert!(
        !shown.contains("Decoy") && !shown.contains("Clone"),
        "{shown}"
    );
    // Pictures are the host's: a guest's preview shows their text, never a file of its own.
    let image = blocks.iter().find_map(|block| match &block.kind {
        ViewKind::Image(image) => Some(image.clone()),
        _ => None,
    });
    let image = image.expect("the picture's block");
    assert_eq!(image.path, None);
    assert_eq!(image.alt, "A picture");

    // The guest's own clone still resolves among its own files.
    let own = session
        .engine(ada)
        .open(&clone.join("modules/ROOT/pages/index.adoc"))
        .unwrap();
    let shown = text_of(&preview(session.engine(ada), own));
    assert!(shown.contains("Clone partial."), "{shown}");
    assert!(!shown.contains("Host partial"), "{shown}");

    // Nothing in the shared pages' folder is ever opened from disk: the partial's path is the
    // host's partial, and a file the host does not share is refused.
    let opened = session.engine(ada).open(&decoy).unwrap();
    assert_eq!(
        session.engine(ada).document(opened).unwrap().text_string(),
        "Host partial.\n"
    );
    let unshared = index_path.with_file_name("decoy.adoc");
    fs::create_dir_all(unshared.parent().unwrap()).unwrap();
    fs::write(&unshared, "Decoy page.\n").unwrap();
    assert!(matches!(
        session.engine(ada).open(&unshared),
        Err(EngineError::Refused(_))
    ));
    session.assert_no_host_paths();

    // Of the host's component, its name and version only: nothing else in its `antora.yml`,
    // its navigation or a page it does not share is ever sent.
    let sent = sent(&session);
    for hushed in ["Hushed", "hushed", "nav.adoc", "start_page"] {
        assert!(!sent.contains(hushed), "the host sent {hushed:?}");
    }
    assert!(sent.contains("guide") && sent.contains("1.0"));

    // An xref to a page the host does not share leads nowhere, and completion offers only
    // shared files.
    let guest = session.engine(ada);
    let end = guest.document(index).unwrap().text.len();
    guest
        .invoke(
            adoc_engine::commands::ids::EDIT_INSERT_TEXT,
            adoc_engine::CommandArgs::Text {
                id: index,
                range: adoc_core::SourceRange::new(end, end),
                text: "\nxref:hushed-page.adoc[] xref:".to_owned(),
            },
        )
        .unwrap();
    session.settle();
    let guest = session.engine(ada);
    let text = guest.document(index).unwrap().text_string();
    let hushed = text.find("hushed-page").unwrap() + 2;
    assert_eq!(guest.definition(index, hushed), None);
    let offered: Vec<String> = guest
        .completions(index, text.len())
        .into_iter()
        .map(|item| format!("{item:?}"))
        .collect();
    assert!(
        offered.iter().any(|item| item.contains("other.adoc")),
        "{offered:?}"
    );
    assert!(
        !offered.iter().any(|item| item.contains("hushed")),
        "{offered:?}"
    );

    // The guest's own workspace holds no shared page: not in Quick Open, the symbols, the
    // recent files or the graph.
    let guest = session.engine(ada);
    let shared = |path: &Path| path.starts_with(index_path.ancestors().nth(4).unwrap());
    assert!(guest
        .quick_open("", 1000)
        .iter()
        .all(|item| !shared(&item.path)));
    assert!(guest
        .symbols("Index", None)
        .iter()
        .all(|symbol| !shared(&symbol.position.path)));
    let graph = guest.graph(
        &adoc_engine::GraphScope::Workspace,
        &adoc_engine::GraphFilter::default(),
    );
    assert!(graph.nodes.iter().all(|node| match &node.key {
        adoc_engine::GraphKey::File(path) => !shared(path),
        _ => true,
    }));
    fs::remove_dir_all(index_path.ancestors().nth(4).unwrap()).ok();
}

/// A partial the other side edits shows in the preview of the shared page including it.
#[test]
fn a_shared_partial_edited_elsewhere_shows_in_the_preview() {
    let AntoraSession {
        mut session, ada, ..
    } = antora_session();
    let index = open_shared(&mut session, ada, "modules/ROOT/pages/index.adoc");
    let _note = open_shared(&mut session, ada, "modules/ROOT/partials/note.adoc");
    assert!(text_of(&preview(session.engine(ada), index)).contains("Host partial."));
    let file = file_of(&session.host, "modules/ROOT/partials/note.adoc");
    let note = session.host.session_document(file).unwrap();
    type_at(&mut session.host, note, 0, "Edited. ");
    session.settle();
    let shown = text_of(&preview(session.engine(ada), index));
    assert!(shown.contains("Edited. Host partial."), "{shown}");
}

/// More component versions than one message carries: those that fit are sent, the rest left
/// out, their pages resolving as plain files, and the session goes on.
#[test]
fn components_beyond_a_message_are_left_out() {
    let directory = tempfile::tempdir().unwrap();
    let long = "x".repeat(200);
    let mut offered = Vec::new();
    for n in 0..300 {
        let dir = format!("c{n:03}{long}");
        write(
            directory.path(),
            &[
                (
                    &format!("{dir}/antora.yml"),
                    format!("name: c{n:03}\nversion: ~\n").as_bytes(),
                ),
                (
                    &format!("{dir}/modules/ROOT/pages/p.adoc"),
                    b"= P\n\nxref:c000::p.adoc[] xref:c001::p.adoc[]\n",
                ),
            ],
        );
        offered.push(format!("{dir}/modules/ROOT/pages/p.adoc"));
    }
    let offered: Vec<&str> = offered.iter().map(String::as_str).collect();
    let guest = tempfile::tempdir().unwrap();
    let (mut session, ada) = share(directory, &offered, opened(guest.path()));
    let sent = sent(&session);
    assert!(sent.contains("c000") && !sent.contains("\"c299\""));

    let resolves = |session: &mut Session, page: &str, target: &str| {
        let id = open_shared(session, ada, page);
        let engine = session.engine(ada);
        let text = engine.document(id).unwrap().text_string();
        let offset = text.find(target).unwrap() + 2;
        engine.definition(id, offset).is_some()
    };
    let first = format!("c000{long}/modules/ROOT/pages/p.adoc");
    let last = format!("c299{long}/modules/ROOT/pages/p.adoc");
    assert!(resolves(&mut session, &first, "c001::"));
    assert!(!resolves(&mut session, &last, "c000::"));
}

/// A link from a shared page to another shared page leads there, through the host.
#[test]
fn a_link_to_another_shared_page_opens_it_from_the_host() {
    let AntoraSession {
        mut session, ada, ..
    } = antora_session();
    let index = open_shared(&mut session, ada, "modules/ROOT/pages/index.adoc");
    let text = session.engine(ada).document(index).unwrap().text_string();
    let offset = text.find("other.adoc").unwrap() + 2;
    let target = session
        .engine(ada)
        .definition(index, offset)
        .expect("the link resolves among the shared files");
    let adoc_engine::DefinitionTarget::Location(location) = target else {
        panic!("not a page: {target:?}");
    };
    let path = location.path.clone();
    assert!(path.ends_with("modules/ROOT/pages/other.adoc"), "{path:?}");
    assert!(!path.starts_with(session.directory.path()));
    // Following it as the app does, with preview tabs, asks the host, without a failure, and
    // shows it once it arrives.
    session.engine(ada).enable_preview_tabs();
    let outcome = session
        .engine(ada)
        .invoke(
            adoc_engine::commands::ids::NAVIGATE_FOLLOW_LINK,
            adoc_engine::CommandArgs::Offset { id: index, offset },
        )
        .unwrap();
    assert!(
        matches!(outcome, adoc_engine::CommandOutcome::Done),
        "{outcome:?}"
    );
    session.settle();
    let other = session.engine(ada).active().unwrap();
    assert_eq!(
        session.engine(ada).document(other).unwrap().text_string(),
        "= Other\n\nThe other page.\n"
    );
}

/// Two participants edit two pages at once; each page converges on its own, and one page
/// starting again in a new epoch leaves the other as it was (PRD §10).
#[test]
fn collab_pages_converge_independently() {
    let mut session = Session::new(&[("one.adoc", "= One\n"), ("two.adoc", "= Two\n")]);
    let ada = session.guest(1, true);
    let two = open_shared(&mut session, ada, "two.adoc");
    for round in 0..10 {
        guest_types(&mut session, ada, 0, "a");
        let typed = session.engine(ada);
        let end = typed.document(two).unwrap().text.len();
        typed
            .invoke(
                adoc_engine::commands::ids::EDIT_INSERT_TEXT,
                adoc_engine::CommandArgs::Text {
                    id: two,
                    range: adoc_core::SourceRange::new(end, end),
                    text: "b".to_owned(),
                },
            )
            .unwrap();
        let one = session.host_doc();
        type_at(&mut session.host, one, 0, &round.to_string());
        if round % 3 == 0 {
            session.settle();
        }
    }
    session.settle();
    let host_two = session.host.session_document(2).unwrap();
    assert_eq!(session.guest_text(ada), session.host_text());
    assert_eq!(
        session.engine(ada).document(two).unwrap().text_string(),
        session.host.document(host_two).unwrap().text_string()
    );
    assert_eq!(
        session.host.document(host_two).unwrap().text_string(),
        format!("= Two\n{}", "b".repeat(10))
    );

    // Page one starts again from its file; page two keeps its epoch and its text.
    fs::write(session.directory.path().join("one.adoc"), "= Outside\n").unwrap();
    let one = session.host_doc();
    assert!(session.host.save(one).is_err());
    session
        .host
        .resolve_disk_conflict(one, adoc_engine::DiskResolution::ReloadDisk)
        .unwrap();
    session.settle();
    assert_eq!(session.host.shared_file(one).unwrap().1, 2);
    assert_eq!(session.host.shared_file(host_two).unwrap().1, 1);
    assert_eq!(session.guest_text(ada), "= Outside\n");
    assert_eq!(
        session.engine(ada).document(two).unwrap().text_string(),
        format!("= Two\n{}", "b".repeat(10))
    );
}
