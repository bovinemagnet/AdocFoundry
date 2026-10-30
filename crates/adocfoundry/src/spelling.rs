//! What both editors offer on a misspelt word, and carrying it out through the engine: a
//! suggestion replaces the word in one undo step; a dictionary takes the word.

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, CommandArgs, DocumentId, Engine, EngineError};
use gpui::{App, Entity};

use crate::app::AppState;

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Replace { range: SourceRange, text: String },
    AddToWorkspace(SourceRange),
    AddToUser(SourceRange),
}

/// A spelling choice made in the source editor's menu, whose items are actions.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = adocfoundry, no_json)]
pub struct ApplySpelling {
    pub document: DocumentId,
    pub choice: Choice,
}

/// The choices for the misspelt word at `offset` of `document`: its suggestions, then the
/// dictionaries it may be added to (the user's only with a user settings file). Empty when no
/// misspelt word is there.
pub fn choices(engine: &Engine, document: DocumentId, offset: usize) -> Vec<(String, Choice)> {
    let Some(misspelling) = engine
        .misspellings(document)
        .iter()
        .find(|misspelling| misspelling.range.start <= offset && offset <= misspelling.range.end)
    else {
        return Vec::new();
    };
    let range = misspelling.range;
    let mut choices: Vec<(String, Choice)> = misspelling
        .suggestions
        .iter()
        .map(|text| {
            (
                text.clone(),
                Choice::Replace {
                    range,
                    text: text.clone(),
                },
            )
        })
        .collect();
    choices.push((
        "Add to Workspace Dictionary".to_owned(),
        Choice::AddToWorkspace(range),
    ));
    if engine.settings().user_file().is_some() {
        choices.push((
            "Add to User Dictionary".to_owned(),
            Choice::AddToUser(range),
        ));
    }
    choices
}

/// Carries out `choice` in `document`.
pub fn apply(
    app: &Entity<AppState>,
    document: DocumentId,
    choice: &Choice,
    cx: &mut App,
) -> Result<(), EngineError> {
    let (command, args) = match choice.clone() {
        Choice::Replace { range, text } => (
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id: document,
                range,
                text,
            },
        ),
        Choice::AddToWorkspace(range) => (
            ids::SPELLING_ADD_TO_WORKSPACE,
            CommandArgs::Range {
                id: document,
                range,
            },
        ),
        Choice::AddToUser(range) => (
            ids::SPELLING_ADD_TO_USER,
            CommandArgs::Range {
                id: document,
                range,
            },
        ),
    };
    app.update(cx, |app, cx| app.invoke(command, args, cx))
        .map(|_| ())
}
