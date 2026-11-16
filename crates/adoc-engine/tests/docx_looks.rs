//! A Word export's look: the PDF theme, a Word template or none, chosen by the export, a
//! profile or the settings. The Word looks PRD (`docs/prd/PRD-AdocFoundry_DOCX_Themes.md`),
//! §9 tests 2 (`docx_look_follows_precedence`) and 3 (`docx_settings_apply_over_the_look`).

#![cfg(feature = "docx")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    DocxExportArgs, Engine, ExportSummary, ExportTarget, LookChoice, LookKind, Settings,
};

const WAIT: Duration = Duration::from_secs(120);

/// A theme with a page of its own: US Letter, an inch at the top and bottom, half an inch at
/// the sides.
const NARROW: &str = "extends: default\npage:\n  size: Letter\n  margin: [1in, 0.5in]\n";

/// A workspace of one page, `page.adoc` holding `header` after its title, with `settings` as
/// its `.adocfoundry.toml`, a theme `narrow` and a template `corporate.dotx` in `themes/`.
fn workspace(settings: &str, header: &str) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    fs::create_dir_all(root.join("themes")).unwrap();
    fs::write(root.join("themes/narrow-theme.yml"), NARROW).unwrap();
    fs::write(
        root.join("themes/wide-theme.yml"),
        "extends: default\npage:\n  layout: landscape\n",
    )
    .unwrap();
    fs::write(root.join("themes/corporate.dotx"), b"PK\x05\x06").unwrap();
    fs::write(root.join("page.adoc"), format!("= Page\n{header}\nText.\n")).unwrap();
    fs::write(root.join(".adocfoundry.toml"), settings).unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

/// Exports the page with `look` as its own choice.
fn export(
    engine: &mut Engine,
    root: &Path,
    look: Option<LookChoice>,
) -> Result<(ExportSummary, String), String> {
    let id = engine.open(&root.join("page.adoc")).unwrap();
    let output = root.join("page.docx");
    let summary = engine
        .export_docx_blocking(
            DocxExportArgs {
                target: ExportTarget::Page(id),
                output: output.clone(),
                attributes: Vec::new(),
                paper: None,
                landscape: None,
                look,
            },
            WAIT,
        )
        .map_err(|error| error.to_string())?;
    let entries = adoc_docx::testing::assert_valid_package(&fs::read(&output).unwrap());
    let document = adoc_docx::testing::text(&entries, "word/document.xml").to_owned();
    Ok((summary, document))
}

/// The look an export used, as (kind, name).
fn look(summary: &ExportSummary) -> (LookKind, String) {
    let look = summary
        .look
        .as_ref()
        .expect("a Word export reports its look");
    (look.kind, look.name.clone())
}

const A4: &str = r#"<w:pgSz w:w="11906" w:h="16838"/>"#;
const LETTER: &str = r#"<w:pgSz w:w="12240" w:h="15840"/>"#;

#[test]
fn docx_look_follows_precedence() {
    // No theme named anywhere: no theme, today's page, whatever `[docx] look` defaults to.
    let (_directory, root, mut engine) = workspace("", "");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::None, "none".to_owned()));
    assert!(document.contains(A4), "{document}");
    assert!(document.contains(r#"w:top="1440""#));

    // `[pdf] theme` written: Word follows it, its page included.
    let settings = "[pdf]\ntheme = \"narrow\"\nthemes_dir = \"themes\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "narrow".to_owned()));
    assert_eq!(
        summary.look.as_ref().unwrap().path.as_deref(),
        Some(root.join("themes/narrow-theme.yml").as_path())
    );
    assert!(document.contains(LETTER), "{document}");
    assert!(document.contains(r#"w:top="1440" w:right="720" w:bottom="1440" w:left="720""#));

    // The page's `pdf-theme` wins over `[pdf] theme`, and its `pdf-page-size` over the theme's.
    let (_directory, root, mut engine) =
        workspace(settings, ":pdf-theme: wide\n:pdf-page-size: A4\n");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "wide".to_owned()));
    assert!(document.contains(r#"<w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>"#));

    // The export's own choice wins over the settings and the page.
    let (summary, document) = export(&mut engine, &root, Some(LookChoice::None)).unwrap();
    assert_eq!(look(&summary).0, LookKind::None);
    assert!(document.contains(A4));
    let (summary, document) = export(
        &mut engine,
        &root,
        Some(LookChoice::PdfTheme("narrow".to_owned())),
    )
    .unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "narrow".to_owned()));
    // The page's own size still wins over the theme's.
    assert!(document.contains(A4), "{document}");
    let (summary, _) = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("corporate".to_owned())),
    )
    .unwrap();
    assert_eq!(look(&summary), (LookKind::Template, "corporate".to_owned()));
    assert_eq!(
        summary.look.as_ref().unwrap().path.as_deref(),
        Some(root.join("themes/corporate.dotx").as_path())
    );
    // `--theme default` names the bundled default theme.
    let (summary, _) = export(
        &mut engine,
        &root,
        Some(LookChoice::PdfTheme("default".to_owned())),
    )
    .unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "default".to_owned()));

    // `[docx] look` sets the default: none, or a template it names.
    let (_directory, root, mut engine) =
        workspace(&format!("{settings}\n[docx]\nlook = \"none\"\n"), "");
    assert_eq!(
        look(&export(&mut engine, &root, None).unwrap().0).0,
        LookKind::None
    );
    let (_directory, root, mut engine) = workspace(
        &format!("{settings}\n[docx]\nlook = \"template\"\ntemplate = \"corporate\"\n"),
        "",
    );
    let (summary, _) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::Template, "corporate".to_owned()));

    // A profile's choice is its own, and an export's wins over it.
    let profiles = format!(
        "{settings}\n[docx.profiles.plain]\npage = \"page.adoc\"\nlook = \"none\"\n\n\
         [docx.profiles.themed]\npage = \"page.adoc\"\ntheme = \"wide\"\n\n\
         [docx.profiles.both]\npage = \"page.adoc\"\ntheme = \"wide\"\ntemplate = \"corporate\"\n"
    );
    let (_directory, _root, mut engine) = workspace(&profiles, "");
    let mut args = engine.docx_profile_export_args("plain").unwrap();
    assert_eq!(args.look, Some(LookChoice::None));
    assert_eq!(
        look(&engine.export_docx_blocking(args.clone(), WAIT).unwrap()).0,
        LookKind::None
    );
    args.look = Some(LookChoice::PdfTheme("narrow".to_owned()));
    assert_eq!(
        look(&engine.export_docx_blocking(args, WAIT).unwrap()).1,
        "narrow"
    );
    assert_eq!(
        engine.docx_profile_export_args("themed").unwrap().look,
        Some(LookChoice::PdfTheme("wide".to_owned()))
    );
    // A profile names at most one look.
    assert!(engine.docx_profile_export_args("both").is_err());
    assert!(
        engine
            .settings()
            .warnings
            .iter()
            .any(|warning| warning.contains("both") && warning.contains("at most one")),
        "{:?}",
        engine.settings().warnings
    );

    // A theme or template that is not there fails the export, naming where it was looked for.
    let (_directory, root, mut engine) = workspace(settings, "");
    let error = export(
        &mut engine,
        &root,
        Some(LookChoice::PdfTheme("missing".to_owned())),
    )
    .unwrap_err();
    assert!(error.contains("missing-theme.yml"), "{error}");
    let error = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("missing".to_owned())),
    )
    .unwrap_err();
    assert!(error.contains("missing.dotx"), "{error}");
    let (_directory, root, mut engine) = workspace("[docx]\nlook = \"template\"\n", "");
    let error = export(&mut engine, &root, None).unwrap_err();
    assert!(error.contains("no template"), "{error}");
}

#[test]
fn docx_settings_apply_over_the_look() {
    // The theme gives Letter and its margins; the settings write the paper and the top margin.
    let settings = "[pdf]\ntheme = \"narrow\"\nthemes_dir = \"themes\"\n\n\
                    [docx]\npaper = \"a4\"\n\n[docx.margins]\ntop = \"30mm\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary).0, LookKind::PdfTheme);
    assert!(document.contains(A4), "the written paper wins: {document}");
    // The written top margin wins; the sides and bottom are the theme's.
    assert!(
        document.contains(r#"w:top="1701" w:right="720" w:bottom="1440" w:left="720""#),
        "{document}"
    );

    // Orientation written in the settings wins over a landscape theme.
    let settings =
        "[pdf]\ntheme = \"wide\"\nthemes_dir = \"themes\"\n\n[docx]\norientation = \"portrait\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let (_, document) = export(&mut engine, &root, None).unwrap();
    assert!(document.contains(A4), "{document}");

    // Nothing written: the theme's page, every side of it.
    let (_directory, root, mut engine) =
        workspace("[pdf]\ntheme = \"narrow\"\nthemes_dir = \"themes\"\n", "");
    let (_, document) = export(&mut engine, &root, None).unwrap();
    assert!(document.contains(LETTER));
}
