//! Issue #103: publication profiles, saved in the workspace settings and exported the same way
//! whichever front end runs them.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_diagrams::{DiagramCache, Diagrams, MermaidProvider};
use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineEvent, ExportTarget, PdfStandard, Settings,
};

const WAIT: Duration = Duration::from_secs(120);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The book fixture copied to a temporary directory and indexed, with `workspace_settings` as
/// its `.adocfoundry.toml`.
fn book_workspace(workspace_settings: &str) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    fs::write(root.join(".adocfoundry.toml"), workspace_settings).unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.set_diagrams(Diagrams::with_providers(
        DiagramCache::new(directory.path().join("cache")),
        vec![Box::new(MermaidProvider)],
    ));
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

/// The positions of `needles` in `haystack`, which must all be present.
fn positions(haystack: &str, needles: &[&str]) -> Vec<usize> {
    needles
        .iter()
        .map(|needle| {
            haystack
                .find(needle)
                .unwrap_or_else(|| panic!("{needle:?} is not in the Typst"))
        })
        .collect()
}

const PROFILES: &str = r#"
[pdf.profiles.admin-guide]
nav = ["guide/modules/admin/nav.adoc", "guide/modules/ROOT/nav.adoc"]
output = "out/admin-guide.pdf"
theme = "default-sans"
standards = ["pdf-ua-1"]
attributes = { product = "Acme Server", pdf-title = "Acme Administration", author = "Acme Docs", description = "How to run Acme." }

[pdf.profiles.install]
page = "guide/modules/ROOT/pages/install.adoc"

[pdf.profiles.guide]
component = "1.0@guide"
"#;

#[test]
fn profiles_are_read_from_the_workspace_settings() {
    let (_directory, root, engine) = book_workspace(PROFILES);
    assert!(
        engine.settings().warnings.is_empty(),
        "{:?}",
        engine.settings().warnings
    );
    let names: Vec<&str> = engine
        .settings()
        .pdf
        .profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect();
    assert_eq!(names, ["admin-guide", "guide", "install"]);

    let args = engine.profile_export_args("admin-guide").unwrap();
    assert_eq!(
        args.target,
        ExportTarget::Navigation(vec![
            root.join("guide/modules/admin/nav.adoc"),
            root.join("guide/modules/ROOT/nav.adoc"),
        ])
    );
    assert_eq!(args.output, root.join("out/admin-guide.pdf"));
    assert_eq!(args.theme.as_deref(), Some("default-sans"));
    assert_eq!(args.standards, vec![PdfStandard::Ua1]);
    assert!(args
        .attributes
        .contains(&("product".to_owned(), "Acme Server".to_owned())));

    let args = engine.profile_export_args("install").unwrap();
    assert_eq!(
        args.target,
        ExportTarget::File(root.join("guide/modules/ROOT/pages/install.adoc"))
    );
    // Without an output, the profile's name in the first root.
    assert_eq!(args.output, root.join("install.pdf"));

    let args = engine.profile_export_args("guide").unwrap();
    assert_eq!(
        args.target,
        ExportTarget::Component {
            name: "guide".to_owned(),
            version: Some("1.0".to_owned())
        }
    );
    assert!(engine.profile_export_args("missing").is_err());
}

#[test]
fn a_bad_profile_is_a_warning() {
    for (settings, problem) in [
        (
            "[pdf.profiles.x]\noutput = \"x.pdf\"\n",
            "expected one of `page`, `component` or `nav`",
        ),
        (
            "[pdf.profiles.x]\npage = \"a.adoc\"\ncomponent = \"guide\"\n",
            "expected one of `page`, `component` or `nav`",
        ),
        (
            "[pdf.profiles.x]\npage = \"a.adoc\"\ncolour = \"red\"\n",
            "unknown key `colour`",
        ),
        (
            "[pdf.profiles.x]\nnav = []\n",
            "expected one of `page`, `component` or `nav`",
        ),
    ] {
        let (_directory, _root, engine) = book_workspace(settings);
        let warnings = &engine.settings().warnings;
        assert!(
            warnings.iter().any(|warning| warning.contains(problem)),
            "{settings:?}: {warnings:?}"
        );
        assert!(engine.settings().pdf.profiles.is_empty());
    }
}

#[test]
fn a_profile_exports_its_navigation_files_in_its_order_with_its_attributes() {
    let (_directory, root, mut engine) = book_workspace(PROFILES);
    // A page that names the profile's attribute.
    let install = root.join("guide/modules/ROOT/pages/install.adoc");
    let text = fs::read_to_string(&install).unwrap();
    fs::write(
        &install,
        text.replace("takes a few minutes", "of {product} takes a few minutes"),
    )
    .unwrap();
    let mut args = engine.profile_export_args("admin-guide").unwrap();
    let typst = root.join("out/admin-guide.typ");
    args.typst = Some(typst.clone());
    let summary = engine
        .export_blocking(args, WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(summary.output, root.join("out/admin-guide.pdf"));
    let pdf = fs::read(&summary.output).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    // The metadata comes from the profile's attributes.
    let pdf = String::from_utf8_lossy(&pdf);
    for expected in ["Acme Administration", "Acme Docs", "How to run Acme."] {
        assert!(pdf.contains(expected), "{expected:?} is not in the metadata");
    }
    let typst = fs::read_to_string(typst).unwrap();
    let order = positions(
        &typst,
        &[
            "#adoc-title-page(title: [#\"Acme Administration\"]",
            "#adoc-part[#\"Administration\"]",
            "[#\"Backup\"]",
            "[#\"Restore\"]",
            "#adoc-part[#\"Getting Started\"]",
            "[#\"Install\"]",
            "Acme Server",
            "[#\"Use\"]",
        ],
    );
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
}

#[test]
fn the_profile_command_writes_what_export_blocking_writes() {
    // The app runs the command; the command line exports the same arguments and waits.
    let (_directory, root, mut engine) = book_workspace(PROFILES);
    let args = engine.profile_export_args("install").unwrap();
    let blocking = engine
        .export_blocking(args.clone(), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let expected = fs::read(&blocking.output).unwrap();
    fs::remove_file(&blocking.output).unwrap();

    let events = engine.subscribe();
    engine
        .invoke(
            ids::EXPORT_PDF_PROFILE,
            CommandArgs::Profile("install".to_owned()),
        )
        .unwrap();
    while engine.is_exporting() {
        assert!(engine.wait_for_workers(WAIT));
    }
    let (output, result) = events
        .try_iter()
        .find_map(|event| match event {
            EngineEvent::ExportFinished { output, result } => Some((output, result)),
            _ => None,
        })
        .expect("an ExportFinished event");
    assert_eq!(output, root.join("install.pdf"));
    result.unwrap();
    assert_eq!(
        fs::read(output).unwrap(),
        expected,
        "the same PDF either way"
    );
}

#[test]
fn themes_lists_the_bundled_and_the_workspace_themes() {
    let (_directory, root, engine) = book_workspace("[pdf]\nthemes_dir = \"themes\"\n");
    fs::create_dir_all(root.join("themes")).unwrap();
    fs::write(root.join("themes/acme-theme.yml"), "extends: default\n").unwrap();
    fs::write(root.join("themes/notes.yml"), "not a theme name\n").unwrap();
    let themes = engine.pdf_themes();
    let names: Vec<&str> = themes.iter().map(|theme| theme.name.as_str()).collect();
    for bundled in ["default", "default-sans", "default-for-print", "base"] {
        assert!(names.contains(&bundled), "{names:?}");
    }
    let acme = themes.iter().find(|theme| theme.name == "acme").unwrap();
    assert_eq!(
        acme.file.as_deref(),
        Some(root.join("themes/acme-theme.yml").as_path())
    );
    assert!(!names.contains(&"notes"));
}
