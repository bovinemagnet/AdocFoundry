//! `export_docx`: Word export through the server, listed and callable only with
//! `--allow-write`, confined to the roots like every write.

mod common;

use std::path::Path;

use common::{copy_tree, fixture, Client};
use serde_json::json;

/// The DOCX export PRD, §10 test 16.
#[test]
fn docx_mcp_export_is_write_gated() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("book"));
    copy_tree(&fixture("pdf/book"), &root);
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[docx.profiles.manual]\nnav = [\"guide/modules/admin/nav.adoc\", \"guide/modules/ROOT/nav.adoc\"]\noutput = \"build/manual.docx\"\n",
    )
    .unwrap();
    let page = "guide/modules/ROOT/pages/install.adoc";

    // Without `--allow-write`: not listed, and refused when called anyway.
    let mut client = Client::start(&root, false);
    assert!(!client.tool_names().iter().any(|name| name == "export_docx"));
    let result = client.call_result("export_docx", json!({ "path": page, "output": "a.docx" }));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["_meta"]["error"]["code"], "permission_denied",
        "{result}"
    );
    assert!(!root.join("a.docx").exists());
    client.stop();

    let mut client = Client::start(&root, true);
    assert!(client.tool_names().iter().any(|name| name == "export_docx"));

    // A page as an article.
    let exported = client
        .call(
            "export_docx",
            json!({ "path": page, "output": "install.docx" }),
        )
        .unwrap();
    assert_eq!(exported["output"], "install.docx", "{exported}");
    assert_eq!(exported["pages"], 1, "{exported}");
    assert_eq!(exported["sources"], json!([page]), "{exported}");
    assert!(is_docx(&root.join("install.docx")));

    // A component version as a book, its pages in navigation order.
    let exported = client
        .call(
            "export_docx",
            json!({ "component": "1.0@guide", "output": "guide.docx" }),
        )
        .unwrap();
    let sources = exported["sources"].as_array().unwrap();
    assert!(sources.len() > 1, "{exported}");
    assert_eq!(exported["pages"], sources.len(), "{exported}");
    assert!(is_docx(&root.join("guide.docx")));

    // A profile writes what the engine's command writes.
    let exported = client
        .call("export_docx", json!({ "profile": "manual" }))
        .unwrap();
    assert_eq!(exported["output"], "build/manual.docx", "{exported}");
    let from_server = std::fs::read(root.join("build/manual.docx")).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(common::WAIT));
    let mut args = engine.docx_profile_export_args("manual").unwrap();
    let from_engine = directory.path().join("engine.docx");
    args.output = from_engine.clone();
    engine.export_docx_blocking(args, common::WAIT).unwrap();
    assert!(
        std::fs::read(&from_engine).unwrap() == from_server,
        "the server's document differs from the engine's"
    );

    // What is refused, and what each refusal says.
    std::fs::write(root.join("notes.txt"), "keep me").unwrap();
    let refusals = [
        (json!({ "path": page }), "invalid_argument"),
        (
            json!({ "path": page, "component": "guide", "output": "x.docx" }),
            "invalid_argument",
        ),
        (
            json!({ "profile": "manual", "path": page, "output": "x.docx" }),
            "invalid_argument",
        ),
        (json!({ "profile": "missing" }), "not_found"),
        (
            json!({ "path": page, "output": "../escaped.docx" }),
            "outside_workspace",
        ),
        (
            json!({ "path": page, "output": "page.pdf" }),
            "invalid_argument",
        ),
        (
            json!({ "path": page, "output": "notes.txt" }),
            "invalid_argument",
        ),
        (json!({ "path": page, "output": page }), "invalid_argument"),
    ];
    let before = common::snapshot(&root);
    for (arguments, code) in refusals {
        let result = client.call_result("export_docx", arguments.clone());
        assert_eq!(result["isError"], true, "{arguments}: {result}");
        assert_eq!(
            result["_meta"]["error"]["code"], code,
            "{arguments}: {result}"
        );
    }
    assert_eq!(
        common::snapshot(&root),
        before,
        "nothing written or replaced"
    );
    assert!(!directory.path().join("escaped.docx").exists());

    // A link is never written through, whether it leads outside the roots or stays inside.
    #[cfg(unix)]
    {
        let outside = directory.path().join("linked.docx");
        std::os::unix::fs::symlink(&outside, root.join("out.docx")).unwrap();
        std::os::unix::fs::symlink(root.join("install.docx"), root.join("inside.docx")).unwrap();
        let installed = std::fs::read(root.join("install.docx")).unwrap();
        for output in ["out.docx", "inside.docx"] {
            let result =
                client.call_result("export_docx", json!({ "path": page, "output": output }));
            assert_eq!(result["isError"], true, "{output}: {result}");
        }
        assert!(!outside.exists());
        assert_eq!(std::fs::read(root.join("install.docx")).unwrap(), installed);
    }
    client.stop();
}

fn is_docx(path: &Path) -> bool {
    std::fs::read(path).unwrap().starts_with(b"PK\x03\x04")
}

/// The Word looks PRD's §9 test 10, for the MCP server: `theme`, `template` and `no_theme`
/// write the same bytes as the engine exporting with the same `LookChoice` (the command line's
/// and the app's tests hold theirs to it too); the answer names the look; a theme or template
/// outside the roots is refused, however it is named.
#[test]
fn docx_look_choice_everywhere() {
    use adoc_engine::{DocxExportArgs, Engine, ExportTarget, LookChoice, Settings};

    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("book"));
    copy_tree(&fixture("pdf/book"), &root);
    std::fs::create_dir_all(root.join("themes")).unwrap();
    let sans = "extends: default-sans\nbase:\n  font-color: #112233\n";
    std::fs::write(root.join("themes/sans-theme.yml"), sans).unwrap();
    let template = adoc_docx::testing::template(|_| {});
    std::fs::write(root.join("themes/corporate.dotx"), &template).unwrap();
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf]\nthemes_dir = \"themes\"\n",
    )
    .unwrap();
    // The same files outside the roots, reached directly and through links inside them.
    std::fs::write(directory.path().join("outside-theme.yml"), sans).unwrap();
    std::fs::write(directory.path().join("outside.dotx"), &template).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            directory.path().join("outside-theme.yml"),
            root.join("linked-theme.yml"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("outside.dotx"),
            root.join("linked.dotx"),
        )
        .unwrap();
        // The same, in the themes folder, where a bare name finds them.
        std::os::unix::fs::symlink(
            directory.path().join("outside-theme.yml"),
            root.join("themes/linked-theme.yml"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("outside.dotx"),
            root.join("themes/linked.dotx"),
        )
        .unwrap();
    }
    let page = "guide/modules/ROOT/pages/install.adoc";

    // The reference: the engine itself, with no user settings, as the server is started here.
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(60)));
    let id = engine.open(&root.join(page)).unwrap();
    let mut reference = |look: LookChoice, name: &str| {
        let output = directory.path().join(name);
        engine
            .export_docx_blocking(
                DocxExportArgs {
                    target: ExportTarget::Page(id),
                    output: output.clone(),
                    attributes: Vec::new(),
                    paper: None,
                    landscape: None,
                    look: Some(look),
                },
                std::time::Duration::from_secs(120),
            )
            .unwrap();
        std::fs::read(output).unwrap()
    };
    let themed = reference(LookChoice::PdfTheme("sans".to_owned()), "themed.docx");
    let templated = reference(
        LookChoice::Template("corporate".to_owned()),
        "templated.docx",
    );
    let plain = reference(LookChoice::None, "plain.docx");

    let home = tempfile::tempdir().unwrap();
    let env: [(&str, Option<&std::ffi::OsStr>); 2] = [
        ("HOME", Some(home.path().as_os_str())),
        ("XDG_CONFIG_HOME", Some(home.path().as_os_str())),
    ];
    let (mut client, _) = Client::start_with_env(&root, &["--allow-write"], "2025-06-18", &env);
    for (look, expected, kind, name) in [
        (json!({ "theme": "sans" }), &themed, "pdf-theme", "sans"),
        (
            json!({ "theme": "themes/sans-theme.yml" }),
            &themed,
            "pdf-theme",
            "themes/sans-theme.yml",
        ),
        (
            json!({ "template": "corporate" }),
            &templated,
            "template",
            "corporate",
        ),
        (json!({ "no_theme": true }), &plain, "none", "none"),
    ] {
        let mut arguments = json!({ "path": page, "output": "mcp.docx" });
        for (key, value) in look.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        let exported = client.call("export_docx", arguments.clone()).unwrap();
        assert_eq!(exported["look"]["kind"], kind, "{arguments}: {exported}");
        assert_eq!(exported["look"]["name"], name, "{arguments}: {exported}");
        assert!(
            std::fs::read(root.join("mcp.docx")).unwrap() == *expected,
            "{arguments} wrote other bytes than the engine"
        );
    }
    let exported = client
        .call(
            "export_docx",
            json!({ "path": page, "output": "mcp.docx", "template": "corporate" }),
        )
        .unwrap();
    assert_eq!(
        exported["look"]["path"], "themes/corporate.dotx",
        "{exported}"
    );

    // At most one look.
    let refused = client.call(
        "export_docx",
        json!({ "path": page, "output": "mcp.docx", "theme": "sans", "no_theme": true }),
    );
    assert!(refused.is_err_and(|error| error.contains("at most one")));

    // Outside the roots: refused, and nothing written.
    let outside_theme = directory
        .path()
        .join("outside-theme.yml")
        .to_string_lossy()
        .into_owned();
    let outside_template = directory
        .path()
        .join("outside.dotx")
        .to_string_lossy()
        .into_owned();
    let mut attempts = vec![
        json!({ "theme": outside_theme }),
        json!({ "theme": "../outside-theme.yml" }),
        json!({ "template": outside_template }),
        json!({ "template": "../outside.dotx" }),
    ];
    if cfg!(unix) {
        attempts.push(json!({ "theme": "linked-theme.yml" }));
        attempts.push(json!({ "template": "linked.dotx" }));
        // By name: the server passes a name on, and the engine refuses the link.
        attempts.push(json!({ "theme": "linked" }));
        attempts.push(json!({ "template": "linked" }));
    }
    for look in attempts {
        let mut arguments = json!({ "path": page, "output": "outside.docx" });
        for (key, value) in look.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        let refused = client.call("export_docx", arguments.clone());
        assert!(refused.is_err(), "{arguments} was exported: {refused:?}");
        assert!(!root.join("outside.docx").exists(), "{arguments}");
    }
    client.stop();
}
