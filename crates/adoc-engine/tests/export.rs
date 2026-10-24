//! WP 4.5: the engine's PDF export, of a page and of a component version as a book. The
//! generated Typst is written beside each PDF and checked for the structure it gives the book.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_diagrams::{DiagramCache, Diagrams, MermaidProvider};
use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineEvent, ExportArgs, ExportSummary, ExportTarget,
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

/// The book fixture copied to a temporary directory and indexed, with Mermaid built in, and
/// `workspace_settings` as the workspace's `.adocfoundry.toml`.
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

/// Exports `target` and returns the summary and the generated Typst.
fn export(engine: &mut Engine, directory: &Path, target: ExportTarget) -> (ExportSummary, String) {
    let output = directory.join("out.pdf");
    let typst = directory.join("out.typ");
    let summary = engine
        .export_blocking(
            ExportArgs {
                target,
                output: output.clone(),
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: Some(typst.clone()),
            },
            WAIT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(fs::read(&output).unwrap().starts_with(b"%PDF-"));
    (summary, fs::read_to_string(typst).unwrap())
}

fn book() -> ExportTarget {
    ExportTarget::Component {
        name: "guide".to_owned(),
        version: Some("1.0".to_owned()),
    }
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

#[test]
fn exports_one_page() {
    let (directory, root, mut engine) = book_workspace("");
    let id = engine
        .open(&root.join("guide/modules/ROOT/pages/install.adoc"))
        .unwrap();
    let (summary, typst) = export(&mut engine, directory.path(), ExportTarget::Page(id));
    assert_eq!(summary.pages, 1);
    assert!(typst.contains("#adoc-doctitle[#\"Install\"]"));
    assert!(
        typst.contains("#heading(level: 2, outlined: true, bookmarked: true)[#\"Requirements\"]")
    );
}

#[test]
fn exports_a_component_in_nav_order() {
    let (directory, _root, mut engine) = book_workspace("");
    let (summary, typst) = export(&mut engine, directory.path(), book());
    assert!(summary.pages >= 8, "{} pages", summary.pages);
    // The start page leads as a preface, then the two nav files as parts, in order.
    let order = positions(
        &typst,
        &[
            "#adoc-title-page(title: [#\"The Guide\"]",
            "#adoc-toc(min-level: 1)",
            "[#\"Welcome\"]",
            "#adoc-part[#\"Getting Started\"]",
            "[#\"Install\"]",
            "[#\"Configure\"]",
            "[#\"Use\"]",
            "#adoc-part[#\"Administration\"]",
            // A title that links to a page leads its part with that page.
            "[#\"Administering the Guide\"]",
            "[#\"Backup\"]",
            "[#\"Recovery\"]",
            "[#\"Restore\"]",
            // A page with no title takes its entry's text.
            "[#\"Empty Page\"]",
        ],
    );
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    assert!(typst.contains("revision: [#\"Version 1.0\"]"));
}

#[test]
fn nav_depth_shifts_section_levels() {
    let (directory, _root, mut engine) = book_workspace("");
    let (_, typst) = export(&mut engine, directory.path(), book());
    // A chapter is a level 2 Typst heading (h2), a page beneath it a level 3, and each
    // page's own sections move down by its depth.
    assert!(typst.contains("#heading(level: 2, outlined: true, bookmarked: true)[#\"Install\"]"));
    assert!(typst.contains("#heading(level: 3, outlined: true, bookmarked: true)[#\"Configure\"]"));
    assert!(typst.contains("[#\"Requirements\"]"));
    let requirements = typst.find("[#\"Requirements\"]").unwrap();
    let line_start = typst[..requirements].rfind('\n').unwrap();
    assert!(typst[line_start..requirements].contains("#heading(level: 3"));
    let options = typst.find("[#\"Options\"]").unwrap();
    let line_start = typst[..options].rfind('\n').unwrap();
    assert!(typst[line_start..options].contains("#heading(level: 4"));
}

#[test]
fn xrefs_between_book_pages_are_internal() {
    let (directory, _root, mut engine) = book_workspace("");
    let (_, typst) = export(&mut engine, directory.path(), book());
    // Install refers to Configure's `_options` section; Use refers to Install's page.
    assert!(
        typst.contains("#adoc-link(label(\"p3#_options\"), [#\"the options\"])"),
        "{typst}"
    );
    assert!(typst.contains("#adoc-link(label(\"p2\"), [#\"Install\"])"));
}

#[test]
fn xrefs_out_of_the_book_use_the_site_url() {
    let (directory, _root, mut engine) =
        book_workspace("[pdf]\nsite_url = \"https://docs.example.com/\"\n");
    let (_, typst) = export(&mut engine, directory.path(), book());
    assert!(
        typst.contains(
            "#adoc-link(\"https://docs.example.com/other/index.html\", [#\"other component\"])"
        ),
        "{typst}"
    );
    // Without a site, the reference is text.
    let (directory, _root, mut engine) = book_workspace("");
    let (_, typst) = export(&mut engine, directory.path(), book());
    assert!(typst.contains("#\"other component\""));
    assert!(!typst.contains("other/index.html"));
}

#[test]
fn a_page_s_xrefs_to_other_pages_keep_their_coordinates() {
    // A page exported alone links every page it names to its place on the site, with the
    // component, version and module, as a book links the pages it does not hold.
    let expected = [
        "#adoc-link(\"https://docs.example.com/guide/1.0/configure.html#_options\", [#\"the options\"])",
        "#adoc-link(\"https://docs.example.com/other/index.html\", [#\"other component\"])",
    ];
    let (directory, root, mut engine) =
        book_workspace("[pdf]\nsite_url = \"https://docs.example.com/\"\n");
    let page = engine
        .open(&root.join("guide/modules/ROOT/pages/install.adoc"))
        .unwrap();
    let (_, typst) = export(&mut engine, directory.path(), ExportTarget::Page(page));
    for link in expected {
        assert!(typst.contains(link), "{link} in {typst}");
    }

    // The playbook's `site.url` serves when the settings name no site.
    let (directory, root, _) = book_workspace("");
    fs::write(
        root.join("antora-playbook.yml"),
        "site:\n  title: Docs\n  url: https://docs.example.com\n",
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let page = engine
        .open(&root.join("guide/modules/ROOT/pages/install.adoc"))
        .unwrap();
    let (_, typst) = export(&mut engine, directory.path(), ExportTarget::Page(page));
    for link in expected {
        assert!(typst.contains(link), "{link} in {typst}");
    }
}

#[test]
fn diagrams_render_before_compiling() {
    let (directory, _root, mut engine) = book_workspace("");
    let (summary, typst) = export(&mut engine, directory.path(), book());
    assert!(
        typst.contains("#adoc-image(image(\"/files/"),
        "the diagram is an image"
    );
    assert!(
        !typst.contains("graph LR"),
        "the diagram's source is not shown"
    );
    assert!(
        summary
            .diagnostics
            .iter()
            .all(|d| !d.message.contains("diagram")),
        "{:?}",
        summary.diagnostics
    );
}

#[test]
fn export_runs_off_the_engine_thread() {
    let (directory, root, mut engine) = book_workspace("");
    let events = engine.subscribe();
    let id = engine
        .open(&root.join("guide/modules/ROOT/pages/use.adoc"))
        .unwrap();
    let output = directory.path().join("async.pdf");
    engine
        .invoke(
            ids::EXPORT_PDF_PAGE,
            CommandArgs::Export(ExportArgs {
                target: ExportTarget::Page(id),
                output: output.clone(),
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
            }),
        )
        .unwrap();
    // The command returns at once; the export finishes on workers.
    assert!(engine.is_exporting());
    assert!(engine.wait_for_workers(WAIT));
    assert!(!engine.is_exporting());
    let finished = events.try_iter().find_map(|event| match event {
        EngineEvent::ExportFinished { output, result } => Some((output, result)),
        _ => None,
    });
    let (written, result) = finished.expect("an ExportFinished event");
    assert_eq!(written, output);
    assert_eq!(result.unwrap().pages, 1);
    assert!(output.is_file());
}

#[test]
fn unsaved_buffers_are_exported() {
    let (directory, root, mut engine) = book_workspace("");
    let page = root.join("guide/modules/ROOT/pages/use.adoc");
    let id = engine.open(&page).unwrap();
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
    let (_, typst) = export(&mut engine, directory.path(), ExportTarget::Page(id));
    assert!(typst.contains("An unsaved sentence."));
    assert!(!fs::read_to_string(&page).unwrap().contains("unsaved"));
    // A book reads the open buffer too.
    let (_, typst) = export(&mut engine, directory.path(), book());
    assert!(typst.contains("An unsaved sentence."));
}

#[test]
fn theme_setting_selects_the_theme() {
    let (directory, root, mut engine) = book_workspace("[pdf]\ntheme = \"default-sans\"\n");
    let id = engine
        .open(&root.join("guide/modules/ROOT/pages/use.adoc"))
        .unwrap();
    let (_, typst) = export(&mut engine, directory.path(), ExportTarget::Page(id));
    assert!(typst.contains("\"base_font_family\": \"Noto Sans\""));
    // The export's own theme wins over the setting.
    let summary = engine
        .export_blocking(
            ExportArgs {
                target: ExportTarget::Page(id),
                output: directory.path().join("print.pdf"),
                theme: Some("default-for-print".to_owned()),
                standards: Vec::new(),
                fonts_dir: None,
                typst: Some(directory.path().join("print.typ")),
            },
            WAIT,
        )
        .unwrap();
    assert_eq!(summary.pages, 1);
    let typst = fs::read_to_string(directory.path().join("print.typ")).unwrap();
    assert!(typst.contains("\"link_font_color\": none"));
    // A theme that does not exist is an error, not a PDF.
    let error = engine
        .export_blocking(
            ExportArgs {
                target: ExportTarget::Page(id),
                output: directory.path().join("missing.pdf"),
                theme: Some("nowhere".to_owned()),
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
            },
            WAIT,
        )
        .unwrap_err();
    assert!(error.to_string().contains("nowhere"), "{error}");
}

/// The book written to `ADOC_PDF_OUT` for inspection.
#[test]
#[ignore = "writes files for inspection; needs ADOC_PDF_OUT"]
fn writes_the_book() {
    let out = PathBuf::from(std::env::var("ADOC_PDF_OUT").expect("ADOC_PDF_OUT"));
    let (directory, _root, mut engine) = book_workspace("");
    let (summary, _) = export(&mut engine, directory.path(), book());
    fs::copy(directory.path().join("out.pdf"), out.join("book.pdf")).unwrap();
    fs::copy(directory.path().join("out.typ"), out.join("book.typ")).unwrap();
    eprintln!("{} pages, {:?}", summary.pages, summary.diagnostics);
}

#[test]
fn pages_the_parser_cannot_read_are_reported() {
    let (directory, root, mut engine) = book_workspace("");
    let page = root.join("guide/modules/ROOT/pages/unreadable.adoc");
    // A line range acdc cannot read makes it reject the page.
    fs::write(&page, "= Unreadable\n\ninclude::install.adoc[lines=abc]\n").unwrap();
    let id = engine.open(&page).unwrap();
    let (summary, _) = export(&mut engine, directory.path(), ExportTarget::Page(id));
    let reported = summary.diagnostics.iter().find(|diagnostic| {
        diagnostic
            .location
            .as_ref()
            .is_some_and(|(path, _)| path == &page)
    });
    let reported = reported.unwrap_or_else(|| panic!("{:?}", summary.diagnostics));
    assert!(
        reported.message.contains("shown as its source"),
        "{reported}"
    );
}

#[test]
fn unconvertible_stem_is_reported() {
    let (directory, root, mut engine) = book_workspace("");
    let page = root.join("guide/modules/ROOT/pages/maths.adoc");
    fs::write(
        &page,
        "= Maths\n\nA typeset formula: latexmath:[\\frac{a}{b}].\n\n[latexmath]\n++++\n\\href{https://example.com}{x}\n++++\n",
    )
    .unwrap();
    let id = engine.open(&page).unwrap();
    let (summary, typst) = export(&mut engine, directory.path(), ExportTarget::Page(id));
    assert!(typst.contains("#adoc-stem(\"frac(a ,b )\""), "{typst}");
    let reported = summary
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.starts_with("STEM not typeset"))
        .unwrap_or_else(|| panic!("{:?}", summary.diagnostics));
    assert_eq!(
        reported
            .location
            .as_ref()
            .map(|(path, line)| (path.as_path(), *line)),
        Some((page.as_path(), 5))
    );
}
