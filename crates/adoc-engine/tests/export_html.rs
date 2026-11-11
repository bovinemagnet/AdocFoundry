//! Export Page to HTML: what `adocfoundry-cli render` writes, with images embedded.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use adoc_engine::{
    commands::ids, CommandArgs, Engine, EngineEvent, ExportArgs, ExportSummary, ExportTarget,
    Settings,
};

const PAGE: &str = "\
= Exported Page

A paragraph with *strong* text.

image::pixel.png[A pixel]

[source,rust]
----
fn main() {}
----
";

/// A one-pixel PNG.
const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn export(
    engine: &mut Engine,
    events: &Receiver<EngineEvent>,
    id: adoc_engine::DocumentId,
    output: &Path,
) -> (PathBuf, Result<ExportSummary, String>) {
    engine
        .invoke(
            ids::EXPORT_HTML_PAGE,
            CommandArgs::Export(ExportArgs {
                target: ExportTarget::Page(id),
                output: output.to_path_buf(),
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
                attributes: Vec::new(),
            }),
        )
        .unwrap();
    // As a front end's event loop would: pump until the export reports.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        engine.wait_for_workers(Duration::from_millis(50));
        engine.pump();
        if let Some(finished) = events.try_iter().find_map(|event| match event {
            EngineEvent::ExportFinished { output, result } => Some((output, result)),
            _ => None,
        }) {
            return finished;
        }
    }
    panic!("the export did not finish");
}

#[test]
fn a_page_exports_as_the_cli_renders_it_with_images_embedded() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, PAGE).unwrap();
    std::fs::write(directory.path().join("pixel.png"), PIXEL).unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(&page).unwrap();
    let events = engine.subscribe();
    // What `adocfoundry-cli render` writes for the same page.
    let rendered = engine.render_blocking(id, Duration::from_secs(10)).unwrap();
    let cli = adoc_render::to_html(&rendered.view, &engine.html_options(true));
    assert!(cli.contains("src=\"file://"), "{cli}");

    let output = directory.path().join("out/page.html");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    let (written, result) = export(&mut engine, &events, id, &output);
    assert_eq!(written, output);
    let summary = result.unwrap();
    assert_eq!(summary.output, output);

    let html = std::fs::read_to_string(&output).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    assert!(html.contains("<strong>strong</strong>"), "{html}");
    assert!(html.contains("src=\"data:image/png;base64,"), "{html}");
    assert!(
        !html.contains("file://"),
        "nothing points back at the workspace"
    );
    assert_eq!(
        html,
        adoc_render::embed_local_files(&cli),
        "the CLI's page, embedded"
    );
}

#[test]
fn unsaved_edits_are_exported() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Saved\n").unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(8, 8),
                text: "\nTyped but not saved.\n".to_owned(),
            },
        )
        .unwrap();
    let events = engine.subscribe();
    let output = directory.path().join("page.html");
    let (_, result) = export(&mut engine, &events, id, &output);
    result.unwrap();
    assert!(std::fs::read_to_string(&output)
        .unwrap()
        .contains("Typed but not saved."));
}

#[test]
fn a_failed_write_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Page\n").unwrap();
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    let events = engine.subscribe();
    let output = directory.path().join("missing-folder/page.html");
    let (_, result) = export(&mut engine, &events, id, &output);
    let error = result.unwrap_err();
    assert!(error.contains("missing-folder"), "{error}");
}

/// An Antora workspace whose page links to a page in another module, with a playbook giving the
/// site's address when `site` is set.
fn linked_workspace(directory: &Path, site: Option<&str>) -> PathBuf {
    let write = |path: &str, text: &str| {
        let path = directory.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("docs/antora.yml", "name: guide\nversion: '1.0'\n");
    write(
        "docs/modules/ROOT/pages/index.adoc",
        "= Index\n\nSee xref:security:authentication.adoc#tokens[Auth].\n",
    );
    write(
        "docs/modules/security/pages/authentication.adoc",
        "= Authentication\n\n[#tokens]\n== Tokens\n",
    );
    if let Some(site) = site {
        write(
            "antora-playbook.yml",
            &format!("site:\n  title: Docs\n  url: {site}\ncontent:\n  sources:\n  - url: .\n    start_path: docs\n"),
        );
    }
    adoc_engine::normalize_path(&directory.join("docs/modules/ROOT/pages/index.adoc"))
}

fn exported_html(directory: &Path, page: &Path) -> String {
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine
        .open_workspace(vec![adoc_engine::normalize_path(directory)])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    let id = engine.open(page).unwrap();
    let events = engine.subscribe();
    let output = directory.join("out.html");
    let (_, result) = export(&mut engine, &events, id, &output);
    result.unwrap();
    std::fs::read_to_string(output).unwrap()
}

#[test]
fn a_cross_module_xref_leads_to_the_page_s_export_not_a_resource_id() {
    let directory = tempfile::tempdir().unwrap();
    let page = linked_workspace(directory.path(), None);
    let html = exported_html(directory.path(), &page);
    assert!(
        html.contains("<a href=\"../../security/pages/authentication.html#tokens\">Auth</a>"),
        "{html}"
    );
    assert!(!html.contains("security:authentication"), "{html}");
}

#[test]
fn with_the_site_s_address_a_cross_module_xref_leads_to_the_published_page() {
    let directory = tempfile::tempdir().unwrap();
    let page = linked_workspace(directory.path(), Some("https://docs.example.com"));
    let html = exported_html(directory.path(), &page);
    assert!(
        html.contains(
            "<a href=\"https://docs.example.com/guide/1.0/security/authentication.html#tokens\">Auth</a>"
        ),
        "{html}"
    );
}
