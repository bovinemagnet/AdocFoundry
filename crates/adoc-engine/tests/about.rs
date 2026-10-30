//! #161: Help › About, its licence and links, and the third-party notices it reads.

use adoc_engine::{commands::ids, About, CommandArgs, CommandOutcome, Engine, Notices, Settings};

fn about() -> About {
    let mut engine = Engine::new(Settings::default());
    match engine.invoke(ids::APP_ABOUT, CommandArgs::None) {
        Ok(CommandOutcome::About(about)) => *about,
        other => panic!("app.about gave {other:?}"),
    }
}

const NOTICES: &str = r#"{
    "components": [
        {"name": "alpha", "version": "1.0.0", "licence": "MIT OR Apache-2.0", "note": null,
         "texts": [["LICENSE-MIT", 0], ["LICENSE-APACHE", 1]]},
        {"name": "beta", "version": "2.1.0", "licence": "MIT",
         "note": "beta ships no licence file.", "texts": [["MIT", 0]]}
    ],
    "texts": ["MIT text", "Apache text"]
}"#;

#[test]
fn about_gives_the_licence_and_the_links() {
    let about = about();
    assert_eq!(about.licence, "Apache-2.0");
    assert!(about
        .licence_text
        .trim_start()
        .starts_with("Apache License"));
    assert_eq!(
        about.repository,
        "https://github.com/bovinemagnet/AdocFoundry"
    );
    assert_eq!(
        about.documentation,
        "https://bovinemagnet.github.io/AdocFoundry"
    );
    assert_eq!(about.version, env!("CARGO_PKG_VERSION"));
    assert!(about.notices.components.is_empty());
}

#[test]
fn the_front_end_names_the_program_and_gives_the_notices() {
    let about = about()
        .program(
            "AdocFoundry",
            "9.9.9",
            Some("x86_64-unknown-linux-gnu, release"),
        )
        .notices(Notices::from_json(NOTICES).unwrap());
    assert_eq!(about.name, "AdocFoundry");
    assert_eq!(about.version, "9.9.9");
    assert_eq!(
        about.build.as_deref(),
        Some("x86_64-unknown-linux-gnu, release")
    );
    let names: Vec<&str> = about
        .notices
        .components
        .iter()
        .map(|notice| notice.name.as_str())
        .collect();
    assert_eq!(names, ["alpha", "beta"]);
}

#[test]
fn each_notice_reads_as_its_licence_texts() {
    let notices = Notices::from_json(NOTICES).unwrap();
    let alpha = &notices.components[0];
    assert_eq!(
        notices.texts(alpha).collect::<Vec<_>>(),
        [
            ("LICENSE-MIT", "MIT text"),
            ("LICENSE-APACHE", "Apache text")
        ]
    );
    assert_eq!(
        notices.text(alpha),
        "alpha 1.0.0\nLicence: MIT OR Apache-2.0\n\n\
         LICENSE-MIT\n\nMIT text\n\n\
         LICENSE-APACHE\n\nApache text\n"
    );
    assert_eq!(
        notices.text(&notices.components[1]),
        "beta 2.1.0\nLicence: MIT\n\nbeta ships no licence file.\n\nMIT\n\nMIT text\n"
    );
}

#[test]
fn notices_naming_a_text_they_lack_are_refused() {
    let broken = NOTICES.replace(r#"["MIT", 0]"#, r#"["MIT", 7]"#);
    assert!(Notices::from_json(&broken).is_err());
    assert!(Notices::from_json("not json").is_err());
}
