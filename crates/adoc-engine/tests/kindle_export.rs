//! Kindle export through the engine, over the publication plan PDF and Word export use
//! (`docs/prd/PRD-AdocFoundry_Kindle_Export.md`).

#![cfg(feature = "kindle")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineError, ExportTarget, KindleExportArgs, KindleFormat,
    Settings,
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

/// The book fixture copied to a temporary directory and indexed.
fn book_workspace() -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

fn page(root: &Path, relative: &str) -> PathBuf {
    root.join("guide/modules").join(relative)
}

/// An output whose extension is not the format's is refused before anything is read, and
/// nothing is written in either format's place.
#[test]
fn kindle_outputs_must_carry_their_format_extension() {
    let (directory, root, mut engine) = book_workspace();
    let target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));
    for (format, name) in [
        (KindleFormat::Kfx, "book.azw3"),
        (KindleFormat::Azw3, "book.kfx"),
        (KindleFormat::Azw3, "book.epub"),
        (KindleFormat::Kfx, "no-extension"),
    ] {
        let output = directory.path().join(name);
        let result = engine.export_kindle_blocking(
            KindleExportArgs::new(target.clone(), format, output.clone()),
            WAIT,
        );
        assert!(
            matches!(result, Err(EngineError::InvalidRequest(_))),
            "{name}: {result:?}"
        );
        assert!(!output.exists(), "{name}");
    }
}

fn book() -> ExportTarget {
    ExportTarget::Component {
        name: "guide".to_owned(),
        version: Some("1.0".to_owned()),
    }
}

/// The pages a PDF and an AZW3 of `target` hold, and the AZW3 written.
#[cfg(feature = "typst")]
fn both(
    engine: &mut Engine,
    directory: &Path,
    target: ExportTarget,
) -> (Vec<PathBuf>, Vec<PathBuf>, PathBuf) {
    let pdf = engine
        .export_blocking(
            adoc_engine::ExportArgs {
                target: target.clone(),
                output: directory.join("out.pdf"),
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
                attributes: Vec::new(),
            },
            WAIT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let output = directory.join("out.azw3");
    let kindle = engine
        .export_kindle_blocking(
            KindleExportArgs::new(target, KindleFormat::Azw3, output.clone()),
            WAIT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    (pdf.sources, kindle.sources, output)
}

/// The book's text as a reader finds it.
fn book_text(path: &Path) -> String {
    adoc_kindle::testing::read_azw3(&fs::read(path).unwrap())
        .unwrap()
        .plain_text()
}

/// PRD §12 test 1: for an article, a component version as a book and navigation files given
/// in an order, Kindle takes the pages PDF takes, in the same order, unsaved buffers included.
/// The Kindle export reaches neither `adoc-pdf` nor Typst (`kindle_builds_alone`).
#[cfg(feature = "typst")]
#[test]
fn kindle_and_pdf_select_the_same_pages() {
    let (directory, root, mut engine) = book_workspace();
    let folder = directory.path();

    let use_page = page(&root, "ROOT/pages/use.adoc");
    let id = engine.open(&use_page).unwrap();
    let end = engine.document(id).unwrap().text.to_string().len();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(end, end),
                text: "\nAn unsaved sentence.\n".to_owned(),
            },
        )
        .unwrap();

    let (pdf, kindle, output) = both(&mut engine, folder, ExportTarget::Page(id));
    assert_eq!(pdf, std::slice::from_ref(&use_page));
    assert_eq!(kindle, pdf);
    assert!(book_text(&output).contains("An unsaved sentence."));
    assert!(!fs::read_to_string(&use_page).unwrap().contains("unsaved"));

    let (pdf, kindle, output) = both(&mut engine, folder, book());
    assert_eq!(pdf.len(), 8);
    assert_eq!(kindle, pdf);
    assert!(
        book_text(&output).contains("An unsaved sentence."),
        "a book reads the open buffer too"
    );

    let (pdf, kindle, _) = both(
        &mut engine,
        folder,
        ExportTarget::File(page(&root, "admin/pages/backup.adoc")),
    );
    assert_eq!(kindle, pdf);

    let (pdf, kindle, output) = both(
        &mut engine,
        folder,
        ExportTarget::Navigation(vec![
            page(&root, "admin/nav.adoc"),
            page(&root, "ROOT/nav.adoc"),
        ]),
    );
    assert_eq!(pdf.first(), Some(&page(&root, "ROOT/pages/index.adoc")));
    assert_eq!(kindle, pdf);
    // The book holds the pages in that order.
    let text = book_text(&output);
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(at("Backup") < at("Install"));
}

fn contents(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

/// PRD §12 test 15: a failed export, a link, a wrong extension, a folder, a folder that
/// refuses the write and a path outside the roots each leave the existing file as it was.
#[test]
fn kindle_failed_export_keeps_existing_output() {
    let (directory, root, mut engine) = book_workspace();
    let folder = directory.path();
    let existing = folder.join("existing.kfx");
    fs::write(&existing, b"the previous book").unwrap();
    let page_target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));
    let kfx = |target: ExportTarget, output: &Path| {
        KindleExportArgs::new(target, KindleFormat::Kfx, output.to_path_buf())
    };

    // A target that does not exist.
    let error = engine
        .export_kindle_blocking(
            kfx(
                ExportTarget::Component {
                    name: "missing".to_owned(),
                    version: None,
                },
                &existing,
            ),
            WAIT,
        )
        .unwrap_err();
    assert!(matches!(error, EngineError::NotFound(_)), "{error:?}");
    assert_eq!(contents(&existing).unwrap(), b"the previous book");

    // An extension that is not the format's: never an AZW3 written as `.kfx` or the reverse.
    let error = engine
        .export_kindle_blocking(
            KindleExportArgs::new(page_target.clone(), KindleFormat::Azw3, existing.clone()),
            WAIT,
        )
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidRequest(_)), "{error:?}");
    assert_eq!(contents(&existing).unwrap(), b"the previous book");

    // A folder at the output.
    let folder_output = folder.join("folder.kfx");
    fs::create_dir(&folder_output).unwrap();
    assert!(engine
        .export_kindle_blocking(kfx(page_target.clone(), &folder_output), WAIT)
        .is_err());
    assert!(folder_output.is_dir());

    #[cfg(unix)]
    {
        // A link at the output is never written through.
        let elsewhere = folder.join("elsewhere.kfx");
        fs::write(&elsewhere, b"another book").unwrap();
        let link = folder.join("link.kfx");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let error = engine
            .export_kindle_blocking(kfx(page_target.clone(), &link), WAIT)
            .unwrap_err();
        assert!(matches!(error, EngineError::InvalidRequest(_)), "{error:?}");
        assert_eq!(contents(&elsewhere).unwrap(), b"another book");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());

        // A folder that refuses the write: the book there is left as it was.
        use std::os::unix::fs::PermissionsExt;
        let locked = folder.join("locked");
        fs::create_dir(&locked).unwrap();
        let inside = locked.join("out.kfx");
        fs::write(&inside, b"the locked book").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = engine.export_kindle_blocking(kfx(page_target.clone(), &inside), WAIT);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "{result:?}");
        assert_eq!(contents(&inside).unwrap(), b"the locked book");
        assert_eq!(
            fs::read_dir(&locked).unwrap().count(),
            1,
            "no temporary file left"
        );
    }

    // A profile writing outside the workspace is refused.
    fs::write(
        root.join(".adocfoundry.toml"),
        "[kindle.profiles.away]\npage = \"guide/modules/ROOT/pages/install.adoc\"\noutput = \"../existing.kfx\"\n",
    )
    .unwrap();
    engine.reload_settings().unwrap();
    let error = engine.kindle_profile_export_args("away").unwrap_err();
    assert!(matches!(error, EngineError::Refused(_)), "{error:?}");
    assert_eq!(contents(&existing).unwrap(), b"the previous book");

    // A successful export replaces the book whole, leaving no temporary file.
    engine
        .export_kindle_blocking(kfx(page_target, &existing), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(&contents(&existing).unwrap()[..4], b"CONT");
    let temporary = fs::read_dir(folder)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp"))
        .count();
    assert_eq!(temporary, 0);
}

/// PRD §12 test 14: page, component and navigation profiles resolve to the right pages,
/// format and output; invalid ones are refused, and `[kindle]` gives the defaults.
#[test]
fn kindle_profiles_resolve_and_validate() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    fs::write(
        root.join(".adocfoundry.toml"),
        r#"
[kindle]
format = "kfx"
creator = "The Team"

[kindle.profiles.page]
page = "guide/modules/ROOT/pages/install.adoc"
output = "build/install.azw3"
title = "Installing"

[kindle.profiles.handbook]
component = "1.0@guide"

[kindle.profiles.admin]
nav = ["guide/modules/admin/nav.adoc"]
format = "azw3"
output = "build/admin.azw3"

[kindle.profiles.wrong]
page = "guide/modules/ROOT/pages/install.adoc"
format = "kfx"
output = "wrong.azw3"

[kindle.profiles.unknown]
component = "nowhere"
"#,
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    // The invalid profile is refused when the settings are read.
    assert!(
        settings
            .warnings
            .iter()
            .any(|warning| warning.contains("kindle.profiles.wrong")),
        "{:?}",
        settings.warnings
    );
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(
        engine.kindle_profiles(),
        ["admin", "handbook", "page", "unknown"]
    );

    // The page profile: its own format from its output, the defaults' creator.
    let args = engine.kindle_profile_export_args("page").unwrap();
    assert_eq!(args.format, KindleFormat::Azw3);
    assert_eq!(args.output, root.join("build/install.azw3"));
    let summary = engine.export_kindle_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources, [page(&root, "ROOT/pages/install.adoc")]);
    let read = adoc_kindle::testing::read_azw3(&fs::read(root.join("build/install.azw3")).unwrap())
        .unwrap();
    assert_eq!(read.title, "Installing");
    assert_eq!(read.exth_text(100), ["The Team"]);

    // The component profile: `[kindle] format`, `<name>.kfx` at the root, the whole book.
    let args = engine.kindle_profile_export_args("handbook").unwrap();
    assert_eq!(args.format, KindleFormat::Kfx);
    assert_eq!(args.output, root.join("handbook.kfx"));
    let summary = engine.export_kindle_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources.len(), 8);
    let read =
        adoc_kindle::testing::read_kfx(&fs::read(root.join("handbook.kfx")).unwrap()).unwrap();
    assert_eq!(read.reading_order().len(), read.chapters().unwrap().len());

    // The navigation profile: the start page as a preface, as the shared plan has it, then
    // the administration guide alone.
    let args = engine.kindle_profile_export_args("admin").unwrap();
    let summary = engine.export_kindle_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources[0], page(&root, "ROOT/pages/index.adoc"));
    let admin = root.join("guide/modules/admin");
    assert!(summary.sources[1..]
        .iter()
        .all(|source| source.starts_with(&admin)));

    // Through the command, as the app and MCP server export a profile.
    fs::remove_file(root.join("build/install.azw3")).unwrap();
    engine
        .invoke(
            ids::EXPORT_KINDLE_PROFILE,
            CommandArgs::Profile("page".to_owned()),
        )
        .unwrap();
    assert!(engine.wait_for_workers(WAIT));
    for _ in 0..200 {
        engine.pump();
        if root.join("build/install.azw3").is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(root.join("build/install.azw3").is_file());

    // Problems found without exporting.
    assert!(engine.kindle_profile_problems("page").is_empty());
    let problems = engine.kindle_profile_problems("unknown");
    assert!(
        problems.iter().any(|problem| problem.contains("nowhere")),
        "{problems:?}"
    );
    assert!(engine.kindle_profile_export_args("missing").is_err());
    assert!(engine.kindle_profile_export_args("wrong").is_err());
}
