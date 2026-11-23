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
    commands::ids, normalize_path, revision_of, CoAuthor, CommandArgs, Comment, CommentId,
    CommentRequest, DocumentId, Engine, EngineError, EngineEvent, ManualClock, Settings,
    SuggestionState, COMMENT_SETTLE, INDEX_SETTLE,
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
        self.engine_keeping_state(None)
    }

    /// An engine keeping its state, co-authors included, in `state`.
    fn engine_keeping_state(&self, state: Option<&Path>) -> Engine {
        let user = self.root.parent().unwrap().join("settings.toml");
        fs::write(
            &user,
            "[comments]\nname = \"Paul Snow\"\nemail = \"paul@example.org\"\n",
        )
        .unwrap();
        let settings = Settings::load(Some(&user), None)
            .unwrap()
            .with_git_opt_in()
            .with_state_dir(state.map(Path::to_path_buf));
        let mut engine = Engine::with_clock(settings, Arc::new(self.clock.clone()));
        engine.open_workspace(vec![self.root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        engine
    }

    fn page(&self) -> PathBuf {
        self.root.join("modules/ROOT/pages/install.adoc")
    }

    /// A new page of `text` named `name`.
    fn write_page(&self, name: &str, text: &str) -> PathBuf {
        let page = self.root.join("modules/ROOT/pages").join(name);
        fs::write(&page, text).unwrap();
        page
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
            original: None,
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

/// A front end's box holds the range it was begun on while the page may change: the text it
/// showed goes with the request, and a range no longer holding it is refused, so a replacement
/// is never paired with text its author did not see.
#[test]
fn a_suggestion_begun_on_other_text_is_refused() {
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let range = range_of(&engine, id, "the installer");
    let begun = |range| CommentRequest::Suggest {
        id,
        range,
        original: Some("the installer".to_owned()),
        replacement: "Gatekeeper".to_owned(),
        text: String::new(),
    };
    // Typed before the range while the box is open: the range now holds other text.
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(range.start - "macOS ".len(), range.start - "macOS ".len()),
                text: "Apple ".to_owned(),
            },
        )
        .unwrap();
    let before = engine.comments(id).len();
    let reason = refused(invoke(&mut engine, ids::COMMENTS_SUGGEST, begun(range)));
    assert_eq!(
        reason,
        "The text changed while you were suggesting; try again."
    );
    assert_eq!(engine.comments(id).len(), before, "nothing stored");

    // Begun again on the text as it is, it is made.
    let range = range_of(&engine, id, "the installer");
    invoke(&mut engine, ids::COMMENTS_SUGGEST, begun(range)).unwrap();
    assert_eq!(engine.comments(id).len(), before + 1);
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

fn accept(engine: &mut Engine, id: DocumentId, suggestion: CommentId) -> Result<(), EngineError> {
    invoke(
        engine,
        ids::COMMENTS_ACCEPT,
        CommentRequest::Accept {
            id,
            comment: suggestion,
        },
    )
}

fn reopen(engine: &mut Engine, id: DocumentId, suggestion: CommentId) {
    invoke(
        engine,
        ids::COMMENTS_REOPEN,
        CommentRequest::Resolve {
            id,
            comment: suggestion,
            resolved: false,
        },
    )
    .unwrap();
}

fn range_text(engine: &Engine, id: DocumentId, suggestion: CommentId) -> String {
    let range = comment(engine, id, suggestion).range;
    text(engine, id)[range.start..range.end].to_owned()
}

#[test]
fn accepting_applies_the_suggested_edit() {
    const REPLACEMENT: &str = "asks once before it starts";
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let before = text(&engine, id);
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(&mut engine, id, range, REPLACEMENT, "").unwrap();
    let events = engine.subscribe();
    accept(&mut engine, id, made).unwrap();

    assert_eq!(text(&engine, id), before.replacen(WINDOWS, REPLACEMENT, 1));
    let changes = events
        .try_iter()
        .filter(|event| matches!(event, EngineEvent::DocumentChanged { id: changed, .. } if *changed == id))
        .count();
    assert_eq!(changes, 1, "one edit of the page");
    assert_eq!(engine.undo_stack(id).unwrap().len(), 1, "one undo step");
    assert!(engine.document(id).unwrap().dirty);
    let found = comment(&engine, id, made);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Accepted);
    assert!(found.resolved.is_some());
    assert_eq!(
        found.range,
        SourceRange::new(range.start, range.start + REPLACEMENT.len())
    );
    assert!(!found.can.accept && !found.can.reject);
    assert!(found.can.resolve, "it may be reopened");
    let reason = refused(accept(&mut engine, id, made));
    assert_eq!(reason, "This suggestion is decided already.");
    assert_eq!(text(&engine, id), before.replacen(WINDOWS, REPLACEMENT, 1));

    setup.settle(&mut engine);
    let written = fs::read_to_string(setup.sidecar()).unwrap();
    assert!(written.contains("outcome = \"accepted\""), "{written}");
    assert_eq!(
        fs::read_to_string(setup.page()).unwrap(),
        before,
        "accepting edits the page, not its file"
    );
}

#[test]
fn undoing_an_accept_restores_the_text() {
    const REPLACEMENT: &str = "starts at once";
    let setup = Setup::new();
    let mut engine = setup.engine();
    let id = engine.open(&setup.page()).unwrap();
    let before = text(&engine, id);
    let after = before.replacen(WINDOWS, REPLACEMENT, 1);
    let range = range_of(&engine, id, WINDOWS);
    let made = suggest(&mut engine, id, range, REPLACEMENT, "").unwrap();
    accept(&mut engine, id, made).unwrap();
    assert_eq!(text(&engine, id), after);

    engine
        .invoke(ids::EDIT_UNDO, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(text(&engine, id), before);
    assert_eq!(
        state(&engine, id, made),
        SuggestionState::Accepted,
        "the outcome is a decision, not the text"
    );
    assert_eq!(comment(&engine, id, made).range, range);

    reopen(&mut engine, id, made);
    assert_eq!(state(&engine, id, made), SuggestionState::Pending);
    accept(&mut engine, id, made).unwrap();
    assert_eq!(text(&engine, id), after);
    assert_eq!(state(&engine, id, made), SuggestionState::Accepted);
}

#[test]
fn overlapping_suggestions_never_corrupt_the_text() {
    let setup = Setup::new();
    let original = "= Fox\n\nThe quick brown fox jumps over the lazy dog.\n";
    let page = setup.write_page("fox.adoc", original);
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    // No common start or end, so each edit replaces the whole of its range.
    let range = range_of(&engine, id, "quick brown");
    let a = suggest(&mut engine, id, range, "slow red", "").unwrap();
    let range = range_of(&engine, id, "brown fox");
    let b = suggest(&mut engine, id, range, "grey wolf", "").unwrap();
    let range = range_of(&engine, id, "lazy dog");
    let c = suggest(&mut engine, id, range, "sleepy cat", "").unwrap();

    accept(&mut engine, id, a).unwrap();
    let edited = original.replacen("quick brown", "slow red", 1);
    assert_eq!(text(&engine, id), edited);
    assert_eq!(state(&engine, id, b), SuggestionState::Outdated);
    assert_eq!(state(&engine, id, c), SuggestionState::Pending);
    assert_eq!(range_text(&engine, id, c), "lazy dog");
    let reason = refused(accept(&mut engine, id, b));
    assert_eq!(reason, "The text has changed since this was suggested.");
    assert_eq!(text(&engine, id), edited, "exactly the one edit accepted");
}

#[test]
fn accepting_cuts_the_edit_at_character_boundaries() {
    let setup = Setup::new();
    // "è" and "é" share their first byte: only whole characters are replaced.
    let original = "= Units\n\nThe word mètre is French.\n";
    let page = setup.write_page("units.adoc", original);
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let word = range_of(&engine, id, "word");
    let plain = add(&mut engine, id, word, "Which word?");
    let range = range_of(&engine, id, "mètre");
    let made = suggest(&mut engine, id, range, "métre", "").unwrap();
    let removed = comment(&engine, id, made).suggestion.unwrap().removed;
    assert_eq!(&original[removed.start..removed.end], "è");
    setup.settle(&mut engine);
    let written =
        fs::read_to_string(setup.root.join("modules/ROOT/pages/.units.adoc.afd")).unwrap();

    accept(&mut engine, id, made).unwrap();
    let edited = text(&engine, id);
    assert_eq!(edited, original.replacen("mètre", "métre", 1));
    assert!(std::str::from_utf8(edited.as_bytes()).is_ok());
    assert_eq!(range_text(&engine, id, made), "métre");
    let untouched = comment(&engine, id, plain);
    assert_eq!(untouched.range, word);
    assert!(!untouched.detached);
    setup.settle(&mut engine);
    let rewritten =
        fs::read_to_string(setup.root.join("modules/ROOT/pages/.units.adoc.afd")).unwrap();
    let block = |file: &str| {
        let start = file.find("Which word?").unwrap();
        file[..start]
            .rsplit("[[comment]]")
            .next()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        block(&rewritten),
        block(&written),
        "the comment before is not written again"
    );
}

#[test]
fn an_insertion_among_repeated_letters_is_applied_once() {
    let setup = Setup::new();
    let original = "= Letters\n\nSay aa now.\n";
    let page = setup.write_page("letters.adoc", original);
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let range = range_of(&engine, id, "aa");
    let made = suggest(&mut engine, id, range, "aaa", "").unwrap();
    accept(&mut engine, id, made).unwrap();
    let edited = text(&engine, id);
    assert_eq!(edited, "= Letters\n\nSay aaa now.\n");
    assert_eq!(edited.matches("aaa").count(), 1);
    assert_eq!(
        comment(&engine, id, made).range,
        SourceRange::new(range.start, range.start + 3),
        "the range covers the replacement, whatever its edges"
    );
    assert_eq!(range_text(&engine, id, made), "aaa");

    assert!(engine.undo(id));
    assert_eq!(text(&engine, id), original);
    assert_eq!(comment(&engine, id, made).range, range);
    reopen(&mut engine, id, made);
    assert_eq!(state(&engine, id, made), SuggestionState::Pending);
}

#[test]
fn accepting_a_deletion_leaves_its_suggestion_on_no_text() {
    let setup = Setup::new();
    let original = "= Cut\n\nSign it first, really, then ship it.\n";
    let page = setup.write_page("cut.adoc", original);
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let range = range_of(&engine, id, " really,");
    let made = suggest(&mut engine, id, range, "", "").unwrap();
    accept(&mut engine, id, made).unwrap();
    assert_eq!(text(&engine, id), "= Cut\n\nSign it first, then ship it.\n");
    let found = comment(&engine, id, made);
    assert_eq!(found.range, SourceRange::new(range.start, range.start));
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Accepted);

    assert!(engine.undo(id));
    setup.settle(&mut engine);
    assert_eq!(text(&engine, id), original);
    reopen(&mut engine, id, made);
    let found = comment(&engine, id, made);
    assert!(!found.detached, "found again by its words");
    assert_eq!(found.range, range);
    assert_eq!(found.suggestion.unwrap().state, SuggestionState::Pending);
}

#[test]
fn accepting_credits_the_suggestions_author() {
    let setup = Setup::new();
    let kept = tempfile::tempdir().unwrap();
    let suggested = setup.root.join("modules/ROOT/pages/suggested.adoc");
    let mut engine = setup.engine_keeping_state(Some(kept.path()));
    let id = engine.open(&suggested).unwrap();
    let ada = engine.comments(id)[0].id;
    assert_eq!(state(&engine, id, ada), SuggestionState::Pending);
    accept(&mut engine, id, ada).unwrap();
    assert_eq!(
        text(&engine, id),
        "= Suggested\n\nThe installer asks once before it starts.\n"
    );
    let credited = vec![CoAuthor {
        name: "Ada Lovelace".to_owned(),
        email: "ada@example.org".to_owned(),
    }];
    assert_eq!(
        engine.coauthors_of(std::slice::from_ref(&suggested)),
        credited
    );
    // Dropping the engine waits for its writes.
    drop(engine);
    let mut engine = setup.engine_keeping_state(Some(kept.path()));
    assert_eq!(
        engine.coauthors_of(std::slice::from_ref(&suggested)),
        credited
    );

    // One's own suggestion credits nobody.
    let id = engine.open(&setup.page()).unwrap();
    let range = range_of(&engine, id, WINDOWS);
    let own = suggest(&mut engine, id, range, "starts at once", "").unwrap();
    accept(&mut engine, id, own).unwrap();
    assert_eq!(state(&engine, id, own), SuggestionState::Accepted);
    assert!(engine.coauthors_of(&[setup.page()]).is_empty());

    // Nor does a suggestion with no address.
    let words = "= Nameless\n\nIt asks twice.\n";
    let nameless = setup.write_page("nameless.adoc", words);
    fs::write(
        setup.root.join("modules/ROOT/pages/.nameless.adoc.afd"),
        format!(
            "format = 2\n\n[[comment]]\nid = \"00000000000000a1\"\nauthor = \"Grace Hopper\"\n\
             created = 2026-11-22T09:00:00Z\nstart = 15\nend = 25\nrevision = \"{}\"\n\
             quote = \"asks twice\"\nbefore = \"= Nameless\\n\\nIt \"\nafter = \".\\n\"\n\
             text = \"\"\nresolved = false\n\n[comment.suggestion]\noriginal = \"asks twice\"\n\
             replacement = \"asks once\"\n",
            revision_of(words)
        ),
    )
    .unwrap();
    let id = engine.open(&nameless).unwrap();
    let grace = engine.comments(id)[0].id;
    assert_eq!(state(&engine, id, grace), SuggestionState::Pending);
    accept(&mut engine, id, grace).unwrap();
    assert_eq!(text(&engine, id), "= Nameless\n\nIt asks once.\n");
    assert!(engine.coauthors_of(&[nameless]).is_empty());
}

#[test]
fn nothing_is_accepted_from_a_read_only_comments_file() {
    let setup = Setup::new();
    let pages = setup.root.join("modules/ROOT/pages");
    let suggested = pages.join("suggested.adoc");
    let sidecar = pages.join(".suggested.adoc.afd");
    let newer = fs::read_to_string(&sidecar)
        .unwrap()
        .replace("format = 2", "format = 3");
    fs::write(&sidecar, &newer).unwrap();
    let mut engine = setup.engine();
    let id = engine.open(&suggested).unwrap();
    let before = text(&engine, id);
    let ada = engine.comments(id)[0].id;
    let result = accept(&mut engine, id, ada);
    assert!(matches!(result, Err(EngineError::Refused(_))), "{result:?}");
    assert_eq!(text(&engine, id), before);
    assert!(!engine.document(id).unwrap().dirty);
    assert!(engine.undo_stack(id).unwrap().is_empty());
    assert_eq!(fs::read_to_string(&sidecar).unwrap(), newer);
}

#[test]
fn a_suggestion_that_changes_nothing_is_not_accepted() {
    let setup = Setup::new();
    let pages = setup.root.join("modules/ROOT/pages");
    let suggested = pages.join("suggested.adoc");
    let sidecar = pages.join(".suggested.adoc.afd");
    // Edited by hand to suggest the text it is on.
    let unchanged = fs::read_to_string(&sidecar).unwrap().replace(
        "replacement = \"asks once\"",
        "replacement = \"asks twice\"",
    );
    fs::write(&sidecar, unchanged).unwrap();
    let mut engine = setup.engine();
    let id = engine.open(&suggested).unwrap();
    let before = text(&engine, id);
    let made = engine.comments(id)[0].id;
    assert_eq!(state(&engine, id, made), SuggestionState::Pending);
    let reason = refused(accept(&mut engine, id, made));
    assert_eq!(reason, "The suggestion changes nothing.");
    assert_eq!(text(&engine, id), before);
    assert!(engine.undo_stack(id).unwrap().is_empty());
    assert_eq!(state(&engine, id, made), SuggestionState::Pending);
}

#[test]
fn undoing_an_accept_at_a_range_edge_restores_its_range() {
    // Each deletes at one edge of its range: its start, then its end.
    for (name, original, replacement) in [
        ("start.adoc", "very good", "good"),
        ("end.adoc", "good today, really", "good today"),
    ] {
        let setup = Setup::new();
        let page = setup.write_page(name, "= Edge\n\nIt is very good today, really.\n");
        let mut engine = setup.engine();
        let id = engine.open(&page).unwrap();
        let before = text(&engine, id);
        let after = before.replacen(original, replacement, 1);
        let range = range_of(&engine, id, original);
        let made = suggest(&mut engine, id, range, replacement, "").unwrap();
        accept(&mut engine, id, made).unwrap();
        assert_eq!(text(&engine, id), after, "{name}");
        assert_eq!(range_text(&engine, id, made), replacement, "{name}");

        assert!(engine.undo(id));
        assert_eq!(text(&engine, id), before, "{name}");
        assert_eq!(comment(&engine, id, made).range, range, "{name}");
        assert_eq!(range_text(&engine, id, made), original, "{name}");
        // Redone and undone again: over the replacement, then the original.
        assert!(engine.redo(id));
        assert_eq!(text(&engine, id), after, "{name}");
        assert_eq!(range_text(&engine, id, made), replacement, "{name}");
        assert!(engine.undo(id));
        assert_eq!(comment(&engine, id, made).range, range, "{name}");
        reopen(&mut engine, id, made);
        assert_eq!(state(&engine, id, made), SuggestionState::Pending, "{name}");
        accept(&mut engine, id, made).unwrap();
        assert_eq!(text(&engine, id), after, "{name}");
        assert_eq!(range_text(&engine, id, made), replacement, "{name}");
    }
}

#[test]
fn undoing_an_accepted_deletion_never_moves_it() {
    let setup = Setup::new();
    let original = "= Twice\n\nShip it, really, today.\n\nThen rest, really, at home.\n";
    let page = setup.write_page("twice.adoc", original);
    let sidecar = setup.root.join("modules/ROOT/pages/.twice.adoc.afd");
    let mut engine = setup.engine();
    let id = engine.open(&page).unwrap();
    let range = range_of(&engine, id, " really,");
    let made = suggest(&mut engine, id, range, "", "").unwrap();
    accept(&mut engine, id, made).unwrap();
    // Long enough for a detached comment to be searched for, and its words are still on the
    // page, in the second sentence.
    setup.settle(&mut engine);
    let found = comment(&engine, id, made);
    assert_eq!(found.range, SourceRange::new(range.start, range.start));

    assert!(engine.undo(id));
    assert_eq!(text(&engine, id), original);
    let found = comment(&engine, id, made);
    assert!(!found.detached);
    assert_eq!(found.range, range, "back in its own place");
    reopen(&mut engine, id, made);
    assert_eq!(state(&engine, id, made), SuggestionState::Pending);
    setup.settle(&mut engine);
    assert_eq!(comment(&engine, id, made).range, range);
    let written = fs::read_to_string(&sidecar).unwrap();
    assert!(!written.contains("uncertain"), "{written}");
}
