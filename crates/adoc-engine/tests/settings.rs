//! Settings layering through the engine.

use std::{fs, time::Duration};

use adoc_engine::{Engine, EngineEvent, Settings, ThemePreference, WORKSPACE_SETTINGS_FILE};

#[test]
fn settings_workspace_overrides_user() {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("adocfoundry.toml");
    fs::write(
        &user,
        "[editor]\nfont_size = 15\ntab_width = 4\ntheme = \"dark\"\n\n[tools]\nasciidoctor = \"/opt/asciidoctor\"\n",
    )
    .unwrap();
    let workspace = directory.path().join("docs");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join(WORKSPACE_SETTINGS_FILE),
        "[editor]\nfont_size = 18\n\n[workspace]\nignore = [\"drafts\"]\n",
    )
    .unwrap();

    let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
    assert_eq!(engine.settings().editor.font_size, 15);
    let events = engine.subscribe();

    engine.open_workspace(vec![workspace]).unwrap();
    engine.wait_for_workers(Duration::from_secs(10));
    let settings = engine.settings();
    assert_eq!(settings.editor.font_size, 18, "workspace overrides user");
    assert_eq!(
        settings.editor.tab_width, 4,
        "user value kept where not overridden"
    );
    assert_eq!(settings.editor.theme, ThemePreference::Dark);
    assert_eq!(settings.workspace.ignore, ["drafts"]);
    assert_eq!(settings.tools.asciidoctor, "/opt/asciidoctor");
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::SettingsChanged));
}

#[test]
fn unknown_setting_key_warns_not_fails() {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("adocfoundry.toml");
    fs::write(
        &user,
        "[editor]\nfont_size = 14\nfont_colour = \"red\"\n\n[experimental]\nflag = true\n",
    )
    .unwrap();

    let settings = Settings::load(Some(&user), None).unwrap();
    assert_eq!(settings.editor.font_size, 14, "known keys still apply");
    assert_eq!(settings.warnings.len(), 2);
    assert!(settings.warnings[0].contains("editor.font_colour"));
    assert!(settings.warnings[1].contains("experimental.flag"));
}
