//! #105: the doctor report, from the settings and tools in temporary directories.

use std::{fs, path::Path, sync::Mutex};

use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DoctorReport, DoctorSection, Engine, Settings,
};

/// Held while a test writes a stand-in tool and runs it, so no other test forks while the
/// script is open for writing (`ETXTBSY`).
static SPAWN: Mutex<()> = Mutex::new(());

fn spawn_guard() -> std::sync::MutexGuard<'static, ()> {
    SPAWN.lock().unwrap_or_else(|error| error.into_inner())
}

/// An engine with `settings` as the user file and `workspace` as `.adocfoundry.toml` in its one
/// root, and the directory both are in.
fn engine_with(settings: &str, workspace: &str) -> (Engine, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("settings.toml");
    fs::write(&file, settings).unwrap();
    let root = directory.path().join("docs");
    fs::create_dir(&root).unwrap();
    fs::write(root.join(".adocfoundry.toml"), workspace).unwrap();
    let mut engine = Engine::new(Settings::load(Some(&file), None).unwrap());
    engine.open_workspace(vec![root]).unwrap();
    (engine, directory)
}

/// The report, with the tools looked for only in `tools`.
fn report_for(engine: &Engine, tools: &Path) -> DoctorReport {
    engine.doctor().search_path(tools.as_os_str()).run()
}

fn section<'a>(report: &'a DoctorReport, title: &str) -> &'a DoctorSection {
    report
        .section(title)
        .unwrap_or_else(|| panic!("no {title} section in\n{}", report.to_text()))
}

#[test]
fn the_report_names_the_build_the_system_and_the_settings() {
    let (engine, directory) = engine_with("[editor]\nmystery = 1\n", "");
    let empty = tempfile::tempdir().unwrap();
    let report = engine
        .doctor()
        .program("adocfoundry-cli", "9.9.9")
        .search_path(empty.path().as_os_str())
        .run();

    let titles: Vec<&str> = report
        .sections
        .iter()
        .map(|section| section.title.as_str())
        .collect();
    assert_eq!(titles, ["Program", "System", "PDF", "Tools", "Settings"]);

    let program = section(&report, "Program");
    assert_eq!(program.value("Version"), Some("adocfoundry-cli 9.9.9"));
    assert_eq!(program.value("Features"), Some("typst, git"));
    let system = section(&report, "System");
    assert_eq!(system.value("Architecture"), Some(std::env::consts::ARCH));
    assert!(!system.value("OS").unwrap().is_empty());

    let settings = section(&report, "Settings");
    let user = directory.path().join("settings.toml");
    assert_eq!(
        settings.value("User settings"),
        Some(user.display().to_string().as_str())
    );
    let workspace = directory.path().join("docs").join(".adocfoundry.toml");
    assert_eq!(
        settings.value("Workspace settings"),
        Some(workspace.display().to_string().as_str())
    );
    let warnings: Vec<&str> = settings.values("Warning").collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("`editor.mystery`"), "{warnings:?}");

    let text = report.to_text();
    assert!(
        text.starts_with("AdocFoundry diagnostics\n\nProgram\n"),
        "{text}"
    );
    assert!(
        text.contains("  Version:  adocfoundry-cli 9.9.9\n"),
        "{text}"
    );
}

#[test]
fn a_front_ends_own_section_follows_the_systems() {
    let (engine, _directory) = engine_with("", "");
    let empty = tempfile::tempdir().unwrap();
    let mut doctor = engine.doctor().search_path(empty.path().as_os_str());
    doctor.add_section(DoctorSection::new("Display").line("Graphics", "Metal"));
    let report = doctor.run();
    assert_eq!(report.sections[2].title, "Display");
    assert_eq!(report.sections[2].value("Graphics"), Some("Metal"));
}

#[test]
fn missing_tools_say_why() {
    let (engine, directory) = engine_with("[tools]\nasciidoctor = \"/nowhere/asciidoctor\"\n", "");
    let report = report_for(&engine, directory.path());
    let tools = section(&report, "Tools");
    assert_eq!(
        tools.value("asciidoctor"),
        Some("not found: [tools] asciidoctor is /nowhere/asciidoctor, which is not a file")
    );
    for tool in ["dot", "plantuml", "d2", "git"] {
        assert_eq!(tools.value(tool), Some("not found on PATH"), "{tool}");
    }
}

#[cfg(unix)]
#[test]
fn tools_found_report_their_path_and_version() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = spawn_guard();
    let (engine, _directory) = engine_with("", "");
    let tools = tempfile::tempdir().unwrap();
    let stand_in = |name: &str, script: &str| {
        let path = tools.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    // Graphviz prints its version on standard error.
    let dot = stand_in("dot", "echo 'dot - graphviz version 99.0.0' >&2");
    let d2 = stand_in("d2", "echo\necho v9.9.9");
    let git = stand_in("git", "exit 3");

    let report = report_for(&engine, tools.path());
    let section = section(&report, "Tools");
    assert_eq!(
        section.value("dot"),
        Some(format!("{} (dot - graphviz version 99.0.0)", dot.display()).as_str())
    );
    assert_eq!(
        section.value("d2"),
        Some(format!("{} (v9.9.9)", d2.display()).as_str())
    );
    assert_eq!(
        section.value("git"),
        Some(format!("{} (exited with exit status: 3)", git.display()).as_str())
    );
}

#[test]
fn the_pdf_section_names_the_theme_and_its_missing_fonts() {
    let (engine, _directory) = engine_with("", "");
    let empty = tempfile::tempdir().unwrap();
    let report = report_for(&engine, empty.path());
    let pdf = section(&report, "PDF");
    assert_eq!(pdf.value("Theme"), Some("default (bundled)"));
    assert_eq!(
        pdf.value("Fonts directory"),
        Some("none (the bundled fonts only)")
    );
    assert_eq!(pdf.value("Fonts"), Some("all found"));

    let (engine, directory) = engine_with(
        "",
        "[pdf]\ntheme = \"lost\"\nthemes_dir = \"themes\"\nfonts_dir = \"fonts\"\n",
    );
    let themes = directory.path().join("docs").join("themes");
    fs::create_dir(&themes).unwrap();
    fs::write(
        themes.join("lost-theme.yml"),
        "extends: default\nfont:\n  catalog:\n    merge: true\n    Lost:\n      normal: lost.ttf\n",
    )
    .unwrap();
    let report = report_for(&engine, empty.path());
    let pdf = section(&report, "PDF");
    assert_eq!(
        pdf.value("Theme"),
        Some(themes.join("lost-theme.yml").display().to_string().as_str())
    );
    assert_eq!(
        pdf.value("Fonts directory"),
        Some(
            directory
                .path()
                .join("docs")
                .join("fonts")
                .display()
                .to_string()
                .as_str()
        )
    );
    assert!(pdf.value("Font families").unwrap().contains("Lost"));
    assert_eq!(
        pdf.values("Missing").collect::<Vec<_>>(),
        ["font lost.ttf for Lost not found"]
    );
}

#[test]
fn the_diagnostics_command_hands_the_front_end_the_doctor() {
    let (mut engine, _directory) = engine_with("", "");
    let doctor = engine.doctor();
    assert_eq!(
        engine
            .invoke(ids::APP_DIAGNOSTICS, CommandArgs::None)
            .unwrap(),
        CommandOutcome::Diagnostics(Box::new(doctor))
    );
}
