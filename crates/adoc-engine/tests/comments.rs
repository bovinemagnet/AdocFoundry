//! Comments on a page's text (#762, COMMENTS 1/4): kept in `.<page>.afd` beside the page, held
//! by the open page as ranges that follow its text, found again by their quote when the page
//! changed elsewhere, and written only when they change.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, CommandOutcome, CommentId, CommentRequest,
    CommentsState, DocumentId, Engine, EngineError, ManualClock, Settings, COMMENT_SETTLE,
    INDEX_SETTLE,
};

const QUOTE: &str = "the installer refuses an unsigned package";

struct Setup {
    _directory: tempfile::TempDir,
    root: PathBuf,
    clock: ManualClock,
}

impl Setup {
    /// A copy of `tests/fixtures/comments`, whose install page has one comment with a reply.
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(&directory.path().join("docs"));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/comments");
        copy(&fixture, &root);
        Self {
            _directory: directory,
            root,
            clock: ManualClock::new(),
        }
    }

    fn engine(&self) -> Engine {
        // A user settings file of the test's own, which opening the workspace reads again.
        let user = self.root.parent().unwrap().join("settings.toml");
        fs::write(
            &user,
            "[comments]\nname = \"Paul Snow\"\nemail = \"paul@example.org\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None).unwrap().with_git_opt_in();
        let mut engine = Engine::with_clock(settings, Arc::new(self.clock.clone()));
        engine.open_workspace(vec![self.root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        engine
    }

    fn page(&self) -> PathBuf {
        self.root.join("modules/ROOT/pages/install.adoc")
    }

    fn sidecar(&self) -> PathBuf {
        self.root.join("modules/ROOT/pages/.install.adoc.afd")
    }

    /// Past the settle of a comment change, with what it wrote on disk, and any write again
    /// after a conflict.
    fn settle(&self, engine: &mut Engine) {
        self.clock.advance(COMMENT_SETTLE + INDEX_SETTLE);
        // A write, its answer, and a write again after a conflict and its answer.
        for _ in 0..3 {
            engine.pump();
            assert!(engine.wait_for_workers(Duration::from_secs(30)));
        }
    }
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn request(engine: &mut Engine, command: adoc_engine::CommandId, request: CommentRequest) {
    engine
        .invoke(command, CommandArgs::Comment(request))
        .unwrap();
}

fn add(engine: &mut Engine, id: DocumentId, range: SourceRange, text: &str) -> CommentId {
    let before: Vec<CommentId> = engine.comments(id).iter().map(|c| c.id).collect();
    request(
        engine,
        ids::COMMENTS_ADD,
        CommentRequest::Add {
            id,
            range,
            text: text.to_owned(),
        },
    );
    engine
        .comments(id)
        .into_iter()
        .map(|comment| comment.id)
        .find(|comment| !before.contains(comment))
        .unwrap()
}

fn type_at(engine: &mut Engine, id: DocumentId, at: usize, text: &str) {
    replace(engine, id, SourceRange::new(at, at), text);
}

fn replace(engine: &mut Engine, id: DocumentId, range: SourceRange, text: &str) {
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range,
                text: text.to_owned(),
            },
        )
        .unwrap();
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

fn quotes(engine: &Engine, id: DocumentId) -> Vec<String> {
    engine
        .comments(id)
        .into_iter()
        .map(|comment| comment.quote)
        .collect()
}

#[test]
fn a_pages_comments_are_read_from_beside_it() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    assert_eq!(engine.comments_state(id), CommentsState::Usable);
    let comments = engine.comments(id);
    assert_eq!(comments.len(), 1);
    let comment = &comments[0];
    assert_eq!(comment.quote, QUOTE);
    assert_eq!(
        &text(&engine, id)[comment.range.start..comment.range.end],
        QUOTE
    );
    assert_eq!(comment.author.name, "Ada Lovelace");
    assert_eq!(comment.text, "Is this still true for 0.14?");
    assert_eq!(comment.replies.len(), 1);
    assert_eq!(comment.replies[0].author.name, "Grace Hopper");
    assert!(!comment.detached);
    assert_eq!(
        engine.comment_at(id, comment.range.start + 4),
        Some(comment.id)
    );
    assert_eq!(engine.comment_at(id, 0), None);
}

#[test]
fn a_comment_follows_its_text_through_edits() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let start = text(&engine, id).find(QUOTE).unwrap();
    let range = |engine: &Engine| engine.comments(id)[0].range;

    type_at(&mut engine, id, 0, "// A note.\n");
    assert_eq!(
        engine.comments(id)[0].quote,
        QUOTE,
        "text before it moves it"
    );
    let at = range(&engine);
    assert_eq!(at.start, start + 11);

    type_at(&mut engine, id, at.start, "Yes, ");
    assert_eq!(
        engine.comments(id)[0].quote,
        QUOTE,
        "text at its start is outside it"
    );
    let at = range(&engine);
    type_at(&mut engine, id, at.end, " at all");
    assert_eq!(
        engine.comments(id)[0].quote,
        QUOTE,
        "text at its end is outside it"
    );

    let at = range(&engine);
    type_at(&mut engine, id, at.start + "the installer".len(), " always");
    assert_eq!(
        engine.comments(id)[0].quote,
        "the installer always refuses an unsigned package",
        "text inside grows it"
    );
    let at = range(&engine);
    replace(
        &mut engine,
        id,
        SourceRange::new(at.start, at.start + "the installer always ".len()),
        "",
    );
    assert_eq!(
        engine.comments(id)[0].quote,
        "refuses an unsigned package",
        "deleting part shrinks it"
    );
}

#[test]
fn comments_never_change_the_page() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let page = setup.page();
    let on_disk = fs::read_to_string(&page).unwrap();
    let id = engine.open(&page).unwrap();
    let windows = text(&engine, id).find("the installer asks").unwrap();
    let new = add(
        &mut engine,
        id,
        SourceRange::new(windows, windows + "the installer asks".len()),
        "Asks what?",
    );
    let first = engine.comments(id)[0].id;
    for (command, change) in [
        (
            ids::COMMENTS_REPLY,
            CommentRequest::Reply {
                id,
                comment: new,
                text: "Where to install.".to_owned(),
            },
        ),
        (
            ids::COMMENTS_RESOLVE,
            CommentRequest::Resolve {
                id,
                comment: new,
                resolved: true,
            },
        ),
        (
            ids::COMMENTS_EDIT,
            CommentRequest::Edit {
                id,
                comment: first,
                reply: None,
                text: "Still true?".to_owned(),
            },
        ),
        (
            ids::COMMENTS_DELETE,
            CommentRequest::Delete {
                id,
                comment: first,
                reply: None,
            },
        ),
    ] {
        request(&mut engine, command, change);
        assert_eq!(text(&engine, id), on_disk);
        assert!(!engine.document(id).unwrap().dirty);
    }
    setup.settle(&mut engine);
    assert_eq!(fs::read_to_string(&page).unwrap(), on_disk);
    assert!(engine.undo_stack(id).unwrap().is_empty(), "nothing to undo");
}

#[test]
fn reply_resolve_reopen_edit_and_delete() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let comment = engine.comments(id)[0].id;
    request(
        &mut engine,
        ids::COMMENTS_REPLY,
        CommentRequest::Reply {
            id,
            comment,
            text: "Checked on 0.14.".to_owned(),
        },
    );
    let replies = engine.comments(id)[0].replies.clone();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1].author.name, "Paul Snow");
    assert_eq!(replies[1].author.email.as_deref(), Some("paul@example.org"));

    request(
        &mut engine,
        ids::COMMENTS_RESOLVE,
        CommentRequest::Resolve {
            id,
            comment,
            resolved: true,
        },
    );
    let resolved = engine.comments(id)[0].resolved.clone().unwrap();
    assert_eq!(resolved.by, "Paul Snow");
    assert_eq!(
        engine.comment_at(id, engine.comments(id)[0].range.start),
        None
    );
    request(
        &mut engine,
        ids::COMMENTS_REOPEN,
        CommentRequest::Resolve {
            id,
            comment,
            resolved: false,
        },
    );
    assert!(engine.comments(id)[0].resolved.is_none());

    request(
        &mut engine,
        ids::COMMENTS_EDIT,
        CommentRequest::Edit {
            id,
            comment,
            reply: Some(replies[1].id),
            text: "Checked on 0.14 and 0.15.".to_owned(),
        },
    );
    let edited = &engine.comments(id)[0].replies[1];
    assert_eq!(edited.text, "Checked on 0.14 and 0.15.");
    assert!(edited.edited.is_some());

    request(
        &mut engine,
        ids::COMMENTS_DELETE,
        CommentRequest::Delete {
            id,
            comment,
            reply: Some(replies[0].id),
        },
    );
    assert_eq!(engine.comments(id)[0].replies.len(), 1);

    // A request under another command's name is refused.
    let refused = engine.invoke(
        ids::COMMENTS_RESOLVE,
        CommandArgs::Comment(CommentRequest::Delete {
            id,
            comment,
            reply: None,
        }),
    );
    assert!(matches!(refused, Err(EngineError::InvalidArguments { .. })));
    // And a comment needs some text.
    let empty = engine.invoke(
        ids::COMMENTS_ADD,
        CommandArgs::Comment(CommentRequest::Add {
            id,
            range: SourceRange::new(0, 3),
            text: "  \n".to_owned(),
        }),
    );
    assert!(matches!(empty, Err(EngineError::Refused(_))));
}

#[test]
fn next_and_previous_comment_wrap() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let windows = text(&engine, id).find("the installer asks").unwrap();
    add(
        &mut engine,
        id,
        SourceRange::new(windows, windows + 3),
        "Here.",
    );
    let first = engine.comments(id)[0].range;
    let second = engine.comments(id)[1].range;
    let step = |engine: &mut Engine, command, offset| match engine
        .invoke(
            command,
            CommandArgs::Comment(CommentRequest::Step { id, offset }),
        )
        .unwrap()
    {
        CommandOutcome::Selection(range) => range,
        other => panic!("{other:?}"),
    };
    assert_eq!(step(&mut engine, ids::COMMENTS_NEXT, 0), first);
    assert_eq!(step(&mut engine, ids::COMMENTS_NEXT, first.start), second);
    assert_eq!(
        step(&mut engine, ids::COMMENTS_NEXT, second.start),
        first,
        "wraps"
    );
    assert_eq!(
        step(&mut engine, ids::COMMENTS_PREVIOUS, second.start),
        first
    );
    assert_eq!(
        step(&mut engine, ids::COMMENTS_PREVIOUS, first.start),
        second,
        "wraps"
    );
}

#[test]
fn a_comment_whose_text_is_deleted_is_detached_and_undo_brings_it_back() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let range = engine.comments(id)[0].range;
    replace(
        &mut engine,
        id,
        SourceRange::new(range.start - 1, range.end + 1),
        "",
    );
    let comment = &engine.comments(id)[0];
    assert!(comment.detached);
    assert_eq!(comment.quote, QUOTE, "it keeps the text it was on");
    assert_eq!(engine.comment_at(id, comment.range.start), None);

    assert!(engine.undo(id));
    setup.settle(&mut engine);
    let comment = &engine.comments(id)[0];
    assert!(!comment.detached, "found again once typing paused");
    assert_eq!(comment.range, range);
}

#[test]
fn a_comment_is_written_once_its_settle_passes() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let before = fs::read(setup.sidecar()).unwrap();
    let windows = text(&engine, id).find("the installer asks").unwrap();
    add(
        &mut engine,
        id,
        SourceRange::new(windows, windows + "the installer asks".len()),
        "Asks what?\nAnd when?",
    );
    engine.pump();
    assert_eq!(
        fs::read(setup.sidecar()).unwrap(),
        before,
        "not before its settle"
    );
    assert!(engine.next_due().is_some());
    setup.settle(&mut engine);
    let written = fs::read_to_string(setup.sidecar()).unwrap();
    assert!(written.contains("Asks what?\nAnd when?"), "{written}");
    assert!(written.contains("author = \"Paul Snow\""), "{written}");
    // The comment already there is as it was.
    assert!(
        written.starts_with(&String::from_utf8(before.clone()).unwrap()[..400]),
        "{written}"
    );
    let meta = adoc_engine::page_meta::parse(written.as_bytes()).unwrap();
    assert_eq!(meta.comments.len(), 2);
}

#[test]
fn a_page_save_leaves_the_sidecar_alone() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let before = fs::read(setup.sidecar()).unwrap();
    type_at(&mut engine, id, 0, "// A note.\n");
    engine
        .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        .unwrap();
    setup.settle(&mut engine);
    assert_eq!(fs::read(setup.sidecar()).unwrap(), before);
    // Found again in the saved page, by its quote, though its offsets are old.
    drop(engine);
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    assert_eq!(quotes(&engine, id), [QUOTE]);
}

#[test]
fn a_comment_is_found_again_after_an_outside_change() {
    let setup = Setup::new();
    let page = setup.page();
    let changed = fs::read_to_string(&page).unwrap().replace(
        "= Install\n",
        "= Install\n:toc:\n\nFirst, on Linux the installer refuses nothing.\n",
    );
    fs::write(&page, changed).unwrap();
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let comment = &engine.comments(id)[0];
    assert_eq!(comment.quote, QUOTE);
    assert!(!comment.detached);
    assert_eq!(
        &text(&engine, id)[comment.range.start - 9..comment.range.start],
        "On macOS "
    );
}

#[test]
fn a_comment_whose_text_went_elsewhere_is_detached() {
    let setup = Setup::new();
    let page = setup.page();
    let changed = fs::read_to_string(&page)
        .unwrap()
        .replace(QUOTE, "it is signed");
    fs::write(&page, changed).unwrap();
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let comment = &engine.comments(id)[0];
    assert!(comment.detached);
    assert_eq!(comment.quote, QUOTE);
}

#[test]
fn a_sidecar_changed_on_disk_keeps_both() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let windows = text(&engine, id).find("the installer asks").unwrap();
    add(
        &mut engine,
        id,
        SourceRange::new(windows, windows + 3),
        "Mine.",
    );

    // Someone else's comment arrives from a pull before this one is written.
    let theirs = fs::read_to_string(setup.sidecar()).unwrap()
        + "\n[[comment]]\nid = \"00000000000000aa\"\nauthor = \"Grace Hopper\"\n\
           created = 2026-11-21T07:00:00Z\nstart = 0\nend = 9\nrevision = \"sha256:old\"\n\
           quote = \"= Install\"\ntext = \"Theirs.\"\n";
    fs::write(setup.sidecar(), theirs).unwrap();
    setup.settle(&mut engine);
    let written = fs::read_to_string(setup.sidecar()).unwrap();
    let meta = adoc_engine::page_meta::parse(written.as_bytes()).unwrap();
    let texts: Vec<&str> = meta
        .comments
        .iter()
        .map(|comment| comment.text.as_str())
        .collect();
    assert!(texts.contains(&"Mine."), "{written}");
    assert!(texts.contains(&"Theirs."), "{written}");
    assert!(texts.contains(&"Is this still true for 0.14?"), "{written}");
    assert_eq!(engine.comments(id).len(), 3);
}

#[test]
fn an_unusable_sidecar_is_left_alone() {
    for (name, broken) in [
        (
            "conflict",
            "<<<<<<< HEAD\nformat = 1\n=======\nformat = 1\n>>>>>>> theirs\n".to_owned(),
        ),
        ("wrong type", "format = 1\n[[comment]]\nid = 7\n".to_owned()),
        ("duplicate ids", {
            let one = fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/fixtures/comments/modules/ROOT/pages/.install.adoc.afd"),
            )
            .unwrap();
            let again = one.split_once("[[comment]]").unwrap().1;
            format!("{one}\n[[comment]]{again}")
        }),
    ] {
        let setup = Setup::new();
        fs::write(setup.sidecar(), &broken).unwrap();
        let mut engine = setup.engine();
        let id = engine.open(&setup.page()).unwrap();
        let CommentsState::Unusable(reason) = engine.comments_state(id) else {
            panic!("{name}: usable");
        };
        if name == "conflict" {
            assert!(reason.contains("merge conflict"), "{reason}");
        }
        assert!(engine.comments(id).is_empty(), "{name}");
        let refused = engine.invoke(
            ids::COMMENTS_ADD,
            CommandArgs::Comment(CommentRequest::Add {
                id,
                range: SourceRange::new(0, 3),
                text: "Hello".to_owned(),
            }),
        );
        assert!(matches!(refused, Err(EngineError::Refused(_))), "{name}");
        setup.settle(&mut engine);
        drop(engine);
        assert_eq!(
            fs::read_to_string(setup.sidecar()).unwrap(),
            broken,
            "{name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_failed_sidecar_write_keeps_the_change() {
    use std::os::unix::fs::PermissionsExt as _;
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let folder = setup.sidecar().parent().unwrap().to_path_buf();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
    add(&mut engine, id, SourceRange::new(0, 3), "Kept.");
    setup.settle(&mut engine);
    let state = engine.comments_state(id);
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(&state, CommentsState::Unusable(reason) if reason.contains("could not be written")),
        "{state:?}"
    );
    assert_eq!(engine.comments(id).len(), 2, "the change is held");
    // Written once it can be.
    setup.clock.advance(Duration::from_secs(10));
    setup.settle(&mut engine);
    assert_eq!(engine.comments_state(id), CommentsState::Usable);
    assert!(fs::read_to_string(setup.sidecar())
        .unwrap()
        .contains("Kept."));
}

#[test]
fn the_last_comment_deleted_removes_the_sidecar() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let comment = engine.comments(id)[0].id;
    request(
        &mut engine,
        ids::COMMENTS_DELETE,
        CommentRequest::Delete {
            id,
            comment,
            reply: None,
        },
    );
    setup.settle(&mut engine);
    assert!(!setup.sidecar().exists());
}

#[test]
fn a_comment_on_a_page_closed_before_its_settle_is_written() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    add(&mut engine, id, SourceRange::new(0, 3), "Before closing.");
    engine.close(id).unwrap();
    setup.settle(&mut engine);
    assert!(fs::read_to_string(setup.sidecar())
        .unwrap()
        .contains("Before closing."));

    // And one made just before the engine goes.
    let id = engine.open(&setup.page()).unwrap();
    add(&mut engine, id, SourceRange::new(0, 3), "Before quitting.");
    drop(engine);
    assert!(fs::read_to_string(setup.sidecar())
        .unwrap()
        .contains("Before quitting."));
}

#[test]
fn a_page_without_comments_has_no_sidecar_until_one_is_made() {
    let setup = Setup::new();
    fs::remove_file(setup.sidecar()).unwrap();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    assert!(engine.comments(id).is_empty());
    assert_eq!(engine.comments_state(id), CommentsState::Usable);
    setup.settle(&mut engine);
    assert!(!setup.sidecar().exists());
    add(&mut engine, id, SourceRange::new(0, 9), "The first.");
    setup.settle(&mut engine);
    let written = fs::read_to_string(setup.sidecar()).unwrap();
    assert!(
        written.starts_with("# AdocFoundry page metadata\nformat = 1\n"),
        "{written}"
    );
}

#[test]
fn the_comment_author_comes_from_settings_first() {
    let setup = Setup::new();
    let engine = setup.engine();
    let author = engine.comment_author();
    assert_eq!(author.name, "Paul Snow");
    assert_eq!(author.email.as_deref(), Some("paul@example.org"));
}
