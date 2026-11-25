//! `[keymap]`: the keys a user gives commands, over the commands' own.

use std::{fs, path::Path};

use adoc_engine::{commands::ids, Engine, Settings};

/// An engine whose user settings file holds `text`.
fn engine(text: &str) -> (tempfile::TempDir, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("adocfoundry.toml");
    fs::write(&user, text).unwrap();
    let settings = Settings::load(Some(&user), None).unwrap().with_git_opt_in();
    (directory, Engine::new(settings))
}

fn write(directory: &Path, text: &str) {
    fs::write(directory.join("adocfoundry.toml"), text).unwrap();
}

/// What is wrong with a keymap, whether the settings say so or the engine.
fn problems(engine: &Engine) -> Vec<String> {
    let mut problems = engine.settings().warnings.clone();
    problems.extend(engine.keymap().problems().iter().cloned());
    problems
}

#[test]
fn without_a_keymap_every_command_has_its_own_key() {
    let (_directory, engine) = engine("");
    let keymap = engine.keymap();
    for spec in engine.commands().all() {
        assert_eq!(keymap.keys(spec.id), spec.default_binding, "{:?}", spec.id);
        assert!(!keymap.is_custom(spec.id));
    }
    assert!(problems(&engine).is_empty());
}

/// Every default is in the notation the settings file uses, and no two commands share one.
#[test]
fn the_defaults_are_written_as_the_settings_file_would_write_them() {
    let (_directory, engine) = engine("");
    let mut seen = std::collections::BTreeMap::new();
    for spec in engine.commands().all() {
        let Some(keys) = spec.default_binding else {
            continue;
        };
        assert_eq!(adoc_engine::Keys::parse(keys).unwrap().as_str(), keys);
        assert_eq!(seen.insert(keys, spec.id), None, "{keys}");
    }
}

#[test]
fn a_command_is_given_other_keys_or_none() {
    let (_directory, engine) = engine(
        "[keymap]\n\"document.close_all\" = \"cmd-k x\"\n\"document.save\" = \"\"\n\
         \"edit.copy_markdown\" = \"cmd-alt-c\"\n",
    );
    let keymap = engine.keymap();
    assert_eq!(keymap.keys(ids::DOCUMENT_CLOSE_ALL), Some("cmd-k x"));
    assert_eq!(keymap.keys(ids::DOCUMENT_SAVE), None);
    assert_eq!(keymap.keys(ids::EDIT_COPY_MARKDOWN), Some("cmd-alt-c"));
    assert!(keymap.is_custom(ids::DOCUMENT_CLOSE_ALL));
    assert!(keymap.is_custom(ids::DOCUMENT_SAVE));
    assert_eq!(keymap.keys(ids::DOCUMENT_CLOSE), Some("cmd-w"));
    assert!(problems(&engine).is_empty(), "{:?}", problems(&engine));
}

/// `document.save = "…"` without quotes is a table inside `[keymap]` to TOML.
#[test]
fn an_id_needs_no_quotes() {
    let (_directory, engine) = engine("[keymap]\ndocument.close_all = \"cmd-k x\"\n");
    assert_eq!(
        engine.keymap().keys(ids::DOCUMENT_CLOSE_ALL),
        Some("cmd-k x")
    );
    assert!(problems(&engine).is_empty());
}

#[test]
fn keys_may_be_written_in_any_order_and_case() {
    let (_directory, engine) =
        engine("[keymap]\n\"document.close_all\" = \" Shift-CMD-X   ctrl-Enter \"\n");
    assert_eq!(
        engine.keymap().keys(ids::DOCUMENT_CLOSE_ALL),
        Some("cmd-shift-x ctrl-enter")
    );
}

/// A key given to one command is taken from the command that had it.
#[test]
fn a_key_is_taken_from_the_command_it_belongs_to() {
    let (_directory, engine) = engine("[keymap]\n\"document.close_all\" = \"cmd-s\"\n");
    let keymap = engine.keymap();
    assert_eq!(keymap.keys(ids::DOCUMENT_CLOSE_ALL), Some("cmd-s"));
    assert_eq!(keymap.keys(ids::DOCUMENT_SAVE), None);
    assert!(!keymap.is_custom(ids::DOCUMENT_SAVE));
    assert!(problems(&engine).is_empty());
}

#[test]
fn two_commands_can_swap_keys() {
    let (_directory, engine) =
        engine("[keymap]\n\"document.save\" = \"cmd-w\"\n\"document.close\" = \"cmd-s\"\n");
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_SAVE), Some("cmd-w"));
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_CLOSE), Some("cmd-s"));
    assert!(problems(&engine).is_empty());
}

#[test]
fn one_key_given_to_two_commands_is_the_firsts_and_warned_about() {
    let (_directory, engine) =
        engine("[keymap]\n\"document.close_all\" = \"cmd-j\"\n\"document.save\" = \"cmd-j\"\n");
    let keymap = engine.keymap();
    assert_eq!(keymap.keys(ids::DOCUMENT_CLOSE_ALL), Some("cmd-j"));
    assert_eq!(keymap.keys(ids::DOCUMENT_SAVE), Some("cmd-s"), "its own");
    let problems = problems(&engine);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("`keymap.document.save`")
            && problems[0].contains("document.close_all"),
        "{problems:?}"
    );
}

#[test]
fn an_unknown_command_and_keys_that_cannot_be_read_are_warned_about() {
    let (_directory, engine) = engine(
        "[keymap]\n\"no.such\" = \"cmd-j\"\n\"document.save\" = \"cmd-\"\n\
         \"document.close\" = \"hyper-s\"\n\"document.open\" = 7\n\"edit.undo\" = \"cmd-foo\"\n",
    );
    let problems = problems(&engine);
    for name in [
        "no.such",
        "document.save",
        "document.close",
        "document.open",
        "edit.undo",
    ] {
        assert_eq!(
            problems
                .iter()
                .filter(|problem| problem.contains(&format!("`keymap.{name}`")))
                .count(),
            1,
            "{name}: {problems:?}"
        );
    }
    // Each keeps the key it had.
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_SAVE), Some("cmd-s"));
    assert_eq!(engine.keymap().keys(ids::EDIT_UNDO), Some("cmd-z"));
}

/// Keys that type would make typing impossible.
#[test]
fn a_key_that_types_is_refused() {
    for keys in [
        "x",
        "shift-x",
        "space",
        "enter",
        "backspace",
        "left",
        "shift-tab",
        "escape",
        "x cmd-s",
    ] {
        let (_directory, engine) = engine(&format!("[keymap]\n\"document.save\" = \"{keys}\"\n"));
        assert_eq!(
            engine.keymap().keys(ids::DOCUMENT_SAVE),
            Some("cmd-s"),
            "{keys}"
        );
        assert_eq!(problems(&engine).len(), 1, "{keys}");
    }
    // A command whose own key is such a key may be given it again, and a chord may end in one.
    let (_directory, engine) = engine(
        "[keymap]\n\"edit.indent_list_item\" = \"tab\"\n\"document.save\" = \"cmd-k x\"\n\
         \"document.close\" = \"f5\"\n",
    );
    assert!(problems(&engine).is_empty(), "{:?}", problems(&engine));
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_CLOSE), Some("f5"));
}

/// A key that also begins another command's keys leaves those hard or impossible to type.
#[test]
fn a_key_that_begins_anothers_is_warned_about() {
    let (_directory, engine) = engine("[keymap]\n\"document.save\" = \"cmd-k\"\n");
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_SAVE), Some("cmd-k"));
    let one = problems(&engine);
    assert_eq!(one.len(), 1, "{one:?}");
    assert!(one[0].contains("`keymap.document.save`"), "{one:?}");

    let (_directory, engine) = self::engine("[keymap]\n\"document.close\" = \"cmd-s x\"\n");
    let two = problems(&engine);
    assert_eq!(two.len(), 1, "{two:?}");
    assert!(two[0].contains("document.save"), "{two:?}");
}

#[test]
fn a_workspace_file_cannot_set_keys() {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("adocfoundry.toml");
    fs::write(
        directory.path().join(".adocfoundry.toml"),
        "[keymap]\n\"document.save\" = \"cmd-j\"\n",
    )
    .unwrap();
    let settings = Settings::load(Some(&user), Some(directory.path()))
        .unwrap()
        .with_git_opt_in();
    let engine = Engine::new(settings);
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_SAVE), Some("cmd-s"));
    let problems = problems(&engine);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("can only be set in the user settings file"));
}

#[test]
fn a_reload_changes_the_keys_and_taking_the_line_out_gives_the_default_back() {
    let (directory, mut engine) = engine("");
    write(
        directory.path(),
        "[keymap]\n\"document.save\" = \"cmd-j\"\n",
    );
    engine.reload_settings().unwrap();
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_SAVE), Some("cmd-j"));
    write(directory.path(), "");
    engine.reload_settings().unwrap();
    assert_eq!(engine.keymap().keys(ids::DOCUMENT_SAVE), Some("cmd-s"));
}
