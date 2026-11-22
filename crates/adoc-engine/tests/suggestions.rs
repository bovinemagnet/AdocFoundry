//! Suggested changes on a page's text (SUGGEST 1/4): a comment that proposes a replacement for
//! its range, pending while the range holds the text it was suggested on, outdated otherwise,
//! and decided by accepting or rejecting it.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, normalize_path, CommandArgs, Comment, CommentId, CommentRequest, DocumentId,
    Engine, EngineError, ManualClock, Settings, SuggestionState, COMMENT_SETTLE, INDEX_SETTLE,
};

const WINDOWS: &str = "asks before it starts";

struct Setup {
    _directory: tempfile::TempDir,
    root: PathBuf,
    clock: ManualClock,
}

impl Setup {
    /// A copy of `tests/fixtures/comments`.
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

    /// Past the settle of a comment change, with what it wrote on disk.
    fn settle(&self, engine: &mut Engine) {
        self.clock.advance(COMMENT_SETTLE + INDEX_SETTLE);
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

fn invoke(
    engine: &mut Engine,
    command: adoc_engine::CommandId,
    request: CommentRequest,
) -> Result<(), EngineError> {
    engine
        .invoke(command, CommandArgs::Comment(request))
        .map(|_| ())
}

fn suggest(
    engine: &mut Engine,
    id: DocumentId,
    range: SourceRange,
    replacement: &str,
    note: &str,
) -> Result<CommentId, EngineError> {
    let before: Vec<CommentId> = engine.comments(id).iter().map(|c| c.id).collect();
    invoke(
        engine,
        ids::COMMENTS_SUGGEST,
        CommentRequest::Suggest {
            id,
            range,
            replacement: replacement.to_owned(),
            text: note.to_owned(),
        },
    )?;
    Ok(engine
        .comments(id)
        .into_iter()
        .map(|comment| comment.id)
        .find(|comment| !before.contains(comment))
        .unwrap())
}

fn add(engine: &mut Engine, id: DocumentId, range: SourceRange, text: &str) -> CommentId {
    let before: Vec<CommentId> = engine.comments(id).iter().map(|c| c.id).collect();
    invoke(
        engine,
        ids::COMMENTS_ADD,
        CommentRequest::Add {
            id,
            range,
            text: text.to_owned(),
        },
    )
    .unwrap();
    engine
        .comments(id)
        .into_iter()
        .map(|comment| comment.id)
        .find(|comment| !before.contains(comment))
        .unwrap()
}

fn comment(engine: &Engine, id: DocumentId, comment: CommentId) -> Comment {
    engine
        .comments(id)
        .into_iter()
        .find(|found| found.id == comment)
        .unwrap()
}

fn state(engine: &Engine, id: DocumentId, suggestion: CommentId) -> SuggestionState {
    comment(engine, id, suggestion).suggestion.unwrap().state
}

fn text(engine: &Engine, id: DocumentId) -> String {
    engine.document(id).unwrap().text.to_string()
}

fn range_of(engine: &Engine, id: DocumentId, words: &str) -> SourceRange {
    let start = text(engine, id).find(words).unwrap();
    SourceRange::new(start, start + words.len())
}

fn refused(result: Result<impl std::fmt::Debug, EngineError>) -> String {
    match result {
        Err(EngineError::Refused(reason)) => reason,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_suggestion_is_made_on_the_selection() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let before = text(&engine, id);
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(
        &mut engine,
        id,
        range,
        "asks once before it starts",
        "  Clearer.  ",
    )
    .unwrap();
    let found = comment(&engine, id, made);
    let suggestion = found.suggestion.clone().unwrap();
    assert_eq!(suggestion.original, WINDOWS);
    assert_eq!(suggestion.replacement, "asks once before it starts");
    assert_eq!(suggestion.state, SuggestionState::Pending);
    assert_eq!(found.range, range);
    assert_eq!(found.text, "  Clearer.", "the note is trimmed at its end");
    assert!(found.can.accept && found.can.reject);
    assert!(!found.can.resolve, "resolved by accepting or rejecting");
    // Only what differs would be replaced: "once " typed after "asks ".
    assert_eq!(
        suggestion.removed,
        SourceRange::new(range.start + 5, range.start + 5)
    );
    assert_eq!(suggestion.inserted, "once ");
    assert_eq!(text(&engine, id), before);
    assert!(!engine.document(id).unwrap().dirty);
    assert!(engine.undo_stack(id).unwrap().is_empty());

    // A plain comment suggests nothing and cannot be accepted or rejected.
    let plain = engine.comments(id)[0].clone();
    assert!(plain.suggestion.is_none());
    assert!(!plain.can.accept && !plain.can.reject);
    // A suggestion's note may be empty.
    let windows = range_of(&engine, id, "Windows");
    let quiet = suggest(&mut engine, id, windows, "", "").unwrap();
    assert_eq!(comment(&engine, id, quiet).text, "");
}

#[test]
fn rejecting_leaves_no_trace_in_the_text() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let before = text(&engine, id);
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(&mut engine, id, range, "starts at once", "").unwrap();
    invoke(
        &mut engine,
        ids::COMMENTS_REJECT,
        CommentRequest::Reject { id, comment: made },
    )
    .unwrap();
    assert_eq!(text(&engine, id), before);
    assert!(!engine.document(id).unwrap().dirty);
    let found = comment(&engine, id, made);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Rejected);
    assert!(found.resolved.is_some());
    assert!(!found.can.accept && !found.can.reject);
    assert!(found.can.resolve, "it may be reopened");
    let reason = refused(invoke(
        &mut engine,
        ids::COMMENTS_EDIT,
        CommentRequest::EditSuggestion {
            id,
            comment: made,
            replacement: "starts now".to_owned(),
            text: String::new(),
        },
    ));
    assert_eq!(reason, "Reopen this suggestion first.");
    setup.settle(&mut engine);
    let written = fs::read_to_string(setup.sidecar()).unwrap();
    assert!(written.contains("outcome = \"rejected\""), "{written}");
    assert!(written.contains("format = 2"), "{written}");
    assert_eq!(fs::read_to_string(setup.page()).unwrap(), before);

    invoke(
        &mut engine,
        ids::COMMENTS_REOPEN,
        CommentRequest::Resolve {
            id,
            comment: made,
            resolved: false,
        },
    )
    .unwrap();
    let found = comment(&engine, id, made);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Pending);
    assert!(found.resolved.is_none());
    setup.settle(&mut engine);
    let written = fs::read_to_string(setup.sidecar()).unwrap();
    assert!(!written.contains("outcome"), "{written}");
}

#[test]
fn an_outdated_suggestion_cannot_be_accepted() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(&mut engine, id, range, "asks once before it starts", "").unwrap();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(range.start + 5, range.start + 5),
                text: "twice ".to_owned(),
            },
        )
        .unwrap();
    let found = comment(&engine, id, made);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Outdated);
    assert!(!found.can.accept);
    assert!(found.can.reject, "an outdated one may still be rejected");
    let reason = refused(invoke(
        &mut engine,
        ids::COMMENTS_ACCEPT,
        CommentRequest::Accept { id, comment: made },
    ));
    assert_eq!(reason, "The text has changed since this was suggested.");

    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(state(&engine, id, made), SuggestionState::Pending);
}

#[test]
fn a_suggestion_whose_text_is_deleted_is_found_again_only_where_it_was() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(&mut engine, id, range, "starts at once", "").unwrap();
    let around = SourceRange::new(range.start - 1, range.end + 1);
    let delete = |engine: &mut Engine| {
        engine
            .invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: around,
                    text: String::new(),
                },
            )
            .unwrap();
    };
    delete(&mut engine);
    let found = comment(&engine, id, made);
    assert!(found.detached);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Outdated);

    // Undone: found again in its own place, and pending.
    assert!(engine.undo(id));
    setup.settle(&mut engine);
    let found = comment(&engine, id, made);
    assert!(!found.detached);
    assert_eq!(found.range, range);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Pending);

    // Deleted again and its words typed elsewhere: found there, but not sure of it.
    delete(&mut engine);
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(0, 0),
                text: format!("{WINDOWS}\n"),
            },
        )
        .unwrap();
    setup.settle(&mut engine);
    let found = comment(&engine, id, made);
    assert!(!found.detached);
    assert_eq!(found.range, SourceRange::new(0, WINDOWS.len()));
    assert_eq!(
        found.suggestion.unwrap().state,
        SuggestionState::Outdated,
        "the same words, not the same place"
    );
}

#[test]
fn a_suggestion_never_moves_to_the_same_words_elsewhere() {
    const WORDS: &str = "the build signs the package";
    let setup = Setup::new();
    let page = setup.root.join("modules/ROOT/pages/twice.adoc");
    let original = format!(
        "= Twice\n\nOn Linux {WORDS} first.\n\nOn macOS {WORDS} first.\n\nAnd that is all.\n"
    );
    fs::write(&page, &original).unwrap();
    let sidecar = setup.root.join("modules/ROOT/pages/.twice.adoc.afd");
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let second = original.rfind(WORDS).unwrap();
    let second = SourceRange::new(second, second + WORDS.len());
    let made = suggest(&mut engine, id, second, "the release job signs it", "").unwrap();
    let plain = add(&mut engine, id, second, "Which build?");
    assert_eq!(state(&engine, id, made), SuggestionState::Pending);
    setup.settle(&mut engine);
    engine.close(id).unwrap();
    setup.settle(&mut engine);
    assert!(fs::read_to_string(&sidecar)
        .unwrap()
        .contains("the release job"));

    // The second sentence is rewritten elsewhere, its words kept: the first occurrence is now
    // the better match for the words around them.
    let changed = original.replace(
        &format!("On macOS {WORDS} first."),
        &format!("Then {WORDS} last."),
    );
    fs::write(&page, &changed).unwrap();
    let id = engine.open(&page).unwrap();
    let found = comment(&engine, id, made);
    assert_eq!(
        &text(&engine, id)[found.range.start..found.range.end],
        WORDS,
        "placed as a comment would be"
    );
    assert_eq!(
        found.suggestion.unwrap().state,
        SuggestionState::Outdated,
        "never pending on words it was not suggested on"
    );
    assert!(!found.can.accept);
    let moved = comment(&engine, id, plain);
    assert!(!moved.detached, "a plain comment still moves to its words");
    assert_eq!(
        &text(&engine, id)[moved.range.start..moved.range.end],
        WORDS
    );

    // Written again, with the page's revision, it stays uncertain.
    invoke(
        &mut engine,
        ids::COMMENTS_REPLY,
        CommentRequest::Reply {
            id,
            comment: made,
            text: "Where did this go?".to_owned(),
        },
    )
    .unwrap();
    setup.settle(&mut engine);
    engine.close(id).unwrap();
    setup.settle(&mut engine);
    let written = fs::read_to_string(&sidecar).unwrap();
    assert!(written.contains("uncertain = true"), "{written}");
    let id = engine.open(&page).unwrap();
    assert_eq!(state(&engine, id, made), SuggestionState::Outdated);

    // Its author suggests again on the text as it is: certain, and pending.
    let range = comment(&engine, id, made).range;
    invoke(
        &mut engine,
        ids::COMMENTS_EDIT,
        CommentRequest::EditSuggestion {
            id,
            comment: made,
            replacement: "the release job signs it".to_owned(),
            text: String::new(),
        },
    )
    .unwrap();
    let found = comment(&engine, id, made);
    assert_eq!(found.range, range);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Pending);
    setup.settle(&mut engine);
    let written = fs::read_to_string(&sidecar).unwrap();
    assert!(!written.contains("uncertain"), "{written}");
}

#[test]
fn a_suggestion_is_found_again_after_an_outside_change() {
    let setup = Setup::new();
    let page = setup.page();
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(&mut engine, id, range, "starts at once", "Shorter.").unwrap();
    setup.settle(&mut engine);
    engine.close(id).unwrap();
    setup.settle(&mut engine);

    let added = "A paragraph added on another branch.\n\n";
    let changed = fs::read_to_string(&page)
        .unwrap()
        .replacen("\n\n", &format!("\n\n{added}"), 1);
    fs::write(&page, changed).unwrap();
    let id = engine.open(&page).unwrap();
    let found = comment(&engine, id, made);
    assert_eq!(
        found.range,
        SourceRange::new(range.start + added.len(), range.end + added.len())
    );
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Pending);
}

#[test]
fn suggesting_is_refused_beyond_its_limits() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let range = range_of(&engine, id, WINDOWS);

    let empty = suggest(
        &mut engine,
        id,
        SourceRange::new(range.start, range.start),
        "x",
        "",
    );
    assert!(
        matches!(empty, Err(EngineError::InvalidRequest(_))),
        "{empty:?}"
    );

    let long = "word ".repeat(1_000);
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(range.end, range.end),
                text: format!(" {long}"),
            },
        )
        .unwrap();
    let reason = refused(suggest(
        &mut engine,
        id,
        SourceRange::new(range.start, range.start + 4 * 1024 + 1),
        "x",
        "",
    ));
    assert_eq!(reason, "Select at most 4 KiB of text to suggest a change.");
    assert!(suggest(
        &mut engine,
        id,
        SourceRange::new(range.start, range.start + 4 * 1024),
        "x",
        "",
    )
    .is_ok());

    refused(suggest(&mut engine, id, range, WINDOWS, ""));
    refused(suggest(&mut engine, id, range, "asks\u{7} first", ""));
    refused(suggest(
        &mut engine,
        id,
        range,
        &"x".repeat(8 * 1024 + 1),
        "",
    ));
    refused(suggest(
        &mut engine,
        id,
        range,
        "x",
        &"y".repeat(8 * 1024 + 1),
    ));
    assert!(suggest(&mut engine, id, range, "asks\n\tfirst\r\n", "").is_ok());
}

#[test]
fn resolving_a_suggestion_is_refused() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(&mut engine, id, range, "starts at once", "").unwrap();
    let reason = refused(invoke(
        &mut engine,
        ids::COMMENTS_RESOLVE,
        CommentRequest::Resolve {
            id,
            comment: made,
            resolved: true,
        },
    ));
    assert_eq!(
        reason,
        "Accept or reject a suggestion instead of resolving it."
    );
    let found = comment(&engine, id, made);
    assert!(found.resolved.is_none());
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Pending);
}
