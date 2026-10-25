//! WP 6.4: the update check as the app drives it, with the settings and the state it keeps in
//! a temporary configuration directory.

use adoc_engine::{commands::ids, updates, CommandArgs, CommandOutcome, Engine, Settings};

const NOW: u64 = 1_800_000_000;

fn feed(tag: &str) -> String {
    format!(r#"{{"tag_name": "{tag}", "html_url": "https://example.org/{tag}", "assets": []}}"#)
}

fn engine_with(settings: &str) -> (Engine, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("settings.toml");
    std::fs::write(&file, settings).unwrap();
    let settings = Settings::load(Some(&file), None).unwrap();
    assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    (Engine::new(settings), directory)
}

#[test]
fn updates_can_be_switched_off() {
    let (mut engine, _directory) = engine_with("[updates]\ncheck = false\n");
    assert!(!engine.settings().updates.check);
    assert!(!engine.update_check_due(NOW));
    // The command is the user asking, so it still checks.
    assert_eq!(
        engine
            .invoke(ids::APP_CHECK_FOR_UPDATES, CommandArgs::None)
            .unwrap(),
        CommandOutcome::CheckForUpdates(updates::FEED.to_owned())
    );

    let (engine, _directory) = engine_with("");
    assert!(engine.settings().updates.check, "on by default");
    assert!(engine.update_check_due(NOW));
}

#[test]
fn a_check_at_start_up_announces_each_release_once() {
    let (mut engine, directory) = engine_with("");
    let newer = feed("v99.0.0");
    let release = engine.record_update_check(&newer, NOW, false).unwrap();
    assert_eq!(
        release.map(|release| release.tag).as_deref(),
        Some("v99.0.0")
    );
    assert!(directory.path().join(updates::STATE_FILE).is_file());
    // Not due again for a day, and not announced again at start-up.
    assert!(!engine.update_check_due(NOW + 60));
    assert!(engine.update_check_due(NOW + updates::CHECK_INTERVAL));
    assert_eq!(
        engine.record_update_check(&newer, NOW + 60, false),
        Ok(None)
    );
    // Asked for, it is shown again.
    assert!(engine
        .record_update_check(&newer, NOW + 60, true)
        .unwrap()
        .is_some());
    // Nothing newer: nothing to announce, asked or not.
    assert_eq!(
        engine.record_update_check(&feed("v0.0.1"), NOW, true),
        Ok(None)
    );
    // An unreadable feed records nothing.
    let (mut fresh, _directory) = engine_with("");
    assert!(fresh.record_update_check("<html>", NOW, false).is_err());
    assert!(fresh.update_check_due(NOW));
}

#[test]
fn updates_can_only_be_set_by_the_user() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join(adoc_engine::WORKSPACE_SETTINGS_FILE),
        "[updates]\ncheck = false\n",
    )
    .unwrap();
    let settings = Settings::load(None, Some(directory.path())).unwrap();
    assert!(settings.updates.check);
    assert_eq!(settings.warnings.len(), 1, "{:?}", settings.warnings);
}
