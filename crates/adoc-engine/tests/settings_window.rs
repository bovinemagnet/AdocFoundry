//! The settings a window offers, what they are now, and saving one to the user settings file.

use std::{fs, path::PathBuf};

use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineError, EngineEvent, SettingChange, SettingKind,
    SettingSource, SettingValue, Settings,
};

struct Setup {
    directory: tempfile::TempDir,
    engine: Engine,
}

fn setup(user: &str, workspace: &str) -> Setup {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("adocfoundry.toml"), user).unwrap();
    fs::write(directory.path().join(".adocfoundry.toml"), workspace).unwrap();
    let settings = Settings::load(
        Some(&directory.path().join("adocfoundry.toml")),
        Some(directory.path()),
    )
    .unwrap()
    .with_git_opt_in();
    Setup {
        engine: Engine::new(settings),
        directory,
    }
}

impl Setup {
    fn user(&self) -> PathBuf {
        self.directory.path().join("adocfoundry.toml")
    }

    fn text(&self) -> String {
        fs::read_to_string(self.user()).unwrap()
    }

    fn save(&mut self, id: &str, value: Option<SettingValue>) -> Result<(), EngineError> {
        self.engine
            .invoke(
                ids::SETTINGS_SAVE,
                CommandArgs::Setting(SettingChange {
                    id: id.to_owned(),
                    value,
                }),
            )
            .map(|_| ())
    }

    fn value(&self, id: &str) -> SettingValue {
        self.engine.setting(id).unwrap().0
    }

    fn source(&self, id: &str) -> SettingSource {
        self.engine.setting(id).unwrap().1
    }
}

fn text(text: &str) -> SettingValue {
    SettingValue::Text(text.to_owned())
}

/// A value of `kind` that is not its default.
fn another(kind: &SettingKind, value: &SettingValue) -> SettingValue {
    match (kind, value) {
        (SettingKind::Switch, SettingValue::Bool(on)) => SettingValue::Bool(!on),
        (SettingKind::Number { min, .. }, _) => SettingValue::Number(f64::from(*min + 2)),
        (SettingKind::Scale { .. }, _) => SettingValue::Number(1.5),
        (SettingKind::Choice(choices), _) => text(choices.last().unwrap().0),
        (SettingKind::List, _) => SettingValue::List(vec!["pdf-a-2a".to_owned()]),
        (SettingKind::FontSet, _) => text("legible"),
        (SettingKind::Text | SettingKind::LightTheme | SettingKind::DarkTheme, _) => text("else"),
        (kind, value) => panic!("{value:?} is not of {kind:?}"),
    }
}

/// Each is offered once, is of its kind, is saved and read back, and goes back to its default
/// when taken out.
#[test]
fn every_setting_is_saved_and_read_back() {
    let specs = adoc_engine::setting_specs();
    for wanted in [
        "editor.font_family",
        "editor.font_size",
        "editor.soft_wrap",
        "editor.tab_width",
        "appearance.mode",
        "workspace.ignore",
        "tools.git",
        "git.enabled",
        "pdf.theme",
    ] {
        assert!(specs.iter().any(|spec| spec.id == wanted), "{wanted}");
    }
    let mut seen = std::collections::BTreeSet::new();
    for spec in specs {
        assert!(seen.insert(spec.id), "{} is offered twice", spec.id);
        assert!(!spec.title.is_empty() && !spec.description.is_empty());
        let mut setup = setup("", "");
        let default = setup.value(spec.id);
        assert_eq!(setup.source(spec.id), SettingSource::Default, "{}", spec.id);
        let value = another(&spec.kind, &default);
        assert_ne!(value, default, "{}", spec.id);
        setup.save(spec.id, Some(value.clone())).unwrap();
        assert_eq!(setup.value(spec.id), value, "{}: {}", spec.id, setup.text());
        assert_eq!(setup.source(spec.id), SettingSource::User, "{}", spec.id);
        assert!(
            setup.engine.settings().warnings.is_empty(),
            "{}: {:?}",
            spec.id,
            setup.engine.settings().warnings
        );
        // As another reader of the file finds it.
        let read = Settings::load(Some(&setup.user()), None).unwrap();
        assert!(read.warnings.is_empty());
        setup.save(spec.id, None).unwrap();
        assert_eq!(setup.value(spec.id), default, "{}", spec.id);
        assert_eq!(setup.source(spec.id), SettingSource::Default, "{}", spec.id);
    }
}

/// Only the setting's own value changes: comments, order and what is not known are kept.
#[test]
fn saving_changes_only_the_setting() {
    let mut setup = setup(
        "# mine\n[editor]\n# big enough\nfont_size = 13 # points\nmystery = true\n\n[keymap]\n\
         \"document.save\" = \"cmd-j\"\n",
        "",
    );
    setup
        .save("editor.font_size", Some(SettingValue::Number(18.)))
        .unwrap();
    assert_eq!(
        setup.text(),
        "# mine\n[editor]\n# big enough\nfont_size = 18 # points\nmystery = true\n\n[keymap]\n\
         \"document.save\" = \"cmd-j\"\n"
    );
    setup
        .save("git.show_markers", Some(SettingValue::Bool(false)))
        .unwrap();
    assert!(setup.text().ends_with("[git]\nshow_markers = false\n"));
    assert!(setup.text().starts_with("# mine\n[editor]\n# big enough\n"));
}

#[test]
fn a_saved_setting_applies_at_once() {
    let mut setup = setup("", "");
    let events = setup.engine.subscribe();
    setup
        .save("editor.font_size", Some(SettingValue::Number(20.)))
        .unwrap();
    assert_eq!(setup.engine.settings().editor.font_size, 20);
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::SettingsChanged));
}

/// What no settings file could say is refused, and the file is left as it was.
#[test]
fn a_value_that_cannot_be_used_is_refused() {
    let mut setup = setup("[editor]\ntab_width = 3\n", "");
    let before = setup.text();
    for (id, value) in [
        ("editor.tab_width", SettingValue::Number(0.)),
        ("editor.tab_width", SettingValue::Number(2.5)),
        ("editor.tab_width", text("wide")),
        ("editor.auto_save", text("sometimes")),
        ("pdf.standards", SettingValue::List(vec!["pdf-x".into()])),
        ("appearance.font_scale", SettingValue::Number(9.)),
        ("no.such", SettingValue::Bool(true)),
    ] {
        assert!(
            setup.save(id, Some(value.clone())).is_err(),
            "{id} {value:?}"
        );
    }
    assert_eq!(setup.text(), before);
}

/// A setting the workspace's file gives is shown as such, and cannot be changed over it.
#[test]
fn a_setting_the_workspace_gives_is_not_saved() {
    let mut setup = setup("[editor]\nfont_size = 13\n", "[editor]\nfont_size = 30\n");
    assert_eq!(setup.value("editor.font_size"), SettingValue::Number(30.));
    assert_eq!(setup.source("editor.font_size"), SettingSource::Workspace);
    let before = setup.text();
    let error = setup
        .save("editor.font_size", Some(SettingValue::Number(18.)))
        .unwrap_err();
    assert!(error.to_string().contains(".adocfoundry.toml"), "{error}");
    assert_eq!(setup.text(), before);
}

/// The older `[editor] theme` gives way to the mode saved.
#[test]
fn saving_the_mode_takes_out_the_older_setting() {
    let mut setup = setup("[editor]\ntheme = \"dark\"\n", "");
    assert_eq!(setup.value("appearance.mode"), text("dark"));
    setup.save("appearance.mode", Some(text("light"))).unwrap();
    assert_eq!(setup.value("appearance.mode"), text("light"));
    assert!(!setup.text().contains("theme ="), "{}", setup.text());
    assert!(setup.engine.settings().warnings.is_empty());
}

#[test]
fn an_empty_text_is_the_default() {
    let mut setup = setup("[tools]\ndot = \"/usr/bin/dot\"\n", "");
    setup.save("tools.dot", Some(text("  "))).unwrap();
    assert_eq!(setup.value("tools.dot"), text(""));
    assert_eq!(setup.source("tools.dot"), SettingSource::Default);
}

#[test]
fn without_a_user_file_nothing_is_saved() {
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let refused = engine.invoke(
        ids::SETTINGS_SAVE,
        CommandArgs::Setting(SettingChange {
            id: "git.enabled".to_owned(),
            value: Some(SettingValue::Bool(false)),
        }),
    );
    assert!(refused.is_err());
}

/// A file that does not parse is left alone.
#[test]
fn a_broken_file_is_not_written() {
    let mut setup = setup("", "");
    fs::write(setup.user(), "[editor\n").unwrap();
    assert!(setup
        .save("editor.tab_width", Some(SettingValue::Number(4.)))
        .is_err());
    assert_eq!(setup.text(), "[editor\n");
}

/// The file open in a tab follows a save; with edits of its own unsaved, it is not written under
/// them.
#[test]
fn a_settings_file_open_in_a_tab_follows() {
    let mut setup = setup("[editor]\ntab_width = 3\n", "");
    let id = setup.engine.open(&setup.user()).unwrap();
    setup
        .save("editor.tab_width", Some(SettingValue::Number(4.)))
        .unwrap();
    let document = setup.engine.document(id).unwrap();
    assert_eq!(document.text.to_string(), "[editor]\ntab_width = 4\n");
    assert!(!document.dirty);

    setup
        .engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(0, 0),
                text: "# more\n".to_owned(),
            },
        )
        .unwrap();
    let error = setup
        .save("editor.tab_width", Some(SettingValue::Number(8.)))
        .unwrap_err();
    assert!(error.to_string().contains("unsaved"), "{error}");
    assert_eq!(setup.text(), "[editor]\ntab_width = 4\n");
}

/// A settings file that is a link, as into a dotfiles repository, stays one.
#[cfg(unix)]
#[test]
fn a_linked_settings_file_stays_a_link() {
    let mut setup = setup("", "");
    let real = setup.directory.path().join("dotfiles.toml");
    fs::write(&real, "[editor]\ntab_width = 3\n").unwrap();
    fs::remove_file(setup.user()).unwrap();
    std::os::unix::fs::symlink(&real, setup.user()).unwrap();
    setup
        .save("editor.tab_width", Some(SettingValue::Number(4.)))
        .unwrap();
    assert!(fs::symlink_metadata(setup.user()).unwrap().is_symlink());
    assert_eq!(
        fs::read_to_string(real).unwrap(),
        "[editor]\ntab_width = 4\n"
    );
}

#[test]
fn a_settings_file_that_is_not_there_is_made() {
    let mut setup = setup("", "");
    fs::remove_file(setup.user()).unwrap();
    setup
        .save("editor.soft_wrap", Some(SettingValue::Bool(false)))
        .unwrap();
    assert_eq!(setup.text(), "[editor]\nsoft_wrap = false\n");
}

/// Opening the settings is the front end's to do; their file is made when it is asked for.
#[test]
fn the_settings_file_is_opened_by_its_own_command() {
    let mut setup = setup("", "");
    fs::remove_file(setup.user()).unwrap();
    let opened = setup
        .engine
        .invoke(ids::SETTINGS_OPEN, CommandArgs::None)
        .unwrap();
    assert_eq!(opened, adoc_engine::CommandOutcome::Done);
    assert!(!setup.user().exists());
    let opened = setup
        .engine
        .invoke(ids::SETTINGS_OPEN_FILE, CommandArgs::None)
        .unwrap();
    assert_eq!(opened, adoc_engine::CommandOutcome::External(setup.user()));
    assert!(setup.user().exists());
}
