//! Authoring profiles (#102): named sets of attributes in `[authoring.profiles.<name>]`, one of
//! which the preview, the visual editor and exports read pages with.

use std::{fs, path::Path, sync::Arc, time::Duration};

use adoc_engine::{
    commands::ids, CommandArgs, DocBookExportArgs, Engine, EngineError, EngineEvent, ExportTarget,
    ManualClock, Settings,
};

const PAGE: &str = "= Install\n\nifdef::cloud[]\nSign in to the cloud console.\nendif::[]\n\
                    ifndef::cloud[]\nRun the installer.\nendif::[]\n\nThe {edition} edition.\n";

const SETTINGS: &str = "[authoring.profiles.cloud]\nattributes = { cloud = true, edition = \"Cloud\" }\n\n\
                        [authoring.profiles.on-premises]\nattributes = { edition = \"On-premises\" }\n\n\
                        [authoring.profiles.broken]\nattributes = { edition = [1] }\n";

const WAIT: Duration = Duration::from_secs(20);

/// A workspace whose settings file names the profiles, its page open.
fn workspace() -> (tempfile::TempDir, Engine, adoc_engine::DocumentId) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    fs::write(root.join(".adocfoundry.toml"), SETTINGS).unwrap();
    fs::write(root.join("install.adoc"), PAGE).unwrap();
    let settings = Settings::load(None, Some(&root)).unwrap().with_git_opt_in();
    let mut engine = Engine::with_clock(settings, Arc::new(ManualClock::new()));
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&root.join("install.adoc")).unwrap();
    (directory, engine, id)
}

fn preview_html(engine: &mut Engine, id: adoc_engine::DocumentId) -> String {
    let rendered = engine.render_blocking(id, WAIT).expect("a preview");
    adoc_render::to_html(&rendered.view, &adoc_render::HtmlOptions::default())
}

fn choose(engine: &mut Engine, profile: Option<&str>) -> Result<(), EngineError> {
    engine
        .invoke(
            ids::PREVIEW_AUTHORING_PROFILE,
            CommandArgs::AuthoringProfile(profile.map(str::to_owned)),
        )
        .map(|_| ())
}

#[test]
fn authoring_profiles_are_read_from_the_settings() {
    let (_directory, engine, _) = workspace();

    let names: Vec<&str> = engine
        .authoring_profiles()
        .iter()
        .map(|profile| profile.name.as_str())
        .collect();
    assert_eq!(names, ["cloud", "on-premises"]);
    assert_eq!(
        engine.authoring_profiles()[0].attributes,
        [
            ("cloud".to_owned(), String::new()),
            ("edition".to_owned(), "Cloud".to_owned())
        ]
    );
    assert!(
        engine
            .settings()
            .warnings
            .iter()
            .any(|warning| warning.contains("authoring.profiles.broken")),
        "{:?}",
        engine.settings().warnings
    );
    assert_eq!(engine.authoring_profile(), None);
}

/// The preview shows the conditional content and values the chosen profile gives, and goes
/// back to the page's own once none is chosen.
#[test]
fn the_preview_follows_the_authoring_profile() {
    let (_directory, mut engine, id) = workspace();

    let plain = preview_html(&mut engine, id);
    assert!(plain.contains("Run the installer."), "{plain}");
    assert!(!plain.contains("cloud console"), "{plain}");

    let events = engine.subscribe();
    choose(&mut engine, Some("cloud")).unwrap();
    assert_eq!(
        engine
            .authoring_profile()
            .map(|profile| profile.name.as_str()),
        Some("cloud")
    );
    assert!(events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::AuthoringProfileChanged)));
    let cloud = preview_html(&mut engine, id);
    assert!(cloud.contains("Sign in to the cloud console."), "{cloud}");
    assert!(!cloud.contains("Run the installer."), "{cloud}");
    assert!(cloud.contains("The Cloud edition."), "{cloud}");

    choose(&mut engine, Some("on-premises")).unwrap();
    let on_premises = preview_html(&mut engine, id);
    assert!(on_premises.contains("Run the installer."), "{on_premises}");
    assert!(
        on_premises.contains("The On-premises edition."),
        "{on_premises}"
    );

    choose(&mut engine, None).unwrap();
    assert_eq!(engine.authoring_profile(), None);
    assert_eq!(preview_html(&mut engine, id), plain);
}

#[test]
fn an_unknown_authoring_profile_is_refused() {
    let (_directory, mut engine, _) = workspace();
    choose(&mut engine, Some("cloud")).unwrap();

    assert!(matches!(
        choose(&mut engine, Some("broken")),
        Err(EngineError::Refused(_))
    ));
    assert_eq!(
        engine
            .authoring_profile()
            .map(|profile| profile.name.as_str()),
        Some("cloud"),
        "a refusal changes nothing"
    );
}

/// An export reads the pages with the chosen profile, as the preview does.
#[test]
fn exports_follow_the_authoring_profile() {
    let (directory, mut engine, _) = workspace();
    let page = adoc_index::normalize_path(directory.path()).join("install.adoc");
    let output = directory.path().join("out.xml");
    let export = |engine: &mut Engine| {
        engine
            .export_docbook_blocking(
                DocBookExportArgs::new(ExportTarget::File(page.clone()), output.clone()),
                WAIT,
            )
            .unwrap_or_else(|error| panic!("{error}"));
        fs::read_to_string(&output).unwrap()
    };

    assert!(export(&mut engine).contains("Run the installer."));
    choose(&mut engine, Some("cloud")).unwrap();
    let cloud = export(&mut engine);
    assert!(cloud.contains("Sign in to the cloud console."), "{cloud}");
    assert!(cloud.contains("The Cloud edition."), "{cloud}");
}

/// A profile taken out of the settings is no longer chosen.
#[test]
fn a_profile_taken_out_of_the_settings_is_no_longer_chosen() {
    let (directory, mut engine, _) = workspace();
    choose(&mut engine, Some("cloud")).unwrap();

    fs::write(
        Path::new(directory.path()).join(".adocfoundry.toml"),
        "[authoring.profiles.on-premises]\nattributes = { edition = \"On-premises\" }\n",
    )
    .unwrap();
    engine.reload_settings().unwrap();

    assert_eq!(engine.authoring_profile(), None);
}

/// A profile's attributes are hard set, as `antora.yml`'s are: a page's own header does not
/// change them.
#[test]
fn a_profiles_attributes_win_over_the_page_header() {
    let (directory, mut engine, _) = workspace();
    let page = adoc_index::normalize_path(directory.path()).join("header.adoc");
    fs::write(
        &page,
        "= Header\n:edition: Standard\n\nThe {edition} edition.\n",
    )
    .unwrap();
    let id = engine.open(&page).unwrap();
    assert!(preview_html(&mut engine, id).contains("The Standard edition."));

    choose(&mut engine, Some("cloud")).unwrap();

    let html = preview_html(&mut engine, id);
    assert!(html.contains("The Cloud edition."), "{html}");
}
