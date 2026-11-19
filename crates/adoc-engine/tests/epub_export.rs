//! EPUB export through the engine, over the publication plan PDF, Word and Kindle export use
//! (`docs/prd/PRD-AdocFoundry_EPUB_Export.md`, EPUB 2/9).

#![cfg(feature = "epub")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineError, EpubExportArgs, ExportFormat, ExportTarget,
    ProtectionRefusal, Settings,
};

const WAIT: Duration = Duration::from_secs(120);
/// 2026-11-19T10:30:00Z.
const MODIFIED: i64 = 1_795_084_200;

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

fn book() -> ExportTarget {
    ExportTarget::Component {
        name: "guide".to_owned(),
        version: Some("1.0".to_owned()),
    }
}

/// An export of `target` to `output`, dated.
fn epub(target: ExportTarget, output: &Path) -> EpubExportArgs {
    EpubExportArgs {
        modified: Some(MODIFIED),
        ..EpubExportArgs::new(target, output.to_path_buf())
    }
}

/// The book's chapters' text, in spine order, as a reader reads it.
fn book_text(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    let (files, package) = adoc_epub::testing::assert_valid_ocf(&bytes);
    package
        .spine
        .iter()
        .map(|idref| {
            let (path, _, _) = &package.manifest[idref];
            let text = adoc_epub::testing::text(&files, path);
            adoc_epub::testing::parse(path, text)
                .descendants()
                .filter_map(|node| node.text())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An output that is not an `.epub` file is refused before anything is read, and nothing is
/// written.
#[test]
fn epub_outputs_must_be_epub_files() {
    let (directory, root, mut engine) = book_workspace();
    let target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));
    for name in ["book.azw3", "book.pdf", "no-extension", "book.epub.zip"] {
        let output = directory.path().join(name);
        let result = engine.export_epub_blocking(epub(target.clone(), &output), WAIT);
        assert!(
            matches!(result, Err(EngineError::InvalidRequest(_))),
            "{name}: {result:?}"
        );
        assert!(!output.exists(), "{name}");
    }
    // Any letter case of the extension is an EPUB.
    let output = directory.path().join("book.EPUB");
    engine
        .export_epub_blocking(epub(target, &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    adoc_epub::testing::assert_valid_ocf(&fs::read(&output).unwrap());
}

/// The pages a PDF and an EPUB of `target` hold, and the EPUB written.
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
    let output = directory.join("out.epub");
    let summary = engine
        .export_epub_blocking(epub(target, &output), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(summary.output, output);
    (pdf.sources, summary.sources, output)
}

/// PRD §11 test 4: for an article, a component version as a book and navigation files given
/// in an order, EPUB takes the pages PDF takes, in the same order, unsaved buffers included.
/// The EPUB export reaches neither `adoc-pdf` nor Typst (`epub_builds_alone`).
#[cfg(feature = "typst")]
#[test]
fn epub_and_pdf_select_the_same_pages() {
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

    let (pdf, epub, output) = both(&mut engine, folder, ExportTarget::Page(id));
    assert_eq!(pdf, std::slice::from_ref(&use_page));
    assert_eq!(epub, pdf);
    assert!(book_text(&output).contains("An unsaved sentence."));
    assert!(!fs::read_to_string(&use_page).unwrap().contains("unsaved"));

    let (pdf, epub, output) = both(&mut engine, folder, book());
    assert_eq!(pdf.len(), 8);
    assert_eq!(epub, pdf);
    assert!(
        book_text(&output).contains("An unsaved sentence."),
        "a book reads the open buffer too"
    );

    let (pdf, epub, _) = both(
        &mut engine,
        folder,
        ExportTarget::File(page(&root, "admin/pages/backup.adoc")),
    );
    assert_eq!(epub, pdf);

    let (pdf, epub, output) = both(
        &mut engine,
        folder,
        ExportTarget::Navigation(vec![
            page(&root, "admin/nav.adoc"),
            page(&root, "ROOT/nav.adoc"),
        ]),
    );
    assert_eq!(pdf.first(), Some(&page(&root, "ROOT/pages/index.adoc")));
    assert_eq!(epub, pdf);
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

/// PRD §11 test 6: a failed export, a link, a wrong extension, a folder and a folder that
/// refuses the write each leave the existing file as it was.
#[test]
fn epub_failed_export_keeps_existing_output() {
    let (directory, root, mut engine) = book_workspace();
    let folder = directory.path();
    let existing = folder.join("existing.epub");
    fs::write(&existing, b"the previous book").unwrap();
    let page_target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));

    // A target that does not exist.
    let error = engine
        .export_epub_blocking(
            epub(
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

    // A time an EPUB cannot be dated with fails the export, leaving the book.
    let summary = engine.export_epub_blocking(
        EpubExportArgs {
            modified: Some(0),
            ..epub(page_target.clone(), &existing)
        },
        WAIT,
    );
    assert!(summary.is_err(), "{summary:?}");
    assert!(
        format!("{summary:?}").contains("1980 to 2107"),
        "{summary:?}"
    );
    assert_eq!(contents(&existing).unwrap(), b"the previous book");

    // A folder at the output.
    let folder_output = folder.join("folder.epub");
    fs::create_dir(&folder_output).unwrap();
    assert!(engine
        .export_epub_blocking(epub(page_target.clone(), &folder_output), WAIT)
        .is_err());
    assert!(folder_output.is_dir());

    #[cfg(unix)]
    {
        // A link at the output is never written through.
        let elsewhere = folder.join("elsewhere.epub");
        fs::write(&elsewhere, b"another book").unwrap();
        let link = folder.join("link.epub");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let error = engine
            .export_epub_blocking(epub(page_target.clone(), &link), WAIT)
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
        let inside = locked.join("out.epub");
        fs::write(&inside, b"the locked book").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = engine.export_epub_blocking(epub(page_target.clone(), &inside), WAIT);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "{result:?}");
        assert_eq!(contents(&inside).unwrap(), b"the locked book");
        assert_eq!(
            fs::read_dir(&locked).unwrap().count(),
            1,
            "no temporary file left"
        );
    }

    // A successful export replaces the book whole, leaving no temporary file, dated as asked.
    let summary = engine
        .export_epub_blocking(epub(page_target, &existing), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(summary.pages, 1);
    assert!(!summary.protected);
    let (_, package) = adoc_epub::testing::assert_valid_ocf(&contents(&existing).unwrap());
    assert_eq!(package.modified, "2026-11-19T10:30:00Z");
    assert!(
        summary
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("dated with the time")),
        "a given time is not reported: {:?}",
        summary.diagnostics
    );
    let temporary = fs::read_dir(folder)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp"))
        .count();
    assert_eq!(temporary, 0);
}

/// A page that asks to be read-only is refused before anything is read: an EPUB carries no
/// editing restriction (READONLY PRD).
#[test]
fn epub_refuses_a_read_only_page() {
    let (directory, root, mut engine) = book_workspace();
    let protected = page(&root, "ROOT/pages/install.adoc");
    let text = fs::read_to_string(&protected).unwrap();
    let (title, rest) = text.split_once('\n').unwrap();
    fs::write(
        &protected,
        format!("{title}\n:export-read-only: true\n{rest}"),
    )
    .unwrap();
    let id = engine.open(&protected).unwrap();
    let output = directory.path().join("install.epub");
    let refused = engine.invoke(
        ids::EXPORT_EPUB_PAGE,
        CommandArgs::ExportEpub(epub(ExportTarget::Page(id), &output)),
    );
    match refused {
        Err(EngineError::ExportProtection(refusal)) => {
            assert!(
                matches!(
                    *refusal,
                    ProtectionRefusal::UnsupportedFormat {
                        format: ExportFormat::Epub,
                        ..
                    }
                ),
                "{refusal:?}"
            );
            assert!(refusal.to_string().contains("EPUB"), "{refusal}");
        }
        other => panic!("{other:?}"),
    }
    assert!(engine.wait_for_workers(WAIT));
    assert!(!output.exists());
}

/// The reference book: the Word reference book with the Kindle fixture laid over it, and two
/// more references on its links page: one through a page alias, one to an id nothing defines.
fn reference(folder: &Path) -> (PathBuf, Engine) {
    let root = folder.join("reference");
    copy_tree(&fixture("docx/reference"), &root);
    copy_tree(&fixture("kindle/reference"), &root);
    let links = root.join("manual/modules/ROOT/pages/links.adoc");
    let mut text = fs::read_to_string(&links).unwrap();
    text.push_str(
        "\nThe extras are xref:more.adoc[reached by an alias], and \
         xref:install.adoc#nowhere[a missing place] is not.\n",
    );
    fs::write(&links, text).unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (root, engine)
}

/// PRD §11 test 11: over an Antora book with nested navigation, two components, a page alias,
/// an id two pages define, references across modules, a missing id and a reference out of the
/// book, every local link and fragment resolves, and nothing names a local file or an Antora
/// resource.
#[test]
fn epub_internal_links_resolve() {
    let directory = tempfile::tempdir().unwrap();
    let (root, mut engine) = reference(directory.path());
    let output = directory.path().join("manual.epub");
    let summary = engine
        .export_epub_blocking(
            epub(
                ExportTarget::Component {
                    name: "manual".to_owned(),
                    version: Some("2.0".to_owned()),
                },
                &output,
            ),
            WAIT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let bytes = fs::read(&output).unwrap();
    let links = adoc_epub::testing::assert_links_resolve(&bytes);

    // The chapter each page is: the one whose first heading is the page's title. Parts and
    // navigation headings are chapters too, so a page's place among the sources is not it.
    let files = adoc_epub::testing::files(&bytes);
    let chapter_of = |relative: &str| {
        let source = summary
            .sources
            .iter()
            .find(|source| source.ends_with(relative))
            .unwrap_or_else(|| panic!("{relative} in {:?}", summary.sources));
        let title = fs::read_to_string(source).unwrap();
        let title = title
            .lines()
            .next()
            .unwrap()
            .trim_start_matches("= ")
            .to_owned();
        let name = files
            .iter()
            .filter(|(name, _)| name.starts_with("EPUB/text/"))
            .find(|(name, data)| {
                let text = String::from_utf8_lossy(data);
                let document = adoc_epub::testing::parse(name, &text);
                document
                    .descendants()
                    .find(|node| node.tag_name().name() == "h1")
                    .map(|h1| {
                        h1.descendants()
                            .filter(|node| node.is_text())
                            .filter_map(|node| node.text())
                            .collect::<String>()
                    })
                    == Some(title.clone())
            })
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| panic!("no chapter is titled {title}"));
        name.trim_start_matches("EPUB/text/").to_owned()
    };
    let link = |text: &str| {
        links
            .iter()
            .find(|link| link.text == text)
            .unwrap_or_else(|| panic!("a link {text:?} in {links:?}"))
    };
    // Across modules, and through a page alias, to the pages' chapters.
    assert!(link("security")
        .href
        .starts_with(&chapter_of("admin/pages/security.adoc")));
    assert!(link("reached by an alias")
        .href
        .starts_with(&chapter_of("ROOT/pages/extras.adoc")));
    // An id two pages define: each reference leads to its own page's.
    let install = link("the requirements");
    assert!(install
        .href
        .starts_with(&chapter_of("ROOT/pages/install.adoc")));
    assert!(install.href.contains('#'));
    let extras_own = links
        .iter()
        .find(|link| {
            link.from.ends_with(&chapter_of("ROOT/pages/extras.adoc")) && link.href.starts_with('#')
        })
        .expect("the extras page's own requirements, as a fragment");
    assert_ne!(
        install.href.split_once('#').unwrap().1,
        extras_own.href.trim_start_matches('#')
    );
    // Out of the book: the web address as written; the API page and the missing id as text,
    // each with a located warning.
    assert_eq!(link("its site").href, "https://example.com/product");
    assert!(!links
        .iter()
        .any(|link| link.text == "the API reference" || link.text == "a missing place"));
    for wanted in ["leads outside the book", "names nothing in the book"] {
        assert!(
            summary
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(wanted)
                    && diagnostic
                        .location
                        .as_ref()
                        .is_some_and(|(path, _)| path.ends_with("links.adoc"))),
            "{wanted}: {:?}",
            summary.diagnostics
        );
    }
    // No file names a local file or an Antora resource.
    for (name, data) in files {
        if name.ends_with(".xhtml") || name.ends_with(".opf") {
            let text = String::from_utf8(data).unwrap();
            for scheme in ["file:", "antora:", root.to_string_lossy().as_ref()] {
                assert!(!text.contains(scheme), "{name} holds {scheme}");
            }
        }
    }
}

/// PRD §11 test 17: page, component and navigation profiles resolve to the right pages and
/// outputs, with `[epub]`'s defaults; invalid ones are refused when the settings are read, and
/// an output, cover or stylesheet outside the workspace when the profile is used.
#[test]
fn epub_profiles_resolve_and_validate() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    fs::create_dir_all(root.join("style")).unwrap();
    fs::write(
        root.join("style/reader.css"),
        "body { font-family: serif; }\n",
    )
    .unwrap();
    fs::write(
        root.join("style/remote.css"),
        "@import url(https://example.com/x.css);\n",
    )
    .unwrap();
    fs::write(
        root.join(".adocfoundry.toml"),
        r#"
[epub]
style = "default"
creator = "The Team"
language = "en-GB"
css = "style/reader.css"
modified = "2026-11-19T10:30:00Z"

[epub.profiles.page]
page = "guide/modules/ROOT/pages/install.adoc"
output = "build/install.epub"
title = "Installing"

[epub.profiles.handbook]
component = "1.0@guide"
modified = "2026-01-01"

[epub.profiles.admin]
nav = ["guide/modules/admin/nav.adoc"]
output = "build/admin.epub"
identifier = "urn:isbn:9780000000002"

[epub.profiles.away]
page = "guide/modules/ROOT/pages/install.adoc"
output = "../away.epub"

[epub.profiles.remote]
page = "guide/modules/ROOT/pages/install.adoc"
css = "style/remote.css"

[epub.profiles.outside-css]
page = "guide/modules/ROOT/pages/install.adoc"
css = "../../elsewhere.css"

[epub.profiles.wrong]
page = "guide/modules/ROOT/pages/install.adoc"
output = "wrong.azw3"

[epub.profiles.odd]
page = "guide/modules/ROOT/pages/install.adoc"
format = "kfx"

[epub.profiles.unknown]
component = "nowhere"
"#,
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    // Invalid profiles are refused when the settings are read.
    for name in ["epub.profiles.wrong", "epub.profiles.odd"] {
        assert!(
            settings
                .warnings
                .iter()
                .any(|warning| warning.contains(name)),
            "{name}: {:?}",
            settings.warnings
        );
    }
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(
        engine.epub_profiles(),
        [
            "admin",
            "away",
            "handbook",
            "outside-css",
            "page",
            "remote",
            "unknown"
        ]
    );
    let package = |path: &Path| {
        let bytes = fs::read(path).unwrap();
        let (files, package) = adoc_epub::testing::assert_valid_ocf(&bytes);
        (files, package)
    };

    // The page profile: its own title and output, `[epub]`'s creator, language, stylesheet and
    // date.
    let args = engine.epub_profile_export_args("page").unwrap();
    assert_eq!(args.output, root.join("build/install.epub"));
    assert_eq!(
        args.css, None,
        "the default stylesheet is `[epub]`'s, applied on export"
    );
    let summary = engine.export_epub_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources, [page(&root, "ROOT/pages/install.adoc")]);
    let (files, opf) = package(&root.join("build/install.epub"));
    assert_eq!(opf.title, "Installing");
    assert_eq!(opf.language, "en-GB");
    assert_eq!(opf.modified, "2026-11-19T10:30:00Z");
    assert!(
        adoc_epub::testing::text(&files, &opf.path).contains("<dc:creator>The Team</dc:creator>")
    );
    assert_eq!(
        adoc_epub::testing::text(&files, "EPUB/css/user.css"),
        "body { font-family: serif; }\n"
    );
    assert!(
        summary
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("dated with the time")),
        "a date in the settings is repeatable"
    );

    // The component profile: `<name>.epub` at the root, the whole book, its own date.
    let args = engine.epub_profile_export_args("handbook").unwrap();
    assert_eq!(args.output, root.join("handbook.epub"));
    let summary = engine.export_epub_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources.len(), 8);
    let (_, opf) = package(&root.join("handbook.epub"));
    assert_eq!(opf.modified, "2026-01-01T00:00:00Z");

    // The navigation profile: the start page as a preface, then the administration guide.
    let args = engine.epub_profile_export_args("admin").unwrap();
    let summary = engine.export_epub_blocking(args, WAIT).unwrap();
    assert_eq!(summary.sources[0], page(&root, "ROOT/pages/index.adoc"));
    let admin = root.join("guide/modules/admin");
    assert!(summary.sources[1..]
        .iter()
        .all(|source| source.starts_with(&admin)));
    let (_, opf) = package(&root.join("build/admin.epub"));
    assert_eq!(opf.identifier, "urn:isbn:9780000000002");

    // Through the command, as the app and MCP server export a profile.
    fs::remove_file(root.join("build/install.epub")).unwrap();
    engine
        .invoke(
            ids::EXPORT_EPUB_PROFILE,
            CommandArgs::Profile("page".to_owned()),
        )
        .unwrap();
    assert!(engine.wait_for_workers(WAIT));
    for _ in 0..200 {
        engine.pump();
        if root.join("build/install.epub").is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(root.join("build/install.epub").is_file());

    // Unsafe paths are refused, and a stylesheet that reaches outside the book fails the
    // export, leaving nothing behind.
    for name in ["away", "outside-css"] {
        let error = engine.epub_profile_export_args(name).unwrap_err();
        assert!(
            matches!(error, EngineError::Refused(_)),
            "{name}: {error:?}"
        );
    }
    assert!(!directory.path().join("away.epub").exists());
    let args = engine.epub_profile_export_args("remote").unwrap();
    let error = engine.export_epub_blocking(args, WAIT).unwrap_err();
    assert!(error.to_string().contains("@import"), "{error}");
    assert!(!root.join("remote.epub").exists());

    // Problems found without exporting.
    assert!(engine.epub_profile_problems("page").is_empty());
    let problems = engine.epub_profile_problems("unknown");
    assert!(
        problems.iter().any(|problem| problem.contains("nowhere")),
        "{problems:?}"
    );
    assert!(engine.epub_profile_export_args("missing").is_err());
    assert!(engine.epub_profile_export_args("wrong").is_err());
}

/// PRD §11 test 18: an EPUB profile selects the same pages, in the same order, as the
/// equivalent PDF profile, and neither format's settings change the other's output.
#[cfg(all(feature = "typst", feature = "kindle"))]
#[test]
fn epub_settings_leave_other_formats_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    let open = |settings_text: &str| {
        fs::write(root.join(".adocfoundry.toml"), settings_text).unwrap();
        let mut settings = Settings::load(None, Some(&root)).unwrap();
        settings.git.enabled = false;
        let mut engine = Engine::new(settings);
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(WAIT));
        engine
    };
    let profiles = r#"
[pdf.profiles.guide]
component = "1.0@guide"
output = "guide.pdf"

[epub.profiles.guide]
component = "1.0@guide"
output = "guide.epub"
modified = "2026-11-19"
"#;
    let mut engine = open(profiles);
    let pdf = engine
        .export_blocking(engine.profile_export_args("guide").unwrap(), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let profiled = engine
        .export_epub_blocking(engine.epub_profile_export_args("guide").unwrap(), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        profiled.sources, pdf.sources,
        "the same pages, in the same order"
    );

    // Every other format's settings, changed, leave the EPUB's bytes as they were; `[epub]`,
    // changed, leaves a Kindle book's.
    let kindle = |engine: &mut Engine| {
        let output = directory.path().join("guide.azw3");
        engine
            .export_kindle_blocking(
                adoc_engine::KindleExportArgs::new(
                    book(),
                    adoc_engine::KindleFormat::Azw3,
                    output.clone(),
                ),
                WAIT,
            )
            .unwrap_or_else(|error| panic!("{error}"));
        fs::read(output).unwrap()
    };
    let epub_bytes = |engine: &mut Engine| {
        let output = directory.path().join("guide.epub");
        engine
            .export_epub_blocking(epub(book(), &output), WAIT)
            .unwrap_or_else(|error| panic!("{error}"));
        fs::read(output).unwrap()
    };
    let (plain_epub, plain_kindle) = (epub_bytes(&mut engine), kindle(&mut engine));
    let mut others = open(&format!(
        "{profiles}\n[pdf]\ntheme = \"default\"\n\n[docx]\npaper = \"letter\"\n\n[kindle]\ntitle = \"Another Title\"\ncreator = \"Someone Else\"\n"
    ));
    assert_eq!(epub_bytes(&mut others), plain_epub);
    let mut with_epub = open(&format!(
        "{profiles}\n[epub]\ntitle = \"An EPUB Title\"\ncreator = \"An EPUB Author\"\nlanguage = \"fr\"\n"
    ));
    assert_eq!(kindle(&mut with_epub), plain_kindle);
    assert_ne!(
        epub_bytes(&mut with_epub),
        plain_epub,
        "`[epub]` changes an EPUB"
    );
}

/// The reference workspace with `[epub]` settings and a profile added to its own.
fn epub_reference(folder: &Path) -> (PathBuf, Engine) {
    let root = folder.join("reference");
    copy_tree(&fixture("docx/reference"), &root);
    copy_tree(&fixture("kindle/reference"), &root);
    let settings_file = root.join(".adocfoundry.toml");
    let mut settings_text = fs::read_to_string(&settings_file).unwrap();
    settings_text.push_str(
        "\n[epub]\npublisher = \"AdocFoundry\"\nlanguage = \"en-GB\"\nmodified = \"2026-11-19\"\n\n\
         [epub.profiles.handbook]\ncomponent = \"2.0@manual\"\noutput = \"build/handbook.epub\"\n",
    );
    fs::write(&settings_file, settings_text).unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (root, engine)
}

/// PRD §11 test 23: a versioned book of two components with nested navigation, partials,
/// footnotes, Japanese and Greek text, code, tables, a cover, PNG, JPEG and SVG pictures, a
/// diagram, formulas, a missing picture, a malformed page, page aliases and an id two pages
/// define passes the in-process validator, with every link resolving and nothing dropped; and
/// it is the same bytes from another engine in another folder, and from its profile.
#[test]
fn epub_reference_book_is_complete() {
    let directory = tempfile::tempdir().unwrap();
    let (root, mut engine) = epub_reference(&directory.path().join("a"));
    let output = directory.path().join("manual.epub");
    let target = ExportTarget::Component {
        name: "manual".to_owned(),
        version: Some("2.0".to_owned()),
    };
    let summary = engine
        .export_epub_blocking(EpubExportArgs::new(target.clone(), output.clone()), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    let relative: Vec<String> = summary
        .sources
        .iter()
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        relative,
        [
            "manual/modules/ROOT/pages/index.adoc",
            "manual/modules/ROOT/pages/install.adoc",
            "manual/modules/ROOT/pages/tables.adoc",
            "manual/modules/ROOT/pages/media.adoc",
            "manual/modules/ROOT/pages/links.adoc",
            "manual/modules/ROOT/pages/extras.adoc",
            "manual/modules/admin/pages/security.adoc",
        ]
    );
    let bytes = fs::read(&output).unwrap();
    assert_eq!(adoc_epub::validate(&bytes), Vec::<String>::new());
    adoc_epub::testing::assert_links_resolve(&bytes);
    let (files, package) = adoc_epub::testing::assert_valid_ocf(&bytes);

    // The metadata, `[epub]`'s over the page's, and the accessibility the content shows.
    assert_eq!(package.title, "The Manual");
    assert_eq!(package.language, "en-GB");
    assert_eq!(package.modified, "2026-11-19T00:00:00Z");
    let opf = adoc_epub::testing::text(&files, &package.path);
    for wanted in [
        "<dc:publisher>AdocFoundry</dc:publisher>",
        "<meta property=\"schema:accessMode\">visual</meta>",
        "<meta property=\"schema:accessibilityFeature\">structuralNavigation</meta>",
    ] {
        assert!(opf.contains(wanted), "{wanted}");
    }

    // The cover first, and the pictures: the cover and logo (PNG), the photograph (JPEG), the
    // SVG drawn as PNG, and, with Typst, the diagram and formulas drawn.
    assert_eq!(package.spine[0], "cover");
    let images: Vec<&(String, String, Option<String>)> = package
        .manifest
        .values()
        .filter(|(_, media, _)| media.starts_with("image/"))
        .collect();
    assert!(images.iter().any(|(_, media, _)| media == "image/jpeg"));
    assert!(images
        .iter()
        .any(|(_, _, properties)| properties.as_deref() == Some("cover-image")));
    assert!(images.len() >= 4, "{images:?}");

    // What the pages hold, partials included, and nothing of the other version or component.
    let mut text = String::new();
    let mut markup = String::new();
    for idref in &package.spine {
        let (path, _, _) = &package.manifest[idref];
        let source = adoc_epub::testing::text(&files, path);
        markup.push_str(source);
        let document = adoc_epub::testing::parse(path, source);
        for node in document.descendants() {
            if node.is_text() {
                text.push_str(node.text().unwrap_or_default());
            }
            if let Some(alt) = node.attribute("alt") {
                text.push_str(alt);
            }
            text.push(' ');
        }
    }
    for wanted in [
        "日本語のテキストを読む",
        "Καλημέρα",
        "A missing picture",
        "never closed",
        "The note, written once.",
        "The pipeline drawn as SVG",
    ] {
        assert!(text.contains(wanted), "{wanted}");
    }
    for absent in ["OLD-VERSION-MARKER", "API-COMPONENT-MARKER"] {
        assert!(!text.contains(absent), "{absent}");
    }
    for element in [
        "<thead>",
        "epub:type=\"noteref\"",
        "epub:type=\"footnote\"",
        "<pre class=\"listing\">",
        "class=\"admonition",
    ] {
        assert!(markup.contains(element), "{element}");
    }
    #[cfg(feature = "typst")]
    assert!(
        text.contains("A = pi r^2"),
        "the formula's source is its description"
    );
    // The missing picture is reported where it is written.
    assert!(summary.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("missing.png")
            && diagnostic
                .location
                .as_ref()
                .is_some_and(|(path, _)| path.ends_with("extras.adoc"))
    }));

    // Another engine, in another folder, writes the same bytes, and so does the profile.
    let (_, mut second) = epub_reference(&directory.path().join("b/elsewhere"));
    let moved = directory.path().join("moved.epub");
    second
        .export_epub_blocking(EpubExportArgs::new(target, moved.clone()), WAIT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        fs::read(&moved).unwrap() == bytes,
        "another engine and folder"
    );
    engine
        .export_epub_blocking(engine.epub_profile_export_args("handbook").unwrap(), WAIT)
        .unwrap();
    assert!(
        fs::read(root.join("build/handbook.epub")).unwrap() == bytes,
        "the profile"
    );
}
